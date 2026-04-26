# Observability Guide

iron-defer exposes three observability surfaces:

- Structured JSON logs (`tracing`)
- OpenTelemetry metrics
- Prometheus scrape endpoint (`/metrics`)

## Metrics

### Lifecycle and reliability

| Metric | Type | Labels | Description |
|---|---|---|---|
| `task_duration_seconds` | Histogram | `queue`, `kind`, `status` | Task handler runtime from dispatch start to completion/failure |
| `task_attempts_total` | Counter | `queue`, `kind` | Claim/dispatch attempts |
| `task_failures_total` | Counter | `queue`, `kind` | Terminal failures |
| `zombie_recoveries_total` | Counter | implementation-defined | Tasks recovered by sweeper |
| `tasks_suspended_total` | Counter | `queue`, `kind` | Transitions into suspended state |
| `suspend_timeout_total` | Counter | `queue` | Auto-failures from suspend timeout watchdog |
| `idempotency_keys_cleaned_total` | Counter | `queue` | Idempotency keys expired and cleaned |

### Throughput/backpressure and pool

| Metric | Type | Labels | Description |
|---|---|---|---|
| `worker_pool_utilization` | Gauge | `queue` | Active worker slots / configured concurrency |
| `claim_backoff_total` | Counter | `queue`, `saturation` | Backoff events during claim path retries |
| `claim_backoff_seconds` | Histogram | `queue` | Backoff delay durations |
| `tasks_pending` | Observable gauge | `queue` | Current pending count |
| `tasks_running` | Observable gauge | `queue` | Current running count |
| `pool_connections_total` | Observable gauge | none | Pool size |
| `pool_connections_idle` | Observable gauge | none | Idle DB connections |
| `pool_connections_active` | Observable gauge | none | In-use DB connections |

Prometheus output uses the `iron_defer_` prefix and standard exporter naming conventions.

## Structured logging

The standalone binary initializes JSON logging by default.

- One structured record per lifecycle transition
- Payload logging is disabled by default (`worker.log_payload = false`)
- `RUST_LOG` controls filtering

Examples:

```sh
RUST_LOG=info iron-defer serve
RUST_LOG=iron_defer=debug,sqlx=warn iron-defer serve
```

For the full field glossary and event catalog, see `docs/guidelines/structured-logging.md`.

## Prometheus scraping

Scrape `/metrics`:

```sh
curl http://localhost:8080/metrics
```

- Returns `200` with `text/plain; version=0.0.4` when metrics are configured
- Returns `404` when metrics registry is unavailable

## Embedded mode setup

In embedded deployments, provide both metric handles and a registry to the builder:

```rust
let registry = prometheus::Registry::new();
let exporter = opentelemetry_prometheus::exporter()
    .with_registry(registry.clone())
    .build()?;
let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
    .with_reader(exporter)
    .build();
let meter = provider.meter("iron_defer");
let metrics = iron_defer::create_metrics(&meter);

let engine = IronDefer::builder()
    .pool(pool)
    .metrics(metrics)
    .prometheus_registry(registry)
    .build()
    .await?;
```

## OTLP export

Enable OTLP export in standalone mode via flag or config:

```sh
iron-defer serve --otlp-endpoint http://collector:4317
```

or

```sh
IRON_DEFER__OBSERVABILITY__OTLP_ENDPOINT=http://collector:4317
```
