mod model;

pub use model::Error;

use async_trait::async_trait;
use futures_channel::{mpsc, oneshot};
use http::{Method, Request, Uri};
use itertools::Itertools;
use opentelemetry::global::GlobalTracerProvider;
use opentelemetry::sdk::export::trace;
use opentelemetry::sdk::export::trace::SpanData;
use opentelemetry::sdk::resource::ResourceDetector;
use opentelemetry::sdk::resource::SdkProvidedResourceDetector;
use opentelemetry::sdk::trace::Config;
use opentelemetry::sdk::trace::Span;
use opentelemetry::sdk::trace::SpanProcessor;
use opentelemetry::sdk::Resource;
use opentelemetry::trace::{SpanId, TraceResult};
use opentelemetry::trace::{StatusCode, TraceError};
use opentelemetry::Key;
use opentelemetry::{global, sdk, trace::TracerProvider, KeyValue};
use opentelemetry_http::HttpClient;
use opentelemetry_semantic_conventions as semcov;
use prost::Message;
use std::collections::BTreeMap;
use std::convert::TryInto;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};
use worker::Context;

use crate::dd_proto;

const DEFAULT_SITE_ENDPOINT: &str = "https://trace.agent.datadoghq.eu/";
const DEFAULT_DD_TRACES_PATH: &str = "api/v0.2/traces";
const DEFAULT_DD_CONTENT_TYPE: &str = "application/x-protobuf";
const DEFAULT_DD_API_KEY_HEADER: &str = "DD-Api-Key";

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Datadog span exporter
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct DatadogExporter {
    client: Box<dyn HttpClient>,
    request_url: Uri,
    service_name: String,
    env: String,
    tags: BTreeMap<String, String>,
    host_name: String,
    key: String,
    runtime_id: String,
    container_id: String,
    app_version: String,
}

impl DatadogExporter {
    fn new(
        service_name: String,
        request_url: Uri,
        client: Box<dyn HttpClient>,
        key: String,
        env: String,
        tags: BTreeMap<String, String>,
        host_name: String,
        runtime_id: String,
        container_id: String,
        app_version: String,
    ) -> Self {
        DatadogExporter {
            client,
            request_url,
            service_name,
            key,
            tags,
            env,
            host_name,
            runtime_id,
            container_id,
            app_version,
        }
    }
}

/// Create a new Datadog exporter pipeline builder.
#[must_use]
pub fn new_pipeline() -> DatadogPipelineBuilder {
    DatadogPipelineBuilder::default()
}

/// Builder for `ExporterConfig` struct.
#[derive(Debug)]
pub struct DatadogPipelineBuilder {
    service_name: Option<String>,
    agent_endpoint: String,
    api_key: Option<String>,
    trace_config: Option<sdk::trace::Config>,
    client: Option<Box<dyn HttpClient>>,
    env: Option<String>,
    tags: Option<BTreeMap<String, String>>,
    host_name: Option<String>,
    runtime_id: Option<String>,
    container_id: Option<String>,
    app_version: Option<String>,
}

impl Default for DatadogPipelineBuilder {
    fn default() -> Self {
        DatadogPipelineBuilder {
            service_name: None,
            agent_endpoint: DEFAULT_SITE_ENDPOINT.to_string(),
            trace_config: None,
            api_key: None,
            #[cfg(not(feature = "surf-client"))]
            client: None,
            #[cfg(feature = "surf-client")]
            client: Some(Box::new(surf::Client::new())),
            env: None,
            tags: None,
            host_name: None,
            runtime_id: None,
            container_id: None,
            app_version: None,
        }
    }
}

/// A [`SpanProcessor`] that exports asynchronously when asked to do it.
///
/// Will only work in a Cloudflare worker context, at the end of you applicaiton, just before
/// sending the Response to the visitor, flush your provider.
#[derive(Debug)]
pub struct WASMWorkerSpanProcessor {
    sender: RwLock<mpsc::Sender<Option<SpanData>>>,
    flush: RwLock<Option<oneshot::Sender<()>>>,
}

impl WASMWorkerSpanProcessor {
    pub(crate) fn new(mut exporter: Box<dyn trace::SpanExporter>, ctx: &Context) -> Self {
        let (span_tx, mut span_rx) = mpsc::channel::<Option<SpanData>>(256);
        let (flush_tx, flush_rx) = oneshot::channel();

        ctx.wait_until(async move {
            if flush_rx.await.is_ok() {
                let mut acc = Vec::with_capacity(64);
                while let Ok(Some(Some(span))) = span_rx.try_next() {
                    acc.push(span);
                }

                if let Err(err) = exporter.export(acc).await {
                    global::handle_error(err);
                }
            }
        });

        WASMWorkerSpanProcessor {
            sender: RwLock::new(span_tx),
            flush: RwLock::new(Some(flush_tx)),
        }
    }
}

impl SpanProcessor for WASMWorkerSpanProcessor {
    fn on_start(&self, _span: &mut Span, _cx: &opentelemetry::Context) {
        // Ignored
    }

    fn on_end(&self, span: SpanData) {
        let mut lock = match self.sender.write() {
            Ok(a) => a,
            Err(err) => {
                global::handle_error(TraceError::from(format!("error processing span {:?}", err)));
                return;
            }
        };

        if let Err(err) = lock.try_send(Some(span)) {
            global::handle_error(TraceError::from(format!("error processing span {:?}", err)));
        }
    }

    fn force_flush(&self) -> TraceResult<()> {
        // Need to be called as we are in a worker context, we don't want to send the spans too
        // early, we should wait for the main application to be finished before doing this.
        let mut lock = match self.flush.write() {
            Ok(a) => a,
            Err(err) => {
                global::handle_error(TraceError::from(format!("error processing span {:?}", err)));
                return Err(TraceError::from(format!("error processing span {:?}", err)));
            }
        };

        if let Some(sender) = lock.take() {
            let _ = sender.send(());
        }

        Ok(())
    }

    fn shutdown(&mut self) -> TraceResult<()> {
        // We ignore the Shutdown as we are in a Worker process, either it'll be shutdown by the
        // worker termination or it'll keep existing.
        //
        // TODO: Better handle it later.
        Ok(())
    }
}

impl DatadogPipelineBuilder {
    /// Building a new exporter.
    ///
    /// This is useful if you are manually constructing a pipeline.
    ///
    /// # Errors
    ///
    /// If the Endpoint or the `APIKey` are not properly set.
    pub fn build_exporter(mut self) -> Result<DatadogExporter, TraceError> {
        let (_, service_name) = self.build_config_and_service_name();
        self.build_exporter_with_service_name(service_name)
    }

    fn build_config_and_service_name(&mut self) -> (Config, String) {
        let service_name = self.service_name.take();
        if let Some(service_name) = service_name {
            let config = if let Some(mut cfg) = self.trace_config.take() {
                cfg.resource = cfg.resource.map(|r| {
                    let without_service_name = r
                        .iter()
                        .filter(|(k, _v)| **k != semcov::resource::SERVICE_NAME)
                        .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
                        .collect::<Vec<KeyValue>>();
                    Arc::new(Resource::new(without_service_name))
                });
                cfg
            } else {
                Config {
                    resource: Some(Arc::new(Resource::empty())),
                    ..Default::default()
                }
            };
            (config, service_name)
        } else {
            let service_name = SdkProvidedResourceDetector
                .detect(Duration::from_secs(0))
                .get(semcov::resource::SERVICE_NAME)
                .unwrap()
                .to_string();
            (
                Config {
                    // use a empty resource to prevent TracerProvider to assign a service name.
                    resource: Some(Arc::new(Resource::empty())),
                    ..Default::default()
                },
                service_name,
            )
        }
    }

    fn build_exporter_with_service_name(
        self,
        service_name: String,
    ) -> Result<DatadogExporter, TraceError> {
        if let Some(client) = self.client {
            let endpoint = self.agent_endpoint + DEFAULT_DD_TRACES_PATH;
            let exporter = DatadogExporter::new(
                service_name,
                endpoint.parse().map_err::<Error, _>(Into::into)?,
                client,
                self.api_key
                    .ok_or_else(|| TraceError::Other("APIKey not provied".into()))?,
                self.env.unwrap_or_default(),
                self.tags.unwrap_or_default(),
                self.host_name.unwrap_or_default(),
                self.runtime_id.unwrap_or_default(),
                self.container_id.unwrap_or_default(),
                self.app_version.unwrap_or_default(),
            );
            Ok(exporter)
        } else {
            Err(Error::NoHttpClient.into())
        }
    }

    /// Install the Datadog worker trace exporter pipeline using a simple span processor.
    ///
    /// # Errors
    ///
    /// If the Endpoint or the `APIKey` are not properly set.
    pub fn install(
        mut self,
        ctx: &Context,
    ) -> Result<(sdk::trace::Tracer, GlobalTracerProvider), TraceError> {
        let (config, service_name) = self.build_config_and_service_name();
        let exporter = self.build_exporter_with_service_name(service_name)?;
        let mut provider_builder = sdk::trace::TracerProvider::builder()
            .with_span_processor(WASMWorkerSpanProcessor::new(Box::new(exporter), ctx));
        provider_builder = provider_builder.with_config(config);
        let provider = provider_builder.build();
        let tracer = provider.versioned_tracer(
            "opentelemetry-datadog-cloudflare",
            Some(env!("CARGO_PKG_VERSION")),
            None,
        );
        let p = global::set_tracer_provider(provider);
        Ok((tracer, p))
    }

    /// Assign the service name under which to group traces
    #[must_use]
    pub fn with_service_name<T: Into<String>>(mut self, name: T) -> Self {
        self.service_name = Some(name.into());
        self
    }

    /// Assign the Datadog trace endpoint
    #[must_use]
    pub fn with_endpoint<T: Into<String>>(mut self, endpoint: T) -> Self {
        self.agent_endpoint = endpoint.into();
        self
    }

    #[must_use]
    pub fn with_api_key<T: Into<String>>(mut self, key: Option<T>) -> Self {
        self.api_key = key.map(Into::into);
        self
    }

    /// Choose the http client used by uploader
    #[must_use]
    pub fn with_http_client<T: HttpClient + 'static>(
        mut self,
        client: Box<dyn HttpClient>,
    ) -> Self {
        self.client = Some(client);
        self
    }

    /// Assign the SDK trace configuration
    #[must_use]
    pub fn with_trace_config(mut self, config: sdk::trace::Config) -> Self {
        self.trace_config = Some(config);
        self
    }

    /// Assign the env
    #[must_use]
    pub fn with_env(mut self, env: String) -> Self {
        self.env = Some(env);
        self
    }

    /// Assign the host_name
    #[must_use]
    pub fn with_host_name(mut self, host_name: String) -> Self {
        self.host_name = Some(host_name);
        self
    }

    /// Assign the runtime_id
    #[must_use]
    pub fn with_runtime_id(mut self, runtime_id: String) -> Self {
        self.runtime_id = Some(runtime_id);
        self
    }

    /// Assign the container_id
    #[must_use]
    pub fn with_container_id(mut self, container_id: String) -> Self {
        self.container_id = Some(container_id);
        self
    }

    /// Assign the app_version
    #[must_use]
    pub fn with_app_version(mut self, app_version: String) -> Self {
        self.app_version = Some(app_version);
        self
    }

    /// Assign the tags
    #[must_use]
    pub fn with_tags(mut self, tags: BTreeMap<String, String>) -> Self {
        self.tags = Some(tags);
        self
    }
}

fn group_into_traces(spans: Vec<SpanData>) -> Vec<Vec<SpanData>> {
    spans
        .into_iter()
        .into_group_map_by(|span_data| span_data.span_context.trace_id())
        .into_iter()
        .map(|(_, trace)| trace)
        .collect()
}

/// Helper function whish should be rewritte, as we only need u64 for `TraceID`
pub(crate) fn u128_to_u64s(n: u128) -> [u64; 2] {
    let bytes = n.to_ne_bytes();
    let (mut high, mut low) = bytes.split_at(8);

    if cfg!(target_endian = "little") {
        std::mem::swap(&mut high, &mut low);
    }

    [
        u64::from_ne_bytes(high.try_into().unwrap()),
        u64::from_ne_bytes(low.try_into().unwrap()),
    ]
}

fn trace_into_dd_tracer_payload(exporter: &DatadogExporter, trace: SpanData) -> dd_proto::Span {
    let trace_id = trace.span_context.trace_id();
    let span_id: SpanId = trace.span_context.span_id();
    let span_id = u64::from_be_bytes(span_id.to_bytes());
    let parent_id = trace.parent_span_id;
    let parent_id = u64::from_be_bytes(parent_id.to_bytes());

    let resource = trace
        .attributes
        .get(&Key::from_static_str("code.namespace"))
        .map(std::string::ToString::to_string)
        .unwrap_or_default();
    let [t0, _t1] = u128_to_u64s(u128::from_be_bytes(trace_id.to_bytes()));

    #[allow(clippy::cast_possible_truncation)]
    let start = trace
        .start_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    #[allow(clippy::cast_possible_truncation)]
    let duration = trace
        .end_time
        .duration_since(trace.start_time)
        .unwrap()
        .as_nanos() as i64;

    let meta = trace
        .attributes
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<BTreeMap<String, String>>();

    dd_proto::Span {
        service: exporter.service_name.clone(),
        name: trace.name.to_string(),
        resource,
        r#type: "http".to_string(),
        trace_id: t0,
        span_id,
        parent_id,
        error: match trace.status_code {
            StatusCode::Unset | StatusCode::Ok => 0,
            StatusCode::Error => 1,
        },
        start,
        duration,
        meta,
        metrics: BTreeMap::new(),
        meta_struct: BTreeMap::new(),
    }
}

fn trace_into_chunk(spans: Vec<dd_proto::Span>) -> dd_proto::TraceChunk {
    dd_proto::TraceChunk {
        // This should not happen for Datadog originated traces, but in case this field is not populated
        // we default to 1 (https://github.com/DataDog/datadog-agent/blob/eac2327/pkg/trace/sampler/sampler.go#L54-L55),
        // which is what the Datadog trace-agent is doing for OTLP originated traces, as per
        // https://github.com/DataDog/datadog-agent/blob/3ea2eb4/pkg/trace/api/otlp.go#L309.
        priority: 100i32,
        origin: "lambda".to_string(),
        spans,
        tags: BTreeMap::new(),
        dropped_trace: false,
    }
}

impl DatadogExporter {
    fn trace_into_tracer(&self, chunks: Vec<dd_proto::TraceChunk>) -> dd_proto::TracerPayload {
        dd_proto::TracerPayload {
            container_id: self.container_id.clone(),
            language_name: "rust".to_string(),
            language_version: String::new(),
            tracer_version: VERSION.to_string(),
            runtime_id: self.runtime_id.clone(),
            chunks,
            app_version: self.app_version.clone(),
        }
    }

    fn trace_build(&self, tracer: Vec<dd_proto::TracerPayload>) -> dd_proto::TracePayload {
        dd_proto::TracePayload {
            host_name: self.host_name.clone(),
            env: self.env.clone(),
            traces: vec![],
            transactions: vec![],
            tracer_payloads: tracer,
            tags: self.tags.clone(),
            agent_version: VERSION.to_string(),
            target_tps: 1000f64,
            error_tps: 1000f64,
        }
    }
}

#[async_trait]
impl trace::SpanExporter for DatadogExporter {
    /// Export spans to datadog
    // TODO: Should split & batch them when it's too big, check Vector reference.
    async fn export(&mut self, batch: Vec<SpanData>) -> trace::ExportResult {
        let traces: Vec<Vec<SpanData>> = group_into_traces(batch);

        let chunks: Vec<dd_proto::TraceChunk> = traces
            .into_iter()
            .map(|spans| {
                trace_into_chunk(
                    spans
                        .into_iter()
                        .map(|trace| trace_into_dd_tracer_payload(self, trace))
                        .collect(),
                )
            })
            .collect();

        let traces = self.trace_into_tracer(chunks);

        let trace = self.trace_build(vec![traces]);
        let trace = trace.encode_to_vec();

        let req = Request::builder()
            .method(Method::POST)
            .uri(self.request_url.clone())
            .header(http::header::CONTENT_TYPE, DEFAULT_DD_CONTENT_TYPE)
            .header("X-Datadog-Reported-Languages", "rust")
            .header(DEFAULT_DD_API_KEY_HEADER, self.key.clone())
            .body(trace)
            .map_err::<Error, _>(Into::into)?;

        self.client.send(req).await?;
        Ok(())
    }
}
