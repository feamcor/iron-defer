# How to Harden Production

## Goal

Reduce operational risk before and during production rollout.

## When to Use

Use this as a release-readiness checklist for new environments and major upgrades.

## Prerequisites

- production-grade config profile
- CI pipeline with quality gates
- observability stack for logs and metrics

## Steps

1. Set explicit runtime limits and timeouts:

   - `database.max_connections`
   - `worker.concurrency`
   - worker timing controls (`poll_interval`, `lease_duration`, `shutdown_timeout`)

2. Enforce transport and secret hygiene:

   - use TLS where applicable
   - source secrets from environment or secret manager
   - keep `worker.log_payload=false` unless explicitly approved

3. Configure operational safeguards:

   - readiness and liveness probes
   - metrics scraping and alerting
   - centralized structured logging with retention policy

4. Ensure quality gates pass:

   ```sh
   cargo fmt --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo deny check
   cargo test --workspace
   cargo sqlx prepare --check --workspace
   ```

## Verification

- all CI quality gates pass
- service passes health/readiness checks in target environment
- rollback and incident runbooks are available and tested

## Troubleshooting

- If readiness flaps, inspect DB connectivity and pool saturation.
- If logs expose sensitive data, re-check payload logging and redaction settings.
