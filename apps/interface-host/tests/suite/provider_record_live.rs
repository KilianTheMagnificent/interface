//! Records the provider replay fixtures from the real CLIs (live, opt-in).
//!
//! Each scenario runs the same code the replay tests run
//! (`provider_replay.rs`), with `provider-record` standing at the provider's
//! path in front of the installed CLI: the cheapest model, the shortest
//! prompts, a fresh temp directory outside any project. The scrubbed
//! transcript is written to `$INTERFACE_RECORD_DIR/<engine>/<scenario>.json`
//! (point it at `crates/provider-replay/fixtures`), and the raw journal is
//! deleted. A scenario that cannot be produced is retried once, then left
//! unrecorded: a transcript is never written by hand.
//!
//! Spends a little provider usage. Run from a plain shell (Claude Code's
//! nesting variables turn transcript saving off, so a resume would fail):
//!
//! `INTERFACE_LIVE_PROBES=1 INTERFACE_RECORD_DIR=$PWD/crates/provider-replay/fixtures cargo test -p interface-host --test suite provider_record_live -- --ignored --nocapture --test-threads=1`
//!
//! `INTERFACE_RECORD_ONLY=claude/simple` (comma-separated) records a subset.
//! `INTERFACE_RECORD_KEEP_JOURNAL=<private dir>` keeps each raw journal there
//! instead of deleting it, so `refinish_replay_fixtures` can scrub it again
//! after a scrubber fix without recording again; delete that directory once
//! the fixtures are clean.
#![cfg(unix)]

use std::{
    env,
    os::unix::fs::DirBuilderExt as _,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::FutureExt as _;
use provider_replay::{Identity, Meta};

use super::provider_replay::{Engine, Provider, SCENARIOS, run_scenario};

const ENABLE: &str = "INTERFACE_LIVE_PROBES";
const RECORD_DIR: &str = "INTERFACE_RECORD_DIR";
const ONLY: &str = "INTERFACE_RECORD_ONLY";
const KEEP: &str = "INTERFACE_RECORD_KEEP_JOURNAL";
/// Bound on one wait against a real provider.
const LIVE_LIMIT: Duration = Duration::from_secs(180);
/// Claude Code stamps its child processes with these (`agent_gate_live.rs`).
const CLAUDE_NESTING_MARKERS: [&str; 9] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];
/// A fixture must stay below this.
const MAX_FIXTURE_BYTES: u64 = 200 * 1024;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider recording: set INTERFACE_LIVE_PROBES=1 and INTERFACE_RECORD_DIR (spends Claude quota)"]
async fn record_claude_replay_fixtures() {
    record(Engine::Claude, &SCENARIOS).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider recording: set INTERFACE_LIVE_PROBES=1 and INTERFACE_RECORD_DIR (spends Codex quota)"]
async fn record_codex_replay_fixtures() {
    record(Engine::Codex, &SCENARIOS).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider recording: set INTERFACE_LIVE_PROBES=1 and INTERFACE_RECORD_DIR (spends Grok quota)"]
async fn record_grok_replay_fixtures() {
    record(Engine::Grok, &["simple"]).await;
}

/// Scrubs every kept journal again into `$INTERFACE_RECORD_DIR`. Spends
/// nothing: no provider runs.
#[test]
#[ignore = "re-scrubs kept recordings: set INTERFACE_RECORD_KEEP_JOURNAL and INTERFACE_RECORD_DIR"]
fn refinish_replay_fixtures() {
    let (Some(keep), Some(directory)) = (env::var_os(KEEP), env::var_os(RECORD_DIR)) else {
        println!("skipped; set {KEEP} and {RECORD_DIR}");
        return;
    };
    let (keep, directory) = (PathBuf::from(keep), PathBuf::from(directory));
    for engine in std::fs::read_dir(&keep).expect("kept journals").flatten() {
        for entry in std::fs::read_dir(engine.path())
            .expect("engine folder")
            .flatten()
        {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(scenario) = name.strip_suffix(".meta.json") else {
                continue;
            };
            let meta: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("meta")).expect("meta");
            let field = |key: &str| meta[key].as_str().unwrap_or_default().to_owned();
            let meta = Meta {
                engine: field("engine"),
                protocol: field("protocol"),
                cli_version: field("cliVersion"),
                scenario: scenario.to_owned(),
                recorded_at: field("recordedAt"),
            };
            let out = engine.path().join(format!("{scenario}.json"));
            provider_replay::finish(&out, &meta, &Identity::current()).expect("finish");
            let destination = directory.join(engine.file_name());
            std::fs::create_dir_all(&destination).expect("fixture folder");
            std::fs::copy(&out, destination.join(format!("{scenario}.json"))).expect("copy");
            println!("{}/{scenario}: scrubbed again", meta.engine);
        }
    }
}

async fn record(engine: Engine, scenarios: &[&str]) {
    if env::var(ENABLE).as_deref() != Ok("1") {
        println!("{}: skipped; set {ENABLE}=1 to record", engine.folder());
        return;
    }
    let Some(directory) = env::var_os(RECORD_DIR).map(PathBuf::from) else {
        println!("{}: skipped; set {RECORD_DIR}", engine.folder());
        return;
    };
    if engine == Engine::Claude {
        let set: Vec<&str> = CLAUDE_NESTING_MARKERS
            .iter()
            .copied()
            .filter(|name| env::var_os(name).is_some())
            .collect();
        assert!(
            set.is_empty(),
            "record Claude outside Claude Code: {set:?} are set (prefix the command with \
             `env -u` for each)"
        );
    }
    let target = real_cli(engine);
    let version = provider_replay::record::cli_version(&target);
    println!(
        "{}: recording with {} ({version})",
        engine.folder(),
        target.display()
    );
    let only: Vec<String> = env::var(ONLY)
        .map(|only| only.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut outcomes = Vec::new();
    for scenario in scenarios {
        let name = format!("{}/{scenario}", engine.folder());
        if !only.is_empty() && !only.contains(&name) {
            continue;
        }
        let mut recorded = None;
        for attempt in 1..=2 {
            match record_once(engine, scenario, &target, &version, &directory).await {
                Ok(path) => {
                    recorded = Some(path);
                    break;
                }
                Err(why) => println!("{name}: attempt {attempt} not recorded: {why}"),
            }
        }
        outcomes.push(match recorded {
            Some(path) => format!("{name}: recorded {}", path.display()),
            None => format!("{name}: NOT recorded (two attempts failed)"),
        });
    }
    println!("{}", outcomes.join("\n"));
}

/// One recording of `scenario` into `directory`, or why it could not be made.
async fn record_once(
    engine: Engine,
    scenario: &str,
    target: &Path,
    version: &str,
    directory: &Path,
) -> Result<PathBuf, String> {
    let staging = tempfile::tempdir().map_err(|error| error.to_string())?;
    let out = std::fs::canonicalize(staging.path())
        .map_err(|error| error.to_string())?
        .join(format!("{scenario}.json"));
    let provider = Provider::Record {
        target: target.to_path_buf(),
        out: out.clone(),
    };
    let run = AssertUnwindSafe(run_scenario(engine, scenario, &provider, LIVE_LIMIT))
        .catch_unwind()
        .await;
    let journal = provider_replay::journal_directory(&out);
    let result = match run {
        Err(panic) => Err(panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_else(|| "the scenario panicked".to_owned())),
        Ok(teardown) => {
            let stderr = teardown.stderr;
            if !stderr.trim().is_empty() {
                println!("{}/{scenario}: recorder stderr:\n{stderr}", engine.folder());
            }
            let meta = Meta {
                engine: engine.folder().to_owned(),
                protocol: provider_replay::record::protocol_of(engine.folder()).to_owned(),
                cli_version: version.to_owned(),
                scenario: scenario.to_owned(),
                recorded_at: provider_replay::record::today(),
            };
            provider_replay::finish(&out, &meta, &Identity::current()).and_then(|_| {
                let size = std::fs::metadata(&out)
                    .map_err(|error| error.to_string())?
                    .len();
                if size > MAX_FIXTURE_BYTES {
                    return Err(format!("the transcript is {size} bytes, over the limit"));
                }
                let destination = directory.join(engine.folder());
                std::fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
                let path = destination.join(format!("{scenario}.json"));
                std::fs::copy(&out, &path).map_err(|error| error.to_string())?;
                Ok(path)
            })
        }
    };
    // The raw journal holds the unscrubbed session: kept only in the
    // owner-private directory asked for, else deleted.
    match env::var_os(KEEP).map(PathBuf::from) {
        Some(keep) if result.is_ok() => {
            let kept = keep.join(engine.folder());
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&kept)
                .map_err(|error| error.to_string())?;
            let _ = std::fs::remove_dir_all(kept.join(format!("{scenario}.json.parts")));
            std::fs::rename(&journal, kept.join(format!("{scenario}.json.parts")))
                .map_err(|error| error.to_string())?;
            let meta = serde_json::json!({
                "engine": engine.folder(),
                "protocol": provider_replay::record::protocol_of(engine.folder()),
                "cliVersion": version,
                "recordedAt": provider_replay::record::today(),
            });
            std::fs::write(kept.join(format!("{scenario}.meta.json")), meta.to_string())
                .map_err(|error| error.to_string())?;
        }
        _ => {
            let _ = std::fs::remove_dir_all(&journal);
        }
    }
    result
}

/// The installed CLI, found the way the host finds it.
fn real_cli(engine: Engine) -> PathBuf {
    let home = PathBuf::from(env::var_os("HOME").expect("HOME"));
    let (variable, name, known) = match engine {
        Engine::Claude => (
            "CLAUDE_CODE_EXECUTABLE",
            "claude",
            vec![
                home.join(".local/bin/claude"),
                home.join(".claude/local/claude"),
            ],
        ),
        Engine::Codex => (
            "CODEX_EXECUTABLE",
            "codex",
            vec![home.join(".local/bin/codex")],
        ),
        Engine::Grok => (
            "GROK_EXECUTABLE",
            "grok",
            vec![home.join(".grok/bin/grok"), home.join(".local/bin/grok")],
        ),
    };
    env::var_os(variable)
        .map(PathBuf::from)
        .into_iter()
        .chain(env::var_os("PATH").into_iter().flat_map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(name))
                .collect::<Vec<_>>()
        }))
        .chain(known)
        .chain([
            PathBuf::from(format!("/opt/homebrew/bin/{name}")),
            PathBuf::from(format!("/usr/local/bin/{name}")),
        ])
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
        .unwrap_or_else(|| panic!("{name} is not installed (set {variable})"))
}
