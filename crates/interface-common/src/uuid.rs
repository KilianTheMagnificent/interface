//! Canonical UUID text checks shared by the host and the terminal crates.
//!
//! Every identifier these paths mint or accept is the lower-case hyphenated
//! form, so the check is a byte shape, not a parse: no crate here needs the
//! `uuid` dependency to refuse an upper-case or braced spelling.

/// `value` is a lower-case, hyphenated RFC 4122 UUID whose version nibble is
/// `version` (`b'4'` for the ids the renderer and Claude mint, `b'7'` for
/// Codex thread ids).
#[must_use]
pub fn canonical_uuid(value: &str, version: u8) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes[14] == version
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) || matches!(byte, b'0'..=b'9' | b'a'..=b'f')
        })
}

/// A canonical version-4 UUID.
#[must_use]
pub fn canonical_uuid_v4(value: &str) -> bool {
    canonical_uuid(value, b'4')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte check must agree with the `uuid` crate's reading of a
    /// version-4, RFC 4122 id in its canonical hyphenated spelling: that is
    /// the predicate the host used to compute with the crate.
    fn crate_says_canonical_v4(value: &str) -> bool {
        uuid::Uuid::parse_str(value).is_ok_and(|parsed| {
            parsed.get_version() == Some(uuid::Version::Random)
                && parsed.get_variant() == uuid::Variant::RFC4122
                && parsed.hyphenated().to_string() == value
        })
    }

    #[test]
    fn byte_check_matches_the_uuid_crate() {
        let samples = [
            "0f8b4c2e-9c3a-4d7e-8f1a-2b3c4d5e6f70",
            "0F8B4C2E-9C3A-4D7E-8F1A-2B3C4D5E6F70",
            "0f8b4c2e-9c3a-4d7e-cf1a-2b3c4d5e6f70",
            "0f8b4c2e-9c3a-7d7e-8f1a-2b3c4d5e6f70",
            "{0f8b4c2e-9c3a-4d7e-8f1a-2b3c4d5e6f70}",
            "0f8b4c2e9c3a4d7e8f1a2b3c4d5e6f70",
            "0f8b4c2e-9c3a-4d7e-8f1a-2b3c4d5e6f7",
            "",
            "0f8b4c2e-9c3a-4d7e-8f1a-2b3c4d5e6f7g",
        ];
        for sample in samples {
            assert_eq!(
                canonical_uuid_v4(sample),
                crate_says_canonical_v4(sample),
                "{sample:?}"
            );
        }
        for _ in 0..64 {
            let minted = uuid::Uuid::new_v4().to_string();
            assert!(canonical_uuid_v4(&minted), "{minted}");
            assert!(!canonical_uuid(&minted, b'7'), "{minted}");
        }
    }

    #[test]
    fn version_nibble_selects_the_family() {
        let v7 = "0192f1a6-3b4c-7d5e-9f6a-7b8c9d0e1f2a";
        assert!(canonical_uuid(v7, b'7'));
        assert!(!canonical_uuid_v4(v7));
    }
}
