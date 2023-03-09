//! # `OpenTelemetry` Datadog Exporter for Cloudflare
//!
//! An `OpenTelemetry` datadog exporter implementation for Cloudflare
//!
//! ## Quirks
//!
//! There are currently some incompatibilities between Datadog and `OpenTelemetry`, and this manifests
//! as minor quirks to this exporter.
//!
//! Firstly Datadog uses `operation_name` to describe what `OpenTracing` would call a component.
//! Or to put it another way, in `OpenTracing` the operation / span name's are relatively
//! granular and might be used to identify a specific endpoint. In datadog, however, they
//! are less granular - it is expected in Datadog that a service will have single
//! primary span name that is the root of all traces within that service, with an additional piece of
//! metadata called `resource_name` providing granularity. See [here](https://docs.datadoghq.com/tracing/guide/configuring-primary-operation/)
//!
//! The Datadog Golang API takes the approach of using a `resource.name` `OpenTelemetry` attribute to set the
//! `resource_name`. See [here](https://github.com/DataDog/dd-trace-go/blob/ecb0b805ef25b00888a2fb62d465a5aa95e7301e/ddtrace/opentracer/tracer.go#L10)
//!
//! Unfortunately, this breaks compatibility with other `OpenTelemetry` exporters which expect
//! a more granular operation name - as per the `OpenTracing` specification.
//!
//! This exporter therefore takes a different approach of naming the span with the name of the
//! tracing provider, and using the span name to set the `resource_name`. This should in most cases
//! lead to the behaviour that users expect.
//!
//! Datadog additionally has a `span_type` string that alters the rendering of the spans in the web UI.
//! This can be set as the `span.type` `OpenTelemetry` span attribute.
//!
//! For standard values see [here](https://github.com/DataDog/dd-trace-go/blob/ecb0b805ef25b00888a2fb62d465a5aa95e7301e/ddtrace/ext/app_types.go#L31)
//!
//! ## Bring your own http client
//!
//! Users can choose appropriate http clients to align with their runtime.
//!
//! Based on the feature enabled. The only client available is surf, feel free to implement
//! other http clients.
//!
//! Note that async http clients may need specific runtime otherwise it will panic. User should make
//! sure the http client is running in appropriate runime.
//!
//! Users can always use their own http clients by implementing `HttpClient` trait.
//!
pub(crate) mod dd_proto {
    include!(concat!(env!("OUT_DIR"), "/dd_trace.rs"));
}

mod exporter;

mod propagator {
    use opentelemetry::{
        propagation::{text_map_propagator::FieldIter, Extractor, Injector, TextMapPropagator},
        trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState},
        Context,
    };

    use crate::exporter::u128_to_u64s;

    const DATADOG_TRACE_ID_HEADER: &str = "x-datadog-trace-id";
    const DATADOG_PARENT_ID_HEADER: &str = "x-datadog-parent-id";
    const DATADOG_SAMPLING_PRIORITY_HEADER: &str = "x-datadog-sampling-priority";

    const TRACE_FLAG_DEFERRED: TraceFlags = TraceFlags::new(0x02);

    lazy_static::lazy_static! {
        static ref DATADOG_HEADER_FIELDS: [String; 3] = [
            DATADOG_TRACE_ID_HEADER.to_string(),
            DATADOG_PARENT_ID_HEADER.to_string(),
            DATADOG_SAMPLING_PRIORITY_HEADER.to_string(),
        ];
    }

    enum SamplingPriority {
        UserReject = -1,
        AutoReject = 0,
        AutoKeep = 1,
        UserKeep = 2,
    }

    #[derive(Debug)]
    enum ExtractError {
        TraceId,
        SpanId,
        SamplingPriority,
    }

    /// Extracts and injects `SpanContext`s into `Extractor`s or `Injector`s using Datadog's header format.
    ///
    /// The Datadog header format does not have an explicit spec, but can be divined from the client libraries,
    /// such as [dd-trace-go]
    ///
    /// ## Example
    ///
    /// ```
    /// use opentelemetry::global;
    /// use opentelemetry_datadog_cloudflare::DatadogPropagator;
    ///
    /// global::set_text_map_propagator(DatadogPropagator::default());
    /// ```
    ///
    /// [dd-trace-go]: https://github.com/DataDog/dd-trace-go/blob/v1.28.0/ddtrace/tracer/textmap.go#L293
    #[derive(Clone, Debug, Default)]
    #[allow(clippy::module_name_repetitions)]
    pub struct DatadogPropagator {
        _private: (),
    }

    impl DatadogPropagator {
        /// Creates a new `DatadogPropagator`.
        #[must_use]
        pub fn new() -> Self {
            DatadogPropagator::default()
        }

        fn extract_trace_id(trace_id: &str) -> Result<TraceId, ExtractError> {
            trace_id
                .parse::<u64>()
                .map(|id| TraceId::from(u128::from(id).to_be_bytes()))
                .map_err(|_| ExtractError::TraceId)
        }

        fn extract_span_id(span_id: &str) -> Result<SpanId, ExtractError> {
            span_id
                .parse::<u64>()
                .map(|id| SpanId::from(id.to_be_bytes()))
                .map_err(|_| ExtractError::SpanId)
        }

        fn extract_sampling_priority(
            sampling_priority: &str,
        ) -> Result<SamplingPriority, ExtractError> {
            let i = sampling_priority
                .parse::<i32>()
                .map_err(|_| ExtractError::SamplingPriority)?;

            match i {
                -1 => Ok(SamplingPriority::UserReject),
                0 => Ok(SamplingPriority::AutoReject),
                1 => Ok(SamplingPriority::AutoKeep),
                2 => Ok(SamplingPriority::UserKeep),
                _ => Err(ExtractError::SamplingPriority),
            }
        }

        fn extract_span_context(extractor: &dyn Extractor) -> Result<SpanContext, ExtractError> {
            let trace_id =
                Self::extract_trace_id(extractor.get(DATADOG_TRACE_ID_HEADER).unwrap_or(""))?;
            // If we have a trace_id but can't get the parent span, we default it to invalid instead of completely erroring
            // out so that the rest of the spans aren't completely lost
            let span_id =
                Self::extract_span_id(extractor.get(DATADOG_PARENT_ID_HEADER).unwrap_or(""))
                    .unwrap_or(SpanId::INVALID);
            let sampling_priority = Self::extract_sampling_priority(
                extractor
                    .get(DATADOG_SAMPLING_PRIORITY_HEADER)
                    .unwrap_or(""),
            );
            let sampled = match sampling_priority {
                Ok(SamplingPriority::UserReject | SamplingPriority::AutoReject) => {
                    TraceFlags::default()
                }
                Ok(SamplingPriority::UserKeep | SamplingPriority::AutoKeep) => TraceFlags::SAMPLED,
                // Treat the sampling as DEFERRED instead of erroring on extracting the span context
                Err(_) => TRACE_FLAG_DEFERRED,
            };

            let trace_state = TraceState::default();

            Ok(SpanContext::new(
                trace_id,
                span_id,
                sampled,
                true,
                trace_state,
            ))
        }
    }

    impl TextMapPropagator for DatadogPropagator {
        fn inject_context(&self, cx: &Context, injector: &mut dyn Injector) {
            let span = cx.span();
            let span_context = span.span_context();
            if span_context.is_valid() {
                let [t0, _] = u128_to_u64s(u128::from_be_bytes(span_context.trace_id().to_bytes()));
                injector.set(DATADOG_TRACE_ID_HEADER, t0.to_string());
                injector.set(
                    DATADOG_PARENT_ID_HEADER,
                    u64::from_be_bytes(span_context.span_id().to_bytes()).to_string(),
                );

                if span_context.trace_flags() & TRACE_FLAG_DEFERRED != TRACE_FLAG_DEFERRED {
                    let sampling_priority = if span_context.is_sampled() {
                        SamplingPriority::AutoKeep
                    } else {
                        SamplingPriority::AutoReject
                    };

                    injector.set(
                        DATADOG_SAMPLING_PRIORITY_HEADER,
                        (sampling_priority as i32).to_string(),
                    );
                }
            }
        }

        fn extract_with_context(&self, cx: &Context, extractor: &dyn Extractor) -> Context {
            let extracted = Self::extract_span_context(extractor)
                .unwrap_or_else(|_| SpanContext::empty_context());

            cx.with_remote_span_context(extracted)
        }

        fn fields(&self) -> FieldIter<'_> {
            FieldIter::new(DATADOG_HEADER_FIELDS.as_ref())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use opentelemetry::testing::trace::TestSpan;
        use opentelemetry::trace::TraceState;
        use std::collections::HashMap;

        #[rustfmt::skip]
        fn extract_test_data() -> Vec<(Vec<(&'static str, &'static str)>, SpanContext)> {
            vec![
                (vec![], SpanContext::empty_context()),
                (vec![(DATADOG_SAMPLING_PRIORITY_HEADER, "0")], SpanContext::empty_context()),
                (vec![(DATADOG_TRACE_ID_HEADER, "garbage")], SpanContext::empty_context()),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "garbage")], SpanContext::new(TraceId::from_u128(1234), SpanId::INVALID, TRACE_FLAG_DEFERRED, true, TraceState::default())),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "12")], SpanContext::new(TraceId::from_u128(1234), SpanId::from_u64(12), TRACE_FLAG_DEFERRED, true, TraceState::default())),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "12"), (DATADOG_SAMPLING_PRIORITY_HEADER, "0")], SpanContext::new(TraceId::from_u128(1234), SpanId::from_u64(12), TraceFlags::default(), true, TraceState::default())),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "12"), (DATADOG_SAMPLING_PRIORITY_HEADER, "1")], SpanContext::new(TraceId::from_u128(1234), SpanId::from_u64(12), TraceFlags::SAMPLED, true, TraceState::default())),
            ]
        }

        #[rustfmt::skip]
        fn inject_test_data() -> Vec<(Vec<(&'static str, &'static str)>, SpanContext)> {
            vec![
                (vec![], SpanContext::empty_context()),
                (vec![], SpanContext::new(TraceId::INVALID, SpanId::INVALID, TRACE_FLAG_DEFERRED, true, TraceState::default())),
                (vec![], SpanContext::new(TraceId::from_hex("1234").unwrap(), SpanId::INVALID, TRACE_FLAG_DEFERRED, true, TraceState::default())),
                (vec![], SpanContext::new(TraceId::from_hex("1234").unwrap(), SpanId::INVALID, TraceFlags::SAMPLED, true, TraceState::default())),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "12")], SpanContext::new(TraceId::from_u128(1234), SpanId::from_u64(12), TRACE_FLAG_DEFERRED, true, TraceState::default())),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "12"), (DATADOG_SAMPLING_PRIORITY_HEADER, "0")], SpanContext::new(TraceId::from_u128(1234), SpanId::from_u64(12), TraceFlags::default(), true, TraceState::default())),
                (vec![(DATADOG_TRACE_ID_HEADER, "1234"), (DATADOG_PARENT_ID_HEADER, "12"), (DATADOG_SAMPLING_PRIORITY_HEADER, "1")], SpanContext::new(TraceId::from_u128(1234), SpanId::from_u64(12), TraceFlags::SAMPLED, true, TraceState::default())),
            ]
        }

        #[test]
        fn test_extract() {
            for (header_list, expected) in extract_test_data() {
                let map: HashMap<String, String> = header_list
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();

                let propagator = DatadogPropagator::default();
                let context = propagator.extract(&map);
                assert_eq!(context.span().span_context(), &expected);
            }
        }

        #[test]
        fn test_extract_empty() {
            let map: HashMap<String, String> = HashMap::new();
            let propagator = DatadogPropagator::default();
            let context = propagator.extract(&map);
            assert_eq!(context.span().span_context(), &SpanContext::empty_context());
        }

        #[test]
        fn test_inject() {
            let propagator = DatadogPropagator::default();
            for (header_values, span_context) in inject_test_data() {
                let mut injector: HashMap<String, String> = HashMap::new();
                propagator.inject_context(
                    &Context::current_with_span(TestSpan(span_context)),
                    &mut injector,
                );

                if !header_values.is_empty() {
                    for (k, v) in header_values {
                        let injected_value: Option<&String> = injector.get(k);
                        assert_eq!(injected_value, Some(&v.to_string()));
                        injector.remove(k);
                    }
                }
                assert!(injector.is_empty());
            }
        }
    }
}

pub use exporter::{
    new_pipeline, DatadogExporter, DatadogPipelineBuilder, Error, SpanProcessExt,
    WASMWorkerSpanProcessor,
};
pub use propagator::DatadogPropagator;
