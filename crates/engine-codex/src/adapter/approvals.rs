//! Approvals and questions: routing server requests, the approval facts the
//! policy reads (file-change paths, shell-wrapper unwrapping), and the
//! answers sent back.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use interface_common::sync::lock;
use interface_contracts::{
    agents::ApprovalFacts,
    engine::{EngineEvent, EngineEventKind},
};
use interface_engine::{
    ApprovalResponse, ApprovalVerdict,
    jsonrpc::{RpcId, RpcRemoteError, RpcServerRequest},
    shell_words::{is_plain_byte, shell_word},
};
use serde_json::{Map, Value, json};

use crate::transport::CodexTransport;

use super::session::SessionShared;

const MAX_PENDING_APPROVALS: usize = 128;

/// Interface-owned approval input key carrying a file change's stashed paths.
const INTERFACE_PATHS_KEY: &str = "interfacePaths";
/// File-change items whose paths are remembered for approval facts.
pub(super) const MAX_FILE_CHANGE_ITEMS: usize = 256;

static NEXT_APPROVAL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(super) struct PendingApproval {
    pub(super) rpc_id: RpcId,
    pub(super) method: String,
    pub(super) params: Option<Value>,
    pub(super) responding: bool,
}

pub(super) async fn deny_pending_approvals(
    shared: &SessionShared,
    transport: &CodexTransport,
    note: Option<&str>,
) {
    let pending = std::mem::take(&mut *lock(&shared.pending_approvals));
    for (_, pending) in pending {
        if pending.responding {
            continue;
        }
        let result = denial_result(&pending.method, note);
        let _ = transport.server_result(&pending.rpc_id, result).await;
    }
}

pub(super) async fn handle_server_request(
    shared: &Arc<SessionShared>,
    transport: &Arc<CodexTransport>,
    request: RpcServerRequest,
) {
    if !is_approval_or_question(&request.method) {
        let _ = transport
            .server_error(
                &request.id,
                &RpcRemoteError {
                    code: -32_601,
                    message: format!("unsupported client request {}", request.method),
                    data: None,
                },
            )
            .await;
        return;
    }
    if !server_request_matches(shared, request.params.as_ref()) {
        let _ = transport
            .server_error(
                &request.id,
                &RpcRemoteError {
                    code: -32_602,
                    message: "request belongs to a different Codex thread".into(),
                    data: None,
                },
            )
            .await;
        return;
    }
    if lock(&shared.pending_approvals).len() >= MAX_PENDING_APPROVALS {
        let _ = transport
            .server_error(
                &request.id,
                &RpcRemoteError {
                    code: -32_000,
                    message: "Codex approval queue capacity was reached".into(),
                    data: None,
                },
            )
            .await;
        return;
    }
    let approval_id = format!(
        "codex:{}:{}",
        lock(&shared.session_id),
        NEXT_APPROVAL_ID.fetch_add(1, Ordering::Relaxed)
    );
    let (tool, summary, mut input) = approval_display(&request.method, request.params.as_ref());
    if tool == "fileChange" {
        attach_file_change_paths(shared, &mut input);
    }
    lock(&shared.pending_approvals).insert(
        approval_id.clone(),
        PendingApproval {
            rpc_id: request.id,
            method: request.method,
            params: request.params,
            responding: false,
        },
    );
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input,
        },
    });
}

fn server_request_matches(shared: &SessionShared, params: Option<&Value>) -> bool {
    let protocol = lock(&shared.protocol);
    let expected = protocol.thread_id();
    let actual = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("threadId"))
        .and_then(Value::as_str);
    actual.is_none() || expected == actual
}

fn is_approval_or_question(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "applyPatchApproval"
            | "execCommandApproval"
    ) || is_question_method(method)
}

fn is_question_method(method: &str) -> bool {
    let lower = method.to_ascii_lowercase();
    lower.contains("question") || lower.contains("requestuserinput")
}

fn approval_display(method: &str, params: Option<&Value>) -> (String, String, Value) {
    let input = params.cloned().unwrap_or_else(|| json!({}));
    if is_question_method(method) {
        return ("question".into(), "Codex asks a question".into(), input);
    }
    let tool = if method.contains("fileChange") || method.contains("Patch") {
        "fileChange"
    } else if method.contains("permissions") {
        "permissions"
    } else {
        "command"
    };
    (
        tool.into(),
        format!("Codex requests approval for {tool}"),
        input,
    )
}

/// Remembers the paths a file-change item touches, from `item/started` and
/// `item/fileChange/patchUpdated`, so its path-less approval can name them.
pub(super) fn note_file_change_paths(shared: &SessionShared, method: &str, params: Option<&Value>) {
    let Some(params) = params else {
        return;
    };
    match method {
        "item/started" | "item/completed" => {
            let Some(item) = params
                .get("item")
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("fileChange"))
            else {
                return;
            };
            let Some(id) = non_empty(item.get("id")) else {
                return;
            };
            let mut stash = lock(&shared.file_change_paths);
            if method == "item/completed" {
                stash.retain(|(known, _)| known != id);
            } else {
                remember_file_change_paths(
                    &mut stash,
                    id,
                    file_change_targets(item.get("changes")),
                );
            }
        }
        "item/fileChange/patchUpdated" => {
            if let Some(id) = non_empty(params.get("itemId")) {
                remember_file_change_paths(
                    &mut lock(&shared.file_change_paths),
                    id,
                    file_change_targets(params.get("changes")),
                );
            }
        }
        _ => {}
    }
}

fn non_empty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// Every `path` and `move_path` in a change list; `None` when any change is
/// unreadable, so the approval stays `Unknown` rather than under-reporting.
pub(super) fn file_change_targets(changes: Option<&Value>) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    for change in changes?.as_array()? {
        let path = change.get("path").and_then(Value::as_str)?;
        let moved = match change.get("kind").and_then(|kind| kind.get("move_path")) {
            None | Some(Value::Null) => None,
            Some(target) => Some(target.as_str()?),
        };
        for target in std::iter::once(path).chain(moved) {
            if !paths.iter().any(|known| known == target) {
                paths.push(target.to_owned());
            }
        }
    }
    Some(paths)
}

pub(super) fn remember_file_change_paths(
    stash: &mut VecDeque<(String, Option<Vec<String>>)>,
    item_id: &str,
    paths: Option<Vec<String>>,
) {
    if let Some((_, known)) = stash.iter_mut().find(|(known, _)| known == item_id) {
        *known = match (known.take(), paths) {
            (Some(mut known), Some(paths)) => {
                for path in paths {
                    if !known.contains(&path) {
                        known.push(path);
                    }
                }
                Some(known)
            }
            _ => None,
        };
        return;
    }
    stash.push_back((item_id.to_owned(), paths));
    while stash.len() > MAX_FILE_CHANGE_ITEMS {
        stash.pop_front();
    }
}

/// Puts the stashed paths under `interfacePaths`. Only Interface writes that
/// key: a provider-supplied value is always removed first.
fn attach_file_change_paths(shared: &SessionShared, input: &mut Value) {
    let Some(input) = input.as_object_mut() else {
        return;
    };
    input.remove(INTERFACE_PATHS_KEY);
    let paths = input
        .get("itemId")
        .and_then(Value::as_str)
        .and_then(|id| {
            lock(&shared.file_change_paths)
                .iter()
                .find(|(known, _)| known == id)
                .and_then(|(_, paths)| paths.clone())
        })
        .filter(|paths| !paths.is_empty());
    if let Some(paths) = paths {
        input.insert(INTERFACE_PATHS_KEY.into(), json!(paths));
    }
}

/// Provider-neutral facts for a Codex approval. `permissions`, questions,
/// stdin writes, network escalations, root grants and anything without a
/// readable command or stashed paths stay `Unknown`.
pub(super) fn codex_approval_facts(tool: &str, input: &Value) -> ApprovalFacts {
    let present = |key: &str| input.get(key).is_some_and(|value| !value.is_null());
    let facts = match tool {
        // `writeStdin` feeds a running process: its parent command says nothing
        // about what the input does, and a network approval widens the sandbox.
        "command"
            if input.get("kind").is_some_and(|kind| kind != "command")
                || present("networkApprovalContext") =>
        {
            None
        }
        // A grant asks to write under that root for the rest of the session.
        "fileChange" if present("grantRoot") => None,
        "command" => command_text(input.get("command")).map(|command| ApprovalFacts::Shell {
            command,
            cwd: input
                .get("cwd")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }),
        "fileChange" => input
            .get(INTERFACE_PATHS_KEY)
            .and_then(Value::as_array)
            .filter(|paths| !paths.is_empty())
            .and_then(|paths| {
                paths
                    .iter()
                    .map(|path| path.as_str().map(ToOwned::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .map(|paths| ApprovalFacts::Write { paths }),
        _ => None,
    };
    facts.unwrap_or_else(|| ApprovalFacts::Unknown {
        tool: tool.to_owned(),
    })
}

/// The command as the policy should read it: one harness shell wrapper
/// ([`is_harness_wrapper`]) is replaced by its script; any other command line
/// is kept as sent, and an argv array is joined as POSIX shell words.
fn command_text(command: Option<&Value>) -> Option<String> {
    let text = match command? {
        Value::String(text) => match posix_words(text).as_deref() {
            Some([shell, flag, script]) if is_harness_wrapper(shell, flag, script) => {
                script.clone()
            }
            _ => text.clone(),
        },
        Value::Array(parts) if !parts.is_empty() => {
            let words = parts
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?;
            match words.as_slice() {
                [shell, flag, script] if is_harness_wrapper(shell, flag, script) => {
                    (*script).to_owned()
                }
                _ => words
                    .iter()
                    .map(|word| shell_word(word))
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        }
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// Directories a harness login shell may be named from by absolute path. A
/// shell anywhere else, such as a link an agent made in its own folder, is
/// never unwrapped.
const SYSTEM_SHELL_DIRECTORIES: [&str; 4] =
    ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"];

/// Codex runs a model command as `<login shell> -lc <script>` and reports that
/// wrapper as the approval's command, which the policy would treat as an
/// interpreter. Exactly this shape unwraps: `zsh`, `bash` or `sh`, bare or in
/// a [`SYSTEM_SHELL_DIRECTORIES`] entry, then `-c` or `-lc`, then a script
/// without NUL. Anything else stays wrapped, so the policy asks.
fn is_harness_wrapper(shell: &str, flag: &str, script: &str) -> bool {
    let name = match shell.rsplit_once('/') {
        None => shell,
        Some((directory, name)) if SYSTEM_SHELL_DIRECTORIES.contains(&directory) => name,
        Some(_) => return false,
    };
    matches!(name, "zsh" | "bash" | "sh") && matches!(flag, "-c" | "-lc") && !script.contains('\0')
}

/// Splits a command line into words the way a POSIX shell reads them, undoing
/// what [`shell_word`] and Codex's quoter write: single quotes, double quotes
/// with the `\"`, `\\`, `\$` and `` \` `` escapes (another backslash stays
/// literal), and the `\'` of the `'\''` idiom. Words are separated by spaces
/// and tabs. `None` for unbalanced quotes, a `$` or backquote expansion inside
/// double quotes, a line continuation, any other unquoted backslash, and any
/// other unquoted byte outside [`is_plain_byte`], since each can change what
/// runs.
fn posix_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(next) = chars.next() {
        match next {
            ' ' | '\t' => words.extend(word.take()),
            '\'' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        quoted => current.push(quoted),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' => return None,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '\\' | '$' | '`') => current.push(escaped),
                            '\n' => return None,
                            literal => {
                                current.push('\\');
                                current.push(literal);
                            }
                        },
                        quoted => current.push(quoted),
                    }
                }
            }
            '\\' => match chars.next()? {
                '\'' => word.get_or_insert_with(String::new).push('\''),
                _ => return None,
            },
            plain if u8::try_from(plain).is_ok_and(is_plain_byte) => {
                word.get_or_insert_with(String::new).push(plain);
            }
            _ => return None,
        }
    }
    words.extend(word);
    Some(words)
}

pub(super) fn approval_result(
    pending: &PendingApproval,
    verdict: ApprovalVerdict,
    response: &ApprovalResponse,
) -> Value {
    if is_question_method(&pending.method) && verdict == ApprovalVerdict::Allow {
        let answers = response
            .answers
            .iter()
            .cloned()
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<Map<_, _>>();
        return json!({"answers": answers});
    }
    match verdict {
        ApprovalVerdict::Allow => allow_result(&pending.method, pending.params.as_ref()),
        ApprovalVerdict::Deny => denial_result(&pending.method, response.note.as_deref()),
    }
}

fn allow_result(method: &str, params: Option<&Value>) -> Value {
    match method {
        "item/permissions/requestApproval" => json!({
            "permissions": params
                .and_then(|params| params.get("permissions"))
                .cloned()
                .unwrap_or_else(|| json!([])),
            "scope": "turn"
        }),
        "applyPatchApproval" | "execCommandApproval" => Value::String("approved".into()),
        _ => json!({"decision": "accept"}),
    }
}

pub(super) fn denial_result(method: &str, note: Option<&str>) -> Value {
    match method {
        "item/permissions/requestApproval" => json!({"permissions": {}, "scope": "turn"}),
        "applyPatchApproval" | "execCommandApproval" => {
            json!({"denied": {"rejection": note.unwrap_or("Denied by user")}})
        }
        _ if is_question_method(method) => {
            json!({"answers": {}})
        }
        _ => json!({"decision": "decline"}),
    }
}
