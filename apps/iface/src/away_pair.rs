//! Opt-in packet-backed pairing experiment. Production delegation stays disabled.
use crate::{
    away_cycle,
    errors::{CLI_EXIT, CommandResult},
};
use interface_collaboration::{
    away::{Progress, digest},
    away_cycle::{Config, Status, Store},
    delegates::DelegateRunner,
};
use interface_host::CliDelegate;
use interface_storage::{
    JsonLimits, PrivateDirectory, TargetSnapshot, WriterLock, atomic_write_json, read_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

const FILE: &str = "pair.v1.json";
const USAGE: &str = "Opt-in pairing prototype (content proposals; at most one sidekick and one lead reviewer)
iface away pair start --config <absolute PairConfig JSON> --directory <NEW absolute directory>
iface away pair status|stop|recover|accept|take-over --directory <run directory>
iface away pair revise --directory <run directory> --message <feedback>
iface away pair compare --baseline <lead-only run directory> --candidate <paired run directory>

PairConfig v1: {version:1, cycle:<Away cycle config v1>, leadEffort, sidekickEffort}.
cycle.reviewModel selects the lead; cycle.workerModel selects the sidekick.
Maximum 3 rounds and 600 seconds. Explicit low/medium/high/xhigh effort for each role.
Scope, protected acceptance checks and objective are mandatory. No tools or children in worker turns.
Each role has durable packets; provider invocations are fresh, not persistent native sessions.
Successful checks and review hold a proposal. Accept applies with compare-and-save; revise/take-over
record a decision without applying. Interrupted/unknown application is never replayed.
A lead-only comparison uses the same lead model/effort for both roles under the same protocol.
";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PairConfig {
    version: u8,
    cycle: Config,
    pub(super) lead_effort: String,
    pub(super) sidekick_effort: String,
}
impl PairConfig {
    fn validate(&self) -> Result<(), String> {
        self.cycle.validate()?;
        if self.version != 1
            || self.cycle.duration_seconds > 600
            || self.cycle.max_iterations > 3
            || [&self.lead_effort, &self.sidekick_effort]
                .iter()
                .any(|s| !matches!(s.as_str(), "low" | "medium" | "high" | "xhigh"))
        {
            return Err("Pair v1 requires at most 600 seconds, 3 rounds and explicit supported role efforts".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u8,
    config: PairConfig,
    decision: String,
    candidate: Option<String>,
    message: Option<String>,
    receipt: Option<Value>,
    finished_at_ms: Option<u64>,
}
fn limits() -> JsonLimits {
    JsonLimits::new(2 * 1024 * 1024, 48, 100_000).expect("fixed limits")
}
fn read(root: &Path) -> Result<Record, String> {
    let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let loaded = read_json::<Record>(&dir, FILE, limits())
        .map_err(|e| e.to_string())?
        .ok_or("No pair run here")?;
    let record = loaded.value().clone();
    record.config.validate()?;
    if record.version != 1
        || !matches!(
            record.decision.as_str(),
            "pending" | "applying" | "accepted" | "revise" | "take-over"
        )
    {
        return Err("Unknown pair decision; no effects permitted".into());
    }
    Ok(record)
}
pub(super) fn initialize(root: &Path, config: &PairConfig) -> Result<(), String> {
    config.validate()?;
    let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let lock = WriterLock::acquire(&dir, "pair.lock").map_err(|e| e.to_string())?;
    atomic_write_json(
        &dir,
        &lock,
        FILE,
        &Record {
            version: 1,
            config: config.clone(),
            decision: "pending".into(),
            candidate: None,
            message: None,
            receipt: None,
            finished_at_ms: None,
        },
        &TargetSnapshot::Missing,
        limits(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn mutate(
    root: &Path,
    change: impl FnOnce(&mut Record) -> Result<(), String>,
) -> Result<Record, String> {
    let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let lock = WriterLock::acquire(&dir, "pair.lock").map_err(|e| e.to_string())?;
    let loaded = read_json::<Record>(&dir, FILE, limits())
        .map_err(|e| e.to_string())?
        .ok_or("No pair run here")?;
    let mut record = loaded.value().clone();
    record.config.validate()?;
    change(&mut record)?;
    atomic_write_json(
        &dir,
        &lock,
        FILE,
        &record,
        &TargetSnapshot::File(loaded.snapshot().clone()),
        limits(),
    )
    .map_err(|e| e.to_string())?;
    Ok(record)
}
fn metrics(root: &Path) -> Result<Value, String> {
    let record = read(root)?;
    let run = Store::read(root)?;
    if digest(&json!(record.config.cycle)) != digest(&json!(run.config)) {
        return Err("Pair scope differs from cycle evidence".into());
    }
    let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let mut calls = Vec::new();
    for item in &run.iterations {
        if let Some(proposal) = &item.proposal {
            calls.push(json!({"role":"sidekick", "round":item.number, "model":proposal["actualModel"], "requestedModel":proposal["requestedModel"], "effort":record.config.sidekick_effort, "usage":proposal["usage"], "elapsedMs":proposal["elapsedMs"]}));
        }
        if let Some(review) = read_json::<Value>(
            &dir,
            &format!("review-provider-{}.json", item.number),
            JsonLimits::new(4 * 1024 * 1024, 48, 200_000).expect("limits"),
        )
        .map_err(|e| e.to_string())?
        {
            let review = review.value();
            calls.push(json!({"role":"lead", "round":item.number, "model":review["actualModel"], "requestedModel":review["requestedModel"], "effort":record.config.lead_effort, "usage":review["usage"], "elapsedMs":review["elapsedMs"]}));
        }
    }
    let cost = calls
        .iter()
        .map(|call| call["usage"]["costUsd"].as_f64())
        .collect::<Option<Vec<_>>>()
        .filter(|values| !values.is_empty())
        .map(|values| values.iter().sum::<f64>());
    let complete = run.status == Status::Completed;
    Ok(
        json!({"version":1,"run":run,"pair":record,"roles":calls,"reportedCostUsd":if complete {cost} else {None},
        "elapsedMs":record.finished_at_ms.map(|at| at.saturating_sub(run.started_at_ms)),
        "heldCandidate":run.iterations.last().and_then(|i| i.application.as_ref()).is_some_and(|a| a["state"] == "held"),
        "hasUnknownOutcome":run.status == Status::Unknown || record.decision == "applying",
        "productionDelegationEnabled":false,"contextMode":"separate durable packets, fresh provider invocations",
        "measurementLimits":["Missing model or usage stays null; requested model is not reported model.","Incomplete provider attempts can have unreported cost. Elapsed time includes setup only after the controller starts.","A lead-only baseline uses the same lead model for both proposal and review under this fixed protocol; it is not an unconstrained single-agent benchmark."]}),
    )
}
async fn decide(root: &Path, verb: &str, message: Option<&str>) -> Result<Value, String> {
    let dir = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let _controller = WriterLock::acquire(&dir, "cycle.lock")
        .map_err(|_| "Run is still controlled; Stop and inspect before deciding")?;
    let _controls = WriterLock::acquire(&dir, "control.lock").map_err(|e| e.to_string())?;
    let record = read(root)?;
    let run = Store::read(root)?;
    if run.status == Status::Running || record.decision != "pending" {
        return Err("Run is active or already decided; unknown effects are never replayed".into());
    }
    if digest(&json!(record.config.cycle)) != digest(&json!(run.config)) {
        return Err("Pair scope changed".into());
    }
    if verb != "accept" {
        mutate(root, |record| {
            record.decision = verb.into();
            record.message = message.map(Into::into);
            Ok(())
        })?;
        return metrics(root);
    }
    let controls = Store::control(root)?;
    let item = run.iterations.last().ok_or("No candidate to accept")?;
    if run.status != Status::Completed
        || controls.stop
        || controls.revision != item.feedback_revision
        || item
            .application
            .as_ref()
            .is_none_or(|a| a["state"] != "held")
    {
        return Err("Accept requires a completed, held, checked candidate with current feedback and no Stop".into());
    }
    let proposal = item.proposal.as_ref().ok_or("No proposal")?;
    let validation = item.validation.as_ref().ok_or("No validation")?;
    interface_workspace::delegation::verify_inputs(&run.config.project, &validation["inputs"])?;
    // Persist uncertainty BEFORE file effects. A crash cannot authorize replay.
    mutate(root, |record| {
        record.decision = "applying".into();
        record.candidate.clone_from(&item.candidate_sha256);
        Ok(())
    })?;
    let receipt = CliDelegate::default()
        .apply(run.config.project, proposal.clone())
        .await?;
    mutate(root, |record| {
        if receipt["state"] == "applied" {
            record.decision = "accepted".into();
        }
        record.receipt = Some(receipt);
        Ok(())
    })?;
    metrics(root)
}
fn options(args: &[String]) -> Result<(&str, BTreeMap<&str, &str>), String> {
    let verb = args.first().ok_or(USAGE)?.as_str();
    let required: &[&str] = match verb {
        "start" => &["--directory", "--config"],
        "revise" => &["--directory", "--message"],
        "compare" => &["--baseline", "--candidate"],
        "status" | "stop" | "recover" | "accept" | "take-over" => &["--directory"],
        _ => return Err(USAGE.into()),
    };
    let values = away_cycle::named_options(
        &args[1..],
        required,
        |name, value| {
            if name == "--message" {
                !value.trim().is_empty() && value.len() <= 4096
            } else {
                Path::new(value).is_absolute()
            }
        },
        "Unknown or repeated pair option",
        "Pair requires all named options, absolute paths and bounded feedback",
    )?;
    Ok((verb, values))
}
pub(super) async fn run(args: &[String], home: &Path, progress: Option<Progress>) -> CommandResult {
    if args.is_empty() || args == ["help"] || args == ["--help"] {
        return CommandResult::success(USAGE.into());
    }
    let (verb, values) = match options(args) {
        Ok(value) => value,
        Err(error) => return CommandResult::error(error, CLI_EXIT.validation),
    };
    let result: Result<Value,String> = async {
        if verb == "compare" {
            let baseline = metrics(Path::new(values["--baseline"]))?; let candidate = metrics(Path::new(values["--candidate"]))?;
            let a = &baseline["run"]["config"]; let b = &candidate["run"]["config"];
            let same_task = ["objective", "files", "contextFiles", "validation"].iter().all(|key| a[key] == b[key]);
            return Ok(json!({"version":1,"sameTaskContract":same_task,"baseline":baseline,"candidate":candidate,"verdict":"Inspect protected-check outcomes and total reported costs. No automatic quality or savings claim; starting file hashes must also match."}));
        }
        let root = Path::new(values["--directory"]);
        match verb {
            "start" => {
                let pair: PairConfig = away_cycle::read_config(Path::new(values["--config"]))?; pair.validate()?;
                away_cycle::start_config(pair.cycle.clone(), root, home, progress, Some(pair)).await?;
                mutate(root, |record| { record.finished_at_ms = Some(interface_collaboration::away_cycle::now_ms()?); Ok(()) })?;
            }
            "stop" => { read(root)?; Store::stop(root)?; }
            "recover" => { read(root)?; Store::recover(root)?; }
            "accept" | "revise" | "take-over" => return decide(root, verb, values.get("--message").copied()).await,
            _ => {}
        }
        metrics(root)
    }.await;
    match result {
        Ok(value) => CommandResult {
            exit_code: if value["hasUnknownOutcome"] == true {
                CLI_EXIT.unknown
            } else {
                CLI_EXIT.success
            },
            stdout: format!(
                "{}\n",
                crate::sanitize::for_display(
                    &serde_json::to_string_pretty(&value).unwrap_or_default()
                )
            ),
            stderr: String::new(),
        },
        Err(error) => CommandResult::error(
            error,
            if verb == "start" {
                if values
                    .get("--directory")
                    .is_some_and(|root| Store::read(Path::new(root)).is_ok())
                {
                    CLI_EXIT.unknown
                } else {
                    CLI_EXIT.unsent
                }
            } else if verb == "accept"
                && values.get("--directory").is_some_and(|root| {
                    read(Path::new(root)).map_or(true, |record| record.decision == "applying")
                })
            {
                CLI_EXIT.unknown
            } else {
                CLI_EXIT.validation
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_resume_no_unbounded_or_implicit_provider_selection() {
        let args = |value: &str| {
            value
                .split_whitespace()
                .map(Into::into)
                .collect::<Vec<String>>()
        };
        assert!(
            options(&args(
                "start --directory /tmp/run --config /tmp/config.json"
            ))
            .is_ok()
        );
        for value in [
            "resume --directory /tmp/run",
            "accept --directory relative",
            "start --directory /tmp/run",
            "stop --directory /tmp/a --directory /tmp/b",
        ] {
            assert!(options(&args(value)).is_err());
        }
    }
}

#[cfg(test)]
#[path = "away_pair_tests.rs"]
mod evaluation_tests;
