# Security Guidelines

Security is a first-class concern in iron-defer. This guide covers OWASP-relevant patterns for Rust, supply chain security, and secret handling.

For compliance-framework attestation (PCI DSS Req. 10, SOC 2 CC7.2, DORA, NIS2, GDPR, HIPAA, ISO 27001:2022), see the evidence runbook at [`compliance-evidence.md`](./compliance-evidence.md) — it maps each framework's requirement to a concrete iron-defer artifact (SQL query, metric, log event, or configuration default) an auditor can inspect.

---

## OWASP Top 10 — Rust Relevance

### A01: Broken Access Control

iron-defer's management API must authenticate and authorize every request.

```rust
use axum::{middleware, Router};

// Apply authentication middleware to all routes
let app = Router::new()
    .nest("/api/v1", protected_routes())
    .layer(middleware::from_fn_with_state(state.clone(), authenticate));

async fn authenticate(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = extract_bearer_token(&req)?;
    let claims = state.auth.verify_token(&token).await?;
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
```

### A02: Cryptographic Failures

- Never implement cryptography — use audited crates: `ring`, `rustls`, `argon2`, `sha2`
- Use `rustls` for TLS — avoids OpenSSL (banned via `cargo deny`)
- Password hashing: `argon2` with tuned parameters, never `md5`/`sha1` for passwords
- Secrets in memory: use `zeroize` to clear sensitive data when dropped

```rust
use zeroize::Zeroizing;

// Automatically zeroed on drop
let api_key: Zeroizing<String> = Zeroizing::new(load_secret_from_env()?);
```

```toml
[dependencies]
zeroize = { version = "1", features = ["derive"] }
```

```rust
use zeroize::Zeroize;

#[derive(Zeroize)]
#[zeroize(drop)]
pub struct Credentials {
    pub api_key: String,
    pub secret: Vec<u8>,
}
```

### A03: Injection

Rust's type system mitigates many injection vectors, but explicit protection is still required.

**SQL Injection:** Never interpolate values into SQL strings. Use `sqlx` parameterized queries exclusively:

```rust
// Safe: parameterized
sqlx::query!("SELECT * FROM tasks WHERE id = $1", id.as_uuid())

// NEVER do this:
let query = format!("SELECT * FROM tasks WHERE id = '{}'", id); // SQL injection
```

**Command Injection:** Avoid `std::process::Command` with user-controlled input. If shell execution is necessary:

```rust
use std::process::Command;

// Safe: arguments passed as separate strings, never interpolated into a shell string
Command::new("program")
    .arg("--flag")
    .arg(user_input)  // passed as a distinct argument, not shell-interpreted
    .output()?;

// NEVER:
Command::new("sh").arg("-c").arg(format!("program {}", user_input))  // command injection
```

**Log Injection:** Sanitize user-controlled strings before logging:

```rust
// Tracing structured fields are safe — values are serialized, not interpolated into format strings
tracing::info!(task_name = %task.name, "task created");  // Safe

// Avoid this pattern when name could contain newlines or control chars:
tracing::info!("task created: {}", user_controlled_string);
```

### A04: Insecure Design

- Task payloads are validated at the boundary (see [ADR-0006](../adr/0006-serialization-serde.md))
- Enforce body size limits on all HTTP endpoints
- Rate limiting via `tower-governor` or similar middleware
- Task execution is idempotent — at-least-once delivery requires safe re-execution

### A05: Security Misconfiguration

- No default credentials anywhere in the codebase
- Development defaults must fail in production (detect via `IRON_DEFER_PROFILE`)
- `deny_unknown_fields` on all config structs catches misconfiguration at startup
- TLS enforced in non-local environments — `rustls` required
- Database connection requires SSL in non-local: `?sslmode=require` in connection URL

### A06: Vulnerable and Outdated Components

Handled by supply chain gates — see [Supply Chain Security](#supply-chain-security) section.

### A07: Identification and Authentication Failures

- Use `jsonwebtoken` or `jwt-simple` for JWT — validated cryptographically
- Implement token expiry — never issue non-expiring tokens
- Use constant-time comparison for secrets: `subtle::ConstantTimeEq`

```rust
use subtle::ConstantTimeEq;

fn verify_api_key(provided: &[u8], expected: &[u8]) -> bool {
    // Timing-safe comparison — prevents timing attacks
    provided.ct_eq(expected).into()
}
```

### A08: Software and Data Integrity Failures

- Verify checksums of any downloaded artifacts in build scripts
- Pin dependency versions in `Cargo.lock` — commit `Cargo.lock` for binaries
- Use `cargo deny` to enforce allowed registries (no unknown git sources)

### A09: Security Logging and Monitoring Failures

- Log all authentication failures with contextual metadata (IP, timestamp, attempted resource)
- Log task state transitions with actor identity
- Never log credentials, API keys, or PII
- See [structured-logging.md](./structured-logging.md) for the iron-defer runbook — field glossary, lifecycle event catalogue, payload-privacy opt-in semantics, and test-time capture patterns

```rust
// Log authentication failure — metadata without credentials
tracing::warn!(
    ip = %request_ip,
    path = %request_path,
    "authentication failed"
    // token value intentionally omitted
);
```

### A10: Server-Side Request Forgery (SSRF)

iron-defer makes outbound HTTP calls (webhook delivery). Validate URLs against an allowlist or use network-level controls:

```rust
pub fn validate_webhook_url(url: &url::Url) -> Result<(), TaskError> {
    match url.scheme() {
        "https" => {} // only HTTPS allowed
        _ => return Err(TaskError::InvalidPayload {
            reason: "webhook URL must use HTTPS".into(),
        }),
    }

    // Block private/loopback ranges in non-development environments
    if let Some(host) = url.host_str() {
        if is_private_host(host) {
            return Err(TaskError::InvalidPayload {
                reason: "webhook URL must not target private addresses".into(),
            });
        }
    }

    Ok(())
}
```

---

## Supply Chain Security

### Dependency Vetting Checklist

Before adding any new dependency, verify:

1. **License:** allowed by `deny.toml` policy
2. **Maintenance:** actively maintained, recent commits
3. **Advisories:** `cargo audit` shows no known vulnerabilities
4. **Popularity:** sufficient downloads/stars to indicate community review
5. **Minimize features:** disable default features, enable only what you need
6. **Transitive deps:** `cargo tree -d` shows no unacceptable duplicates

```bash
# Check transitive deps and duplicates
cargo tree --duplicates

# Check a crate's features before adding
cargo add some-crate --dry-run
```

### Cargo.lock Policy

- **Commit `Cargo.lock`** for all binaries and the workspace root
- Do not commit `Cargo.lock` for published library crates (standard convention)
- Reproducible builds require a committed lockfile

### `cargo deny` Supply Chain Rules

See [quality-gates.md](quality-gates.md) for full `deny.toml` configuration.

Key rules:
- Unknown registries: `deny`
- Unknown git sources: `deny`
- Yanked crates: `deny`
- OpenSSL: `deny` (use `rustls`)

### Dependency Updates

- Run `cargo update` regularly (weekly in active development)
- `cargo audit` runs on every CI build and catches new advisories on existing deps
- Use `cargo-upgrades` or Dependabot for proactive version upgrades

---

## Secret Handling

### Rules

1. **No secrets in source code** — no hardcoded keys, passwords, or tokens
2. **No secrets in `Cargo.toml`** — not even test credentials
3. **No secrets in logs** — explicitly `skip(...)` sensitive fields in `#[instrument]`
4. **Secrets in memory:** use `Zeroizing<T>` for secrets that must be stored in structs
5. **`.env` files:** gitignored, never committed; `.env.example` with placeholders is committed

### Detecting Leaked Secrets

Use `git-secrets` or `trufflehog` in pre-commit hooks:

```bash
# .git/hooks/pre-commit
git secrets --scan
```

### Environment Variable Security

```bash
# Never:
DATABASE_URL=postgres://admin:password123@localhost/iron_defer  # in committed files

# Always: inject at runtime
# .env (gitignored):
DATABASE_URL=postgres://admin:${DB_PASSWORD}@localhost/iron_defer
```

In production, use a secrets manager (HashiCorp Vault, AWS Secrets Manager, etc.) to inject `DATABASE_URL` and other secrets as environment variables at container startup.

---

## Input Validation

All external input is validated at the adapter boundary before entering the domain:

```rust
// Adapter layer validates input
pub async fn create_task(
    State(state): State<AppState>,
    Json(body): Json<CreateTaskRequest>,  // deserialized here
) -> Result<Json<TaskResponse>, AppError> {
    // Validate and convert to domain command
    let command = CreateTaskCommand::try_from(body)
        .map_err(|e| AppError::validation(e))?;

    // Domain only receives validated input
    let task = state.task_service.create(command).await?;
    Ok(Json(TaskResponse::from(task)))
}
```

### Validation Rules

- **String lengths:** enforce maximum lengths on all string inputs
- **UUIDs:** always parse via `Uuid::parse_str()`, never trust raw strings from users
- **Enums:** use `TryFrom<&str>` with explicit error on unknown variants
- **URLs:** parse via `url::Url::parse()` and validate scheme/host
- **Dates:** validate ranges (no dates before epoch, no unreasonably far future dates)

---

## TLS Configuration

```rust
// reqwest client — TLS always enabled
let client = reqwest::Client::builder()
    .use_rustls_tls()      // explicit rustls (not native-tls/OpenSSL)
    .https_only(true)      // reject plain HTTP in production
    .min_tls_version(reqwest::tls::Version::TLS_1_2)
    .build()?;
```

For axum with TLS (via `axum-server`):
```rust
use axum_server::tls_rustls::RustlsConfig;

let config = RustlsConfig::from_pem_file(cert_path, key_path).await?;
axum_server::bind_rustls(addr, config)
    .serve(app.into_make_service())
    .await?;
```

---

## References

- [OWASP Top 10](https://owasp.org/www-project-top-ten/)
- [OWASP Rust Cheat Sheet](https://cheatsheetseries.owasp.org/)
- [RustSec Advisory Database](https://rustsec.org/)
- [`zeroize` crate](https://docs.rs/zeroize)
- [`subtle` crate (constant-time operations)](https://docs.rs/subtle)
- [Rust Security Working Group](https://www.rust-lang.org/governance/wgs/wg-security-response)
