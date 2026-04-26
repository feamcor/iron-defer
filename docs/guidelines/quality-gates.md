# Quality Gates

All quality gates are **blocking** — pull requests cannot merge until every gate passes. No exceptions.

---

## Gate Summary

| Gate | Command | Failure Condition | Status |
|------|---------|------------------|--------|
| Formatting | `cargo fmt --check` | Any formatting diff | Enforced |
| Linting | `cargo clippy --workspace --all-targets -- -D warnings` | Any warning or error | Enforced |
| Dependencies | `cargo deny check` | Banned crate, advisory, or license | Enforced |
| Tests | `cargo test --workspace` | Any failing test | Enforced |
| SQLx Cache | `cargo sqlx prepare --check` | Stale offline query cache | Enforced |
| Dead Dependencies | `cargo machete` | Unused dependency | Optional local gate |
| Coverage | `cargo tarpaulin --fail-under 80` | Domain coverage < 80% | Optional local gate |

---

## 1. Formatting — `rustfmt`

### Configuration

`rustfmt.toml` at workspace root:

```toml
edition = "2024"
max_width = 100
```

### Usage

```bash
# Check (CI — fails on any diff)
cargo fmt --check

# Fix locally
cargo fmt
```

Never commit unformatted code. Configure your editor to run `rustfmt` on save.

---

## 2. Linting — Clippy

### Running Clippy

```bash
# CI command — treats all warnings as errors
cargo clippy --workspace --all-targets -- -D warnings
```

### Aspirational: Pedantic Linting

`clippy::pedantic` is an optional hardening profile for local review. If you
adopt it in a crate, each crate root (`lib.rs` / `main.rs`) should include:

```rust
#![deny(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]  // intentional: TaskError is clearer than Error
```

Never use a blanket `#![allow(clippy::all)]`.

---

## 3. Dependency Policy — `cargo deny`

### Current Scope

`deny.toml` enforces bans, advisories, and licenses:
- Banned crates: `openssl`, `openssl-sys`, `openssl-src`, `native-tls`
- Banned sqlx features: `runtime-tokio-native-tls`, `runtime-async-std-native-tls`, `tls-native-tls`
- Advisory: `vulnerability = "deny"`, `unmaintained = "warn"`
- Known false-positives in `[advisories].ignore` with documented justifications

### Usage

```bash
cargo deny check
```

This runs all checks (bans + advisories + licenses) in a single command.

---

## 4. Dead Dependencies — `cargo machete`

```bash
cargo machete
```

Detects `Cargo.toml` entries for crates not actually imported. Dead dependencies are:
- Supply chain risk (unnecessary exposure)
- Compile time cost
- Maintenance burden

Remove them. If a dependency is used only in tests or build scripts, verify it's in the right `[dependencies]` section (`[dev-dependencies]` or `[build-dependencies]`).

---

## 5. Tests

### Structure

```
crates/domain/tests/       # unit tests for domain logic
crates/application/tests/  # unit tests with mock adapters
crates/infrastructure/tests/ # integration tests (testcontainers)
crates/api/tests/          # E2E tests (full stack)
```

### Running

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p iron-defer-domain

# With output (for debugging)
cargo test -- --nocapture
```

### Async Tests

```rust
#[tokio::test]
async fn test_task_creation() {
    // ...
}

// For tests needing full tokio runtime
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_claim() {
    // ...
}
```

### Test Organization

```rust
// In-module unit tests for private functions
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_display_is_uuid_format() {
        let id = TaskId::new();
        assert!(id.to_string().contains('-'));
    }
}
```

---

## 6. Coverage — `cargo tarpaulin`

### Threshold

- `domain` crate: **80% minimum** (recommended local threshold)
- `application` crate: **70% minimum**
- `infrastructure` crate: coverage tracked but not gated (I/O-heavy, hard to cover without containers)
- `api` crate: E2E test coverage tracked separately

### Running

```bash
# Domain crate with threshold enforcement
cargo tarpaulin -p iron-defer-domain --fail-under 80

# Full workspace report (no threshold)
cargo tarpaulin --workspace --out Html --output-dir coverage/
```

### What Not to Cover

Exclude generated code and trivial delegation from coverage requirements:

```rust
// tarpaulin::skip for generated or trivial code
#[cfg(not(tarpaulin_include))]
impl Default for TaskId {
    fn default() -> Self { Self::new() }
}
```

---

## 7. `unsafe` Policy

| Use | Policy |
|-----|--------|
| `unsafe` blocks | Require a `// SAFETY:` comment on the line before |
| `unsafe` in library crates | Requires PR review from a second contributor |
| `unsafe` in `domain` crate | **Prohibited** — no exceptions |
| FFI | Isolated to a dedicated `ffi` module in `infrastructure` |

```rust
// SAFETY: ptr is guaranteed non-null by the caller contract documented in X.
let value = unsafe { *ptr };
```

The `// SAFETY:` comment must explain:
1. What invariant guarantees this is safe
2. Where that invariant is established

---

## CI Pipeline

```yaml
# .github/workflows/ci.yml (reference)
jobs:
  quality:
    steps:
      - run: cargo fmt --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo deny check
      - run: cargo test --workspace
      - run: cargo sqlx prepare --check --workspace
```

Five enforced gates run on every PR. `cargo machete` and `cargo tarpaulin` are documented as optional local hardening checks.

---

## References

- [Clippy Lints](https://rust-lang.github.io/rust-clippy/master/)
- [`cargo deny`](https://embarkstudios.github.io/cargo-deny/)
- [`cargo tarpaulin`](https://github.com/xd009642/tarpaulin)
- [`cargo machete`](https://github.com/bnjbvr/cargo-machete)
