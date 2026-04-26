//! Validated task-kind newtype.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

/// A validated task kind discriminator.
///
/// Construction goes through [`TaskKind::try_from`] to enforce the
/// invariant: the kind string must be non-empty. This matches the
/// SQL `CHECK (kind <> '')` constraint in migration 0001 and the
/// `Task::KIND` associated constant contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskKind(String);

impl TaskKind {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl TryFrom<&str> for TaskKind {
    type Error = ValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(ValidationError::EmptyTaskKind);
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for TaskKind {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(ValidationError::EmptyTaskKind);
        }
        Ok(Self(value))
    }
}

impl From<TaskKind> for String {
    fn from(kind: TaskKind) -> Self {
        kind.0
    }
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TaskKind {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for TaskKind {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for TaskKind {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_kind_accepted() {
        let kind = TaskKind::try_from("payment_webhook").unwrap();
        assert_eq!(kind.as_str(), "payment_webhook");
    }

    #[test]
    fn empty_kind_rejected() {
        let err = TaskKind::try_from("").unwrap_err();
        assert!(matches!(err, ValidationError::EmptyTaskKind));
    }

    #[test]
    fn serde_round_trips() {
        let kind = TaskKind::try_from("echo").unwrap();
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"echo\"");
        let back: TaskKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn display_matches_inner() {
        let kind = TaskKind::try_from("my_task").unwrap();
        assert_eq!(format!("{kind}"), "my_task");
    }

    #[test]
    fn partial_eq_str() {
        let kind = TaskKind::try_from("echo").unwrap();
        assert!(kind == "echo");
        assert!(kind != "other");
    }
}
