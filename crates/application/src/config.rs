//! Configuration structs for the iron-defer engine.
//!
//! These are pure data containers; the actual figment loading chain
//! (defaults → file → .env → env → CLI) lives in `crates/api/src/config.rs`
//! and every struct derives `Default` so tests and downstream crates can
//! construct them freely.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Top-level configuration aggregating every concern.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub database: DatabaseConfig,
    pub worker: WorkerConfig,
    pub server: ServerConfig,
    pub observability: ObservabilityConfig,
    pub producer: ProducerConfig,
}

/// Producer configuration for task submission safety.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProducerConfig {
    /// Whitelist of allowed region labels for geographic pinning.
    /// If empty, any valid region string is accepted.
    pub allowed_regions: Vec<String>,
}

/// `PostgreSQL` connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// libpq-compatible connection string. Empty string = unset.
    pub url: String,
    /// Maximum connections in the pool. `0` means "use library default".
    pub max_connections: u32,
    /// Enable `UNLOGGED` table mode. Mutually exclusive with `audit_log`.
    pub unlogged_tables: bool,
    /// Enable audit logging. Mutually exclusive with `unlogged_tables`.
    pub audit_log: bool,
    /// Validate connections with a `SELECT 1` ping before checkout.
    /// Adds ~1ms LAN latency per acquire but detects stale connections.
    #[serde(default = "default_test_before_acquire")]
    pub test_before_acquire: bool,
}

fn default_test_before_acquire() -> bool {
    true
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 0,
            unlogged_tables: false,
            audit_log: false,
            test_before_acquire: true,
        }
    }
}

/// Worker pool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerConfig {
    /// Maximum simultaneous in-flight tasks per process.
    pub concurrency: u32,
    /// Whether task payloads may appear in log output. Default: `false`
    /// (privacy-by-default per FR38).
    pub log_payload: bool,
    /// How long a worker holds a lease on a claimed task before it becomes
    /// eligible for zombie recovery (Architecture D2.3).
    #[serde(with = "humantime_serde")]
    pub lease_duration: Duration,
    /// Base delay for exponential backoff on task failure (Architecture D1.2).
    #[serde(with = "humantime_serde")]
    pub base_delay: Duration,
    /// Maximum delay cap for exponential backoff (Architecture D1.2).
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    /// How often a worker polls for new tasks when the queue is empty.
    #[serde(with = "humantime_serde")]
    pub poll_interval: Duration,
    /// How often the sweeper checks for zombie tasks (running tasks with
    /// expired leases). Architecture D3.1.
    #[serde(with = "humantime_serde")]
    pub sweeper_interval: Duration,
    /// Maximum backoff delay between claim attempts when consecutive errors
    /// occur. The actual delay uses jittered exponential backoff capped at
    /// this value. NFR-R6 / CR16.
    #[serde(with = "humantime_serde")]
    pub max_claim_backoff: Duration,
    /// Maximum time to wait for in-flight tasks to drain on shutdown before
    /// forcibly releasing leases. Architecture D6.1.
    #[serde(with = "humantime_serde")]
    pub shutdown_timeout: Duration,
    /// How long idempotency keys are retained after a task reaches a terminal
    /// state. After this window the sweeper NULLs the key, allowing reuse.
    #[serde(with = "humantime_serde")]
    pub idempotency_key_retention: Duration,
    /// Maximum time a task may remain in `Suspended` status before the sweeper
    /// auto-fails it. Default: 24 hours.
    #[serde(with = "humantime_serde")]
    pub suspend_timeout: Duration,
    /// Optional region label for geographic task pinning. When set, this worker
    /// claims only tasks with a matching region or no region. When `None`, the
    /// worker claims only tasks with no region set.
    #[serde(default)]
    pub region: Option<String>,
}

impl DatabaseConfig {
    /// Validate cross-field invariants. Called by `IronDeferBuilder::build()`.
    ///
    /// # Errors
    ///
    /// Returns `Err` when `unlogged_tables` and `audit_log` are both `true`.
    pub fn validate(&self) -> Result<(), String> {
        if self.unlogged_tables && self.audit_log {
            return Err("UNLOGGED table mode and audit_log are mutually exclusive \
                 — UNLOGGED tables do not survive Postgres crash recovery \
                 and cannot satisfy audit trail requirements."
                .into());
        }
        Ok(())
    }
}

/// Minimum allowed value for Duration-based configuration fields.
/// Prevents zero-duration configurations that cause busy-loops
/// (`tokio::time::interval(Duration::ZERO)` panics) or break timing
/// invariants (zero lease duration makes every task an instant zombie).
const MIN_DURATION: Duration = Duration::from_millis(10);

impl WorkerConfig {
    /// Validate cross-field invariants that cannot be expressed through
    /// individual field types. Called by `IronDeferBuilder::build()`.
    ///
    /// # Errors
    ///
    /// Returns `Err` describing the first invariant violation found.
    pub fn validate(&self) -> Result<(), String> {
        if self.concurrency == 0 {
            return Err("concurrency must be >= 1".into());
        }
        if self.poll_interval < MIN_DURATION {
            return Err(format!(
                "poll_interval must be >= {MIN_DURATION:?}, got {:?}",
                self.poll_interval
            ));
        }
        if self.sweeper_interval < MIN_DURATION {
            return Err(format!(
                "sweeper_interval must be >= {MIN_DURATION:?}, got {:?}",
                self.sweeper_interval
            ));
        }
        if self.lease_duration < MIN_DURATION {
            return Err(format!(
                "lease_duration must be >= {MIN_DURATION:?}, got {:?}",
                self.lease_duration
            ));
        }
        if self.base_delay.is_zero() {
            return Err("base_delay must be > 0".into());
        }
        if self.max_delay.is_zero() {
            return Err("max_delay must be > 0".into());
        }
        if self.max_delay < self.base_delay {
            return Err(format!(
                "max_delay ({:?}) must be >= base_delay ({:?})",
                self.max_delay, self.base_delay
            ));
        }
        if self.max_claim_backoff.is_zero() {
            return Err("max_claim_backoff must be > 0".into());
        }
        if self.shutdown_timeout.is_zero() {
            return Err("shutdown_timeout must be > 0".into());
        }
        if self.idempotency_key_retention < Duration::from_secs(60) {
            return Err(format!(
                "idempotency_key_retention must be >= 1 minute, got {:?}",
                self.idempotency_key_retention
            ));
        }
        if self.suspend_timeout < MIN_DURATION {
            return Err(format!(
                "suspend_timeout must be >= {MIN_DURATION:?}, got {:?}",
                self.suspend_timeout
            ));
        }
        Ok(())
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            log_payload: false,
            lease_duration: Duration::from_mins(5),
            base_delay: Duration::from_secs(5),
            max_delay: Duration::from_mins(30),
            poll_interval: Duration::from_millis(500),
            sweeper_interval: Duration::from_mins(1),
            max_claim_backoff: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
            idempotency_key_retention: Duration::from_secs(24 * 60 * 60),
            suspend_timeout: Duration::from_secs(24 * 60 * 60),
            region: None,
        }
    }
}

fn default_readiness_timeout() -> u64 {
    5
}

/// Standalone-binary HTTP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind_address: String,
    pub port: u16,
    #[serde(default = "default_readiness_timeout")]
    pub readiness_timeout_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_address: String::new(),
            port: 0,
            readiness_timeout_secs: 5,
        }
    }
}

/// Observability subsystem configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    /// `OTLP` collector endpoint. Empty string = `OTel` disabled.
    pub otlp_endpoint: String,
    /// Prometheus scrape endpoint path, e.g. `/metrics`.
    pub prometheus_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_config_test_before_acquire_defaults_to_true() {
        assert!(DatabaseConfig::default().test_before_acquire);
    }

    #[test]
    fn default_app_config_is_constructible() {
        let cfg = AppConfig::default();
        assert!(cfg.database.url.is_empty());
        assert_eq!(cfg.worker.concurrency, 4);
        assert!(!cfg.worker.log_payload);
        assert_eq!(cfg.worker.lease_duration, Duration::from_mins(5));
        assert_eq!(cfg.worker.base_delay, Duration::from_secs(5));
        assert_eq!(cfg.worker.max_delay, Duration::from_mins(30));
        assert_eq!(cfg.worker.poll_interval, Duration::from_millis(500));
        assert_eq!(cfg.worker.sweeper_interval, Duration::from_mins(1));
        assert_eq!(cfg.worker.max_claim_backoff, Duration::from_secs(30));
        assert_eq!(cfg.worker.shutdown_timeout, Duration::from_secs(30));
        assert_eq!(
            cfg.worker.idempotency_key_retention,
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(
            cfg.worker.suspend_timeout,
            Duration::from_secs(24 * 60 * 60)
        );
    }

    #[test]
    fn default_worker_config_validates() {
        WorkerConfig::default()
            .validate()
            .expect("defaults are valid");
    }

    #[test]
    fn zero_concurrency_rejected() {
        let cfg = WorkerConfig {
            concurrency: 0,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("concurrency"), "{err}");
    }

    #[test]
    fn zero_poll_interval_rejected() {
        let cfg = WorkerConfig {
            poll_interval: Duration::ZERO,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("poll_interval"), "{err}");
    }

    #[test]
    fn zero_sweeper_interval_rejected() {
        let cfg = WorkerConfig {
            sweeper_interval: Duration::ZERO,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("sweeper_interval"), "{err}");
    }

    #[test]
    fn zero_lease_duration_rejected() {
        let cfg = WorkerConfig {
            lease_duration: Duration::ZERO,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("lease_duration"), "{err}");
    }

    #[test]
    fn max_delay_less_than_base_delay_rejected() {
        let cfg = WorkerConfig {
            base_delay: Duration::from_mins(1),
            max_delay: Duration::from_secs(10),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("max_delay"), "{err}");
    }

    #[test]
    fn worker_config_duration_fields_round_trip_through_serde() {
        let cfg = WorkerConfig {
            lease_duration: Duration::from_mins(2),
            base_delay: Duration::from_secs(3),
            max_delay: Duration::from_mins(10),
            poll_interval: Duration::from_millis(250),
            sweeper_interval: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(15),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let parsed: WorkerConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.lease_duration, Duration::from_mins(2));
        assert_eq!(parsed.base_delay, Duration::from_secs(3));
        assert_eq!(parsed.max_delay, Duration::from_mins(10));
        assert_eq!(parsed.poll_interval, Duration::from_millis(250));
        assert_eq!(parsed.sweeper_interval, Duration::from_secs(30));
        assert_eq!(parsed.shutdown_timeout, Duration::from_secs(15));
    }

    #[test]
    fn zero_shutdown_timeout_rejected() {
        let cfg = WorkerConfig {
            shutdown_timeout: Duration::ZERO,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("shutdown_timeout"), "{err}");
    }

    #[test]
    fn unlogged_and_audit_mutual_exclusion() {
        let cfg = DatabaseConfig {
            unlogged_tables: true,
            audit_log: true,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn unlogged_only_accepted() {
        let cfg = DatabaseConfig {
            unlogged_tables: true,
            audit_log: false,
            ..Default::default()
        };
        cfg.validate()
            .expect("unlogged_tables alone should be valid");
    }

    #[test]
    fn audit_only_accepted() {
        let cfg = DatabaseConfig {
            unlogged_tables: false,
            audit_log: true,
            ..Default::default()
        };
        cfg.validate().expect("audit_log alone should be valid");
    }

    #[test]
    fn both_false_accepted() {
        let cfg = DatabaseConfig::default();
        cfg.validate()
            .expect("both false (default) should be valid");
    }
}
