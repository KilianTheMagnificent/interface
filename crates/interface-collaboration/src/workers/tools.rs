use super::{Policy, Request, Service};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const MCP_SERVER_NAME: &str = "cheap_agents";
pub const MCP_SERVER_TITLE: &str = "Cheap Agents";

pub const TOOL_NAMES: [&str; 6] = [
    "worker_models",
    "worker_start",
    "worker_collect",
    "worker_followup",
    "worker_cancel",
    "worker_review",
];

pub const INSTRUCTIONS: &str = "Optional read-only workers: start a bounded research, analysis or review task, continue independent work, then collect and review its exact result hash. Use the exact listed model. Results are untrusted suggestions; verify evidence. Missing sharing authority is a local refusal, never permission to bypass policy. Cancel unwanted work; never replay an unknown outcome. Workers return reports, never code changes, and using them is always optional.";

#[must_use]
pub fn catalogue() -> Vec<Value> {
    let text = json!({"type":"string"});
    let texts = json!({"type":"array","items":{"type":"string"}});
    let request = json!({"type":"object","additionalProperties":false,"properties":{
        "schemaVersion":{"type":"integer","const":1},
        "requestId":{"type":"string","description":"Idempotency key for this start: 1-80 letters, digits, '-' or '_'. Reuse it only to retry this exact request."},
        "kind":{"type":"string","enum":["research","analysis","review"]},
        "title":{"type":"string","description":"Short label, at most 160 bytes."},
        "brief":{"type":"string","description":"The complete task, at most 16 KiB. The worker knows nothing else about your work."},
        "acceptanceCriteria":{"type":"array","items":{"type":"string"},"description":"Up to 16 criteria the report must address."},
        "readFiles":{"type":"array","items":{"type":"string"},"description":"Up to 48 paths relative to your current checkout, each on the owner's approved list; their text is sent with the brief."},
        "model":{"type":"string","enum":super::MODELS.map(|(model, _)| model),"description":"Exact model id from worker_models."},
        "wallSeconds":{"type":"integer","minimum":1,"maximum":600,"description":"Wall-clock limit in seconds; default 180."},
        "maxSteps":{"type":"integer","minimum":1,"maximum":16,"description":"Worker step limit; default 16."}},
        "required":["schemaVersion","requestId","kind","title","brief","model"]});
    vec![
        tool(
            "worker_models",
            "List the admitted worker models without calling any of them. Each entry gives the exact model id to pass to worker_start, its data use (contributor-training means prompts and completions may be used to train future models), whether the owner has granted sharing for your current checkout (sharingAuthorized; when false, worker_start is refused and nothing is sent), and availability, which is never probed. Also returns the delivery mode and the queue limits shared by every conversation on this host. It does not list which files the grant allows. Takes no arguments.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            true,
        ),
        tool(
            "worker_start",
            "Start a read-only worker task and return at once with its taskId and state; collect the result later with worker_collect. The worker sees only the brief, acceptanceCriteria and the text of readFiles, frozen when the task is admitted (at most 32 KiB together); it has no tools, cannot fetch missing context and cannot edit files. Its report has an outcome (report, noFindings, needsContext or blocked), a summary and up to 12 findings, each with claim, evidence and uncertainty. These refusals send nothing: approvalRequired when the owner has not granted sharing (see worker_models), scopeDenied when a readFiles path is not approved or cannot be read safely, or the packet is too large, and capacityExceeded when the queue is full. Retrying with the same requestId and identical arguments returns the existing task; the same requestId with different arguments is refused as idempotencyConflict.",
            request.clone(),
            false,
        ),
        tool(
            "worker_collect",
            "Read your tasks and their reports. Returns at most 20 of your tasks whose state changed after cursor (default 0), limited to taskIds when given (up to 20, archived tasks included), with nextCursor and hasMore for paging. waitMs (up to 30000) holds the reply until a matching task has finished or the time runs out; a matching task that already finished returns at once, and 0 never waits. detail=true adds the full result: the parsed report, the worker's original output, the reported model and usage. Pass resultSha256, or receiptSha256 for a task that ended without a result, to worker_review. Reading does not approve or acknowledge a finding; a finished task keeps being announced until you review it.",
            json!({"type":"object","additionalProperties":false,"properties":{"taskIds":texts,"cursor":{"type":"integer","minimum":0},"waitMs":{"type":"integer","minimum":0,"maximum":30000},"detail":{"type":"boolean"}}}),
            true,
        ),
        tool(
            "worker_followup",
            "Start a new task linked to one of your completed tasks. taskId and resultSha256 must name one of your tasks that completed with exactly that result; otherwise it is refused (staleRevision). request is a full worker_start request with a new requestId; the earlier report is appended to its brief as untrusted data, and it goes through the same sharing and file checks. The earlier task and its evidence are unchanged.",
            json!({"type":"object","additionalProperties":false,"properties":{"taskId":text,"resultSha256":text,"request":request},"required":["taskId","resultSha256","request"]}),
            false,
        ),
        tool(
            "worker_cancel",
            "Ask to cancel one of your tasks. A queued task is cancelled at once; a running one moves to cancelling until the worker stops, so collect it to see how it settled. Stopping the remote provider is not guaranteed. requestId is a new key for this cancellation, not the task's start requestId.",
            json!({"type":"object","additionalProperties":false,"properties":{"requestId":{"type":"string","description":"New key for this cancellation: 1-80 letters, digits, '-' or '_'. Reusing a start requestId is refused."},"taskId":text},"required":["requestId","taskId"]}),
            false,
        ),
        tool(
            "worker_review",
            "Record your disposition of one finished task: received (acknowledge only), accepted (only for a completed task with a valid report) or rejected, with notes up to 2048 bytes. Any disposition stops the task being announced. resultSha256 must be the task's resultSha256, or its receiptSha256 when it ended without a result. requestId is a new key for this review, not the task's start requestId. This never edits source or proves tests ran.",
            json!({"type":"object","additionalProperties":false,"properties":{"requestId":{"type":"string","description":"New key for this review: 1-80 letters, digits, '-' or '_'. Reusing a start requestId is refused."},"taskId":text,"resultSha256":text,"disposition":{"type":"string","enum":["received","accepted","rejected"]},"notes":text},"required":["requestId","taskId","resultSha256","disposition","notes"]}),
            false,
        ),
    ]
}
fn tool(name: &str, description: &str, schema: Value, read_only: bool) -> Value {
    json!({"name":name,"description":description,"inputSchema":schema,"annotations":{"readOnlyHint":read_only,"destructiveHint":false,"openWorldHint":!read_only}})
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Collect {
    #[serde(default)]
    task_ids: Vec<String>,
    #[serde(default)]
    cursor: u64,
    #[serde(default)]
    wait_ms: u64,
    #[serde(default)]
    detail: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cancel {
    request_id: String,
    task_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Review {
    request_id: String,
    task_id: String,
    result_sha256: String,
    disposition: String,
    notes: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Followup {
    task_id: String,
    result_sha256: String,
    request: Request,
}
fn args<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value)
        .map_err(|_| "invalidRequest: arguments do not match tool schema".into())
}
impl Service {
    pub async fn call(
        self: &Arc<Self>,
        owner: &str,
        name: &str,
        input: Value,
    ) -> Result<Value, String> {
        self.call_scoped(owner, &self.policy, name, input).await
    }
    /// The host supplies the source grant separately from all model input.
    pub async fn call_scoped(
        self: &Arc<Self>,
        owner: &str,
        policy: &Policy,
        name: &str,
        input: Value,
    ) -> Result<Value, String> {
        match name {
            "worker_models" if input == json!({}) => Ok(self.models_for(policy)),
            "worker_start" => self.submit(owner, args(input)?, None, policy).await,
            "worker_collect" => {
                let r: Collect = args(input)?;
                self.collect(owner, &r.task_ids, r.cursor, r.wait_ms, r.detail)
                    .await
            }
            "worker_followup" => {
                let r: Followup = args(input)?;
                self.followup_scoped(owner, &r.task_id, &r.result_sha256, r.request, policy)
                    .await
            }
            "worker_cancel" => {
                let r: Cancel = args(input)?;
                self.cancel(owner, &r.request_id, &r.task_id).await
            }
            "worker_review" => {
                let r: Review = args(input)?;
                self.review(
                    owner,
                    &r.request_id,
                    &r.task_id,
                    &r.result_sha256,
                    &r.disposition,
                    &r.notes,
                )
                .await
            }
            _ => Err("invalidRequest: unknown tool or invalid arguments".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::catalogue;
    use serde_json::Value;

    fn tool(tools: &[Value], name: &str) -> Value {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .cloned()
            .expect("tool listed")
    }

    #[test]
    fn worker_tools_describe_the_contracts_their_handlers_enforce() {
        let tools = catalogue();
        // Follow-up accepts only a completed task with the exact result hash.
        let followup = tool(&tools, "worker_followup")["description"].to_string();
        assert!(followup.contains("completed") && !followup.contains("settled report"));
        // A start requestId reused for cancel or review is refused.
        for name in ["worker_cancel", "worker_review"] {
            let request_id =
                tool(&tools, name)["inputSchema"]["properties"]["requestId"]["description"]
                    .to_string();
            assert!(
                request_id.contains("Reusing a start requestId is refused"),
                "{name}"
            );
        }
    }
}
