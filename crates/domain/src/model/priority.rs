//! Task priority newtype.

use serde::{Deserialize, Serialize};

/// Task execution priority. Higher values are claimed first.
///
/// Wraps `i16` to match the Postgres `SMALLINT` column type.
/// No validation — the full `i16` range is semantically valid
/// (negative priorities are lower than default 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Priority(i16);

impl Priority {
    pub const DEFAULT: Self = Self(0);

    #[must_use]
    pub const fn new(value: i16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> i16 {
        self.0
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl From<i16> for Priority {
    fn from(value: i16) -> Self {
        Self(value)
    }
}

impl From<Priority> for i16 {
    fn from(p: Priority) -> Self {
        p.0
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zero() {
        assert_eq!(Priority::default().get(), 0);
    }

    #[test]
    fn ordering_higher_is_greater() {
        assert!(Priority::new(10) > Priority::new(5));
        assert!(Priority::new(-1) < Priority::new(0));
    }

    #[test]
    fn serde_round_trips() {
        let p = Priority::new(42);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "42");
        let back: Priority = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
