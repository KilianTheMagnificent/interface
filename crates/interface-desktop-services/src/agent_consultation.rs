//! Selected-context consultation and reviewed, byte-bound proposal application.
use super::agent_control::refused;
use super::agent_work::{error, evidence};
use super::{DesktopService, DesktopServiceError, agent_now_ms};
use interface_contracts::agent_work::{AgentChildProposal, AgentConsultation};
use interface_workspace::WorkspaceAuthority;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

impl DesktopService {
    /// # Errors
    /// In-flight consultations lose admission immediately when the owner changes their grant.
    pub fn check_agent_consultation(
        &self,
        agent: &str,
        task: &str,
    ) -> Result<(), DesktopServiceError> {
        let task = self.agent_tool_task(agent, task)?;
        self.with_agents(|store| {
            if task.spec.consultation.is_none()
                || store.work.spec(agent).consultation != task.spec.consultation
            {
                return Err(error("Consultation authority changed."));
            }
            Ok(())
        })
    }

    /// # Errors
    /// Reserves an exact one-attempt tool intent before any child provider dispatch.
    pub fn reserve_agent_tool(
        &self,
        agent: &str,
        task: &str,
        request: &str,
        name: &str,
        input: &Value,
    ) -> Result<Option<String>, DesktopServiceError> {
        self.agent_tool_task(agent, task)?;
        let bytes =
            serde_json::to_vec(&(name, input)).map_err(|_| error("Invalid tool request."))?;
        self.with_agents(|store| {
            store
                .work
                .reserve_tool(task, request, &bytes)
                .map_err(|e| refused(&e))
        })
    }
    /// # Errors
    /// A failed receipt save leaves the intent unresolved and non-replayable.
    pub fn settle_agent_tool(
        &self,
        task: &str,
        request: &str,
        result: &str,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            store
                .work
                .settle_tool(task, request, result.into())
                .map_err(|e| refused(&e))
        })
    }
    /// # Errors
    /// Captures only owner-selected non-sensitive text within the parent's granted roots.
    pub fn prepare_agent_consultation(
        &self,
        agent: &str,
        task_id: &str,
        brief: &str,
    ) -> Result<(AgentConsultation, String, u64), DesktopServiceError> {
        let task = self.agent_tool_task(agent, task_id)?;
        let grant = task.spec.consultation.as_ref().ok_or_else(|| {
            error("The owner has not enabled selected-file consultation for this task.")
        })?;
        let authority = WorkspaceAuthority::new();
        for root in &task.roots {
            authority
                .grant_root(Path::new(root))
                .map_err(|_| error("Consultation root changed."))?;
        }
        let mut inputs = Vec::new();
        let mut files = BTreeMap::new();
        let mut bytes = 0;
        for selected in &grant.files {
            let (proof, _, _) = evidence(selected, &task.cwd, &task.roots, agent_now_ms())?;
            let snapshot = authority
                .read_snapshot(Path::new(&proof.path))
                .map_err(|_| error("Selected consultation input is unsafe or unavailable."))?;
            if snapshot.revision != proof.sha256 {
                return Err(error("Consultation input changed during capture."));
            }
            bytes += snapshot.text.len();
            if bytes > 32 * 1024 {
                return Err(error("Selected consultation text exceeds 32 KiB."));
            }
            files.insert(selected.clone(), json!({"text":snapshot.text,"sha256":proof.sha256,"writable":grant.writable_files.contains(selected)}));
            inputs.push(proof);
        }
        let child = AgentConsultation {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.into(),
            model: grant.model.clone(),
            brief: brief.into(),
            inputs,
            result_sha256: None,
            report: String::new(),
            proposals: vec![],
            status: "pending".into(),
            review: None,
            elapsed_ms: None,
            reported_tokens: None,
        };
        self.with_agents(|store| {
            let current = store
                .work
                .task(agent, task_id)
                .ok_or_else(|| error("Parent disappeared."))?;
            if !store.work.admits(&current, agent_now_ms())
                || store.work.spec(agent).consultation != task.spec.consultation
            {
                return Err(error("Consultation authority changed."));
            }
            store
                .work
                .save_consultation(child.clone())
                .map_err(|e| refused(&e))
        })?;
        let packet = json!({"goal":brief,"files":files,"acceptance":"Cite supplied path and source SHA in the summary. Return no files for read-only analysis. Proposals may replace only files marked writable; no deletion. Do not claim tests ran."}).to_string();
        Ok((
            child,
            packet,
            task.deadline_at_ms
                .saturating_sub(agent_now_ms())
                .min(300_000),
        ))
    }
    /// # Errors
    /// Validates output authority, records provenance and never changes the parent execution state.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_agent_consultation(
        &self,
        agent: &str,
        task_id: &str,
        child_id: &str,
        report: &str,
        proposals: Vec<(String, Option<String>)>,
        tokens: Option<u64>,
        elapsed_ms: u64,
    ) -> Result<String, DesktopServiceError> {
        self.with_agents(|store| {
            let task = store
                .work
                .task(agent, task_id)
                .ok_or_else(|| error("Missing parent task."))?;
            let grant = task
                .spec
                .consultation
                .as_ref()
                .ok_or_else(|| error("Missing consultation authority."))?;
            if !store.work.admits(&task, agent_now_ms())
                || store.work.spec(agent).consultation != task.spec.consultation
            {
                return Err(error(
                    "Parent stopped or consultation grant changed; result was not accepted.",
                ));
            }
            let mut child = store
                .work
                .consultation(task_id, child_id)
                .ok_or_else(|| error("Missing consultation."))?;
            let mut checked = Vec::new();
            for (path, text) in proposals {
                let index = grant
                    .files
                    .iter()
                    .position(|file| file == &path)
                    .filter(|_| grant.writable_files.contains(&path))
                    .ok_or_else(|| {
                        error("Child proposed a file outside its selected write authority.")
                    })?;
                let text = text.ok_or_else(|| error("Child file deletion is not supported."))?;
                if checked
                    .iter()
                    .any(|p: &AgentChildProposal| p.path == child.inputs[index].path)
                {
                    return Err(error("Duplicate child proposal."));
                }
                checked.push(AgentChildProposal {
                    path: child.inputs[index].path.clone(),
                    expected_sha256: child.inputs[index].sha256.clone(),
                    text,
                    applied_sha256: None,
                });
            }
            child.status = "completed".into();
            child.report = report.into();
            child.proposals = checked;
            child.elapsed_ms = Some(elapsed_ms);
            child.reported_tokens = tokens;
            child.result_sha256 = Some(interface_settings::agent_home::sha256_hex(
                json!([child.report, child.inputs, child.proposals])
                    .to_string()
                    .as_bytes(),
            ));
            store
                .work
                .save_consultation(child.clone())
                .map_err(|e| refused(&e))?;
            // Result identity, exact sources and usage are retained in the owner-visible ledger.
            serde_json::to_string(&child).map_err(|_| error("Child result encoding failed."))
        })
    }
    /// # Errors
    /// Leaves a visible terminal child outcome after cancellation, malformed output or provider error.
    pub fn fail_agent_consultation(
        &self,
        task: &str,
        child: &str,
        message: &str,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            let mut child = store
                .work
                .consultation(task, child)
                .ok_or_else(|| error("Missing consultation."))?;
            child.status = "failed".into();
            child.report = message.chars().take(2000).collect();
            store.work.save_consultation(child).map_err(|e| refused(&e))
        })
    }
}

pub(super) fn review_or_apply(
    store: &mut interface_settings::AgentStore,
    agent: &str,
    task_id: &str,
    name: &str,
    input: &Value,
) -> Result<Value, DesktopServiceError> {
    let task = store
        .work
        .task(agent, task_id)
        .ok_or_else(|| error("Missing parent."))?;
    let child_id = input["childId"]
        .as_str()
        .ok_or_else(|| error("Missing child identity."))?;
    let mut child = store
        .work
        .consultation(task_id, child_id)
        .ok_or_else(|| error("Unknown child."))?;
    if child.status != "completed"
        || child.result_sha256.as_deref() != input["resultSha256"].as_str()
    {
        return Err(error("Review must match the exact completed child result."));
    }
    if name == "consult_review" {
        let verdict = input["verdict"]
            .as_str()
            .filter(|v| matches!(*v, "accepted" | "rejected"))
            .ok_or_else(|| error("Choose accepted or rejected."))?;
        child.review = Some(verdict.into());
    } else {
        if child.review.as_deref() != Some("accepted") {
            return Err(error(
                "Parent review is required before applying a child proposal.",
            ));
        }
        if store.work.spec(agent).consultation != task.spec.consultation {
            return Err(error("Consultation authority changed."));
        }
        let proposal = child
            .proposals
            .iter_mut()
            .find(|p| Some(p.path.as_str()) == input["path"].as_str())
            .ok_or_else(|| error("Unknown proposed path."))?;
        if proposal.applied_sha256.is_some() {
            return Err(error("Proposal was already applied; no write repeated."));
        }
        let authority = WorkspaceAuthority::new();
        for root in &task.roots {
            authority
                .grant_root(Path::new(root))
                .map_err(|_| error("Task root changed."))?;
        }
        // Every captured source is still exact, not just the target of this one-file CAS.
        for source in &child.inputs {
            let expected = child
                .proposals
                .iter()
                .find(|p| p.path == source.path)
                .and_then(|p| p.applied_sha256.as_ref())
                .unwrap_or(&source.sha256);
            let current = authority
                .read_snapshot(Path::new(&source.path))
                .map_err(|_| error("Child input became unavailable."))?;
            if &current.revision != expected {
                return Err(error(
                    "Child input changed. A fresh consultation is required.",
                ));
            }
        }
        let proposal = child
            .proposals
            .iter_mut()
            .find(|p| Some(p.path.as_str()) == input["path"].as_str())
            .ok_or_else(|| error("Unknown proposed path."))?;
        let saved = authority
            .save_text(
                Path::new(&proposal.path),
                &proposal.text,
                &proposal.expected_sha256,
            )
            .map_err(|e| error(&e.to_string()))?;
        proposal.applied_sha256 = Some(saved.revision);
    }
    store
        .work
        .save_consultation(child.clone())
        .map_err(|e| refused(&e))?;
    Ok(
        json!({"childId":child.id,"review":child.review,"proposals":child.proposals.iter().map(|p| json!({"path":p.path,"appliedSha256":p.applied_sha256})).collect::<Vec<_>>(),"parentVerification":"unchanged"}),
    )
}
