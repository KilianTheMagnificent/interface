//! The version Interface's Rust binaries report.

/// The build variable `cargo xtask package-mac` sets on the release build.
pub const PRODUCT_VERSION_VARIABLE: &str = "INTERFACE_PRODUCT_VERSION";

/// Expands, in the crate that invokes it, to the version that crate's binary
/// reports: the [`PRODUCT_VERSION_VARIABLE`] build variable when set, else the
/// invoking crate's version.
///
/// `package-mac` sets the variable on the release build to the Electron app's
/// version (`electron/package.json`, which `promote` bumps); every other build
/// (tests, the gate, Dev) reports the crate version. Cargo rebuilds every
/// target that reads the variable, and everything that depends on it, when
/// it changes, so only the two binaries' own targets invoke this: iface and
/// interface-host's `main.rs` (not its library, which iface also links). The
/// host library and the engines are handed the value at runtime and stay
/// cached across releases.
#[macro_export]
macro_rules! product_version {
    () => {{
        const VERSION: &str = match option_env!("INTERFACE_PRODUCT_VERSION") {
            Some(version) => version,
            None => env!("CARGO_PKG_VERSION"),
        };
        const _: () = assert!(
            $crate::version::is_product_version(VERSION),
            "INTERFACE_PRODUCT_VERSION must be a short version token such as 0.3.140"
        );
        VERSION
    }};
}

/// Whether `version` is a non-empty version token of at most 64 ASCII
/// letters, digits, `.`, `-` and `+` that starts with a digit, so a stray
/// build variable can never put text of any other shape on the wire.
#[must_use]
pub const fn is_product_version(version: &str) -> bool {
    let bytes = version.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 || !bytes[0].is_ascii_digit() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+')) {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reported_version_is_the_stamp_or_else_the_crate_version() {
        assert_eq!(
            crate::product_version!(),
            option_env!("INTERFACE_PRODUCT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(PRODUCT_VERSION_VARIABLE, "INTERFACE_PRODUCT_VERSION");
    }

    #[test]
    fn only_version_tokens_can_be_stamped() {
        for version in ["0.3.10", "0.3.140", "1.0.0-beta.2", "1.0.0+build.7"] {
            assert!(is_product_version(version), "{version}");
        }
        for version in [
            "",
            "v0.3.140",
            "0.3.140 ",
            "0.3.140\n",
            "0.3/140",
            "0.3.140\"}",
            &"9".repeat(65),
        ] {
            assert!(!is_product_version(version), "{version:?}");
        }
    }
}
