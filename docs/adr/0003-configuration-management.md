# ADR-0003: Configuration Management

**Status:** Accepted
**Date:** 2026-04-02

---

## Context

iron-defer is deployed in multiple environments (local development, CI, staging, production). Configuration must:

- Follow the [12-Factor App](https://12factor.net/config) principle: config in the environment
- Support layered overrides: file defaults → env vars → CLI flags
- Be fully typed and validated at startup — no stringly-typed config access at runtime
- Support local development ergonomics via `.env` files without compromising production security
- Never have secrets hardcoded in source or config files committed to VCS

## Decision

We use **`figment`** as the configuration composition layer, **`dotenvy`** for `.env` loading, and **`clap`** for CLI argument parsing. Together they implement a strict precedence chain.

## Precedence Chain

```
defaults (Figment default values)
    ↓
config file (config.toml / config.{profile}.toml)
    ↓
.env file (loaded by dotenvy before figment runs)
    ↓
environment variables (IRON_DEFER__ prefix, __ separator for nesting)
    ↓
CLI flags (highest priority — always win)
```

This chain means:
- Operators can configure via env vars in containers (12-Factor)
- Developers can use `.env` locally without touching shell profiles
- Automated tests can override specific values via CLI flags
- No value is ever inaccessible — every layer is inspectable

## Configuration Struct

All configuration is deserialized into a strongly-typed struct at startup:

```rust
// crates/application/src/config.rs
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub database: DatabaseConfig,
    pub server: ServerConfig,
    pub worker: WorkerConfig,
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub connect_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct WorkerConfig {
    pub concurrency: usize,
    pub poll_interval_ms: u64,
    pub max_retries: u32,
}

#[derive(Debug, Deserialize)]
pub struct ObservabilityConfig {
    pub log_level: String,
    pub enable_console: bool,
}
```

Rules:
- No `Option<T>` unless the value is genuinely optional — use defaults instead
- No `String` for values with a fixed domain — use enums (and implement `Deserialize`)
- `Duration`-like fields stored as primitive (secs/ms) then converted; avoids `figment` format issues

## Loading Implementation

```rust
// crates/api/src/config.rs
use figment::{Figment, providers::{Env, Format, Toml, Serialized}};
use crate::cli::CliArgs;
use application::config::AppConfig;

pub fn load(cli: &CliArgs) -> color_eyre::Result<AppConfig> {
    // Load .env before figment so vars are visible as env vars
    dotenvy::dotenv().ok();  // .ok() — do not fail if .env is absent (production)

    let config = Figment::new()
        .merge(Serialized::defaults(AppConfig::default()))
        .merge(Toml::file("config.toml"))
        .merge(Toml::file(format!("config.{}.toml", std::env::var("IRON_DEFER_PROFILE").unwrap_or_default())))
        .merge(Env::prefixed("IRON_DEFER__").split("__"))
        .merge(cli.to_figment())
        .extract()?;

    Ok(config)
}
```

## Environment Variable Naming

Variables use `IRON_DEFER__` prefix and `__` (double underscore) for nesting:

```bash
IRON_DEFER__DATABASE__URL=postgres://user:pass@localhost/iron_defer
IRON_DEFER__SERVER__PORT=8080
IRON_DEFER__WORKER__CONCURRENCY=4
IRON_DEFER__OBSERVABILITY__OTLP_ENDPOINT=http://collector:4317
```

This maps directly to the nested struct hierarchy. Single `_` within a segment is preserved (e.g., `IRON_DEFER__DATABASE__MAX_CONNECTIONS`).

## CLI Argument Integration

```rust
// crates/api/src/cli.rs
use clap::Parser;
use figment::providers::Serialized;

#[derive(Debug, Parser)]
pub struct CliArgs {
    #[arg(long, env = "IRON_DEFER__SERVER__PORT")]
    pub port: Option<u16>,

    #[arg(long, env = "IRON_DEFER__OBSERVABILITY__OTLP_ENDPOINT")]
    pub otlp_endpoint: Option<String>,

    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
}

impl CliArgs {
    pub fn to_figment(&self) -> Serialized<Self> {
        Serialized::defaults(self)
    }
}
```

`clap` with `env` attributes allows both `--port 9000` and `IRON_DEFER__SERVER__PORT=9000` to work for the same field. CLI wins due to its position at the end of the `Figment` chain.

## Secrets Handling

- **Never** commit `.env` files containing real credentials — `.env` is in `.gitignore`
- Provide `.env.example` with placeholder values for documentation
- In production, inject secrets via environment variables from a secrets manager (Vault, AWS Secrets Manager, etc.)
- `DatabaseConfig.url` contains credentials — never log the full config struct. Log a redacted summary:

```rust
tracing::info!(
    host = %config.server.host,
    port = config.server.port,
    log_level = %config.observability.log_level,
    "configuration loaded"
    // database.url intentionally omitted
);
```

## Profile Support

`IRON_DEFER_PROFILE` selects an overlay file:

```
config.toml          # base defaults
config.local.toml    # local dev overrides (gitignored)
config.test.toml     # CI/test overrides
```

## Consequences

**Positive:**
- Single typed struct — no stringly-typed config access anywhere in the codebase
- 12-Factor compliant — all config can come from environment
- Developer-friendly `.env` support without production risk
- Profiles enable environment-specific defaults without code changes

**Negative:**
- `figment` is not in the standard ecosystem — accepted for its composability
- Nested env var naming (`__`) can be unfamiliar — documented above

## References

- [`figment` crate](https://docs.rs/figment)
- [`dotenvy` crate](https://docs.rs/dotenvy) (maintained fork of `dotenv`)
- [`clap` crate](https://docs.rs/clap)
- [The Twelve-Factor App — Config](https://12factor.net/config)
