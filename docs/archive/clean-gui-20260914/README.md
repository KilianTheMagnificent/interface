# Clean GUI archive — September 14, 2026

The owner requested: "Take all of the clean GUI stuff out of dev and file it away".
Clean is retired from the active renderer. This directory is historical material,
not an active specification or a backlog authorizing more implementation.

## Source preserved

The local branch **`archive/clean-gui-20260914`** pins the complete pre-retirement
Dev tree at **`f2f100ff1227fe4282c7c0e8197f58ec33b7a0f6`**. The commit and earlier
Clean landings also remain reachable through Dev history. The archive branch is
local; publishing a separate remote archive branch was not authorized by the
automatic approval review, and was not retried.

This snapshot contains the original Clean overlays, composer, model/effort menu,
approvals, review sheet, native-input reader, styles and fixture lanes, including
work from before the Direction B overhaul. It also includes unrelated Dev features;
do not merge or reset Dev to this entire snapshot to restore a single feature.

Key historical source paths:

- `electron/src/renderer/components/TerminalClean*`, `CleanModelMenu.tsx`,
  `TerminalConversation.tsx`, and the Clean branches in `TerminalView.tsx`.
- `electron/src/renderer/terminal-clean-*`, `terminal-conversation.*`,
  `terminal-control-observation.*`, `use-terminal-controls.ts`, `clean-diff.*`.
- `electron/src/renderer/clean-panes.css`, `cli-pane-appearance.css`, and
  the Clean-specific branches of `components/Conversation.tsx` and related helpers.
- `electron/src/renderer/fixtures/{clean-panes,terminal-conversation,native-conversation}-fixture.tsx`.

To inspect the original source without changing Dev:

```sh
git show archive/clean-gui-20260914:electron/src/renderer/components/TerminalCleanComposer.tsx
git log archive/clean-gui-20260914 -- electron/src/renderer/components/TerminalCleanComposer.tsx
```

With a new owner instruction to revisit it, create an isolated inspection branch:

```sh
git worktree add -b experiment/clean-gui-revisit /private/tmp/interface-clean-gui-revisit archive/clean-gui-20260914
```

If the local archive branch is unavailable, use the full pinned commit above.
Any future implementation should start from then-current Dev and selectively port
reviewed changes, preserving intervening work and running current project gates.
Re-enabling Clean requires fresh owner authorization.

## Filed design and review records

- [Native input design](CLEAN_NATIVE_PANES.md)
- [Direction B design](2026-09-13-clean-pane-direction-b-design.md)
- [Direction B implementation plan](2026-09-13-clean-pane-direction-b.md)
- [Earlier review-fix plan](2026-09-13-clean-review-fixes.md)
- [Settled appearance history](settled-look-history.md)
- [September 14 repair and live review](historical-repair-review.md)

Paths and test results within those documents describe historical revisions.
They are not evidence that the archived GUI is ready to ship.

## Unresolved GUI findings when retired

1. **Short-pane spacing:** the composer rule for panes at most 300px tall restores
   a fixed left margin, misaligning it with the centered transcript. In the last
   rendered check, a 919 × 232 pane had a 760px transcript at x=419.5 and a 760px
   composer at x=352: a 67.5px offset.
2. **Effort keyboard navigation:** the model menu's Arrow Up/Down handler captures
   keys from effort radio items but traverses model rows. Arrow Down from an
   effort option can move into the model list, so Enter can select a model.

The owner chose retirement rather than another repair cycle. These findings are
preserved for any explicitly requested future evaluation.

## Retirement boundaries

- Native terminal panes, CLI controls, history, generation-fenced input, Fast and
  ordinary Chat/Agent conversations remain available.
- Stored `paneStyle: "clean"` and legacy `cleanInput` values remain readable for
  compatibility; they no longer activate the removed UI. No conversation or
  provider store is deleted or rewritten by this change.
- Shared native prompt guards moved to `terminal-prompt.ts`; phone input and the
  Control assistant retain their bounded screen/empty-prompt protections. Rust
  provider fixes, settings transactions and frozen IPC channels remain intact.
- The shared drop indicator, workspace menus, Away and Control assistant work are
  retained. Refined Chat/Agent working waves retain their separate approval.
- Removed Clean fixture lanes live in Git history. Native emulator geometry,
  legacy preference fallback, input routing, buffers and draft preservation remain
  exercised by the real Electron fixtures, alongside regular conversation tests.
- Release scope is Dev only. Main, updater staging, and installed production are
  separate actions. Inspect the actual landing and running revision with
  `cargo xtask status --fetch` and `cargo xtask landings`.

## Retirement verification

`cargo xtask check --changed` passed the Electron gate on September 14, 2026:
143 unit-test files / 2,692 tests, 42 release checks, all retained real Electron
fixture lanes, TypeScript and desktop/remote builds. The final log is
`/private/tmp/interface-clean-retirement-gate-final.log` (128.2 seconds).
Fixtures include legacy Clean preferences falling back to four native emulators,
retained drafts/scrollback/fullscreen buffers, ordinary conversations, Away,
Control assistant, native workers, menus and layout. Rust source and contracts
were unchanged, so the canonical selector ran the Electron suite only.
No live provider prompt, packaged smoke, production promotion or installation
was part of this retirement gate. Running Dev verification is a separate closeout
receipt, available through the status command above.
