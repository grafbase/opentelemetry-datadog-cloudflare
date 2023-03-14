![OpenTelemetry — An observability framework for cloud-native software.][splash]

[splash]: https://raw.githubusercontent.com/open-telemetry/opentelemetry-rust/main/assets/logo-text.png

# OpenTelemetry Datadog Cloudflare

## Overview

This crate provides additional propagators and exporters for sending telemetry data
to [`Datadog`](https://datadoghq.com) directly without going through the
`Datadog-agent`.

Based on [`opentelemetry-datadog`](https://github.com/open-telemetry/opentelemetry-rust/tree/main/opentelemetry-datadog).

## Features

`opentelemetry-datadog-cloudflare` supports following features:

- `reqwest-client`: use the `reqwest` HTTP client to send spans.

