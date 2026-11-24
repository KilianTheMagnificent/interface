use std::path::{Path, PathBuf};

use interface_engine::{ExecutableResolution, ResolvedExecutable, resolve_executable};

pub const CLAUDE_EXECUTABLE_ENV: &str = "CLAUDE_CODE_EXECUTABLE";

/// Resolve override, login-shell result, then known install paths. The path
/// found is kept as given, never canonicalized: the updater repoints
/// `~/.local/bin/claude` at each new version and prunes the old ones, so a
/// link's target goes stale while the link stays valid.
pub fn resolve_claude_executable(
    home: &Path,
    explicit_override: Option<PathBuf>,
    login_shell_path: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedExecutable> {
    let resolution = ExecutableResolution {
        explicit_override,
        login_shell_path,
        known_locations: vec![
            home.join(".local/bin/claude"),
            home.join(".claude/local/claude"),
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ],
        require_absolute: true,
    };
    resolve_executable(&resolution, exists)
}

#[cfg(test)]
mod tests {
    use interface_engine::ExecutableSource;

    use super::*;

    #[test]
    fn resolver_takes_the_override_then_login_shell_then_known_locations_in_order() {
        let home = Path::new("/Users/test");
        let resolved = resolve_claude_executable(
            home,
            Some("relative/claude".into()),
            Some("/shell/claude".into()),
            |path| {
                matches!(
                    path.to_str(),
                    Some("relative/claude" | "/Users/test/.claude/local/claude")
                )
            },
        )
        .unwrap();
        assert_eq!(
            resolved.path,
            PathBuf::from("/Users/test/.claude/local/claude")
        );
        assert_eq!(resolved.source, ExecutableSource::KnownLocation);
    }
}
