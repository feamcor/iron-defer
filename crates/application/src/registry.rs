//! Task handler registry — type-erased dispatch by `kind` string.
//!
//! Architecture references:
//! - §C4: `TaskHandler` is object-safe and uses **manual**
//!   `Pin<Box<dyn Future + Send>>` return types, NOT `#[async_trait]`. The
//!   per-task hot path runs once per claimed task, and we want explicit
//!   control over the boxing instead of relying on the macro to insert it.
//! - §Process Patterns (TaskRegistry ownership): `TaskRegistry::new` is
//!   constructed in `crates/api/src/lib.rs` ONLY. Other crates may hold a
//!   `&TaskRegistry` (or `Arc<TaskRegistry>`) injected from above, but
//!   never construct one.
//!
//! The bridge from `impl Task` (concrete user types with serde bounds) to
//! `Arc<dyn TaskHandler>` (object-safe, no serde bounds) lives in
//! `crates/api/src/lib.rs` as `TaskHandlerAdapter<T>` and is the only
//! permitted construction site for registered handlers.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use iron_defer_domain::{TaskContext, TaskError};
use tracing::warn;

/// Object-safe, type-erased task handler.
///
/// Stored in `TaskRegistry` keyed by `kind()`. The worker pool
/// looks up the handler for each claimed task and calls `execute(payload, ctx)`
/// to run it.
///
/// **Why manual `Pin<Box<dyn Future + Send>>` instead of `#[async_trait]`:**
/// Architecture §C4 mandates this shape. The trait is the per-task
/// hot path; explicit boxing makes the allocation visible and lets us swap
/// in a different strategy later (e.g. a custom executor) without churning
/// every implementor.
pub trait TaskHandler: Send + Sync {
    /// Stable kind discriminator for this handler. Used as the registry
    /// key. Must match the `KIND` constant of the `Task` impl that this
    /// handler bridges to.
    fn kind(&self) -> &'static str;

    /// Execute the task body with the given payload and runtime context.
    /// The default implementation deserializes the payload into the
    /// concrete `T: Task` type via the bridge in `TaskHandlerAdapter`.
    fn execute<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>>;
}

/// Map of `kind` string → `Arc<dyn TaskHandler>`.
///
/// **Construction is forbidden outside `crates/api/src/lib.rs`** (Architecture
/// §Process Patterns — TaskRegistry ownership). The application crate exposes the type so it can be
/// injected as `Arc<TaskRegistry>` into the worker pool and sweeper, but it
/// never instantiates one. Tests in this file use `TaskRegistry::new` only
/// to exercise the registration / lookup machinery — they never wire the
/// registry into a real engine.
pub struct TaskRegistry {
    handlers: HashMap<&'static str, Arc<dyn TaskHandler>>,
}

impl TaskRegistry {
    /// Construct an empty registry.
    ///
    /// **Constructed only in `crates/api/src/lib.rs`** (via
    /// `IronDeferBuilder::default()`). Other crates must receive a registry
    /// reference from above — never call this constructor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Insert a handler under its `kind()` string.
    ///
    /// If a handler is already registered under the same kind, the new
    /// one **overwrites** the existing entry (`HashMap` insert semantics).
    /// This matches the architecture's "`register::<T>()` is idempotent at
    /// the kind level" expectation — re-registering a kind with a
    /// different handler is treated as a deliberate replacement, not a
    /// silent error. A `tracing::warn!` is emitted on overwrite so an
    /// accidental duplicate-KIND bug at registration time is visible
    /// without changing the API contract.
    pub fn register(&mut self, handler: Arc<dyn TaskHandler>) {
        let kind = handler.kind();
        if self.handlers.contains_key(kind) {
            warn!(
                kind = kind,
                "task handler for kind already registered — overwriting previous registration"
            );
        }
        self.handlers.insert(kind, handler);
    }

    /// Look up a handler by `kind`.
    ///
    /// Returns `None` for unregistered kinds. The worker pool is
    /// responsible for turning a `None` lookup into a descriptive panic
    /// (architecture: "missing registration for a kind panics with a
    /// descriptive message — never silent task drop"). The registry
    /// itself never panics.
    #[must_use]
    pub fn get(&self, kind: &str) -> Option<&Arc<dyn TaskHandler>> {
        self.handlers.get(kind)
    }

    /// Number of registered handlers. Useful for tests and diagnostic
    /// logging at builder time.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// `true` if no handlers are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRegistry")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test handler — returns Ok regardless of input.
    struct MockHandler {
        kind: &'static str,
    }

    impl TaskHandler for MockHandler {
        fn kind(&self) -> &'static str {
            self.kind
        }

        fn execute<'a>(
            &'a self,
            _payload: &'a serde_json::Value,
            _ctx: &'a TaskContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = TaskRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("anything").is_none());
    }

    #[test]
    fn default_registry_is_empty() {
        let registry = TaskRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn register_then_get_returns_handler() {
        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler { kind: "echo" }));

        assert_eq!(registry.len(), 1);
        let handler = registry.get("echo").expect("handler registered");
        assert_eq!(handler.kind(), "echo");
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn register_overwrites_existing_kind() {
        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler { kind: "dup" }));
        registry.register(Arc::new(MockHandler { kind: "dup" }));

        // HashMap insert semantics: still one entry under "dup".
        assert_eq!(registry.len(), 1);
        assert!(registry.get("dup").is_some());
    }

    /// P1-UNIT-004 — `get()` returns `None` for unregistered kinds. The
    /// worker pool is responsible for turning this into a descriptive panic;
    /// the registry itself never panics on lookup miss.
    #[test]
    fn get_returns_none_for_unregistered_kind() {
        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler { kind: "registered" }));
        assert!(
            registry.get("not_registered").is_none(),
            "unregistered kind must return None"
        );
        assert!(registry.get("").is_none(), "empty kind must return None");
    }

    #[test]
    fn debug_impl_lists_registered_kinds() {
        let mut registry = TaskRegistry::new();
        registry.register(Arc::new(MockHandler { kind: "a" }));
        registry.register(Arc::new(MockHandler { kind: "b" }));
        let debug = format!("{registry:?}");
        assert!(debug.contains("TaskRegistry"));
        assert!(debug.contains('a'));
        assert!(debug.contains('b'));
    }
}
