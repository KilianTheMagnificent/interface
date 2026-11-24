//! Pinned on-disk bytes of the collaboration snapshot stores (tests only).
//! The files under `tests/golden/` are the formats already on owners' disks:
//! a refactor that changes one byte of them fails here.
use std::path::Path;

/// Asserts `file` is owner-private (0600) and byte-identical to
/// `tests/golden/<name>`, with the test's temporary `root` spelled `<ROOT>`
/// and each `(value, placeholder)` in `masks` (clock readings, random ids)
/// replaced by its placeholder.
pub(crate) fn assert_golden(name: &str, root: &Path, file: &Path, masks: &[(&str, &str)]) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o600, "{name} must stay owner-private");
    let mut actual = String::from_utf8(std::fs::read(file).unwrap()).unwrap();
    for spelling in [root.canonicalize().unwrap(), root.to_path_buf()] {
        actual = actual.replace(spelling.to_str().unwrap(), "<ROOT>");
    }
    for (value, placeholder) in masks {
        actual = actual.replace(value, placeholder);
    }
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let expected = std::fs::read_to_string(&golden)
        .unwrap_or_else(|error| panic!("missing golden {}: {error}", golden.display()));
    assert_eq!(actual, expected, "{name} bytes changed");
}
