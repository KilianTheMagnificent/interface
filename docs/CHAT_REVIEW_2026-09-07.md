# Chat review — 2026-09-07

Reviewed against Dev `62ed4ba` (0.3.66), with source inspection, a real Claude
Haiku conversation in Interface Dev, and isolated Electron/Rust tests. Chat is
intentionally a plain conversation. Code and Agent own work with project files
and tools. This review preserves that product boundary.

## Findings and modifications

| Priority | Finding | Implemented change |
| --- | --- | --- |
| High | Restoring a Chat could call `setPermissionMode` with a model preset saved in Code. Claude rejects that operation for Chat, so the message would never reach `send`. | Exclude Chat and Voice from workspace permission restoration; retain the existing Code/Agent precedence. |
| Medium | The active Chat pane displayed a permission picker and project attachment controls, although the new-chat card hid them. File/folder/URL attachments serialize references, not their contents; a tool-free model cannot resolve them. | Keep the active pane consistent with the launch card. Hide those controls and detach file-drop/image-paste handlers. The pane's split action also names a chat rather than a project. |
| Medium | A new Chat initially displayed its provider UUID. The first-prompt title appeared only after provider history refreshed. | Rust seeds new Chat titles from the first 80 Unicode characters of the trimmed prompt. Explicit names win; empty prompts retain the session-id fallback. Other session kinds keep their established title behavior. |
| Medium | With no Chat-capable adapter, the model picker fell back to unsupported providers that the host would refuse. | Keep capability filtering strict, disable launch by button and Enter, and provide a Settings-directed explanation when the provider is unavailable. |

No dependencies, frozen DTOs, persisted schemas, provider tool permissions, or
settled visual decisions changed.

## Functional coverage

| Flow | Evidence |
| --- | --- |
| Launch and streaming response | Real Dev Chat returned `CHAT_OK` through Claude Haiku. Rust fake-CLI tests cover empty tools, strict MCP configuration, suppressed setting sources, and rejected permission changes. |
| Follow-up and reload | Real Dev completed a follow-up, restored both turns after renderer reload, and accepted another message. |
| Surface isolation | Live Chat → Code → Chat retained the Chat draft and transcript. Rust tests pin Chat to its private directory and strip worktree requests; stored-session tests cover kind-preserving resume/fork. |
| Settings and unavailable providers | Regression tests cover saved `auto`, `plan`, and bypass presets; real Electron tests cover both missing and unavailable Claude providers, including the Enter path. |
| Failed sends and busy queues | Real Electron tests verify that a definitely-unsent rejection restores the draft and a rejected queue leaves it intact without sending directly. |
| Stop and approvals | Electron tests exercise Stop ownership and active approval callbacks. The shared runtime suite covers ambiguous delivery quarantine and close/resume identity. Plain Chat deliberately has no tool approvals. |
| Transcript interaction and efficiency | Existing real Electron fixtures cover find, rapid session switching, and a 1,001-row transcript where ten streaming updates repaint one row. History paging and live/history separation were inspected and retained. |

Scoped verification passed: 129 renderer tests, 24 runtime integration tests,
TypeScript checks, and the expanded Electron DOM fixture. The canonical full
gate and Dev closeout are recorded in the task's delivery report.

## Assessment and limits

The implementation already has useful workspace guarantees: explicit surfaces,
provider-owned capability boundaries, a visible Stop path, per-session drafts,
bounded history, serialized sends, and no automatic replay of ambiguous effects.
The changes above correct places where the Chat UI disagreed with those rules.

The review criteria draw on simplicity, observable results, and stopping
conditions in [Anthropic's engineering guidance](https://www.anthropic.com/engineering/building-effective-agents),
and human control, transparency, and privacy in its
[trustworthy-agent framework](https://www.anthropic.com/research/trustworthy-agents).
Those principles inform the assessment; they do not certify this application.

Live testing used one Claude model, not every account/model combination. Chat
does not provide web research or file-reading tools. Unsent drafts survive pane
and surface switches but remain memory-only across a full renderer reload.
Provider model aliases can still appear as raw ids in an active pane. Shared
orchestration recovery and collaboration advisory revision handling are separate
from Chat's tool-free provider path and were not changed here. Packaged-app,
signing, updater, and production-runtime verification were not part of this review.
