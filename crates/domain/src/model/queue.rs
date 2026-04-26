//! Validated queue-name newtype.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;

/// A validated queue name.
///
/// Construction goes through [`QueueName::try_from`] to enforce invariants:
/// non-empty, no whitespace, no control or zero-width characters, length
/// at most [`QueueName::MAX_LEN`] bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct QueueName(String);

impl QueueName {
    /// Maximum queue name length in bytes. Chosen to fit comfortably in
    /// log fields, metric labels, and Postgres B-tree index keys without
    /// imposing a Postgres-specific limit.
    pub const MAX_LEN: usize = 128;

    /// Borrow the inner queue name as a `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype, returning the inner `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    fn validate(value: &str) -> Result<(), ValidationError> {
        if value.is_empty() {
            return Err(ValidationError::EmptyQueueName);
        }
        if value.len() > Self::MAX_LEN {
            return Err(ValidationError::QueueNameTooLong {
                length: value.len(),
                max: Self::MAX_LEN,
            });
        }
        for c in value.chars() {
            if c.is_whitespace() {
                return Err(ValidationError::QueueNameWhitespace {
                    value: value.to_owned(),
                });
            }
            // Reject ASCII NUL, all control characters (C0/C1), and the
            // common zero-width / bidi format characters that produce
            // visually-indistinguishable queue names.
            if c == '\0'
                || c.is_control()
                || matches!(
                    c,
                    '\u{200B}' // ZERO WIDTH SPACE
                    | '\u{200C}' // ZWNJ
                    | '\u{200D}' // ZWJ
                    | '\u{2060}' // WORD JOINER
                    | '\u{FEFF}' // BOM / ZWNBSP
                    | '\u{00AD}' // SOFT HYPHEN
                    | '\u{202A}'..='\u{202E}' // bidi embedding/override
                    | '\u{2066}'..='\u{2069}' // bidi isolate
                )
            {
                return Err(ValidationError::QueueNameForbiddenChar {
                    value: value.to_owned(),
                });
            }
        }
        Ok(())
    }
}

impl TryFrom<&str> for QueueName {
    type Error = ValidationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::validate(value)?;
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for QueueName {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::validate(&value)?;
        Ok(Self(value))
    }
}

impl From<QueueName> for String {
    fn from(name: QueueName) -> Self {
        name.0
    }
}

impl std::fmt::Display for QueueName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Statistics for a single queue (or queue+region pair), as returned by the `GET /queues` endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueStatistics {
    pub queue: QueueName,
    pub region: Option<String>,
    pub pending: u64,
    pub running: u64,
    pub suspended: u64,
    pub active_workers: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_queue_name_is_rejected() {
        let err = QueueName::try_from("").unwrap_err();
        assert!(matches!(err, ValidationError::EmptyQueueName));
    }

    #[test]
    fn whitespace_in_queue_name_is_rejected() {
        let err = QueueName::try_from(" foo").unwrap_err();
        assert!(matches!(err, ValidationError::QueueNameWhitespace { .. }));

        let err = QueueName::try_from("foo bar").unwrap_err();
        assert!(matches!(err, ValidationError::QueueNameWhitespace { .. }));
    }

    #[test]
    fn valid_queue_name_round_trips() {
        let name = QueueName::try_from("payments").expect("valid");
        assert_eq!(name.as_str(), "payments");
        assert_eq!(String::from(name), "payments");
    }

    #[test]
    fn queue_name_serde_round_trip() {
        let name = QueueName::try_from("notifications").expect("valid");
        let json = serde_json::to_string(&name).expect("serialize");
        assert_eq!(json, "\"notifications\"");
        let parsed: QueueName = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(name, parsed);
    }

    #[test]
    fn queue_name_serde_rejects_invalid() {
        let result: Result<QueueName, _> = serde_json::from_str("\"\"");
        assert!(result.is_err());
        let result: Result<QueueName, _> = serde_json::from_str("\" leading\"");
        assert!(result.is_err());
    }

    #[test]
    fn nul_byte_is_rejected() {
        let err = QueueName::try_from("foo\0bar").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::QueueNameForbiddenChar { .. }
        ));
    }

    #[test]
    fn control_characters_are_rejected() {
        let err = QueueName::try_from("foo\x01bar").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::QueueNameForbiddenChar { .. }
        ));
    }

    #[test]
    fn zero_width_characters_are_rejected() {
        let err = QueueName::try_from("foo\u{200B}bar").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::QueueNameForbiddenChar { .. }
        ));
        let err = QueueName::try_from("foo\u{FEFF}bar").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::QueueNameForbiddenChar { .. }
        ));
    }

    #[test]
    fn bidi_override_is_rejected() {
        let err = QueueName::try_from("foo\u{202E}evil").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::QueueNameForbiddenChar { .. }
        ));
    }

    #[test]
    fn unicode_whitespace_is_rejected() {
        // NBSP (U+00A0) and ideographic space (U+3000) are caught by
        // is_whitespace, not by the forbidden-char branch.
        let err = QueueName::try_from("foo\u{00A0}bar").unwrap_err();
        assert!(matches!(err, ValidationError::QueueNameWhitespace { .. }));
        let err = QueueName::try_from("\u{3000}foo").unwrap_err();
        assert!(matches!(err, ValidationError::QueueNameWhitespace { .. }));
    }

    #[test]
    fn over_length_queue_name_is_rejected() {
        let too_long = "a".repeat(QueueName::MAX_LEN + 1);
        let err = QueueName::try_from(too_long.as_str()).unwrap_err();
        assert!(matches!(err, ValidationError::QueueNameTooLong { .. }));
    }

    #[test]
    fn max_length_queue_name_is_accepted() {
        let just_right = "a".repeat(QueueName::MAX_LEN);
        let name = QueueName::try_from(just_right.as_str()).expect("valid");
        assert_eq!(name.as_str().len(), QueueName::MAX_LEN);
    }
}
