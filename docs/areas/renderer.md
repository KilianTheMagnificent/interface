# electron/src/renderer — the React shell

Module map, entry points and mechanism for the React shell. Every decision
testable without a DOM was pushed down into a plain `.ts` module — the renderer
test suite combines pure-function/static markup tests with an isolated Electron
DOM lane (`npm run test:renderer`). That lane exercises streaming, find/focus,
session switching, real preload/MessagePort/xterm credits and theme screenshots.
Interaction and visual coverage run as separate Electron processes, a few at a
time, each with its own profile and a deadline sized to its own work
(`electron/scripts/renderer-lanes.cjs`, which also lists the lane switches); the
final report joins their results. Fixtures share `fixtures/fixture-support.ts`
(`assert`, `frame(s)`, `wait`, `until`). Rules
for editing here: `electron/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

**Shell and state**

- `components/AwayControl.tsx` — one-click project Away start, saved options,
  progress, exact Stop and session/history links. Rust owns scheduling and state;
  [Project Away](../PROJECT_AWAY.md) describes the execution and recovery contract.

- `App.tsx` — the shell: sole `window.iface` owner, all cross-pane state,
  keyboard chords, toolbar/statusbar/surfaces, and the launch/send/approval/
  checkpoint orchestration.
  Agent-store reads (`use-agent-store.ts`) do not overlap, pause while the
  document is hidden, and retain the current immutable DTO when the returned
  state is unchanged.
  `refreshAll("focus")` (window focus, becoming visible) re-reads sessions
  without `probeEngines` or `engineOps`, and relists collaborations quietly;
  `"full"` (mount, session-library changes) probes;
  `"explicit"` (the Refresh actions) probes and lists once more after the probe
  answers, because the host's probe restarts the provider listings it keeps.
  A refresh's failures are kept one per failure (`RefreshFailure`), each its
  own corner notice; `refreshError` is still their one joined line, which
  Settings › Models & providers quotes and the first-listing latch reads.
- App's own state blocks that moved out verbatim, each called once from App
  with the api (or the App verbs it needs) as a parameter, never
  `window.iface`: `use-thread-bridges.ts` (open in terminal, continue as
  thread, hand to a teammate, second opinion, Continue with, the opinion
  cards, `threadBridges`); `use-output-preview-state.ts` (the thread output
  preview's file and openers, Close, the effects that hide it; whether it is
  visible, `threadPreviewOpen`, reads App's layout and stays in App);
  `use-quick-open.ts` (the file picker's store, its one reset and its opener;
  a pick, ⌘P and the folder-change close read later session state and stay
  in App); `use-turn-restore.ts` (the per-turn Undo's capture, the settled-turn
  re-key effect and the restore; the restore points and notice stay App state);
  `use-agent-store.ts` (the Rust agent store, its serialized mutation and
  request queue and the 5 s visible poll); `use-subagent-detail.ts` (the
  subagent detail column's selection, its open, close, re-target and reveal
  routes, the effects that close it, and the agent, parent and prompt it
  draws; `workspaceTile` is exported from there for App's own focus code);
  `use-return-recap.ts` (the return recap's attention tracking, lines, timers
  and dismissal; a deleted session's teardown in App clears its stores);
  `use-away-controller.ts` (Away's panel state, the project it starts in, the
  provider check and the loop thread to open; Away's reported session lists,
  the quit guard's count and the thread cards stay in App).
- `latest-read.ts` coalesces read-only diff and focus refresh requests. It never
  coalesces provider sends or mutations. `useThreadIndex` only reconciles its
  retained row identities when its immutable Threads view changes.
- Threads (the Chat surface's thread view; [design](../superpowers/specs/2026-10-06-threads-design.md)).
  `use-threads.ts` holds Rust's `ThreadsView`; nothing here derives a section,
  unread state or permission.
  - New thread card: `Composer`'s `thread` prop draws one centred column
    (`.composer-thread-card`, every rule scoped to it): the headline with the
    project picker in it, the prompt card with a chip footer, a workspace
    drawer under it, and the "or chat without a project" / key-hint line.
    Since the Threads look (owner, 2026-10-23, the Claude app's Code mode)
    the card draws no project hue but the 8px square before the picked
    project's name (a ring for No project): a serif greeting beside the still
    Pair (`ThinkingMark`), the project picker, the workspace as Local
    checkout / New worktree segments with the branch, and the prompt card with
    Start as the theme's accent square.
    "No project" is the chat card exactly (kind `chat`); a project is kind
    `code` with every engine, the access mode and the drawer (Local checkout /
    New worktree with a base branch / an existing worktree as a third
    segment). `thread-card.ts` holds the pure model (the headline and drawer
    wording, projects, per-project workspace memory under
    `interface.threadCard.v1`, `launchThreadCard`: prepare → App's `launch` →
    bind, discard on a failed launch, one worktree per Shift-chosen model);
    `use-thread-card.ts` is App's state for it. The card holds the launch lock
    for the first model's sequence only: the other Shift-chosen models start
    after it in the background (`launch`'s `extra` option: no lock, nothing on
    the card touched), and those that fail are named on the action error
    line. ⌘Enter starts in the background (`launch`'s `background` option)
    and leaves the card on the same project. Where a
    fresh card opens is `threadCardOpening`: a seed (Edit from turn 1, a
    Threads pane's split +, a project's own +) first, else the inbox scope's
    project (No project included), else the last project launched from.
  - Thread pane: `AgentTile` is the Threads grid's tile. Its head (owner,
    2026-11-15) is two lines: the short title (`threadDisplayTitle` in
    `thread-title.ts`: a title still its first prompt — `titleSource`
    "prompt" — cut to its first line near 60 characters on a word; a user or
    generated title as it is; the whole title in the tooltip; the inbox row
    uses the same), then one plain line: the project's hue square and name
    (`projectLabel`, "No project" with a hollow ring for a plain chat), then
    `ThreadPaneHead.tsx` in the `headDetail` slot ("on <branch>" — the
    recorded branch, else `checkoutBranch`, the ship read's — and where edits
    land from `threadHeadWhere`, "worktree removed — recreated on next
    message", a `PR #N · state` link — a `target="_blank"` anchor the main
    window's open handler validates and hands to the OS — and Forked/Edited
    from, each after a quiet dot), then the Changes button
    (`ThreadDiffToggle` with `threadChangeTotals`, unknown never zero; a
    switched thread totals every segment, protocol 69) in
    `headActions` and `ThreadPaneMenu`'s ⋯ (split, maximize ⌘⇧M, close). The
    provider mark and the access mode are the composer's chips, not the head's.
    The head names the thread's provider account only when that provider has
    more than one (protocol 60, `provider-accounts-view.ts`
    `threadAccountLabel`; "account removed" for one that is gone); the New
    thread card's account control, Settings → Models & providers' per-provider
    Accounts menu (`components/ProviderAccountsSection.tsx`, drawn in
    `components/SettingsProviders.tsx`; Sign in only on an account that is not
    signed in) and the usage rows per account
    read the same view, which `use-provider-accounts.ts` keeps in `App.tsx`.
    Composer references (T3 §9, protocol 63): selecting text in a reply
    offers Quote (`ConversationRows.tsx` `TextBlock`), which the composer
    writes as `> …` with the caret after it (`composer-quote.ts`); a
    terminal pane's ⋯ "Send selection to thread…" writes a fenced block into
    the chosen thread's draft (`IntegratedTerminalCenter.tsx`,
    `components/ComposerReferenceDialogs.tsx`); the context picker's Thread
    kind lists the project's other threads (`thread-reference.ts`) and its
    chip's block comes from Rust's `validatePromptContext`, placed as quoted
    data by `serializePromptWithContext`. ⌘S and the + menu's "Stash draft"
    park a Threads composer's draft and chips in Rust (`use-draft-stash.ts`,
    `draft-stash-view.ts`); "Stashed (n)" lists them
    (`components/StashedDrafts.tsx`).
    Quick wins (T3 §12, protocol 67): the ship bar's commit form and Push &
    open PR's new form carry Write with AI (`components/WriteWithAi.tsx`,
    `use-thread-ship.ts` `suggestText`, off with the reason from
    `thread-ship.ts` `writeWithAiBlocked`). A thread whose agent asks for a
    secret docks `components/ThreadSecretCard.tsx` with the approval dock
    (Conversation's `dockCard`); the value is only an argument to Give
    (`use-thread-secrets.ts`); Settings → Threads lists held secrets
    (`components/ThreadSecretsSettings.tsx`). `/btw <question>` in a thread's
    composer (or ⋯ → Side question…) asks a side question; App draws
    `components/SideQuestionPanel.tsx` in the detail column from the side
    thread's events (`side-question.ts`), Bring back quotes the answer into the
    draft.
    Folder trust (protocol 72, `folder-trust.ts`): before a new Claude
    thread's first prompt, `use-thread-card.ts` asks the host `folderTrust`
    for the card's project and, when nobody has answered, App's
    `ConfirmDialog` asks "Trust this folder?" (Trust folder / Not now record
    the answer and send; Escape sends nothing and keeps the draft; a host
    that cannot be asked never blocks the send). An untrusted thread's pane
    shows the line in `ThreadOpinionCards` with Trust folder, and an open
    thread nobody was asked about asks once per window.
    The grid's `tileConversation(…, threadLayout = true)` tells the pane to use
    the Threads layout; the Agent surface's pane passes nothing. `ThreadDiffPanel.tsx`
    takes the subagent column's slot and rules (`.detail-pane`, `has-detail`;
    the subagent detail wins), driven by `use-thread-pane.ts`'s
    `useThreadDiff` (`latest-read.ts` keeps one read in flight and only the
    newest pending selection, ignores obsolete results, and clears on unmount;
    re-read when a turn ends) and ⌘D (`threadDiffToggleAction`: a panel hidden behind a
    subagent detail is shown, not closed; closing it from inside hands focus
    back to the Diff toggle); `thread-diff.ts` holds its scopes and wording,
    `parsePatch` (hunks, numbered lines, the unchanged-line counts between
    hunks) and `formatDiffQuote`. The panel stacks every file under a sticky
    header; its scope menu lists each turn newest first, then Whole thread /
    Uncommitted / Branch. With `onQuote`, a line (Shift for a range, arrows
    and Shift+arrows from the keyboard) takes a comment and ⌘↵ hands App a
    `ThreadDiffQuote`; quoted lines stay marked until the thread or scope
    changes. File headers lead with a `FileBadge` and draw the folders dimmer
    than the name (`diffPathParts`); the totals end in a `DiffstatBar`; added
    and removed rows carry a 3px edge bar; `syntax-tokens.ts` lightly
    highlights each line on its own (no state across lines, lines over 2,000
    characters plain), drawn as spans. `turnUndo` offers the pane's own per-turn Undo
    (`restoreTurnCheckpoint`) beside a local thread's turn, never a worktree
    thread's. Its `ship` prop draws `ThreadShipBar.tsx` at the foot (protocol
    37): Commit… (the editable message over the exact files Rust listed, and
    what it leaves out), a branch off the default branch, Push & open PR, the
    pull request with its checks, Fix failing checks, Bring in review comments
    and Merge… behind `ConfirmDialog`. `use-thread-ship.ts`'s
    `ThreadShipController` runs each Rust ship command once per thread, holds
    every control after an unknown outcome until a read settles it
    (`readSettles`), and never retries; `thread-ship.ts` holds the words and the
    latest changes card's one next step (`shipNext`, `shipCardView`, carried on
    `ConversationThread.ship` through `withShip`). Fix failing checks reaches
    the pane as `threadSend` (sent as Retry sends, or queued behind a running
    turn); review comments reach the draft through `appendDiffQuote`, as a
    line comment does.
    Any way a thread becomes the Threads surface's focused pane visits it
    (`threadFocusVisit`, an App effect beside `selectSession`).
  - Turn controls: `Conversation`'s optional `thread` / `onThreadTurnDiff` /
    `onThreadEditFrom` / `onThreadForkAt` (the phone passes none).
    `thread-turns.ts` maps user rows to `ThreadView.turns` by prompt digest
    (SHA-256 of the CRLF-folded, Rust-trimmed prompt; k-th repeat ↔ k-th
    turn, aligned from the newest end when either list lacks its start); an
    unmatched row, a row whose turn crosses another matched row's order, and
    Claude's own interrupt markers and `!` bash echoes get no control. A
    matched row's change summary opens its turn diff and "Edit from here"
    opens `ThreadEditDialog` ("Keep the current files" / "Restore the files
    to before this message"; this thread is settled either way). Turn 1 goes
    in steps: `restoreTurn 0` when restoring (its recovery point is a status
    line), `settle`, the original's session closed, then the card on the same
    worktree (`existing`; a missing one is a New worktree, never Local). A
    finished turn's end gets "Fork from here". A worktree thread drops the
    Undo control; a local one keeps it. `thread-pane.ts` sends
    `editFrom`/`forkAt`, one per pane at a time (`ThreadPaneLock`; the
    pane's Fork and Edit are off until the answer), and the new thread's
    pane takes focus.
  - Threads layout (owner, 2026-10-06 redesign): `threadLayout` is true only
    for panes in the Threads grid; `.conv.is-thread-layout` sets the 736px
    reading lane the composer shares. `thread-transcript.ts` (pure) splits the
    merged rows into turns and folds everything between a finished turn's
    prompt and its final message into one work line ("Worked for 41s", then a
    chip per kind of work from `threadWorkChips`: "read 3", "edited 2"; its
    accessible name is the whole `threadWorkSentence`), never an error, failed or unfinished call, failed
    subagent, waiting approval, CLI-written row or the live seam. The duration
    is the run whose `startedAt` equals the turn's `sentAt` (`ThreadRun`,
    carried on `ConversationThread.lastRun`), else this window's own turn
    clock, else left out. The live turn never folds; Detailed opens every
    fold and a dial press re-seeds them. `components/ThreadTurnParts.tsx`
    draws the quiet line under a prompt (time, Copy, Edit from here), the
    work line, the changes card (Rust's counts, else the edit rows', with the
    five-square `DiffstatBar`; file rows from the edit rows, each led by its
    `FileBadge`; Review opens the turn diff) and the turn's controls
    (time, Copy, Fork from here, Undo turn) — always under the latest finished
    answer, on hover or focus under earlier ones. The bubble toolbar, the
    floating Fork row and the lone turn clock are not drawn in this layout.
    A request the owner can answer docks above the composer
    (`components/ThreadApprovalDock.tsx`, words and bodies in `thread-frame.ts`:
    "Run a command?" with `$ command`, "Edit a file?" / "Write a file?" with the
    file row and a small diff, Deny then Allow once; ⌘↵/⌘⌫ answer it from the
    pane or an empty composer); a failed call opens its output in a tinted block.
    Styles live in `components/ThreadTranscript.css`, imported after `ConversationRows`.
    Project hue: the "hued glow" (2026-10-06) went with the Threads look
    (owner, 2026-10-23). The pane draws no aurora, lit bubble, hued inline
    code, Review, rail or composer edge; a project's hue survives only as the
    8px square before its name in the inbox, the pane head and the new-thread
    picker, and the theme's accent draws Send, the working head line and the
    caret (`thread-hue.test.ts` holds that the transcript and the composer
    read no hue). Icons in Threads are `ThreadGlyph`'s line set; a step's
    glyph comes from its tool (`thread-look.ts`).
  - Live turn (owner, 2026-10-07; refined chrome only — Bare CLI keeps its
    running rows and working strip): the running turn draws itself under its
    prompt (`components/ThreadLiveTurn.tsx`, the pure model in
    `thread-live-turn.ts`, every step's words from `thread-live.ts`): a railed
    trail of steps, each its tool's glyph on the rail with its state on the
    right (`<button aria-expanded>` opening the transcript's own row in place;
    `+N −M` only once a call has a result; a running Edit/MultiEdit/Write
    peeks at up to four lines of its input), with the turn's prose
    interleaved, then a status line under it (since the Threads look,
    2026-10-23: the Pair mark `ThinkingMark`, the newest running step's verb
    and project-relative target or Thinking, or an amber dot and "Waiting for
    you"; an m:ss clock from `threadElapsedLabel`, the inbox's). An approval the owner can answer is never a
    card in the transcript — the composer docks it — only the waiting step
    (the running call matched by tool and summary, else the request itself).
    A steer (a user row with no Rust turn after a matched turn, where no Rust
    ordinal is free for it: `threadSteerKeys`) is part of its run: a "You
    steered" row live, a "steered N" chip once folded. A turn this pane
    watched finish plays `useThreadFinishBeat` (checks, the trail collapsing
    into the work line, the changes card and controls arriving); reduced
    motion skips to the end. `threadTurnTiming` remembers each turn's
    duration and end per window (bounded LRU), so turn 1's "Worked for"
    outlives turn 2's send. Scrolled away, the jump capsule carries the live
    step and clock, and in this layout never says "Approval waiting". A
    prompt this pane sends rises into place. No `.conv-working` strip is
    drawn while a live turn is.
  - Thread composer: `Conversation`'s `threadLayout` (a pane in the Threads
    grid only) draws `ConversationComposer` as one box on the transcript's
    lane (`ThreadComposer.css`, imported last): the field, then "+"
    (`ThreadComposerMenu.tsx`: context kinds, the slash launcher, the detail
    dial and the session-context readout, as an ARIA menu with roving focus),
    model · effort · access chips (`ThreadRunSettings`; the access rows are
    `access-chip.ts`'s; the access mode is no longer in the pane head), Fast, and the verbs —
    a 32px accent-square Send at rest, Queue ↵ / Steer ⌥↵ chips and a square
    Stop in the strongest ink mid-turn. Working, its edge stays the theme's
    hairline; waiting on the owner the edge and a still 3px ring turn amber.
    No dirty pill. Queued follow-ups use
    `SteeringQueue`'s compact card. Send, queue and steer are the same code
    paths as every other pane; `thread-composer.ts` holds the words, the
    menu's keyboard and `threadComposerAnswersApproval`. The pane head
    (`AgentTile` + `ThreadPaneHead.css` + `theme.css`) is two lines, 52px at
    least: the title, then the project's hue square and name, "on <branch>",
    where edits land, its pull request and lineage; a bordered Changes
    button; the title button wraps the title alone, so focus draws the whole
    ring. The kept prompt (`KeptPromptLine` in `PromptRecoveryPanel.tsx`,
    passed by `SessionTile` as the composer's `composerTop`, memoized so a
    streamed event leaves the composer alone) is one line inside the card's
    top edge with Retry or Continue safely, Put it back and Discard, drawn
    only when there is something to recover (`keptPromptShows`: not sent, an
    unknown outcome, or a failed or interrupted turn); a message delivered
    and answered draws none. The head
    holds a still 2px accent line while working and an amber one with a
    "Needs you" pill while waiting. Streamed words settle in
    (`markdown.tsx` `advanceFresh`, paced reveal only) behind an inline accent
    caret.
  - `thread-view-fixture.tsx` drives the card in each mode and a worktree
    thread's pane with its diff panel, and `thread-composer-fixture.tsx`
    presses the Threads composer (`INTERFACE_THREADS_ONLY=1 npm run
    test:renderer`).
- `collaboration-controller.ts` / `use-collaborations.ts` — injected API, list
  request lifetime, push/mutation receipt reconciliation and subscription cleanup.
- `orchestration-controller.ts` / `use-orchestration.ts` — the orchestration
  snapshot (newest revision per host run, equal reads keep identity), the
  canvas's nine intents (each re-reads the snapshot after the host answers) and
  the host's change pushes; `useCanvasReads` re-reads coordination status and
  the model catalogue, debounced, after each accepted snapshot read.
  `use-store-state.ts` mirrors a controller's snapshot into React state.
- `away-view.ts` — immutable detailed/compact Away projections; unchanged
  revisions retain identity and same-revision storage uncertainty cannot clear.
- `terminal-activity-tracker.ts` — per-generation parser and quiet-timer lifetime;
  completion counts publish independently of activity changes. Prune/dispose
  cancels timers. "Waiting" is Rust's answer (`TerminalSummary.attention`,
  protocol v44) handed in through `hostAttention`; the parser's own waiting
  only sequences working and idle. `terminal-view-focus.ts` guards delayed setup focus against
  replaced handles, hidden workspaces and a newly focused pane.
- `SurfaceSwitch` draws the centered Agent / Code / Threads navigation (Threads
  keeps the internal `"chat"` surface id and its storage keys). Equal
  toolbar side rails keep it centered; compact utility buttons include Usage,
  Voice, Attention and `WorkspaceMoreMenu` for Command palette. Settings lives in a persistent
  account-style sidebar footer below the scrolling workspace list: one quiet
  row, with "N providers need setup" and an amber dot on its right when
  providers need setup (owner, 2026-11-15); that part opens Models &
  providers, and the hidden sidebar's toggle carries the same dot. The
  toolbar fixture checks geometry and menu focus in wide and narrow windows.
- `store.ts` — `appReducer` + `initialState`: session registry, per-session event
  log and provider-command snapshots. `live-events.ts` owns frame batching,
  byte/count retention and delta coalescing; unresolved approvals and current
  control facts form a separate reserve and are never clipped to fit display history.
  A log over its caps is compacted to a 90% low-water mark
  (`LIVE_COMPACTION_LOW_WATER`), so streaming at the cap appends (and the
  conversation projection stays incremental) between compactions. Each live
  event keeps the ordinal it was appended at (`liveEventOrdinal`), and the
  reducer mints generated ids from it, so a compaction never renames the rows
  it keeps; a history page counts its ids from its end, so "Load older" keeps
  the rows already drawn. Each item also remembers the ordinal that created it
  (`liveItemOrdinal`), and a sent message's echo is placed by it
  (`PendingEcho.at`, `echoPosition`), so a compaction never slides the echo
  below the reply it prompted.
- `draft-store.ts` — composer drafts per session key, outside React state. App
  owns one store and publishes the collaboration draft state (whether the
  draft is empty, all the host keeps) when a write changes it
  (`draftWriteNeedsPublish`: every empty, never non-empty after non-empty),
  after every send, and once per newly listed session; only Claude, Codex and
  Grok sessions (`isCollaborationSourceKey`, the host's
  `CollaborationEngineId::ALL`) are published at all. Its revision is seeded
  from the clock (`nextDraftRevision`) because the host
  refuses any revision at or below one it accepted before a reload;
  the composer reads its entry with `useSyncExternalStore`, so a keystroke
  re-renders that composer alone, and every pane of one session shares it.
- `app-session-projection.ts` — memoized config/catalogue/progress/approval views.
  `conversation-display.ts` — pure row grouping and display labels.
- `history-paging.ts` — `HistoryPagingController` (history read generations,
  settled page size, the freeze once a session is live) and `PendingEventBatch`
  (frame-batched live events with a per-key deletion fence).
- `components/ProjectedConversation.tsx` / `conversation-projection.ts` — scoped
  incremental flat streaming projection, with the reference reducer retained
  for structural changes and unchanged row/subagent identity reuse. One
  projection per session (`conversationProjectionFor`) is shared by the pane,
  the subagent detail column and the Agent task inspector; each caller passes
  the session's current log, and `dropSessionClientState` releases it.
  `stable-values.ts` compares immutable DTO values; callbacks use committed refs.
- Conversation, Composer and FormSelect styles are imported once from component
  CSS files. Preserve cascade order and CLI/refined variants when consolidating.
- `terminal-workspace-activity.ts` sends ordered workspace visibility facts;
  Rust owns [idle sleep and wake](../WORKSPACE_TERMINAL_SLEEP.md).
- `IntegratedTerminalCenter` receives a typed terminal API from App, not its own
  `window.iface` reference; xterm completion/disposal releases output credits.
- `TerminalViewPool` owns visited emulators by terminal ID and generation. Pane
  slots move their existing DOM mounts; inactive workspaces park them hidden and
  inert while output still reaches the same parser and bounded scrollback.
  Each view is a memoized `PooledTerminalView` (in `IntegratedTerminalCenter`)
  taking its summary, `active`, `working` (from the pool's `workingKeys`) and one
  committed actions object, so a center render redraws only emulators whose own
  facts changed. `TerminalPane` is memoized too: `terminalPaneMembers` hands each
  pane only its tabs' entries, the center keeps equal `TerminalPaneTerminal`
  objects (`retainEqualItems`), and `renderTerminal` no longer changes with
  activity. Each pane sits in a `terminal-pane-frame` tile, the grid cell
  maximize and pane motion move (until 2026-10-14 it was the retired
  delegation split). The interaction fixture
  counts pane and emulator renders for title, activity, focus and splitter
  changes.
  Restart/dismissal retires old instances. The replay tail is only for initial
  attachment, not ordinary workspace switching. Pane body focus and terminal-tab
  drops follow native DOM ancestry across these center-owned React portals.
  `terminal-preservation-fixture.tsx` exceeds both replay limits, switches three
  times, checks screen/scrollback, split UTF-8/CSI, typing/paste, credits and cleanup.
- `canvas-font.ts` (2026-11-15): Chromium 150 (Electron 43.8.10, 0.3.177)
  refuses a 2D canvas `font` that begins with white space and keeps
  `10px sans-serif`. xterm's WebGL atlas writes upright text's font as
  `${style} ${weight} …` with an empty style, so every terminal's upright
  glyphs were tiny sans-serif spread across cells measured for the real font.
  `TerminalView.tsx` installs a guard at module scope that takes leading white
  space off the `font` setter of on-screen and off-screen 2D canvases; the
  `terminal-caret` lanes assert the real engine takes xterm's font string. If
  terminal text in a hands-free capture is tiny and letter-spaced, this guard
  is missing or the engine changed again: it is not a hands-free artifact.
- `terminal-webgl-pool.ts` retains up to 12 recently used GPU renderers across
  workspace switches. Only parked renderers can be evicted; their emulator,
  buffers and PTY keep running. Eviction explicitly releases the GL context
  rather than waiting for garbage collection, below Chromium's 16-context cap.
  The DOM fallback draws the same font visibly thinner, so a visible pane never
  stays on it by accident: one refused a full cache of visible renderers waits
  for the next release or park, and a lost context (sleep, display change, GPU
  restart) reloads after 1/5/15 s while slotted, the budget renewing after a
  minute up or on the next slot. A pane out of view (another workspace, a
  background tab, a hidden surface: TerminalView's `active` as well as its
  slot) is parked, and a renderer parked for 60 s is given back unless its
  pane still paints (under a dialog); after 2 min of a hidden window every
  renderer is, and the panes that were on screen reload on show. A returning
  pane reloads in the layout effect that shows it, before that frame paints
  (about 10 ms per pane), and a view mounted out of view takes none.
  Screen-reader mode (a string and an event per printed character, flushed
  only by a render) follows the same window on the pool's clock
  (`terminalWebglPool.later`): off for a view out of view past 60 s or created
  out of view, back on in the layout effect that shows it; a focused pane and
  one still painting under a dialog keep it. The pool's parking container sits
  outside every workspace and pane screen, so a parked pane skips an appearance
  change (it would read the 12px fallback for `--terminal-reading-size` and the
  window's `--fg`) and repaints when its slot takes it back, before that fit.
  `terminal-workspace-switch-fixture.tsx` verifies warm four-pane switches cause
  no GPU setup/teardown, passes the idle window on an injected clock
  (`terminalWebglPool.useClock`) and checks the returning workspace reloads
  before its first frame, loses a visible pane's context and waits for WebGL to
  return, visits 20 and 40 panes to exercise eviction and return, changes the
  text size with a workspace out of view and checks its panes on return, and
  checks the actual context bound, history, input and unmount cleanup. Timings
  are informational; lifecycle/resource assertions run in the renderer gate.
  See [workspace switching measurements](../PERFORMANCE_WORKSPACE_SWITCHING_2026-09-23.md).
- `TerminalLauncher` presents the Rust-owned roster as neutral provider tiles,
  eight session-count choices, shared-folder isolation and a numbered launch
  preview. `workspace-launcher-fixture.tsx` checks selection, unavailable agents,
  folder handoff, launch locking and scrolling at 1100, 640 and 360px in both
  styles and light/dark. `INTERFACE_WORKSPACE_LAUNCHER_ONLY=1 npm run test:renderer`
  runs this lane alone.
  The Workspaces plus menu also opens `CreateProjectDialog`: it shows Rust's
  Projects folder (`projectsFolder` read, protocol 40) with Change… (the native
  picker, then Rust stores the choice), App calls the Rust `createProject`
  operation and hands its canonical path to the existing workspace launcher.
  `project-creation-fixture.tsx` covers menu choices, the folder line and
  Change…, cancel, duplicate-name errors, pending submission locking and dialog
  focus/layout.

- `components/OutputPreviewPanel.tsx` / `OutputPdfPreview.tsx` — bounded project
  file browser and version-bound Markdown/text/diff/image/PDF inspection beside
  native terminals, and beside a project thread (review B5): the changes card's
  and Changes panel's Preview, and Quick Open's ⌘↵, open a PDF, image or
  Markdown file in the detail column the Changes panel uses (`openRequest`,
  `outputPreviewTarget` in `output-preview.ts`). Snapshots and unsent feedback
  stay in the renderer session; the Rust reader owns authority and revisions.
  See [output previews](../OUTPUT_PREVIEW.md).
- `outside-agents.ts` — Settings → Threads → Outside agents (protocol 65):
  the levels' words (Off, Read only, Can start and message threads; absent
  is off) and the one-line `claude mcp add` / `codex mcp add` setup the row
  shows; a thread an outside agent started reads "Started from outside
  (<client>)" in its pane head (`startedFromOutsideWords`).
- `started-threads.ts` / `components/StartedThreads.tsx` — team tools
  (protocol 57): the "Started threads" line above a thread's composer, each
  thread its agent started (lineage `child`) as a status dot and its title,
  newest first, six then "+N more", derived from the Threads view; a started
  thread's head reads "Started by <caller>" (`lineageWords`). The setting is
  Settings → Threads → "Let agents start and read other threads".
- `visual-reply.ts` / `components/VisualReply.tsx` — visual replies (protocol
  61, T3 §8): a `visual` transcript item (from `visual.shown`) draws a card
  with the page in `<iframe sandbox="allow-scripts">` on an `iface-visual:`
  URL, "Source" and "Open larger" (an overlay over most of the window). App
  builds the one `visualFrameLoader` (`threadJournal` `visual`, then
  `visualFrame` register with the wrapped page); it reaches rows through
  `ConversationItemContext.visualFrames`. Output preview's Render on an
  `.html`/`.svg` file uses `RenderedPagePreview`, the same frame.
- `thread-schedules.ts` / `use-thread-schedules.ts` /
  `components/{ThreadScheduleDialog,ThreadScheduleLine,ThreadSchedulesSettings}.tsx`
  — scheduled prompts (protocol 62): the pane ⋯ "Schedule a prompt…" dialog
  (once, every N minutes or hours, at least 5, or a calendar), the "Next:"
  line above the composer with Edit, Cancel and "+N more", the Ship bar's
  Watch toggle (`ThreadShipBar` `watch`), Settings → Threads → "Scheduled
  prompts", and the transcript's fold of a scheduled or watch prompt
  (`scheduledPromptFold`, drawn by `ThreadHandoffPrompt`). Protocol 66: a
  `resume` schedule draws "Resumes at <time> · Cancel" (`resumeLineText`)
  and folds as "Resumed after the limit reset"; Settings names it "Resume at
  reset" (`scheduleRowLabel`). The limit line (`ThreadOpinionCards`) adds
  Resume at reset and Snooze until reset beside its other offers once Rust's
  `ThreadView.limit` knows the reset (`threadLimitReset`), alone when there is
  nowhere to switch; Resume carries the queue's first row when it never left
  (`resumeQueuedPrompt`) and App removes that row once Rust holds it.
- `thread-opinion.ts` / `components/ThreadOpinionCards.tsx` — Second opinion
  (review B4), Switch to (protocol 59, T3 §5; it replaced Continue with) and
  the limit offer (review E): which other providers a thread offers
  (`threadProviderOffers`: `opinion` and `switchTo`, from the probes), each
  consult on the thread as a card above its composer (`threadOpinionCards`:
  review the exact packet and Ask, the other provider reading, its reply held
  until Send to <provider> or Dismiss, a failure), and Switch to <provider>
  when the thread's provider reported its limit (`threadLimitOffer`, the
  provider with the most headroom; never automatic). A thread is at its
  limit while its event log's latest usage names one or Rust still records
  one (`threadLimitNotices`: Claude's error result clears the log's notice
  at once). The pane `⋯` and the
  inbox row menu offer both; App runs `secondOpinion`, `switchProvider`
  (`use-thread-bridges.ts` → `thread-pane.ts` `switchThreadTo`) and the
  collaboration verdicts. A switch moves the pane in place
  (`workspace.ts` `replaceWorkspaceKey`), the draft and attachments
  (`carryKeyed`) to the new head, drops the old session's client state, seeds
  the pane with the thread's history from before the switch (`switchSeed`:
  the new head is live before its pane opens), and echoes an ACP session's
  launch handoff. The head says "Switched from <provider>"
  (`threadSwitchedFrom`), a handoff row is folded ("Handed the thread to
  Codex", `threadHandoffLine`, drawn by `ThreadHandoffPrompt` in
  `ThreadAwayParts.tsx`), the inbox leaves a chain's earlier sessions out,
  and only the current session's stretch (after the last "Switched to …" row,
  `headStretchPromptRows`) gets turn controls.
- `components/BrowserPane.tsx` / `browser-pane.ts` / `browser-page.ts` — the
  Browser pane: a layout tab (`browser:` id, `isBrowserTabId`) whose body is
  `BrowserPane` (address bar, notices, and a slot). The `<webview>` (main pins
  it; see `electron-main.md`) belongs to a `BrowserPage` in the centre's
  `BrowserPages`, which lives in `.browser-page-layer` and is CSS-anchored over
  the slot, because a `<webview>` that leaves the document reloads and panes
  move on every layout change. `IntegratedTerminalCenter` owns the pages, their
  labels, the server lookup (`inspectTerminalServers`) and the saved address
  (`setBrowserTabUrl`). main's Browser events (`iface:browserEvent`, which
  App alone subscribes to and hands to the centre's `browserEvent`) reach
  their page by guest id (`BrowserPage.forWebContents`): a link's new tab
  (`openBrowserTabBeside`, right after the page), ⌘F (the find field, the
  terminal search's look, portalled over the page) and ⌘L in the page, and a
  download's line in the pane. A page's fullscreen fills the window from
  Chromium's top layer (the `<webview>` is the app's fullscreen element);
  `.browser-page.is-fullscreen` only moves its host along. `BrowserPane` is `memo()`'d with stable props, so a
  terminal's title or activity never redraws it. `BrowserColumn` is the agent
  overview's Browser tab: one `BrowserPage` with its own layer in the column,
  made when the tab is first shown and disposed with the column.
  See [workspace tools](../WORKSPACE_TOOLS.md).
- `browser-point.ts` / `browser-point-page.ts` — point and fix in the Browser
  pane: Pick and Region in the bar, a card with the picture, plain words, the
  selector, a comment and the thread, and Add to draft (App's `addBrowserNote`:
  the note into `conversationDrafts`, the PNG through `attachPastedImages`;
  never sends). The picker runs in the page's main world via
  `<webview>.executeJavaScript` (each in-page function is sent as source text,
  so it must stay self-contained); `BrowserPage` crops with `capturePage` and
  keeps the card and the width preset (Fit, Phone, Tablet, Desktop). Every
  page result goes through `browserPickFromPage` before it is shown.
  `INTERFACE_BROWSER_POINT_ONLY=1 npm run test:renderer` runs its lane (a real
  page in a real `<webview>`; the only lane that enables the tag). See
  [point and fix](../superpowers/specs/2026-11-14-t3-systems-design.md).

**Pure model and rules**

- The conversation model, one module per rule set, each imported directly
  (there is no barrel): `conversation-reducer.ts` (`ConvItem` and the `conversationItems`
  reducer with subagent grouping, plan and context reports),
  `transcript-merge.ts` (`TranscriptRow`, `PendingEcho` and `mergeTranscript`
  ordering), `approval-keys.ts` (verdicts and the approval chord),
  `transcript-import.ts` (imported-session markers), `transcript-density.ts`
  (the density dial), `tool-activity.ts` (tool/file-change/work-group labels,
  turn change summaries, `toolGlyph`, tool input blocks), `transcript-search.ts`
  (the bounded find corpus) and `transcript-navigation.ts` (truncation notice,
  jump capsule, prompt anchor, paced reveal).
- The view models, one module per rule set behind the `viewmodels.ts` barrel:
  `sidebar-view.ts` (sidebar rows/groups/filters, terminal-to-project
  projection, import labels), `session-activity.ts` (turn clock and progress,
  the live activity line, and the single `needsAttention` predicate feeding the
  toolbar attention tray and the dock badge), `session-config-view.ts`
  (engine-capability map, config and fast mode in force, permission picker)
  and `usage-view.ts` (plan-usage views and each provider's `usageHeadroom`,
  "38% left", on the model chips and launcher tiles; `plan-usage-reads.ts`'s
  `PlanUsageReads`, used by `use-plan-usage.ts`,
  decides when the usage CLIs run: Refresh at once, a finished turn no sooner
  than a minute after the previous read began, and nothing but Refresh while
  the window is hidden). `sessionSurfaceOf` keeps conversations on
  Chat, including imported, voice, and code-capable sessions; Agent tasks stay
  on Agent. Code's sidebar contains only separately projected CLI workspaces.
  Sidebar filtering, navigation and restored conversation panes share this view
  classification. Provider execution kinds and their defaults remain unchanged;
  saved terminal workspaces remain Code-owned. Code therefore has no
  conversation pane layout (`conversation-layouts.ts`; its old slot and the v1
  layout are swept from localStorage on load): every session reveal (sidebar,
  notification, fork, a canvas node, Agent's Files touched, the
  adversarial-review pair) opens on the session's own Chat or Agent surface,
  and the review pair's two-pane reservation is Chat's. The palette's Open in
  external terminal opens the project of the terminal in front.
  `Sidebar` uses a stronger neutral selection fill for a workspace and a quieter
  one for its owning project,
  aligned count chips, and a 2-column by 3-row dot ticker beside the working CLI status dot.
  `sidebar-reference-fixture.tsx` covers selection, keyboard navigation, narrow
  rows, live-to-idle transitions, and reduced motion in light/dark and both styles.
  `INTERFACE_SIDEBAR_REFERENCE_ONLY=1 npm run test:renderer` runs that lane alone.
  Project menus offer persistent Pin project / Unpin project actions. Pinned
  projects keep their expandable workspace groups in Pinned, with independent
  workspace pins shown once. Empty pinned projects remain launchable; search
  and project archiving still filter them. Paths distinguish identical names.
  `workspace-hierarchy-fixture.tsx` exercises both pin kinds, collapse and focus.
  Workspace/project menus wait for a visible item before moving focus, with a
  bounded wait that cancels if focus moves elsewhere or the menu closes.
- The Threads inbox ([Threads design](../superpowers/specs/2026-10-06-threads-design.md)):
  on the Threads surface only, `Sidebar` draws `components/ThreadInbox.tsx`
  in place of the project list, the Needs attention lift and the project
  folds, and puts its project scope picker in the header; Code and Agent
  render exactly as before. `thread-inbox.ts` is the pure model: the Threads
  listing (the session sidebar's filters, before its search) joined to
  Rust's `ThreadsView` into Pinned / Active / Snoozed / Settled with Rust's
  order keys (unkeyed rows after, newest first), the scope filter (All
  projects, a project, No project), the one-mark status ladder (needs you >
  working > failed > unread done > woke), what each row draws, and the
  commands the row menu, drags and Undo send. Pinned and Active rows are two
  lines (54px, 2px apart, since the Threads look of 2026-10-23): the title
  with one slot (`threadRowSlot`: Needs you as the one filled amber pill,
  Working as the 13px Pair mark (`ThinkingMark`) and an m:ss clock, Failed as
  an outline pill, Back as a muted word, else the age after the unread dot),
  then line 2 (`threadRowLine`), led by the project's 8px hue square (a ring
  for No project): while
  the session is live, its running step ("Editing login.ts", from
  `threadLiveFacts` in `session-activity.ts` over its own log and
  `thread-live.ts`), what it asks ("Run npm test?") or its error's first line;
  otherwise project and branch with the pull request chip or diff totals
  (`threadRowTrail`), and the provider's own mark at the right. A row that
  needs you is drawn first in Pinned and Active (`threadNeedsYouFirst`, a FLIP
  slide in `ThreadRowsMotion`); drags and Move up/down still compute their
  places from Rust's order (`threadDropBeforeKey`). The project's hue
  (`project-hue.ts`) is only that square; selection and hover are the theme's
  row fills, and the sidebar draws no header band. The pills' and lines' inks
  hold 4.5:1 on any row state
  (`ThreadInbox.test.ts` measures every palette). Snoozed and Settled are shelves pushed to the foot
  of the list, in slim 34px rows (the square at half strength; `threadShelfSlot`:
  wake time or age). A row's age is the newer of the
  session's `lastActivity` and its last run's end (`threadActivityAt`). On
  hover or keyboard focus the slot gives its box to Snooze (the presets as a
  menu) and Settle, or to Wake / Un-settle on a shelf; the row menu is the
  right-click and ⇧F10 menu, its chord hints from `THREAD_MENU_CHORDS`.
  A session Rust has no record for is Active, created at
  `firstSeen`, never unread — until Rust's view has loaded; then only for
  five minutes after `firstSeen` (Rust leaves hidden workers out on
  purpose). Chat's session list hides the same workers: App's
  `workerSessionKeys` reads the view's `hiddenKeys` (protocol 58), which Rust
  takes from the whole orchestration history, plus the 64-agent orchestration
  snapshot's workers, which name a just-spawned worker before Rust's next view. A recorded app-started thread is exempt from the sidebar's
  small-transcript declutter (`threadListingRevealed`). The row menu offers
  Refresh pull request for a tracked worktree thread, and the hover card
  names its pull request. `use-thread-inbox.ts` is App's half: the scope
  preference, the command sender (the first refusal is reported, nothing is
  retried) and the visit when a thread's run ends while it is the focused
  pane of a focused window; `selectSession` visits the thread it opens.
  Snoozed and Settled start folded (a renderer preference); Settled pages 50
  at a time; a search opens every section. Settle, snooze and archive offer a
  six-second Undo toast and ⌘Z while the sidebar has focus. Chords: ⌘⇧[ / ⌘⇧]
  previous/next thread, ⌘⇧S settle, ⌘⌥P pin (⌘⇧P stays the command palette).
  Notifications for threads you are not looking at stay on the existing
  attention path. `components/sidebar-controls.ts` holds the menu keyboard
  model, overlay placement, confirmations and peek timing both lists share.
  `thread-inbox-fixture.tsx` (`INTERFACE_THREAD_INBOX_ONLY=1 npm run
  test:renderer`) drives the real Sidebar: section order, shelves, row
  geometry and slots, pills and their contrast, hue squares, hover
  actions, folds, scope, menu and submenus, drags, Undo and chords, with
  screenshots.
- `workspace.ts` — the up-to-four agent pane layout algebra and overlay keyboard
  arbitration.
- `terminal.css` paints idle and selected pane edges above their content. The
  native emulator keeps its fitted whole-row height, bottom-aligned inside a 2px
  inset, so fractional row space stays above the CLI rather than below its footer.
- `terminal-announcements.ts` bounds xterm's offscreen live region to 2,048 UTF-16
  code units, retaining recent output with an omission notice. Cursor redraws
  without line feeds bypass xterm's own line-count limit. The observer clips at
  the mutation checkpoint; `terminal.css` prevents wrapping in the 1px box.
  Accessible terminal rows, PTY data and scrollback remain intact. The terminal
  interaction fixture exercises sustained cursor animations in four real panes
  before input/history resets, as well as oversized updates and input clearing.
- CLI panes always display the native emulator. Clean GUI was retired by the
  owner on 2026-09-14; implementation, design and review history are preserved
  in [the archive guide](../archive/clean-gui-20260914/README.md). `appearance.ts`
  renders legacy stored `paneStyle: "clean"` as Terminal without rewriting the
  frozen appearance contract. Settings no longer offers Clean or its input choice.
- `terminal-prompt.ts` retains bounded native screen translation and empty-prompt
  guards for phone input and workspace sleep (the Control assistant that also
  used them was retired on 2026-10-14). The Rust settings/approval
  backend and generation-fenced input contracts remain unchanged.
- `cli-pane-appearance-fixture.tsx` checks four real emulators across legacy
  appearance values, palettes, chrome and widths: native geometry, input routing,
  draft/scrollback preservation, fullscreen buffers, and absence of Clean overlays.
- `ConversationPaneSurface` covers saved conversations without remounting drafts,
  disclosures or scroll state, and publishes `PaneVisibleContext` (the Agent
  surface provides it too). A covered pane holds the log it last drew while only
  streamed text arrives, and catches up when shown, on a turn or liveness
  change, or on any other event; its reveal snaps instead of pacing.
  `conversation-surface-fixture.tsx` checks draft and scroll preservation,
  per-session sending, approvals, the covered hold, the snap and the pin across
  hide and show, and that streamed Markdown draws what a whole parse draws.
- `agent-overview.ts` / `components/AgentOverview.tsx` — the right-side agent
  overview (`clamp(360px, 22vw, 560px)`, an Agents / Tasks / Browser tab row, its own
  toggle at the rail's right edge): every agent pane across every saved workspace, grouped
  Needs attention / Working / Done / Idle / Stopped (or by workspace) from the
  center's registry, workspace records, live activity and done marks (plain
  shells excluded), with chips, search and Close idle. Live cards (owner,
  2026-10-01) show each agent's last screen lines: the center reads them off
  the mounted xterm buffers through `terminalCardPreview(…, limit)` while the
  overview is open (on open, on activity changes and every 2 s), and keeps
  `agentOverviewSince` — a status first seen has no clock, only a watched
  change starts one. Close idle confirms in the panel, then ends each idle
  agent the way deleting its workspace would. The terminal center owns
  its open state and tab (`localStorage`, closed on Agents by default) and `locateTerminal`,
  which switches workspace, attaches the tab and hands `TerminalPane` a
  `locate` nonce for its one-shot `is-locating` ring. `agent-overview-fixture.tsx`
  (`INTERFACE_AGENT_OVERVIEW_ONLY=1 npm run test:renderer`) drives it end to end.
- `workspace-hierarchy-fixture.tsx` checks the complete shell with three populated
  xterm panes at 1920, 1280 and 760px in both styles and light/dark, including
  header bounds (the head holds only tabs and actions: no folder, state
  words, Find or Files), the Find row sitting above the text, sidebar
  toggle, maximize/restore and preserved native input.
  `INTERFACE_WORKSPACE_HIERARCHY_ONLY=1 npm run test:renderer` runs that lane.
- `terminal-workspace.ts` — the terminal pane/split tree: normalization,
  focus/tab cycling, open/close/move, splitting, ratios, tidy, balanced attach.
  Repeated focus/active-tab selection retains identity, including maximized panes.
  Focusing another pane's already-active tab preserves the pane array. Center
  refits follow focus/visibility/geometry changes, not unrelated registry titles.
  `terminal-interaction-fixture.tsx` checks four real emulators with long history,
  repeated focus, background title changes, scrollback and typing/pane switches
  under output load. `INTERFACE_TERMINAL_INTERACTION_ONLY=1 npm run test:renderer`
  runs this lane alone; its counters are deterministic and timings informational.
- `terminal-workspaces.ts` — saved terminal workspaces: membership, chat-resume
  bindings, roster normalization, rename/forget and reopen requests. Selection
  preserves the saved roster order; newly created workspaces start at the front.
  A workspace reopens only the saved panes its layout shows, and
  `forgetClosedTabs` drops the rest from the roster when it loads.
- No terminal runs where no pane shows it (owner, 2026-10-01). Before this, a
  closed tab kept running unseen and every launch respawned it: production
  had 24 such terminals. Now:
  - Closing a tab or a pane's header × ends the terminal through
    `endTerminals`, the same path as Close idle.
  - A launched terminal the host started (a pass-off, or a voice or phone
    `start_pane`) is seated in the active workspace.
  - Anything still running outside every layout is ended by
    `terminal-hidden-sweep.ts` once it has been hidden and quiet for 30 s.
    Terminals in flight are skipped: creates, reopens, closes and launch
    batches. So is any terminal that is working or waiting.
  - The hands-free run covered all three: a closed pane leaving the host
    registry, an unplaced terminal swept at about 30 s, and a saved closed
    tab not respawned after a reload.
- A created terminal is placed in the workspace that asked for it.
  `createTerminal` remembers the workspace shown when it was called. If the
  owner selects another one before the host answers, the new tab, split or
  replaced tab slot goes into the asking workspace's saved layout, never into
  the live one. An agent CLI that exits 0 by itself is handed a shell by Rust
  (protocol 50), whatever is shown; the renderer only reflects it
  (`terminal-shell-handoff.ts` `placeHandedOffShell`): the shell, whose
  summary `replaces` the agent, takes the agent's tab slot in the live layout
  and in any saved (parked) layout, and is filed in the agent's place, on the
  push or on load after a reload. `reopenRequests` never reopens an agent a
  live shell replaced. `terminalExitHandoffFixture` and
  `terminalHandoffSwitchFixture`, in the `terminal-reopen` lane, cover a
  parked exit, a switch before the shell arrives and a reload that missed it.
- `terminal-reopen-retry.ts` — `reopenWithRetry`: a bounded, dependency-injected
  retry around a saved pane's reopen. Right after a host restart the host can
  refuse a cold reopen transiently (managed-worktree ledger still verifying,
  proof sidecar mid-settle: `terminal-authority-unavailable` /
  `-reopen-authority-unproven` / `-reopen-authority-mismatch`). Each retry
  rebuilds the request from the freshly settled host roster; the host authority
  gate is unchanged and re-runs every attempt, so a genuinely gone session still
  ends as a failed tab. A refusal the host marks retryable retries too; one
  carrying any other host code, marked unsafe to replay (`host-exited`), or with
  an error-shape marker this build cannot read goes straight to the shell
  fallbacks. The code arrives in the v1 error shape (`shared/ipc-error.ts`,
  read by `ipc-message.ts`); a refusal without it (main's own, such as the
  terminal data channel still connecting) keeps the retry.
- `components/TerminalReopenFailure.tsx` — the failed tab's body: the host's
  refusal in words instead of an empty pane. The host refuses a cwd that is a
  link, so a project folder that moved and left a link behind (for example
  `~/Desktop/Projects/X` → `~/Developer/X`) fails every saved pane in it.
  `iface:resolveMovedTerminalFolder` tells the tab where the link leads, and
  "Reopen in …" saves the relocated roster with `movedFolders` through
  `TerminalWorkspacePersistenceQueue.saveMove`, which keeps the move on any
  newer snapshot that replaces it, then retries each failed pane there.
- `terminal-output-state.ts` — reconciling PTY output generations with
  control-plane summaries, summary merging, output-buffer pruning. A winning
  row equal to the one held keeps the list (a resize reply and its lifecycle
  echo); the lifecycle side effects still run for it, and the center then moves
  only `registryRef` (the revision fence) without rendering.
- `terminal-stream-router.ts` — `TerminalStreamRouter`, the one owner of
  per-stream output state: mounted views and their unacknowledged writes (the
  output credits), replay tails, byte cursors, sleeping screens and retired
  terminal ids. `deliver` routes a PTY chunk; `attach` restores a remounted
  view (bracketed paste for Claude/Codex, modes, notice, tail or a repaint
  request); `detach` credits unfinished writes; `retire(id, minimumGeneration?)`
  ends every fact of a removed terminal or of passed generations, and `sync`
  follows the registry. `IntegratedTerminalCenter` keeps only the React wiring,
  the redraw pulse and the resize queue.
- `terminal-replay-buffer.ts` — head-indexed replay deque, capped at 1 MiB and
  4,096 chunks, preserving UTF-8/escape boundary metadata during eviction.
- `terminal-replay-modes.ts` — bounded observation of buffer, mouse, cursor-key
  and bracketed-paste modes across the full PTY stream. Truncated TUI recovery
  restores those modes before repainting; Clear retains them and a new stream
  generation starts fresh. Mode resets remain authoritative.
- `terminal-resize-queue.ts` — debounced, at-most-one-in-flight PTY resize sender.
- `terminal-redraw-pulse.ts` — generation-fenced recovery after truncated agent
  replay. The center holds the temporary geometry for at least 250 ms (beyond
  OpenTUI's 100 ms resize debounce) even if animation output arrives; a 500 ms
  no-output fallback restores the fitted geometry. Output alone does not prove
  that the provider handled SIGWINCH. The real-xterm fixture covers two OpenCode
  panes, replay eviction, ongoing delta output and three workspace round trips,
  including scrolling history in each pane and disabling TUI input on exit.
  OpenCode additionally waits for OpenTUI's resize-time `CSI 14 t` pixel query,
  observed with the full stream's parser boundary, rather than accepting an
  animation frame as progress. Its fallback is three seconds; other providers
  retain 500 ms. This covers a busy event loop collapsing both geometries into
  a no-op. The fixture includes an 800 ms busy interval and a fragmented query;
  isolated OpenCode 1.18.30 PTY testing reproduced the old no-output recovery
  and confirmed complete screen output when waiting for the resize query.
- `steering-queue.ts` — durable per-session prompt queue: normalization/limits,
  auto-deliver gating, turn start classification, in-flight and paused
  markers, reordering, the localStorage codec.
- `turn-events.ts` — the one turn-ended (`eventEndsTurn`) and turn-failed
  (`eventFailsTurn`) predicate, read by the steering queue (under its
  `steeringQueue*` names), `live-events.ts`'s approval reserve,
  `activeApprovalsFor` and the subagent-card reducer.
- `outbound-lane.ts` / `use-outbound-lane.ts` — `OutboundLane`, the single
  outbound lane for composer prompts, provider commands and queued steering
  prompts, behind an injected port (App's is the only one; it is the
  `window.iface` send/interrupt path plus App's refs). It owns the durable
  queue map, the per-session send/turn latches and attempt generations, the
  unknown-vs-definite outcome split (`deliveryOutcomeUnknown`: a failed
  transmission is unknown unless the host's typed error proves nothing was
  delivered — replay-safe, or `effect-definitely-rejected`; an untyped or
  unreadable one is unknown; a failure before transmission stays definite), the durable
  pause on Stop or a bad turn end, and one-per-turn auto-delivery. App forwards
  every engine event to `observe` and runs `autoDeliver` after each render; a
  queue map storage refused is written again after the render that shows it.
  `outbound-lane.test.ts` drives it with a fake port.
- `prompt-context.ts` — the `PromptContextItem` model: kinds, capabilities,
  normalization with limits, token estimates, and the provider-neutral prompt
  serializer with its atomic budget rules.
- `prompt-attach.ts` / `prompt-drop.ts` / `prompt-paste.ts` / `prompt-submit.ts` —
  authorized paths → context chips with thumbnails; drop acceptance, hover
  counting and the on-disk/in-memory split of dropped files; which clipboard
  entries become images (text always wins); Enter/Shift+Enter/IME rules.
  Both prompt surfaces (the launch card and a session's composer) spread one
  `promptDropZone` and paste through one `pasteImagesIntoDraft`.
- `provider-commands.ts` — slash-command matching, Enter-submits gating,
  canonical command text, intent classification, the command-menu key map.
  `use-provider-command-menu.ts` is the slash menu both prompt surfaces draw:
  its rows, active row, keys, insert and run.
- `model-catalog.ts` / `model-fallbacks.ts` / `model-presets.ts` — live-session
  picker rows and catalogue merging; cold-render-only built-in Grok/Qwen rows;
  the `interface.modelPresets.v1` store with per-engine MRU.
- `session-status.ts` — the composer footer view-model.
- `conversation-pane-rules.ts` — the pane's pure rules: shortcut ownership,
  submit/steer/escape dispatch, the failed-send draft, the restore
  confirmation and the finished-turn announcement.
- `display-paths.ts` — how a path is shown (`displayPath`, `homeRelativePath`,
  summary path shortening); shared by the transcript and the terminal headers.
- `conversation-types.ts` — the pane's caller-facing shapes: `SessionConfigView`,
  `HistoryView` (with the history prop contract) and `TurnRestorePoint`.
- `session-presentation.ts` — hides permission controls for Chat/Voice and project
  references for tool-free Chat; the first-send restore guard uses the same
  permission boundary. Rust remains the operation authority.
- `access-modes.ts` — permission-mode copy, tone class, fallback/reported merge.
- `diff-stats.ts` — per-session added/removed counts from `diff.produced`
  events with honest "unknown" rendering. A diff whose call came back
  `ok: false` (declined or failed) is skipped (`appliedDiffEvents`; over
  transcript items the same rule is `tool-activity.ts`'s `appliedDiffs`). The live log keeps each diff event
  without its file bodies (`slimDiffEvent`), its line delta cached on it.
- `recap.ts` — "while you were away" snapshots and the provable-clauses-only line.
- `review.ts` — credential-filtered Review patch context within a hard budget.
- `turn-checkpoints.ts` — per-turn Restore offers keyed to transcript rows,
  scoped to this app run.
- `canvas-layout.ts` — agent-canvas node positions and hidden nodes in
  localStorage, deterministic auto-placement, edge paths.
- `terminal-activity.ts` — the PTY activity projection and explicit CLI completion
  counter; a quiet timeout is never evidence of a completed turn.
  `isTerminalProtocolReply` keeps xterm's own focus and capability replies
  from answering a waiting pane. Its title glyphs, Codex OSC 9 phrases and
  reply rule are shared with `crates/interface-terminal` through
  `electron/src/fixtures/terminal-agent-activity-v1.txt`, which both suites read.
  Delimiter searches advance independently within each chunk, including when
  ESC or BEL is absent. `scripts/benchmark-terminal-activity.mjs` reproduces the
  parser timing against an optional baseline Git revision without opening a server.
- `notify.ts` — notification delivery, including permission continuations, and
  the edge-trigger helpers for sessions and terminal input/completion signals.
- `terminal-notifications.ts` — the terminal notification effect shared by App
  and the DOM fixture: records completion in the bell menu before attempting
  the OS alert and routes clicks to the originating pane and workspace.
- `corner-notices.ts` — the main area's error notices (owner, 2026-11-15; a
  full-width bar across its top before): a refresh failure in plain words
  ("Codex could not refresh its sessions."), its stable id, newest-first
  order, three shown with the rest folded into "N more", and how far the
  stack rises (`cornerLift`) to clear a composer. `components/CornerNotices.tsx`
  draws them in `.workspace-content`'s bottom-right corner, measuring
  `.thread-composer`, `.conv-composer` and `.terminal-startup-download` only
  while something shows. App feeds it the action error, a rollback's result
  and each refresh failure; only a refresh has Try again (it re-runs
  `refreshAll("explicit")`). Errors keep their live-region role and the CSS
  `error-linger` countdown (folded ones stay mounted so theirs runs); hovering
  or focusing the stack holds them all. The stack is inert under the
  composer panel and Settings, like the notices above it. Covered by
  `corner-notices.test.ts` and the `workspace-toolbar` DOM lane.
- `attention-tray.ts` — the toolbar bell's state: live needs (blocked agents and
  panes, derived from the same listing the badge counts) plus finished/failed
  events recorded at the notify edge; rows, unread count, wording. Drawn by
  `components/AttentionTray.tsx`; a row jumps to its session or pane
  (`IntegratedTerminalCenterHandle.focusTerminal`).
- `use-task-inspector.ts` — the Rust-owned task harness and `TaskInspector`:
  one serialized lane for every harness command (a read on mount and each
  window focus), the inspection target and its epoch, and the inspector's
  props. Each harness answer replaces the tray's event rows with the
  harness's attention history (`harnessAttentionEvents`); the bell's count is
  `attentionUnreadCount`. Attaching a follow-up to a terminal still composes
  the saved draft in the renderer (read, append, save with its revision).
- `launch.ts` — `ComposerForm` → `LaunchRequest` validation and the engine
  fallback.
- `composer-model.ts` — the launch card's pure model (`components/Composer.tsx`
  draws it): engine choices, scope and team fields, access-mode and effort
  vocabularies, the combined provider+model choice codec, pre-launch command
  scope, field errors and form seeding. `effort-levels.ts` is the effort
  vocabulary every effort surface shares (names, ladder order, the top tier).
- `settings.ts` — `AppPreferences` schema, the `interface.settings.v1` storage
  boundary, provider probe summaries, updater state → product copy.
- `appearance.ts` — renderer-owned appearance attributes and the `useAppearance`
  IPC hook. `appearance-controller.ts` serializes/coalesces material saves and
  rolls back failed final writes to acknowledged values. Rust owns persistence.
  `applyAppearance` remembers what the document shows; `useAppearance` starts
  from it (main.tsx's pre-mount read), so a launch never repaints the light
  sentinel, and `interface-theme-change` goes out only when the palette,
  terminal glass or terminal text size changes (`appearance.test.ts`).
- Looks (2026-11-15, [guide](../THEMES.md#looks-a-backdrop-behind-glass)): a
  theme file that sets `--theme-backdrop` or `--theme-scene` gets one backdrop
  under the window and glass over it. `theme-look.ts` decides
  (`themeDrawsBackdrop`), adds the `.theme-backdrop` elements to `<body>` at
  `z-index: -1` and sets `data-theme-look="backdrop"`; `theme-look.css`
  scopes every glass rule to that attribute, so built-in palettes and plain
  theme files are untouched, and sets `--terminal-glass-active` so xterm
  drops its ground. An SVG scene is decoded from its data URL, cleaned
  (`cleanScene`: drawing elements only, no handlers or outside links) and
  drawn inside a shadow root. Motion: the backdrop's animations are always
  paused in CSS, and a stepper moves each one's `currentTime`
  (`stepBackdrop`) at the rate `backdropFrameRate` gives: 30 a second with
  the window in front (`data-window-active`), no Reduce motion and the
  `themeMotion` preference not `off`; on the battery
  (`navigator.getBattery()`, `charging` false) 15 a second, none after a
  minute without key, pointer or wheel input, and none at all when the
  preference is `plugged`. A timer wakes just before a frame is due and the
  step is taken in `requestAnimationFrame`, so an undrawn window steps
  nothing; the animation list is read every two seconds, and a look with
  nothing to animate stops the stepper until the theme, the window size or
  the setting changes. Blurs of zero are not written: `data-blur` on the
  backdrop and `data-theme-frost` on the root (`drawsBlur`) gate the art's
  `filter` and the panels' `backdrop-filter`, because `blur(0px)` is still
  composited as a filter on every redraw. The sidebar is frosted
  (owner, 2026-11-16) without a `backdrop-filter`, which blurred what was
  behind it again on every frame: `createThemeFrost` adds
  `.theme-backdrop-frost` after the backdrop, `paintFrost` clones the art
  into its pane (the scene drawn again in its own shadow root, a plain canvas
  the shader's picture is copied to by `MountedShader.copyTo`), and
  `theme-look.css` blurs that pane and places the box by the sidebar's own
  (anchor positioning on `--interface-sidebar`, so no script measures the
  page). The copy is not stepped: `refreshFrost` sets each of its animations
  to its partner's time (`pairFrostAnimations`) when `frostRefreshDue` says
  so (every 500 ms, 1000 ms on the battery), when the backdrop comes to rest
  and in `lookAgain`. Stowed (`.app-grid.no-sidebar`), in the narrow window
  and under Reduce transparency the sidebar is solid and the frost is not
  drawn. `loop-rest.ts` leaves the
  backdrop's loops alone. `docs/theme-examples/check-theme.mjs` names what a
  theme would draw again on every frame; the test runs it over the examples.
  The picker marks moving looks: the host adds `--theme-look` (`moving` for a
  shader, a scene with CSS or SMIL animation, or an `animation` on the
  backdrop or its layers, read from the text and erring toward `moving`;
  `still` for any other look;
  `CUSTOM_THEME_LOOK_PROPERTY` in `interface-settings`) to a theme file's
  `preview`, `customThemeMoves` reads it, and `ThemeSwatch` draws a play mark.
  Shaders (2026-11-15, ported from TN Code): `--theme-shader` carries a GLSL
  ES 3.00 fragment shader as a data URL (`shaderSettings`); `theme-shader.ts`
  draws it on a `canvas.theme-backdrop-shader` at the front of the art, at
  `--theme-shader-scale` of the window (never over `SHADER_MAX_PIXELS`), on
  the low-power GPU, with `u_resolution`, `u_time` and, from
  `--theme-shader-image`, `u_image`. It owns no timer: the stepper calls
  `advance`, which draws when the shader's own rate says a frame is due, so
  every rest and the battery pace apply. `docs/theme-examples/tn-code` holds
  TN Code's eighteen themes and four shaders, written by `port-themes.mjs`.
  The same file maps
  `--theme-danger/-success/-warning/-pro` onto status, diff, mode and
  `--ansi-*` tokens with fallbacks equal to `theme.css`'s values
  (`theme-look.test.ts` holds them equal). Examples: `docs/theme-examples`.
- Theme files (2026-10-02, [guide](../THEMES.md)): `applyAppearance` sets
  `data-theme` to `custom-dark`/`custom-light` for a file and adopts its CSS
  as a constructed stylesheet after every app sheet (`applyThemeFile`).
  `theme.css` keeps the whole token system in `@layer interface-palette`, so a
  file's plain `:root` rule wins; the custom palettes derive every token from
  eight `--theme-*` keys with `color-mix`, and `theme-files.test.ts` evaluates
  them (empty file and samples) against the AA bars. `theme-previews.ts`
  draws a file's swatch from the colours Rust read out of it;
  `terminal-palettes.ts` reads `--ansi-*` off the computed style. The host
  pushes `iface:appearanceChanged` when the folder changes and
  `useAppearance` re-reads. Settings (`components/SettingsAppearance.tsx`)
  leads with the files whose backdrop moves, under "Live backgrounds" with
  the one Motion menu (On / Still on battery / Off over `themeMotion`), then
  shows one tab at a time: Light, Dark and "Your themes · N" (the other
  files), opening on the tab that holds the selected theme. While Follow
  macOS is on, its Light/Dark menus sit in its row.
- Appearance › Window › See-through panes holds independent Terminals and Sidebar translucency
  sliders. A drag paints each step directly and saves once where the slider
  comes to rest (`SliderCommit`). `translucency.css` changes only background
  materials; zero preserves the settled solid look. `TerminalView` enables alpha at creation and changes
  its theme in place so slider changes retain buffers/input. Explicit ANSI
  background colors stay intact. macOS Reduce Transparency renders solid
  surfaces without changing stored values; other platforms retain a solid
  native fallback. `window-material.ts` translates settings into macOS vibrancy.
- Color themes share the dense tile structure (owner, 2026-10-01, replacing
  Porcelain's frames): each terminal pane is a flush tile in the screen colour
  (`--terminal-content-bg`) whose 26px head is a band (`--terminal-header-bg`,
  focused `--terminal-header-bg-focused`) over a `--terminal-header-border`
  hairline; 6px gutters, 7px corners, no lift. The Interface `light`/`dark`
  blocks are Porcelain and Porcelain Night (repainted near-black); the bare
  :root block is only the fallback. A screen carries its own text tiers
  (`--terminal-fg*`) and `--terminal-color-scheme`, so Porcelain's dark screens
  in light chrome stay legible. Theme blocks contain palette colors, fonts and
  those two keywords, while geometry, type sizes, motion and shadow shapes are
  shared.
  Floating menus/dialogs use root-resolved translucent fills and a shared blur;
  persistent surfaces, Bare CLI and Reduce Transparency retain solid materials.
  The sidebar keeps its independent native translucency setting. Each palette supplies
  distinct body, idle-header and focused-header colors, a neutral tile edge and
  its own focus edge and frame shadow colour.
  `TerminalView` handles `interface-theme-change` in place without recreating
  the emulator or losing scrollback. The Electron fixture compares component
  structure across all 10 palettes, both styles and two widths, and switches
  all palettes around one real terminal.
- Themes since 2026-09-28 (owner): Porcelain, Porcelain Night, Clay Light/Dark
  (the `anthropic-*` ids), Solarized Light/Dark, Catppuccin Mocha, Carbon
  (`grok-build`, the one Bare CLI palette), Navy (`hermes`) and Ice
  (`hermes-light`). Sauteed Paper and Warm Editorial folded into Clay and
  Nightshade into Carbon: Rust still parses those ids and answers with the
  replacement (`ThemePreference::canonical`), and `canonicalThemePreference`
  does the same for renderer-remembered copies. Each theme also hands xterm
  its own 16 ANSI colours (`terminal-palettes.ts`, measured against its screen
  by `terminal-palettes.test.ts`), and Settings draws every theme as a swatch
  from `theme-previews.ts`, which `theme-previews.test.ts` holds to theme.css.
- `markdown.tsx` — the transcript markdown renderer. A growing message resumes
  its parse after its last top-level blank line (`parseMarkdownIncremental`,
  proven equal to `parseMarkdown` by `markdown-incremental.test.ts`) and draws
  through memoized block views, so settled blocks skip a streamed token.
- `contrast.ts` — WCAG hex parsing, luminance, contrast ratio, 8-digit
  compositing. Test-only; see `electron/AGENTS.md`.
- `visible-polling.ts` — one read at a time for shared drafts, Away, Control,
  the agent store, the agent work view and remote access settings (only while
  that Settings section is shown);
  completion-scheduled timers pause while hidden and refresh on return.
  `shared-draft-polling.ts` paces the phone's shared-draft reads: every 2 s
  for the first reads, then doubling to 10 s while the Mac's revision stays
  the same; a change, a local save, focus or the page showing again quickens it.
- `agent-reads.ts` — which agent requests are reads (queued, but never mark
  the page busy) and the agent work view's poll, which runs only while the
  agent page is on screen and keeps the view while its revision is unchanged.
- `helpers.ts` — the three primitives shared across renderer modules: `asRecord`,
  `normalizedToken`, `clamp`.

**components/** — the conversation pane is four files. `Conversation.tsx` is
the shell: `ConversationProps` and its caller contract, the merged transcript
rows, this window's echoes (`postEcho`), the density dial's state, the drop zone,
and stable forwarders for the caller's callbacks. It draws two memoized halves:
`ConversationTranscript.tsx` (display-row folding, find and its Cmd/Ctrl+F
listener, the approval chord, the render-phase session reset and the scroll
pin/anchor/jump capsule, running-subagent and working strips, the queue slot,
the turn clock, the restore dialog) and `ConversationComposer.tsx` (the draft,
the provider-command menu and pre-launch discovery, send/queue/steer, paste,
the context picker and meter, the density button, the config pills).
`ConversationRows.tsx` holds every item renderer and its memo comparators.
`glyphs.tsx` holds `Icon` and `StateGlyph`, which the transcript and
`SubagentPanel.tsx` share; the subagent status word, sentence and glyph state
live beside them in `tool-activity.ts`.
`Composer.tsx` (the pre-launch card: launch form seeding, combined
provider+model picker, effort/access vocabularies, session scoping, the two-step
bypass confirmation, multimodel team fields, pre-launch command discovery),
`Sidebar.tsx` (project-grouped navigation with the pinned needs-attention lift),
`SettingsPage.tsx` (appearance, session defaults,
provider probes and ACP profiles, customization inventory, Threads — Rust's
`ThreadSettings`, read from `threads.view` and written whole with `setSettings` —
updater panel, shortcut reference; it fills the window under the toolbar,
mounted by `App.tsx` beside `<main>` as `.workspace-settings-overlay`, with its
own menu in the sidebar's place and one 760px column of row cards; Updates
shows one status card per updater state, "What's new" from `whats-new.ts`
(each landing's name as a sentence, New / Fixed / Looks / Faster, internal and
record landings left out) and the full `UpdateLogPanel` behind "Developer
log", both from the lazy `UpdateLogPanel` chunk),
`AgentMenu.tsx` (searchable vertical agent roster, vector characters, and the
New agent form: name, what it does and icon first, provider catalog/custom model
defaults behind Advanced; with no agent selected every Agent New opens it;
`agent-roster.ts` projects latest assigned tasks,
and `agent-appearance.ts` maps existing saved colours to stable silhouettes;
its FormSelect popovers carry a scoped class above the modal),
`AgentTask.tsx` (always-visible conversation with an optional activity/files/
provider-command inspector, derived from the live log and history only while it
is open and on screen; command discovery is not a complete tool inventory),
`IntegratedTerminalCenter.tsx` (terminal registry, saved workspaces, output
routing through `TerminalStreamRouter`, launch batches, every terminal IPC
call; a lifecycle upsert carrying
`passedOffFrom` for an id it never minted is a pane an agent's `pass_off` tool
opened, seated beside its source by `placePassedOffTerminal` and filed into
the active workspace; a launched terminal it first sees that way without
`passedOffFrom` is a voice or phone `start_pane`, seated balanced), `TerminalWorkspace.tsx`
(chrome plus the pure split-tree/equal-grid renderer), `TerminalPane.tsx` (header
drags swap whole panes; same-pane tab drags reorder tabs; the head is the
tabs, each with its status dot, and the icon actions, with no folder, state
words, Find or Files, and its ··· menu leads with Copy and Paste),
`TerminalView.tsx` (the xterm.js wrapper; nothing of it floats over the text:
the ··· menu's Copy and Paste reach the shown emulator through
`TERMINAL_VIEW_COMMAND_EVENT` (`terminal-view-attachment.ts`), and ⌘F opens a
Find row the screen gives up),
`AdversarialReviewDialog.tsx` (the objective-gated
builder/reviewer dialog the launcher's review mode became),
`AgentCanvas.tsx` (the orchestration graph, with `CollaborationPanel` stacked
under it), `AppSelect.tsx` (the app's one picker/popover plus
`positionAppSelect`), `FormSelect.tsx` (AppSelect wearing the input tokens, for
selects inside a form), `ModalBehavior.ts` (`useModalFocus`,
`modalKeyAction`, `rovingTabKeyAction`, `logicalListFocusIndex`,
`focusPastTrigger`),
`palette-match.ts` (the palette's pure file ranker and match positions), and
`status-words.ts` (the single source for the visible status word across the
sidebar and the workspace tile). The remaining components are dialogs, menus and
tiles that do what their filename says.

**Entry points** — `App()`; the mount-once effect that subscribes to
`onRegistry`/`onEvent`/`onImportProgress`/`onWorktree`; `App.tsx`'s `launch()`
(listDir probe → prompt-context reauthorization → serialize → createSession →
echo → workspace switch) and `OutboundLane.sendWithContext()` (the single
outbound-prompt lane every send goes through); `conversationItems`; `mergeTranscript`;
`buildSidebarView`/`buildUsageView`;
`IntegratedTerminalCenter`; `AgentCanvas`.

**Invariants** — the rules an editor must not break are in `electron/AGENTS.md`;
this is the mechanism behind them.

- Every outbound/config mutation is guarded by a per-(session, field) attempt
  token (`ConfigAttempts` in `session-config-tokens.ts` for settings, attempt
  generations in `OutboundLane` for sends). A late continuation compares its
  attempt before touching state; never revert a newer accepted change.
- Live history and stored history stay disjoint. `startedHere` keys never load
  history, and `props.items` is the live list only. Prose is never deduped.
- Turn checkpoints never cross an app run: the map lives only in renderer
  memory, so a reload starts it empty (legacy `iface.turnCheckpoints.*`
  localStorage keys are swept on load). A checkpoint with `fileCount === 0` is
  never recorded.
- Every localStorage boundary is tolerant: `parse*` returns a valid empty state on
  corrupt or foreign data, `write*` returns false rather than throwing.
- Workspace layout: at most `MAX_WORKSPACE_TILES` unique non-empty openKeys;
  `focusedKey` is null exactly when openKeys is empty; `maximizedKey` is null or
  equals `focusedKey`. `normalize()` applies at every public boundary.
- Row keys are namespaced `h:` / `l:` / `e:` because generated ids restart per
  list; only tool/approval/subagent ids are engine-minted and comparable across
  lists.
- A streamed token costs the transcript only its delta. `ConvItem`s are immutable
  once the reducer returns them, so `mergeTranscript`, `expandImportedRows`,
  `producedDiffs` and the tool-name classifiers cache per item or name, and
  `retainDisplayRows` hands every unchanged display row its previous object.
  Mutating an item in place would serve stale cached rows. The run, file-change
  and grid rows compare `all` field by field (`sameWorkRowProps`); a new read
  of `all` in them goes there too. The same holds outside the transcript:
  Markdown keeps parsed blocks and `subagentActivityRows` caches per child item.
- The echo drop rule is a three-way contract between `echoSeen`,
  `PendingEcho.historyKnown` and `mergeTranscript` rule 3. Changing one without
  the others silently deletes messages the user just sent.
- Approval verdicts live in the caller's `ApprovalVerdicts` map keyed by
  `approvalVerdictKey(sessionKey, approvalId)` — never component state.
- The `assertNever` arms exist so a new `EngineEvent`/`ConvItem` kind is a compile
  error rather than a silently invisible row. Keep them.
- Layout writes go through the terminal-workspace helpers; the component never
  edits the split tree inline.
- `retainUnresolvedSteerRequestId` must keep reusing the same `clientRequestId`
  until a steer definitively succeeds; `launchTerminalBatch` must keep
  distinguishing confirmed / unknown-registry / failed;
  `launchAdversarialReview` must release its reserved panes exactly once.
- Persisted-but-unused fields are frozen too: `AppPreferences.editorFontSize`/
  `editorWordWrap`/`editorMinimap` are normalized and stored with no UI.

**Gotchas**

- The suites that read renderer source with `readFileSync` or match exact
  substrings (`Conversation.queue.test.ts` matches
  `"displayRows.map(displayRowKeys)"`): `agent-canvas.test.ts`,
  `workspace-ui.test.ts`, `collaboration-draft-publish.test.ts`,
  `chrome-mode.test.ts`,
  `recap.test.ts`, `ui-components.test.ts`, `update-restart-ui.test.ts`,
  `status-waiting-allowlist.test.ts`, `prompt-drop-integration.test.ts`,
  `prompt-images-ui.test.ts`, `error-boundary.test.ts`,
  `theme-accessibility.test.ts`, `conversation.test.ts` and several component
  tests. Read the failing assertion before assuming a real regression.
- `App.tsx` exports only `App`. Pure logic it uses lives in plain modules its
  tests import directly (`history-paging.ts`, `app-session-projection.ts`,
  `outbound-lane.ts`, `session-config-tokens.ts`, `engine-choices.ts` for the
  engine table and each engine's picker choices, `subagent-detail.ts`,
  `palette-commands.ts` for the palette's shell commands), so no node test
  loads App.
- Refs published through counters: `liveKeys`/`closedKeys` are refs, and
  `liveVersion`/`configWake`/`settledTurnWake` exist solely to make a ref
  mutation visible; the outbound lane's latches (`isSending`,
  `outboundPending`) move its snapshot's `wake`. Reading one of those in a
  memo means adding its counter to the deps.
- Session panes render through `memo(SessionTile)` (`components/SessionTile.tsx`,
  with the return recap strip and its `RECAP_CSS`). `tileConversation` reads
  every per-pane value, ref-derived facts included, during App's render and
  passes it as a prop; callbacks arrive through one `sessionTileActions` object
  (`useCommittedActions`) whose members never change identity, the pane's two
  host reads (`loadProviderCommands`, `pathForFile`) included, so App stays the
  one `window.iface` caller. A tile that read a ref itself would render stale. `app-memo-props.test.ts` pins this boundary
  and the memoized Sidebar, AgentCanvas and IntegratedTerminalCenter props.
- Sidebar session rows render through `memo(SidebarSessionRow)`, callbacks
  through one committed `rowActions` object. The listing rebuilds row objects
  on every registry change, so the memo compares the fields a row draws
  (`sameDrawnRow`); a row that starts reading another `SidebarRow` field must
  add it there. The glass header clipping rebinds only when `headerClipKey`
  (the list's structure) changes; its ResizeObserver also watches the list's
  own children, so anything that grows moves the clips with it.
  `sidebar-row-fixture.tsx` counts row renders for pushes, peeks and renames
  (`INTERFACE_SHELL_RENDERS_ONLY=1 npm run test:renderer` runs its lane alone).
- The command palette's highlight is a command id (`paletteHighlightIndex`),
  not a row number: a registry push while it is open keeps the chosen command
  highlighted, and Enter runs it. ⌘P indexes its file list once per load
  (`indexPaletteFiles`); a keystroke only scores (`rankPaletteIndex`).
- The partial dependency arrays that are deliberate: the settled-turn re-key
  effect (`use-turn-restore.ts`) depends on `settledTurnWake` alone, and `composerDefaults` lists
  `state.composerOpen` though the body never reads it — it is the invalidation
  trigger for a non-reactive `readStored`.
- The Sidebar force-expand effect relies on the component staying mounted: App
  hides the sidebar with CSS, never by unmounting, or every show would clear the
  user's persisted collapses.
- `queueErrors` is delivery-**blocking** and `queueNotices` is advisory. Routing a
  rejected enqueue through `queueErrors` strands an otherwise healthy queue.
- **Pasted images in agent panes are the CLI's own paste.** `insertPaths` in
  `IntegratedTerminalCenter.tsx` sends Ctrl+V (`terminalImagePasteInput`) to a
  claude/codex/grok/qwen/Antigravity pane and never writes a file: each CLI reads the
  clipboard image itself and draws its own placeholder (`[Image #1]`). Shell
  panes and dropped files take the write-file route, and the resulting path is
  **pasted** through xterm (`handle.paste`), one paste per path, never typed:
  Claude Code and Codex only recognise a bracketed paste that is a single image
  path, and then draw the same `[Image #1]` themselves. Nothing is painted over
  the terminal cells; a shell pane shows the path, as Terminal.app would.
  xterm brackets a paste only if *that instance* saw the program's `CSI ? 2004 h`,
  which the CLIs send once at startup, so a remounted claude/codex pane (workspace
  switch, split, Clear) re-asserts it before its replay
  (`terminalReplayAssertsBracketedPaste`); otherwise every later paste arrives as
  typed text and the path stays a path.
- **xterm 6 hides every decoration while the alternate buffer is active**, and
  Claude Code runs there. A new decoration that must show in an agent pane has
  to set its own `display` from the marker row and viewport.
- `Item()` in `ConversationRows.tsx` must stay a plain function — the tests
  call it directly — which is why the one stateful transcript item is split out.
- The session-switch reset happens **during render** and the scroll pin is a
  `useLayoutEffect` latch, because a passive-effect reset would land a frame
  after the pin. Do not move either into `useEffect`. Both live in
  `ConversationTranscript`; the composer and the shell re-run the pin from
  their own layout effects (`controls.pin`), because a commit only they made
  can resize the scroller.
- The conversation pane's two halves are memoized, and the shell keeps their
  props equal across a keystroke and a streamed token: callbacks go through
  `usePaneCallbacks` forwarders to the props it last committed (`latest`), and
  fresh caller objects (approval Sets, capabilities, context reports) are kept
  by content. A send reads `latest()` once and keeps those callbacks for its
  answer. `conversation-render-fixture.tsx` (the shell-renders lane) counts a
  keystroke drawing no transcript row and a streamed token drawing no
  composer; the node suite's "pressed" harness (`drawPane` in
  `conversation.test.ts`) renders both halves on its probe.
- `TerminalWorkspace` accepts a `switcher` prop but production never passes it:
  `IntegratedTerminalCenter` publishes it via `onSwitcherChange` and the Sidebar
  renders it. Only a test exercises that branch — the current example of an
  export whose only caller is its own test.
- `terminalSurfaceVisibleRef.current` is assigned during render, and several
  effects read the ref while listing the individual booleans as dependencies.
  Changing one without the other silently breaks focus and unread behaviour.
- `setTerminalWorkspaces` writes are debounced 400 ms; `workspacesRef.current` is
  updated synchronously inside the state updater because the timer reads the ref.
- AgentCanvas pointer gestures read `layoutRef`/`zoomRef`, never render closures,
  because a snapshot can land mid-drag. `BROWSE_ROW = "\0browse"` and
  `PROVIDER_DEFAULT_ROW = ""` are sentinels: a NUL cannot appear in a path or
  model id, and the empty string means "send no field at all".
- `coordinatedTaskActive` treats an unnamed task state as still active on
  purpose, and `canvasCoordinators` (the one lease rule behind
  `canvasCoordinator` and the canvas's `coordinatorById`) reads only *active*
  worktree leases — relaxing either would strand or mislabel a live lease.
- `AppSelect` renders every option and hides non-matches with the `hidden`
  attribute rather than filtering, so indices stay stable for
  `aria-activedescendant`.
- Several components deliberately return the same markup when `document ===
  undefined` instead of portalling; that is what lets the dependency-free node
  tests inspect their ARIA contract.
- `prelaunchCommandCache`/`prelaunchCommandAnswers` are module-level Maps never
  evicted for the window's life. Deliberate.

The host-projected terminal roster includes OMP (`omp`), drawn with a pi
monogram. The launcher, pane menus, and saved profile memory retain that id.
A reloaded OMP pane launches OMP in its saved folder without selecting a prior
conversation; OMP's own resume command controls that choice.

Terminal pane headers render `TerminalFastModeToggle` beside the ellipsis only
when the host supplies live Fast metadata. The callback sends the native key
over the existing generation-fenced terminal data plane. Pending clicks stay
off/on as last reported until a CLI state update arrives; a timeout explains
that the change was not observed. Model changes, unsupported metadata, process
exit and generation changes discard the old control/pending presentation.

`TerminalStartupDownloadNotice` sits over the bottom-right of a running pane's
body while the host reports `startupDownload`: the files its program is still
waiting for iCloud Drive to download, with a tooltip on Keep Downloaded. It
overlays rather than resizes the terminal, since a resize would signal the
program, and disappears with the host's next summary once the files are local.

`terminal-cursor.ts` translates plain clicks in idle Codex and Claude native
prompt bands into Ctrl-B/F cursor movement. Multiline clicks observe the CLI's
cursor after each row crossing, preserving soft wraps, newlines and graphemes.
Output clicks, selection drags, modifiers, scrollback, mouse-tracking programs,
disabled input and busy agents retain their native behavior. New typing, paste,
focus loss, resize or changed prompt text cancels a pending placement. The
terminal interaction DOM lane exercises this against real xterm instances.

`terminal-caret.ts` draws the reference blue bar in a focused pane, two CSS
pixels wide and 85% of the row tall, centred and snapped to device pixels.
`--terminal-caret-color` keeps it blue across palettes, with a deeper shade on
light backgrounds. The bar is the configured default (`cursorStyle` in `TerminalView`),
because Claude never sends DECSCUSR and Codex sends `CSI 0 SP q`; a program that
asks for a block or underline keeps it. It tracks style and DECTCEM through parser
hooks, places the caret on `onRender`, and makes only the theme's cursor colour
transparent while it draws, which the WebGL glyph atlas does not key on.
`TerminalView` routes palette changes through it. Block, underline, unfocused
and hidden cursors stay xterm's.
`INTERFACE_TERMINAL_CARET_ONLY=1 npm run test:renderer` checks real xterm at
standard and Retina density in separate Electron processes and captures both
light and dark reference prompts.

Codex (0.157+) takes the pointer in its alternate screen and draws its own
selection, so a drag selects nothing in xterm. When xterm has no selection, the
program has mouse tracking on, and it pushed kitty keyboard flags with the
disambiguate bit, ⌘C sends `CSI 99;9u` (Super+C), which Codex copies to the
system clipboard itself (`terminalCopyGoesToProgram` in `terminal-keys.ts`).
xterm 6 ignores kitty requests; `terminal-kitty-keyboard.ts` follows the
per-screen flag stacks through parser hooks, and `TerminalReplayModes` restores
them after a truncated remount. vim, htop and tmux capture the mouse without
kitty flags and keep ordinary ⌘C. The terminal interaction DOM lane checks this
(`programCopy`).

The OpenCode (`opencode`) terminal profile is also host-projected, with its own
terminal glyph. Saved workspaces retain the host-confirmed conversation ID;
reopening requests that exact chat in the saved folder. Id-less legacy records
retain resume intent so the host can open OpenCode's native session picker.

Muse Code (`muse`) is host-projected with a bold M mark and a neutral accent.
Saved workspaces and profile memory retain it; reopening starts Muse Code in the
saved folder. The pane menu's Pass off list leaves it out because its TUI takes
no initial prompt.

Kimi Code (`kimi`) is host-projected with Lobe Icons' Kimi mark (ink K, Kimi-blue
dot) and a Kimi-blue accent. Saved workspaces retain it and reopen the
`kimi-<uuid>` session its host plugin reported (protocol v55); one without that
proof reopens fresh, and the renderer never mints a Kimi id. Pass off offers it:
the host types the brief into its TUI once the editor is ready. The phone draws
the same mark as two template layers (`ProviderKimi` in ink, `ProviderKimiDot`
in Kimi blue).

The pane `+` (`TerminalPaneNewMenu`) exposes the [workspace tool menu](../WORKSPACE_TOOLS.md):
existing harness launches, native utilities, Split Down, a shell tab in the
source pane, and a fresh workspace launcher. Printed chords are only the ones
`terminal-keys.ts` binds. Browser adds a Browser tab to the source pane
(`openBrowserTabInPane`; the terminal center owns its pages, as it owns Files); App owns
the `openWorkspaceTool` boundary for Finder, Simulator and the Browser's "Open
in default browser".

### Pane motion

`components/TerminalPaneMotion.tsx` measures pane rectangles before React's DOM
mutation and animates the existing tiles after it: 260ms resize/reflow, 200ms
swaps, 140ms entry and 120ms exit. `terminal-pane-motion.ts` excludes focus,
output and divider ratios from the animation key. Terminal bodies retain their
destination dimensions throughout motion; native text is never scaled and the
emulator only fits the final geometry. Interrupted changes start at the current
visible rectangle. Reduced Motion, window resize, workspace scrolling, hiding
and unmount cancel and release all animation styles. Exit visuals are inert,
identity-free DOM/canvas copies removed after the fade; they hold no terminal
handles. Both automatic and custom layouts use stable mount portals.

The sidebar hide/show is a `.app-grid` column transition (the column collapses
to `0px`, never drops, so it can interpolate). App's layout effect calls
`holdTerminalsWhileSliding`, which marks the grid `data-layout-sliding` so
TerminalView's ResizeObserver skips fits, then refits every emulator once via
the pane layout event when the transition finishes or is interrupted.
`INTERFACE_WORKSPACE_HIERARCHY_ONLY=1` runs one motion-on pass over it, which
also checks that a project's rows fold shut and open.

A divider drag marks its split `data-layout-dragging` (`throttleTerminalFits`)
from pointer-down to release: TerminalView's ResizeObserver then refits on a
trailing 50ms throttle (`TerminalFitThrottle`) instead of every frame, and the
release refits every emulator in the split once, exactly, through the pane
layout event; the PTY resize follows that final fit. Keyboard and double-click
ratio changes still fit at once.

### Panel and surface motion

`presence.ts` keeps a toggled surface mounted through its exit: `usePresence`
reports `still` (including the first render, so a remembered panel appears in
place on launch), `entering` or `leaving`; `presenceClass` maps that to
`is-entering` / `is-leaving`, and the stylesheet owns the motion. A leaving
surface is inert and unmounts on its own `animationend` (a 480ms fallback, or at
once under Reduced Motion). `RetainedPresence` redraws the last open rendering
while leaving, for surfaces whose data clears on close (the subagent detail,
the sidebar's folded rows). Users: the agent overview and output preview
columns (`TerminalWorkspace`: each slides in from the right edge via a negative
margin while `holdTerminalsDuring` holds the panes; a sheet glides where they
float; the preview's presence lives in `IntegratedTerminalCenter` and a
workspace switch shows or hides it in place via `usePresence`'s `instant`), the narrow sidebar drawer and the subagent detail pane
(`App`), and project row folding (`Sidebar`'s `FoldingRows`, height via
`interpolate-size`). Menus, dialogs, panel disclosures and small sidebar reveals
arrive through zero-specificity `:where()` lists in `theme.css` ("Floating
surfaces arrive"), with a matching Reduced Motion list that `presence.test.ts`
keeps in step. Transcript disclosures, the Settings card and terminal surfaces
deliberately do not animate. `INTERFACE_AGENT_OVERVIEW_ONLY=1` has motion-on
passes for the docked slide and the floating sheet.

`INTERFACE_PANE_MOTION_ONLY=1 npm run test:renderer` exercises real xterm buffers,
focus/input during motion, interrupted restore, add/close, mount identity and
no per-frame refits in both arrangements and wide/narrow normal/reduced motion,
plus a scripted divider drag (throttled fits, exact final geometry and PTY size).

### Looping motion and battery

Any running loop makes Chromium draw the whole window at the display's refresh
rate (120 Hz on ProMotion), which is most of Interface's idle power. Every
`infinite` animation therefore steps at about 12 frames a second (`steps()` per
keyframe segment) and reads `animation-play-state: var(--ambient-play-state)`.
`window-activity.ts`, installed by `main.tsx`, marks the root
`data-window-active="false"` while Interface is not the app in front
(`document.hasFocus()` and visibility, re-read a task after `blur` so a Browser
pane's webview still counts), and `theme.css` turns that into `paused`. Only
loops read it: one-shot motion keeps running because presence, notices and pane
motion wait on `animationend`. The ready update control animates three times
and replays on return. `window-activity.test.ts` scans every sheet and CSS
template for `infinite` rules and holds each to both. Settled look: "Looping
motion budget".

`steps()` does not lower the frame rate: Chromium still draws every refresh
while a stepped loop runs (a trace on 2026-10-27 showed ~54 frames a second
with the sidebar ticker against ~21 without). `loop-rest.ts`, installed by
`main.tsx`, therefore listens for `animationstart` and, once an `infinite` CSS
animation has run about three seconds of its own time, pauses it on the first
keyframe of its current cycle through the Web Animations API. A paused CSS
animation then ignores `animation-play-state`, so it stays still on return to
the front; a remounted element or a renamed animation is a new animation and
runs again. Time paused behind other apps does not count, and the Effort
popover's hover-gated loops are skipped. Driving loops from a 12 fps JS clock
instead was measured and saved only about a fifth of their cost, because each
clock tick is a main-thread frame.

Loops inside `.sidebar` are not rested (owner, 2026-10-28): the working ticker,
the folder's working dot and the Threads inbox's marks are how you see an agent
is still working, so they loop as their rules say and rest only through
`--ambient-play-state` and the stowed-sidebar rule. A timer stepping them 12
times a second was tried and measured: about 70 → 38 frames a second, but no
CPU saved (13.7–18.4% against 14.3–15.9% with a 10 Hz pane), since Chromium's
own opacity loop draws on the compositor while each timer step is a
main-thread frame.

Terminal output follows the same signal. A shown window keeps painting behind
other apps (`main/window-visibility.ts`), and each chunk a working agent's TUI
writes (a spinner frame, a token count) made Chromium draw the window again:
about two frames per chunk, per pane. `terminal-output-pacing.ts` holds
`TerminalView` writes while the root's `windowActive` is "false" and applies
every pane's together on one 250 ms timer, flushing a stream early at 32 writes
or 64 KiB so it stays inside the transport's credit window. In front, writes go
straight to xterm after anything still held, and returning to the front flushes
at once. Held writes count as pending, so `controlScreen()` and the router's
screen evidence never report a screen ahead of its bytes.
