![OpenTelemetry — An observability framework for cloud-native software.][splash]

[splash]: https://raw.githubusercontent.com/open-telemetry/opentelemetry-rust/main/assets/logo-text.png

# OpenTelemetry Datadog Cloudflare

## Overview

[`OpenTelemetry`] is a collection of tools, APIs, and SDKs used to instrument,
generate, collect, and export telemetry data (metrics, logs, and traces) for
analysis in order to understand your software's performance and behavior.

This crate provides additional propagators and exporters for sending telemetry data
to [`Datadog`](https://datadoghq.com) directly.

Based on [`opentelemetry-datadog`](https://github.com/open-telemetry/opentelemetry-rust/tree/main/opentelemetry-datadog).

## Features

`opentelemetry-datadog-cloudflare` supports following features:

- `reqwest-blocking-client`: use `reqwest` blocking http client to send spans.
- `reqwest-client`: use `reqwest` http client to send spans.
- `surf-client`: use `surf` http client to send spans.
