use std::{collections::HashSet, fmt::Display, hash::Hash};

use chrono::{DateTime, Utc};

/// Frozen millisecond-UTC form that `validate_timestamp` accepts.
pub const CANONICAL_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    pub at: String,
    pub reason: String,
}

impl ValidationError {
    pub(crate) fn new(at: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            at: at.into(),
            reason: reason.into(),
        }
    }

    /// Prefixes `self.at` with `prefix`, inserting `.` unless `at` is empty or an index.
    #[must_use]
    pub(crate) fn under(mut self, prefix: &str) -> Self {
        self.at = join_path(prefix, &self.at);
        self
    }

    /// Prepends `[i]` without a leading dot so array paths compose as `field[i]`.
    #[must_use]
    pub(crate) fn at_index(mut self, index: usize) -> Self {
        self.at = if self.at.is_empty() {
            format!("[{index}]")
        } else if self.at.starts_with('[') {
            format!("[{index}]{}", self.at)
        } else {
            format!("[{index}].{}", self.at)
        };
        self
    }
}

fn join_path(prefix: &str, at: &str) -> String {
    if at.is_empty() {
        prefix.to_owned()
    } else if at.starts_with('[') {
        format!("{prefix}{at}")
    } else {
        format!("{prefix}.{at}")
    }
}

impl Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.at, self.reason)
    }
}

impl std::error::Error for ValidationError {}

/// Formats `at` with the same frozen millisecond-UTC form `validate_timestamp` accepts.
#[must_use]
pub fn canonical_timestamp(at: DateTime<Utc>) -> String {
    at.format(CANONICAL_TIMESTAMP_FORMAT).to_string()
}

pub(crate) fn bounded_text(
    value: &str,
    at: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<(), ValidationError> {
    let forbidden_control = value
        .chars()
        .any(|character| matches!(character as u32, 0..=8 | 11 | 12 | 14..=31 | 127..=159));
    if forbidden_control || (!allow_empty && value.trim().is_empty()) || value.len() > max_bytes {
        return Err(ValidationError::new(
            at,
            format!(
                "must be {} bounded normalized text",
                if allow_empty { "" } else { "non-empty" }
            )
            .trim()
            .to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn bounded_trimmed(
    value: &str,
    at: &str,
    max_bytes: usize,
) -> Result<(), ValidationError> {
    bounded_text(value, at, max_bytes, false)?;
    if value.trim() != value {
        return Err(ValidationError::new(
            at,
            "must not have surrounding whitespace",
        ));
    }
    Ok(())
}

/// Matches `YYYY-MM-DDTHH:MM:SS.mmmZ` exactly: a fixed 24 bytes with a
/// four-digit year, uppercase `T`, three fraction digits and a literal `Z`.
fn canonical_timestamp_shape(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 24
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'.',
            23 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
        })
}

/// Parses `value` only when it is exactly the frozen canonical form
/// [`canonical_timestamp`] prints.
///
/// The cheap fixed-shape check runs first. Any value of that shape that
/// parses as RFC 3339 renders back to itself through
/// [`CANONICAL_TIMESTAMP_FORMAT`] (zero-padded fields, exactly three fraction
/// digits, `Z` is UTC, a leap second renders as `60`), so no round-trip is
/// needed to accept exactly the canonical set.
#[must_use]
pub fn parse_canonical_timestamp(value: &str) -> Option<DateTime<Utc>> {
    if !canonical_timestamp_shape(value) {
        return None;
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

pub(crate) fn validate_timestamp(value: &str, at: &str) -> Result<(), ValidationError> {
    if parse_canonical_timestamp(value).is_some() {
        Ok(())
    } else {
        Err(ValidationError::new(
            at,
            "must be a canonical ISO timestamp",
        ))
    }
}

pub(crate) fn windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

pub(crate) fn validate_absolute_path(
    value: &str,
    at: &str,
    max_bytes: usize,
    allow_unc: bool,
    reject_dot_segments: bool,
) -> Result<(), ValidationError> {
    bounded_trimmed(value, at, max_bytes)?;
    let drive = windows_absolute(value);
    let unc = allow_unc
        && value.starts_with("\\\\")
        && value[2..]
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .count()
            >= 2;
    if !value.starts_with('/') && !drive && !unc {
        return Err(ValidationError::new(at, "must be an absolute path"));
    }
    if reject_dot_segments
        && value
            .split(['/', '\\'])
            .any(|part| matches!(part, "." | ".."))
    {
        return Err(ValidationError::new(
            at,
            "must not contain dot path segments",
        ));
    }
    Ok(())
}

pub(crate) fn ensure_unique<T>(
    values: impl IntoIterator<Item = T>,
    at: &str,
) -> Result<(), ValidationError>
where
    T: Eq + Hash,
{
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(ValidationError::new(at, "must contain unique values"));
        }
    }
    Ok(())
}

/// `value` is exactly `bytes` bytes written as lowercase hex.
#[must_use]
pub fn lowercase_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes.saturating_mul(2)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn validate_digest(value: &str, at: &str) -> Result<(), ValidationError> {
    if lowercase_hex(value, 32) {
        Ok(())
    } else {
        Err(ValidationError::new(
            at,
            "must be a lowercase SHA-256 digest",
        ))
    }
}

pub(crate) fn validate_identifier(
    value: &str,
    at: &str,
    max_bytes: usize,
) -> Result<(), ValidationError> {
    bounded_trimmed(value, at, max_bytes)?;
    if value
        .bytes()
        .any(|byte| matches!(byte, b'\t' | b'\n' | b'\r'))
    {
        return Err(ValidationError::new(
            at,
            "must not contain control characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{SecondsFormat, TimeZone, Timelike};

    #[test]
    fn canonical_timestamp_format_is_the_frozen_millis_z_form() {
        assert_eq!(CANONICAL_TIMESTAMP_FORMAT, "%Y-%m-%dT%H:%M:%S%.3fZ");
    }

    #[test]
    fn canonical_timestamp_matches_rfc3339_millis_z() {
        let samples = [
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
                .unwrap()
                .with_nanosecond(0)
                .unwrap(),
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
                .unwrap()
                .with_nanosecond(1_000_000)
                .unwrap(),
            Utc.with_ymd_and_hms(2016, 12, 31, 23, 59, 59)
                .unwrap()
                .with_nanosecond(1_000_000_000)
                .unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 1, 12, 34, 56)
                .unwrap()
                .with_nanosecond(0)
                .unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 1, 12, 34, 56)
                .unwrap()
                .with_nanosecond(999_000_000)
                .unwrap(),
            // Sub-millisecond digits truncate rather than round in both renderings.
            Utc.with_ymd_and_hms(2026, 9, 1, 12, 34, 56)
                .unwrap()
                .with_nanosecond(999_999_999)
                .unwrap(),
        ];
        for sample in samples {
            assert_eq!(
                canonical_timestamp(sample),
                sample.to_rfc3339_opts(SecondsFormat::Millis, true)
            );
            assert!(validate_timestamp(&canonical_timestamp(sample), "at").is_ok());
        }
    }

    #[test]
    fn timestamp_gate_keeps_the_canonical_accept_reject_set() {
        for value in [
            "2024-01-01T00:00:00.000Z",
            "2016-12-31T23:59:60.000Z",
            "9999-12-31T23:59:59.999Z",
            "0000-01-01T00:00:00.000Z",
            "2024-02-29T12:00:00.000Z",
        ] {
            assert!(validate_timestamp(value, "at").is_ok(), "{value}");
        }
        for value in [
            "2023-02-29T12:00:00.000Z",
            "2024-01-01T00:00:00.000Z extra",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00.0000Z",
            "2024-01-01T00:00:00.000+00:00",
            "2024-01-01T00:00:00.000-05:00",
            "2024-01-01t00:00:00.000z",
            "2024-01-01T00:00:00.000Z ",
            "2024-13-01T00:00:00.000Z",
            "not a timestamp",
            "",
        ] {
            assert!(validate_timestamp(value, "at").is_err(), "{value}");
        }
    }

    #[test]
    fn parsed_timestamps_accept_exactly_what_the_round_trip_copies_accepted() {
        // The collaboration and iface stores each parsed, re-rendered and
        // compared before this parser was shared.
        fn round_trip(value: &str) -> Option<DateTime<Utc>> {
            let parsed = DateTime::parse_from_rfc3339(value)
                .ok()?
                .with_timezone(&Utc);
            (canonical_timestamp(parsed) == value).then_some(parsed)
        }
        for value in [
            "2024-01-01T00:00:00.000Z",
            "2016-12-31T23:59:60.000Z",
            "9999-12-31T23:59:59.999Z",
            "0000-01-01T00:00:00.000Z",
            "2024-02-29T12:00:00.000Z",
            "2023-02-29T12:00:00.000Z",
            "2024-01-01T00:00:00.000Z extra",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00.0000Z",
            "2024-01-01T00:00:00.000+00:00",
            "2024-01-01T00:00:00.000-05:00",
            "2024-01-01t00:00:00.000z",
            "2024-01-01 00:00:00.000Z",
            "2024-01-01T00:00:00.000Z ",
            "2024-13-01T00:00:00.000Z",
            "2026-09-27T05:56:12.123456789+00:00",
            "not a timestamp",
            "",
        ] {
            assert_eq!(
                parse_canonical_timestamp(value),
                round_trip(value),
                "{value}"
            );
            assert_eq!(
                validate_timestamp(value, "at").is_ok(),
                round_trip(value).is_some(),
                "{value}"
            );
        }
        let now = Utc::now();
        let text = canonical_timestamp(now);
        assert_eq!(
            parse_canonical_timestamp(&text).map(canonical_timestamp),
            Some(text)
        );
    }

    #[test]
    fn digest_gate_is_lowercase_hex_of_32_bytes() {
        assert!(validate_digest(&"a".repeat(64), "digest").is_ok());
        assert!(validate_digest(&"0123456789abcdef".repeat(4), "digest").is_ok());
        for value in [
            "A".repeat(64),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
        ] {
            assert!(validate_digest(&value, "digest").is_err(), "{value}");
        }
        assert!(lowercase_hex("", 0));
        assert!(lowercase_hex("00ff", 2));
        assert!(!lowercase_hex("00FF", 2));
    }

    #[test]
    fn validation_error_path_joins_indexes_without_extra_dots() {
        let nested = ValidationError::new("reason", "x")
            .at_index(0)
            .under("capabilities")
            .under("snapshot.agents");
        assert_eq!(nested.at, "snapshot.agents.capabilities[0].reason");
        let array = ValidationError::new("", "must be a stable identifier")
            .at_index(1)
            .under("dependencyIds")
            .under("plan.tasks[3]");
        assert_eq!(array.at, "plan.tasks[3].dependencyIds[1]");
    }

    #[test]
    fn identifier_rejects_tab_lf_cr_after_bounded_text() {
        assert!(validate_identifier("ok", "id", 32).is_ok());
        assert!(validate_identifier("bad\tid", "id", 32).is_err());
        assert!(validate_identifier("bad\nid", "id", 32).is_err());
        assert!(validate_identifier("bad\rid", "id", 32).is_err());
    }

    #[test]
    fn absolute_path_rejects_dot_segments_on_either_separator() {
        assert!(validate_absolute_path("/tmp/ok", "path", 64, false, true).is_ok());
        assert!(validate_absolute_path("/tmp/./ok", "path", 64, false, true).is_err());
        assert!(validate_absolute_path("/tmp/../ok", "path", 64, false, true).is_err());
        assert!(validate_absolute_path(r"C:\tmp\..\ok", "path", 64, false, true).is_err());
        assert!(validate_absolute_path(r"/tmp\.\ok", "path", 64, false, true).is_err());
    }
}
