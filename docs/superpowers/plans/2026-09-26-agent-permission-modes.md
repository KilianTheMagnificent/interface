# Agent permission modes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each agent teammate a permission mode: Gated (today), Auto (the
provider's own auto mode) or Bypass (no prompts, no sandbox).

**Architecture:**
- The mode lives in a Rust-owned sidecar store, `agent-permissions.v1.json`,
  inside `AgentStore`.
- The host reads it at every agent launch and resume and sends either
  `approvalGate: true` or `agentAccess: "auto"|"bypass"`.
- The Claude and Codex adapters map `agentAccess` to their provider flags. ACP
  refuses it.
- The renderer edits the mode in the agent dialog's Advanced section and shows
  it on the thread bar and in the Computer view.

**Tech Stack:** Rust (serde, interface-storage), TypeScript/React, Vitest.

**Spec:** `docs/superpowers/specs/2026-09-25-agent-permission-modes-design.md`

Execution note: the owner said "go for it". This plan is executed natively in
the authoring session, so tasks name interfaces and tests rather than every
line of code.

## Global Constraints

- Gated behavior, stored bytes and every existing feature stay unchanged. The
  owner said: "make sure we do not lose any functionality or features".
- Host protocol goes 31 → 32 (`docs/PROTOCOL_FACTS.md`, both handshake
  constants).
- `agents.v2.json` bytes are unchanged; only `agent-permissions.v1.json` is new.
- Claude Auto: `--permission-mode auto`. Claude Bypass: `bypassPermissions`
  (the adapter already adds `--allow-dangerously-skip-permissions`).
- Codex Auto: `on-request` + `auto_review` + the gated sandbox, MCP and
  feature settings.
- Codex Bypass: `never` + `danger-full-access`, with the owner's MCP servers
  and features untouched.
- Escalations in Auto/Bypass go to the owner. Only verified trusted host tools
  are auto-allowed.
- Settled look "New agent asks for name, purpose and icon first" changes with
  the owner's approval (2026-09-26). It needs a `Settled-look:` trailer and an
  entry update.

## Review Focus

1. **Rollback:** an older build ignores the sidecar, so agents run Gated. Pin
   that saving a mode leaves `agents.v2.json` byte-identical (Task 2).
2. **Stale entries:** a sidecar entry for an agent removed by an older build
   must not block opening. Unknown ids are dropped from responses and on the
   next save (Task 2).
3. **Mode changed during a live thread:** the live session keeps its mode,
   and the next resume uses the new one. The resume requirement reads the
   current mode (Task 7).
4. **Claude Auto on an unsupported model:** `system/init` reports another
   mode and the adapter emits the notice, but no failure (Task 4).
5. **Grok agent with Auto/Bypass:** still refused definitely-unsent before
   launch (Task 6).

---

### Task 1: Contracts (Rust + TypeScript, protocol 32)

**Files:**
- Modify: `crates/interface-contracts/src/agents.rs` (mode enum, command, response field)
- Modify: `crates/interface-contracts/src/engine.rs` (`AgentAccess`, launch/resume fields, `validate_agent_access`)
- Create: `electron/src/fixtures/agent-permission-modes-v32.json`
- Modify: `electron/src/shared/agents.ts`, `electron/src/shared/agents.test.ts`
- Modify: `apps/interface-host/src/protocol.rs`, `electron/src/main/rust-host-client.ts`, `docs/PROTOCOL_FACTS.md`

**Interfaces produced:**
- `AgentPermissionMode { Gated, Auto, Bypass }`, serialized as
  `"gated"|"auto"|"bypass"`, with `Default = Gated` and
  `fn access(self) -> Option<AgentAccess>`.
- `AgentCommand::SavePermissionMode { agent_id: String, mode: AgentPermissionMode }`.
- `AgentControlResponse.permission_modes: BTreeMap<String, AgentPermissionMode>`,
  with `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`.
- `AgentAccess { Auto, Bypass }`, serialized as `"auto"|"bypass"`.
- `LaunchOptions.agent_access` and `ResumeOptions.agent_access`: `Option<AgentAccess>`.
- `pub fn validate_agent_access(access: Option<AgentAccess>, approval_gate: Option<bool>, kind: SessionKind) -> Result<(), &'static str>`.
- TS: `AgentPermissionMode`, the `savePermissionMode` request, and
  `permissionModes?` on `AgentControlResponse`.

- [ ] Test: the fixture round-trips the commands (all three modes) and a
  response with `permissionModes`. Launch and resume options with
  `agentAccess` round-trip. `validate_agent_access` refuses access + gate and
  a non-agent kind.
- [ ] Implement, then bump the protocol to 32 in the handshake constants,
  `PROTOCOL_FACTS.md` and the fixture's `protocolVersion`.
- [ ] Run `cargo test -p interface-contracts` and
  `npx vitest run src/shared/agents.test.ts`.
- [ ] Commit.

### Task 2: Sidecar store

**Files:**
- Create: `crates/interface-settings/src/agent_permissions.rs`
- Modify: `crates/interface-settings/src/agents.rs`, `lib.rs`

**Interfaces produced:**
- `pub struct AgentPermissions`, with `open(path)`, `mode(&self, agent_id) -> AgentPermissionMode`,
  `save(&mut self, agent_id, mode, known: &[String]) -> Result<(), SettingsError>`,
  `forget(&mut self, agent_id)` and `modes(&self, known) -> BTreeMap<..>`.
- `AgentStore.permissions`: opened beside the other sidecars.
- `AgentStore::response` fills `permission_modes`.
- `AgentStore` applies `SavePermissionMode`, and `RemoveAgent` forgets the entry.
- `AgentBinding.access: Option<AgentAccess>`.

- [ ] Test:
  - Save auto/bypass/gated with a round trip across reopen.
  - Refuse an unknown agent.
  - Refuse a corrupt file and a version other than 1.
  - Keep stale ids on open, but leave them out of responses and drop them on
    the next save.
  - `removeAgent` drops the entry.
  - `agents.v2.json` stays byte-identical across a mode save.
  - The binding carries the mode.
- [ ] Implement using the `agent_starts.rs` pattern: leased writer, atomic
  write, `uncertain` flag.
- [ ] Run `cargo test -p interface-settings agent`.
- [ ] Commit.

### Task 3: Desktop services

**Files:**
- Modify: `crates/interface-desktop-services/src/agent_control.rs`

**Interfaces produced:**
- `SavePermissionMode` is routed to the store, with an unknown agent refused.
- `AgentThreadLaunch.access: Option<AgentAccess>`.
- `pub fn agent_access(&self, agent_id: &str) -> Option<AgentAccess>`.

- [ ] Test through the desktop service: the save round-trips into the response,
  and the thread launch carries the access.
- [ ] Implement.
- [ ] Run `cargo test -p interface-desktop-services agent`.
- [ ] Commit.

### Task 4: engine-claude

**Files:**
- Modify: `crates/engine-claude/src/adapter.rs`, `adapter_tests.rs`, `mapper.rs` (if needed for init mode)

Behavior:
- Create and resume apply `agent_access`. Auto uses `PermissionMode::Auto`,
  Bypass uses `BypassPermissions`, and neither adds the gate hook.
- Agent task tools attach for gated *and* access sessions.
- `RunConfiguration.agent_access` is kept. `SetPermissionMode` to a different
  mode on an access session is refused with the message "This agent's
  permission mode comes from its settings".
- Resume sets the launch mode from `agent_access` instead of `Default`.
- `system/init` reporting `permissionMode` other than `auto` on an Auto
  session emits the info notice from the spec.

- [ ] Test:
  - Arguments for create and resume in Auto and Bypass: the mode, no
    `PreToolUse` gate hook, `--add-dir`, the agent-tools MCP.
  - Access + gate refused.
  - The notice on a mismatched init.
- [ ] Implement.
- [ ] Run `cargo test -p engine-claude`.
- [ ] Commit.

### Task 5: engine-codex

**Files:**
- Modify: `crates/engine-codex/src/adapter.rs` and its tests

Behavior:
- The gate becomes `GateRequest { mode: CodexAgentMode, cwd, directories }`,
  with `CodexAgentMode { Gated, Auto, Bypass }`.
- Gated and Auto keep `gate_config`, the feature-disable spawn args and the
  full sandbox verification. Auto uses `on-request` + `auto_review`.
- Bypass uses config `{mcp_servers: {<agent tools>}}` only, with `never`,
  `DangerFullAccess` and no feature args. Its verification checks
  `approvalPolicy == "never"` and sandbox type `dangerFullAccess`.
- Agent tools attach for all agent modes.
- Collaboration stays off for agent sessions.
- Permission-mode changes are refused for access sessions.

- [ ] Test against the fake app-server: the params and verification for Auto
  and Bypass, a mismatch that is outcome-unknown, and Bypass not listing or
  disabling MCP servers.
- [ ] Implement.
- [ ] Run `cargo test -p engine-codex`.
- [ ] Commit.

### Task 6: engine-acp

- [ ] Test and implement: `agent_access` on create or resume is refused
  definitely-unsent.
- [ ] Commit.

### Task 7: Host

**Files:**
- Modify: `apps/interface-host/src/agents.rs`, `dispatcher.rs` and the tests

Behavior:
- `thread_options(launch, text)` and the routine-run options send
  `agentAccess` for Auto/Bypass and `approvalGate: true` for Gated.
- `ResumeRequirement` gains `Access(AgentAccess, Vec<String>)`. The dispatcher
  sets `agent_access` and clears `approval_gate` for it.
- `AgentHost::resume_requirement` reads `binding.access`.
- The explicit-resume insertion adds `approvalGate` only when Gated.
- Approval driver: when `binding.access` is `Some`, a trusted host tool is
  allowed and everything else is asked of the owner with reason
  `providerAuto` or `bypass`. `agent_policy` is not called.

- [ ] Test: launch payloads per mode, the resume requirement per mode, and the
  approval routing for Auto/Bypass alongside unchanged Gated.
- [ ] Implement.
- [ ] Run `cargo test -p interface-host agents` and the dispatcher tests.
- [ ] Commit.

### Task 8: Renderer

**Files:**
- Modify: `electron/src/renderer/components/AgentMenu.tsx`, `App.tsx`,
  `components/AgentWorkspacePanel.tsx` and their tests, `docs/SETTLED_LOOKS.md`

Behavior:
- `AgentProfileDialog` takes `initialPermission` and calls
  `onSubmit(profile, permission)`.
- A **Permissions** `FormSelect` sits after Effort. Bypass shows an amber hint;
  Auto on a Claude model outside Opus/Sonnet 4.6+ or Fable shows a note.
- The summary appends ` · Auto` or ` · Bypass`.
- `App.submitAgentProfile` saves the profile, then sends `savePermissionMode`
  when the mode changed.
- The thread bar shows a badge `Auto` or `Bypass`.
- The Computer "Approval gate" row becomes "Permissions" and shows text per
  mode, keeping today's text for Gated.

- [ ] Tests:
  - The dialog field order is unchanged except for the new field after Effort.
  - The summary suffix and the warnings.
  - The submit passes the mode.
  - The thread bar badge.
  - The Computer row text per mode.
- [ ] Implement. Run the renderer CSS guard tests and fix any amber allowlist
  entry.
- [ ] Update `docs/SETTLED_LOOKS.md` and commit with the `Settled-look:` trailer.

### Task 9: Docs, gate, live probes, closeout

- [ ] `docs/areas/agents.md`: the mode table, storage, approval routing.
- [ ] `docs/agent-map.json`/CODEMAP if the navigation check requires them for
  the new file.
- [ ] Run the full `cargo xtask check`.
- [ ] Live probes: a Claude Auto agent turn and a Codex `auto_review` turn,
  plus a Bypass launch per provider, using the real CLIs in a scratch folder.
- [ ] `cargo xtask closeout --kind feature ...` and verify Dev.
