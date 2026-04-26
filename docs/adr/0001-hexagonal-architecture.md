# ADR-0001: Hexagonal Architecture and SOLID Principles

**Status:** Accepted
**Date:** 2026-04-02

---

## Context

iron-defer is a distributed system for at-least-once execution of critical background tasks. It requires:

- Clear separation between domain logic and infrastructure concerns
- Testability at every layer without requiring live infrastructure
- Flexibility to swap adapters (e.g., different databases, message brokers) without touching business logic
- Long-term maintainability across a growing codebase

## Decision

We adopt **Hexagonal Architecture** (Ports and Adapters) as the foundational structural pattern, organized as a **Cargo workspace** with distinct crates per layer.

## Workspace Layout

```
iron-defer/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── domain/                 # Layer 1: pure domain
│   ├── application/            # Layer 2: use cases
│   ├── infrastructure/         # Layer 3: adapters
│   └── api/                    # Layer 4: entry points
└── migrations/                 # sqlx migrations
```

### Layer Rules

| Layer | Allowed Dependencies | Forbidden |
|-------|---------------------|-----------|
| `domain` | std, core Rust crates only | async runtimes, HTTP, DB, framework types |
| `application` | `domain` | direct DB/HTTP calls |
| `infrastructure` | `domain`, `application`, all external crates | business logic |
| `api` | all crates | anything except wiring |

Violations are detectable via `cargo tree` and enforced in code review.

## Ports and Adapters

### Ports (Traits in `domain` or `application`)

Ports are Rust traits representing capabilities the application needs:

```rust
// crates/application/src/ports/task_repository.rs
use async_trait::async_trait;
use crate::domain::{Task, TaskId, TaskError};

#[async_trait]
pub trait TaskRepository: Send + Sync {
    async fn find_by_id(&self, id: &TaskId) -> Result<Task, TaskError>;
    async fn save(&self, task: &Task) -> Result<(), TaskError>;
    async fn list_pending(&self) -> Result<Vec<Task>, TaskError>;
}
```

- Ports live in `application` or `domain`, never in `infrastructure`
- Traits must be `Send + Sync` for async use
- Use `Arc<dyn Port>` for runtime injection into application state

### Adapters (Implementations in `infrastructure`)

```rust
// crates/infrastructure/src/adapters/postgres_task_repository.rs
pub struct PostgresTaskRepository {
    pool: sqlx::PgPool,
}

#[async_trait]
impl TaskRepository for PostgresTaskRepository {
    async fn find_by_id(&self, id: &TaskId) -> Result<Task, TaskError> {
        // sqlx query, row -> domain mapping
    }
}
```

Adapters:
- Implement traits defined in `application`
- Handle all translation between domain types and external representations
- Are the only layer that imports external crate types (sqlx rows, HTTP bodies, etc.)

## SOLID Principles Mapping

### Single Responsibility
Each crate and each module has one reason to change. `domain` changes when business rules change. `infrastructure` changes when external systems change. They never change for each other's reasons.

### Open/Closed
Extend behavior by adding new trait implementations, not by modifying existing ones. New storage backends = new struct implementing the repository trait.

### Liskov Substitution
All trait implementations must be substitutable. Tests use mock or in-memory implementations; production uses real adapters. Both must satisfy the trait contract fully.

### Interface Segregation
Keep traits focused. Prefer many small traits over one large trait:

```rust
// Prefer this:
pub trait TaskReader: Send + Sync {
    async fn find_by_id(&self, id: &TaskId) -> Result<Task, TaskError>;
}

pub trait TaskWriter: Send + Sync {
    async fn save(&self, task: &Task) -> Result<(), TaskError>;
}

// Over this:
pub trait TaskRepository: Send + Sync {
    async fn find_by_id(...);
    async fn save(...);
    async fn delete(...);
    async fn list(...);
    async fn count(...);
    // ...many more
}
```

### Dependency Inversion
High-level modules (`application`) depend on abstractions (traits). Low-level modules (`infrastructure`) depend on the same abstractions. Wiring happens at application startup in `api`.

```rust
// api/src/main.rs — dependency wiring
let pool = PgPool::connect(&config.database_url).await?;
let task_repo: Arc<dyn TaskRepository> = Arc::new(PostgresTaskRepository::new(pool));
let task_service = TaskService::new(task_repo);
```

## Testing Strategy

| Layer | Test Type | Infrastructure Needed |
|-------|-----------|----------------------|
| `domain` | unit | none |
| `application` | unit with mock adapters | none |
| `infrastructure` | integration | real DB via testcontainers |
| `api` | integration / E2E | full stack |

The port abstraction is what makes unit testing the application layer possible without a real database.

## Consequences

**Positive:**
- Domain logic is pure and fast to test
- Infrastructure can be swapped without touching business rules
- Clear boundaries prevent accidental coupling
- Dependency direction is always inward (toward domain)

**Negative:**
- More boilerplate for simple CRUD — accepted trade-off for long-term maintainability
- Requires discipline to keep layers honest — enforced by crate boundaries in Cargo

## References

- [Hexagonal Architecture — Alistair Cockburn](https://alistair.cockburn.us/hexagonal-architecture/)
- [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/)
- [The Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
