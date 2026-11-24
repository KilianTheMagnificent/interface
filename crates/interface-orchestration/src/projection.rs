#![allow(clippy::missing_errors_doc)]

use std::io::{self, Write};

use interface_contracts::{
    orchestration::{
        AgentNodeState, AgentNodeV1, MailboxMessageV1, MessageDeliveryState, ORCHESTRATOR_ROLE,
        OrchestrationSnapshotV1, OrchestratorAutonomy, RoleClaimState, WorktreeLeaseState,
    },
    orchestration_control::{
        OrchestrationAgentPageV1, OrchestrationControlAgentV1, OrchestrationControlEventV1,
        OrchestrationControlMessageSummaryV1, OrchestrationControlSnapshotV1,
        OrchestrationEventPageV1, OrchestrationInboxPageV1, OrchestrationProjectionOmittedV1,
        OrchestrationSnapshotProjectionV1, OrchestrationTranscriptPageV1,
    },
};
use serde::Serialize;

use crate::{OrchestrationError, OrchestrationErrorCode, Result};

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;

pub const ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrchestrationControlSnapshotLimits {
    pub agents: usize,
    pub messages: usize,
    pub role_claims: usize,
    pub worktree_leases: usize,
    pub orchestrator_policies: usize,
    pub inbox_messages: usize,
    pub events_page: usize,
    pub agents_page: usize,
    pub transcript_lines: usize,
}

pub const ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS: OrchestrationControlSnapshotLimits =
    OrchestrationControlSnapshotLimits {
        agents: 64,
        messages: 64,
        role_claims: 32,
        worktree_leases: 32,
        orchestrator_policies: 32,
        inbox_messages: 4,
        events_page: 64,
        agents_page: 32,
        transcript_lines: 64,
    };

struct ByteCounter(usize);

impl Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bytes(value: &impl Serialize) -> Result<usize> {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value).map_err(|error| {
        OrchestrationError::new(OrchestrationErrorCode::InvalidState, error.to_string())
    })?;
    Ok(counter.0)
}

pub fn orchestration_control_public_bytes(value: &impl Serialize) -> Result<usize> {
    bytes(value)
}

pub fn assert_orchestration_control_public_budget(
    value: &impl Serialize,
    label: &str,
) -> Result<()> {
    if bytes(value)? > ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES {
        Err(OrchestrationError::new(
            OrchestrationErrorCode::PublicBudget,
            format!("{label} exceeds the orchestration control public byte budget."),
        ))
    } else {
        Ok(())
    }
}

fn suffix<T: Clone>(values: &[T], maximum: usize) -> Vec<T> {
    values[values.len().saturating_sub(maximum)..].to_vec()
}

fn prioritized<T: Clone>(values: &[T], maximum: usize, important: impl Fn(&T) -> bool) -> Vec<T> {
    let mut ranked = values
        .iter()
        .enumerate()
        .map(|(index, value)| (important(value), index))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    ranked.truncate(maximum);
    ranked.sort_unstable_by_key(|&(_, index)| index);
    ranked
        .into_iter()
        .map(|(_, index)| values[index].clone())
        .collect()
}

/// Which end of an over-budget page loses an entry: the oldest kept entry, or
/// the newest one. The two ends are not interchangeable — a transcript and an
/// inbox drop history, an event page drops the far end of the window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrimEnd {
    Oldest,
    Newest,
}

/// Shrink an already-selected page until it fits the public byte budget, never
/// below `floor` entries. `build` sees the surviving entries and how many were
/// dropped, so each page keeps its own truncation and cursor vocabulary; the
/// last page is still asserted, so a floor-sized page over budget fails closed.
fn shrink_page_to_budget<T: Serialize, P: Serialize>(
    selected: &[T],
    floor: usize,
    end: TrimEnd,
    build: impl Fn(&[T], usize, PageContents) -> P,
    label: &str,
) -> Result<P> {
    let sizes = selected.iter().map(bytes).collect::<Result<Vec<_>>>()?;
    let mut content_bytes: usize = sizes.iter().sum();
    let mut range = 0..selected.len();
    loop {
        let kept = &selected[range.clone()];
        let removed = selected.len() - kept.len();
        let envelope = build(kept, removed, PageContents::Omit);
        let size = bytes(&envelope)? + content_bytes + kept.len().saturating_sub(1);
        if size <= ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES || kept.len() <= floor {
            let page = build(kept, removed, PageContents::Include);
            assert_orchestration_control_public_budget(&page, label)?;
            return Ok(page);
        }
        if end == TrimEnd::Oldest {
            content_bytes -= sizes[range.start];
            range.start += 1;
        } else {
            range.end -= 1;
            content_bytes -= sizes[range.end];
        }
    }
}

/// Envelope measurements keep cursors/counters exact but omit the one array
/// whose serialized contributions are already known. Empty-array brackets are
/// included in the envelope; only elements and their commas are added back.
#[derive(Clone, Copy)]
enum PageContents {
    Include,
    Omit,
}

impl PageContents {
    fn entries<T: Clone>(self, values: &[T]) -> Vec<T> {
        match self {
            Self::Include => values.to_vec(),
            Self::Omit => Vec::new(),
        }
    }
}

#[must_use]
pub fn bounded_utf8(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[must_use]
pub fn orchestration_event_floor(snapshot: &OrchestrationSnapshotV1) -> u64 {
    snapshot.event_floor_sequence.unwrap_or_else(|| {
        snapshot
            .events
            .first()
            .map_or(snapshot.revision, |event| event.sequence.saturating_sub(1))
    })
}

#[must_use]
pub fn project_orchestration_agent_for_control(agent: &AgentNodeV1) -> OrchestrationControlAgentV1 {
    agent.into()
}

/// Whether an agent is an orchestration worker: it has a parent and is not
/// an orchestrator itself. Its session belongs to that orchestrator, so the
/// session lists leave it out.
#[must_use]
pub fn is_orchestration_worker(parent_agent_id: Option<&str>, role: Option<&str>) -> bool {
    parent_agent_id.is_some() && role != Some(ORCHESTRATOR_ROLE)
}

/// The session keys of every worker in `agents`. Pass the whole agent list,
/// never the bounded control snapshot: that keeps 64 agents, so older
/// finished workers would fall out of it.
#[must_use]
pub fn worker_session_keys(agents: &[AgentNodeV1]) -> Vec<String> {
    agents
        .iter()
        .filter(|agent| {
            is_orchestration_worker(agent.parent_agent_id.as_deref(), agent.role.as_deref())
        })
        .filter_map(|agent| agent.session_key.clone())
        .collect()
}

struct CursorPageSpec<'a> {
    cursor: Option<&'a str>,
    limit: usize,
    oversize: &'a str,
    label: &'a str,
}

/// Fill one cursor page with as many projected entries as the public byte
/// budget allows. `build` receives the entries, `hasMore`, and the next cursor,
/// and is called for every trial page so the boundary is measured on the exact
/// bytes the caller will send.
fn project_cursor_page<S, T: Clone + Serialize, P: Serialize>(
    items: &[S],
    spec: &CursorPageSpec<'_>,
    identify: impl Fn(&S) -> &str,
    project: impl Fn(&S) -> T,
    build: impl Fn(&[T], bool, Option<String>, PageContents) -> P,
) -> Result<Option<P>> {
    let start = if let Some(cursor) = spec.cursor {
        let Some(index) = items.iter().position(|item| identify(item) == cursor) else {
            return Ok(None);
        };
        index + 1
    } else {
        0
    };
    let mut selected = Vec::new();
    let mut index = start;
    let mut content_bytes = 0;
    while index < items.len() && selected.len() < spec.limit {
        let next = project(&items[index]);
        content_bytes += bytes(&next)? + usize::from(!selected.is_empty());
        selected.push(next);
        if bytes(&build(
            &selected,
            index + 1 < items.len(),
            None,
            PageContents::Omit,
        ))? + content_bytes
            > ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES
        {
            selected.pop();
            break;
        }
        index += 1;
    }
    if index == start && start < items.len() {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::PublicBudget,
            spec.oversize,
        ));
    }
    // The cursor is the identity of the last item this page consumed; a page
    // that consumed nothing carries no cursor.
    let has_more = index < items.len();
    let page = build(
        &selected,
        has_more,
        has_more
            .then(|| index.checked_sub(1))
            .flatten()
            .map(|last| identify(&items[last]).to_owned()),
        PageContents::Include,
    );
    assert_orchestration_control_public_budget(&page, spec.label)?;
    Ok(Some(page))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPageInput {
    pub after_agent_id: Option<String>,
    pub limit: usize,
}

pub fn project_orchestration_agents_for_control(
    agents: &[AgentNodeV1],
    input: &AgentPageInput,
) -> Result<Option<OrchestrationAgentPageV1>> {
    project_cursor_page(
        agents,
        &CursorPageSpec {
            cursor: input.after_agent_id.as_deref(),
            limit: input
                .limit
                .min(ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.agents_page),
            oversize: "One orchestration agent exceeds the public wire budget.",
            label: "Orchestration agent page",
        },
        |agent| agent.agent_id.as_str(),
        project_orchestration_agent_for_control,
        |agents, has_more, next_after_agent_id, contents| OrchestrationAgentPageV1 {
            version: 1,
            after_agent_id: input.after_agent_id.clone(),
            next_after_agent_id,
            has_more,
            agents: contents.entries(agents),
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptProjectionInput {
    pub agent_id: String,
    pub requested_lines: u64,
    pub lines: Vec<String>,
    pub source_was_truncated: bool,
}

pub fn project_orchestration_transcript_for_control(
    input: &TranscriptProjectionInput,
) -> Result<OrchestrationTranscriptPageV1> {
    let start = input
        .lines
        .len()
        .saturating_sub(ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.transcript_lines);
    let selected = input.lines[start..]
        .iter()
        .map(|line| bounded_utf8(&line.replace('\0', ""), 4 * 1024))
        .collect::<Vec<_>>();
    let truncated = input.source_was_truncated || start > 0;
    shrink_page_to_budget(
        &selected,
        0,
        TrimEnd::Oldest,
        |lines, removed, contents| OrchestrationTranscriptPageV1 {
            version: 1,
            agent_id: input.agent_id.clone(),
            requested_lines: input.requested_lines,
            truncated: truncated || removed > 0,
            lines: contents.entries(lines),
        },
        "Orchestration transcript page",
    )
}

#[must_use]
pub fn project_orchestration_message_for_control(
    message: &MailboxMessageV1,
    include_body: bool,
) -> MailboxMessageV1 {
    let mut projected = message.clone();
    if !include_body {
        projected.body = None;
        projected.notice_summary = None;
    }
    for receipt in &mut projected.receipts {
        receipt.detail = None;
        receipt.provider_receipt_id = None;
        if let Some(code) = &receipt.error_code {
            receipt.error_code = Some(bounded_utf8(code, 256));
        }
    }
    projected
}

fn message_summary(message: &MailboxMessageV1) -> OrchestrationControlMessageSummaryV1 {
    OrchestrationControlMessageSummaryV1 {
        version: 1,
        message_id: message.message_id.clone(),
        thread_id: message.thread_id.clone(),
        parent_message_id: message.parent_message_id.clone(),
        origin: message.origin,
        sender_agent_id: message.sender_agent_id.clone(),
        recipient_agent_id: message.recipient_agent_id.clone(),
        expect_reply: message.expect_reply,
        delivery_state: message
            .receipts
            .last()
            .map_or(MessageDeliveryState::Requested, |item| item.state),
        created_at: message.created_at.clone(),
        updated_at: message.updated_at.clone(),
    }
}

/// Subtract each removed element and comma once, including omission-counter
/// digit growth. Drain once per collection, preserving the legacy removal order.
fn trim_collection<T: Serialize>(
    values: &mut Vec<T>,
    omitted: &mut u64,
    total: &mut usize,
    truncated: &mut bool,
) -> Result<()> {
    let mut removed = 0;
    while *total > ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES && removed < values.len() {
        *total -= bytes(&values[removed])? + usize::from(values.len() - removed > 1);
        let before = bytes(omitted)?;
        *omitted += 1;
        *total += bytes(omitted)? - before;
        *total -= usize::from(!*truncated);
        *truncated = true;
        removed += 1;
    }
    values.drain(..removed);
    Ok(())
}

fn trim_snapshot(projected: &mut OrchestrationControlSnapshotV1) -> Result<()> {
    let mut total = bytes(projected)?;
    if total <= ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES {
        return Ok(());
    }
    // Change false to true only after removing the first item, as before.
    let truncated = &mut projected.projection.truncated;
    let omitted = &mut projected.projection.omitted;
    trim_collection(
        &mut projected.messages,
        &mut omitted.messages,
        &mut total,
        truncated,
    )?;
    trim_collection(
        &mut projected.role_claims,
        &mut omitted.role_claims,
        &mut total,
        truncated,
    )?;
    trim_collection(
        &mut projected.worktree_leases,
        &mut omitted.worktree_leases,
        &mut total,
        truncated,
    )?;
    trim_collection(
        &mut projected.agents,
        &mut omitted.agents,
        &mut total,
        truncated,
    )?;
    trim_collection(
        &mut projected.orchestrator_policies,
        &mut omitted.orchestrator_policies,
        &mut total,
        truncated,
    )?;
    if total > ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES {
        return Err(OrchestrationError::new(
            OrchestrationErrorCode::PublicBudget,
            "Minimal orchestration control snapshot exceeds its wire budget.",
        ));
    }
    assert_orchestration_control_public_budget(projected, "Orchestration control snapshot")
}

pub fn project_orchestration_snapshot_for_control(
    snapshot: &OrchestrationSnapshotV1,
) -> Result<OrchestrationControlSnapshotV1> {
    let agents = prioritized(
        &snapshot.agents,
        ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.agents,
        |agent| {
            matches!(
                agent.state,
                AgentNodeState::Planned
                    | AgentNodeState::Starting
                    | AgentNodeState::Ready
                    | AgentNodeState::Working
                    | AgentNodeState::Waiting
                    | AgentNodeState::Idle
                    | AgentNodeState::Unknown
            )
        },
    )
    .iter()
    .map(OrchestrationControlAgentV1::from)
    .collect::<Vec<_>>();
    let messages = suffix(
        &snapshot.messages,
        ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.messages,
    )
    .iter()
    .map(message_summary)
    .collect::<Vec<_>>();
    let role_claims = prioritized(
        &snapshot.role_claims,
        ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.role_claims,
        |claim| claim.state == RoleClaimState::Active,
    );
    let worktree_leases = prioritized(
        &snapshot.worktree_leases,
        ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.worktree_leases,
        |lease| {
            matches!(
                lease.state,
                WorktreeLeaseState::Active
                    | WorktreeLeaseState::Escrow
                    | WorktreeLeaseState::Unknown
            )
        },
    );
    let orchestrator_policies = prioritized(
        &snapshot.orchestrator_policies,
        ORCHESTRATION_CONTROL_SNAPSHOT_LIMITS.orchestrator_policies,
        |policy| policy.autonomy == OrchestratorAutonomy::Paused,
    );
    let omitted = OrchestrationProjectionOmittedV1 {
        agents: (snapshot.agents.len() - agents.len()) as u64,
        messages: (snapshot.messages.len() - messages.len()) as u64,
        role_claims: (snapshot.role_claims.len() - role_claims.len()) as u64,
        worktree_leases: (snapshot.worktree_leases.len() - worktree_leases.len()) as u64,
        orchestrator_policies: (snapshot.orchestrator_policies.len() - orchestrator_policies.len())
            as u64,
    };
    let truncated = omitted.agents > 0
        || omitted.messages > 0
        || omitted.role_claims > 0
        || omitted.worktree_leases > 0
        || omitted.orchestrator_policies > 0;
    let mut projected = OrchestrationControlSnapshotV1 {
        version: 1,
        host_id: snapshot.host_id.clone(),
        runtime_epoch: snapshot.runtime_epoch.clone(),
        revision: snapshot.revision,
        created_at: snapshot.created_at.clone(),
        updated_at: snapshot.updated_at.clone(),
        agents,
        messages,
        role_claims,
        worktree_leases,
        orchestrator_policies,
        projection: OrchestrationSnapshotProjectionV1 {
            version: 1,
            truncated,
            event_floor_sequence: orchestration_event_floor(snapshot),
            omitted,
        },
    };
    trim_snapshot(&mut projected)?;
    Ok(projected)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboxProjectionInput {
    pub agent_id: String,
    pub thread_id: Option<String>,
    pub before_message_id: Option<String>,
    pub limit: usize,
}

pub fn project_orchestration_inbox_for_control(
    snapshot: &OrchestrationSnapshotV1,
    input: &InboxProjectionInput,
) -> Result<Option<OrchestrationInboxPageV1>> {
    let matching = snapshot
        .messages
        .iter()
        .filter(|message| {
            message.recipient_agent_id == input.agent_id
                && input
                    .thread_id
                    .as_deref()
                    .is_none_or(|thread| message.thread_id == thread)
        })
        .collect::<Vec<_>>();
    let end = if let Some(cursor) = input.before_message_id.as_deref() {
        let Some(index) = matching
            .iter()
            .position(|message| message.message_id == cursor)
        else {
            return Ok(None);
        };
        index
    } else {
        matching.len()
    };
    let requested_start = end.saturating_sub(input.limit);
    let selected = matching[requested_start..end]
        .iter()
        .map(|message| project_orchestration_message_for_control(message, true))
        .collect::<Vec<_>>();
    shrink_page_to_budget(
        &selected,
        1,
        TrimEnd::Oldest,
        |messages, removed, contents| {
            let has_more = requested_start > 0 || removed > 0;
            OrchestrationInboxPageV1 {
                version: 1,
                agent_id: input.agent_id.clone(),
                thread_id: input.thread_id.clone(),
                before_message_id: input.before_message_id.clone(),
                next_before_message_id: if has_more {
                    messages.first().map(|item| item.message_id.clone())
                } else {
                    None
                },
                has_more,
                messages: contents.entries(messages),
            }
        },
        "Orchestration inbox page",
    )
    .map(Some)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventProjectionInput {
    pub sequence: u64,
    pub limit: usize,
}

pub fn project_orchestration_events_for_control(
    snapshot: &OrchestrationSnapshotV1,
    input: EventProjectionInput,
) -> Result<OrchestrationEventPageV1> {
    let floor = orchestration_event_floor(snapshot);
    let start = snapshot
        .events
        .partition_point(|event| event.sequence <= input.sequence);
    let matching = &snapshot.events[start..];
    let selected = matching
        .iter()
        .take(input.limit)
        .map(OrchestrationControlEventV1::from)
        .collect::<Vec<_>>();
    shrink_page_to_budget(
        &selected,
        1,
        TrimEnd::Newest,
        |events, _removed, contents| OrchestrationEventPageV1 {
            version: 1,
            requested_sequence: input.sequence,
            floor_sequence: floor,
            next_sequence: events
                .last()
                .map_or(input.sequence.max(floor), |event| event.sequence),
            truncated: input.sequence < floor,
            has_more: matching.len() > events.len(),
            events: contents.entries(events),
        },
        "Orchestration event page",
    )
}
