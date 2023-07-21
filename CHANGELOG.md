# Changelog

## [Unreleased]

## [0.12.0]

-   Use a thread local instead of rwlock to store the spans
-   Drop opentelemetry::http as it was pulling the `blocking` feature of reqwest and causing hanging issues
-   Fixes a runtime panic when determining the span duration

## [0.11.0]

-   Drop unused dependencies (including the worker crate).
-   Switch to using reqwest as the HTTP client.

## [0.10.0]

-   Add an extension that provides an async `force_flush`
-   Use `opentelemetry-spanprocessor-any` crate in favour of `opentelemetry` to have the `Any` patch available 

## [0.9.0]

## [0.8.0]

-   Upgrade opentelemetry to `v0.18`

## [0.7.0]

-   Upgrade worker-rs to `v0.0.12`

## [0.6.0]

-   Upgrade worker-rs to `v0.0.11`

## [0.5.0]

### Feat

-   Upgrade worker-rs to `v0.0.10`

## [0.4.1]

### Misc

-   Clippy

## [0.4.0]

### Feat

-   Add some utility functions to add some data to the root trace

## [0.3.0]

### Feat

-   `with_api_key` take an option.

## [0.2.0]

### Misc

-   Remove useless comments

## [0.1.12]

-   Fix deploy step

## [0.1.11]

-   Use workflow token to trigger deploy (bis)

## [0.1.10]

-   Use workflow token to trigger deploy

## [0.1.9]

-   Change to token to create a release

## [0.1.8]

-   Downgrade the release creation

## [0.1.7]

-   Solve the release creation

## [0.1.6]

-   Downgrade deployment Action

## [0.1.5]

-   Change token for deployment

## [0.1.4]

-   Update CI, remove preview to create deployment

## [0.1.3]

-   Update CI Merge

## [0.1.2]

-   Test release

## [0.1.1]

-   Update README.md
-   Update Actions

## [0.1.0]

-   Datadog exporter from `opentelemetry-datadog`.
