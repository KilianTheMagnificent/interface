//! Filesystem identity and containment helpers shared by the workspace
//! editor, project search, and customization inventory.
//!
//! Unix `MetadataExt` is imported unconditionally: this crate only builds on
//! Unix, so path separators are already `/`.

use std::fs::Metadata;
use std::path::Path;

pub(crate) use interface_common::fsid::FileIdentity;

pub(crate) fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity::of(metadata)
}

pub(crate) fn inside(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

pub(crate) fn relative_string(root: &Path, target: &Path) -> Option<String> {
    target.strip_prefix(root).ok()?.to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{inside, relative_string};
    use std::path::Path;

    #[test]
    fn containment_is_component_wise() {
        assert!(inside(Path::new("/work"), Path::new("/work")));
        assert!(inside(Path::new("/work"), Path::new("/work/src")));
        assert!(!inside(Path::new("/work"), Path::new("/work-secret")));
    }

    #[test]
    fn relative_string_keeps_utf8_slash_paths() {
        assert_eq!(
            relative_string(Path::new("/work"), Path::new("/work/src/app.ts")).as_deref(),
            Some("src/app.ts")
        );
        assert_eq!(
            relative_string(Path::new("/work"), Path::new("/other/app.ts")),
            None
        );
    }
}
