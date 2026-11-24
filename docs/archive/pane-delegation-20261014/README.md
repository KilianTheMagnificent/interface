# Pane delegation archive — October 14, 2026

The owner disabled pane delegation (background workers a terminal agent handed
work to) on 2026-09-09; on 2026-10-06 its service stopped being built by default
(landing `dd91183d`). On 2026-10-14 the owner chose "Archive all three" dormant
features in the opportunity review
(https://claude.ai/artifact/V6oyGmCTdQ7mMLUVUkvUTd, item F3). Pane delegation is
retired: its service, workflow tools, hooks, the default-off `pane-delegation`
cargo feature and its gated files are gone. This directory is historical
material, not a specification or a backlog; re-enabling delegation requires a
new owner instruction.

## Source preserved

The local branch **`archive/pane-delegation-20261014`** pins the complete
pre-retirement Dev tree at **`78dc388d4c2a7f301a10fac7b5d68afb7292b915`**.
Inspect or build the feature there (`cargo test -p interface-collaboration -p
interface-host --features interface-host/pane-delegation`), never by resetting
Dev to it. Key paths in that tree:

- `crates/interface-collaboration/src/{delegates.rs,delegates/,delegate_workflow.rs,delegate_workflow_tests.rs}`:
  `DelegateService`, the workflow store, validation, review and apply.
- `apps/interface-host/src/{pane_delegation_workflow.rs,pane_delegate_tests.rs,pane_delegate_repository_tests.rs,agy_delegate_tests.rs,worker_terminal.rs}`
  and the delegation halves of `pane_mcp.rs`, `terminal.rs`, `delegation_hook.rs`,
  `capture_hook.rs`, the Claude/Codex session hooks, `bootstrap.rs` and `main.rs`.
- `crates/engine-acp/src/agy_worker.rs` (Antigravity content worker) and
  `crates/interface-terminal/src/worker_view.rs` (native OpenCode worker view).
- Renderer: `TerminalDelegationSplit.tsx`, `TerminalDelegationPanel.tsx`,
  `DelegationTaskCard.tsx`, `terminal-delegation.css`, `fixtures/delegation-fixture.tsx`.

## Filed records

- [Design and operating record](delegation.md) (was `docs/areas/delegation.md`).
- Evaluations of 2026-09-07: [evaluation](DELEGATION_EVALUATION_2026-09-07.md),
  [model comparison](DELEGATION_MODEL_COMPARISON_2026-09-07.md),
  [School OS evaluation](DELEGATION_SCHOOL_OS_EVALUATION_2026-09-07.md),
  [worker quality](DELEGATION_WORKER_QUALITY_2026-09-07.md), their
  [evidence](evidence/delegation/) and [graders](scripts/).

## Retirement boundaries

- **Wire protocol kept.** `iface:readTerminalDelegation` stays in the channel
  table, the preload and the shared DTOs (`interface-contracts/src/delegation.rs`,
  `electron/src/shared/delegation.ts`, fixture `terminal-delegation-v12.json`).
  The host answers a current terminal generation with
  `{version:3, available:false, workflows:[], detail:null}` and refuses a stale
  one with `terminal-generation-mismatch`, exactly as default builds did since
  2026-10-06. No count or protocol number changes.
- **Pane MCP.** The `iface` server lists and serves only `pass_off`,
  `read_pane` and `send_to_pane` (it already advertised only these). The nine
  delegation tools and the `interface/delegation-checkpoint` method are gone;
  calls to them get the transport's unknown-tool answer.
- **Hooks.** Claude and Codex panes get the same hooks as before (receipts,
  activity and the `--interface-worker-notice` hook for report workers), byte
  for byte, so Codex's trust hashes are unchanged. A session launched before the
  retirement may still run `interface-host --interface-delegation-hook`; that
  mode is accepted and does nothing, as it did in default builds.
- **Kept, used elsewhere.** The Away CLI's worker runner stays:
  `DelegateRunner` (`run`, `apply`), `DelegateJob`, `DelegateRequest`,
  `CliDelegate` (now Claude and Codex workers only), `delegate_quality`,
  `scripts/delegation-quality-grade.py` (embedded by `iface away`) and its test,
  `interface-workspace`'s packet/apply/validation modules,
  `interface-registry::delegate_worktree` (report workers use it) and the
  `ValidationPlan` contract. `DelegateActivity`, the runner's `validate` and
  `verify_validation` methods and `interface-desktop-services::delegate_validation`
  were kept at first but had no reader or caller; they were removed on
  2026-11-15 (Away cycles validate in iface's `CycleExecutor::validate`). The report workers (`cheap_agents`, Workers MCP)
  are a separate feature and are unchanged. `engine-acp`'s OpenCode worker keeps
  its native-view session code for now (shared file with the report runner).
- **Your data stays on disk, unread.** Nothing reads or writes
  `panes/delegates/` under the app data folder
  (`~/Library/Application Support/Interface/` for the installed app,
  `~/Library/Application Support/Iface Dev/` for Dev): `delegates.v1.json`,
  `delegates.lock`, `result-*.json`, `task-*/` packets, `validation-*`,
  `validation-run-*.json` and `workflows/workflows.v1.json`. Nothing deletes
  them. `panes/report-workers/` belongs to the report workers.
- **Settled look.** "Delegated workers beside their principal" in
  `docs/SETTLED_LOOKS.md` covers the renderer split and the pane menu's "Show
  delegated work". Removing those needs the owner's approval of that entry's
  change; the renderer part lands separately.
