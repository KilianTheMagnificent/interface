use super::*;
use async_trait::async_trait;
use interface_collaboration::away_cycle::{self as cycle, Executor, Review};
use interface_contracts::delegate_validation::ValidationPlan;
use interface_workspace::delegation::{ProposedFile, prepare};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tokio::sync::watch;
struct FixtureWorker {
    root: PathBuf,
    config: Config,
    wrong_first: bool,
}
#[async_trait]
impl Executor for FixtureWorker {
    async fn propose(
        &self,
        number: u8,
        context: &Value,
        _: watch::Receiver<bool>,
    ) -> Result<Value, String> {
        let packet = prepare(
            &self.config.project,
            &self.root.join(format!("proposal-{number}")),
            &self.config.files,
            &self.config.context_files,
        )?;
        let changes = packet.propose(&[ProposedFile {
            path: "value.py".into(),
            after: Some(
                if self.wrong_first && number == 1 {
                    "def value(): return 0\n"
                } else {
                    "def value(): return 42\n"
                }
                .into(),
            ),
        }])?;
        let ack = json!({"feedbackRevision":context["revision"],"addressedFeedbackIds":[],"addressedReviewIds":[],"summary":"Fixture candidate; no live model used"});
        Ok(
            json!({"summary":json!({"outcome":"changes","summary":ack.to_string()}).to_string(),"changes":changes,"actualModel":null,"requestedModel":self.config.worker_model,"usage":null}),
        )
    }
    // The same disposable-copy checks a real Away cycle runs.
    async fn validate(&self, number: u8, proposal: &Value) -> Result<Value, String> {
        crate::away_cycle::CycleExecutor {
            root: self.root.clone(),
            config: self.config.clone(),
            runner: CliDelegate::default(),
            pairing: None,
        }
        .validate(number, proposal)
        .await
    }
    async fn review(
        &self,
        number: u8,
        _: &Value,
        _: &Value,
        candidate: &str,
        context: &Value,
        _: watch::Receiver<bool>,
    ) -> Result<Review, String> {
        crate::private_write::write_private_bytes(
            &self.root.join(format!("review-provider-{number}.json")),
            json!({"actualModel":null,"requestedModel":self.config.review_model,"usage":null})
                .to_string()
                .as_bytes(),
        )?;
        Ok(Review {
            version: 1,
            candidate_sha256: candidate.into(),
            feedback_revision: context["revision"].as_u64().unwrap(),
            approved: true,
            summary: "Fixture review of fixed arithmetic".into(),
            findings: vec![],
        })
    }
    async fn apply(&self, _: &Value, _: &Value) -> Result<Value, String> {
        Ok(json!({"state":"held"}))
    }
}
async fn completed(root: &Path, wrong_first: bool) -> PathBuf {
    let project = root.join("project");
    let run_root = root.join("run");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir(&run_root).unwrap();
    let run_root = fs::canonicalize(run_root).unwrap();
    fs::set_permissions(&run_root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(project.join("value.py"), "def value(): return -1\n").unwrap();
    fs::write(
        project.join("test.py"),
        "from value import value\nassert value() == 42, 'counterexample: expected 42'\n",
    )
    .unwrap();
    let config = Config {
        version: 1,
        project: fs::canonicalize(project).unwrap(),
        objective: "Return the expected value".into(),
        files: vec!["value.py".into()],
        context_files: vec!["test.py".into()],
        validation: ValidationPlan {
            version: 1,
            files: vec!["test.py".into()],
            protected_files: vec!["test.py".into()],
            checks: vec![vec![
                "/usr/bin/python3".into(),
                "-B".into(),
                "test.py".into(),
            ]],
        },
        worker_model: if wrong_first {
            "codex/fixture-sidekick"
        } else {
            "codex/fixture-lead"
        }
        .into(),
        review_model: "codex/fixture-lead".into(),
        duration_seconds: 20,
        max_iterations: 3,
    };
    let pair = PairConfig {
        version: 1,
        cycle: config.clone(),
        lead_effort: "high".into(),
        sidekick_effort: "high".into(),
    };
    initialize(&run_root, &pair).unwrap();
    let store = Store::create(&run_root, config.clone()).unwrap();
    let (_tx, rx) = watch::channel(false);
    let run = cycle::drive(
        store,
        &FixtureWorker {
            root: run_root.clone(),
            config,
            wrong_first,
        },
        rx,
        None,
    )
    .await
    .unwrap();
    assert_eq!(run.status, Status::Completed, "{}", run.detail);
    mutate(&run_root, |record| {
        record.finished_at_ms = Some(cycle::now_ms()?);
        Ok(())
    })
    .unwrap();
    run_root
}
#[tokio::test]
async fn local_evaluation_holds_both_candidates_counts_failed_round_and_refuses_replay() {
    let base = tempfile::tempdir().unwrap();
    let pair = tempfile::tempdir().unwrap();
    let baseline = completed(base.path(), false).await;
    let candidate = completed(pair.path(), true).await;
    let a = metrics(&baseline).unwrap();
    let b = metrics(&candidate).unwrap();
    assert_eq!(a["roles"].as_array().unwrap().len(), 2);
    assert_eq!(b["roles"].as_array().unwrap().len(), 3);
    assert_eq!(b["run"]["iterations"][0]["validation"]["passed"], false);
    assert_eq!(
        b["run"]["iterations"][0]["validation"]["inputRole"],
        "originalSourceCompareAndSaveGuards"
    );
    assert!(a["reportedCostUsd"].is_null() && b["reportedCostUsd"].is_null());
    assert_eq!(
        fs::read_to_string(pair.path().join("project/value.py")).unwrap(),
        "def value(): return -1\n"
    );
    let accepted = decide(&candidate, "accept", None).await.unwrap();
    assert_eq!(accepted["pair"]["decision"], "accepted");
    assert_eq!(
        fs::read_to_string(pair.path().join("project/value.py")).unwrap(),
        "def value(): return 42\n"
    );
    assert!(decide(&candidate, "accept", None).await.is_err());
    let revised = decide(
        &baseline,
        "revise",
        Some("Keep the original implementation"),
    )
    .await
    .unwrap();
    assert_eq!(revised["pair"]["decision"], "revise");
    assert_eq!(
        fs::read_to_string(base.path().join("project/value.py")).unwrap(),
        "def value(): return -1\n"
    );
    eprintln!(
        "Fixture evaluation: lead-only 2 role calls; faulty-first pair 3; both pass protected checks before hold; real model quality and cost not measured."
    );
}
#[tokio::test]
async fn stop_source_drift_and_unknown_application_cannot_be_accepted() {
    for scenario in ["stop", "drift", "unknown"] {
        let temp = tempfile::tempdir().unwrap();
        let root = completed(temp.path(), false).await;
        match scenario {
            "stop" => {
                Store::stop(&root).unwrap();
            }
            "drift" => {
                fs::write(temp.path().join("project/value.py"), "concurrent edit\n").unwrap();
            }
            _ => {
                mutate(&root, |record| {
                    record.decision = "applying".into();
                    Ok(())
                })
                .unwrap();
            }
        }
        assert!(decide(&root, "accept", None).await.is_err(), "{scenario}");
        assert_ne!(
            fs::read_to_string(temp.path().join("project/value.py")).unwrap(),
            "def value(): return 42\n"
        );
    }
}

#[test]
fn option_refusals_keep_their_pair_wording_and_value_rules() {
    let args = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };
    let unknown = "Unknown or repeated pair option";
    let incomplete = "Pair requires all named options, absolute paths and bounded feedback";
    let longest = "x".repeat(4096);
    assert!(
        options(&args(&[
            "compare",
            "--baseline",
            "/tmp/a",
            "--candidate",
            "/tmp/b"
        ]))
        .is_ok()
    );
    assert!(
        options(&args(&[
            "revise",
            "--directory",
            "/tmp/r",
            "--message",
            &longest
        ]))
        .is_ok()
    );
    let too_long = format!("{longest}x");
    for (input, message) in [
        (
            vec!["stop", "--directory", "/tmp/r", "--message", "x"],
            unknown,
        ),
        (
            vec!["accept", "--directory", "/tmp/a", "--directory", "/tmp/b"],
            unknown,
        ),
        (
            vec!["start", "--directory", "/tmp/r", "--config", "relative"],
            incomplete,
        ),
        (
            vec!["revise", "--directory", "/tmp/r", "--message", "   "],
            incomplete,
        ),
        (
            vec!["revise", "--directory", "/tmp/r", "--message", &too_long],
            incomplete,
        ),
        (
            vec!["revise", "--directory", "/tmp/r", "--message"],
            incomplete,
        ),
        (vec!["stop"], incomplete),
    ] {
        assert_eq!(options(&args(&input)).unwrap_err(), message, "{input:?}");
    }
}
