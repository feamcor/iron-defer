# How to Run Chaos Tests

## Goal

Validate recovery behavior during worker crashes, DB outages, and shutdown disruptions.

## When to Use

Use this before production rollouts and regularly in staging/CI.

## Prerequisites

- Docker runtime available
- Rust test environment ready

## Steps

1. Run targeted chaos tests:

   ```sh
   cargo test -p iron-defer --test chaos_worker_crash_test
   cargo test -p iron-defer --test chaos_db_outage_test
   cargo test -p iron-defer --test chaos_sigterm_test
   ```

2. Verify integrity with E2E checks:

   ```sh
   cargo test -p iron-defer --test e2e_data_integrity_test
   ```

3. After each `cargo test`, clean up testcontainers:

   ```sh
   docker ps -aq --filter "label=org.testcontainers=true" | xargs -r docker rm -f 2>/dev/null; docker ps -aq --filter "ancestor=postgres:11-alpine" | xargs -r docker rm -f 2>/dev/null
   ```

## Verification

- tasks recover to retryable or terminal states after disruption
- readiness and queue processing return after dependency recovery

## Troubleshooting

- Stuck `running` tasks: inspect lease/sweeper settings.
- Missing retries: inspect `maxAttempts`, backoff config, and error types.
- Post-outage readiness failures: inspect DB pool saturation and reconnection behavior.
