# Postgres Reconnection Behavior

iron-defer handles transient Postgres outages without introducing a custom
reconnection loop. SQLx's built-in `PgPool` reconnection is the primary
mechanism; iron-defer configures it with resilience-appropriate options and
surfaces pool saturation as an operator-visible warning.

## Standalone-mode pool defaults

`iron_defer_infrastructure::create_pool()` constructs a `PgPool` with the
following defaults (all exported as `pub const` in
[`crates/infrastructure/src/db.rs`](../../crates/infrastructure/src/db.rs)):

| Constant | Value | Rationale |
|---|---|---|
| `DEFAULT_MAX_CONNECTIONS` | `10` | Default standalone pool size. |
| `DEFAULT_MIN_CONNECTIONS` | `0` | Lets the pool go fully cold during outages so it does not spin retry storms against an unreachable server. |
| `DEFAULT_ACQUIRE_TIMEOUT` | `5s` | Bounds caller-visible latency when the pool is saturated or reconnecting. |
| `DEFAULT_IDLE_TIMEOUT` | `300s` | Recycles idle connections so stale TCP sessions do not silently linger across Postgres restarts. |
| `DEFAULT_MAX_LIFETIME` | `1800s` | Upper bound on connection age regardless of idleness; guards against TCP half-open states. |
| `test_before_acquire` | `true` | Pings each checked-out connection so stale connections are dropped and replaced transparently. |

**`test_before_acquire = true` is the keystone.** After a Postgres restart, a
pool holding half-open TCP sockets would otherwise hand out broken connections
until each one failed a real query. The ping short-circuits that cycle with
a ~1 ms LAN round-trip per acquire — negligible for a 500 ms poll interval,
essential during chaos recovery.

## `test_before_acquire` overhead

Each pool checkout with `test_before_acquire(true)` executes a `SELECT 1` ping.
Measured overhead: ~1ms on LAN, higher on WAN. This is configurable via
`DatabaseConfig.test_before_acquire` (default: `true`).

## Pool recovery interaction

During a Postgres outage, all pooled connections become stale. When the
database recovers, each stale connection consumes up to `acquire_timeout`
(default: 5s) for the ping + reconnect cycle. With a pool of 10 connections,
worst-case recovery time is ~50s if all connections are stale and each
reconnect attempt takes the full timeout.

**Operator guidance:** size `acquire_timeout` to at least 2x the expected
reconnect time. For LAN deployments with sub-second reconnects, the 5s
default is generous. For WAN or cloud deployments with longer failover
times, consider increasing to 10 to 15 seconds.

## Migration failure modes

SQLx migrations are transactional per-file on Postgres. If a migration
file partially applies, the entire file is rolled back. Cross-file partial
state (e.g., migration 0001 applied, 0002 failed) requires manual recovery
via `sqlx migrate revert`.

`IronDefer::migrator()` (exposed at `crates/api/src/lib.rs`) returns the
static `Migrator` for callers who manage migrations externally.

## Embedded-library mode

`crates/api/src/lib.rs` accepts a caller-provided `sqlx::PgPool` via
`IronDefer::builder().pool(...)`. Callers who construct their own pool should
use `iron_defer_infrastructure::recommended_pool_options()` as a starting
point — it pre-configures the hardened defaults from the table above. The
caller can further customize (e.g., adjust `max_connections`) before calling
`.connect()`.

Callers who construct their own pool without `recommended_pool_options()` are
responsible for their own options but should mirror `test_before_acquire = true`
for the same resilience guarantees.

## Pool saturation signal

When the worker poll loop observes a claim error whose root cause is
`sqlx::Error::PoolTimedOut`, it emits a `warn!` log (not `error!`) tagged
`event = "pool_saturated"` with `worker_id`, `queue`, and error text. No task
payload or kind appears in this log (FR38 payload privacy).

Tasks are not lost on saturation: the failed claim releases its permit and
the poll loop retries on the next tick. Tasks stay in `Pending` and are
claimed on the next successful poll cycle.

### Operator tuning

- Frequent `pool_saturated` warnings → increase `max_connections`, investigate
  slow downstream queries, or profile worker handler durations.
- Sustained saturation → a long-running handler is holding connections;
  check handler code for unbounded queries or network calls without timeouts.

The `pool_wait_queue_depth` gauge is not currently emitted.

## What iron-defer does NOT do

- No custom reconnection loop. SQLx reconnects transparently; writing our
  own would duplicate that work and break the caller-provided-`PgPool`
  contract (architecture line 488).
- No watchdog task that reconstructs the `PgPool` on error.
- No custom retry-with-jitter loop on consecutive claim errors. The
  `tokio::time::interval` poll cadence (default 500 ms) is the retry schedule.
