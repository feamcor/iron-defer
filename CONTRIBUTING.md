# Contributing to iron-defer

This document describes the standards, tooling, and conventions required for contributing to **iron-defer**. All contributors — human and AI — are expected to follow these guidelines.

---

## Toolchain Requirements

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | stable (MSRV in `Cargo.toml`) | Language toolchain |
| `rustfmt` | bundled with toolchain | Code formatting |
| `clippy` | bundled with toolchain | Linting |
| `cargo-deny` | latest | Supply chain and advisory checks |
| `sqlx-cli` | latest | Database migrations |

Install all tools:

```bash
rustup component add rustfmt clippy
cargo install cargo-deny sqlx-cli
```

## Code Standards

All code in this repository must comply with:

- **Microsoft Rust Guidelines** — idiomatic Rust patterns and API design
- **Clippy** — `cargo clippy --workspace --all-targets -- -D warnings`
- **rustfmt** — formatting checked in CI; run `cargo fmt` before committing
- **OWASP Top 10** — security-relevant patterns enforced at code review
- **12-Factor App** — configuration sourced from environment, never hardcoded

---

## Architecture Decision Records

Key architectural decisions are documented as ADRs in [`docs/adr/`](docs/adr/):

| ADR | Decision |
|-----|---------|
| [ADR-0001](docs/adr/0001-hexagonal-architecture.md) | Hexagonal Architecture + SOLID Principles |
| [ADR-0002](docs/adr/0002-error-handling.md) | Error Handling Strategy |
| [ADR-0003](docs/adr/0003-configuration-management.md) | Configuration Management |
| [ADR-0004](docs/adr/0004-async-runtime-tokio-ecosystem.md) | Async Runtime and Tokio Ecosystem |
| [ADR-0005](docs/adr/0005-database-layer-sqlx.md) | Database Layer with SQLx |
| [ADR-0006](docs/adr/0006-serialization-serde.md) | Serialization with Serde |

Read the relevant ADR before making changes to a subsystem.

---

## Guidelines

Detailed how-to guides are in [`docs/guidelines/`](docs/guidelines/):

| Guide | Topic |
|-------|-------|
| [rust-idioms.md](docs/guidelines/rust-idioms.md) | Newtype pattern, traits, generics, builders |
| [quality-gates.md](docs/guidelines/quality-gates.md) | CI gates, clippy, coverage, unsafe policy |
| [security.md](docs/guidelines/security.md) | OWASP, supply chain, secret handling |

---

## CI Quality Gates

Every pull request must pass all gates. No exceptions.

```bash
cargo fmt --check                          # formatting
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                           # bans + advisories + licenses
cargo test --workspace                     # all tests
cargo sqlx prepare --check --workspace     # offline query cache
```

---

## Commit and PR Standards

- Commits: imperative mood, present tense (`Add`, `Fix`, `Remove`)
- PRs: one logical change per PR
- New behavior must include tests; untested code is not mergeable
- All `unsafe` blocks must include a `// SAFETY:` comment explaining the invariant

---

## Project Structure

```
iron-defer/
├── Cargo.toml              # workspace root
├── crates/
│   ├── domain/             # pure domain logic, no I/O
│   ├── application/        # use cases, orchestration
│   ├── infrastructure/     # DB, HTTP clients, external adapters
│   └── api/                # axum HTTP server, CLI entry points
├── docs/
│   ├── adr/                # Architecture Decision Records
│   └── guidelines/         # How-to guides
└── migrations/             # sqlx migrations
```

See [ADR-0001](docs/adr/0001-hexagonal-architecture.md) for the rationale behind this layout.
