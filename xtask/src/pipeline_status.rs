//! Read back each handoff independently; a recorded step is not a live state.
use crate::{
    gate, gate_cache, pipeline_state,
    promotion_run::{Phase, Run},
};
use anyhow::Result;
use serde_json::{Value, json};
use std::{path::Path, process::Command};

pub fn status(root: &Path, json_output: bool, fetch: bool) -> Result<()> {
    if fetch {
        gate::git_capture(root, &["fetch", "origin"])?;
    }
    let state = pipeline_state::location(root)?;
    let dev = gate::rev_parse(root, "origin/dev")?;
    let main = gate::rev_parse(root, "origin/main")?;
    let landing: Option<Value> = pipeline_state::read(&state.join("landing.json"))?;
    let instance: Option<Value> = pipeline_state::read(&state.join("dev.json"))?;
    let running = instance.as_ref().is_some_and(instance_matches);
    let checks = match gate_cache::subject(root) {
        Ok(subject) => {
            let covered = gate_cache::covered(root, &subject)?;
            json!({"identity": {"rust": subject.rust, "electron": subject.js},
                "rust": covered.rust, "electron": covered.js})
        }
        Err(error) => json!({"rust": false, "electron": false, "unavailable": error.to_string()}),
    };
    let promotion: Option<Run> = pipeline_state::read(&state.join("promotion.json"))?;
    let feed = crate::release::default_feed_target()?;
    let feed_version = crate::promote::manifest_version(&feed)?;
    let artifact = promotion.as_ref().and_then(|run| run.package.as_ref()).map(|receipt| {
        let valid = crate::artifact_receipt::validate(receipt, &receipt.commit, &receipt.version);
        json!({"revision": receipt.commit, "version": receipt.version, "verified": valid.is_ok(),
            "error": valid.err().map(|error| error.to_string()), "verifiedAt": receipt.verified_at})
    });
    // A feed that holds this version with other bytes is an error, not a
    // mismatch: promote refuses to replace it, so resuming cannot finish.
    let feed_check = promotion
        .as_ref()
        .and_then(|run| run.package.as_ref())
        .map(|receipt| crate::artifact_receipt::feed_matches(receipt, &feed));
    let feed_matches = matches!(feed_check, Some(Ok(true)));
    let feed_error = feed_check
        .and_then(Result::err)
        .map(|error| error.to_string());
    let feed_state = if feed_error.is_some() {
        Feed::Conflict
    } else if feed_matches {
        Feed::Matches
    } else {
        Feed::Other
    };
    let next = next_action(
        running,
        instance
            .as_ref()
            .and_then(|v| v.get("revision"))
            .and_then(Value::as_str)
            == Some(&dev),
        promotion.as_ref().map(|run| run.phase),
        feed_state,
    );
    let value = json!({"schema": 1, "refsRefreshed": fetch, "dev": dev, "main": main,
        "landing": landing, "devInstance": {"runningVerifiedRevision": running, "lastVerification": instance},
        "checksForThisSource": checks, "package": artifact, "promotion": promotion,
        "feed": {"version": feed_version, "matchesCandidate": feed_matches, "error": feed_error},
        "nextAction": next});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    render(&value);
    Ok(())
}

fn render(value: &Value) {
    let short = |value: &Value| value.as_str().map_or("unknown", gate::short).to_owned();
    println!(
        "pipeline: dev {} | main {}{}",
        short(&value["dev"]),
        short(&value["main"]),
        if value["refsRefreshed"] == true {
            ""
        } else {
            " (locally recorded refs; --fetch refreshes)"
        }
    );
    let instance = &value["devInstance"];
    println!(
        "  Dev app: {} {}",
        if instance["runningVerifiedRevision"] == true {
            "running verified revision"
        } else {
            "unverified or stopped"
        },
        short(&instance["lastVerification"]["revision"])
    );
    println!(
        "  Checks for this source: Rust {}, Electron {}",
        value["checksForThisSource"]["rust"], value["checksForThisSource"]["electron"]
    );
    let run = &value["promotion"];
    if run.is_null() {
        println!("  Release: none prepared by this pipeline");
    } else {
        println!(
            "  Release {}: {} | package verified {}",
            run["version"], run["phase"], value["package"]["verified"]
        );
        if let Some(timings) = run["timings_ms"].as_object() {
            for (phase, ms) in timings {
                println!(
                    "    {}: {:.1}s",
                    timing_label(phase, timings.contains_key(Phase::Checked.as_str())),
                    std::time::Duration::from_millis(ms.as_u64().unwrap_or(0)).as_secs_f64()
                );
            }
        }
        if !run["last_error"].is_null() {
            println!("  Last failure: {}", run["last_error"]);
        }
    }
    println!(
        "  Feed: {} | matches candidate: {}",
        value["feed"]["version"], value["feed"]["matchesCandidate"]
    );
    if let Some(error) = value["feed"]["error"].as_str() {
        println!("  Feed error: {error}");
    }
    println!(
        "  Next: {}",
        value["nextAction"]
            .as_str()
            .unwrap_or("inspect status --json")
    );
}

/// How a promote phase's time reads. `packaged` is timed from before the
/// checkout through signing and smoke, so when promote ran the gate there its
/// time includes the `checked` phase; the record keeps both keys as written.
fn timing_label(phase: &str, checked: bool) -> String {
    if phase == Phase::Packaged.as_str() && checked {
        "packaged (includes checked)".to_owned()
    } else {
        phase.to_owned()
    }
}

fn instance_matches(record: &Value) -> bool {
    let Some(pid) = record.get("pid").and_then(Value::as_u64) else {
        return false;
    };
    let Some(checkout) = record.get("checkout").and_then(Value::as_str) else {
        return false;
    };
    let current = gate::rev_parse(Path::new(checkout), "HEAD").ok();
    current.as_deref() == record.get("revision").and_then(Value::as_str)
        && gate::tree_is_clean(Path::new(checkout)).unwrap_or(false)
        && Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout)
                        .contains("Interface Dev.app/Contents/MacOS/Electron")
            })
}

/// What the update feed holds, against the prepared candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Feed {
    /// Exactly the candidate's verified bytes.
    Matches,
    /// Nothing, or another version.
    Other,
    /// The candidate's version with other bytes, or bytes that fail their
    /// checksums: promote refuses to replace either.
    Conflict,
}

fn next_action(
    running: bool,
    current: bool,
    promotion_phase: Option<Phase>,
    feed: Feed,
) -> &'static str {
    if let Some(phase) = promotion_phase {
        if feed == Feed::Conflict {
            return "inspect the update feed (feed.error): promote refuses to replace what it holds, so resuming cannot stage this candidate";
        }
        if phase != Phase::Staged || feed != Feed::Matches {
            return "resume the original promote command; its approved candidate and completed work are retained";
        }
    }
    if !running || !current {
        return "cargo xtask dev-restart, then test the verified Dev revision";
    }
    "review Dev; promote the approved revision or named landings when ready"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_time_says_it_includes_the_gate() {
        assert_eq!(
            timing_label("packaged", true),
            "packaged (includes checked)"
        );
        assert_eq!(timing_label("packaged", false), "packaged");
        assert_eq!(timing_label("checked", true), "checked");
    }

    #[test]
    fn next_action_names_a_feed_conflict_instead_of_resuming_promote() {
        let resume = "resume the original promote command";
        assert!(next_action(true, true, Some(Phase::Packaged), Feed::Other).starts_with(resume));
        assert!(next_action(true, true, Some(Phase::Staged), Feed::Other).starts_with(resume));
        for phase in [Phase::Packaged, Phase::Staged] {
            assert!(
                next_action(true, true, Some(phase), Feed::Conflict)
                    .starts_with("inspect the update feed"),
                "{phase}"
            );
        }
        assert!(
            next_action(true, true, Some(Phase::Staged), Feed::Matches).starts_with("review Dev")
        );
        assert!(next_action(false, true, None, Feed::Other).starts_with("cargo xtask dev-restart"));
        assert!(next_action(true, true, None, Feed::Other).starts_with("review Dev"));
    }
}
