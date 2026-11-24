//! Cosmetic foreground identity. Deliberately does not authenticate chat IDs.

use interface_contracts::terminal::TerminalLaunchProfileId;

#[cfg(unix)]
pub(crate) fn read(
    group: u32,
) -> Result<Option<TerminalLaunchProfileId>, crate::TerminalBackendError> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let unavailable =
        || crate::TerminalBackendError::new("foreground program names were unavailable");
    let mut child = crate::PsProbe {
        columns: &["comm"],
        select: &["-ww", "-g", &group.to_string()],
        utc: false,
        stderr: false,
    }
    .command()
    .stdout(Stdio::piped())
    .spawn()
    .map_err(|_| unavailable())?;
    let stdout = child.stdout.take().ok_or_else(unavailable)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err(unavailable());
            }
        }
    }
    let bytes = reader
        .join()
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
    if bytes.len() > 16 * 1024 {
        return Err(unavailable());
    }
    Ok(profile_from_names(&String::from_utf8_lossy(&bytes)))
}

/// The first wait before re-reading an unchanged foreground job.
#[cfg(unix)]
const FIRST_RECHECK: std::time::Duration = std::time::Duration::from_secs(1);
/// The longest wait while a launcher may still exec a provider. A launcher
/// such as `npx codex` execs the provider inside the same process group, so
/// an unchanged group is re-read this often for [`LAUNCHER_WINDOW`], and for
/// as long as it runs an agent.
#[cfg(unix)]
const MAX_RECHECK: std::time::Duration = std::time::Duration::from_secs(4);
/// How long an unchanged job without an agent stays on [`MAX_RECHECK`].
#[cfg(unix)]
const LAUNCHER_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
/// The longest wait for a job that has run a minute without an agent: a dev
/// server, an editor or `tail -f` stops costing a `ps` every few seconds.
#[cfg(unix)]
const IDLE_MAX_RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

/// Remembers the last foreground job's profile so the once-a-second shell
/// display poll does not spawn `ps` for a job that is still in front. A new
/// foreground process group is always read at once; an unchanged one is
/// re-read after a wait that doubles up to [`MAX_RECHECK`], and up to
/// [`IDLE_MAX_RECHECK`] once it has shown no agent for [`LAUNCHER_WINDOW`].
#[cfg(unix)]
#[derive(Debug, Default)]
pub(crate) struct ForegroundProfileCache {
    entry: Option<CachedForeground>,
}

#[cfg(unix)]
#[derive(Debug)]
struct CachedForeground {
    group: u32,
    profile: Option<TerminalLaunchProfileId>,
    /// When this group, or its current profile, was first read.
    since: std::time::Instant,
    next_read: std::time::Instant,
    wait: std::time::Duration,
}

#[cfg(unix)]
impl ForegroundProfileCache {
    /// The profile of foreground `group`: remembered while its re-read is not
    /// yet due, otherwise from `read`. A failed read is not remembered.
    pub(crate) fn profile_or_read<E>(
        &mut self,
        group: u32,
        now: std::time::Instant,
        read: impl FnOnce() -> Result<Option<TerminalLaunchProfileId>, E>,
    ) -> Result<Option<TerminalLaunchProfileId>, E> {
        let previous = self.entry.as_ref().filter(|entry| entry.group == group);
        if let Some(entry) = previous
            && now < entry.next_read
        {
            return Ok(entry.profile);
        }
        let profile = read()?;
        let (since, wait) = previous.map_or((now, FIRST_RECHECK), |entry| {
            // A changed profile restarts the launcher window.
            let since = if entry.profile == profile {
                entry.since
            } else {
                now
            };
            let cap = if profile.is_none() && now.duration_since(since) >= LAUNCHER_WINDOW {
                IDLE_MAX_RECHECK
            } else {
                MAX_RECHECK
            };
            (since, (entry.wait * 2).min(cap))
        });
        self.entry = Some(CachedForeground {
            group,
            profile,
            since,
            next_read: now + wait,
            wait,
        });
        Ok(profile)
    }

    /// Forgets the job once the shell itself is back in front.
    pub(crate) fn clear(&mut self) {
        self.entry = None;
    }
}

fn profile_from_names(names: &str) -> Option<TerminalLaunchProfileId> {
    let mut found = None;
    for name in names.lines() {
        let path = std::path::Path::new(name.trim());
        let profile = match path.file_name()?.to_str()? {
            "codex" => TerminalLaunchProfileId::Codex,
            "claude" => TerminalLaunchProfileId::Claude,
            // npm's OpenCode shim links to `opencode.exe`, and the kernel
            // reports whichever name was executed.
            "opencode" | "opencode.exe" => TerminalLaunchProfileId::Opencode,
            "grok" => TerminalLaunchProfileId::Grok,
            "omp" => TerminalLaunchProfileId::Omp,
            "agy" => TerminalLaunchProfileId::Gemini,
            // Muse Code's installed `muse` launcher execs the native
            // `muse-bin-<version>` beside it.
            "muse" => TerminalLaunchProfileId::Muse,
            native if muse_native_binary(native) => TerminalLaunchProfileId::Muse,
            // Kimi Code ships one native `kimi` and retitles its process
            // `kimi-code`; accept either name `ps` may report.
            "kimi" | "kimi-code" => TerminalLaunchProfileId::Kimi,
            version if claude_version_path(path, version) => TerminalLaunchProfileId::Claude,
            _ => continue,
        };
        if found.is_some_and(|previous| previous != profile) {
            return None;
        }
        found = Some(profile);
    }
    found
}

fn muse_native_binary(name: &str) -> bool {
    name.strip_prefix("muse-bin-")
        .and_then(|version| version.chars().next())
        .is_some_and(|first| first.is_ascii_digit())
}

fn claude_version_path(path: &std::path::Path, version: &str) -> bool {
    // Claude's native installer executes claude/versions/2.1.270 directly, so
    // the kernel reports a version as the basename. This is display identity
    // only; a provider receipt still owns every resumable conversation ID.
    path.parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|name| name == "versions")
        && path
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::file_name)
            .is_some_and(|name| name == "claude")
        && version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn an_unchanged_foreground_job_is_reread_with_a_capped_backoff() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut cache = ForegroundProfileCache::default();
        let mut reads = Vec::new();
        let mut poll = |cache: &mut ForegroundProfileCache, group, second, profile| {
            cache
                .profile_or_read(group, at(second), || {
                    reads.push((group, second));
                    Ok::<_, ()>(profile)
                })
                .unwrap()
        };

        // A job polled every second is read at 0, 1, 3, 7, 11 and 15 s: the
        // wait doubles from one second and stops at four.
        for second in 0..=16 {
            assert_eq!(poll(&mut cache, 7, second, None), None);
        }
        // The remembered profile is served until the re-read is due.
        let codex = Some(TerminalLaunchProfileId::Codex);
        assert_eq!(poll(&mut cache, 7, 19, codex), codex);
        assert_eq!(poll(&mut cache, 7, 20, None), codex);
        // A different foreground group is read at once and restarts the wait.
        assert_eq!(poll(&mut cache, 8, 21, None), None);
        assert_eq!(poll(&mut cache, 8, 21, codex), None);
        assert_eq!(poll(&mut cache, 8, 22, codex), codex);
        // Once the shell is back in front, the next job is read at once.
        cache.clear();
        assert_eq!(poll(&mut cache, 8, 22, None), None);
        assert_eq!(
            reads,
            [
                (7, 0),
                (7, 1),
                (7, 3),
                (7, 7),
                (7, 11),
                (7, 15),
                (7, 19),
                (8, 21),
                (8, 22),
                (8, 22),
            ]
        );

        // A failed read is not remembered.
        assert_eq!(
            cache.profile_or_read(9, at(30), || Err::<Option<_>, _>("busy")),
            Err("busy")
        );
        assert_eq!(
            cache.profile_or_read(9, at(30), || Ok::<_, ()>(codex)),
            Ok(codex)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_job_without_an_agent_backs_off_to_thirty_seconds_after_its_first_minute() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut cache = ForegroundProfileCache::default();
        let mut reads = Vec::new();
        let mut poll = |cache: &mut ForegroundProfileCache, group, second, profile| {
            cache
                .profile_or_read(group, at(second), || {
                    reads.push((group, second));
                    Ok::<_, ()>(profile)
                })
                .unwrap()
        };

        // Every 4 s for the first minute, then 8, 16 and at most 30 s.
        for second in 0..=180 {
            assert_eq!(poll(&mut cache, 7, second, None), None);
        }
        // An agent appearing in the same group is found at the next read and
        // keeps it on 4 s: it may exit and leave its launcher in front.
        let codex = Some(TerminalLaunchProfileId::Codex);
        for second in 181..=219 {
            poll(&mut cache, 7, second, codex);
        }
        // So does a job without an agent for its first minute after that.
        for second in 220..=290 {
            poll(&mut cache, 7, second, None);
        }
        // A different foreground group is read at once and starts over.
        for second in 291..=294 {
            poll(&mut cache, 8, second, None);
        }
        let mut expected = vec![0, 1, 3];
        expected.extend((7..=59).step_by(4));
        expected.extend([63, 71, 87, 117, 147, 177]);
        expected.extend((207..=283).step_by(4));
        let mut expected = expected
            .into_iter()
            .map(|second| (7, second))
            .collect::<Vec<_>>();
        expected.extend([(8, 291), (8, 292), (8, 294)]);
        assert_eq!(reads, expected);
    }

    #[test]
    fn recognizes_native_children_of_node_launchers_without_reading_argv() {
        assert_eq!(
            profile_from_names("/opt/bin/node\n/opt/codex/vendor/bin/codex\n"),
            Some(TerminalLaunchProfileId::Codex)
        );
        assert_eq!(
            profile_from_names("claude\n"),
            Some(TerminalLaunchProfileId::Claude)
        );
        for (names, profile) in [
            ("opencode", TerminalLaunchProfileId::Opencode),
            (
                "/lib/node_modules/opencode-ai/bin/opencode.exe",
                TerminalLaunchProfileId::Opencode,
            ),
            ("/Users/test/.grok/bin/grok", TerminalLaunchProfileId::Grok),
            ("omp", TerminalLaunchProfileId::Omp),
            ("agy", TerminalLaunchProfileId::Gemini),
            ("/Users/test/.local/bin/muse", TerminalLaunchProfileId::Muse),
            (
                "/Users/test/.local/bin/muse-bin-1.4.6-R2",
                TerminalLaunchProfileId::Muse,
            ),
            (
                "/Users/test/.kimi-code/bin/kimi",
                TerminalLaunchProfileId::Kimi,
            ),
            ("kimi-code", TerminalLaunchProfileId::Kimi),
        ] {
            assert_eq!(profile_from_names(names), Some(profile));
        }
        assert_eq!(
            profile_from_names("/Users/test/.local/share/claude/versions/2.1.270\n"),
            Some(TerminalLaunchProfileId::Claude)
        );
        for names in [
            "/bin/zsh",
            "/bin/sleep",
            "codex-helper",
            "codex\nclaude",
            "/other/versions/2.1.270",
            "/claude/versions/current",
            "/claude/versions/2..270",
            "codex\n/claude/versions/2.1.270",
            "muse-bin-",
            "muse-bin-current",
            "kimi-legacy",
            "",
        ] {
            assert_eq!(profile_from_names(names), None);
        }
    }
}
