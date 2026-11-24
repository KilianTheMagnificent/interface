//! Byte-level pins for the collaboration record contract.
//!
//! `coordinator_paths_keep_their_pinned_bytes` drives every coordinator path
//! (approval refresh, drift refusals, both effect ladders, peer acceptance and
//! rebind, settlement outcomes, recovery) with a clock that advances one
//! millisecond per read, and compares the outcome, receipt and SHA-256 of
//! every persisted record file after each step with
//! `tests/golden/collaboration-coordinator-trace.txt`. A refactor that moves a
//! clock read, an id, a refusal code or one persisted byte fails here.
//!
//! `single_field_mutations_keep_their_first_failure_messages` mutates one
//! field at a time of every record, transition, receipt, audit event and
//! intent those paths produce and pins the first failure message each
//! mutation meets. The corpus is large, so the test pins its digest; every
//! run writes the full text to `$CARGO_TARGET_TMPDIR`, so two revisions can be
//! diffed line by line.
//!
//! On a mismatch the actual text is written beside the test binary's temp
//! directory and the failure names it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use interface_collaboration::broker::{
    CollaborationModelAuthority, collaboration_source_lineage_id,
};
use interface_collaboration::coordinator::{
    ApprovalAction, ApprovalPreview, CollaborationCoordinator, CoordinatorError, EffectCommand,
    EffectOutcome, EffectSuccess, PacketBuildResult, PeerAction, PendingPeer, PreparedTarget,
    SourceSnapshot, source_dispatch_guard_hash,
};
use interface_collaboration::journal::CollaborationJournal;
use interface_collaboration::state::{
    COLLABORATION_ERROR_CODES, COLLABORATION_STATES, CollaborationApprovalDecision,
    CollaborationAuditEventV1, CollaborationEffectKind, CollaborationEffectStatus,
    CollaborationEngineId, CollaborationErrorCode, CollaborationIntentInput,
    CollaborationLiveStatus, CollaborationMode, CollaborationOrigin, CollaborationPeerDecision,
    CollaborationPeerResultState, CollaborationPolicyOverrides, CollaborationReceiptV1,
    CollaborationRecordV1, CollaborationTransitionPatch, CollaborationTransitionRequest,
    CollaborationWorktreeOwnerV1, CollaborationWorktreeV1, CreateCollaborationRecordOptions,
    ManagedWorktreeV1, UnmanagedWorktreeV1, create_collaboration_record,
    maintenance_effect_for_state, normalize_collaboration_intent, receipt_for_collaboration,
    sha256, transition_collaboration_record,
};
use serde::Serialize;
use serde_json::{Value, json};
use tempfile::TempDir;

const START: &str = "2026-08-27T12:00:00.000Z";
const TRACE_GOLDEN: &str = "collaboration-coordinator-trace.txt";
const FIRST_FAILURES: &str = "collaboration-first-failures.txt";
/// SHA-256 and line count of the first-failure corpus text.
const FIRST_FAILURES_SHA256: &str =
    "52463401257d7287e5e11837109de9f4252dd9684b45466dd2868a783d0bf661";
const FIRST_FAILURES_LINES: usize = 6_256;

// ---------------------------------------------------------------------------
// Coordinator paths
// ---------------------------------------------------------------------------

/// Every distinct revision of every record the coordinator paths wrote, in
/// write order per record.
type Histories = Vec<Vec<String>>;

#[derive(Default)]
struct Corpus {
    trace: Vec<String>,
    histories: Histories,
}

struct Flow<'a> {
    name: &'static str,
    corpus: &'a mut Corpus,
    _temp: TempDir,
    root: PathBuf,
    clock: Arc<Mutex<DateTime<Utc>>>,
    ids: Arc<Mutex<u64>>,
    coordinator: Option<CollaborationCoordinator>,
    files: BTreeMap<String, Vec<String>>,
}

impl<'a> Flow<'a> {
    fn new(name: &'static str, corpus: &'a mut Corpus) -> Self {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("collaboration");
        let mut flow = Self {
            name,
            corpus,
            _temp: temp,
            root,
            clock: Arc::new(Mutex::new(parse(START))),
            ids: Arc::new(Mutex::new(0)),
            coordinator: None,
            files: BTreeMap::new(),
        };
        flow.open();
        flow.step("recover", show_debug, CollaborationCoordinator::recover);
        flow
    }

    /// Opens the journal without recovering, as a restarted host would.
    fn open(&mut self) {
        self.coordinator = None;
        let journal = CollaborationJournal::open(&self.root).expect("journal");
        let clock = Arc::clone(&self.clock);
        let ids = Arc::clone(&self.ids);
        self.coordinator = Some(CollaborationCoordinator::with_generators(
            journal,
            None,
            CollaborationPolicyOverrides::default(),
            move || {
                let mut now = clock.lock().expect("clock");
                let at = format_at(*now);
                *now += Duration::milliseconds(1);
                at
            },
            move |kind| {
                let mut current = ids.lock().expect("ids");
                *current += 1;
                format!("{kind}-{current}")
            },
        ));
    }

    fn jump(&mut self, at: &str) {
        *self.clock.lock().expect("clock") = parse(at);
        self.corpus.trace.push(format!("{} clock {at}", self.name));
    }

    fn step<T>(
        &mut self,
        label: &str,
        show: impl Fn(&T) -> String,
        op: impl FnOnce(&mut CollaborationCoordinator) -> Result<T, CoordinatorError>,
    ) -> Option<T> {
        let outcome = op(self.coordinator.as_mut().expect("open coordinator"));
        let mut line = match &outcome {
            Ok(value) => format!("{} {label}: {}", self.name, show(value)),
            Err(error) => format!("{} {label}: {}", self.name, show_error(error)),
        };
        self.snapshot(&mut line);
        self.corpus.trace.push(line);
        outcome.ok()
    }

    fn receipt(
        &mut self,
        label: &str,
        op: impl FnOnce(
            &mut CollaborationCoordinator,
        ) -> Result<CollaborationReceiptV1, CoordinatorError>,
    ) -> Option<CollaborationReceiptV1> {
        self.step(label, show_receipt, op)
    }

    fn command(
        &mut self,
        label: &str,
        op: impl FnOnce(
            &mut CollaborationCoordinator,
        ) -> Result<Option<EffectCommand>, CoordinatorError>,
    ) -> Option<EffectCommand> {
        self.step(label, |command| show_command(command.as_ref()), op)
            .flatten()
    }

    fn preview(&mut self, id: &str) -> ApprovalAction {
        let preview = self
            .step("preview", show_json, |c| c.approval_preview(id))
            .flatten()
            .expect("pending approval");
        approval_action(&preview)
    }

    fn peer(&mut self, id: &str) -> PeerAction {
        let peer = self
            .step("pending-peer", show_json, |c| c.pending_peer(id))
            .flatten()
            .expect("pending peer");
        peer_action(&peer)
    }

    fn note_acceptance(&mut self, id: &str) {
        let record = self
            .coordinator
            .as_ref()
            .expect("open coordinator")
            .runtime_record(id)
            .expect("record");
        let receipt = record
            .peer_result
            .and_then(|result| result.acceptance)
            .map(|acceptance| acceptance.receipt_id);
        self.corpus
            .trace
            .push(format!("{} acceptance receipt: {receipt:?}", self.name));
    }

    /// Appends to `line` the digest of every record file whose bytes changed.
    fn snapshot(&mut self, line: &mut String) {
        let directory = self.root.join("records");
        let mut names = std::fs::read_dir(&directory)
            .expect("records")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect::<Vec<_>>();
        names.sort();
        for name in names {
            let text = String::from_utf8(std::fs::read(directory.join(&name)).expect("record"))
                .expect("utf-8 record");
            let history = self.files.entry(name.clone()).or_default();
            if history.last() != Some(&text) {
                write!(line, " => {}:{}", &name[..8], &sha256(&text)[..16]).expect("write");
                history.push(text);
            }
        }
    }
}

impl Drop for Flow<'_> {
    fn drop(&mut self) {
        self.coordinator = None;
        self.corpus
            .histories
            .extend(std::mem::take(&mut self.files).into_values());
    }
}

fn parse(at: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(at)
        .expect("timestamp")
        .with_timezone(&Utc)
}

fn format_at(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Short JSON verbatim, anything longer as its size and digest.
fn show_json<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).expect("json");
    if json.len() <= 100 {
        json
    } else {
        format!("{} bytes #{}", json.len(), &sha256(&json)[..16])
    }
}

fn show_receipt(receipt: &CollaborationReceiptV1) -> String {
    let error = receipt.error.as_ref().map_or_else(String::new, |error| {
        format!(" {} retryable={}", error.code.as_str(), error.retryable)
    });
    format!(
        "{} r{}{error} cancellable={} #{}",
        receipt.state,
        receipt.revision,
        receipt.cancellation_safe,
        &sha256(&serde_json::to_string(receipt).expect("json"))[..16]
    )
}

fn show_debug<T: Debug>(value: &T) -> String {
    format!("{value:?}")
}

fn show_command(command: Option<&EffectCommand>) -> String {
    command.map_or_else(
        || "no command".to_owned(),
        |command| {
            format!(
                "{} {} #{}",
                command.kind().as_str(),
                &command.effect_id()[..20],
                &sha256(&format!("{command:?}"))[..16]
            )
        },
    )
}

fn show_error(error: &CoordinatorError) -> String {
    match error {
        CoordinatorError::Ingress {
            code,
            collaboration_id,
            retryable,
        } => format!(
            "refused {} retryable={retryable} record={}",
            code.as_str(),
            collaboration_id.as_deref().unwrap_or("-")
        ),
        other => format!("error {other}"),
    }
}

fn source() -> SourceSnapshot {
    SourceSnapshot {
        engine: CollaborationEngineId::Claude,
        session_key: "claude#source-1".to_owned(),
        provider_session_id: "source-1".to_owned(),
        title: Some("Source".to_owned()),
        registry_revision: 7,
        runtime_epoch: "runtime-epoch-1".to_owned(),
        live_handle_generation: 3,
        live_status: CollaborationLiveStatus::Idle,
        canonical_cwd: "/tmp/project".to_owned(),
        canonical_project_root: "/tmp/project".to_owned(),
        pending_approval_set_hash:
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        pending_approval_count: 0,
        queue_revision: 4,
        queue_depth: 0,
        delivery_revision: 5,
        delivery_unknown: false,
        worktree: Some(CollaborationWorktreeV1::Unmanaged(UnmanagedWorktreeV1 {
            version: 1,
            managed: false,
        })),
    }
}

fn managed_source() -> SourceSnapshot {
    SourceSnapshot {
        worktree: Some(CollaborationWorktreeV1::Managed(ManagedWorktreeV1 {
            version: 1,
            managed: true,
            canonical_path: "/tmp/project-worktree".to_owned(),
            branch: "feature/review".to_owned(),
            owner_revision: 2,
            transfer_id: None,
            release_in_progress: false,
            owner: CollaborationWorktreeOwnerV1::Source {
                session_key: "claude#source-1".to_owned(),
            },
        })),
        ..source()
    }
}

fn other_session() -> SourceSnapshot {
    SourceSnapshot {
        session_key: "claude#source-2".to_owned(),
        provider_session_id: "source-2".to_owned(),
        ..source()
    }
}

fn unconvertible() -> SourceSnapshot {
    SourceSnapshot {
        pending_approval_count: 9_007_199_254_740_992,
        ..source()
    }
}

fn failed_source() -> SourceSnapshot {
    SourceSnapshot {
        live_status: CollaborationLiveStatus::Failed,
        ..source()
    }
}

fn packet() -> PacketBuildResult {
    PacketBuildResult {
        body: "packet context".to_owned(),
        context_bytes: 14,
        omissions: 0,
    }
}

fn bad_packet() -> PacketBuildResult {
    PacketBuildResult {
        body: "tiny".to_owned(),
        context_bytes: 99,
        omissions: 0,
    }
}

fn input(mode: CollaborationMode, request: &str) -> CollaborationIntentInput {
    CollaborationIntentInput {
        version: Some(1),
        mode,
        source_session_key: "claude#source-1".to_owned(),
        target_engine: CollaborationEngineId::Codex,
        objective: "Review the implementation\r\nand Cafe\u{301} notes".to_owned(),
        client_request_id: request.to_owned(),
        origin: CollaborationOrigin::Human,
    }
}

fn approval_action(preview: &ApprovalPreview) -> ApprovalAction {
    ApprovalAction {
        collaboration_id: preview.collaboration_id.clone(),
        approval_id: preview.approval_id.clone(),
        expected_revision: preview.expected_revision,
    }
}

fn peer_action(peer: &PendingPeer) -> PeerAction {
    PeerAction {
        collaboration_id: peer.collaboration_id.clone(),
        result_id: peer.result_id.clone(),
        result_digest: peer.result_digest.clone(),
        expected_revision: peer.expected_revision,
    }
}

fn prepared(name: &str) -> EffectOutcome {
    EffectOutcome::Succeeded {
        value: EffectSuccess::PreparedTarget(PreparedTarget {
            session_key: format!("codex#{name}"),
            provider_session_id: name.to_owned(),
            reconciliation_marker: None,
        }),
        provider_effect_id: Some(format!("provider-{name}")),
    }
}

fn unit(provider: &str) -> EffectOutcome {
    EffectOutcome::Succeeded {
        value: EffectSuccess::Unit,
        provider_effect_id: Some(provider.to_owned()),
    }
}

fn consult_result(text: &str) -> EffectOutcome {
    EffectOutcome::Succeeded {
        value: EffectSuccess::ConsultResult(text.to_owned()),
        provider_effect_id: Some("provider-run".to_owned()),
    }
}

/// Submits one human request and approves it with `source`.
fn approved(flow: &mut Flow<'_>, mode: CollaborationMode, request: &str) -> String {
    let source = source();
    let packet = packet();
    let submitted = flow
        .receipt("submit", |c| {
            c.submit_human(input(mode, request), &source, &packet)
        })
        .expect("submitted");
    let id = submitted.collaboration_id;
    let action = flow.preview(&id);
    flow.receipt("approve", |c| c.approve_human(&action, &source, &packet));
    id
}

/// Approves and reserves target preparation.
fn reserved(flow: &mut Flow<'_>, mode: CollaborationMode, request: &str) -> String {
    let id = approved(flow, mode, request);
    flow.receipt("reserve", |c| c.reserve_prepare(&id, &source(), &packet()));
    id
}

fn consult_ladder(corpus: &mut Corpus) {
    let mut flow = Flow::new("consult", corpus);
    let source = source();
    let packet = packet();
    let begun = flow
        .receipt("begin", |c| {
            c.begin_human(input(CollaborationMode::Consult, "request-1"))
        })
        .expect("begun");
    let id = begun.collaboration_id;
    flow.step("material", show_debug, |c| c.human_preview_material(&id));
    flow.receipt("begin-replay", |c| {
        c.begin_human(input(CollaborationMode::Consult, "request-1"))
    });
    flow.receipt("complete", |c| {
        c.complete_human_preview(&id, &source, &packet)
    });
    flow.receipt("complete-replay", |c| {
        c.complete_human_preview(&id, &source, &packet)
    });
    let action = flow.preview(&id);
    flow.step(
        "pending",
        show_json,
        CollaborationCoordinator::pending_approvals,
    );
    flow.receipt("approve", |c| c.approve_human(&action, &source, &packet));
    flow.receipt("approve-replay", |c| {
        c.approve_human(&action, &source, &packet)
    });
    flow.receipt("deny-after-approve", |c| c.deny_human(&action));
    flow.receipt("reserve", |c| c.reserve_prepare(&id, &source, &packet));
    flow.step("locked", show_json, |c| c.locked_sessions());
    let prepare = flow
        .command("start-prepare", |c| {
            c.start_reserved_effect(&id, Some(&source), Some(&packet))
        })
        .expect("prepare");
    flow.command("start-prepare-again", |c| {
        c.start_reserved_effect(&id, Some(&source), Some(&packet))
    });
    flow.receipt("settle-prepare", |c| {
        c.settle_effect(&id, prepare.effect_id(), prepared("target-1"))
    });
    flow.step("participants", show_json, |c| c.human_participants(&id));
    let run = flow
        .command("start-run", |c| c.start_reserved_effect(&id, None, None))
        .expect("run");
    flow.receipt("settle-run", |c| {
        c.settle_effect(
            &id,
            run.effect_id(),
            consult_result("Peer result\r\nwith a \u{1} control and Cafe\u{301}\r"),
        )
    });
    flow.receipt("hold", |c| c.hold_peer_result(&id));
    flow.receipt("hold-replay", |c| c.hold_peer_result(&id));
    let action = flow.peer(&id);
    flow.step("guard", show_debug, |_| source_dispatch_guard_hash(&source));
    flow.receipt("accept", |c| c.accept_peer_human(&action, &source));
    flow.note_acceptance(&id);
    let mut moved = source.clone();
    moved.queue_revision += 1;
    moved.delivery_revision += 1;
    flow.step("moved-guard", show_debug, |_| {
        source_dispatch_guard_hash(&moved)
    });
    flow.command("start-deliver-moved", |c| {
        c.start_reserved_effect(&id, Some(&moved), None)
    });
    let action = flow.peer(&id);
    flow.receipt("rebind", |c| c.accept_peer_human(&action, &moved));
    flow.note_acceptance(&id);
    let action = flow.peer(&id);
    flow.receipt("rebind-replay", |c| c.accept_peer_human(&action, &moved));
    let deliver = flow
        .command("start-deliver", |c| {
            c.start_reserved_effect(&id, Some(&moved), None)
        })
        .expect("deliver");
    flow.receipt("settle-deliver", |c| {
        c.settle_effect(&id, deliver.effect_id(), unit("provider-deliver"))
    });
    flow.receipt("settle-deliver-again", |c| {
        c.settle_effect(&id, deliver.effect_id(), unit("provider-deliver"))
    });
    flow.receipt("cancel-terminal", |c| c.cancel_human(&id));
    flow.step("receipts", show_json, |c| c.all_receipts());
}

fn handoff_ladder(corpus: &mut Corpus) {
    let mut flow = Flow::new("handoff", corpus);
    let source = source();
    let packet = packet();
    let id = reserved(&mut flow, CollaborationMode::Handoff, "handoff-1");
    let prepare = flow
        .command("start-prepare", |c| {
            c.start_reserved_effect(&id, Some(&source), Some(&packet))
        })
        .expect("prepare");
    flow.receipt("settle-prepare", |c| {
        c.settle_effect(&id, prepare.effect_id(), prepared("handoff-target-1"))
    });
    let busy = SourceSnapshot {
        pending_approval_count: 1,
        ..source.clone()
    };
    flow.command("start-park-busy", |c| {
        c.start_reserved_effect(&id, Some(&busy), Some(&packet))
    });
    let queued = SourceSnapshot {
        queue_depth: 2,
        ..source.clone()
    };
    flow.command("start-park-queued", |c| {
        c.start_reserved_effect(&id, Some(&queued), Some(&packet))
    });
    let park = flow
        .command("start-park", |c| {
            c.start_reserved_effect(&id, Some(&source), Some(&packet))
        })
        .expect("park");
    flow.step("locked", show_json, |c| c.locked_sessions());
    flow.receipt("settle-park-wrong-value", |c| {
        c.settle_effect(&id, park.effect_id(), consult_result("not a park value"))
    });
    flow.receipt("settle-park", |c| {
        c.settle_effect(&id, park.effect_id(), unit("provider-park"))
    });
    let transfer = flow
        .command("start-transfer", |c| {
            c.start_reserved_effect(&id, None, None)
        })
        .expect("transfer");
    flow.receipt("settle-transfer", |c| {
        c.settle_effect(&id, transfer.effect_id(), unit("provider-transfer"))
    });
    let activate = flow
        .command("start-activate", |c| {
            c.start_reserved_effect(&id, None, None)
        })
        .expect("activate");
    flow.receipt("settle-activate", |c| {
        c.settle_effect(&id, activate.effect_id(), unit("provider-activate"))
    });
}

fn approval_refresh(corpus: &mut Corpus) {
    let mut flow = Flow::new("approval-refresh", corpus);
    let source = source();
    let packet = packet();
    let submitted = flow
        .receipt("submit", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "refresh-1"),
                &source,
                &packet,
            )
        })
        .expect("submitted");
    let id = submitted.collaboration_id;
    let original = flow.preview(&id);
    let mut moved = source.clone();
    moved.registry_revision += 1;
    flow.receipt("approve-moved", |c| {
        c.approve_human(&original, &moved, &packet)
    });
    flow.receipt("approve-stale", |c| {
        c.approve_human(&original, &moved, &packet)
    });
    let action = flow.preview(&id);
    let absent = SourceSnapshot {
        worktree: None,
        ..moved.clone()
    };
    let longer = PacketBuildResult {
        body: "packet context, longer".to_owned(),
        context_bytes: 14,
        omissions: 1,
    };
    flow.receipt("approve-new-packet-absent-worktree", |c| {
        c.approve_human(&action, &absent, &longer)
    });
    let action = flow.preview(&id);
    flow.receipt("approve-absent-worktree", |c| {
        c.approve_human(&action, &absent, &longer)
    });
    flow.receipt("reserve-absent-worktree", |c| {
        c.reserve_prepare(&id, &absent, &longer)
    });
}

fn managed_worktree(corpus: &mut Corpus) {
    let mut flow = Flow::new("managed", corpus);
    let managed = managed_source();
    let packet = packet();
    let handoff = flow
        .receipt("submit-handoff", |c| {
            c.submit_human(
                input(CollaborationMode::Handoff, "managed-0"),
                &managed,
                &packet,
            )
        })
        .expect("handoff refused");
    flow.step("participants", show_json, |c| {
        c.human_participants(&handoff.collaboration_id)
    });
    let submitted = flow
        .receipt("submit", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "managed-1"),
                &managed,
                &packet,
            )
        })
        .expect("submitted");
    let id = submitted.collaboration_id;
    let action = flow.preview(&id);
    let moved = SourceSnapshot {
        runtime_epoch: "runtime-epoch-2".to_owned(),
        ..managed.clone()
    };
    flow.receipt("approve-moved", |c| {
        c.approve_human(&action, &moved, &packet)
    });
    let action = flow.preview(&id);
    flow.receipt("approve", |c| c.approve_human(&action, &moved, &packet));
    flow.receipt("reserve", |c| c.reserve_prepare(&id, &moved, &packet));
    let prepare = flow
        .command("start-prepare", |c| {
            c.start_reserved_effect(&id, Some(&moved), Some(&packet))
        })
        .expect("prepare");
    flow.receipt("settle-prepare", |c| {
        c.settle_effect(&id, prepare.effect_id(), prepared("managed-target"))
    });
    let run = flow
        .command("start-run", |c| c.start_reserved_effect(&id, None, None))
        .expect("run");
    flow.receipt("settle-run", |c| {
        c.settle_effect(&id, run.effect_id(), consult_result("managed result"))
    });
    flow.receipt("hold", |c| c.hold_peer_result(&id));
    let action = flow.peer(&id);
    flow.receipt("discard", |c| c.discard_peer_human(&action));
    flow.receipt("discard-replay", |c| c.discard_peer_human(&action));
}

fn approval_refusals(corpus: &mut Corpus) {
    let mut flow = Flow::new("approval-refusals", corpus);
    let packet = packet();
    let refusals = [
        ("failed", failed_source(), packet.clone()),
        ("unconvertible", unconvertible(), packet.clone()),
        ("bad-packet", source(), bad_packet()),
        ("other-session", other_session(), packet.clone()),
    ];
    for (label, snapshot, fresh_packet) in refusals {
        let submitted = flow
            .receipt("submit", |c| {
                c.submit_human(input(CollaborationMode::Consult, label), &source(), &packet)
            })
            .expect("submitted");
        let id = submitted.collaboration_id;
        let action = flow.preview(&id);
        flow.receipt(label, |c| {
            c.approve_human(&action, &snapshot, &fresh_packet)
        });
    }
    flow.receipt("submit-failed-source", |c| {
        c.submit_human(
            input(CollaborationMode::Consult, "preview-failed"),
            &failed_source(),
            &packet,
        )
    });
    flow.receipt("submit-unconvertible", |c| {
        c.submit_human(
            input(CollaborationMode::Consult, "preview-unconvertible"),
            &unconvertible(),
            &packet,
        )
    });
    flow.receipt("submit-bad-packet", |c| {
        c.submit_human(
            input(CollaborationMode::Consult, "preview-bad-packet"),
            &source(),
            &bad_packet(),
        )
    });
    let begun = flow
        .receipt("begin", |c| {
            c.begin_human(input(CollaborationMode::Consult, "preview-refused"))
        })
        .expect("begun");
    flow.receipt("fail-preview", |c| {
        c.fail_human_preview(
            &begun.collaboration_id,
            CollaborationErrorCode::TargetUnhealthy,
            true,
        )
    });
    flow.receipt("conflict", |c| {
        c.begin_human(CollaborationIntentInput {
            objective: "A different objective".to_owned(),
            ..input(CollaborationMode::Consult, "preview-refused")
        })
    });

    drop(flow);

    let mut handoff = Flow::new("handoff-refusals", corpus);
    let submitted = handoff
        .receipt("submit", |c| {
            c.submit_human(
                input(CollaborationMode::Handoff, "idle-1"),
                &source(),
                &packet,
            )
        })
        .expect("submitted");
    let id = submitted.collaboration_id;
    let waiting = SourceSnapshot {
        live_status: CollaborationLiveStatus::Waiting,
        ..source()
    };
    let action = handoff.preview(&id);
    handoff.receipt("approve-waiting", |c| {
        c.approve_human(&action, &waiting, &packet)
    });
    let action = handoff.preview(&id);
    handoff.receipt("approve-waiting-again", |c| {
        c.approve_human(&action, &waiting, &packet)
    });
    handoff.receipt("fail-operation", |c| {
        c.fail_human_operation(&id, CollaborationErrorCode::TargetUnhealthy, true)
    });
    handoff.receipt("fail-operation-again", |c| {
        c.fail_human_operation(&id, CollaborationErrorCode::TargetUnhealthy, true)
    });
}

fn reserve_drift(corpus: &mut Corpus) {
    let mut flow = Flow::new("reserve-drift", corpus);
    let packet = packet();
    let id = approved(&mut flow, CollaborationMode::Consult, "reserve-1");
    let mut moved = source();
    moved.delivery_revision += 1;
    flow.receipt("reserve-moved", |c| c.reserve_prepare(&id, &moved, &packet));
    let action = flow.preview(&id);
    flow.receipt("approve-moved", |c| {
        c.approve_human(&action, &moved, &packet)
    });
    let absent = SourceSnapshot {
        worktree: None,
        ..moved.clone()
    };
    flow.receipt("reserve-absent-worktree", |c| {
        c.reserve_prepare(&id, &absent, &packet)
    });
    flow.receipt("reserve-again", |c| {
        c.reserve_prepare(&id, &absent, &packet)
    });
    let mut drifted = moved.clone();
    drifted.live_handle_generation += 1;
    flow.command("start-prepare-drifted", |c| {
        c.start_reserved_effect(&id, Some(&drifted), Some(&packet))
    });

    let refusals = [
        ("failed", failed_source(), packet.clone()),
        ("unconvertible", unconvertible(), packet.clone()),
        ("bad-packet", source(), bad_packet()),
        ("other-session", other_session(), packet.clone()),
    ];
    for (label, snapshot, fresh_packet) in refusals {
        let id = approved(&mut flow, CollaborationMode::Consult, label);
        flow.receipt(label, |c| c.reserve_prepare(&id, &snapshot, &fresh_packet));
    }
    let submitted = flow
        .receipt("submit-unapproved", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "unapproved"),
                &source(),
                &packet,
            )
        })
        .expect("submitted");
    flow.receipt("reserve-unapproved", |c| {
        c.reserve_prepare(&submitted.collaboration_id, &source(), &packet)
    });
    flow.receipt("cancel", |c| c.cancel_human(&submitted.collaboration_id));
}

fn start_refusals(corpus: &mut Corpus) {
    let mut flow = Flow::new("start-refusals", corpus);
    let packet = packet();
    let starts = [
        ("other-session", Some(other_session()), Some(packet.clone())),
        ("failed", Some(failed_source()), Some(packet.clone())),
        ("unconvertible", Some(unconvertible()), Some(packet.clone())),
        ("bad-packet", Some(source()), Some(bad_packet())),
        ("no-packet", Some(source()), None),
        ("no-source", None, Some(packet.clone())),
    ];
    for (label, snapshot, fresh_packet) in starts {
        let id = reserved(&mut flow, CollaborationMode::Consult, label);
        flow.command(label, |c| {
            c.start_reserved_effect(&id, snapshot.as_ref(), fresh_packet.as_ref())
        });
        flow.receipt("cancel", |c| c.cancel_human(&id));
    }
    let id = reserved(&mut flow, CollaborationMode::Consult, "late");
    flow.jump("2026-08-27T13:00:00.000Z");
    flow.command("start-after-deadline", |c| {
        c.start_reserved_effect(&id, Some(&source()), Some(&packet))
    });
    flow.command("start-unknown-record", |c| {
        c.start_reserved_effect("collaboration-missing", None, None)
    });
}

fn settlements(corpus: &mut Corpus) {
    let mut flow = Flow::new("settlements", corpus);
    let source = source();
    let packet = packet();
    let outcomes = [
        (
            "unsent-ambiguous-code",
            EffectOutcome::ProvenUnsent {
                code: CollaborationErrorCode::ProviderOutcomeUnknown,
            },
        ),
        (
            "unsent",
            EffectOutcome::ProvenUnsent {
                code: CollaborationErrorCode::TargetUnhealthy,
            },
        ),
        (
            "ambiguous",
            EffectOutcome::Ambiguous {
                code: Some(CollaborationErrorCode::BrokerUnavailable),
                provider_effect_id: Some("provider-maybe".to_owned()),
            },
        ),
        ("wrong-value", unit("provider-unit")),
    ];
    for (label, outcome) in outcomes {
        let id = reserved(&mut flow, CollaborationMode::Consult, label);
        let prepare = flow
            .command("start-prepare", |c| {
                c.start_reserved_effect(&id, Some(&source), Some(&packet))
            })
            .expect("prepare");
        flow.receipt(label, |c| {
            c.settle_effect(&id, prepare.effect_id(), outcome)
        });
        flow.receipt("settle-unknown-effect", |c| {
            c.settle_effect(&id, "effect-missing", unit("provider-unit"))
        });
        flow.receipt("cancel", |c| c.cancel_human(&id));
        flow.receipt("fail-operation", |c| {
            c.fail_human_operation(&id, CollaborationErrorCode::TargetUnhealthy, true)
        });
    }
    let id = reserved(&mut flow, CollaborationMode::Consult, "overdue");
    let prepare = flow
        .command("start-prepare", |c| {
            c.start_reserved_effect(&id, Some(&source), Some(&packet))
        })
        .expect("prepare");
    flow.jump("2026-08-27T14:00:00.000Z");
    flow.receipt("settle-after-deadline", |c| {
        c.settle_effect(&id, prepare.effect_id(), prepared("late-target"))
    });
    flow.receipt("reconcile-unknown-refused", |c| {
        c.reconcile_unknown_failed(&id, "marker-1", CollaborationErrorCode::TargetUnhealthy)
    });

    drop(flow);

    let mut delivery = Flow::new("delivery-unknown", corpus);
    let id = reserved(&mut delivery, CollaborationMode::Consult, "delivery");
    let prepare = delivery
        .command("start-prepare", |c| {
            c.start_reserved_effect(&id, Some(&source), Some(&packet))
        })
        .expect("prepare");
    delivery.receipt("settle-prepare", |c| {
        c.settle_effect(&id, prepare.effect_id(), prepared("delivery-target"))
    });
    let run = delivery
        .command("start-run", |c| c.start_reserved_effect(&id, None, None))
        .expect("run");
    delivery.receipt("settle-run-wrong-value", |c| {
        c.settle_effect(&id, run.effect_id(), unit("provider-unit"))
    });
    delivery.receipt("settle-run", |c| {
        c.settle_effect(&id, run.effect_id(), consult_result(&"x".repeat(70_000)))
    });
    delivery.receipt("hold", |c| c.hold_peer_result(&id));
    let action = delivery.peer(&id);
    delivery.receipt("accept", |c| c.accept_peer_human(&action, &source));
    let deliver = delivery
        .command("start-deliver", |c| {
            c.start_reserved_effect(&id, Some(&source), None)
        })
        .expect("deliver");
    delivery.step("locked", show_json, |c| c.locked_sessions());
    delivery.receipt("settle-deliver-ambiguous", |c| {
        c.settle_effect(
            &id,
            deliver.effect_id(),
            EffectOutcome::Ambiguous {
                code: None,
                provider_effect_id: None,
            },
        )
    });
    delivery.step("locked-unknown", show_json, |c| c.locked_sessions());
    delivery.receipt("reconcile", |c| {
        c.reconcile_unknown_failed(&id, "marker-1", CollaborationErrorCode::TargetUnhealthy)
    });
}

fn approvals(corpus: &mut Corpus) {
    let mut flow = Flow::new("approvals", corpus);
    let source = source();
    let packet = packet();
    let denied = flow
        .receipt("submit-denied", |c| {
            c.submit_human(input(CollaborationMode::Consult, "deny"), &source, &packet)
        })
        .expect("submitted");
    let action = flow.preview(&denied.collaboration_id);
    flow.receipt("deny", |c| c.deny_human(&action));
    flow.receipt("deny-replay", |c| c.deny_human(&action));
    let cancelled = flow
        .receipt("submit-cancelled", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "cancel"),
                &source,
                &packet,
            )
        })
        .expect("submitted");
    flow.receipt("cancel", |c| c.cancel_human(&cancelled.collaboration_id));
    let expiring = flow
        .receipt("submit-expiring", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "expire"),
                &source,
                &packet,
            )
        })
        .expect("submitted");
    let expiring_action = flow.preview(&expiring.collaboration_id);
    let second = flow
        .receipt("submit-second", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "expire-2"),
                &source,
                &packet,
            )
        })
        .expect("submitted");
    let second_action = flow.preview(&second.collaboration_id);
    flow.jump("2026-08-27T15:00:00.000Z");
    flow.step("preview-expired", show_json, |c| {
        c.approval_preview(&expiring.collaboration_id)
    });
    flow.step(
        "pending-expired",
        show_json,
        CollaborationCoordinator::pending_approvals,
    );
    flow.receipt("approve-expired", |c| {
        c.approve_human(&expiring_action, &source, &packet)
    });
    flow.receipt("deny-expired", |c| c.deny_human(&second_action));
    let third = flow
        .receipt("submit-third", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "expire-3"),
                &source,
                &packet,
            )
        })
        .expect("submitted");
    flow.jump("2026-08-27T16:00:00.000Z");
    flow.step(
        "expire-all",
        show_json,
        CollaborationCoordinator::expire_pending_approvals,
    );
    flow.step("status", show_json, |c| {
        c.human_status(&third.collaboration_id)
    });
    flow.step("active", show_json, |c| c.active_receipts());

    let lineage = collaboration_source_lineage_id(&source.session_key);
    let authority = CollaborationModelAuthority {
        engine: CollaborationEngineId::Claude,
        source_session_keys: vec![source.session_key.clone()],
        source_lineage_ids: BTreeMap::from([(source.session_key.clone(), lineage)]),
        depth: 0,
    };
    let mut model_input = input(CollaborationMode::Consult, "model-1");
    model_input.source_session_key = String::new();
    let model = flow
        .receipt("submit-model", |c| {
            c.submit_model(model_input.clone(), &authority, &source, &packet)
        })
        .expect("model");
    flow.receipt("model-status", |c| {
        c.model_status(&model.collaboration_id, &authority)
    });
    flow.receipt("cancel-model", |c| {
        c.cancel_model(&model.collaboration_id, &authority)
    });
    let deep = CollaborationModelAuthority {
        depth: 1,
        ..authority.clone()
    };
    flow.receipt("submit-model-deep", |c| {
        c.begin_model(model_input.clone(), &deep)
    });
}

fn recovery(corpus: &mut Corpus) {
    let source = source();
    let packet = packet();
    let mut crash = Flow::new("recovery", corpus);
    let pending = crash
        .receipt("submit-pending", |c| {
            c.submit_human(
                input(CollaborationMode::Consult, "pending"),
                &source,
                &packet,
            )
        })
        .expect("submitted");
    let id = reserved(&mut crash, CollaborationMode::Consult, "crash");
    let command = crash
        .command("start-prepare", |c| {
            c.start_reserved_effect(&id, Some(&source), Some(&packet))
        })
        .expect("prepare");
    crash.open();
    crash.receipt("before-recover", |c| c.human_status(&id));
    crash.jump("2026-08-27T17:00:00.000Z");
    crash.step("recover", show_debug, CollaborationCoordinator::recover);
    crash.receipt("pending-after-recover", |c| {
        c.human_status(&pending.collaboration_id)
    });
    crash.receipt("settle-after-recover", |c| {
        c.settle_effect(&id, command.effect_id(), prepared("recovered-target"))
    });
    crash.step("locked", show_json, |c| c.locked_sessions());
    crash.receipt("reconcile-ambiguous-code", |c| {
        c.reconcile_unknown_failed(&id, "marker-2", CollaborationErrorCode::DeliveryUnknown)
    });
    crash.receipt("reconcile", |c| {
        c.reconcile_unknown_failed(&id, "marker-2", CollaborationErrorCode::TargetUnhealthy)
    });
    crash.step("quiesce", show_debug, |c| {
        c.quiesce();
        Ok(())
    });
    crash.receipt("submit-quiesced", |c| {
        c.submit_human(input(CollaborationMode::Consult, "late"), &source, &packet)
    });
}

fn coordinator_corpus() -> Corpus {
    let mut corpus = Corpus::default();
    consult_ladder(&mut corpus);
    handoff_ladder(&mut corpus);
    approval_refresh(&mut corpus);
    managed_worktree(&mut corpus);
    approval_refusals(&mut corpus);
    reserve_drift(&mut corpus);
    start_refusals(&mut corpus);
    settlements(&mut corpus);
    approvals(&mut corpus);
    recovery(&mut corpus);
    corpus
}

fn assert_text_golden(name: &str, actual: &str) {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let expected = std::fs::read_to_string(&golden).unwrap_or_default();
    if expected != actual {
        let written = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        std::fs::write(&written, actual).expect("write actual");
        panic!(
            "{name} changed; the actual text is in {}",
            written.display()
        );
    }
}

#[test]
fn coordinator_paths_keep_their_pinned_bytes() {
    let corpus = coordinator_corpus();
    let mut text = corpus.trace.join("\n");
    text.push('\n');
    assert_text_golden(TRACE_GOLDEN, &text);
}

// ---------------------------------------------------------------------------
// First-failure corpus
// ---------------------------------------------------------------------------

/// Outcome counts per `(subject path mutation)` key.
type Outcomes = BTreeMap<String, BTreeMap<String, usize>>;

enum Change {
    Set(Value),
    Remove,
}

fn vocabulary<T: Serialize>(values: &[T]) -> Vec<Value> {
    values
        .iter()
        .map(|value| serde_json::to_value(value).expect("vocabulary"))
        .collect()
}

fn enum_vocabulary(path: &[String]) -> Option<Vec<Value>> {
    let key = path.last()?.as_str();
    let parent = path.len().checked_sub(2).map(|index| path[index].as_str());
    Some(match (parent, key) {
        (None, "state") | (_, "from" | "to") => vocabulary(COLLABORATION_STATES),
        (Some("peerResult"), "state") => vocabulary(&[
            CollaborationPeerResultState::Ready,
            CollaborationPeerResultState::Held,
            CollaborationPeerResultState::Accepted,
            CollaborationPeerResultState::Discarded,
            CollaborationPeerResultState::DeliveryPending,
            CollaborationPeerResultState::Delivered,
            CollaborationPeerResultState::Unknown,
        ]),
        (_, "status") => vocabulary(&[
            CollaborationEffectStatus::Reserved,
            CollaborationEffectStatus::Started,
            CollaborationEffectStatus::Succeeded,
            CollaborationEffectStatus::Failed,
            CollaborationEffectStatus::Unknown,
        ]),
        (_, "liveStatus") => vocabulary(&[
            CollaborationLiveStatus::Working,
            CollaborationLiveStatus::Waiting,
            CollaborationLiveStatus::Idle,
            CollaborationLiveStatus::Failed,
        ]),
        (Some("approval"), "decision") => vocabulary(&[
            CollaborationApprovalDecision::Pending,
            CollaborationApprovalDecision::Approved,
            CollaborationApprovalDecision::Denied,
            CollaborationApprovalDecision::Expired,
        ]),
        (Some("acceptance"), "decision") => vocabulary(&[
            CollaborationPeerDecision::Accepted,
            CollaborationPeerDecision::Discarded,
        ]),
        (_, "approvalDecision") => vec![json!("approved"), json!("denied"), json!("expired")],
        (_, "code" | "errorCode" | "reasonCode") => vocabulary(COLLABORATION_ERROR_CODES),
        (_, "mode") => vocabulary(&[CollaborationMode::Consult, CollaborationMode::Handoff]),
        (_, "engine" | "targetEngine" | "sourceEngine") => vocabulary(&[
            CollaborationEngineId::Claude,
            CollaborationEngineId::Codex,
            CollaborationEngineId::AcpGrok,
        ]),
        (_, "origin") => vocabulary(&[CollaborationOrigin::Human, CollaborationOrigin::Model]),
        (Some(effect), "kind") if effect.contains('-') => vocabulary(&[
            CollaborationEffectKind::PrepareTarget,
            CollaborationEffectKind::RunTarget,
            CollaborationEffectKind::ParkSource,
            CollaborationEffectKind::TransferOwnership,
            CollaborationEffectKind::ActivateTarget,
            CollaborationEffectKind::DeliverResult,
        ]),
        _ => return None,
    })
}

fn is_timestamp(value: &str) -> bool {
    value.len() == 24 && value.ends_with('Z') && DateTime::parse_from_rfc3339(value).is_ok()
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn leaf_changes(path: &[String], value: &Value) -> Vec<(String, Change)> {
    let mut changes = Vec::new();
    match value {
        Value::Bool(flag) => changes.push((format!("= {}", !flag), Change::Set(json!(!flag)))),
        Value::Number(number) => {
            let number = number.as_u64().expect("unsigned");
            changes.push((format!("= {}", number + 1), Change::Set(json!(number + 1))));
            if number > 1 {
                changes.push((format!("= {}", number - 1), Change::Set(json!(number - 1))));
            }
            if number > 0 {
                changes.push(("= 0".to_owned(), Change::Set(json!(0))));
            }
            changes.push((
                "= 2^53".to_owned(),
                Change::Set(json!(9_007_199_254_740_992_u64)),
            ));
        }
        Value::String(text) => {
            if let Some(vocabulary) = enum_vocabulary(path) {
                for other in vocabulary {
                    if other != *value {
                        changes.push((format!("= {other}"), Change::Set(other)));
                    }
                }
                return changes;
            }
            if is_timestamp(text) {
                let at = parse(text);
                for (label, shifted) in [
                    ("-1ms", at - Duration::milliseconds(1)),
                    ("+1ms", at + Duration::milliseconds(1)),
                    ("+1d", at + Duration::days(1)),
                ] {
                    changes.push((label.to_owned(), Change::Set(json!(format_at(shifted)))));
                }
                changes.push((
                    "non-canonical".to_owned(),
                    Change::Set(json!(text.replace('.', ",").replace('Z', "+00:00"))),
                ));
            } else if is_digest(text) {
                let last = if text.ends_with('0') { "1" } else { "0" };
                changes.push((
                    "other digest".to_owned(),
                    Change::Set(json!(format!("{}{last}", &text[..63]))),
                ));
                changes.push((
                    "uppercase".to_owned(),
                    Change::Set(json!(text.to_uppercase())),
                ));
            }
            changes.push(("= \"\"".to_owned(), Change::Set(json!(""))));
            changes.push(("= \"x\"".to_owned(), Change::Set(json!("x"))));
            changes.push((
                "trailing space".to_owned(),
                Change::Set(json!(format!("{text} "))),
            ));
            changes.push((
                "escape".to_owned(),
                Change::Set(json!(format!("{text}\u{1b}"))),
            ));
            if text.contains('#') {
                changes.push((
                    "other engine".to_owned(),
                    Change::Set(json!(
                        text.replacen("claude#", "acp:grok#", 1)
                            .replacen("codex#", "claude#", 1)
                    )),
                ));
            }
        }
        Value::Null | Value::Array(_) | Value::Object(_) => {}
    }
    changes
}

/// Optional fields worth adding where a revision leaves them out.
fn insertions(root: &Value) -> Vec<(Vec<String>, Value)> {
    let updated = root.get("updatedAt").cloned().unwrap_or(json!(START));
    let collaboration_id = root.get("collaborationId").cloned().unwrap_or(json!("x"));
    let path = |parts: &[&str]| parts.iter().map(|part| (*part).to_owned()).collect();
    vec![
        (path(&["terminalAt"]), updated.clone()),
        (
            path(&["failure"]),
            json!({"version": 1, "code": "STATE_CORRUPT", "at": updated, "retryable": false, "ambiguous": false}),
        ),
        (
            path(&["failure"]),
            json!({"version": 1, "code": "PROVIDER_OUTCOME_UNKNOWN", "at": updated, "retryable": false, "ambiguous": true}),
        ),
        (path(&["intent", "objective"]), json!("Objective")),
        (
            path(&["intent", "originLineageFingerprint"]),
            json!("a".repeat(64)),
        ),
        (path(&["target", "sessionKey"]), json!("codex#late")),
        (path(&["target", "providerSessionId"]), json!("late")),
        (path(&["target", "reconciliationMarker"]), json!("marker-9")),
        (path(&["packet", "body"]), json!("x")),
        (path(&["peerResult", "body"]), json!("x")),
        (path(&["peerResult", "queueEntryId"]), json!("queue-9")),
        (path(&["approval", "decidedAt"]), updated.clone()),
        (path(&["source", "worktreeOwnerRevision"]), json!(2)),
        (
            path(&["worktree"]),
            json!({"version": 1, "managed": true, "canonicalPath": "/tmp/project-worktree", "branch": "feature/review", "ownerRevision": 2, "releaseInProgress": false, "owner": {"kind": "source", "sessionKey": "claude#source-1"}}),
        ),
        (
            path(&["worktree", "owner"]),
            json!({"kind": "escrow", "collaborationId": collaboration_id}),
        ),
        (
            path(&["worktree", "owner"]),
            json!({"kind": "target", "sessionKey": "codex#target-1"}),
        ),
        (path(&["worktree", "transferId"]), json!("transfer-1")),
    ]
}

fn pointer_get<'v>(value: &'v Value, path: &[String]) -> Option<&'v Value> {
    path.iter().try_fold(value, |node, key| node.get(key))
}

fn apply(root: &Value, path: &[String], change: &Change) -> Value {
    let mut copy = root.clone();
    let (last, parents) = path.split_last().expect("non-empty path");
    let parent = parents
        .iter()
        .try_fold(&mut copy, |node, key| node.get_mut(key))
        .and_then(Value::as_object_mut)
        .expect("parent object");
    match change {
        Change::Set(value) => {
            parent.insert(last.clone(), value.clone());
        }
        Change::Remove => {
            parent.remove(last);
        }
    }
    copy
}

fn collect_leaves(value: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    if let Value::Object(map) = value {
        for (key, child) in map {
            path.push(key.clone());
            out.push(path.clone());
            collect_leaves(child, path, out);
            path.pop();
        }
    }
}

/// Every single-field change of `root`, restricted to paths `keep` accepts.
fn mutations(root: &Value, keep: impl Fn(&[String]) -> bool) -> Vec<(String, Value)> {
    let mut paths = Vec::new();
    collect_leaves(root, &mut Vec::new(), &mut paths);
    let mut out = Vec::new();
    for path in paths.iter().filter(|path| keep(path)) {
        let value = pointer_get(root, path).expect("leaf");
        let name = format!("/{}", path.join("/"));
        out.push((
            format!("{name} removed"),
            apply(root, path, &Change::Remove),
        ));
        for (label, change) in leaf_changes(path, value) {
            out.push((format!("{name} {label}"), apply(root, path, &change)));
        }
    }
    for (path, value) in insertions(root) {
        if !keep(&path) || pointer_get(root, &path).is_some() {
            continue;
        }
        let (_, parents) = path.split_last().expect("path");
        if !pointer_get(root, parents).is_some_and(Value::is_object) && !parents.is_empty() {
            continue;
        }
        let name = format!("/{} added {}", path.join("/"), value);
        out.push((name, apply(root, &path, &Change::Set(value))));
    }
    out
}

fn record_outcome(record: &CollaborationRecordV1) -> String {
    match record.validate() {
        Ok(()) => "ok".to_owned(),
        Err(error) => error.to_string(),
    }
}

fn tally(outcomes: &mut Outcomes, key: String, outcome: String) {
    *outcomes.entry(key).or_default().entry(outcome).or_default() += 1;
}

/// One representative revision per distinct validation shape keeps the
/// corpus fast without losing a state, failure or peer-result combination.
fn representative_records(histories: &Histories) -> Vec<Value> {
    let mut seen = BTreeMap::new();
    for text in histories.iter().flatten() {
        let value: Value = serde_json::from_str(text).expect("record json");
        let shape = format!(
            "{} {} {} {} {}",
            value["intent"]["mode"],
            value["state"],
            value["failure"]["ambiguous"],
            value["peerResult"]["state"],
            value["worktree"]["managed"],
        );
        seen.entry(shape).or_insert(value);
    }
    seen.into_values().collect()
}

fn record_mutations(histories: &Histories, outcomes: &mut Outcomes) {
    for record in representative_records(histories) {
        let parsed: CollaborationRecordV1 = serde_json::from_value(record.clone()).expect("record");
        assert_eq!(record_outcome(&parsed), "ok", "corpus records are valid");
        for (key, mutated) in mutations(&record, |_| true) {
            if let Ok(mutated) = serde_json::from_value::<CollaborationRecordV1>(mutated) {
                tally(outcomes, format!("record {key}"), record_outcome(&mutated));
            }
        }
        let receipt = receipt_for_collaboration(&parsed).expect("receipt");
        let receipt = serde_json::to_value(receipt).expect("receipt json");
        for (key, mutated) in mutations(&receipt, |_| true) {
            if let Ok(mutated) = serde_json::from_value::<CollaborationReceiptV1>(mutated) {
                let outcome = mutated
                    .validate()
                    .map_or_else(|error| error.to_string(), |()| "ok".to_owned());
                tally(outcomes, format!("receipt {key}"), outcome);
            }
        }
    }
}

/// The request the coordinator must have made to turn `before` into `after`.
fn reconstructed_request(
    before: &CollaborationRecordV1,
    after: &CollaborationRecordV1,
) -> CollaborationTransitionRequest {
    let maintenance = (before.state == after.state && before.effects != after.effects)
        .then(|| maintenance_effect_for_state(before.state, before.intent.mode))
        .flatten();
    CollaborationTransitionRequest {
        expected_revision: before.revision,
        expected_state: before.state,
        to: after.state,
        maintenance_effect: maintenance,
        at: after.updated_at.clone(),
        event_id: Some("event-golden".to_owned()),
        patch: CollaborationTransitionPatch {
            source: Some(after.source.clone()),
            target: Some(after.target.clone()),
            packet: after.packet.clone(),
            approval: after.approval.clone(),
            effects: Some(after.effects.clone()),
            peer_result: after.peer_result.clone(),
            worktree: after.worktree.clone(),
            failure: after.failure.clone(),
            clear_failure: before.failure.is_some() && after.failure.is_none(),
        },
    }
}

fn transition_outcome(before: &CollaborationRecordV1, after: &CollaborationRecordV1) -> String {
    match transition_collaboration_record(before, reconstructed_request(before, after)) {
        Ok(result) => {
            if result.record == *after {
                "ok".to_owned()
            } else {
                "ok (other record)".to_owned()
            }
        }
        Err(error) => format!("{:?}: {error}", error.reason),
    }
}

fn settable(path: &[String]) -> bool {
    matches!(
        path.first().map(String::as_str),
        Some(
            "state"
                | "updatedAt"
                | "source"
                | "target"
                | "packet"
                | "approval"
                | "effects"
                | "peerResult"
                | "worktree"
                | "failure"
        )
    )
}

fn transition_mutations(histories: &Histories, outcomes: &mut Outcomes) {
    let mut audits = BTreeMap::new();
    let mut shapes = BTreeSet::new();
    for history in histories {
        let records = history
            .iter()
            .map(|text| serde_json::from_str::<CollaborationRecordV1>(text).expect("record"))
            .collect::<Vec<_>>();
        for pair in records.windows(2) {
            let (before, after) = (&pair[0], &pair[1]);
            if after.revision != before.revision + 1 {
                continue;
            }
            let result =
                transition_collaboration_record(before, reconstructed_request(before, after))
                    .expect("reconstructed transition replays");
            assert_eq!(result.record, *after, "reconstructed transition is exact");
            let shape = format!(
                "{:?} {} {} {:?}",
                before.intent.mode,
                before.state,
                after.state,
                result
                    .audit
                    .reason_code
                    .map(CollaborationErrorCode::is_ambiguous),
            );
            if !shapes.insert(shape) {
                continue;
            }
            audits
                .entry(format!("{} {}", before.state, after.state))
                .or_insert(result.audit);
            let after_value = serde_json::to_value(after).expect("record json");
            for (key, mutated) in mutations(&after_value, settable) {
                if let Ok(mutated) = serde_json::from_value::<CollaborationRecordV1>(mutated) {
                    tally(
                        outcomes,
                        format!("transition {} {key}", before.state),
                        transition_outcome(before, &mutated),
                    );
                }
            }
        }
    }
    for audit in audits.into_values() {
        let value = serde_json::to_value(audit).expect("audit json");
        for (key, mutated) in mutations(&value, |_| true) {
            if let Ok(mutated) = serde_json::from_value::<CollaborationAuditEventV1>(mutated) {
                let outcome = mutated
                    .validate()
                    .map_or_else(|error| error.to_string(), |()| "ok".to_owned());
                tally(outcomes, format!("audit {key}"), outcome);
            }
        }
    }
}

fn intent_mutations(outcomes: &mut Outcomes) {
    let lineage = collaboration_source_lineage_id("claude#source-1");
    for origin in [CollaborationOrigin::Human, CollaborationOrigin::Model] {
        for mode in [CollaborationMode::Consult, CollaborationMode::Handoff] {
            let mut base = input(mode, "intent-1");
            base.origin = origin;
            let value = serde_json::to_value(&base).expect("input json");
            let mut cases = mutations(&value, |_| true);
            cases.push(("unchanged".to_owned(), value.clone()));
            for (key, mutated) in cases {
                let Ok(mutated) = serde_json::from_value::<CollaborationIntentInput>(mutated)
                else {
                    continue;
                };
                for (label, lineage) in [
                    ("without lineage", None),
                    ("with lineage", Some(lineage.as_str())),
                ] {
                    let outcome = match normalize_collaboration_intent(&mutated, lineage) {
                        Ok(intent) => {
                            create_collaboration_record(CreateCollaborationRecordOptions {
                                collaboration_id: "collaboration-intent".to_owned(),
                                intent,
                                created_at: START.to_owned(),
                                depth: 0,
                                limits: CollaborationPolicyOverrides::default(),
                            })
                            .map_or_else(
                                |error| format!("create: {error}"),
                                |record| format!("ok {}", record.dedupe_key),
                            )
                        }
                        Err(error) => error.to_string(),
                    };
                    tally(
                        outcomes,
                        format!("intent {origin:?} {mode:?} {label} {key}"),
                        outcome,
                    );
                }
            }
        }
    }
    let intent = normalize_collaboration_intent(&input(CollaborationMode::Consult, "limits"), None)
        .expect("intent");
    for (label, depth, limits) in [
        ("depth", 2, CollaborationPolicyOverrides::default()),
        (
            "context over packet",
            0,
            CollaborationPolicyOverrides {
                max_packet_bytes: Some(10),
                max_context_bytes: Some(11),
                ..CollaborationPolicyOverrides::default()
            },
        ),
        (
            "zero results",
            0,
            CollaborationPolicyOverrides {
                max_result_bytes: Some(0),
                ..CollaborationPolicyOverrides::default()
            },
        ),
        (
            "handoff timeout on consult",
            0,
            CollaborationPolicyOverrides {
                handoff_prepare_timeout_ms: Some(1),
                ..CollaborationPolicyOverrides::default()
            },
        ),
        (
            "tiny objective",
            0,
            CollaborationPolicyOverrides {
                max_objective_bytes: Some(3),
                ..CollaborationPolicyOverrides::default()
            },
        ),
    ] {
        let outcome = create_collaboration_record(CreateCollaborationRecordOptions {
            collaboration_id: "collaboration-limits".to_owned(),
            intent: intent.clone(),
            created_at: START.to_owned(),
            depth,
            limits,
        })
        .map_or_else(|error| error.to_string(), |_| "ok".to_owned());
        tally(outcomes, format!("create {label}"), outcome);
    }
}

#[test]
fn single_field_mutations_keep_their_first_failure_messages() {
    let corpus = coordinator_corpus();
    let mut outcomes = Outcomes::new();
    record_mutations(&corpus.histories, &mut outcomes);
    transition_mutations(&corpus.histories, &mut outcomes);
    intent_mutations(&mut outcomes);
    let mut text = String::new();
    for (key, results) in &outcomes {
        for (outcome, count) in results {
            writeln!(text, "{key} => {outcome} x{count}").expect("write");
        }
    }
    let written = Path::new(env!("CARGO_TARGET_TMPDIR")).join(FIRST_FAILURES);
    std::fs::write(&written, &text).expect("write corpus");
    assert_eq!(
        (sha256(&text).as_str(), text.lines().count()),
        (FIRST_FAILURES_SHA256, FIRST_FAILURES_LINES),
        "first-failure corpus changed; the full text is in {}",
        written.display()
    );
}
