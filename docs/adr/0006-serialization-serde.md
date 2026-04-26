# ADR-0006: Serialization with Serde

**Status:** Accepted
**Date:** 2026-04-02

---

## Context

iron-defer serializes and deserializes data across multiple boundaries:
- HTTP request/response bodies (JSON via axum)
- Database JSON columns (task payloads)
- Configuration files (TOML via figment)
- Internal message formats

Inconsistent use of `serde` attributes leads to subtle bugs (field name mismatches, unexpected field rejection, missing data), and security issues (accepting unexpected input).

## Decision

We standardize on `serde` for all serialization. Attribute conventions differ by use case — API structs, config structs, and domain structs have different requirements.

## Struct Categories and Rules

### API Response Structs (outbound JSON)

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub payload: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

Rules:
- `rename_all = "camelCase"` — REST API convention
- `skip_serializing_if = "Option::is_none"` — omit nulls in responses
- `Serialize` only (not `Deserialize`) — responses are never deserialized from user input
- Do not use `deny_unknown_fields` — forward compatibility requires accepting new fields from updated clients

### API Request Structs (inbound JSON)

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub payload: serde_json::Value,
    #[serde(default)]
    pub scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

Rules:
- `rename_all = "camelCase"` — match API convention
- Do **not** use `deny_unknown_fields` on public API request types — clients may send extra fields
- `#[serde(default)]` for optional fields rather than `Option<T>` when a sensible default exists
- Validate business constraints after deserialization, not via serde attributes

### Configuration Structs

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
}

fn default_connect_timeout() -> u64 { 5 }
```

Rules:
- `deny_unknown_fields` — catch configuration typos at startup
- `default` functions for optional config with sensible fallbacks
- `Deserialize` only (config is never serialized back out)

### Domain Structs with Newtype Wrappers

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(Uuid);
```

- `#[serde(transparent)]` on newtypes — serializes as the inner type, not as `{ "0": "..." }`
- Implement `Display` and `FromStr` separately for non-serde contexts

### Enum Serialization

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}
```

Rules:
- `rename_all = "snake_case"` for status/type enums in JSON
- For externally-tagged enums with data, use explicit `tag` and `content`:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum TaskResult {
    Success { output: serde_json::Value },
    Failure { error: String, retryable: bool },
}
```

## Security Considerations

### Untrusted Input

For structs that receive data from untrusted sources (external HTTP requests, message queues):

1. Deserialize into a validated request struct
2. Validate business rules explicitly after deserialization
3. Never pass raw `serde_json::Value` into domain logic — always extract and validate fields

```rust
// Adapter layer — never let unvalidated input into domain
let request: CreateTaskRequest = Json::from_request(req, &state).await?;
let command = CreateTaskCommand::try_from(request)?;  // validates here
task_service.create(command).await?;
```

### Large Payload Protection

Always apply body size limits in axum — do not deserialize unbounded input:

```rust
use axum::extract::DefaultBodyLimit;

let app = Router::new()
    .route("/tasks", post(create_task))
    .layer(DefaultBodyLimit::max(1_024 * 1_024));  // 1 MiB limit
```

## Custom Deserializers

When standard attributes are insufficient, implement `Deserialize` manually or use `#[serde(deserialize_with = "...")]`:

```rust
fn deserialize_non_empty_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Err(serde::de::Error::custom("field must not be empty"));
    }
    Ok(s)
}

#[derive(Deserialize)]
pub struct CreateTaskRequest {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub name: String,
}
```

## Feature Gate for Serde in Library Crates

Domain crates should make `serde` optional via feature flag when practical:

```toml
# crates/domain/Cargo.toml
[features]
serde = ["dep:serde"]

[dependencies]
serde = { version = "1", features = ["derive"], optional = true }
```

```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TaskId(Uuid);
```

This keeps the domain crate usable as a pure library without the serde dependency.

## Consequences

**Positive:**
- Consistent serialization conventions across all API endpoints
- `deny_unknown_fields` on config catches typos early
- `transparent` on newtypes preserves ergonomic JSON format
- Security rules enforced at the adapter boundary

**Negative:**
- Per-category conventions require discipline and code review attention
- Custom deserializers add boilerplate — accepted when standard attributes are insufficient

## References

- [serde documentation](https://serde.rs)
- [serde attributes reference](https://serde.rs/attributes.html)
- [OWASP — Mass Assignment](https://cheatsheetseries.owasp.org/cheatsheets/Mass_Assignment_Cheat_Sheet.html)
