# Agent permission modes — design

Date: 2026-09-25. Status: approved in conversation; awaiting written-spec review.

## Intent

Agent teammates always run behind Interface's approval gate today. The owner
wants two more per-agent modes: **Bypass**, with no prompts and no sandbox, and
**Auto**, which hands permission decisions to the provider's own auto mode.
Gated stays the default and keeps today's behavior exactly.

Owner decisions:

- The mode is set per agent and applies to that agent's threads and routines.
- Bypass is full bypass: no prompts and no sandbox.
- Routines follow the agent's mode, including Bypass.
- Storage is a sidecar file plus a typed launch field (approach A).
- In Auto and Bypass, anything the provider still escalates goes to the owner.
  Interface's policy does not answer it, except for the existing verified
  auto-allow of Interface's own task tools.

Out of scope: Grok and other ACP agents, which stay refused. A global default
mode. Per-task modes.

## Modes

| Mode | Claude | Codex |
| --- | --- | --- |
| Gated (default, unchanged) | Permission mode `default`, Interface's `PreToolUse` gate hook, `--permission-prompt-tool stdio` | Approval policy `untrusted`, reviewer `user`, workspace-write sandbox with network and `excludeSlashTmp`, writable roots = the agent's folders, the owner's MCP servers and acting features off |
| Auto | `--permission-mode auto`, no gate hook | Approval policy `on-request`, reviewer `auto_review`, otherwise the gated sandbox, MCP and feature settings |
| Bypass | `--permission-mode bypassPermissions` with `--allow-dangerously-skip-permissions`, no gate hook | Approval policy `never`, sandbox `danger-full-access`, the owner's MCP servers and features left on |

In every mode the agent keeps its folders (`--add-dir` for Claude, writable
roots or instructions for Codex) and Interface's task tools. Claude keeps
`--permission-prompt-tool stdio`, so anything Claude still asks about reaches
Interface.

Claude Auto requires a supported model (Opus 4.6+, Sonnet 4.6+ or Fable on the
Anthropic API). Otherwise Claude starts in Manual. When the `system/init`
message reports a permission mode other than `auto`, the adapter emits an info
notice: "Claude auto mode isn't available for this model or account, so Claude
asks before each action." The session is not failed.

Codex's `thread/start` and `thread/resume` responses are verified for every
mode, and a mismatch is outcome-unknown with the prompt unsent, as today:

- Auto: `on-request`, `auto_review`, plus the full gated sandbox checks.
- Bypass: `never` and sandbox type `dangerFullAccess`.

## Storage

`agent-permissions.v1.json` lives beside `agents.v2.json` in the private data
root. `AgentStore::open` opens it the way it opens `agent-work.v1.json` and
`agent-starts.v1.json`: owner-private directory, leased writer, atomic write.

```json
{"version":1,"modes":{"<agentId>":"auto"}}
```

- Only `auto` and `bypass` entries are stored; an absent agent is gated.
- At most 50 entries, matching the profile bound. The ids must name existing
  agents at save time.
- `removeAgent` drops the agent's entry in the same operation.
- A corrupt or foreign-version file refuses to open, like the other stores;
  it is never reset.
- An older build never reads the file, so a rollback silently returns every
  agent to Gated. `agents.v2.json` bytes are unchanged.

## Contracts (host protocol 31 → 32)

`interface-contracts/src/agents.rs`:

- `AgentPermissionMode { Gated, Auto, Bypass }`, serialized camelCase.
- `AgentCommand::SavePermissionMode { agent_id, mode }`. Saving `Gated`
  removes the entry.
- `AgentControlResponse.permission_modes: BTreeMap<String, AgentPermissionMode>`,
  omitted when empty.

`interface-contracts/src/engine.rs`:

- `AgentAccess { Auto, Bypass }`.
- `LaunchOptions.agent_access` and `ResumeOptions.agent_access`, both
  `Option<AgentAccess>` and omitted when `None`.
- Validation refuses `agent_access` together with `approval_gate: true`, and on
  a non-agent session kind.
- Adapters that do not implement it refuse definitely-unsent: engine-acp, and
  any adapter other than Claude and Codex.

TypeScript mirrors go in `electron/src/shared/agents.ts` and `engine.ts`.
There is a new shared fixture for a response carrying `permissionModes`, and
the fixtures for launch and resume options with `agentAccess`. Rust and
TypeScript both parse them. `docs/PROTOCOL_FACTS.md` and both handshake
constants move to 32.

## Host

- `thread_options`, the routine-run options and the resume insertion in
  `apps/interface-host/src/agents.rs` read the agent's mode at launch. Gated
  sends `approvalGate: true` as today; Auto and Bypass send
  `agentAccess: "auto" | "bypass"` and no `approvalGate`.
- A mode change applies from the next thread start, routine run or resume. A
  live session keeps the mode it launched with.
- `setPermissionMode` on an agent's session stays refused in every mode.
- Approval driver: in Auto and Bypass, a trusted host tool that passes the
  existing provenance and argument checks is still auto-allowed. Every other
  approval is asked of the owner without calling `agent_policy`. The decision
  log reason is `providerAuto` or `bypass`.
- Read-only consultations and scoped checks keep their existing read-only
  launch in every mode.

## Renderer

- `AgentProfileDialog` (`components/AgentMenu.tsx`): the collapsed Advanced
  section gains **Permissions** after effort. It is a select with one line of
  help per choice:
  - Ask: Interface's policy decides, then you.
  - Auto: Claude or Codex decides with its own safety checks.
  - Bypass: no checks; the agent can do anything your account can.
- Bypass shows an amber warning line. Auto shows a note when the chosen Claude
  model is outside the supported families.
- The Advanced summary appends ` · Auto` or ` · Bypass`.
- Saving the dialog sends `savePermissionMode` after `saveAgent` when the mode
  changed.
- The agent thread header shows an **Auto** or **Bypass** label; Gated shows
  none.
- `AGENT_PROVIDER_NOTE` describes gated behavior only; the dialog help covers
  the other modes.

**Settled look:** this extends "New agent asks for name, purpose and icon
first" by adding a field to Advanced and a suffix to its summary. It needs the
owner's approval and a `Settled-look:` trailer, and the entry in
`docs/SETTLED_LOOKS.md` is updated in the same commit. The field order Name →
What it does → Icon and the collapsed Advanced stay.

## Testing

- **Contracts:** serde round-trips and the refusal cases for
  `AgentPermissionMode` and `AgentAccess`, the protocol version assertion, and
  cross-language fixtures.
- **Store:** open/save/remove, bounds, unknown-agent refusal, corrupt-file
  refusal, and `removeAgent` dropping the entry.
- **engine-claude:** launch and resume arguments for Auto and Bypass (no gate
  hook, the right mode, the allow flag for bypass, folders and task tools
  kept); the init-mode notice; `agentAccess` + `approvalGate` refused.
- **engine-codex:** the `thread/start`/`thread/resume` params and response
  checks for Auto and Bypass against the fake app-server, including a mismatch
  that is outcome-unknown; MCP servers and features untouched in Bypass.
- **engine-acp:** `agentAccess` refused definitely-unsent.
- **Host:** launch options per mode for threads, routines and resumes; approval
  routing (Auto/Bypass ask the owner, trusted host tools still allowed, Gated
  unchanged); `setPermissionMode` refused.
- **Renderer:** the dialog field, the summary suffix, the warnings, the save
  sequence, the header label; the agent workspace DOM fixture picks Auto.
- **Gate:** the full `cargo xtask check`. Live probes, reported separately:
  one Claude Auto agent turn, one Codex `auto_review` turn, and one Bypass
  launch per provider.
