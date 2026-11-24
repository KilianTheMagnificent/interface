# Agent teammates core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Interface agents become persistent teammates: a home folder with job and memory files, one resumable thread, a status, and a Rust approval policy that lets them work freely and asks the owner before risky acts.

**Architecture:** Rust owns everything. `interface-contracts` gains the v2 agent DTOs, `ApprovalFacts` and two launch options. `interface-settings` gains the v2 store, home-folder files and a bounded decision log. `interface-desktop-services` gains the pure approval policy and the agent command wiring. The engine crates enforce an approval gate at launch and translate their own tool vocabulary into facts. The host (`apps/interface-host/src/agents.rs`) starts threads, answers approvals by policy, and guards resume/respond for agent sessions. The React renderer only draws the returned view.

**Tech Stack:** Rust 2024 (MSRV 1.88, clippy pedantic, `unsafe` forbidden), serde, tokio; Electron + React + TypeScript + vitest.

**Spec:** `docs/superpowers/specs/2026-09-15-agent-teammates-core-design.md`

## Global Constraints

- No new external dependencies (Rust or npm). Workspace-internal crate edges only.
- Host protocol version 22 → 23. No new IPC channel; `iface:agentControl` changes shape.
- Rust owns canonical state; `App.tsx` is the only `window.iface` caller; components take props.
- Provider effects fail closed: definitely-unsent is the only replay-safe outcome; unknown outcomes are never retried.
- Persisted v1 store bytes are never modified. v2 lives in `agents.v2.json`.
- Settled look change approved by the owner 2026-09-15: status mark on Agent roster rows. Commit trailer: `Settled-look: Agent sidebar has shaped characters and a vertical roster — approved by the owner 2026-09-15`.
- Agent home root: `~/Interface Agents` (production), `~/Interface Dev Agents` when `INTERFACE_DEV_SUPERVISED=1`.
- Every task runs its crate's `cargo test -p <crate>` and `cargo clippy -p <crate> --all-targets -- -D warnings` (or the Electron equivalents) before committing. Put `$HOME/.cargo/bin` on PATH for cargo: `PATH="$HOME/.cargo/bin:$PATH" cargo …`.
- Worktrees live under `/private/tmp`. Never touch `~/Desktop/Projects/Interface-Rust` (the owner's main checkout).

## Deviations from the spec (decided while planning)

1. Thread state lives in `AgentWorkspace.threads: BTreeMap<agentId, AgentThreadState>`, not inside `AgentProfile`, so run snapshots don't copy thread state. Profiles gain only `home`, `folders`, `instructionsFileSha256`.
2. Threads are keyed by runtime session key (`engine#id`), not `{engine, sessionId}`.
3. `AnswerAgentApproval` is dropped. The renderer keeps calling `respond`; the host guard accepts it only for a pending owner approval and logs it. Other responses to agent sessions are refused definitely-unsent.
4. `SendToAgent` becomes `StartAgentThread` (first message only). Later messages use the normal send path on the live thread; resuming a thread goes through the normal resume path, which the host guard gates.
5. The policy module lives in `interface-desktop-services` (it already depends on `interface-workspace`, which owns the credential classifier), not `interface-collaboration`.
6. Probe results: P1 (Claude `PreToolUse` "ask" overrides allow rules) passed on 2026-09-15 with Claude Code; P2 schema check passed (Codex 0.154.0 `thread/start` has `approvalPolicy`, `approvalsReviewer`, `config`; the response echoes `sandbox.writableRoots` and `networkAccess`). Codex must also send `approvalsReviewer: "user"`.

## File Structure

| File | Responsibility | Task |
| --- | --- | --- |
| `crates/interface-contracts/src/agents.rs` | v2 DTOs, commands, view, `ApprovalFacts` | 1 |
| `crates/interface-contracts/src/engine.rs` | `LaunchOptions`/`ResumeOptions` gain `approval_gate`, `additional_directories` | 1 |
| `crates/interface-engine/src/adapter.rs` | `EngineAdapter::approval_facts` default | 1 |
| `crates/interface-runtime/src/runtime.rs` | `SessionRuntime::approval_facts(session_key, tool, input)` | 1 |
| `apps/interface-host/src/protocol.rs`, `electron/src/main/rust-host-client.ts`, `docs/PROTOCOL_FACTS.md` | protocol 23 | 1 |
| `electron/src/shared/agents.ts`, `electron/src/shared/agents.test.ts`, `electron/src/fixtures/agent-workspace-v23.json` | TS mirror + shared fixture | 1 |
| `crates/interface-settings/src/agents.rs` (+ `agents/tests.rs`) | v2 store: migration, threads, folders, statuses, bindings | 2 |
| `crates/interface-settings/src/agent_home.rs` | home folder, marker, instruction/memory files | 2 |
| `crates/interface-settings/src/agent_decisions.rs` | bounded per-agent JSONL decision log | 2 |
| `crates/interface-desktop-services/src/service.rs` (+ `agent_control.rs`) | command wiring, pending approvals, host-facing API | 2 |
| `crates/interface-workspace/src/search.rs` | `pub fn is_credential_path` | 3 |
| `crates/interface-desktop-services/src/agent_shell.rs` | quote-aware shell tokenizer | 3 |
| `crates/interface-desktop-services/src/agent_policy.rs` | pure `classify(facts, scope)` + risky tables + tests | 3 |
| `crates/engine-claude/src/{launch,adapter}.rs` | gate hook, `--add-dir`, facts | 4 |
| `crates/engine-codex/src/adapter.rs` | untrusted + reviewer user + writable roots + verification, facts | 4 |
| `crates/engine-acp/src/adapter.rs` | refuse gate/dirs, facts | 4 |
| `electron/src/renderer/components/{AgentMenu,AgentWorkspacePanel,Conversation}.tsx`, `App.tsx`, fixtures, `docs/SETTLED_LOOKS.md` | UI | 5 |
| `apps/interface-host/src/{agents,main,router,dispatcher}.rs` | thread start, approval driver, guard, routines | 6 |
| `apps/interface-host/tests/agent_gate_live.rs` | ignored live probes P1/P2/P4/P5 | 7 |
| `docs/areas/agents.md`, `docs/AGENT_WORKSPACE_PLAN.md`, `docs/CODEMAP.md`, `docs/TODO.md` | docs | 7 |

Parallelism: Task 1 first. Tasks 2, 3, 4, 5 run in parallel on branches cut from Task 1. Task 6 after 2–4 merge. Task 7 last.

---

### Task 1: Contracts, runtime hook and protocol 23

**Files:**
- Modify: `crates/interface-contracts/src/agents.rs`, `crates/interface-contracts/src/engine.rs`
- Modify: `crates/interface-engine/src/adapter.rs`, `crates/interface-runtime/src/runtime.rs`
- Modify: `apps/interface-host/src/protocol.rs`, `electron/src/main/rust-host-client.ts`, `docs/PROTOCOL_FACTS.md`
- Modify: `electron/src/shared/agents.ts`, `electron/src/shared/agents.test.ts`
- Create: `electron/src/fixtures/agent-workspace-v23.json`

**Interfaces:**
- Produces (Rust, `interface_contracts::agents`): `AGENT_WORKSPACE_VERSION`, `AgentProfile{home, folders, instructions_file_sha256}`, `AgentThreadStatus`, `AgentThread`, `AgentPastThread`, `AgentThreadState`, `AgentWorkspace{threads}`, `AgentStatus`, `AgentPendingApproval`, `AgentFileText`, `AgentFiles`, `AgentDecisionKind`, `AgentDeliveryOutcome`, `AgentDecision`, `AgentControlResponse`, `ApprovalFacts`, new `AgentCommand` variants.
- Produces: `LaunchOptions.approval_gate: Option<bool>`, `LaunchOptions.additional_directories: Option<Vec<String>>`, same two on `ResumeOptions`.
- Produces: `EngineAdapter::approval_facts(&self, tool: &str, input: &serde_json::Value) -> ApprovalFacts` (default `Unknown { tool }`); `SessionRuntime::approval_facts(&self, session_key: &str, tool: &str, input: &Value) -> ApprovalFacts` (unknown engine → `Unknown`).
- Produces (TS, `electron/src/shared/agents.ts`): the same names in camelCase, `AgentControlResponse`, `AgentCommand` union with the new kinds.

- [ ] **Step 1: Add the Rust DTOs.** In `agents.rs`, add to `AgentProfile` (after `shape`):

```rust
    /// Rust-owned: set when the home folder is created. Renderer values are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Rust-owned: changed only by `SaveAgentFolders`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// SHA-256 (lowercase hex) of the instruction files Interface last wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_file_sha256: Option<String>,
```

Add the thread and view types:

```rust
pub const AGENT_WORKSPACE_VERSION: u8 = 2;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentThreadStatus { #[default] Idle, Working, Waiting }

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentThread {
    pub session_key: String,
    pub started_at_ms: u64,
    pub status: AgentThreadStatus,
    #[serde(default)]
    pub interrupted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPastThread { pub session_key: String, pub started_at_ms: u64, pub ended_at_ms: u64 }

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentThreadState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<AgentThread>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub past: Vec<AgentPastThread>,
    #[serde(default)]
    pub last_read_at_ms: u64,
    #[serde(default)]
    pub last_activity_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus { Idle, Working, NeedsAttention, Unread }

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPendingApproval {
    pub agent_id: String,
    pub session_key: String,
    pub approval_id: String,
    pub tool: String,
    pub summary: String,
    pub reason: String,
    pub asked_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileText { pub text: String, pub sha256: String, pub too_large: bool }

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFiles {
    pub home: Option<String>,
    pub exists: bool,
    pub marker_ok: bool,
    pub instructions: Option<AgentFileText>,
    pub instructions_conflict: bool,
    pub memory: Option<AgentFileText>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum ApprovalFacts {
    Read { paths: Vec<String> },
    Search { paths: Vec<String> },
    Write { paths: Vec<String> },
    Delete { paths: Vec<String> },
    Shell { command: String, cwd: Option<String> },
    WebSearch {},
    WebFetch { url: String, method: String },
    Mcp { server: String, tool: String },
    /// Harness-internal bookkeeping with no external effect (todo lists, subagent spawn).
    Internal { tool: String },
    Unknown { tool: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentDecisionKind { Allowed, Asked, OwnerAllowed, OwnerDenied, FailedClosed }

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentDeliveryOutcome { Sent, Unsent, Unknown, Pending }

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDecision {
    pub at_ms: u64,
    pub session_key: String,
    pub approval_id: String,
    pub tool: String,
    pub summary: String,
    pub facts: ApprovalFacts,
    pub decision: AgentDecisionKind,
    pub reason: String,
    pub outcome: AgentDeliveryOutcome,
}

/// Every `iface:agentControl` answer. Never persisted.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentControlResponse {
    #[serde(flatten)]
    pub workspace: AgentWorkspace,
    pub statuses: BTreeMap<String, AgentStatus>,
    pub pending_approvals: Vec<AgentPendingApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<AgentFiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Vec<AgentDecision>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}
```

Add `threads` to `AgentWorkspace` and bump the default version:

```rust
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub threads: BTreeMap<String, AgentThreadState>,
```

`Default` sets `version: AGENT_WORKSPACE_VERSION` and `threads: BTreeMap::new()`.

Add the commands to `AgentCommand`:

```rust
    /// Host-only. Creates the agent's thread with `text` as its first prompt.
    StartAgentThread { agent_id: String, text: String, request_id: String },
    NewThread { expected_revision: u64, agent_id: String },
    MarkRead { agent_id: String },
    ReadAgentFiles { agent_id: String },
    SaveAgentMemory { agent_id: String, expected_sha256: String, text: String },
    OverwriteAgentInstructionsFile { agent_id: String, expected_sha256: String },
    SaveAgentFolders { expected_revision: u64, agent_id: String, folders: Vec<String> },
    ReadAgentDecisions { agent_id: String, limit: u16 },
    RecreateAgentHome { agent_id: String },
    RevealAgentHome { agent_id: String },
```

- [ ] **Step 2: Launch/resume options.** In `engine.rs` add to both `LaunchOptions` and `ResumeOptions`:

```rust
    /// Every tool call must raise `approval.requested`; adapters that cannot guarantee it refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_gate: Option<bool>,
    /// Extra folders granted natively; adapters that cannot grant them refuse definitely-unsent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<Vec<String>>,
```

Fix every struct literal that no longer compiles with `..Default::default()` or explicit `None` (`cargo check --workspace --all-targets`).

- [ ] **Step 3: Adapter and runtime hooks.** In `interface-engine/src/adapter.rs` (`EngineAdapter`):

```rust
    /// Provider-neutral description of an approval request for Interface's agent policy.
    /// Unrecognized tools must stay `Unknown`, which the policy sends to the owner.
    fn approval_facts(&self, tool: &str, _input: &serde_json::Value) -> interface_contracts::agents::ApprovalFacts {
        interface_contracts::agents::ApprovalFacts::Unknown { tool: tool.to_owned() }
    }
```

In `interface-runtime/src/runtime.rs` (`impl SessionRuntime`):

```rust
    /// Facts for an approval raised by `session_key`, from that session's adapter.
    #[must_use]
    pub fn approval_facts(&self, session_key: &str, tool: &str, input: &serde_json::Value) -> interface_contracts::agents::ApprovalFacts {
        let unknown = || interface_contracts::agents::ApprovalFacts::Unknown { tool: tool.to_owned() };
        let Ok((engine, _)) = interface_contracts::ipc::parse_key(session_key) else { return unknown() };
        self.inner.adapters.get(&engine).map_or_else(unknown, |adapter| adapter.approval_facts(tool, input))
    }
```

Add a runtime unit test with a stub adapter proving it delegates and that an unregistered engine yields `Unknown`.

- [ ] **Step 4: Fixture.** Create `electron/src/fixtures/agent-workspace-v23.json` with `"protocolVersion": 23`, a `commands` array containing one of every `AgentCommand` kind (reuse the v14 profile/routine values; add `"home": "/fixture/agents/Research"`, `"folders": ["/fixture/project"]`, `"instructionsFileSha256"` as 64 `a` characters on the saved profile), a v2 `state` with one thread (`"sessionKey": "claude#11111111-1111-4111-8111-111111111111"`, status `working`, one past thread, `lastReadAtMs` < `lastActivityAtMs`), and a `response` object that is `state` flattened plus `statuses`, one `pendingApprovals` entry, `files`, and two `decisions` (one `shell` fact allowed, one `mcp` fact asked).

- [ ] **Step 5: Round-trip tests.** Keep the v14 test (v1 documents still deserialize). Add in `agents.rs` tests:

```rust
#[test]
fn v23_fixture_round_trips_commands_state_and_response() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../../../electron/src/fixtures/agent-workspace-v23.json")).unwrap();
    for value in fixture["commands"].as_array().unwrap() {
        let command: AgentCommand = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(command).unwrap(), *value);
    }
    let state: AgentWorkspace = serde_json::from_value(fixture["state"].clone()).unwrap();
    assert_eq!(state.version, AGENT_WORKSPACE_VERSION);
    assert_eq!(serde_json::to_value(&state).unwrap(), fixture["state"]);
    let response: AgentControlResponse = serde_json::from_value(fixture["response"].clone()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
}

#[test]
fn approval_facts_reject_foreign_fields() {
    assert!(serde_json::from_value::<ApprovalFacts>(serde_json::json!({"kind":"shell","command":"ls","cwd":null,"sudo":true})).is_err());
}
```

- [ ] **Step 6: TS mirror.** Update `electron/src/shared/agents.ts`: add `home?`, `folders?`, `instructionsFileSha256?` to `AgentProfile`; add `AgentThreadStatus`, `AgentThread`, `AgentPastThread`, `AgentThreadState`, `threads?: Record<string, AgentThreadState>` on `AgentWorkspace` with `version: 1 | 2`; add `AgentStatus`, `AgentPendingApproval`, `AgentFileText`, `AgentFiles`, `ApprovalFacts` (discriminated on `kind`), `AgentDecision`, `AgentControlResponse = AgentWorkspace & { statuses: Record<string, AgentStatus>; pendingApprovals: AgentPendingApproval[]; files?: AgentFiles; decisions?: AgentDecision[]; sessionKey?: string }`. Extend the command unions: `AgentMutation` gains `newThread` and `saveAgentFolders` (they carry `expectedRevision`); a new `AgentRequest` union covers `startAgentThread`, `markRead`, `readAgentFiles`, `saveAgentMemory`, `overwriteAgentInstructionsFile`, `readAgentDecisions`, `recreateAgentHome`, `revealAgentHome` (no revision). `AgentCommand = {kind:"read"} | importLegacy | AgentRequest | (AgentMutation & {expectedRevision:number})`. `emptyAgentWorkspace()` returns version 2 and `statuses: {}`/`pendingApprovals: []` via a new `emptyAgentControlResponse()`. Update `ipc.ts` and `preload.cts` return types from `AgentWorkspace` to `AgentControlResponse`. In `agents.test.ts` add a test that parses the v23 fixture, checks every command `kind` is a member of the TS union (a `satisfies` table) and that `response` has `statuses` and `pendingApprovals`.

- [ ] **Step 7: Protocol 23.** `apps/interface-host/src/protocol.rs` `PROTOCOL_VERSION = 23`; `electron/src/main/rust-host-client.ts` `RUST_HOST_PROTOCOL_VERSION = 23`; update any test pinned to 22 (`rg -n "\b22\b" apps/interface-host/src electron/src/main --glob '*test*'` and protocol fixtures); `docs/PROTOCOL_FACTS.md` table row to 23 plus a paragraph: "Protocol v23 changes `iface:agentControl` to the agent teammates v2 response (`statuses`, `pendingApprovals`, optional `files`/`decisions`/`sessionKey`), adds thread, memory, folder and decision commands, and adds `approvalGate`/`additionalDirectories` to launch and resume options. The v1 agent store file is preserved; v2 is `agents.v2.json`."

- [ ] **Step 8: Verify and commit.**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-contracts -p interface-engine -p interface-runtime && PATH="$HOME/.cargo/bin:$PATH" cargo check --workspace --all-targets && (cd electron && npx vitest run src/shared/agents.test.ts && npm run typecheck)`
Expected: all pass. (`typecheck` may fail in renderer files that read `AgentWorkspace` — fix types only where the compiler points; behavior changes belong to Task 5.)

```bash
git add -A && git commit -m "Agent teammates contracts, approval facts hook and protocol 23"
```

---

### Task 2: v2 store, home folder files, decision log, desktop wiring

**Files:**
- Modify: `crates/interface-settings/src/agents.rs`, `crates/interface-settings/src/agents/tests.rs`, `crates/interface-settings/src/lib.rs`
- Create: `crates/interface-settings/src/agent_home.rs`, `crates/interface-settings/src/agent_decisions.rs`
- Modify: `crates/interface-desktop-services/src/service.rs`
- Create: `crates/interface-desktop-services/src/agent_control.rs`

**Interfaces:**
- Consumes: Task 1 types. Task 2 does not call the policy; Task 6 calls `agent_policy::classify` directly.
- Produces (`interface_settings`):
  - `AgentStore::open(v2_file: &Path, v1_file: &Path) -> Result<AgentStore, SettingsError>`
  - `AgentStore::response(&self, pending: &[AgentPendingApproval]) -> AgentControlResponse`
  - `AgentStore::binding(&self, session_key: &str) -> Option<AgentBinding>`
  - `AgentStore::bind_thread(&mut self, agent_id: &str, session_key: &str, now: u64) -> Result<(), SettingsError>`
  - `AgentStore::set_home(&mut self, agent_id: &str, home: &str, instructions_sha: &str) -> Result<(), SettingsError>`
  - `AgentStore::set_instructions_sha(&mut self, agent_id: &str, sha: &str) -> Result<(), SettingsError>`
  - `pub struct AgentBinding { pub agent_id: String, pub roots: Vec<PathBuf>, pub cwd: PathBuf, pub thread: bool }`
  - `agent_home::{agents_root, folder_name, ensure_home, recreate_home, render_instructions, write_instruction_files, read_files, save_memory, reveal, HomeError}`
  - `AgentDecisionLog::{open(dir: &Path), append(&mut self, agent_id: &str, entry: &AgentDecision), read(&self, agent_id: &str, limit: usize) -> Vec<AgentDecision>}`
- Produces (`DesktopService`, all `pub fn`, errors `DesktopServiceError`):
  - `set_agents_root(&self, root: PathBuf)`
  - `agent_binding(&self, session_key: &str) -> Option<AgentBinding>`
  - `prepare_agent_thread(&self, agent_id: &str) -> Result<AgentThreadLaunch, DesktopServiceError>` where `pub struct AgentThreadLaunch { pub engine: String, pub model: String, pub effort: String, pub name: String, pub home: String, pub folders: Vec<String> }`
  - `bind_agent_thread(&self, agent_id: &str, session_key: &str) -> Result<(), DesktopServiceError>`
  - `home_directory(&self) -> &Path`
  - `record_agent_decision(&self, agent_id: &str, entry: &AgentDecision) -> Result<(), DesktopServiceError>`
  - `add_pending_agent_approval(&self, pending: AgentPendingApproval)`
  - `take_pending_agent_approval(&self, session_key: &str, approval_id: &str) -> Option<AgentPendingApproval>`
  - `clear_pending_agent_approvals(&self, session_key: &str)`
  - `agent_control_response(&self) -> Result<Value, DesktopServiceError>`

- [ ] **Step 1: Store tests first.** In `agents/tests.rs` add failing tests:
  - `migrates_v1_once_and_leaves_v1_bytes`: write a v1 file (use the v14 fixture `state`), open with both paths, assert v2 file exists with `version: 2`, v1 bytes unchanged (compare `std::fs::read`), reopen → no second migration (revision unchanged).
  - `open_marks_non_idle_thread_interrupted`.
  - `save_agent_preserves_rust_owned_fields`: store has `home`, `folders`, `sha`; `SaveAgent` with those fields changed/removed keeps the stored values.
  - `engine_change_moves_thread_to_past` and `new_thread_moves_current_to_past_capped_at_20`.
  - `mark_read_is_noop_when_already_read`.
  - `folders_validate_shape` (relative path, >16 entries, duplicates rejected).
  - `observe_thread_status_and_activity` (working → idle updates `last_activity_at_ms`; approval → waiting; SessionClosed → idle).
  - `statuses_precedence`: needsAttention (pending approval, interrupted, waiting run, unknown run) > working (thread working or run running/starting) > unread > idle.
  - `binding_for_thread_and_run`: thread key → roots `[home, folders…]`, cwd home, `thread: true`; run key → roots `[home, folders…, run.cwd]`, cwd run.cwd, `thread: false`; unbound key → `None`.
  - `remove_agent_drops_threads_entry`.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-settings agents` — Expected: FAIL (missing functions).

- [ ] **Step 2: Implement the store.** In `agents.rs`:
  - `validate`: `version == AGENT_WORKSPACE_VERSION`; profile `home` absolute ≤4096; `folders` ≤16, absolute, unique, ≤4096; `instructions_file_sha256` 64 lowercase hex; `threads` keys ⊆ agent ids; thread `session_key` 1..=256; `past.len() <= 20`; run snapshots validated the same way (they embed profiles).
  - `open(v2, v1)`: if `v2` exists read it; else if `v1` exists read it with the same `JsonLimits` into `AgentWorkspace` (new fields default), require `version == 1`, set `version = 2`, and `commit` to the v2 target (snapshot `Missing`). Lock name stays `.agents.lock`. After load, set every `threads[*].current` with status ≠ Idle to `interrupted = true, status = Idle`, plus the existing run recovery, in one commit.
  - `SaveAgent`: copy `home`, `folders`, `instructions_file_sha256` from the stored profile onto the incoming one (new agents get `None`/empty). If the stored `engine` differs and a current thread exists, move it to `past` (`ended_at_ms = now`, cap 20, drop oldest).
  - `RemoveAgent`: also `threads.remove(&id)`.
  - `NewThread`: revision check; move current to past as above.
  - `MarkRead`: if `last_read_at_ms >= last_activity_at_ms` and not interrupted → return without commit; else set `last_read_at_ms = max(now, last_activity_at_ms)`, clear `interrupted`, commit.
  - `SaveAgentFolders`: revision check, shape validation only (filesystem checks happen in desktop services), set `folders`.
  - File/decision/thread-start commands are not store commands: `command` returns `invalid("Handled by the desktop service.")` for them.
  - `bind_thread`: set `threads[agent].current = AgentThread{session_key, started_at_ms: now, status: Working, interrupted: false}`, `last_activity_at_ms = now`, insert the assignment (reuse `make_assignment_room`).
  - `observe`: before the existing run logic, if `key` equals some agent's current thread key, map `StatusChanged{Working}`/`UserMessage` → Working; `ApprovalRequested`/`StatusChanged{Waiting}` → Waiting and `last_activity_at_ms = now`; `StatusChanged{Idle}` after Working/Waiting → Idle and `last_activity_at_ms = now`; `StatusChanged{Failed}`/non-retryable `Error`/`SessionClosed` → Idle and `last_activity_at_ms = now`. Commit only when the state changed. Then continue with the run logic.
  - `statuses(&self, pending)` and `response(&self, pending)`; `binding(&self, key)` per the tests.
  - Keep `#[allow(clippy::too_many_lines)]` only where it already exists; split helpers instead of growing `command`.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-settings agents` — Expected: PASS.

- [ ] **Step 3: Home folder module tests.** Create `agent_home.rs` with `#[cfg(test)] mod tests` (use `tempfile`, already a dev-dependency of the crate — check `Cargo.toml`; if absent, use `std::env::temp_dir()` with a unique subdirectory instead of adding a dependency):
  - `folder_name_sanitizes`: `"Re/se:arch\u{7}  Bot"` → `"Research Bot"`-style (slashes, colons, control chars removed; whitespace collapsed; trimmed; ≤64 chars; empty → `"Agent"`).
  - `ensure_home_creates_marker_and_files_with_unique_suffix`: a pre-existing `Research` folder without marker → creates `Research 2`; marker JSON has the agent id; `AGENTS.md` and `CLAUDE.md` are byte-identical and contain the instructions and home path; `MEMORY.md` equals the template.
  - `ensure_home_refuses_marker_mismatch_and_missing_folder` (returns `HomeError::MarkerMismatch` / `HomeError::Missing`).
  - `write_instruction_files_detects_conflict`: edit `CLAUDE.md` on disk → `Conflict`; matching sha → rewritten and new sha returned.
  - `save_memory_compare_and_save`: stale sha → `Conflict`; >256 KiB → refused; ok → new sha.
  - `read_files_reports_too_large_memory` (file >256 KiB → `too_large: true`, text empty).

- [ ] **Step 4: Implement `agent_home.rs`.**

```rust
pub const MEMORY_LIMIT: usize = 256 * 1024;
pub const MARKER: &str = ".interface-agent.json";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HomeError {
    #[error("This agent's folder is missing. Recreate it to continue.")] Missing,
    #[error("That folder belongs to a different agent.")] MarkerMismatch,
    #[error("The file changed since it was opened. Reload before saving.")] Conflict,
    #[error("Memory is larger than 256 KiB; edit it in Finder.")] TooLarge,
    #[error("{0}")] Io(String),
}

#[must_use]
pub fn agents_root(home_dir: &Path, dev: bool) -> PathBuf {
    home_dir.join(if dev { "Interface Dev Agents" } else { "Interface Agents" })
}
```

  `render_instructions(profile: &AgentProfile, home: &str) -> String` produces exactly the template in the spec (header comment, `# {name}`, instructions, working agreement with `{home}` and folders joined by ", " or "none"). Writes use a sibling temp file (`.{name}.tmp-{pid}`) then `std::fs::rename`. `sha256` uses the `sha2` crate if `interface-settings` already depends on it; otherwise use `interface_storage`'s existing hash helper (search `rg -n "Sha256" crates/interface-storage/src`); do not add a dependency. `reveal(home)` runs `std::process::Command::new("/usr/bin/open").arg(home).status()` and maps failure to `HomeError::Io`.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-settings agent_home` — Expected: PASS.

- [ ] **Step 5: Decision log.** Create `agent_decisions.rs`: directory `<dir>/<hex(agent_id)>/`, files `segment-{:020}.jsonl`; append one JSON line; when the active segment would exceed 256 KiB start the next id; after rotation delete segments beyond the newest 16. `read(agent, limit)` reads newest segments backwards, skips unparsable lines, returns newest-first up to `limit.min(200)`. Tests: append/read order, rotation at the size boundary (use small entries in a loop), retention of 16 segments, torn last line skipped, hex directory names for ids containing `/`.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-settings agent_decisions` — Expected: PASS.

- [ ] **Step 6: Desktop wiring.** Create `interface-desktop-services/src/agent_control.rs` holding the agent command handling extracted from `service.rs` (the `CHANNELS.agent_control` branch calls `self.agent_control(payload)`). State added to `DesktopService`: `pending_agent_approvals: Mutex<Vec<AgentPendingApproval>>` (cap 64, oldest dropped), `agent_decisions: Mutex<Option<AgentDecisionLog>>` (opened at `<settings root>/agents/decisions`), `agents_root: Mutex<Option<PathBuf>>` (default `home_directory.join("Interface Agents")`). `open_agents` opens `agents.v2.json` with `agents.v1.json` as legacy.
  Command handling returns `serde_json::to_value(AgentControlResponse)`:
  - `StartAgentThread` → `invalid-payload` "Starting a thread is handled by the host."
  - `ReadAgentFiles` → `agent_home::read_files`, response `files`.
  - `SaveAgentMemory` → `save_memory`; conflict → error code `agent-file-conflict` (message from `HomeError`).
  - `OverwriteAgentInstructionsFile` → re-render and write when the on-disk sha equals `expected_sha256`; then `store.set_instructions_sha`.
  - `SaveAgentFolders` → for each folder: `std::fs::canonicalize`, must be a directory, not `/`, not `home_directory`, not under `/System`, `/Library`, `/usr`, `/bin`, `/sbin`, `/etc`, `/private/etc`, `/private/var`, not the agent's home, and `!interface_workspace::is_credential_path` (Task 3; until merged, skip this one check behind a `// merged in Task 3` note) → store command with canonical strings → if home exists regenerate instruction files (conflict leaves files untouched).
  - `ReadAgentDecisions` → response `decisions`.
  - `RecreateAgentHome` → `recreate_home` then `store.set_home`.
  - `RevealAgentHome` → `ensure_home` (creates on first use) then `reveal`.
  - `SaveAgent` → store command; then, if the profile has a home and instructions changed, `write_instruction_files` with the stored sha (conflict is silent here; `ReadAgentFiles` reports it).
  - Everything else → store command.
  Every response includes `statuses` and `pending_approvals` computed from the pending list.
  Host-facing functions per the Interfaces block; `prepare_agent_thread` refuses when a current thread exists ("This agent already has a thread; send to it instead."), creates the home with `ensure_home` if `home` is `None` and records it with `set_home`, and refuses `HomeError::Missing`/`MarkerMismatch` with their messages.
  Tests in `service_tests.rs`: dispatch `readAgentFiles` for an agent without a home (exists false), `revealAgentHome` with `agents_root` in a temp dir (home created; stub `reveal` by pointing `agents_root` at a temp dir and asserting the folder exists — run `reveal` only when `INTERFACE_TEST_REVEAL=1`), pending approvals flow into `statuses` as `needsAttention`, `take_pending_agent_approval` removes exactly one matching entry, `saveAgentFolders` refuses `/` and the home directory.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-settings -p interface-desktop-services && PATH="$HOME/.cargo/bin:$PATH" cargo clippy -p interface-settings -p interface-desktop-services --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Commit.** `git add -A && git commit -m "Agent store v2, home folders, decision log and desktop wiring"`

---

### Task 3: Approval policy and shell tokenizer

**Files:**
- Modify: `crates/interface-workspace/src/search.rs` (+ its tests)
- Create: `crates/interface-desktop-services/src/agent_shell.rs`, `crates/interface-desktop-services/src/agent_policy.rs`
- Modify: `crates/interface-desktop-services/src/lib.rs` (`pub mod agent_policy; mod agent_shell;`)

**Interfaces:**
- Consumes: `interface_contracts::agents::ApprovalFacts` (Task 1), `interface_common::paths::lexically_normalized`.
- Produces:
  - `interface_workspace::is_credential_path(path: &str) -> bool` — true when any path segment is a sensitive directory or a credential leaf name (same rules as `segment_is_sensitive`; no generated-directory exclusions).
  - `interface_desktop_services::agent_policy::AgentScope { pub roots: Vec<PathBuf>, pub cwd: PathBuf, pub home_dir: PathBuf }`
  - `interface_desktop_services::agent_policy::PolicyDecision { Allow { reason: String }, AskOwner { reason: String } }` with `fn is_allow(&self) -> bool` and `fn reason(&self) -> &str`
  - `interface_desktop_services::agent_policy::classify(facts: &ApprovalFacts, scope: &AgentScope) -> PolicyDecision`

- [ ] **Step 1: Credential helper.** Add and test:

```rust
/// True when any segment names credential material. Unlike `is_search_excluded_path`,
/// generated directories (node_modules, target) are not excluded.
#[must_use]
pub fn is_credential_path(path: &str) -> bool {
    path.replace('\\', "/").to_lowercase().split('/').filter(|s| !s.is_empty()).any(segment_is_sensitive)
}
```

Tests: `/Users/x/.ssh/id_ed25519`, `/p/.env`, `/p/.env.local`, `/p/secrets/token.txt`, `/p/service-account.json` → true; `/p/src/env.rs`, `/p/node_modules/x.js` → false.

- [ ] **Step 2: Tokenizer tests.** In `agent_shell.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub struct Segment { pub words: Vec<String>, pub redirects: Vec<String>, pub substitution: bool }

#[derive(Debug, PartialEq, Eq)]
pub struct ShellParseError;

/// Splits on unquoted `&&`, `||`, `;`, `|`, `&` and newlines. Quotes are removed from words.
/// `$(`, backticks and `<(`/`>(` set `substitution`. `>`, `>>`, `2>`, `&>`, `>|` capture the next word as a redirect target.
pub fn parse(command: &str) -> Result<Vec<Segment>, ShellParseError>
```

Tests (each asserts exact segments): `ls -la` → one segment; `cd a && cargo test | tee log.txt` → 3 segments with `tee` words; `echo "a && b"` → one segment, word `a && b`; `echo 'x;y'`; `git commit -m "it's"`; `cat <<EOF` → `ShellParseError` (heredocs unsupported → owner); `echo $(whoami)` → substitution; `` echo `id` `` → substitution; `printf x > out.txt 2> err.txt` → redirects `out.txt`, `err.txt`; `cmd &> /dev/null`; unterminated `"abc` → error; `a\ b` → word `a b`; empty/whitespace → zero segments; `FOO=1 BAR=2 npm test` → words keep assignments (policy strips them).

- [ ] **Step 3: Implement the tokenizer** as a single-pass char state machine (states: normal, single-quoted, double-quoted, escape). Heredoc `<<` → error. No dependency.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-desktop-services agent_shell` — Expected: PASS.

- [ ] **Step 4: Policy test table.** In `agent_policy.rs` tests, build `scope()` = roots `["/work/agents/Bot", "/work/project"]`, cwd `/work/agents/Bot`, home_dir `/Users/me`, and a table `(facts, expect_allow, reason_prefix)`. Minimum rows (write all of these; add more to exceed 100):

```text
READ   /etc/hosts                                  allow read
READ   /Users/me/.ssh/id_rsa                       ask   credentialPath
SEARCH []                                          allow search
WRITE  /work/project/src/main.rs                   allow inScopeWrite
WRITE  /work/project/.env                          ask   credentialPath
WRITE  /tmp/x                                      ask   outsideScope
WRITE  /work/project/../other/x                    ask   outsideScope
DELETE /work/agents/Bot/old.txt                    allow inScopeWrite
DELETE /work/elsewhere                             ask   outsideScope
WEBSEARCH                                          allow webSearch
WEBFETCH GET https://example.com                   allow webFetch
WEBFETCH POST https://example.com                  ask   webMethod
MCP github get_issue                               allow mcpRead
MCP github list_pulls                              allow mcpRead
MCP github create_issue                            ask   mcpAction
MCP gmail send_message                             ask   mcpAction
MCP slack post_message                             ask   mcpAction
INTERNAL TodoWrite                                 allow internal
UNKNOWN AskUserQuestion                            ask   unknownTool
SHELL ls -la                                       allow shellInScope
SHELL cargo test -p foo                            allow shellInScope
SHELL cd /work/project && npm test | tail -5       allow shellInScope
SHELL git status && git diff                       allow shellInScope
SHELL git commit -am wip                           allow shellInScope
SHELL git push origin main                         ask   risky:git-push
SHELL git -C /work/project push                    ask   risky:git-push
SHELL git reset --hard HEAD~1                      ask   risky:git-reset-hard
SHELL git clean -fdx                               ask   risky:git-clean
SHELL git branch -D feature                        ask   risky:git-branch-delete
SHELL gh pr create --fill                          ask   risky:gh
SHELL gh pr view 12                                allow shellInScope
SHELL gh api repos/o/r/issues -f title=x           ask   risky:gh
SHELL gh api repos/o/r                             allow shellInScope
SHELL curl https://example.com                     allow shellInScope
SHELL curl -X POST https://example.com             ask   risky:curl-upload
SHELL curl -d a=1 https://example.com              ask   risky:curl-upload
SHELL curl --json '{}' https://x.dev               ask   risky:curl-upload
SHELL wget --post-data=a https://x.dev             ask   risky:curl-upload
SHELL npm publish                                  ask   risky:publish
SHELL cargo publish                                ask   risky:publish
SHELL docker push img                              ask   risky:publish
SHELL vercel --prod                                ask   risky:deploy
SHELL kubectl apply -f x.yaml                      ask   risky:deploy
SHELL kubectl get pods                             allow shellInScope
SHELL terraform plan                               allow shellInScope
SHELL terraform apply                              ask   risky:deploy
SHELL aws s3 ls                                    ask   risky:cloud
SHELL sudo ls                                      ask   risky:system
SHELL ssh host                                     ask   risky:system
SHELL scp a host:b                                 ask   risky:system
SHELL rsync -a a host:/b                           ask   risky:system
SHELL rsync -a a /work/project/b                   allow shellInScope
SHELL open https://example.com                     ask   risky:system
SHELL osascript -e 'x'                             ask   risky:system
SHELL security find-generic-password -s x          ask   risky:system
SHELL launchctl list                               ask   risky:system
SHELL defaults write com.x y 1                     ask   risky:system
SHELL brew install jq                              ask   risky:install
SHELL npm install -g x                             ask   risky:install
SHELL npm install                                  allow shellInScope
SHELL mail -s hi a@b.c                             ask   risky:send
SHELL rm -rf /work/project/build                   allow shellInScope
SHELL rm -rf /work/project                         ask   risky:rm-root
SHELL rm -rf ~                                     ask   risky:rm-root
SHELL rm /tmp/x                                    ask   outsideScope
SHELL mv a.txt /tmp/                               ask   outsideScope
SHELL cp -r src /work/project/backup               allow shellInScope
SHELL ln -s /etc/passwd link                       allow shellInScope
SHELL chmod 600 /Users/me/.ssh/config              ask   credentialPath
SHELL echo hi > /tmp/out.txt                       ask   outsideScope
SHELL echo hi > notes.txt                          allow shellInScope
SHELL echo hi >> /work/project/.env                ask   credentialPath
SHELL cat /Users/me/.aws/credentials               ask   credentialPath
SHELL echo $(whoami)                               ask   shellSubstitution
SHELL bash -c 'git push'                           ask   risky:interpreter
SHELL sh -c ls                                     ask   risky:interpreter
SHELL eval "$CMD"                                  ask   risky:interpreter
SHELL FOO=1 git push                               ask   risky:git-push
SHELL env FOO=1 git push                           ask   risky:git-push
SHELL nohup git push &                             ask   risky:git-push
SHELL xargs git push < list                        ask   risky:git-push
SHELL time cargo build                             allow shellInScope
SHELL python3 script.py                            allow shellInScope
SHELL cat <<EOF                                    ask   unparseable
SHELL echo "unterminated                           ask   unparseable
SHELL /usr/bin/git push                            ask   risky:git-push
SHELL cd /tmp && rm -rf x                          ask   outsideScope
```

Paths in shell arguments are resolved against the scope cwd, updated by a preceding `cd <dir>` segment in the same command; `~` and `~/…` expand to `home_dir`; `$VAR` in a destructive target → `ask unresolvedPath`.

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-desktop-services agent_policy` — Expected: FAIL.

- [ ] **Step 5: Implement `classify`.** Structure:

```rust
pub fn classify(facts: &ApprovalFacts, scope: &AgentScope) -> PolicyDecision {
    use ApprovalFacts as F;
    match facts {
        F::Read { paths } | F::Search { paths } => reads(paths, scope),
        F::Write { paths } | F::Delete { paths } => writes(paths, scope),
        F::WebSearch {} => allow("webSearch"),
        F::WebFetch { method, .. } if method.eq_ignore_ascii_case("GET") => allow("webFetch"),
        F::WebFetch { .. } => ask("webMethod"),
        F::Mcp { tool, .. } if is_read_verb(tool) => allow("mcpRead"),
        F::Mcp { .. } => ask("mcpAction"),
        F::Internal { .. } => allow("internal"),
        F::Unknown { .. } => ask("unknownTool"),
        F::Shell { command, cwd } => shell(command, cwd.as_deref(), scope),
    }
}
```

- `resolve(path, cwd, home_dir) -> PathBuf`: expand `~`, join relative to cwd, `lexically_normalized`; then canonicalize the longest existing ancestor and re-append the remainder (symlink escapes resolve to their real target).
- `in_scope(p)`: some root (also canonicalized the same way) is a prefix of `p`.
- `reads`: any credential path → ask `credentialPath`; else allow `read`/`search`.
- `writes`: credential → `credentialPath`; outside scope → `outsideScope`; else `inScopeWrite`.
- `is_read_verb(tool)`: the MCP tool name, lowercased, after stripping a `server__` prefix, starts with one of `get`, `list`, `read`, `search`, `find`, `fetch`, `query`, `view`, `describe`, `show`, `status`, followed by end, `_`, `-` or an uppercase boundary.
- `shell`: parse (error → `unparseable`); any `substitution` → `shellSubstitution`; for each segment: strip leading `NAME=value` words and wrappers `env`, `time`, `nice`, `nohup`, `command`, `builtin`, `xargs` (repeat); program = basename of first word; `cd <dir>` updates the working cwd; check the risky tables (data: `const RISKY_PROGRAMS: &[(&str, &str)]` and per-program subcommand predicates for `git`, `gh`, `glab`, `curl`, `wget`, `npm`/`yarn`/`pnpm`, `cargo`, `gem`, `twine`, `docker`, `kubectl`, `terraform`, `pulumi`, `vercel`, `netlify`, `fly`/`flyctl`, `heroku`, `firebase`, `brew`, `defaults`, `rsync`, `sh`/`bash`/`zsh`/`dash`/`fish`, `eval`/`exec`/`source`/`.`); destructive file commands `rm`, `rmdir`, `mv`, `cp`, `ln`, `chmod`, `chown`, `touch`, `mkdir`, `tee` → every non-flag target (for `cp`/`mv`/`ln` the last argument; `chmod`/`chown` all arguments after the mode/owner) must be in scope and non-credential; `rm -r`/`-rf`/`-fr` targeting a scope root, `home_dir`, `~` or `/` → `risky:rm-root`; redirect targets (except `/dev/null`) must be in scope and non-credential; `cat`/`less`/`head`/`tail`/`grep`/`rg` arguments that are credential paths → `credentialPath`. Everything else → `shellInScope`.
- Mark the ceiling at the top of the module: `// ponytail: risky-list shell model; scripts written in scope can still send data. Allowlist mode if decision logs show misses.`

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-workspace -p interface-desktop-services agent_ && PATH="$HOME/.cargo/bin:$PATH" cargo clippy -p interface-workspace -p interface-desktop-services --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit.** `git add -A && git commit -m "Agent approval policy and quote-aware shell tokenizer"`

---

### Task 4: Engine approval gate and facts

**Files:**
- Modify: `crates/engine-claude/src/launch.rs`, `crates/engine-claude/src/adapter.rs` (+ tests)
- Modify: `crates/engine-codex/src/adapter.rs` (+ `adapter_tests.rs`)
- Modify: `crates/engine-acp/src/adapter.rs` (+ `adapter_tests.rs`)

**Interfaces:**
- Consumes: `LaunchOptions.approval_gate`, `.additional_directories`, `ResumeOptions` same (Task 1); `ApprovalFacts` (Task 1).
- Produces: `impl EngineAdapter { fn approval_facts(..) }` for `ClaudeAdapter`, `CodexAdapter`, ACP adapter; gated launches per the spec table.

- [ ] **Step 1: Claude launch tests.** In `launch.rs`/`adapter.rs` tests:
  - `approval_gate_merges_pre_tool_use_ask_hook`: create/resume configuration with `approval_gate: Some(true)` → `launch.settings` JSON contains `hooks.PreToolUse[0].matcher == "*"` and a command hook whose `command` prints `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}`; the existing `initial_flag_settings` keys (effort/fast) are preserved; `permission_mode` is `Default` even if `permission_mode: Some("bypassPermissions")` was requested (gate wins).
  - `additional_directories_emit_add_dir`: argv contains `--add-dir <dir>` per directory, in order; relative paths are refused definitely-unsent.
  - `gate_rejected_for_chat_kind` (Chat is tool-free; `approval_gate` with `SessionKind::Chat` → definitely-unsent unsupported).

- [ ] **Step 2: Implement Claude.** Add `pub additional_directories: Vec<PathBuf>` to `LaunchOptions` in `launch.rs`, emitted as `--add-dir` pairs before `--permission-prompt-tool`, validated absolute with `validate_absolute_path`. In `create_configuration` and `resume_configuration`: when `approval_gate == Some(true)`, force `PermissionMode::Default` and merge:

```rust
const APPROVAL_GATE_HOOK: &str = r#"printf '%s' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}'"#;
fn merge_approval_gate(settings: Option<serde_json::Value>) -> serde_json::Value {
    let mut settings = settings.unwrap_or_else(|| serde_json::json!({}));
    settings["hooks"]["PreToolUse"] = serde_json::json!([{"matcher": "*", "hooks": [{"type": "command", "command": APPROVAL_GATE_HOOK}]}]);
    settings
}
```

Also make `host_permission_decision` treat a gated session as `forced_ask = true` so no mode can auto-allow (store `approval_gate` on `RunConfiguration` and pass it where `forced_ask` is computed).

- [ ] **Step 3: Claude facts.** Implement `approval_facts` on `ClaudeAdapter` and test each row:

| tool | input | facts |
| --- | --- | --- |
| `Bash` | `{command}` | `Shell{command, cwd: None}` |
| `Read` | `{file_path}` | `Read{[file_path]}` |
| `Grep`, `Glob` | `{path?}` | `Search{[path] or []}` |
| `Edit`, `MultiEdit`, `Write` | `{file_path}` | `Write{[file_path]}` |
| `NotebookEdit` | `{notebook_path}` | `Write{[notebook_path]}` |
| `WebFetch` | `{url}` | `WebFetch{url, "GET"}` |
| `WebSearch` | any | `WebSearch{}` |
| `mcp__{server}__{tool}` | any | `Mcp{server, tool}` |
| `TodoWrite`, `Task`, `Agent`, `BashOutput`, `KillShell`, `KillBash`, `ListMcpResourcesTool` | any | `Internal{tool}` |
| anything else, or missing/non-string required field | — | `Unknown{tool}` |

- [ ] **Step 4: Codex tests.**
  - `approval_gate_thread_start_params`: with `approval_gate: Some(true)` and `additional_directories: Some(["/w/p"])`, the `thread/start` request carries `approvalPolicy: "untrusted"`, `approvalsReviewer: "user"`, `sandbox: "workspace-write"`, and `config: {"sandbox_workspace_write": {"writable_roots": ["<cwd>", "/w/p"], "network_access": true}}`. Same for `thread/resume`.
  - `approval_gate_verification`: a response whose `approvalPolicy` ≠ `untrusted`, `approvalsReviewer` ≠ `user`, `sandbox.type` ≠ `workspaceWrite`, `sandbox.networkAccess` ≠ true, or `sandbox.writableRoots` missing any requested root → the launch fails outcome-unknown with a message naming the mismatch and the prompt is not sent (mirror the existing read-only verification).
  - `approval_gate_rejects_conflicting_modes`: `approval_gate` plus `sandbox_mode: ReadOnly`/`DangerFullAccess` or `permission_mode: "never"` → definitely-unsent.

- [ ] **Step 5: Implement Codex.** Extend `ThreadStartParams` (and the resume params struct) with `approvals_reviewer: Option<String>` and `config: Option<Value>` (both `skip_serializing_if = "Option::is_none"`), set them only when gated. Generalize `verify_requested_sandbox` into `verify_gate(requested_gate, requested_roots, response)` used for create and resume. Facts: `approval_display` tool names map as `command` → `Shell{command: input.command (string) or input.command (array joined with ' '), cwd: input.cwd}`; `fileChange` → `Write{paths}` from the file-change item's paths stashed when the adapter first sees `item/started` for that `itemId` (store `itemId → Vec<String>` in `SessionShared`, bounded to 256 entries; include the paths in the approval `input` under `"interfacePaths"` so `approval_facts` stays pure); missing paths → `Unknown{tool:"fileChange"}`; `permissions`, `question` → `Unknown`.

- [ ] **Step 6: ACP.** `approval_gate == Some(true)` or non-empty `additional_directories` on create/resume → definitely-unsent `Unsupported` with message `"This ACP agent cannot enforce Interface's approval gate yet, so it can't run as an agent."` (a later task can replace this once probe P3 finds a mechanism). Facts from ACP tool kinds: `read` → `Read{paths}`, `edit`/`move` → `Write{paths}`, `delete` → `Delete{paths}`, `search` → `Search{paths}`, `execute` → `Shell{command}`, `fetch` → `WebFetch{url, "GET"}`, `think` → `Internal`; paths from `rawInput` keys `path`, `file_path`, `abs_path`, `paths`; command from `command` (string or array); anything unresolvable → `Unknown`. Tests per kind plus the refusal.

- [ ] **Step 7: Verify and commit.**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p engine-claude -p engine-codex -p engine-acp && PATH="$HOME/.cargo/bin:$PATH" cargo clippy -p engine-claude -p engine-codex -p engine-acp --all-targets -- -D warnings`
Expected: PASS.

```bash
git add -A && git commit -m "Engine approval gate, extra directories and approval facts"
```

---

### Task 5: Renderer

**Files:**
- Modify: `electron/src/renderer/components/AgentMenu.tsx` (+ `AgentMenu.test.ts`)
- Modify: `electron/src/renderer/components/AgentWorkspacePanel.tsx`
- Create: `electron/src/renderer/components/AgentWorkspacePanel.test.ts`
- Modify: `electron/src/renderer/components/Conversation.tsx` (approval card "Checking…" state)
- Modify: `electron/src/renderer/App.tsx`, `electron/src/renderer/fixtures/agent-workspace-fixture.tsx`
- Modify: `docs/SETTLED_LOOKS.md`

**Interfaces:**
- Consumes: TS types from Task 1 (`AgentControlResponse`, `AgentStatus`, `AgentFiles`, `AgentDecision`, commands).
- Produces: UI only.

- [ ] **Step 1: State type.** In `App.tsx` the agent store state becomes `AgentControlResponse` (`emptyAgentControlResponse()`); `acceptAgentState` keeps its revision rule. Add `requestAgent(command: AgentRequest): Promise<AgentControlResponse | null>` that serializes through `agentQueue` like `mutateAgent`, accepts the returned state, and returns it (null + `agentError` on failure, followed by a `read` reload, never a replay).

- [ ] **Step 2: Roster status (tests first).** `AgentMenu` gets `statuses: Record<string, AgentStatus>` and renders a mark inside each `.agent-menu-row`: `<span className="agent-menu-status" data-status={status} aria-label={label}/>` where label is "Working", "Needs attention", "Unread" or nothing for idle. Working reuses the existing working indicator component used in the sidebar (find it with `rg -n "working" electron/src/renderer/components/Sidebar.tsx`); needsAttention uses the attention amber token; unread uses the tray's quiet dot class. The preview line uses the thread session's latest preview when `threads[agent].current` exists, else the existing latest-task preview. Tests: each status renders its mark and accessible label; idle renders none; roster order unchanged.

- [ ] **Step 3: Agent page tabs (tests first).** `AgentWorkspacePanel` views become `"thread" | "memory" | "routines" | "runs" | "computer"`, default `"thread"`. New props: `response: AgentControlResponse`, `onRequest: (command: AgentRequest) => Promise<AgentControlResponse | null>`, `onOpenThread: (sessionKey: string) => void`.
  - Thread: when `threads[agent.id]?.current` exists, render `children` (App mounts that session's conversation) plus a "New thread" button (`onCommand({kind:"newThread", agentId})`) and a past-threads menu (`onOpenThread(past.sessionKey)`). When it is `interrupted`, show "Interface restarted while this agent was working. Check the conversation before continuing." When no thread exists, render a composer: textarea `id={`agent-thread-first-${agent.id}`}` labelled "Message {name}" and a "Send" button → `onRequest({kind:"startAgentThread", agentId, text, requestId: newAgentId()})`; keep the draft on failure; disable while pending.
  - Memory: on mount call `readAgentFiles`; show the home path with "Show in Finder" (`revealAgentHome`); a textarea for memory text (`id` stable) and "Save memory" → `saveAgentMemory` with the loaded sha; on `agent-file-conflict` error reload files and say "MEMORY.md changed since you opened it. Reloaded." When `files.instructionsConflict`, show both texts and "Use Interface's version" → `overwriteAgentInstructionsFile`. When `!files.exists && files.home`, show "Recreate folder" → `recreateAgentHome`. `tooLarge` → read-only note with Show in Finder.
  - Computer: home folder line; granted folders list with "Add folder" (`onPickDirectory` then `saveAgentFolders` with the added canonical path) and "Remove"; the line "Asks before: sending, publishing or pushing, deleting outside its folders, money, system or credential access"; the provider note "Claude and Codex agents run with the approval gate. Codex agents run without MCP servers. Other providers are refused until they support it."; latest 50 decisions (`readAgentDecisions`, limit 50) as rows: time, tool, summary, decision label (Allowed / Asked you / You allowed / You denied / Refused on error).
  - Routines: `addRoutine` defaults `cwd` to `agent.home ?? defaultDirectory`.
  Tests (vitest + the repo's existing render helpers — copy the pattern from `AgentMenu.test.ts`): default tab is Thread; first-message composer calls `startAgentThread` with the typed text; Memory conflict message; Computer renders folders, decisions and the asks-before line.

- [ ] **Step 4: App wiring.**
  - Pass `statuses` and `response` into `AgentMenu`/`AgentWorkspacePanel`.
  - Thread tab mounting: when the selected agent has a current thread, set the agent task pane to that session (`selectSession(threadKey)` on tab entry). After `startAgentThread` succeeds, with `response.sessionKey`: `historyPaging.current.markStartedHere(key)`, `appendConversationEcho(key, launchPendingEcho(text, []))`, `markLive(key)`, `dispatch({type:"select", key})`.
  - Mark read: when the Thread tab is visible for an agent whose status is `unread` or whose thread is `interrupted`, call `requestAgent({kind:"markRead", agentId})` once per change of `lastActivityAtMs`.
  - Approvals: build `agentApprovalGate(key, approvalId): "owner" | "checking" | null` — null when `key` is not any agent's current thread key and not any run's `sessionKey`; `"owner"` when `pendingApprovals` has `{sessionKey:key, approvalId}`; otherwise `"checking"`. Pass it to `Conversation`. When a live event `approval.requested` arrives for an agent-bound key, trigger an immediate `read`.
  - Existing `answerApproval` stays the only path that responds.
  - The delete-agent confirmation adds "Its folder stays at {home}." when the agent has a home.

- [ ] **Step 5: Conversation checking state.** Add optional prop `approvalGate?: (approvalId: string) => "owner" | "checking" | null`. For an approval card whose gate is `"checking"`, render the request summary with the text "Checking with Interface's approval policy…" and no Allow/Deny controls or keyboard chords; `"owner"`/null keep today's card. Test in the existing Conversation test file.

- [ ] **Step 6: Fixture and settled look.** Update `agent-workspace-fixture.tsx` so the DOM fixture shows an agent with each status, a thread with a pending "Checking…" card, a Memory conflict, and a Computer tab with folders and decisions. In `docs/SETTLED_LOOKS.md` revise the "Agent sidebar has shaped characters and a vertical roster" entry: add "Each row also carries a status mark: the working indicator, the attention amber for needs attention, or the tray's quiet unread dot. The preview line shows the agent's latest thread message when it has a thread." Add `agent-menu-status` to its Markers and a `Revised: 2026-09-15, owner approved the teammate status mark.` line.

- [ ] **Step 7: Verify and commit.**

Run: `cd electron && npm ci && npm run typecheck && npx vitest run && npm run test:renderer`
Expected: PASS (if `test:renderer` cannot launch Electron in the sandbox, record the exact error and run it later from Task 7).

```bash
git add -A && git commit -m "Agent teammate thread, memory, computer tabs and roster status

Settled-look: Agent sidebar has shaped characters and a vertical roster — approved by the owner 2026-09-15"
```

---

### Task 6: Host integration

**Files:**
- Modify: `apps/interface-host/src/agents.rs` (+ tests), `apps/interface-host/src/main.rs`, `apps/interface-host/src/router.rs`, `apps/interface-host/src/dispatcher.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces:
  - `pub struct AgentHost` with `pub fn new(desktop: Arc<DesktopService>, runtime: Arc<dyn HostDispatcher>, facts: Arc<dyn ApprovalFactsSource>) -> Arc<Self>`
  - `pub trait ApprovalFactsSource: Send + Sync { fn approval_facts(&self, session_key: &str, tool: &str, input: &Value) -> ApprovalFacts; }` implemented for `SessionRuntime`
  - `impl HostDispatcher for AgentHost` handling `CHANNELS.agent_control`
  - `pub struct AgentRuntimeGuard { inner: Arc<dyn HostDispatcher>, agents: Arc<AgentHost> }` implementing `HostDispatcher` for the runtime bridge operations
  - `AgentEventTap` gains `host: Arc<AgentHost>`

- [ ] **Step 1: Tests first** (fake `HostDispatcher` recording operations, real `DesktopService` in a temp dir as the existing tests do):
  - `start_thread_creates_gated_session_in_home_and_binds_it`: `startAgentThread` → one `create_session` with `options.cwd == home`, `approvalGate: true`, `kind: "agent"`, `prompt == text`, no instructions prefix, `additionalDirectories` omitted when no folders; response `sessionKey` set; store thread bound; second call with the same `requestId` returns the same response without a second create.
  - `start_thread_unsent_error_is_replay_safe_and_unknown_is_not`.
  - `approval_allowed_by_policy_is_answered_once_and_logged`: event tap sees `approval.requested` for the thread key with a `Read` fact → exactly one `respond {verdict:"allow"}`; decision log has `allowed`.
  - `approval_risky_becomes_pending_and_generic_respond_is_owner_answer`: `Shell git push` → no respond; pending entry; guard `respond` for that id forwards once and logs `ownerAllowed`; a second respond → refused definitely-unsent.
  - `respond_for_non_pending_agent_approval_is_refused`.
  - `approval_before_bind_is_buffered_then_handled`.
  - `log_failure_asks_owner`.
  - `resume_of_agent_thread_injects_gate_and_directories`: guard `resume_session {sessionKey}` for a thread key forwards `approvalGate: true` and the agent's folders; a non-agent key is forwarded unchanged.
  - `routine_run_launch_is_gated`: run with `cwd == home` → no instructions prefix, `approvalGate: true`; run with another cwd → prefix kept, `additionalDirectories` = home + folders.
  - `remove_agent_closes_thread_session_first` (best-effort `close_session`, then store removal).
  - `session_closed_clears_pending_approvals`.

- [ ] **Step 2: Implement `AgentHost`.**
  - State: `pending_starts: Mutex<HashMap<String, Result<Value, DispatchError>>>` (request-id dedupe, bounded 128), `launching: AtomicUsize`, `buffered: Mutex<Vec<(String, EngineEvent)>>` (bounded 64; overflow → drop oldest and log).
  - `startAgentThread`: `desktop.prepare_agent_thread` (blocking section via `spawn_blocking`) → `launching += 1` → dispatch `create_session {engine, options:{cwd: home, prompt: text, name, kind:"agent", model?, effort?, approvalGate:true, additionalDirectories?}}` → on key `desktop.bind_agent_thread`, then `desktop.agent_control_response()` with `sessionKey` → `launching -= 1` → drain buffered events for bound keys through the approval driver. A bind failure after a successful create returns an outcome-unknown error (never unsent).
  - Other `agent_control` commands: `removeAgent` first closes the thread session (`close_session`, errors ignored), then everything is forwarded to the desktop dispatcher.
  - Approval driver `handle_approval(key, approval_id, tool, summary, input)`: `binding = desktop.agent_binding(key)`; none → buffer if `launching > 0` else ignore; facts → `agent_policy::classify(&facts, &AgentScope{roots: binding.roots, cwd: binding.cwd, home_dir: desktop.home_directory().to_path_buf()})` → build `AgentDecision` → `record_agent_decision` (error → treat as `AskOwner{reason:"policyError"}` with decision `FailedClosed`) → allow: dispatch `respond {sessionKey, approvalId, verdict:"allow"}` once, then record a second entry with the outcome (`Sent`, `Unsent`, `Unknown`); ask: `add_pending_agent_approval`.
  - `AgentEventTap::observe_session_event`: existing forwarding and store observe; then `ApprovalRequested` → spawn `host.handle_approval`; `SessionClosed` → `desktop.clear_pending_agent_approvals(key)`.
  - `AgentRuntimeGuard::dispatch`: `respond` for a bound key → `take_pending_agent_approval` → some: forward, record `OwnerAllowed`/`OwnerDenied` with outcome; none → `DispatchError::new(DEFINITELY_UNSENT_CODE, "Interface is still checking this request with the agent approval policy.", false, true)`. `resume_session` for a bound key → add `approvalGate: true` and `additionalDirectories` (home + folders, or run roots) to the payload before forwarding. Everything else forwards untouched.
  - `AgentRunner::launch`: build options with `approvalGate: true`; if `run.cwd == run.profile.home` skip the instructions prefix; else keep it and pass `additionalDirectories` = home (if any) + folders (omitting `run.cwd`).
- [ ] **Step 3: Wiring.** `main.rs`: build `AgentHost` after the runtime dispatcher (`ApprovalFactsSource` = `session_runtime.clone()`), set `desktop.set_agents_root(agents_root(&roots.home, std::env::var("INTERFACE_DEV_SUPERVISED").as_deref() == Ok("1")))`, give `AgentEventTap` the host, wrap the router's runtime handle in `AgentRuntimeGuard`, and route `CHANNELS.agent_control` to `AgentHost` ahead of the desktop dispatcher in `router.rs`. `dispatcher.rs`: the `Resume` request payload accepts optional `approvalGate` and `additionalDirectories` and merges them into `ResumeOptions`.

- [ ] **Step 4: Verify and commit.**

Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-host && PATH="$HOME/.cargo/bin:$PATH" cargo clippy -p interface-host --all-targets -- -D warnings`
Expected: PASS.

```bash
git add -A && git commit -m "Host agent threads, approval driver and runtime guard"
```

---

### Task 7: Live probes, docs, gate, closeout

**Files:**
- Create: `apps/interface-host/tests/agent_gate_live.rs`
- Modify: `docs/areas/agents.md`, `docs/AGENT_WORKSPACE_PLAN.md`, `docs/CODEMAP.md`, `docs/TODO.md`

- [ ] **Step 1: Live probe harness.** `#[ignore]` tests that build a real `SessionRuntime` with the Claude and Codex adapters, a temp agent home with generated files and `.claude/settings.local.json` allowing `Bash(git push:*)`, a scratch git repo with a local bare remote, then:
  - P1/P4 Claude: create a gated session, prompt "Read MEMORY.md and tell me your job, then run `git push origin main`." Assert an `approval.requested` for `Read` arrives, the adapter's facts classify as `Read`; deny the push approval; assert the reply names the job text.
  - P2 Codex: same with Codex; assert the `thread/start` verification passed (session ready) and the push raises `approval.requested` with `Shell` facts.
  - P5: close and resume the Claude session with the gate; send "What did I ask you before?"; assert it references the job.
  Run: `PATH="$HOME/.cargo/bin:$PATH" cargo test -p interface-host --test agent_gate_live -- --ignored --nocapture` and record results with the date in `docs/areas/agents.md`.
- [ ] **Step 2: Docs.** `docs/areas/agents.md`: v2 store, home folders, threads, approval policy and its limits, provider support matrix, probe results. `docs/AGENT_WORKSPACE_PLAN.md` Progress: Milestone 2 done (Rust store), agent teammates core implemented with link to the spec. `docs/CODEMAP.md`: mention `agent_policy.rs`, `agent_home.rs`, `agent_decisions.rs`. `docs/TODO.md`: update the Agent teammates entry.
- [ ] **Step 3: Gate.** `PATH="$HOME/.cargo/bin:$PATH" cargo xtask check` (both suites). Fix failures; never claim a pass without the output.
- [ ] **Step 4: Closeout.** `PATH="$HOME/.cargo/bin:$PATH" cargo xtask closeout --change "Agent teammates: home folders, threads, memory and risky-act approvals"` and confirm Dev verified the landing revision.
