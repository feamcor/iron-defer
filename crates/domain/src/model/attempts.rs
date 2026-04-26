//! Attempt count and max-attempts newtypes.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

/// Number of times a task has been claimed. Always >= 0.
///
/// Wraps `i32` to match the Postgres `INTEGER` column type.
/// Construction via `TryFrom` rejects negative values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "i32", into = "i32")]
pub struct AttemptCount(i32);

impl AttemptCount {
    pub const ZERO: Self = Self(0);

    /// # Errors
    ///
    /// Returns `Err` if `value` is negative.
    pub const fn new(value: i32) -> Result<Self, ValidationError> {
        if value < 0 {
            return Err(ValidationError::NegativeAttemptCount { value });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl Default for AttemptCount {
    fn default() -> Self {
        Self::ZERO
    }
}

impl TryFrom<i32> for AttemptCount {
    type Error = ValidationError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AttemptCount> for i32 {
    fn from(a: AttemptCount) -> Self {
        a.0
    }
}

impl std::fmt::Display for AttemptCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Maximum number of attempts before a task is terminally failed. Always >= 1.
///
/// Wraps `i32` to match the Postgres `INTEGER` column type.
/// Construction via `TryFrom` rejects values < 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "i32", into = "i32")]
pub struct MaxAttempts(i32);

impl MaxAttempts {
    pub const DEFAULT: Self = Self(3);

    /// # Errors
    ///
    /// Returns `Err` if `value` is less than 1.
    pub const fn new(value: i32) -> Result<Self, ValidationError> {
        if value < 1 {
            return Err(ValidationError::InvalidMaxAttempts { value });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl Default for MaxAttempts {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<i32> for MaxAttempts {
    type Error = ValidationError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MaxAttempts> for i32 {
    fn from(m: MaxAttempts) -> Self {
        m.0
    }
}

impl std::fmt::Display for MaxAttempts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_count_zero_ok() {
        assert_eq!(AttemptCount::new(0).unwrap().get(), 0);
    }

    #[test]
    fn attempt_count_negative_rejected() {
        assert!(AttemptCount::new(-1).is_err());
    }

    #[test]
    fn attempt_count_serde_round_trips() {
        let a = AttemptCount::new(5).unwrap();
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, "5");
        let back: AttemptCount = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn max_attempts_one_ok() {
        assert_eq!(MaxAttempts::new(1).unwrap().get(), 1);
    }

    #[test]
    fn max_attempts_zero_rejected() {
        assert!(MaxAttempts::new(0).is_err());
    }

    #[test]
    fn max_attempts_negative_rejected() {
        assert!(MaxAttempts::new(-1).is_err());
    }

    #[test]
    fn max_attempts_default_is_three() {
        assert_eq!(MaxAttempts::default().get(), 3);
    }

    #[test]
    fn attempt_count_ordering() {
        assert!(AttemptCount::new(2).unwrap() > AttemptCount::new(1).unwrap());
    }

    #[test]
    fn attempt_vs_max_comparison() {
        let attempts = AttemptCount::new(3).unwrap();
        let max = MaxAttempts::new(3).unwrap();
        assert_eq!(attempts.get(), max.get());
    }
}
