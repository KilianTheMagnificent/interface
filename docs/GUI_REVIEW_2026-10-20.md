# GUI Review & Specifically Identified To-Do (October 20, 2026)

Full GUI review across **Threads**, **Code (Terminal workspaces)**, **Agent (Teammates)**, **Secondary Panels & Modals**, **Settings**, **iPhone Remote**, and the **Ratatui TUI**, cross-referenced against [SETTLED_LOOKS.md](SETTLED_LOOKS.md).

Each item has a stable ID (`GUI-01` .. `GUI-18`) so any slice or bucket can be picked up, verified, and closed out on `dev` independently.

---

## Bucket 1: Real Bugs (Broken Controls & Unstyled Surfaces)

- [x] **`GUI-01` — Fix Agent primary CTA buttons turning white-on-light on hover & staying bright when disabled**
  - **Files:** `electron/src/renderer/components/AgentMenu.tsx:230-237`, `electron/src/renderer/components/AgentWorkspacePanel.tsx:288,520,533,572,612`, `electron/src/renderer/components/ConfirmDialog.tsx:46-49`
  - **Problem:** `.agent-dialog-button:hover:not(:disabled)` (`0,3,0`) outranks `.agent-dialog-submit` (`0,1,0`) and has no `.agent-dialog-submit:hover` override, replacing `background: var(--accent)` with `var(--row-hover)` while keeping white `var(--accent-fg)` text in light themes. When `:disabled`, `.agent-dialog-submit` and `.confirm-dialog-confirm` only set `color: var(--fg-faint)` while keeping their full-strength `var(--accent)` background fill.
  - **Fix:** Add `.agent-dialog-submit:hover:not(:disabled) { background: var(--accent-hover); border-color: var(--accent-hover); }` and reset `:disabled` on `.agent-dialog-submit` and `.confirm-dialog-confirm` to `background: var(--surface-raised); border-color: var(--border);`.

- [x] **`GUI-02` — Fix `"From terminals"` inbox shelf hover actions, shared pagination & false drop highlight**
  - **Files:** `electron/src/renderer/components/ThreadInbox.tsx:329-331,767-798,943,1187-1214,1396-1438,1617-1623`
  - **Problem:**
    1. `row.section === "fromTerminals"` falls into the Active `else` branch in `ThreadRow`, rendering two 24px buttons (**Snooze** + **Settle** = 48px) inside a 24px slim slot (clipping the title). Clicking **Snooze** silently does nothing (`openSnooze` returns `null` because `snooze` is omitted from `threadMenuEntries` for external-settled rows), and clicking **Settle** moves the CLI session out of `From terminals` into `Settled`.
    2. `Settled` and `From terminals` both slice and increment the single `settledShown` state variable (`useState(SETTLED_PAGE_SIZE)`), so clicking `"Show N more"` in one expands both.
    3. `dragOver` is suppressed for `"snoozed"` (`line 1396`) but not `"fromTerminals"`, highlighting `From terminals` with `.is-drop-zone` on drag-over even though dropping there is a no-op.
  - **Fix:** Give `fromTerminals` its own single hover quick-action (`Continue as thread`) or keep its actions in `⋯`, give it an independent `fromTerminalsShown` pagination state, and exclude `fromTerminals` from `.is-drop-zone`.

- [x] **`GUI-03` — Style `PromptRecoveryPanel` and `PendingLaunchRecoveryPanel`**
  - **Files:** `electron/src/renderer/components/PromptRecoveryPanel.tsx:1-88`, `electron/src/renderer/components/Conversation.tsx:803`, `electron/src/renderer/App.tsx:6298`, `electron/src/renderer/theme.css`
  - **Problem:** Neither `.prompt-recovery` nor `.prompt-recovery-launch` is defined in any stylesheet. When an interrupted turn or unknown-outcome launch triggers recovery, it renders raw browser `<button>`, native `<select>`, unbounded `<pre>{receipt.input.text}</pre>`, and unstyled `<p role="alert">`.
  - **Fix:** Add `.prompt-recovery` and `.prompt-recovery-launch` card/banner styles in `theme.css`, apply `.btn` / `.btn-primary` to buttons, bound `<pre>` with scroll and monospace tokens, style `[role="alert"]` with `var(--danger-fg)`, and replace the raw `<select>` (`line 77`) with `AppSelect`.

- [x] **`GUI-04` — Lock thread composer and mutating turn actions while an Away loop is running**
  - **Files:** `electron/src/renderer/components/ConversationComposer.tsx:842-870`, `electron/src/renderer/components/ConversationTranscript.tsx:236-249,1108-1122`, `electron/src/renderer/components/ThreadTurnParts.tsx:76-101`, `electron/src/renderer/App.tsx`
  - **Problem:** While `thread.away?.state === "running" || "stopping"`, the composer still accepts and queues prompts (which the host then refuses with `threads-away-running`). Between work rounds (while the read-only reviewer session runs), `thread.busy` on the worker session is `false`, so `Edit from here` and `Undo turn` appear enabled and fail when clicked.
  - **Fix:** Pass `sendLocked` / `sendLockReason` into `Conversation` while `thread.away` is running or stopping, and disable mutating turn actions (`Edit from here`, `Undo turn`) for the duration of the loop.

- [x] **`GUI-05` — Disable or wire inert `"Codex accounts…"` item in the window `···` menu**
  - **Files:** `electron/src/renderer/components/TerminalWorkspace.tsx:503-505`
  - **Problem:** `<button type="button" className="terminal-menu-launch" role="menuitem">` has no `onClick` and no `disabled` attribute; clicking it does nothing and leaves the menu open.
  - **Fix:** Add `disabled` (and a quiet `"Soon"` hint) so it does not behave like a broken button *(note: `docs/SETTLED_LOOKS.md:451` names the placeholder; confirm with the owner before changing or removing the row)*.

---

## Bucket 2: Left-Behind Panels (Missed the October Polish)

- [x] **`GUI-06` — Fix `AgentWorkPanel`, `AgentCalendarPanel` & `AgentWorkspacePanel` forms and tab hierarchy**
  - **Files:** `electron/src/renderer/components/AgentWorkPanel.tsx:38-122`, `electron/src/renderer/components/AgentCalendarPanel.tsx:27-43`, `electron/src/renderer/components/AgentWorkspacePanel.tsx:172-228,498-637`, `electron/src/renderer/components/AgentMenu.tsx:203`
  - **Fix:**
    1. Change dead `className="agent-dialog-button is-primary"` on `"Save contract"` (`AgentWorkPanel.tsx:78`) to `className="agent-dialog-button agent-dialog-submit"`.
    2. Change `className="agent-dialog-input"` (`height: 30px`) on the 3 multi-line `<textarea>` fields (`AgentWorkPanel.tsx:47,59,60`) to `className="agent-dialog-textarea"`.
    3. Render `<AgentComputerView>` (`<h2>Computer</h2>`) at the top of the `Computer` tab instead of below `Results and limits` (`AgentWorkspacePanel.tsx:561-569`), and display `ProviderLogo` + `providerLabel(agent.engine)` in `<AgentReadiness>`.
    4. Replace the 4 raw native `<select>` elements (`AgentWorkspacePanel.tsx:605`, `AgentWorkPanel.tsx:66,118`, `AgentCalendarPanel.tsx:35`) with `FormSelect` / `AppSelect`.
    5. Wrap full-width stacked action buttons (`AgentWorkspacePanel.tsx:601-602`, `AgentWorkPanel.tsx:78-79`) in `.agent-workspace-actions`, close the `Past conversations` `<details>` menu on outside click / `Escape` (`AgentWorkspacePanel.tsx:498-506`), and change `.agent-dialog-shapes` (`AgentMenu.tsx:203`) from `repeat(9, 42px)` to `repeat(auto-fill, minmax(40px, 1fr))` so it does not overflow narrow windows.

- [x] **`GUI-07` — Restyle `TaskInspector` (`Task details` / `Context inventory…`) modal**
  - **Files:** `electron/src/renderer/components/TaskInspector.tsx:28-99`
  - **Fix:** Replace the raw `<button>Close</button>` stacked above `<h2>Task details</h2>` (`lines 47-48`) with a standard modal header row (`<h2>` on the left, `.codicon-close` button on the right), fix `--focus-ring` → `--focus` (`line 44`), add `:hover:not(:disabled)` on buttons, and format Context inventory with Settings-style card rows instead of raw bullet lists.

- [x] **`GUI-08` — Align `RemoteAccessPanel` and `CustomizationSection` with `SettingsPage`**
  - **Files:** `electron/src/renderer/components/RemoteAccessPanel.tsx:102-151`, `electron/src/renderer/components/SettingsPage.tsx:797-830,924,1572`
  - **Fix:** Restyle `RemoteAccessPanel` using `.settings-card` and `.settings-row`, group `"Approve iPhone"` and `"Decline"` on the right of `.remote-row` (`line 131`) with primary/danger button distinction, add a border and `var(--radius-container)` to the `#fff` QR box (`line 125`), place the Customization `Provider` `FormSelect` (`SettingsPage.tsx:797`) inside a card row, and track `customizationChecking` per MCP server instead of disabling all MCP rows at once.

- [x] **`GUI-09` — Fix `AgentCanvas` popover clipping, button hierarchy & port target size**
  - **Files:** `electron/src/renderer/components/AgentCanvas.tsx:1160-1250,1389-1422`, `electron/src/renderer/theme.css:3327-3338`
  - **Fix:** Add `max-height` and `overflow-y: auto` to `.canvas-policy` (so its 12 stacked fields do not clip off the top of the canvas), put `"Link"` / `"Save policy"` and `"Cancel"` in a horizontal action row with primary/secondary styling, increase `.canvas-port` hit area from `14×14px` to `24×24px`, and add a collapse toggle for the bottom `.canvas-coordination` panel (`max-height: 45%`).

- [x] **`GUI-10` — Modernize `OutputPreviewPanel` & `OutputPdfPreview` controls and vertical layout**
  - **Files:** `electron/src/renderer/components/OutputPreviewPanel.tsx:222-468`, `electron/src/renderer/components/OutputPreviewPanel.css:15-84`, `electron/src/renderer/components/OutputPdfPreview.tsx:81-91`
  - **Fix:** Replace the 3 native `<select>` elements with `AppSelect`, replace literal `↑`, `↻`, `›`, `×`, `‹` text with `Icon` / `.codicon`, overlay `.output-browser` or collapse `.output-feedback` while browsing so 5 vertical bands (~680px) do not crush `.output-content` to a 100px slit, restore primary/destructive button styling, and disable `"Save draft"` when `!dirtyDraft(current)`.

- [x] **`GUI-11` — Update stale surface name in `AdversarialReviewDialog`**
  - **Files:** `electron/src/renderer/components/AdversarialReviewDialog.tsx:171`
  - **Fix:** Update *"Opens two agent chat panes on the Agent surface"* to name **Threads** (where `openAdversarialReviewSessions` in `App.tsx:4754` opens them).

---

## Bucket 3: Threads & Code Paper Cuts

- [x] **`GUI-12` — Prevent thread title starvation in split panes & fix 360px `ThreadPaneMenu` width**
  - **Files:** `electron/src/renderer/components/AgentTile.tsx:105-135`, `electron/src/renderer/components/ThreadPaneHead.tsx:97-289`, `electron/src/renderer/components/ThreadPaneHead.css:5-62`
  - **Fix:** Give `.workspace-tile-name` a `min-width` floor and allow `.thread-head` and `.workspace-tile-project` to shrink (`flex: 0 1 auto`) in narrow/split tiles before the thread title collapses to `…`; shorten the inline missing-worktree text to `"Worktree removed"` with the full explanation in `title`; add `.is-compact` and roving `tabIndex` to `ThreadPaneMenu` (`ThreadPaneHead.tsx:270`) so it renders at `220px` instead of `360px`.

- [x] **`GUI-13` — Prevent `ThreadComposerMenu` (`+` menu) from clipping under tile headers in short splits**
  - **Files:** `electron/src/renderer/components/ThreadComposerMenu.tsx:191-274`, `electron/src/renderer/components/ThreadComposer.css:160-172`
  - **Fix:** Clamp `.thread-composer-menu` height to the containing tile's available space above the composer (or portal it like `AppSelect`) instead of `max-height: min(420px, 60vh)`.

- [x] **`GUI-14` — Fix New thread card (`Composer.tsx`) wrapped separator, `?` trail & Shift-click hint**
  - **Files:** `electron/src/renderer/components/Composer.tsx:901,1098-1132`, `electron/src/renderer/components/Composer.css:591,648-653,830`
  - **Fix:** Hide the leading `.composer-thread-sep` whenever `.composer-control-secondary` wraps onto a second line (not only at `@container (max-width: 620px)`), keep `?` attached to `.composer-thread-project` so it never wraps alone, and show a subtle hint in the Model `AppSelect` when Shift-click multi-model fan-out (`severalModels`) is active.

- [x] **`GUI-15` — Pre-select attributed thread files in `CommitSelectionForm` & reduce `ThreadDiffPanel` crowding**
  - **Files:** `electron/src/renderer/components/CommitSelectionForm.tsx:12-64`, `electron/src/renderer/components/ThreadShipBar.tsx:43-90`, `electron/src/renderer/components/ThreadDiffPanel.tsx:116-148`
  - **Fix:** Pre-select files with `file.attributed && !file.blocked` when opening `CommitSelectionForm` (or provide a one-click `"Select thread files"` button), avoid triple-nested vertical scrollbars inside `.tship`, and collapse `.tdiff-ghost-text` (`"Undo turn"`) to icon-only at `@container (max-width: 480px)`.

- [x] **`GUI-16` — Keep `Live panes` (`AgentOverview`) filter chips on one row at 360px & remove duplicate status dot**
  - **Files:** `electron/src/renderer/components/AgentOverview.tsx:128-140`, `electron/src/renderer/terminal.css:1396-1416`
  - **Fix:** Tighten `.agent-overview-filter` horizontal padding from `12px` to `9px` so all 5 filter pills (`All`, `Needs you`, `Working`, `Done`, `Idle`) fit on one line at the default `360px` column width on 1440px–1600px displays, and drop the duplicate status dot inside `.agent-overview-row-status` *(requires owner confirmation if touching a marker line in `docs/SETTLED_LOOKS.md:1318`)*.

- [x] **`GUI-17` — Add dividers to `ThreadInbox` context menu & polish second-opinion / retry states**
  - **Files:** `electron/src/renderer/components/ThreadInbox.tsx:1648-1753`, `electron/src/renderer/thread-inbox.ts:978-1052`, `electron/src/renderer/components/ThreadOpinion.css:19`, `electron/src/renderer/components/ThreadOpinionCards.tsx:48-115`, `electron/src/renderer/components/ThreadTurnParts.tsx:296-304`
  - **Fix:** Add `<div role="separator" />` dividers between Open/Bridges, Inbox triage, Thread management, and Archive/Delete in the 22-item thread menu; change `.thread-opinion-line.is-error` (`ThreadOpinion.css:19`) from `var(--fg)` to `var(--danger-fg)`; style delivered second-opinion advisories as peer-attributed cards instead of right-aligned `.conv-turn-user` bubbles; and show `"Retry"` for failed turns vs `"Continue"` for interrupted turns (`ThreadTurnParts.tsx:296-304`).

---

## Bucket 4: iPhone Remote & Standalone TUI

- [x] **`GUI-18` — Fix iOS error tint, fenced code blocks, mobile web debug disclosure & TUI wrapped footer clipping**
  - **Files:** `apps/interface-ios/InterfaceApp.swift:169`, `apps/interface-ios/PhoneThreadViews.swift:23-46,273-276`, `electron/src/remote/RemoteApp.tsx:342-344`, `electron/src/remote/remote.css:6-21`, `apps/iface/src/tui/view.rs:58-105,548-566,835`
  - **Fix:** Use `PhoneStyle.attention` (amber) instead of `PhoneStyle.accent` (mint) on `connectionScreen` error banners; render fenced code blocks (` ``` `) in `ThreadTurnView` on iOS; visually distinguish non-tappable sessionless queued tasks in `PhoneInboxListView`; remove or style the raw `"Task results and input receipt"` disclosure inside `.remote-composer` (`RemoteApp.tsx:342`) and give `.remote-connection.is-online i` a live accent color; and dynamically grow/scroll `footer_height` and `render_approval` in the Ratatui TUI (`view.rs`) so wrapped prompts and long approvals never push the bottom keybind bar out of the frame.
