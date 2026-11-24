# Application menu and the accelerator audit

Until 2026-10-14 the app shipped Electron's stock macOS menu
(`setApplicationMenu` appeared only in the packaged smoke, which removes it).
`electron/src/main/app-menu.ts` now builds the menu and `main.ts` installs it
at startup. Hands-free and packaged-smoke sessions still get no menu: their
hooks turn `Menu.setApplicationMenu` into a no-op.

| Menu | Items |
| --- | --- |
| Interface (app name) | About, Settings… ⌘, , Services, Hide ⌘H, Hide Others ⌥⌘H, Show All, Quit ⌘Q |
| Edit | Electron's stock `editMenu` role, unchanged: Undo, Redo, Cut, Copy, Paste, Paste and Match Style, Delete, Select All, Speech |
| View | Electron's stock `viewMenu` role, unchanged: Reload, Force Reload, Toggle Developer Tools, Actual Size, Zoom In, Zoom Out, Toggle Full Screen |
| Window | Minimize ⌘M, Zoom, Close Pane ⌘W, Close Window ⇧⌘W (the stock File menu's only item, moved here; it had ⌘W until 2026-11-15), Bring All to Front, the window list |
| Help | Reveal Logs (Rust shows `host.log` in Finder), Copy Diagnostics (Rust writes the filtered report and main copies it; a failure shows in the window, a copy that worked is silent) |

Settings… sends `{version: 1, command: "settings"}` on `iface:menuCommand`
(main-only, `shared/app-menu.ts`); the renderer opens Settings. It shows ⌘,
but does not register it (`registerAccelerator: false`): the renderer already
toggles Settings on ⌘, and keeps the chord while a dialog is up, so the
keyboard behaves exactly as before. The stock Help items (Electron's Learn
More, Documentation, Community Discussions, Search Issues) are gone.

## Audit (2026-10-14, dev 0136a22f)

Every chord the renderer handles, and what the menu does with it. Window-level
`keydown` listeners run before a menu key equivalent; a listener that calls
`preventDefault` keeps the chord from the menu. The new menu registers no
accelerator that the stock menu did not (pinned by `app-menu.test.ts`), so no
renderer chord changed hands.

| Chord | Renderer owner | What it does | Menu, before → after |
| --- | --- | --- | --- |
| ⌘, | `App.tsx` | Open or close Settings | none → Settings… (shown, not registered) |
| ⌘P | `App.tsx` | Go to file | none → none |
| ⌘⇧P | `App.tsx`, `workspace.ts` | Command palette | none → none |
| ⌘N | `App.tsx` | New thread, workspace or task | none → none (macOS stock File has no New) |
| ⌘F | `ConversationTranscript.tsx`, `TerminalView.tsx` | Find in a transcript or pane | none → none |
| ⌘K | `Sidebar.tsx` | Filter the sidebar | none → none |
| ⌘D | `IntegratedTerminalCenter.tsx` (capture), `App.tsx` | Split a terminal; in Threads, the diff panel | none → none |
| ⌘⇧D | `IntegratedTerminalCenter.tsx` | Split below | none → none |
| ⌘⇧[ ⌘⇧] | `terminal-keys.ts`, `thread-inbox.ts` | Previous / next terminal or thread | none → none |
| ⌘⇧S | `thread-inbox.ts` | Settle a thread | none → none |
| ⌘S | `ConversationComposer.tsx` (a Threads composer's field, only while it has focus) | Stash the draft (T3 section 9, 2026-11-15) | none → none (no stock menu registers ⌘S) |
| ⌘⌥P | `thread-inbox.ts` | Pin a thread | none → none |
| ⌘Z | `thread-inbox.ts` (sidebar focused, prevents default only when it acts) | Undo a settle, snooze or archive | Edit → Undo → unchanged |
| ⌘⇧M | `App.tsx` | Maximize the focused tile | none → none (Minimize is ⌘M without Shift) |
| ⌘⌥← ⌘⌥→ | `App.tsx` | Cycle tiles | none → none |
| ⌃Tab ⌃⇧Tab | `App.tsx` | Cycle tiles | none → none |
| ⌃` | `App.tsx` | Show terminals | none → none |
| ⌘↩ | `approval-keys.ts`, composers, launcher; `terminal-keys.ts` in a pane (2026-11-15) | Allow / send / launch; in a terminal pane, open a shell beside it | none → none |
| ⌘W | `App.tsx` (`isClosePaneChord`, `closeChordTarget`; 2026-11-15) | Close the selected pane (Settings when it is open; nothing behind a dialog) | Close Window → Close Pane (see below) |
| ⌘⌫ | `approval-keys.ts`; `terminal-keys.ts` in a pane | Deny; delete to line start | none → none |
| ⌘← ⌘→ | `terminal-keys.ts` | Line start / end in a pane | none → none |
| ⌘C | `TerminalView.tsx` (Super+C to a program that owns the pointer, else default) | Copy | Edit → Copy → unchanged |
| ⌘V | xterm's paste event, composers | Paste (text, or images into a prompt) | Edit → Paste → unchanged |

Stock chords the renderer never handled, kept as they were:

| Chord | Item | Note |
| --- | --- | --- |
| ⌘Q | Quit | Runs the before-quit guard (agents still working) |
| ⇧⌘W | Close Window | In Window; hides the window on macOS as ⌘W did until 2026-11-15 |
| ⌘R | Reload | View, unchanged |
| ⇧⌘R | Force Reload | View, unchanged |
| ⌥⌘I | Toggle Developer Tools | View, unchanged |
| ⌘0 ⌘+ ⌘- | Actual Size, Zoom In, Zoom Out | View, unchanged |
| ⌃⌘F | Toggle Full Screen | View, unchanged |
| ⌘M | Minimize | Window, unchanged |
| ⌘H ⌥⌘H | Hide, Hide Others | App menu, unchanged |
| ⌘A ⌘X ⇧⌘Z ⌥⇧⌘V | Select All, Cut, Redo, Paste and Match Style | Edit, unchanged |

## ⌘W closes the selected pane (2026-11-15)

The owner asked for ⌘W to close the selected pane instead of the window. The
page takes the chord in `App.tsx`'s window `keydown` and always calls
`preventDefault`, so the menu never acts on the same press; a held chord
closes one pane. What closes is decided by `closeChordTarget`
(`renderer/workspace.ts`): Settings when it is open, the selected terminal
pane's active tab in Code (exactly as its × does: a terminal ends, a Browser
page closes; nothing while the launcher covers the panes), the
focused tile in Threads and Agent, and nothing while a dialog is in front.

Window › Close Pane registers `Command+W` as well, for the one case the page
never sees the keys: a Browser page has the keyboard. Its click sends
`{version: 1, command: "close-pane"}` on `iface:menuCommand`, only while the
window is visible and focused, and the page runs the same function. Command,
never Control: ⌃W deletes a word in every shell. Close Window keeps its role
and takes ⇧⌘W, the macOS chord wherever ⌘W closes a tab. `app-menu.test.ts`
pins both accelerators.

⌘↩ in a terminal pane opens a shell beside it on the side with more room
(`terminalCenterKeyAction` returns `new-pane`; the capture listener in
`IntegratedTerminalCenter.tsx` splits as the pane's `+` › Terminal row does).
The launcher keeps ⌘↩ for Launch, and a field of the app's own (a page's
address, Find, a rename) keeps its Enter (`newPaneChordAllowedAt`).

## Chords a Browser page takes (2026-11-15)

Keys pressed while a Browser page (a `<webview>` guest) has the keyboard
never reach the renderer, and a menu key equivalent would act on the whole
app: the View menu's Reload reloaded the app, its zoom zoomed the app. Main
now takes these on the guest (`before-input-event` in `main/browser-pane.ts`,
which also keeps them from the menu) and the Browser pane takes the same ones
from its address bar (a `keydown` that calls `preventDefault`, so the menu
does not act either). The rule is one function, `browserKeyAction` in
`shared/browser-events.ts`:

| Chord | In a Browser tab |
| --- | --- |
| ⌘+ ⌘= ⌘− ⌘0 | Zoom the page in, out, back to 100% (Chrome's steps; per site) |
| ⌘R ⇧⌘R | Reload the page; without its cache |
| ⌘[ ⌘] | Back, forward |
| ⌘F | Find in the page |
| ⌘L | The address bar |
| Esc | Leave a page's fullscreen (only while it is fullscreen) |

Nothing else: ⌘W, ⌘T, ⌘Q, ⇧⌘[ ⇧⌘], ⌘D, ⌘P, ⇧⌘P and the rest stay the app's.
Elsewhere in the app the View menu's Reload and zoom are unchanged. While a
page has the keyboard the renderer's own chords (the command palette's ⇧⌘P,
for one) do not reach the app; click outside the page first.

## What is verified where

The menu and the clipboard are out of reach of the hands-free kit (no menu
bar, an in-memory clipboard). `src/main/app-menu.test.ts` pins the template,
the Settings… chord, the Help actions, the absence of new registered chords
and main's wiring; `crates/interface-review/src/diagnostics.rs` and
`apps/interface-host/src/diagnostics_tests.rs` pin the report and its filter.
