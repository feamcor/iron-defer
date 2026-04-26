# Benchmarking Guide

This guide describes how to run and interpret benchmarks for `iron-defer`.

## Prerequisites

- **Postgres:** Benchmarks require a running Postgres instance.
- **Criterion:** We use [Criterion.rs](https://github.com/bheisler/criterion.rs) for statistically significant benchmarking.
- **Environment:** Results are highly dependent on hardware and Postgres configuration. Use a reference environment for formal NFR validation.

## Available Benchmarks

### Checkpoint Persistence Latency
Measures raw SQL UPDATE latency for checkpoint writes with varying payload sizes (1 KiB to 1 MiB).
- **Goal (NFR-R9):** < 50ms at p99 for 1 MiB payloads.
- **Run:**
  ```bash
  DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo bench --bench checkpoint_latency
  ```

### UNLOGGED Table Throughput
Compares throughput of LOGGED vs UNLOGGED `tasks` table.
- **Goal (NFR-SC6):** >= 5x throughput improvement.
- **Setup:** Requires two separate database instances to prevent cross-contamination.
- **Run:**
  ```bash
  DATABASE_URL=postgres://localhost:5432/logged_db \
  DATABASE_URL_UNLOGGED=postgres://localhost:5433/unlogged_db \
  cargo bench --bench unlogged_throughput
  ```
- **Note:** This benchmark is NOT meaningful on default Postgres configs or CI. It requires production tuning of WAL and checkpointer settings.

### Audit Log Overhead
Measures the overhead of transaction-wrapped audit logging.
- **Run:**
  ```bash
  DATABASE_URL=... cargo bench --bench audit_overhead
  ```

## Interpreting Results

Criterion generates HTML reports in `target/criterion/report/index.html`. These reports provide detailed distributions, regressions, and outliers.

For NFR validation, focus on the **p99** (99th percentile) latencies and the **Throughput** (tasks/sec) metrics.
