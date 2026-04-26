//! Worker identity newtype.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a worker process within the iron-defer engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerId(Uuid);

impl WorkerId {
    /// Generate a fresh random worker identifier.
    ///
    /// `Default` is intentionally NOT implemented: identifiers should be
    /// created explicitly so callers do not silently allocate randomness
    /// via `..Default::default()` patterns.
    #[must_use]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Construct a `WorkerId` from a pre-existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Borrow the inner UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Active worker status derived from running tasks grouped by `claimed_by`.
///
/// Workers don't register themselves in the database — a running task with
/// a `claimed_by` UUID represents an active worker. This struct aggregates
/// per-worker, per-queue task counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerStatus {
    pub worker_id: WorkerId,
    pub queue: super::queue::QueueName,
    pub tasks_in_flight: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_ids_are_unique() {
        let a = WorkerId::new();
        let b = WorkerId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn worker_id_round_trips_through_serde() {
        let id = WorkerId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let parsed: WorkerId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, parsed);
    }
}
