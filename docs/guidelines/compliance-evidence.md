# Compliance Evidence Runbook

This runbook maps common audit asks to concrete artifacts produced by iron-defer today.

## Framework-to-evidence map

| Framework | Control area | Evidence artifact | How to collect |
|---|---|---|---|
| PCI DSS Req. 10 | Audit trail | `tasks` rows + optional `task_audit_log` transitions | SQL queries against `tasks` and `task_audit_log` by queue/time window |
| SOC 2 CC7.2 | Monitoring and detection | Structured lifecycle logs (`event`, `task_id`, status transitions) | Aggregate logs and alert on failure/saturation event patterns |
| DORA / operational resilience | Incident metrics | OTel/Prometheus counters and histograms | Scrape `/metrics`, retain time-series for trend and incident reporting |
| NIS2 / supply chain | Dependency governance | `Cargo.lock` + `deny.toml` + CI `cargo deny check` | Run CI gate and archive results |
| GDPR / data minimization | Privacy by default | `worker.log_payload = false` default | Validate effective config and sample logs for payload absence |
| HIPAA Security Rule | Audit controls and transport security | Audit rows + rustls-based dependency policy | Verify DB auditability and dependency tree (`cargo tree`) |
| ISO 27001 A.8.15 | Logging | Structured JSON logs + retention policy | Collect logs centrally and verify retention/access controls |
| ISO 27001 A.8.28 | Secure coding | `#![forbid(unsafe_code)]` in production crates + CI quality gates | Inspect crate roots and CI pipeline outputs |

## Collection checklist

1. Capture a time range and queue(s) for evidence extraction.
2. Export matching SQL snapshots from `tasks` / `task_audit_log`.
3. Export Prometheus snapshots for reliability counters/histograms.
4. Export structured log samples for lifecycle events and failures.
5. Attach CI run showing formatting, clippy, deny, tests, and sqlx cache checks.

## Notes

- Use UTC timestamps consistently across SQL, metrics, and log exports.
- If payload logging is enabled intentionally, document risk acceptance and retention controls.
- Keep evidence bundles immutable once attached to an audit package.
