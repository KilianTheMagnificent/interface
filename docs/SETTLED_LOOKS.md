# Settled looks

Visual decisions the owner has made and does not want re-litigated by a
tidy-up. Each entry names what the look is, where it came from, and the
markers that carry it in the source. `cargo xtask closeout` reads this file:
a push that changes a line containing one of an entry's markers is refused
unless a commit in that push carries the trailer

    Settled-look: <entry name> — approved by the owner <date>

which you may only add after the owner has approved the change in your
session. When you add it, update the entry here in the same commit.

Why this file exists: on 2026-09-05 the owner matched the selected terminal
pane's chrome to a reference (00:15, 00:42). Fifteen hours later a commit
titled "quieter pane chrome", bundled with three unrelated fixes, removed it
and rewrote its pin test; the gate was green and closeout pushed. Pin tests
protect implementation, not intent — whoever changes the look rewrites them.
This ledger and the closeout guard make a reversal something you have to say
out loud.

Format rules the guard depends on: one `## ` heading per entry; a `- Markers:`
line listing backticked strings that appear verbatim in the source lines that
carry the look (a selector, a token name, a class name, an aria-label).

## Porcelain frames and palettes

- Revised: 2026-11-15, owner. Terminal text defaults to 11.5px (Settings
  offers 10–26), not the 12px recorded on 2026-10-01 below; see "Terminal text
  is the owner's Rex type: MonoLisa Bold, 11.5px".

- Revised: 2026-10-02, owner approved theme files. The `custom-dark` and
  `custom-light` palettes a theme file paints over keep this look by default:
  every pane is a frame holding an inset screen with its own
  `--terminal-color-scheme`, and the selected sidebar row takes
  `--sidebar-selected-fill`. No built-in palette changed; a theme file may restyle it.
- Revised: 2026-10-01 (second pass), owner sent a closer BridgeMind screenshot and asked to move "a little closer in some ways". The selected workspace's parent project is tinted again (dark
  `--sidebar-parent-fill` `#ffffff0d`, light `#0000000a`), reversing the
  2026-09-28 clear.

- Revised: 2026-10-01, owner asked for the UI to look much more like a
  BridgeMind screenshot and chose "A · Dense" (with B's live cards and
  sidebar pips) from the canvas https://claude.ai/artifact/DD5wZVCHPHqqXPk5h6ufu3, as the only layout. The frame is
  gone: every pane is a flush tile (7px corners, 6px gutters) whose 26px head
  is a band over the screen with a hairline under it; nothing lifts. The
  `dark` palette (Porcelain Night) is repainted near-black: canvas `#0e0e0d`,
  screens `#0a0a09`, heads `#151514` / focused `#1e1d1b`, neutral edge
  `#262626`. Porcelain light keeps its colours and white head bands over dark
  screens. `--terminal-frame-inset` and the frame shadow tokens are removed.
  Terminal text defaults to 12px (Settings still offers 12–26).

- Revised: 2026-09-28, owner approved the UI review canvas (https://claude.ai/artifact/J74d3QPF1VM3pCj9GAyWoF).
  Every palette now sets its own frame tone, a softer neutral focus edge
  (`--terminal-focus-edge`) and frame shadow colour, so the frame reads in Clay,
  Solarized, Ice and the dark palettes too. A pane waiting on the human lights
  an amber strip in the frame's bottom inset under the screen and says "Needs
  attention" in its head. Instrument Sans is the interface face in every
  palette. The Porcelain palettes clear the selected workspace's parent tint in
  the sidebar (`--sidebar-parent-fill`).

- Settled: 2026-09-26. The owner asked for a visual overhaul that stops the app
  looking like BridgeMind before a public launch, chose Porcelain ("smooth"
  page of the design canvas, https://claude.ai/artifact/QEj5dDHFcnQPe3kyGE4Vfh), and
  approved it as the new default look for every theme.
- Look: every terminal pane is a FRAME in the head colour holding an inset
  SCREEN (4px on three sides, 14px frame corners, 10px screen corners, 38px
  head inside the frame). Resting frames are flat and edged by a neutral
  hairline; the focused frame steps up one shade and lifts on one soft shadow.
  No gradients, no stacked highlights, no grain. Working and waiting status
  lights carry a faint halo in their own hue. The Interface family itself is
  Porcelain: `light` is pale porcelain chrome (#f0f0ee) with white frames around
  dark screens (#18191b), and `dark` is Porcelain Night, warm graphite frames
  (#232220) around near-black screens (#0d0d0c). Both use Instrument Sans for
  the interface. A screen carries its own text tiers and colour scheme, so
  dark screens inside light chrome stay legible. The sidebar's selected row is
  a raised surface with a hairline in both Porcelain palettes.
- Scope: other palettes keep their colours and adopt the frame structure. The
  permission row (Allow / Always / Deny under a screen) from the mock is not
  built: a CLI pane's exact prompt is not known to the app, and typing a key
  blind would be an ambiguous provider effect.
- Carried by: `theme.css` (the `light` and `dark` blocks, their pre-mount copy,
  the frame tokens on :root, the Instrument Sans faces), `terminal.css`
  (`.terminal-pane`, `.terminal-pane-head`, `.terminal-pane-body`, status
  lights), `components/Sidebar.tsx`, `shared/appearance.ts` labels.
- Markers: `--terminal-color-scheme`, `Instrument Sans`, `--sidebar-selected-fill`, `--tile-screen-radius: 0px`
- Verification: `theme-accessibility.test.ts` (screen text on the screen's own
  colour, palette-only blocks, the nested screen radius),
  `TerminalPrimitives.test.ts` pins, `chrome-mode.test.ts`,
  `scripts/smoke-packaged.mjs` geometry (14px gutters and corners, 38px heads,
  one focused lift). Checked in the hands-free app in both palettes.

## Terminal text is the owner's Rex type: MonoLisa Bold, 11.5px

- Settled: 2026-11-15, owner asked for the terminal's font, font settings and
  dimensions to be "what I have in my REX CLI right now". Rex's settings were
  MonoLisa, Bold, 11.5 pt, and its panes measured 15×32 device pixels a cell
  on the Retina display (7.5×16px).
- Look: terminal panes draw MonoLisa Bold at 11.5px by default, on rows the
  font's own height (16px; the 1.2× rows of 2026-10-02 are gone), and a
  cell's width rounds to the nearest device pixel as Ghostty's does (7.5px on
  Retina, where xterm alone floors it to 7). A pane holds the columns and rows
  a Rex pane of its size holds. JetBrainsMono Nerd Font sits behind MonoLisa
  for the powerline and icon glyphs it lacks, and is the face on a Mac
  without MonoLisa (the commercial files are not bundled). Bold has no
  heavier face, so SGR bold text is the same weight as plain text, as in
  Rex. Settings offers 10–14px in half steps and 15–26px; a size the owner
  saved earlier is kept.
- Scope: text inside terminal panes. Pane heads, gutters, corners, the text's
  inset (12px in, 10px under the head) and the blue bar caret are unchanged;
  Rex's block cursor was not copied. The card previews and overview tails
  that use `--font-terminal` take the face at the chrome's regular weight.
- Carried by: `theme.css` (`--font-terminal`, `--terminal-text-weight`),
  `terminal.css` (`--terminal-reading-size`), `components/TerminalView.tsx`
  (`TERMINAL_LINE_HEIGHT`, `terminalFont`), `terminal-cell.ts`, and the
  `WorkspacePresentationV2` default in `interface-contracts`.
- Markers: `--font-terminal: "MonoLisa"`, `--terminal-text-weight: 700`, `TERMINAL_LINE_HEIGHT = 1;`, `cellSnapLetterSpacing(terminalAdvance(`, `terminal_font_half_px: 23`
- Verification: `TerminalPrimitives.test.ts` ("uses the owner's terminal
  face, MonoLisa Bold, instead of the app chrome face"), `terminal-cell.test.ts`
  ("matches the owner's Rex pane: MonoLisa 11.5px is 15 device pixels wide on
  Retina"); in the hands-free app a shell pane measured 15×32 device pixels a
  cell, the figures `rex block call size` reported for the owner's panes.

## Blue terminal typing caret

- Settled: 2026-09-24, owner supplied the blue insertion-caret screenshot and
  requested the same look.
- Look: a steady blue vertical bar, two CSS pixels wide and 85% of a terminal
  row tall, centred on the text and snapped to device pixels. It keeps its
  visible width on Retina displays. The blue deepens on light backgrounds.
- Scope: the default and program-requested bar cursor in focused terminal
  panes. Explicit block/underline styles, hidden cursors and unfocused outlines
  retain their native terminal behavior.
- Carried by: `terminal-caret.ts`, `terminal.css`, `theme.css`.
- Markers: `TERMINAL_CARET_HEIGHT`, `TERMINAL_CARET_WIDTH`, `--terminal-caret-color`
- Verification: existing caret geometry tests and the real xterm fixture cover
  placement, style/visibility controls and focus; renderer captures cover both
  pixel densities and light/dark palettes.

## Light screens hold terminal text to 7:1

- Settled: 2026-10-23. On Clay Light the owner could not read Claude's status
  line (the model name, effort level and token count of their own
  `statusline.sh`) or Claude's yellow "auto mode on". Shown 4.5:1 and 7:1
  side by side, the owner chose 7:1.
- Look: on a light terminal screen xterm darkens any text under 7:1 against
  its cell's background as it paints; SGR-dim text is held to half that. A
  CLI's fixed colours made for dark terminals turn their deep shade (the
  orange reads as rust, the green as forest) and faint tiers stay lighter than
  body text. A theme switch repaints text already on screen.
- Scope: screens whose `--terminal-content-bg` is light (Clay Light, Solarized
  Light, Ice and light theme files). Dark screens, including Porcelain's dark
  screens in light chrome, keep xterm's default of no floor. Terminal glass
  turns the floor off, because xterm measures a transparent background as black.
- Carried by: `terminal-palettes.ts` (`terminalMinimumContrast`),
  `components/TerminalView.tsx` (the option and its repaint).
- Markers: `LIGHT_SCREEN_MINIMUM_CONTRAST`
- Verification: `terminal-palettes.test.ts` ("holds text to 7:1 on light
  screens and leaves dark screens alone"); a real xterm render of the status
  line at no floor, 4.5:1 and 7:1 was shown to the owner.

## Workspace launcher follows the provider-grid reference

- Revised: 2026-10-14 (review E), owner approved ("Approve all four"): an
  agent tile with a plan figure says its headroom under its name in the
  note's dim tier ("38% left", "~81% left" once the read is stale;
  `.terminal-launcher-headroom`), the window and reset in its tooltip and
  "<name>, 38% left" in its accessible name. A provider that reported
  nothing says nothing.
- Revised: 2026-10-14, owner approved review C4's folded launcher. Claude
  Code, Codex and Terminal are always tiles, and a missing one keeps its
  dashed "Needs setup" tile. Any other agent that is not installed leaves the
  grid for a closed "More agents · N not installed" below it, outside the
  radio group: each entry shows its faded mark and install hint (commands set
  as code) in one dashed list, with a Refresh under it that rebuilds the
  catalog. The grid holds what can actually run.
- Revised: 2026-10-01, owner chose "Refine 1 · Polish" over new layouts
  (https://claude.ai/artifact/YYvYm6rjcC8cP5s44pd52Q): same sections, order and
  four-column grid. No intro line under the heading. An unavailable agent is a
  dashed outline with a faded mark and a visible "Needs setup" (the reason
  stays in its tooltip). The eight counts are one joined control on the
  surface, the chosen one keeping its accent edge, and the count says
  "terminal(s)" like the button. Isolation counts the terminals; the folder
  shows its name, then the path as a terminal prints it (`~/…`). Will launch
  ends "in <folder>". Review is a quiet "Builder and reviewer instead" button,
  and ⌘↵ launches, with its hint beside the button.
- Revised: 2026-09-28, owner approved the UI review. Provider tiles, count
  buttons and the folder field are surfaces (`--surface` on a hairline) rather
  than grey wells, and Will launch is one line: the agent's mark, its name and
  × N. Over the launcher an error or notice takes room above the heading
  instead of covering it.

- Settled: 2026-09-10, owner supplied the workspace launcher screenshot.
- Look: a flat workspace surface, compact heading, uppercase section labels,
  neutral provider tiles in three columns, provider logos and an accent outline
  with a filled check for selection. Six count buttons stay in one row. Local
  checkout isolation and the working directory precede a numbered Will launch
  lineup. Narrow surfaces reduce the provider and preview grids to two or one
  column; the selected theme and Bare CLI geometry remain respected.
- Scope: the Rust-owned provider roster and existing batch launch remain
  authoritative. Presets and worktree creation are absent until supported by
  the launcher; Local checkout describes the existing launch destination.
- Carried by: `components/TerminalLauncher.tsx`, `terminal.css`.
- Markers: `.terminal-launcher-agent-grid`, `.terminal-launcher-lineup`, `terminal-launcher-isolation`
- Verification: `workspace-launcher-fixture.tsx` checks the rendered lineup,
  selection, folder handoff, launch locking, responsive layout, and the folded
  More (closed, its install hint, Refresh).
- Revised: 2026-09-10. The owner approved all reference-audit changes for Dev review. The launcher now offers one through eight sessions; count buttons wrap to two rows on narrow surfaces. The provider selection and launch contract remain unchanged.
- Revised: 2026-09-12, owner approved the aesthetic review. Wide launchers (900px and
  up) show provider tiles in four columns so Terminal no longer sits alone on a
  third row; narrower surfaces keep three, two and one. Isolation is a plain
  line of text (Shared folder, with its explanation) until a second isolation
  mode exists, when the selector returns.

## Selected terminal pane lifts its head

- Extended: 2026-11-15, owner. Against the moving backdrops, "particularly
  the darkly colored ones", the selected pane was hard to tell from the
  background; shown a brighter ring instead, the owner asked: "Do it the way
  Rex does it". Rex draws no ring: it keeps the active pane at full strength
  and fades the others (an inactive pane's text measured at 71% of the active
  pane's in the owner's Rex). So in a theme file that draws a backdrop, every
  pane but the selected one fades what is in its head and its screen to 71%,
  crossfading as the selection moves; a pane waiting on the human does not
  fade. Frames, glass, edges and the selected head's step are unchanged, and
  built-in palettes and theme files without a backdrop do not fade
  (`theme-look.css`; `theme-look.test.ts`, "fades every pane but the selected
  one"). Seen in the hands-free app over Orbit, Glyph and Paper Tide with four
  panes: the fade follows the selection, a waiting pane stays whole, and a
  built-in theme does not fade.
- Extended: 2026-11-15, owner approved looks ("Approve both"). In a theme
  file that draws a backdrop, pane heads are glass over it: the idle head is
  `--terminal-header-bg` and the focused head `--terminal-header-bg-focused`,
  each at the look's panel fill, so the focused band still steps up from the
  idle one (`theme-look.css`, scoped to `data-theme-look="backdrop"`). Under
  Reduce transparency they are solid again. Built-in palettes and theme files
  without a backdrop are unchanged.
- Extended: 2026-10-23, owner approved the file viewer restyle. The Output
  preview's head is the same band (`--terminal-header-bg` over a
  `--terminal-header-border` hairline, `--tile-head-h` tall) so it lines up
  with the pane heads beside it; its open files are the tabs in that band
  (`OutputPreviewPanel.css` `.output-preview-head`). The panes are unchanged.
- Revised: 2026-10-02, owner approved theme files. The `custom-dark` and
  `custom-light` palettes a theme file paints over keep this look by default:
  the focused head is mixed one step lighter than the idle head
  (`theme-files.test.ts` checks it). No built-in palette changed; a theme file may restyle it.
- Revised: 2026-10-02, owner chose "D · Reference-matched" on the roomier
  canvas (https://claude.ai/artifact/KqGMLcj3b1xoGwgLGdKsxS) after saying the UI had become "a tad compact". The head band
  is 32px with 13px titles and 24px action targets; its colours and the
  focused step are unchanged.

- Revised: 2026-10-01, owner chose the dense tiles (https://claude.ai/artifact/DD5wZVCHPHqqXPk5h6ufu3). The head is a
  band again (`--terminal-header-bg` over a `--terminal-header-border`
  hairline), and the FOCUSED pane's band steps up to
  `--terminal-header-bg-focused` (dark `#1e1d1b` over `#2a2927`). No lift and
  no frame shadow.

- Revised: 2026-09-28, owner approved the UI review. Clay Light and Dark,
  Solarized Light and Dark and Ice carry retuned frame shades (idle and focused
  `--terminal-header-bg` a clear step from the screen) and screen hairlines.
  The Sauteed Paper, Warm Editorial and Nightshade blocks are gone. Porcelain's
  values are unchanged.

- Revised: 2026-09-26, owner approved Porcelain (see "Porcelain frames and
  palettes"). The head is no longer a band: it is part of the pane's frame
  (`--terminal-header-bg`), and the FOCUSED frame steps up to
  `--terminal-header-bg-focused` and lifts on `--terminal-frame-shadow-focused`.
  `--terminal-header-border` now edges the screen. The sampled Dark values
  below are history; Porcelain Night replaced them.

- Settled: 2026-09-05, owner, matched to the reference Codex pane.
- Look: every pane's head is a band one step above the body (dark themes
  `#131313` over a `#1c1c1c` hairline on the `#060606` body); the FOCUSED
  pane's steps up again (`#232323` over `#303030`). Measured from the
  reference on 2026-09-05 (22:40): idle heads banded, not blended.
- Carried by: `electron/src/renderer/terminal.css`
  `.terminal-pane.is-focused .terminal-pane-head`; `theme.css` tokens
  `--terminal-header-bg-focused` / `--terminal-header-border-focused`.
- Markers: `terminal-header-bg`, `terminal-header-border`, `.terminal-pane.is-focused .terminal-pane-head`
- Extended: 2026-09-11. The owner requested the regular Interface session view
  as an iPhone app and authorized implementation. The phone's selected session
  header reuses the existing focused header tokens and the shared dark theme;
  desktop token definitions, selectors and pane appearance are unchanged.
  Phone-specific spacing and touch controls live in `electron/src/remote/remote.css`.
- History: landed `1b8f41b`, `7fdd780`; lost `a5752b5`; restored `f549185`; idle band restored 2026-09-05 22:40.
- Extended: 2026-09-07, owner requested optional terminal translucency. At 0%
  the sampled colors remain exact; glass settings keep distinct idle and
  focused header bands above the body.
- Extended: 2026-09-07, owner requested the same design across themes. Every
  palette now has separate body, idle and focused header shades; the sampled
  Dark values stay unchanged.
- Extended: 2026-09-08, owner requested a Clean appearance option for the same
  CLI panes. Terminal preserves these exact bands; Clean uses a unified body/head
  surface with the theme's raised focused head.
- Corrected: 2026-09-08, owner requested the previous grey selected head in
  Clean too. Both appearances inherit the focused header and border tokens
  (`#232323` over `#303030` in Dark); selection never uses the purple hover tint.

- Retired: 2026-09-14, the Clean-specific head treatment is archived with Clean
  GUI. Native selected-pane head colors and geometry remain unchanged.
- Extended: 2026-09-17. The owner added Anthropic Light and Anthropic Dark and
  asked to land them on dev for review. Each palette has its own body, idle-head
  and focused-head shades and a neutral tile edge. Dark's values are unchanged.

## Sidebar: one header row, single-line rows, soft count chips

- Revised: 2026-11-16, owner. Threads rows are 52px with a 9px corner, led by
  a 28px tile, and the slot is 11px; see "Threads: the TN Code look". The
  other surfaces' rows are unchanged.

- Revised: 2026-11-15 (UI redesign, footer), owner approved the before/after
  mockups ("Go ahead and apply all of those changes"). The footer is one quiet
  34px row in every surface: "Settings" on the left in the body tier, no gear,
  no version (Settings shows it) and no ⌘, chip (it stays in the tooltip).
  While providers need setup, an amber (`--status-waiting`) 6px dot and
  "N providers need setup" sit on the right of the same row as a second
  button that opens Settings › Models & providers; the rest of the row opens
  Settings. The red banner under Settings is gone. The row's hover and
  selected fills reach under the notice; a narrow row says "N need setup"
  (Bare CLI sooner), and below 180px the dot alone. With the sidebar hidden,
  the same amber dot sits on the sidebar toggle. Carried by `Sidebar.tsx`
  (`.sidebar-footer-row`, `.sidebar-footer-alert`), `App.tsx`
  (`providerAlert`, `.sidebar-toggle-setup-dot`); verified by
  `Sidebar.test.ts` ("puts Settings and the setup notice in one quiet footer
  row") and the `sidebar-reference` DOM lane in both themes, both chromes,
  288 and 240px. Every other look below stands.

- Revised: 2026-10-23 (the Claude-app look), owner approved ("This is good.
  Get it on dev"). On the Threads surface only: two-line rows are 54px, 2px
  apart, on the theme's own hover and selection fills. The letter tile and
  every hued fill go; the project's hue survives as an 8px square before line
  2's words (a hollow ring for No project, half strength on a shelf). Line 2
  is project · branch · the pull request or diff, or the live step or the
  ask, all in the dim tier, and the provider's mark still ends it. A working
  row's slot is the Pair at 13px and its m:ss clock; the unread dot is the
  done blue; Needs you stays the one filled amber pill and Failed an outline.
  Section captions are sentence case with the count at the right; the
  shelves are slim folds under one hairline. The header band is gone. Code
  and Agent keep every look below. See
  [Threads: the Claude-app look](#threads-the-claude-app-look).
- Revised: 2026-10-22, owner approved for battery: the six-dot working ticker
  keeps its 1-second staggered fade but steps at about 12 frames a second, and
  rests while Interface is not the app in front. See
  [Looping motion budget](#looping-motion-budget).
- Revised: 2026-10-14 (review B4), owner approved ("Approve all four"): the
  Threads row menu adds "Second opinion from <provider>" and "Continue with
  <provider>" after Hand to teammate, one row per other ready provider, held
  with the reason while a turn runs. Rows and line one are unchanged.
- Revised: 2026-10-14 (B3, Away loops as Threads), owner approved ("Approve
  and record trailer"): on the Threads surface an Away loop's thread carries
  Away's own coffee glyph before its title (`.thread-inbox-away`, 4px before
  the words, the dim tier, no colour of its own), and its row's accessible
  name says "Away loop" after the title. Line one is otherwise the title and
  its one slot; every other row is unchanged.

- Revised: 2026-10-14 (review C1), owner approved: the Threads inbox ends in a
  third shelf, From terminals, folded by default and paged like Settled. It
  holds the sessions a Code pane or the CLI started (Rust's `fromTerminals`
  section), which the origin filter used to hide from Threads; its rows are
  slim like Settled's, and the row menu's Continue as thread brings one into
  Active. Threads no longer counts those sessions as hidden. Code and Agent
  keep every look below unchanged.

- Revised: 2026-10-07 (less motion), owner: "lets reduce the motion a little";
  chose "Calm the frame". The live step on line 2 no longer shimmers.

- Revised: 2026-10-07 (live turn), owner approved the running-state canvas
  (https://claude.ai/artifact/VbMD8D6Ng4nFRkmexPzJfR, board 4 "Inbox under
  load": "I like all of them. Go ahead and build"). On the Threads surface a
  working row wears no pill: its slot is the settled Wave at 12px and the
  m:ss clock, and line 2 becomes the live step ("Editing login.ts", still,
  in the dim tier) from the session's own log while it runs. Needs you is the
  inbox's one filled pill (amber fill, its own dark ink at 4.5:1 or better,
  two rings on arrival) and line 2 says what it asks ("Run npm test?");
  Failed is an outline pill and line 2 carries the error's first line; the
  unread dot pops in and line 2 crossfades back to project · branch · diff.
  A row that needs you floats to the top of Pinned or Active (display only,
  a 240ms slide); every other row keeps Rust's order, and drag and Move
  up/down still mean positions in it. Code and Agent keep every look below.

- Revised: 2026-10-06 (hued glow), owner picked "Glow, one level calmer, colored
  by the project" from the canvas's Color page ("Ok 3 is good but tone it down
  … I like hued projects and matching colors to that hue"; "yes, build it").
  On the Threads surface the provider pip gives way to the project's letter
  tile (its hue, `project-hue.ts`); the provider's mark ends line 2 in its
  brand colour; Needs you, Working (live dot, soft shimmer) and Failed are
  tinted pills; the selected row and the unread dot take the row's project
  hue; a faint band of the focused thread's hue with a fine dot grid sits
  behind the header. Code and Agent keep every look below unchanged.

- Revised: 2026-10-06 (redesign), owner approved the Threads redesign canvas
  (https://claude.ai/artifact/45jxnLLJVckWPNMQWUcJW4, "Make all of those
  changes"), which replaces the one-line inbox rows chosen earlier that day.
  On the Threads surface only, Pinned and Active sit at the top under their
  micro-labels in two-line 52px rows: the title (unread strongest; a read row
  with no status recedes) with one slot — Needs you, Working m:ss, Failed,
  Back, else the age after the blue done dot for unread — then a
  provider-coloured pip, the project (a hollow ring and "No project" for a
  plain chat), the branch, and the pull request or the thread's diff totals.
  On hover or keyboard focus the slot gives its box to Snooze and Settle (Wake
  / Un-settle on a shelf); the row menu stays on right-click, ⇧F10 and the
  menu key. Snoozed and Settled are folded shelves pushed to the foot of the
  list, slim 34px rows with a half-strength pip and the wake time or age.
  Code and Agent keep every look below unchanged.

  Earlier the same day (superseded): the owner chose the T3-style inbox
  ("lets try the inbox idea") with one-line rows — title, muted project, one
  status mark — and no provider logo or age
  ([Threads design](superpowers/specs/2026-10-06-threads-design.md)).

- Revised: 2026-10-02 (roomier), owner chose "D · Reference-matched"
  (https://claude.ai/artifact/KqGMLcj3b1xoGwgLGdKsxS). Rows are 32px at the 14px label size, and the sidebar is 240px
  (264px from 1440px windows; Bare CLI 256). Pips, tints and the header row's
  layout are unchanged.

- Revised: 2026-10-02, the owner reported that Codex and Claude pips were the
  same colour and asked for them to differ. An agent pane's pip now takes its
  provider's colour (Claude `#b84f31`, Codex `#12805a`, the launcher marks'
  hues) when working and a faint 50% wash of it when idle. A plain shell's
  working pip is the neutral `--fg-muted` tier, because the status orange read
  as Claude. Needs you, done and failed keep their status colours.

- Revised: 2026-10-01 (second pass), owner sent a closer BridgeMind screenshot and asked to move "a little closer in some ways". The selected workspace's parent project row is tinted and set in
  the strong weight, as in the reference. Idle rows still draw no dot
  (2026-09-12); the pips carry each pane's state.

- Revised: 2026-10-01, owner chose the dense look with sidebar pips
  (https://claude.ai/artifact/DD5wZVCHPHqqXPk5h6ufu3). The sidebar is 212px (232px from 1440px windows; Bare CLI 200 /
  224), rows are 26px at the 13px body size, the header row is 38px with a
  12px label, and the Settings footer row is 32px. A workspace row shows one
  small pip per OPEN pane, in pane order, in that pane's status colour
  (working, needs you, done, failed; idle is faint) where the count was, up to
  eight then "+N"; past eight the coloured pips come first so a pane that
  needs you or has finished is never folded away. A closed tab's terminal
  takes no pip (fixed 2026-10-01 after the owner saw seven pips for three
  panes); the row's own glyph still reports a hidden one. The status glyph and
  working ticker stay.

- Revised: 2026-09-28, owner approved the UI review. In the Porcelain palettes
  the selected workspace's parent project keeps medium weight but loses its
  tint, so it never outweighs the raised row. Bare CLI's header is inset to
  line up with its rows. The Agents header matches Tasks, Chats and Workspaces
  (label size, + and a search glyph that opens the search box), and its empty
  text is the body tier. A provider fault sits under Settings in the footer.

- Revised: 2026-09-26, owner approved Porcelain. Rows are 32px at the 14px
  label size. A palette may raise the selected row as its own surface
  (`--sidebar-selected-fill` with `--sidebar-selected-edge` as a hairline); both
  Porcelain palettes do, and the others keep the neutral fill.

- Extended: 2026-09-22. The owner requested project creation from the Workspaces
  plus button. In Code it offers New workspace and New project; the latter asks
  for a name, creates that folder in the Projects folder (a setting since
  2026-10-14: `~/Developer/Projects` by default, Change… in the dialog picks
  another), and opens its workspace launcher. The compact header and existing workspace appearance are retained.

- Restored: 2026-09-21. After reviewing the Dev collections trial, the owner
  asked to scrap it and revisit organization later. Restore the existing flat
  project hierarchy and Pinned section; remove collection headers and the
  Organize folders dialog. The trial is not an approved direction for future work.

- Settled: 2026-09-05, owner, matched to the BridgeMind sidebar reference.
- Look: "Workspaces" label with creation, Filter and Project options as 24px
  icons in one row; Code has one New workspace action opening the CLI launcher;
  project headers at regular weight unless selected;
  children indented under the project name; one line per session row (no
  provider logo, no age, no second line — those live on the hover card);
  count chips are soft filled pills without a ring, hidden at zero.
- Updated: 2026-09-09, owner requested Settings in an account-style area below
  Workspaces. The footer stays below the scrolling list and uses the same
  neutral hover and selection fills; the workspace rows retain their layout.
- Updated: 2026-09-08, owner reversed the separate Clean/Terminal categories.
  Pane appearance is selected in Settings; the same CLI workspaces and controls
  support both looks. No type picker or appearance icon is shown beside rows.
- Carried by: `electron/src/renderer/components/Sidebar.tsx` (`SIDEBAR_CSS`,
  `sidebar-primary-nav`, `.group-count`), `viewmodels.ts` (`SidebarRowMeta`
  has no `twoLine`), `components/ThreadInbox.tsx` (`THREAD_INBOX_CSS`,
  `thread-inbox-row`).
- Markers: `sidebar-primary-nav`, `.group-count,`, `session-row-lines`, `twoLine`, `workspace-tree-ticker`, `--sidebar-row-selected`, `--sidebar-row-h`, `thread-inbox-row`, `thread-inbox-shelves`, `thread-inbox-slot`, `thread-inbox-actions`
- History: landed `3fd0d81`.
- Extended: 2026-09-09, owner supplied a BridgeMind sidebar reference and
  requested its aesthetic and active-workspace ticker. Rows have more vertical
  space and stronger indentation; selected workspaces and their parent projects
  use neutral gray fills and stronger type. Counts align at the right edge,
  with project actions revealed in that same space on hover or keyboard focus.
  A small six-dot ticker (two columns by three rows, refined by the owner on
  2026-09-09) beside the solid status dot animates only while a
  workspace is working. Idle, waiting and failed states remain still; reduced
  motion shows six static dots. This ticker belongs to sidebar CLI workspaces,
  in both appearances, not to terminal pane headers.
- Revised: 2026-09-09, owner approved the five workspace hierarchy recommendations.
  Project and workspace labels use 16px text. The selected workspace uses the
  stronger neutral fill and weight; its parent gets a quieter fill and medium
  weight. Count alignment and working indicators retain their positions.
- Extended: 2026-09-09, owner approved the screenshot comparison recommendations.
  At window widths of 1440px and above, the sidebar widens to 320px in Refined
  and 288px in Bare CLI so the larger workspace labels show more of each task.
  Compact windows keep their existing sidebar widths and overlay behavior.
- Revised: 2026-09-10. The owner approved the full-screen reference audit for Dev review. The sidebar is 320px normally and 16vw (320-440px) on wide Refined windows; Bare CLI uses a 288px minimum. Workspace labels use 20px text in 46px rows. Dark chrome is #252525 with legible secondary text. Code adds persistent Pinned and Folders sections. Explicit workspace names remain authoritative; automatic names follow the first saved task, independent of pane focus.

- Revised: 2026-09-10. The owner explicitly chose the original compact Interface
  scale, matching the reference's layout and colors only. Sidebar widths return
  to 288px normally and 320px on wide Refined windows (288px for Bare CLI),
  with 16px labels and 36px rows. The charcoal palette, stronger selection,
  Pinned/Folders sections, single-line hierarchy and stable names remain.

- Extended: 2026-09-11, owner requested project pinning alongside workspace
  pinning. Project menus offer Pin project / Unpin project; pinned projects
  retain their expandable hierarchy in Pinned. Workspace pins stay independent,
  and project identity uses the full path. Existing compact row geometry remains.
- Revised: 2026-09-12, owner approved the aesthetic review. Dark chrome is #1f1f1f,
  one step between the canvas and the focused pane head. Section labels (Pinned,
  Folders) use the launcher's 11px uppercase micro-label; an empty Pinned
  section says "none yet" on that line instead of a hint sentence and divider.
  Idle rows draw no status dot (absence is idle; working, waiting and failed
  keep theirs, without the halo). Project rows keep the soft count chip; child
  rows show plain aligned digits. Child indent is 26px. The sidebar toggle
  lives at the end of the Workspaces header actions; the window rail shows it
  only while the sidebar is hidden (superseded 2026-09-17: the toggle lives
  only in the rail; see Compact window header). The Settings footer uses a bare gear glyph
  and carries the running version under the label; the status bar no longer
  renders when it has nothing but the version to say.
- Revised: 2026-09-24, owner approved the icon review. Every surface's create
  button (New workspace, New chat, New task) uses the same +; the speech
  bubbles keep meaning a chat context and a subagent message.
- Extended: 2026-09-25, owner asked for a blue dot when an agent is done. A
  resting workspace row shows the blue done dot while one of its agents has a
  finished turn nobody has focused; failed, needs attention and working
  outrank it, and idle rows still draw no dot.
- Extended: 2026-09-28, owner asked for one button that folds every folder.
  After Filter, the header row carries a 24px Collapse all / Expand all toggle
  (the font's collapse-all and expand-all glyphs). It folds or opens every
  listed project, pinned ones included, and shows only while projects are
  listed; each project's own chevron is unchanged.

## Terminal surface: no bar; a four-square Layout toggle in the toolbar

- Revised: 2026-10-14 (review C2, "one word per idea"), owner approved: the
  ··· menu's row for the Code column of live pane cards reads Live panes
  (it read Agent overview), checked while the column is open; its toggle and
  palette row say Live panes too. "Agent" means teammates only. The rows,
  their order and the menu are otherwise unchanged.

- Extended: 2026-10-01, the update control after `.workspace-toolbar-terminal`
  is now `UpdateButton` (design D4, see Compact window header); the pin that
  orders the rail names it. This look is unchanged.

- Extended: 2026-10-01, the agent overview's Agents tab borrows the
  four-square glyph (`codicon-layout-grid`). The Layout rows stay in ···.

- Revised: 2026-09-28, owner approved the UI review: menu rows are sentence
  case (Tidy panes, Flip terminals, Saved sessions…).

- Settled: 2026-09-05, owner. The 32px terminal bar is gone; the toolbar
  carries one four-square Layout button that slides Flip and Tidy out beside
  it with the discrete display transition, and only the button closes it.
- Carried by: `electron/src/renderer/components/TerminalWorkspace.tsx`
  (`TerminalLayoutMenu`), `terminal.css` (`.terminal-menu-check`),
  `codicons.css` (`.codicon-layout-grid`), `App.tsx`
  (`.workspace-toolbar-terminal` and the menu's destination wiring).
- Markers: `terminal-center-bar`, `codicon-layout-grid`, `workspace-toolbar-terminal`, `Terminal layout`, `Tidy panes`, `Saved sessions…`
- History: landed `600bf25`, `4f3853d`, `259f8eb`.
- Updated: 2026-09-09, header cleanup retains the Layout toggle and its actions.
  In compact windows, expanded actions sit below the toggle to avoid navigation.
- Refined: 2026-09-09, owner requested cleanup of the header image. Layout uses
  the same compact target size as its neighboring utilities; its expanding
  actions and toggle behavior stay intact.
- Revised: 2026-09-14, owner supplied a BridgeMind menu reference and asked the
  four-square button to open a menu like it. The slide-out group is gone: the
  button opens a dropdown hanging leftward from its right edge — folder name
  and `~/` path, Tidy Panes and Flip Terminals (check mark while on), a
  divider, then Usage… (the usage panel), Tasks (the Agent surface), Codex
  accounts… (a placeholder that does nothing until account switching exists)
  and Saved Sessions… (the session library). A choice, Escape or an outside
  click closes it.
- Refined: 2026-09-24, owner approved the icon review. The four squares draw
  at a 1.1 stroke on the 16 grid, the weight of the gauge, mic and bell beside
  them, instead of 1.6. Shape, size, menu and behavior are unchanged.
- Revised: 2026-09-26, owner asked to compact the top bar. The four-square
  button is gone from the toolbar; its rows (folder heading, Tidy Panes, Flip
  Terminals, a divider, Tasks, Codex accounts…, Saved Sessions…) are the
  terminal-view section of the window's ··· menu, with Agent overview (checked
  while open) above them. Usage is the ··· menu's own row, so it is not repeated.
  Only the Output preview toggle is still portaled into `.workspace-toolbar-terminal`.

## Terminal tiles: neutral edge, neutral focus ring, no ticker

- Revised: 2026-11-15, owner asked for the terminal's type and dimensions to
  match their Rex terminal. Rows are the font's own cell again (the 1.2× of
  2026-10-02 below is gone) and terminal text is MonoLisa Bold at 11.5px; see
  "Terminal text is the owner's Rex type: MonoLisa Bold, 11.5px". Tiles,
  heads, gutters and the text's inset are unchanged.

- Revised: 2026-11-15 (later the same day), owner asked for the head's words
  to go ("remove the top bar thing from every pane that says find and files,
  as well as the directory that it's working in and the status of the agent
  ... I just need the ... dot"). The head is the tabs and the icon actions
  again: no folder, no state in words ("Idle", "Working · 2 min", "Done"),
  no Find and no Files. Each tab's status dot is its state, the active tab's
  included; a resting state still draws no dot. The amber "Needs attention"
  word beside the strip (owner, 2026-09-28) shows for any tab blocked on
  you, as before the words came. What the same day's earlier revision took
  off the text stays off: nothing floats over the terminal, ⌘F opens the
  Find row above the text, Copy and Paste lead the ··· menu, and Files is
  the `+` menu's row. The narrow-head steps went with the words. Edge, focus
  ring, corners, gutters, head height, title weight and every other part of
  this entry are unchanged. Carried by `TerminalPane.tsx` and
  `terminal.css`; verified by `TerminalPrimitives.test.ts` ("draws no
  folder, no state in words, no Find and no Files: a tab's dot is its
  state") and the workspace-hierarchy lane at 1920, 1280 and 760px.

- Revised: 2026-11-15 (replaced later the same day, see above), owner approved the Code mockups ("Go ahead and apply
  all of those changes"). The head says what the pane is in words: the
  active terminal's name, its folder (the folder's own name, `~` for home,
  the whole path in the tooltip) and its state beside the status dot
  ("Working · 2 min" while an agent's watched spell of work runs, "Idle",
  "Needs attention", "Done"; a resting state still draws no dot). With one
  tab the words follow the name; with several the tabs keep the spare width
  and the words sit before the actions, and the active tab's dot is the
  state's dot. The body toolbar is gone: nothing floats over the terminal's
  text. Its Search became Find, a quiet word in the head before Files (the
  folder's files, as the `+` menu's Files row), then Fast, ···, Expand, `+`
  and close as before; Copy and Paste lead the ··· menu, and ⌘F, ⌘C and ⌘V
  work as before. Find opens a row above the text that the screen gives up
  while it is open. Narrow heads give way in steps so nothing overflows:
  Find and Files turn to icons with tooltips under 540px, the folder goes
  under 420px, the state's word under 340px (the dot stays) and Find and
  Files leave the head under 280px (⌘F still finds; Files is in the `+`
  menu). Four panes side by side in a 1440px window are 280px heads. Edge,
  focus ring, corners, gutters, head height, title weight and every other
  part of this entry are unchanged. Carried by `TerminalPane.tsx`
  (`TerminalPaneStatus`, `.terminal-pane-meta`, `.terminal-pane-word-action`),
  `TerminalView.tsx` (`TERMINAL_VIEW_COMMAND_EVENT`, `.terminal-search`) and
  `terminal.css`; verified by `TerminalPrimitives.test.ts` ("pane head says
  what the pane is") and the workspace-hierarchy lane at 1920, 1280 and 760px.

- Revised: 2026-10-02, owner approved theme files. The `custom-dark` and
  `custom-light` palettes a theme file paints over keep this look by default:
  the tile edge and focus ring are the mixed lightness as a
  neutral grey (`oklch(from … l 0 0)`; `theme-files.test.ts` checks it). No built-in palette changed; a theme file may restyle it.
- Revised: 2026-10-02, owner chose "D · Reference-matched" (https://claude.ai/artifact/KqGMLcj3b1xoGwgLGdKsxS): 12px
  gutters, 10px tile corners and a 32px head. Terminal rows are 1.2× the
  font's cell (`TERMINAL_LINE_HEIGHT`), and text sits 12px in from the tile's
  edge and 10px under the head. Terminal text stays 12px. Box drawing stays
  joined through xterm's custom glyphs; the DOM fallback (over 12 visible
  panes, or a lost WebGL context) draws from the font and can show hairline
  gaps between rows.

- Revised: 2026-10-01 (second pass), owner sent a closer BridgeMind screenshot and asked to move "a little closer in some ways". 8px gutters and 8px tile corners; every pane's name is in the
  strong weight. Terminal text stays 12px: the reference's ~10px would need a
  new workspace-presentation schema version (v1 accepts 12–26).

- Revised: 2026-10-01, owner chose the dense tiles (https://claude.ai/artifact/DD5wZVCHPHqqXPk5h6ufu3). 6px gutters,
  7px tile corners, 26px heads with 12px titles and 20px action targets (all
  four actions still drawn), terminal text 8px from the tile edge and 6px
  under the head (`.terminal-xterm` padding `6px 0 2px 8px`). A waiting pane's
  amber strip is a 2px line along the tile's bottom edge. Six panes stay
  three columns down to 1100px windows. The window rail is 40px with a 13px
  wordmark, the 24px Agent / Code / Chat switch, and a usage readout (provider
  mark, the fullest plan window's percent, a short bar and the window's name)
  that opens Usage; it shows only when a figure was measured.

- Revised: 2026-09-28, owner approved the UI review. Terminal text clears the
  rounded screen edge by 10px (`.terminal-xterm` padding `14px 0 2px 10px`),
  reversing the 2026-09-10 full-width entry bar. Tile edges stay neutral; each
  palette sets its own neutral focus edge.

- Revised: 2026-09-26, owner approved Porcelain. Tiles are frames around inset
  screens: 14px gutters and 14px frame corners, 38px heads, a neutral edge in
  every palette (Porcelain `#dddddd`, Porcelain Night `#333333`) and a firmer
  neutral focus edge (`--terminal-focus-edge`). The native terminal keeps its
  full-width entry bar (no side inset, 2px bottom) inside the rounded screen.
  The window switch is a flat recessed track with the active tab raised on a
  hairline.

- Revised: 2026-09-11, owner requested the pane swap reference. A temporary
  drag capsule reuses the truncated terminal title; a hovered swap destination
  carries an accent outline and tint for the duration of the drag.

- Settled: 2026-09-05, owner. Tile edge a neutral grey (`--terminal-tile-edge`,
  dark `#262626`), never the blue-tinted `--border-strong`; focus ring
  `--fg-faint` (never the selection accent); no ✻ ticker beside the tab title;
  the pane's name always shows in its head (the owner reversed the
  reference's title-less head on 2026-09-06); the body toolbar and the
  scrollbar thumb appear only under the pointer (the body toolbar was
  retired 2026-11-15, see above); 16px gap, 10px radius,
  no horizontal padding around the native terminal. The window switch follows the compact
  Agent / Code / Chat reference recorded below.
- Carried by: `terminal.css` `.terminal-center` (scoped tile tokens),
  `.terminal-pane`, `.terminal-pane.is-focused`, `.terminal-tab-title`,
  `.terminal-xterm`; `theme.css`
  `.surface-switch`, `.surface-switch-tab.is-active`, `--terminal-tile-edge`.
- Markers: `terminal-tile-edge`, `.terminal-pane.is-focused {`, `.terminal-pane.is-focused::after`, `terminal-tab-ticker`, `.terminal-tab-title {`, `.surface-switch {`, `.surface-switch-tab.is-active {`, `overviewRuler: { width: 8 }`, `.scrollbar > .slider`
- History: landed `600bf25`; edge, gap, radius, toolbar and scrollbar measured against the reference 2026-09-05 22:40; single-tab title clip added then reversed by the owner 2026-09-06.
- Updated: 2026-09-09, owner requested visible borders on unselected panes and
  a CLI model footer close to the bottom edge. Every pane now draws its neutral
  edge above the content; selected panes keep their brighter ring. The emulator
  is bottom-aligned with a 2px bottom inset, retaining the 18px left inset.
- Revised: 2026-09-09, owner supplied Codex pane references and requested the
  entry bar fill the terminal width. Native Terminal and Clean controls now have
  no horizontal wrapper padding, leaving CLI-owned text insets and the scrollbar
  gutter. The existing top spacing and 2px bottom alignment remain.
- Refined: 2026-09-10, owner supplied a thin-scrollbar pane reference. Terminal
  and Clean history use a rounded 4px thumb in an 8px gutter, with the theme's
  subdued scrollbar colors. Thumbs appear under the pointer and brighten while
  hovered or dragged; the gutter stays fixed so content does not shift.
- Updated: 2026-09-09, owner supplied the compact window-header reference.
  Its Agent / Code / Chat switch has a thin neutral edge and neutral active fill.
- Revised: 2026-09-09, owner approved the five workspace hierarchy recommendations.
  Terminal panes sit directly on the workspace canvas without a second shell
  border or perimeter padding. The existing pane edges, corners and split gaps
  stay; native text is 14px, and Terminal headers are 34px with 13px titles.
  Active names use the strong text tier. Secondary actions stay reachable in
  24px targets with quieter resting colors and stronger hover/keyboard focus.
  Rust shortens automatic request-style titles by removing conversational
  lead-ins and truncating at word boundaries. Explicit Interface renames win.
- Revised: 2026-09-11, owner approved stable pane and workspace task names.
  Codex attention/status, Fast mode, model metadata and provisional UUIDs do
  not become names. Prefer explicit pane names and saved Codex chat names,
  falling back to a bounded label from the first actual request. Manual pane
  names survive a full host restart. A workspace retains its initial useful
  task name across focus changes, closed panes and restarts until renamed.
- Extended: 2026-09-09, owner approved completing the screenshot comparison
  improvements on Dev. The active tab uses spare header width in tabbed panes.
  The existing maximize action says Expand (Restore when expanded) in headers
  at least 460px wide; smaller headers retain the icon. Expanding preserves the
  split layout, mounted terminals and input, with the same neutral pane focus.
- Extended: 2026-09-07, owner requested theme consistency. Neutral terminal
  edges now apply to every palette, with Dark's existing edge unchanged.
- Revised: 2026-09-10. The owner approved the full-screen reference audit for Dev review. Terminal text defaults to 20px with a persistent 12-26px setting. Pane heads are 52px with 32px actions; maximize and restore are icon-only. Tiles use 18px corners and a shared 18px gap for automatic and manual layouts, with an 18px left text inset, 10px right inset and 4px bottom inset. Eight automatic panes use four columns on wide windows and two or one on smaller windows. Automatic panes keep at least 220px height and scroll when the window cannot fit every row. Pane names, Fast, neutral borders and focus, purple header hover, native emulators and input remain. Leading image markers and request boilerplate are removed from displayed automatic titles only.

- Revised: 2026-09-10. After reviewing Dev against the video, the owner approved
  reducing the default terminal text from 20px to 18px. The saved 12-26px setting
  remains authoritative. Sidebar, pane heads, gutters, corners and layout stay
  at the approved reference proportions; populated four- and eight-pane views
  are the comparison surface for reading density.

- Revised: 2026-09-10. The owner explicitly restored the original compact scale:
  14px terminal text at normal weight, 34px pane heads and 24px action targets.
  Both split modes use a compact 12px gutter and 10px corners. The retained
  horizontal text insets are 18px left and 10px right, with 14px top and 2px
  bottom. Automatic panes retain scrolling with a compact 180px minimum.
  Eight-pane layout, icon-only maximize, colors and title improvements remain.
- Restored: 2026-09-10, owner reported losing the good terminal spacing and
  requested its return on Dev. Restore the earlier full-width native entry bar:
  zero horizontal wrapper padding, retaining 14px top and 2px bottom. Compact
  typography, pane headers, gutters and corners stay as reviewed.
- Revised: 2026-09-12, owner approved the aesthetic review. Ellipsis and close
  rest visible on every head; Expand and New tab rest hidden and return under
  the pointer, keyboard focus, or while their menu is open (both stay in the
  ellipsis menu). Idle, starting and exited head dots draw nothing.
- Revised: 2026-09-14, owner reported the hover-only Expand and New tab read
  as missing and requested their return. All four header actions (ellipsis,
  Expand, New tab, close) rest visible on every head with the existing
  quieter resting tone.
- Extended: 2026-09-17. The owner added Anthropic Light and Anthropic Dark and
  asked to land them on dev for review. Each palette has its own body, idle-head
  and focused-head shades and a neutral tile edge. Dark's values are unchanged.
- Revised: 2026-09-24, owner approved the icon review. Header actions (and
  the agent tile's) draw at the standard 16px icon tier inside the unchanged
  24px targets, at the font's own line weight: the 0.75px text stroke is gone
  and the Expand/Restore diagonal masks use a 1.25 stroke instead of 2.25, so
  the ellipsis no longer vanishes beside a bold arrow. Order, resting tone and
  hover behavior are unchanged.
- Extended: 2026-09-25, owner asked for a blue dot when an agent is done. A
  head, tab or card dot turns blue (`--status-done`, one blue per theme) when
  its agent finishes a turn while the pane is not focused, and clears when the
  pane is focused or the agent works, asks, exits or fails again. Idle,
  starting and exited dots still draw nothing; the word is "Done".
- Restored: 2026-09-26, owner asked for the original pane buttons back ("the
  smaller thicker plus"). Header actions (ellipsis, Expand/Restore, +, close,
  and the agent tile's) return to the compact 12px tier with the 0.75px text
  stroke, and the diagonal masks to a 2.25 stroke, reversing that part of the
  2026-09-24 icon review. Targets, order, resting tone, hover and every other
  icon-review change are unchanged.

## Color themes share the Dark workspace structure

- Revised: 2026-10-02, owner approved theme files: users keep their own themes
  as `.css` files in `~/.config/interface/themes`, applied as full CSS the way
  Obsidian's are. The owner chose full CSS knowing a file may change more than
  colours and fonts; that freedom is the user's, in their own file. The ten
  built-in palettes are unchanged and still change colours and fonts only. The
  token system moved, value for value, into `@layer interface-palette` so a
  file's plain `:root` rule wins. Two new palettes, `custom-dark` and
  `custom-light`, are what a file paints over: every token is mixed from eight
  `--theme-*` key colours, and an empty file of either kind clears AA. Settings
  lists theme files under "Yours" after the built-in swatches, and Follow
  macOS gains Light and Dark pickers that accept any theme.
- Revised: 2026-09-28, owner approved the UI review. Ten palettes remain:
  Porcelain, Porcelain Night, Clay Light and Dark (the `anthropic-*` ids,
  renamed from Anthropic), Solarized Light and Dark, Catppuccin Mocha, Carbon
  (`grok-build`, renamed from Grok Build; the one Bare CLI palette), Navy
  (`hermes`) and Ice (`hermes-light`). Sauteed Paper and Warm Editorial folded
  into Clay and Nightshade into Carbon; their ids still parse and resolve to the
  replacement. Each palette hands terminals its own 16 ANSI colours, each at
  least 4.5:1 on its screen. Solarized's working light is cyan, Ice's is orange
  and Carbon's failed light is rose, so no two states share a hue. Settings
  picks themes from swatches drawn in each theme's colours, with a Follow macOS
  switch.

- Revised: 2026-09-26, owner approved Porcelain as the shared structure. Every
  palette uses Porcelain's frame-and-screen panes; palettes still change only
  colours and fonts. The Interface light and dark blocks are now Porcelain and
  Porcelain Night, so the bare :root block is only the fallback for tokens a
  palette does not set. A palette may declare `--terminal-color-scheme` (a
  keyword, like `--theme-kind`) when its screens and chrome differ.

- Settled: 2026-09-07, owner requested that every theme follow the current Dark
  design and differ only in colors and/or fonts.
- Look: shared spacing, component layout, corner shapes, heading sizes, motion,
  solid surfaces by default and shadow geometry. Palettes retain their colors.
  The owner separately requested optional terminal/sidebar translucency on
  2026-09-07; the same two material controls apply across every palette/style. Hermes uses
  the same empty states and composer as Dark, without separate wordmarks or a
  watermark. Bare CLI remains an independently selected style, consistent
  across its palettes too.
- Carried by: `theme.css`, `fixtures/theme-consistency-fixture.tsx`,
  `chrome-mode.test.ts`, `theme-accessibility.test.ts`.
- Markers: `--overlay-shadow-soft-color`, `Theme blocks change colors, never blur, spacing or elevation.`
- History: unified at the owner's request on 2026-09-07.
- Revised: 2026-09-22, owner supplied a translucent menu reference. Floating
  menus and dialogs now share a lightly translucent, blurred material across
  palettes. Workspace backgrounds and the terminal/sidebar sliders retain
  their existing behavior. Bare CLI and Reduce Transparency keep solid fills.
- Extended: 2026-09-17. The owner added Anthropic Light and Anthropic Dark and
  asked to land them on dev for review. Each palette has its own body, idle-head
  and focused-head shades and a neutral tile edge. Dark's values are unchanged.

## Floating menus and dialogs use a subtle frosted background

- Settled: 2026-09-22, owner requested the slightly transparent window appearance
  in the supplied screenshot, specifically the pane `+` menu.
- Look: floating menu and dialog backgrounds use 64% of their palette fill over
  an 8px backdrop blur with 110% saturation. Text, icons, borders, geometry and
  focus treatments retain their existing appearance. Compact pane menus retain
  their lifted neutral tint. Bare CLI and Reduce Transparency render solidly.
- Corrected: 2026-09-22, the owner showed that the first Dev version still looked
  solid. The stronger transparency and smaller blur now reveal underlying
  content; compact menus use a 16% neutral lift to match the reference charcoal.
- Carried by: `theme.css` through the existing `is-floating` surface hook.
- Markers: `--floating-opacity`, `--floating-blur`, `--floating-saturate`, `--floating-dropdown-bg`, `--floating-widget-bg`, `--floating-menu-bg`

## Sidebar workspaces stay put when selected

- Settled: 2026-09-06, owner. Clicking a workspace must not move it to the top.
- Look: selecting an existing workspace preserves its saved sidebar position,
  including across restarts. New workspaces still start at the front; the
  existing live-work priority remains independent of selection.
- Carried by: `components/IntegratedTerminalCenter.tsx` (`switchWorkspace`).
- Markers: `Selection preserves the saved sidebar order.`
- History: removed the selection-triggered roster promotion on 2026-09-06.

## Pane header drops swap positions

- Settled: 2026-09-08, owner requested that dragging a pane from its top onto
  another pane exchange their positions.
- Look: the two whole panes trade occupied positions, keeping their tabs and
  active terminals; dropping within the same pane still reorders its tabs.
- Carried by: `terminal-pane-drag.ts`, `components/TerminalPane.tsx`.
- Markers: `dropTerminalPane`, `Drag onto another pane to swap positions`
- History: replaces cross-pane header drops that combined tabs into one pane.
- Dev trial: 2026-09-15, owner authorized the proposed pane motion on Dev.
  The two occupied panes now animate to their exchanged positions over 200ms;
  tab membership, terminal identity and the drop operation remain unchanged.

## New panes split the pane they open from

- Settled: 2026-09-14, owner requested that the pane header's `+` stop halving
  the pane it was opened from.
- Revised: 2026-09-21, owner requested automatic tidying whenever a new pane is
  opened, specifically three panes in one row becoming a 2×2 grid on the fourth.
- Revised: 2026-09-28, owner approved the Browser row following the same rule.
- Revised: 2026-10-06, owner asked for a new pane to split the space its source
  pane occupied in two: the re-tidy moved panes the owner had not touched, and
  their glide read as strange motion.
- Revised: 2026-11-15, owner asked for the in-app browser to "just open a
  Google Chrome tab in the pane where I open the browser": the Browser row no
  longer splits its pane; it adds a Browser tab to it (see
  [workspace tools](WORKSPACE_TOOLS.md)).
- Look: the pane `+`, its Terminal and agent choices give the new pane half of
  the source pane, along the source's longer side as drawn
  (side by side when it is wider than tall, stacked otherwise). No other pane
  moves or resizes. A drawn equal grid first takes the tree matching its cells,
  so the source is halved where the grid showed it. The new pane follows its
  source and receives focus; tabs and terminal mounts survive. The Browser row
  splits nothing: it adds a Browser tab to the source pane, shown and focused.
  A Browser opened from a terminal's link halves that terminal's pane the same
  way as the `+`, so the terminal stays in view. Explicit
  Split Right/Down controls and ⌘D/⌘⇧D keep their fixed direction; Tidy Panes
  still makes the equal grid on request.
- Carried by: `terminal-workspace.ts`, `components/IntegratedTerminalCenter.tsx`,
  `components/TerminalPane.tsx`.
- Markers: `splitTerminalPaneInPlace`, `drawnPaneSplitOrientation`
- Verification: `terminal-workspace.test.ts` (`splitTerminalPaneInPlace`) pins
  the tree; the real renderer pane-motion fixture checks that the neighbours
  keep their rectangles, the source is halved, and a rebuilt identical layout
  does not replay the motion.
- History: replaces "New panes automatically tidy the workspace" (2026-09-21),
  which itself replaced the rule that `+` kept expanding only its source row.

## Pane swap preview follows the dragged terminal

- Settled: 2026-09-11, owner supplied a reference with a tinted target and Swap badge.
- Look: dragging a pane header or tab shows a compact floating capsule with
  its status dot, provider icon and truncated title. Over another pane, the
  capsule adds the swap icon and Swap label; the destination gets an accent
  outline and translucent tint. The source and same-pane tab reorders do not
  show a swap target. Feedback clears on drop, cancellation or leaving the target.
- Carried by: `terminal-pane-drag-preview.ts`, `components/TerminalPane.tsx`, `terminal.css`.
- Markers: `terminal-pane-drag-preview`, `.terminal-pane.is-drop-target`
- Verification: rendered dark/refined at 1100px and light/refined and dark/CLI
  at 640px; four layouts, eight swaps and 22 preserved xterm instances. Native
  Chromium pointer drag verified target feedback, exchanged positions and cleanup.

## Attention tray

- Extended: 2026-09-28, owner approved the UI review: a Finished row's check
  is the blue done colour, matching the pane, tab and sidebar dots.

- Settled: 2026-09-05, owner. A bell with a count badge in the toolbar
  replaces the "N need you" pill; its panel lists blocked agents and ringing
  panes (live, self-clearing) and finished/failed turns (until opened or
  cleared), each with where and since when; a row jumps to its session or pane.
- Carried by: `electron/src/renderer/attention-tray.ts`,
  `components/AttentionTray.tsx`, `App.tsx` (`<AttentionTray`).
- Markers: `<AttentionTray`, `attention-tray-count`, `codicon-bell`, `workspace-toolbar-attention`
- History: landed `05efc5d`.
- Updated: 2026-09-09, the owner-requested compact header keeps the bell and
  count in its right utility group, with the same tray behavior.

- Revised: 2026-09-09, owner requested a cleanup of the supplied header image.
  The neutral count sits beside the bell inside its button, capped visually at
  99+ while the accessible name retains the exact total. It never covers the icon.
- Revised: 2026-09-12, owner approved the aesthetic review. The number beside
  the bell counts live needs only (blocked agents, ringing panes) in the
  attention amber; unread finished or failed turns show a quiet dot instead.
  The accessible name still states the full total and unread count.
- Revised: 2026-09-26, owner asked to compact the top bar and moved the bell
  into the ··· menu. The ··· button now carries the same badge (live count in
  the attention amber, 99+ cap, else the quiet dot) and its accessible
  description states the full total and unread count; its name stays "More
  workspace options". The menu's Attention row opens the same tray under ···.

## Terminal Fast mode beside the pane menu

- Settled: 2026-09-07, owner requested a small lightning toggle next to the three dots.
- Look: Terminal appearance places a lightning icon immediately before the pane
  ellipsis. It appears only when Interface knows the active terminal's model
  supports Fast. The Clean bottom-bar variant was archived on 2026-09-14.
- Updated: 2026-09-08, owner requested settings controls in the Clean bottom bar
  so they can change CLI settings without switching into Terminal controls.
- Revised: 2026-09-09, owner approved all five workspace recommendations,
  including the compact group explicitly naming model/effort, Fast and window
  actions. Fast shares 24px targets and the quieter resting action tone;
  enabled/disabled states, position and behavior retain their existing rules.
- Carried by: `components/TerminalPane.tsx`, `components/TerminalFastModeToggle.tsx`, `components/FastBolt.tsx`,
  with the former Clean controls preserved in the Clean archive.
- Markers: `terminal-fast-mode`
- History: added at the owner's request on 2026-09-07.
- Extended: 2026-09-24, owner approved the icon review. The pane's outlined
  bolt moved into `components/FastBolt.tsx` unchanged, and the composer's Fast
  toggle draws the same bolt instead of the font's zap. The pane toggle's
  position, size and behavior are unchanged.

## Agent sidebar has shaped characters and a vertical roster

- Settled: 2026-09-07, owner, using the supplied Grok Bot screenshot.
- Look: agents have distinct coloured vector characters with recognizable
  silhouettes. Search leads a vertical roster; each row pairs its avatar with
  its name and latest task preview, with a soft full-row selection. Task
  navigation follows the agent list. Selecting an agent preserves roster order.
  Each row also carries a status mark: the working indicator, the attention
  amber for needs attention, or the tray's quiet unread dot. The preview line
  shows the agent's latest thread message when it has a thread.
- Carried by: `components/AgentMenu.tsx`, `agent-appearance.ts`,
  `components/Sidebar.tsx` (agent menu before task navigation).
- Markers: `AGENT_CHARACTERS`, `.agent-menu-roster`, `.agent-menu-row[aria-selected`, `Agent identities lead;`, `agent-menu-status`
- History: replaces the two-column initials grid with the owner-requested roster.
  On 2026-09-09 the owner requested unique shape and color choices in the Agent
  build-out. Shape and color are independently selectable; the original six
  character paths and the vertical roster remain the migration defaults.
- Revised: 2026-09-15, owner approved the teammate status mark.
- Revised: 2026-09-24, owner approved the icon review. The star and hexagon
  shapes round their corners like the six original characters; colors, faces,
  sizes and the roster are unchanged.

## Terminal and sidebar translucency

- Settled: 2026-09-07, owner requested Codex-like translucency sliders.
- Look: Appearance has independent Terminal panes and Sidebar sliders, from
  Solid (0%) to Glass (100%). Both default to zero. Only backgrounds become
  translucent; text, controls, neutral pane edges and focus rings stay opaque.
  macOS supplies native behind-window blur, and Reduce Transparency restores
  solid rendering while retaining the selected values. Header hierarchy stays.
- Carried by: `components/TranslucencySlider.tsx`, `translucency.css`,
  `appearance.ts`, `main/window-material.ts`.
- Markers: `data-terminal-glass`, `data-sidebar-glass`, `translucency-slider`
- History: added at the owner's request on 2026-09-07.
- Corrected: 2026-09-08, owner requested fixes for the glass rendering bugs.
  Project headers share the sidebar material, with rows clipped beneath sticky
  labels. Settings and Clean panes appear in place without fading their text.
  The native material backing is established at window creation and retained
  across slider changes so dismissed content clears from glass panes.
- Revised: 2026-11-15, owner approved the Settings redesign ("Go ahead and
  apply all of those changes"). In Settings › Appearance the row is named
  See-through panes, and its two sliders sit side by side on that row, each on
  one line: its name (Terminals, Sidebar), the slider, its percentage. The
  Solid and Glass end labels are not drawn there (`.settings-page
  .translucency-slider-endpoints`). The values, the zero defaults and what
  turns translucent are unchanged.

## Delegated workers beside their principal

- Retired: 2026-10-14, approved by the owner ("Approve and record trailer").
  The owner archived pane delegation (opportunity review
  F3, "Archive all three"): the worker split, its live worker panes and the
  pane menu's "Show delegated work" are gone. Each pane keeps the same grid
  tile, now `terminal-pane-frame`; maximize, pane motion and the automatic grid
  are unchanged. Archive: [source, design and data](archive/pane-delegation-20261014/README.md).
  Re-enabling delegation requires a new owner instruction. The record below is
  historical.
- Settled: 2026-09-08, owner requested a terminal split instead of the header delegation icon.
- Look: delegated work automatically opens an adjacent pane with live messages
  and tool activity. The principal remains visible and keeps its typing focus.
  Two simultaneous workers share the adjacent column; the divider is resizable.
  Hiding the view keeps workers running. The pane menu reopens delegated work,
  and proposal review remains available from the worker pane.
- Carried by: `components/TerminalDelegationSplit.tsx`, `components/TerminalPane.tsx`.
- Markers: `terminal-delegation-split`, `Show delegated work`
- History: replaces the delegation tree icon at the owner's request.
- Dev trial: 2026-09-15, the owner's pane-motion request animates the existing
  principal/worker tile as a group. Shared mount slots preserve that group
  between automatic and custom layouts. Worker placement, split controls and
  lifecycle semantics remain unchanged; final motion preference awaits review.
- Updated: 2026-09-08, owner requested the actual OpenCode window. OpenCode
  workers show their native TUI attached to the same session, retaining the
  adjacent split, principal focus, hiding behavior and proposal review.
- Updated: 2026-09-08, owner requested finished worker windows close automatically.
  Each worker pane closes when the worker stops running; the last one restores
  the principal's full width. The pane menu opens saved results for review when
  no workers are running.
- Revised: 2026-09-10. The owner approved the reference workspace scale for Dev review. The automatic grid supports eight principal panes in four columns on wide windows; delegated worker placement remains attached to its principal.

## Clean appearance uses the existing CLI panes

- Retired: 2026-09-14. The owner requested: "Take all of the clean GUI stuff out
  of dev and file it away". Terminal panes now always show their native CLI.
- Removed: Clean appearance/input settings, semantic overlays, custom composer,
  model/effort menus, approval/review overlays, and Clean-specific header styling.
  Chat and Agent conversations retain their regular UI and drafts.
- Compatibility: stored Clean values and frozen wire contracts remain readable;
  they no longer activate a Clean surface or change terminal input routing.
- Archive: [source, design, review and recovery guide](archive/clean-gui-20260914/README.md).
  Re-enabling Clean requires a new owner instruction.
- Carried by: `appearance.ts`, `SettingsPage.tsx`, `TerminalView.tsx`.
- Markers: `data-pane-style="clean"`, `Pane appearance`

## Pane header hover tints only the top bar

- Settled: 2026-09-08, owner requested the supplied pane-header hover reference.
- Look: hovering the top bar adds a subtle purple tint to that bar only. Hover
  never draws a pane outline or glow, and never changes which pane is selected.
  The existing selected-pane outline remains reserved for actual selection.
  Selection and header keyboard focus retain the theme's grey focused head;
  purple appears only during pointer hover. Reduce Motion removes the fade.
  Both Clean and Terminal appearances keep their focused and glass backgrounds.
- Revised: 2026-09-09, owner approved reducing the visual weight of secondary
  pane controls. Their resting text is quieter; hover and keyboard focus bring
  it back to the previous muted tone. The purple wash itself is unchanged.
- Carried by: `terminal.css`.
- Markers: `.terminal-pane-head:hover`, `rgb(139 126 189 / 16%)`
- History: corrected on 2026-09-08 after the owner clarified that the reference
  calls for a purple top bar, not the pane-wide outline previously implemented.
  The owner then clarified that selection remains grey and only hover is purple.

## Clean working Wave

**Settled:** 2026-09-08. The owner selected Wave from the animated previews.

- The retired Clean appearance used one provider-neutral five-bar Wave for active
  work in the transcript footer and agent activity. Compact
  locations use the same movement at 16px; the transcript mark is 24px.
- Updated: 2026-09-08, owner requested the previous green dots in the sidebar
  and pane top bars. Sidebar project headers, workspace/session rows and hover
  previews, plus pane headers and title cards, retain their previous indicators.
  Wave remains in the conversation working status and activity details.
- Rounded bars rise and fall in a smooth staggered 1.5-second loop, using the
  theme's working color. Status words and elapsed time stay steady.
- Idle, waiting, failed and finished states retain their existing meanings.
  Terminal appearance keeps its existing marks. Reduced motion shows a static
  wave at full opacity.

**Carried by:** `WorkingIndicator.tsx`, `WorkingIndicator.css` and their status
callers. `working-indicator-fixture.tsx` checks actual rendered mode, state and
motion boundaries across providers, widths and palettes.
**Markers:** `working-wave`, `working-indicator`
- Revised: 2026-10-23 (the Claude-app look), owner approved: in a Threads
  pane the Pair (`ThinkingMark`) replaces the Wave where the 2026-10-07
  extension put it: the live turn's status line (now under its trail), the
  Latest capsule, and a working row's slot in the Threads inbox at 13px. A
  running subagent's or tool's state glyph in the transcript keeps the Wave,
  as does every surface outside Threads. See
  [Threads: the Claude-app look](#threads-the-claude-app-look).
- Revised: 2026-10-22, owner approved for battery: the Wave keeps its
  staggered 1.5-second rise and fall but steps at about 12 frames a second
  instead of easing at the display's rate, and rests while Interface is not
  the app in front. See [Looping motion budget](#looping-motion-budget).
- Extended: 2026-10-07 (live turn), owner approved: in a Threads pane the
  Wave also marks the live turn's header, under its prompt (the bottom
  working strip is gone there), and a working row's slot in the Threads
  inbox at 12px. Same movement, colour and reduced-motion rule.
- Revised: 2026-09-12, owner approved the aesthetic review. The Wave also draws
  the working state in the app's own transcripts (Chat and Agent surfaces) and
  the Agent task header in Refined chrome. CLI panes keep the appearance rule:
  Terminal keeps its marks, Clean shows the Wave. Bare CLI keeps its glyphs.
- Retired in CLI panes: 2026-09-14, with all Clean GUI. The separately approved
  Refined Chat/Agent transcript and Agent task-header Wave remains. Clean-only
  canvas and delegated-task waves are removed.

## Terminal effort in the pane top bar

- Settled: 2026-09-09, owner requested removing the effort selector from the
  pane top bar to reduce clutter.
- Look: pane headers contain no effort selector. Fast retains its existing
  position beside the pane menu. Clean composer controls were archived with
  Clean GUI on 2026-09-14. Use the CLI's native model and effort controls.
- Carried by: `components/TerminalPane.tsx`, `components/TerminalView.tsx`.
- Markers: `terminal-effort-control`, `terminal-pane-agent-controls`, `assertNoHeaderEffort`
- History: added and refined at the owner's request on 2026-09-09, then removed
  from the header at the owner's request the same day.
- Reconfirmed: 2026-09-10, owner requested removing or restyling the remaining
  production header selector. Remove it to match the surrounding icon controls.

## Compact window header

- Revised: 2026-11-15 (UI redesign), owner approved the before/after mockups
  ("Go ahead and apply all of those changes"). The sidebar footer is one
  quiet Settings row with the amber "N providers need setup" on its right;
  see [Sidebar: one header row, single-line rows, soft count chips](#sidebar-one-header-row-single-line-rows-soft-count-chips).
  The provider fault is no longer a danger-tier row under Settings. While the
  sidebar is hidden, the toggle carries a 6px amber dot for that notice (its
  accessible description and tooltip name it). Errors that used to be a
  full-width bar across the top of the work area are small notices in its
  bottom-right corner (`components/CornerNotices.tsx`), which never cover a
  pane header or a composer. The toggle, rail and switch are otherwise
  unchanged.

- Revised: 2026-10-22, owner approved for battery ("Animate 3 times, then
  rest"): a ready update's orange dot pops and its words drift three times,
  then rest with the dot showing. Coming back to Interface from another app
  plays them three more times. The tile, colours, words and reduced-motion
  rule are unchanged. See [Looping motion budget](#looping-motion-budget).

- Revised: 2026-10-20, owner asked for a Codex-like hidden sidebar that comes
  back on hover, then found the window's left edge too far and chose the
  toggle beside the traffic lights. The one sidebar toggle sits just right of
  the traffic-light slot, before the wordmark, in both states and at every
  width (it no longer rides to the sidebar's right edge; it still never
  moves). While the sidebar is hidden on a desktop window, resting the
  pointer on the toggle for 120 ms slides the panel out underneath it, over
  the panes, to exactly where its column puts it; nothing moves or refits.
  It slides away 300 ms after the pointer leaves it, at once on a press
  outside, and waits while one of its menus, confirmations or its search is
  in use. Clicking the toggle keeps it without a jump. A drag over the toggle
  never summons it, and neither does the pointer a click left there when it
  hid the sidebar. The window's left edge does nothing. Shown or hidden is
  remembered across launches (`interface.sidebar.v1`).
- Markers: `SIDEBAR_PEEK_OPEN_MS`, `SIDEBAR_PEEK_CLOSE_MS`, `.app-grid.no-sidebar.sidebar-peek > .sidebar {`, `sidebarPeek.trigger`

- Revised: 2026-10-02, owner chose "D · Reference-matched" (https://claude.ai/artifact/KqGMLcj3b1xoGwgLGdKsxS). The rail
  is 44px and the traffic lights sit at (16, 16), centred in it; the slot
  reserved for them is 70px.

- Revised: 2026-10-01, owner chose design D4 on the update-button canvas
  (https://claude.ai/artifact/KoWCTwmggzw9AaXby2bPJG) to make the update
  control "more colorful and interesting". It is a small blue-violet tile
  (`#5b8cff` to `#a06bff`) with its words in the same colours
  (`components/UpdateButton.tsx`): an up arrow and "Update" while one is
  available; a ring that fills with the download and "Downloading N%"; a
  restart arrow, a popping orange dot on the tile and colour drifting through
  "Restart to update" when ready. Windows 960px and narrower keep the tile
  alone. Reduced motion stills the dot and the drift. This replaces the plain
  download glyph and its 4px dot.

- Revised: 2026-10-01 (second pass), owner sent a closer BridgeMind screenshot and asked to move "a little closer in some ways". The agent overview has its own toggle again at the rail's right
  edge (the sidebar-right glyph, pressed while open), where the column
  opens; ··· keeps its Agent overview row. The usage readout reads like the
  reference: the window and percent ("7d 62%") over a 2px bar as wide as the
  text.

- Revised: 2026-09-28, owner approved the UI review. The Fold's in-window
  plate is monochrome (the theme's strongest ink with a canvas stroke); the
  Dock icon keeps emerald and Dev builds keep the canary chip. Output preview
  uses the open-preview glyph. A provider that needs setup is a danger-tier row
  under Settings in the sidebar footer, not a lone status-strip item. Agent,
  Code and Chat all sit on the bare canvas; only terminal panes are cards.

- Revised: 2026-09-26, owner approved Porcelain. The Agent / Code / Chat switch
  is a flat recessed track without an outline; the active tab is the surface
  colour on a hairline edge.

- Settled: 2026-09-09, owner supplied the minimal top-bar reference.
- Look: centered Agent / Code / Chat switch, thin outline and neutral active
  fill; quiet product wordmark and sidebar toggle on the left; compact icon-only
  utilities on the right. Command palette remains in the ellipsis menu.
  The Dev wordmark keeps its channel badge. Update status retains its accessible
  name and tooltip on the rocket. Pane-level Fast stays in its header.
- Updated: 2026-09-09, owner requested Settings below Workspaces, like an account
  area. A persistent sidebar footer carries a full-width Settings row with a
  round gear icon, shortcut and neutral active fill; the workspace list scrolls
  above it. Settings no longer appears in the top-bar menu.
- Revised: 2026-09-09, owner requested a cleanup of the supplied header image.
  The filled mode switch is 30px high (26px in Bare CLI), with 24px tabs.
  Utility targets share a 28px size, reducing to 24px in compact windows and
  Bare CLI. The update rocket uses the same 16px glyph size and quiet surface
  as its neighbors, with a small status dot instead of a bright filled tile.
- Revised: 2026-09-09, owner approved the stronger top-left anchor. The product
  wordmark uses 15px strong text; the sidebar toggle aligns to the sidebar edge
  on desktop. Hidden-sidebar and narrow layouts keep the toggle beside the
  wordmark; narrow windows use a 13px wordmark. Navigation stays centered.
- Carried by: `App.tsx`, `theme.css`, `SurfaceSwitch.tsx`, `WorkspaceMoreMenu.tsx`,
  `Sidebar.tsx`.
- Markers: `workspace-toolbar-leading`, `workspace-toolbar-actions`, `More workspace options`, `sidebar-footer`, `sidebar-settings-button`
- Verification: rendered layouts at 1280, 760 and 640px, both styles and light/dark;
  menu focus, Escape, workspace selection, Usage and Attention remain reachable.
- Revised: 2026-09-10. The owner approved the full-screen reference audit for Dev review. The window rail is 60px high with a 20px desktop wordmark and larger desktop utility targets. The centered mode switch, compact-window behavior, Dev badge, sidebar-aligned toggle and Settings footer remain.
- Revised: 2026-09-10. The owner restored the original compact Interface scale.
  The rail returns to 44px with its original wordmark and control sizing. The
  original 11/12/13/14/16px metadata-to-navigation type ramp and lighter text
  weights apply throughout. Reference colors and the existing layout remain.
- Revised: 2026-09-12, owner approved the aesthetic review. The Fold mark sits
  at 16px in the accent before the wordmark; the same mark heads the launcher
  and greys itself in empty states. The sidebar toggle moved into the sidebar
  header; the rail carries it only while the sidebar is hidden. The update
  control uses a plain download glyph with the same status dot.
- Revised: 2026-09-17, owner asked for a Claude-app-like sidebar hide whose
  button does not move. One toggle lives in the rail at the sidebar's right
  edge in both states (beside the wordmark only on narrow windows). Hiding
  slides the column to zero over the slow motion token while the panel rides
  off the left edge; terminals hold their grid during the slide and refit once.
  Reduced motion snaps.
- Markers: `workspace-sidebar-toggle`, `.app-grid.no-sidebar {`, `holdTerminalsWhileSliding`
- Revised: 2026-10-14, the owner archived the Control assistant (review item
  F3, "Archive all three"): ··· no longer lists Control assistant.
- Revised: 2026-10-14, owner approved review item C3
  (https://claude.ai/artifact/V6oyGmCTdQ7mMLUVUkvUTd): ··· also lists
  Coordination canvas, Worktrees and Context inventory…, between the terminal
  layout section and Command palette, so they are no longer palette-only.
- Revised: 2026-09-26, owner asked to compact the top bar. The right group is
  Output preview (terminal view), the update pill when present, Voice, Away and
  ···. Layout, Agent overview, Usage and Attention live in the ··· menu (in
  that menu: Attention, Usage, the terminal layout section, Command palette,
  Control assistant); Usage and Attention open their panels under ···.
- Revised: 2026-09-24, owner approved the icon review. The Fold sits on its
  own rounded plate at 18px, the app icon in miniature (emerald with a white
  stroke; the Dev build's canary chip with a charcoal stroke), because the bare
  16px accent stroke read as a » control; empty states grey the plate and the
  stroke. The sidebar toggle fills the LEFT column (its outline twin while the
  sidebar is hidden). Voice live is a filled microphone, and the status strip
  shows the speaker only while it is speaking. Away starts from a coffee glyph
  in one split button with a narrow chevron and a hover hairline; sync stays
  the update check's glyph.

## Native iPhone navigation with shared session views

- Revised: 2026-10-14, owner approved the rendered shared inbox and recording
  the commit trailer. Code/Inbox tabs sit above native cards for Threads and
  Agent work, with Needs you first, progress and saved results. Sessionless
  queued tasks remain informational; completed tasks open read-only details.
  Supported approval cards offer Allow once/Deny; uncertain answers never replay.
  Verified on the disposable iPhone 17 Pro Simulator in the retained charcoal
  and emerald palette, with 40 native tests including the encrypted fixture.

- Approved by the owner: 2026-09-11. Native iOS workspace list, search, navigation
  and Face ID pairing surround Interface’s shared conversation and terminal views.
- Preserve the existing desktop session rendering; use native phone controls for
  connection, unlock, session selection and back navigation. Avoid duplicate web
  workspace navigation inside the selected native session.
- Implementation: `apps/interface-ios/InterfaceApp.swift`, `PhoneThreadViews.swift`,
  `SessionWebView.swift`, and the `is-native-session` presentation in
  `electron/src/remote/`.
- Revised: 2026-09-11. The owner requested a refined phone conversation interface
  with replies and approvals. Native sessions have no terminal switch or key strip.
  The phone uses 16px message text, spacious user bubbles, expandable shared tool
  activity, a rounded message composer and inline single-use approval cards.
  Unsupported session types show an explanation; desktop and Safari terminal
  controls retain their existing presentation. Approval layouts that cannot be
  recognized safely remain on the Mac.
- Revised: 2026-09-11. At the owner's request, the iPhone app adopts Interface's
  existing white Fold mark on its emerald tile for the Home Screen, connection
  screen and shared phone web icon. iOS receives full-bleed opaque artwork and
  applies its own corner mask. The desktop artwork remains the brand reference.

- Revised: 2026-09-11. The owner explicitly requested a better iPhone GUI and
  production release. The phone now uses a unified warm charcoal and emerald
  palette, native workspace filters and grouped session rows, a compact growing
  reply field, and an approval card that temporarily replaces the reply field.
  The brand, shared conversation rendering, native navigation, and Face ID flow
  remain. The new presentation is scoped to iPhone; desktop and Safari controls
  retain their appearance.

- Revised: 2026-09-11. The owner approved Interface-owned QR pairing, Face ID
  unlock, nearby discovery and an encrypted relay. The primary phone setup is
  **Scan Mac QR code**, with production/Dev identity confirmation before pairing.
  The existing address-based connection remains in a secondary disclosure during
  migration. Connection state names the actual nearby or relay route and reports
  an unavailable Mac explicitly; shared session presentation stays unchanged.
- Revised: 2026-09-24, owner approved the icon review. Session rows show the
  desktop's provider marks in their brand colours (asset catalog SVGs drawn from
  `ProviderLogo`), not SF Symbols; shells and unknown profiles keep a terminal
  glyph. Safari's Add to Home Screen gets a 180px PNG of the same opaque Fold
  tile. Row layout, tiles and navigation are unchanged.
- Revised: 2026-09-26, owner approved. The conversation reply bar names its
  provider with the same 13px `ProviderLogo` mark in brand colour (Claude's
  sparkle codicon was never in the vendored subset and drew nothing; Codex's
  `</>` glyph is replaced to match). The phone keeps its charcoal and emerald
  palette; the desktop's Porcelain default does not apply to it.
- Revised: 2026-10-14, the owner dropped Tailscale and archived its route
  (review item F3): the connection screen no longer offers the "Existing
  Tailscale connection" address disclosure or its "Existing connection through
  Tailscale" label, and a route is never named "Connected through Tailscale".
  QR pairing, nearby and relay routes and their presentation are unchanged.
  The phone's control desk (Control assistant) left its connection menu with
  the archived feature.
- Revised: 2026-09-26, owner asked for the phone list sorted by project rather
  than by workspace, matching the Mac sidebar. Sessions group under the project
  the sidebar files them in (a chat's repository; "Other" for chats outside any
  repository, listed last), with "All projects" and project chips; projects sort
  by name, and inside one live sessions come first, then the most recently
  changed. Row, card and header styling are unchanged.
- Revised: 2026-10-14, owner approved (review D6). Needs you comes first: a
  session that needs you (decided on the Mac, roster v4) shows an amber "Needs
  you" label and a dot on its provider tile, sorts first inside its project
  (ahead of "live first"), and lifts its project to the top of the list and the
  chips; the header line says how many need you in place of "Keep the
  conversation going." A Code | Threads segmented control switches to the Mac's
  Threads in its order, drawn as Needs you, Working and Threads groups with the
  same rows and cards. A thread opens a native page: prompt bubbles, the agent's
  words and tool-call counts for its recent turns, an Allow once / Deny card
  (amber edge, "Needs your OK") while it waits on an approval, a question
  deferred to the Mac, and a rounded follow-up field with a send button once it
  is at rest; an uncertain answer or message says to check on the Mac instead
  of offering a repeat. The amber is the desktop's waiting colour; the charcoal
  and emerald palette, navigation and shared session views are otherwise
  unchanged. Implementation: `InterfaceApp.swift`, `PhoneThreadViews.swift`.

## Pane + opens the New pane menu; file drops show a path pill

- Revised: 2026-11-15, owner approved the Code mockups: Copy and Paste left
  the strip that floated over the terminal's text and lead the ··· menu
  (Copy greyed until text is selected; titles name ⌘C and ⌘V, which are not
  printed as chords because the terminal centre does not bind them). The
  ··· menu otherwise keeps Split right/down, Pass off and Clear in order;
  the `+` menu, its rows, order, glyphs, chords and the drop pill are
  unchanged. Verified by `TerminalPrimitives.test.ts` ("puts Copy and Paste
  first in the ··· menu") and the pane-menu lane.

- Revised: 2026-11-15, owner asked for the in-app browser to open "a Google
  Chrome tab in the pane where I open the browser": the Browser row adds a
  Browser tab to this pane instead of a Browser pane beside it. The menu's
  rows, order, glyphs, chords and compact surface are unchanged.
- Revised: 2026-09-28, owner approved the UI review: the last rows read Split
  down, New tab and New session…, sentence case like the ··· menus.

- Settled: 2026-09-14, owner supplied BridgeMind references for the pane `+`
  menu and for dragging an attachment over a pane.
- Look: the `+` opens "New pane" with the folder in mono beneath it, then
  Terminal, Files, Browser and iOS Simulator with muted icons (Browser adds a
  Browser tab to this pane); a
  full-width divider; the existing harness roster with provider marks; a
  divider; Split Down, New Tab and New Session…. Chords print right-aligned in
  the faint tier, and only for chords the terminal centre binds (⌘D, ⌘⇧D). The
  ellipsis keeps Split right/down, Pass off and Clear (delegated work left with
  pane delegation, 2026-10-14). A file
  dragged over a terminal shows a soft selection-tinted radial glow behind one
  centred pill, "Drop to insert path" with a file icon, instead of a wash.
- Carried by: `components/TerminalPaneNewMenu.tsx`, `components/DropIndicator.tsx`/`.css`,
  `terminal.css`.
- Markers: `terminal-pane-new-menu`, `terminal-menu-shortcut`, `drop-indicator-pill`, `Drop to insert path`
- Verification: `terminal-pane-menu-fixture.tsx` routes every row of both menus
  and checks printed chords; `TerminalPrimitives.test.ts` pins row order, chords
  and the drop pill.
- Revised: 2026-09-14, owner supplied a second BridgeMind menu reference and
  asked the New pane menu to look more like it. Every compact popover (New
  pane, ···, Pass off and the toolbar Layout menu) sits on a lifted neutral
  surface (7% `--fg` over `--dropdown-bg`) with a soft neutral border; rows rest
  in `--fg-muted` and brighten to `--fg-strong` on a 9% neutral fill, and that
  fill marks keyboard focus instead of the accent outline. Provider marks keep
  their brand colours. Carried by `.terminal-menu-panel.is-compact` in `terminal.css`.
- Revised: 2026-09-22, owner requested the screenshot's subtle transparency.
  Compact pane menus retain their neutral lift over the shared frosted material;
  layout, rows, shortcuts, hover and keyboard behavior stay the same.
- Corrected: 2026-09-22, owner reported that the first Dev result still appeared
  solid. Compact menus use a 16% foreground tint over the more translucent
  backdrop, keeping the reference's lighter charcoal appearance.
- Revised: 2026-09-14, owner resupplied the drop reference for every attachment
  indicator. One shared `DropIndicator` now serves Terminal panes ("Drop to
  insert path"), Clean panes ("Drop image to attach", previously no cue), chat
  and the new-session composer ("Drop files to attach", previously a dashed
  box). The pill is borderless and bolder with a softer shadow and a wider glow;
  the historical Clean fixture is in the archive.
- Updated: 2026-09-14, Clean was retired. The shared cue remains on native
  Terminal, Chat and new-session composer surfaces.
- Revised: 2026-09-24, owner approved the icon review. One glyph, one
  meaning: Browser is a globe (the external-link arrow stays with links), iOS
  Simulator is the font's phone rather than a CSS-drawn capsule, and Split
  Down / Split right (in ···) are outline panes divided once instead of the
  filled sidebar glyph. Rows, order, chords and the compact surface are
  unchanged.
- Revised: 2026-09-26, owner asked to remove Canvas from the `+` and for
  Browser and iOS Simulator to really work. Canvas is gone from the tools;
  Browser opens an in-app browser pane beside the panes (see
  [workspace tools](WORKSPACE_TOOLS.md)) instead of a URL dialog, and iOS
  Simulator starts an iPhone. The other rows, order, glyphs, chords and the
  compact surface are unchanged.
- Revised: 2026-09-28, owner asked for the browser to be "a pane just like all
  the others" instead of a separate window or side panel. Browser now opens a
  Browser pane in the split beside the source pane, framed like every pane,
  its tab marked with the globe. The menu's rows, order, glyphs, chords and
  compact surface are unchanged.
- Extended: 2026-10-14, owner asked for the Browser pane to look "more like
  the sidebar" (the agent overview). Inside its frame, the bar is the
  overview's tools row: a 34px rounded search field (search glyph while
  typing, globe on a page, loading along its lower edge), pill chips for the
  dev server and the notice's choices, and the page as a rounded card inset
  10px. Fills stay tinted from the screen's own text. The menu is unchanged.

## Settings page follows the BridgeMind settings reference

- Revised: 2026-11-15, the owner approved the Settings redesign mockups
  (Appearance, Models & providers, Threads, Updates) with "Go ahead and apply
  all of those changes."
- Look: Settings fills the window under the toolbar, which stays with its ✕.
  While it is open its own menu takes the sidebar's place: a small bordered
  ‹ Back to app, the heading "Settings", the pages as words (no glyphs), and
  the version ("Interface 0.3.178") at its foot. A page that wants you says so
  beside its name in the menu: "2 need setup" in amber, "1 ready" in the done
  hue. Each page is one centred column at most 760px wide, with no pane inside
  a pane: a large title and a muted subtitle, then groups. Every group is a
  small sentence-case heading over one bordered card, rows divided by
  hairlines. Every row is a name and one short help line on the left and its
  control on the right, with no icon tiles. One switch (no "On"/"Off" word
  beside it; its accessible name and state stay), one bordered menu button
  for every choice, one primary (filled) and one secondary (bordered) button.
  Folder paths, terminal commands, commit hashes, branches and test names sit
  behind a link or disclosure, and a home folder shows as `~`. The pages still
  page by state (all stay mounted, hidden); a ready update opens on Updates. A
  provider's Ready dot is green. Below 840px the menu becomes a strip over the
  page with the pages scrolling sideways. Settings keeps the Refined form
  while it is open. A Privacy page, built from the same rows and placed after
  Updates, was added on 21 Nov 2026 at the owner's request.
- Carried by: `electron/src/renderer/components/SettingsPage.tsx`
  (`SETTINGS_PAGE_CSS`, the menu, Threads, Updates, Shortcuts),
  `components/SettingsControls.tsx` (`SettingsRow`, `SettingsGroup`,
  `SettingsSwitch`), `components/SettingsAppearance.tsx`,
  `components/SettingsProviders.tsx`, `components/ProviderAccountsSection.tsx`,
  `components/RemoteAccessPanel.tsx`, the `.workspace-settings-overlay` rule
  in `theme.css` and its mount beside `<main>` in `App.tsx`.
- Markers: `settings-menu`, `settings-back`, `settings-group-label`, `workspace-settings-overlay {`
- Verification: `SettingsPage.test.ts` pins the menu (Back to app, heading,
  version, badges), the 760px column, the row pattern without tiles, the
  switch without words, the paged sections, Appearance's tabs and Motion
  menu, the providers' Needs setup / Ready groups and the Updates card;
  `whats-new.test.ts` pins What's new; `workspace-ui.test.ts` pins the
  overlay filling the window beside the shell. Seen hidden in Porcelain,
  Porcelain Night and Carbon (Bare CLI) on 2026-11-15.
- Earlier: settled 2026-09-14 from a BridgeMind Settings → Agents reference
  (one section per page); revised 2026-09-24 (one glyph per meaning in the
  rail and row tiles, since removed) and 2026-09-28 (nav and content on one
  sheet; the theme swatch grid; Serif replies as a switch; the green Ready
  dot). Until 2026-11-15 Settings was a floating card beside the live sidebar
  with a 32px icon tile leading every row.

## New agent asks for name, purpose and icon first

- Settled: 2026-09-14, owner asked for Grok Bot's agent creation and approved
  the one-form design.
- Look: in Agent mode with no agent selected, the sidebar button, ⌘N, the
  palette and the empty pane all read New agent and open the agent form instead
  of a plain chat composer; once an agent is selected they return to New task.
  The form asks, in order, for Name, What it does, and Icon: a 64px live preview
  of the character above the shape row and the colour row. Provider, model,
  effort and Permissions (Ask, Auto or Bypass, with a one-line explanation and,
  for Bypass, a warning) sit in a collapsed Advanced section whose summary names
  what new tasks will use ("Codex · Model at launch", plus " · Auto" or
  " · Bypass" when the agent does not ask). The roster's New agent stays
  disabled until the agent store has loaded.
- Revised: 2026-09-26, owner approved adding Permissions to Advanced for agent
  permission modes.
- Carried by: `components/AgentMenu.tsx` (`AgentProfileDialog`), `App.tsx`
  (`agentFirst`, `surfaceNewLabel`).
- Markers: `agent-dialog-advanced`, `agent-dialog-preview`, `const agentFirst = surface === "agent"`
- Verification: `AgentMenu.test.ts` pins the field order and collapsed
  Advanced; `workspace-ui.test.ts` pins every agent-first New; the agent
  workspace DOM fixture opens Advanced before choosing a model; the packaged
  smoke creates the first agent before opening its composer.

## Pane motion — Dev trial

- Authorized: 2026-09-15, owner requested the proposed video-inspired motion on
  Dev to judge it. Final visual acceptance remains pending owner feedback.
- Motion: panes glide between occupied rectangles; resize, maximize and restore
  use 260ms ease-out, swaps 200ms, entry 140ms and exit 120ms. Divider dragging
  stays direct. Reduced Motion immediately applies the final layout.
- Terminal text stays unscaled, terminal buffers and focus stay attached to
  their pane, and rapid reversals begin at the currently visible position.
- Carried by: `components/TerminalPaneMotion.tsx`, `terminal-pane-motion.ts`,
  `terminal-pane-motion.css`, `components/TerminalWorkspace.tsx`.
- Resting geometry, header actions, pane swapping semantics and palettes remain
  as previously approved. This entry records the authorized Dev trial.

## Agent overview beside the panes

- Renamed: 2026-10-14 (review C2), owner approved: the column is Live panes
  in every label (toggle, ··· row, palette, its own name); its tab row reads
  Panes (with the count), Agent tasks (the Agent surface's task list) and
  Browser, and its head counts live panes. Layout, cards and sizes are
  unchanged.

- Extended: 2026-10-14, owner asked for "a browser option in the sidebar" and
  chose a Browser tab that shows a working browser in the column. The tab row
  is Agents, Tasks, Browser (globe), then close; Browser fills the column below
  the tab row with the Browser pane's bar and page card on a 16px inset, and
  keeps its page loaded behind Agents. The Agents view is unchanged.

- Revised: 2026-10-05, owner asked for the right-hand column to be "a little
  less wide" because it took too much room. The column is
  `clamp(360px, 22vw, 560px)` (360px at 1440px windows, 560px on a 2560pt
  display), and the floating sheet at 1080px and below is `min(360px, 92%)`.
  Cards, chips and search are unchanged; the chips still fit one row at 560px.

- Revised: 2026-10-02, owner chose "D · Reference-matched" (https://claude.ai/artifact/KqGMLcj3b1xoGwgLGdKsxS). The
  column is `clamp(420px, 27vw, 680px)`. Cards pad 14px 16px with a 30px icon
  tile, 15px titles (compact rows 13px) and 12px meta; cards sit 10px apart,
  groups 14px. The tab row is 40px, filter chips are 28px at 12px, and search
  and Close idle are 34px.

- Revised: 2026-10-01 (second pass), owner sent a closer BridgeMind screenshot and asked to move "a little closer in some ways". A tab row heads the panel: Agents (with its count) and Tasks (the
  Agent surface's task list), then close. Every card carries its status pill
  on the right ("Working 4m"), names wrap to two lines at 13px, and the
  filter chips are pills. The column is `clamp(420px, 24vw, 600px)` wide, so
  a large display gives it room for one row of chips.

- Revised: 2026-10-01, owner chose A · Dense with B's live cards
  (https://claude.ai/artifact/DD5wZVCHPHqqXPk5h6ufu3). The column is 420px. Its head has the count, a summary and a
  Status / Project grouping switch; the meter has one segment per status in
  the lights' colours; the chips are All / Needs attention / Working / Done /
  Idle; then a search box and Close idle (N), which asks before ending the
  idle agents. Groups add Done (an unseen finished turn). Each card shows the
  agent's mark with its light, name, `Provider · workspace`, how long it has
  been in that status (only when the change was watched), and its last screen
  lines on a screen-coloured strip: three for Needs attention (with an Open
  pane hint and an amber edge), two for Working, the last sentence for Done.
  Idle and stopped agents are one compact line.

- Settled: 2026-09-23, owner supplied the BridgeMind dashboard reference and
  approved the design: a right-side overview of every agent pane, where choosing
  one goes to its workspace and pulses the pane once.
- Look: a sidebar-right toggle beside the Layout button opens a 320px column to
  the right of the panes (closed until first opened; the choice is remembered).
  It heads with the agent count, a one-line summary and a thin working-share
  meter, then All / Needs attention / Working / Idle chips with counts, then
  groups in that urgency order plus Stopped. Rows are outlined cards: provider
  mark, pane name, `Provider · workspace` and a quiet status badge with the
  shared dots (none for idle). Plain shells are not listed. At 1080px and below
  the column floats over the panes and closes after a choice.
- Locate: the chosen pane's edge takes the selection accent for one 900ms fade;
  Reduced Motion holds a still accent ring for the same second. The settled
  neutral focus ring and tile edge are unchanged underneath.
- Carried by: `components/AgentOverview.tsx`, `agent-overview.ts`,
  `terminal.css`, `components/TerminalPane.tsx` (`is-locating`),
  `components/IntegratedTerminalCenter.tsx` (`locateTerminal`).
- Markers: `.agent-overview {`, `agent-overview-row`, `terminal-pane-locate`
- Verification: `agent-overview.test.ts` (grouping, counts, shell exclusion,
  status words and dots); the `agent-overview` DOM lane
  (`INTERFACE_AGENT_OVERVIEW_ONLY=1 npm run test:renderer`) opens it from the
  toolbar, switches workspace from a row, checks focus and one pulse (motion on
  and reduced), and the docked and floating geometry at 1440 and 900px.

## Threads: projects keep a hue, and the open thread wears it

- Settled: 2026-10-06, owner picked "Glow, one level calmer, colored by the
  project" (canvas https://claude.ai/artifact/45jxnLLJVckWPNMQWUcJW4, Color
  page, board 4 "Hued glow"). Every project keeps one of eight light/dark hue
  pairs, chosen from its folder (`project-hue.ts`, `ProjectHue.css`); a plain
  chat is neutral. The Threads pane carries its project's class, and inside
  it the conversation's faint aurora, your message bubbles, inline code
  names, the work line's tinted count chips, Review, the composer's gradient
  edge and glow, and Send all take `--project-h` / `--project-h2`. The New
  thread card takes the chosen project's hue (aurora, picker tile, edge,
  Start); the Changes panel adds file-type badges, a five-square diffstat bar,
  gutter bars and light syntax colours. Status colours never take the hue.
- Revised: 2026-10-07 (live turn, then less motion the same day), owner
  approved: while a turn works the pane head holds a still 2px line of the
  project's pair (the arc that circled the composer's edge and the line's
  sweep were cut when the owner asked for less motion); while the thread
  waits on its owner the composer's edge, glow and that line turn amber. The live turn's rail, running ring, steer row and "steered" chip
  take the hue too. Status colours still never take it.
- Revised: 2026-10-14 (B1, ship a thread's work), owner approved: the
  Changes panel gains a foot (`ThreadShipBar`, `.tship`) for shipping: the
  branch and its upstream, Commit… over exactly the files Rust listed (what
  it leaves out, said), a branch off the default branch first, Push & open
  PR, the pull request with its checks, Fix failing checks, Bring in review
  comments and Merge… behind a confirmation. Neutral ink and the accent
  button; the hue stays where it was.
- Revised: 2026-10-14 (E, B4), owner approved ("Approve all four"): the
  composer's model chip ends in the provider's plan headroom ("38% left",
  "~38% left" once the read is stale) in the dim tier (`.app-select-trailing`),
  its window and reset in the tooltip; a thread Continue with started says
  "Continued from <parent title>" in its head. Neutral ink; the hue stays
  where it was.
- Revised: 2026-10-23 (the Claude-app look), owner approved ("Each theme's
  own colours"): the hued glow goes. No aurora, gradient edge, glow or hued
  fill remains in a Threads pane: your bubbles sit on `--surface-raised`,
  inline code names and the work line's chips are neutral, Review is a quiet
  bordered button, and Send, Start, the head's working line, the thread caret
  and the Changes flash take the theme's accent. The live turn's rail and
  steer row are neutral; its running ring is the working colour. A project's
  hue survives only as an 8px square before its name, in the inbox's line 2,
  the pane head and the New thread card's project chip (a hollow ring for No
  project). Every project still keeps its pair (`project-hue.ts`,
  `ProjectHue.css`), and the Changes panel keeps its file-type badges,
  diffstat bar and syntax colours. Status colours still never take the hue.
- Carried by: `components/ProjectHue.css`, `project-hue.ts`,
  `components/ThreadTranscript.css`, `components/ThreadComposer.css`,
  `components/Composer.css` (`.composer-thread-card`), `syntax-tokens.ts`.
- Markers: `.project-hue-0 {`, `--project-h2: light-dark(`, `.diffstat-bar > i.is-added`

## Threads: a live turn

- Revised: 2026-11-16, owner. The dock's cards take a 14px corner, a hairline
  edge and capsule buttons with the TN Code look; what they say, their amber
  and their one arrival are unchanged. See "Threads: the TN Code look".

- Settled: 2026-10-07, owner approved the running-state canvas
  (https://claude.ai/artifact/VbMD8D6Ng4nFRkmexPzJfR, boards 1–5: "I like
  all of them. Go ahead and build"). In a Threads pane a running turn draws
  itself under its prompt: a header (the Wave, the current step's words from
  `thread-live.ts` — "Editing src/routes/login.ts", held still — and one m:ss
  clock),
  two shimmering bars before any output, then a railed trail of its steps
  (muted check, hued spinner ring, failed mark, amber waiting ring; a step's
  `+N −M` only once its call has a result) interleaved with the streamed
  text, and a four-line peek of what a running edit adds. A steer is a hued
  "You steered" row in the trail, never a new turn. The bottom working strip
  is gone in this layout. When the turn ends the trail folds up into
  "Worked for …" in place (340ms), then the answer, the changes card (rows
  60ms apart) and the controls arrive. Waiting on the owner, the header says
  "Waiting for you" in amber and the request docks above the composer
  ("Run a command?", "Edit a file?", "Write a file?" with its diff; Deny,
  Allow once; ⌘↵/⌘⌫ answer it from an empty composer). Queued follow-ups sit
  in a dashed card above the composer; scrolled away, the Latest capsule
  carries the live step and clock; streamed words settle in (240ms) behind
  a hued caret. Reduced motion shows every end state at once.
- Carried by: `thread-live.ts`, `thread-live-turn.ts`,
  `components/ThreadLiveTurn.tsx`, `components/ThreadTranscript.css`,
  `components/ThreadApprovalDock.tsx`, `thread-frame.ts`,
  `components/ThreadComposer.css`, `components/SteeringQueue.tsx`,
  `components/AgentTile.tsx`, `theme.css` (the live head), `markdown.tsx`
  (settle-in), `components/Conversation.css` (the thread caret).
- Revised: 2026-10-07 (less motion), owner: "lets reduce the motion a little …
  There is just a little too much going on"; chose "Calm the frame" of three
  cuts. Gone: the arc circling the composer's edge, the sweep of the head's
  line (now still), and the shimmer on the step's words in the header, the
  Latest capsule and the inbox. Kept: the Wave, the running step's ring, rows
  rising in, the fold at the end, words settling in, the Needs-you rings.
- Revised: 2026-10-14 (B1), owner approved: the latest turn's changes card
  ends in one quiet ship row (`.thread-ship`): Rust's read in a few words
  and the one next step (Create branch…, Commit…, Push & open PR, Fix
  failing checks, Merge…, or Check again).
- Revised: 2026-10-14 (B4, B5, E), owner approved ("Approve all four"): a
  changes card's PDF, image or Markdown row ends in a quiet Preview
  (`.thread-file-preview`) that opens the file beside the thread in the
  Changes panel's column. Above the composer, second opinions sit as hairline
  cards (`ThreadOpinionCards`, `ThreadOpinion.css`): "Second opinion from
  Codex" with the provider's mark; before anything leaves, what goes out
  ("What is sent") with Ask / Cancel; then the reply on a faint inset,
  "Not sent to Claude Code", with Send to / Dismiss. A provider at its limit
  gets one quiet line above them (`.thread-limit`) with Continue with
  <provider> and that provider's headroom. Nothing continues by itself.
- Revised: 2026-10-23 (the Claude-app look), owner approved: the header
  moves under the trail as one status line: the Pair (`ThinkingMark`), the
  step's words (Thinking until anything shows) and the m:ss clock; waiting,
  an amber dot, "Waiting for you" and the clock in amber; at the finish a
  check and Done. The two shimmering bars go. A step's node on the rail is
  its tool's glyph (read, edit, write, run, plan, search, web, agent, tool),
  and its state sits at the row's right: the running ring, a still amber ring
  and its words while it waits, a failure in the failed colour, a finished
  call's `+N −M` or short note. The steer row sits on the raised tier, and
  the docked request's 30px tile carries its kind's glyph (run, edit, write,
  or a question's comment), amber while it waits. The fold, the settle-in,
  the queue card and the capsule keep their timing. See
  [Threads: the Claude-app look](#threads-the-claude-app-look).
- Markers: `.thread-live-status`, `.thread-step-node`, `thread-dock-card`, `.thread-live-clock`

## Muse Code official mark

- Settled: 2026-10-18. The owner requested the actual Muse Code logo instead of
  the generic M.
- Look: Muse Code uses the official Muse squiggle outline from
  <https://muse.ai/images/favicon/app-squiggle.svg>, with its original square
  canvas, filled in Muse blue: `#0064e0` on light themes and `#0082fb` on dark
  ones, two stops of Muse's official gradient. Muse Code's provider accent
  (sidebar pips, working spark, model chip) uses the same blue.
  The launcher, pane menus and provider icons all use this shared mark. The
  iPhone app's session rows and threads show the same squiggle in `#0082fb`
  (the phone is always dark) instead of the plain terminal glyph.
- Revised: 2026-10-20, owner approved ("Make that change in all places the icon
  appears"): Muse blue replaced the monochrome fill and neutral accent. The
  owner then approved adding the blue mark to the phone ("Yes do both").
- Markers: `data-provider-logo="muse"`, `.provider-logo-muse`, `[data-provider="muse"]`, `ProviderMuse`
- Pin: `electron/src/renderer/components/ProviderLogo.test.ts` —
  `draws Muse Code with the official Muse squiggle in Muse blue`;
  `apps/interface-ios/MacAddressTests.swift` —
  `testMuseCodeSessionsShowTheMuseMarkInMuseBlue`.

## Looping motion budget

- Tightened: 2026-11-15, the owner asked to "optimize this new feature as
  best as you can to reduce its battery life consumption". A moving backdrop
  now also rests on the battery after a minute without key, pointer or wheel
  input, is drawn 15 times a second there instead of 30, and stays still on
  the battery when Settings › Appearance › Still on battery is on (the switch
  shows under Moving backdrops). A step is taken inside the display's own
  frame, a look with nothing to animate runs no timer, and a blur of zero is
  no longer written as `blur(0px)` on the art and on every panel. The three
  examples were redrawn so nothing in them is drawn again on every frame.
  Measured in the hands-free app with two terminal panes open
  (`electron/scripts/handsfree-cost.mjs`): Aurora, Paper Tide and Neon Grid
  27%, 24% and 24% of a core while moving before; 14%, 16% and 12% after and
  10%, 11% and 10% on the battery; the same window resting 6 to 7% before and
  3 to 4% after, where a built-in theme is 2 to 4%.
  Pin `electron/src/renderer/theme-look.test.ts` — `is drawn half as often on
  the battery, and rests there after a minute without input`; `stays still on
  the battery when Settings says so, and moves on the power adapter`.
- Extended: 2026-11-15, owner approved looks ("Approve both") after asking
  that themes "be able to be animated and highly complex". A theme file's
  moving backdrop is the one loop that does not rest after three seconds. It
  is bounded instead: its animations are always paused in CSS and
  `theme-look.ts` steps them 30 times a second, only while Interface is the
  app in front (`data-window-active`), macOS Reduce motion is off and
  Settings › Appearance › Live backgrounds › Motion is not Off (one menu
  since 2026-11-15: On, Still on battery, Off; before, two switches showed
  while a look was painted). Measured in the hands-free app: the Aurora example 51% of
  a core running free, 17% stepped; Paper Tide 19%, Neon Grid 21%; a built-in
  theme 0.1%. Every other loop keeps the budget below. Pin
  `electron/src/renderer/theme-look.test.ts` — `runs only while Interface is
  in front, motion is allowed and Settings leaves it on`; `is drawn thirty
  times a second: each held animation moves on by the time that passed`.
- Settled: 2026-10-22. The owner asked to cut Interface's battery use and chose
  all three recommended options. A hidden-window measurement with a ready
  update and four working agents put Interface at 35–55% CPU, against 10% after
  this change while Interface is in front and 1% while another app is.
- Look: every looping animation (working ticker, Wave, spinners, pulses,
  skeletons, shimmers, the update ring) steps at about 12 frames a second,
  `steps()` per keyframe segment, instead of easing at the display's rate. All
  of them rest while Interface is not the app in front and pick up again on
  return; one-shot motion is untouched. A ready update animates three times and
  rests (see Compact window header). The Effort popover's loops keep their own
  hover gate. Reduced motion is unchanged.
- Carried by: `electron/src/renderer/window-activity.ts` (`data-window-active`
  on the root, from `document.hasFocus()` and visibility, so a Browser pane's
  focus counts as Interface) and `--ambient-play-state` in `theme.css`, read by
  each loop's rule.
- Markers: `--ambient-play-state`, `data-window-active`
- Pin: `electron/src/renderer/window-activity.test.ts` — `steps every loop at
  about 12 frames a second and rests it while Interface is not in front`;
  `lets a ready update say so three times, then rest, and again on return`.
- Extended: 2026-10-23 (the Claude-app look), owner approved: Threads' new
  loops follow the budget. The Pair steps through four five-step segments in
  1.6s (12.5 frames a second) and a running step's ring through eleven in
  0.9s, each with `--ambient-play-state` in its own rule; the inbox's Pair
  also rests while the sidebar is hidden. Reduced motion holds the Pair still.
- Revised: 2026-10-27 (battery), owner chose "Animate, then rest": `steps()`
  never lowered Chromium's frame rate, so one working agent's sidebar ticker
  still held a test window at about 54 frames a second against 21 without it
  (12% CPU against 5%). Every loop now runs for about three seconds of its own
  running time once it starts, then holds the first keyframe of its current
  cycle (a spinner upright, a pulse lit, the ticker's wave frozen mid-step)
  until its element or animation is replaced, which a new working state does.
  Steps, the rest behind other apps and reduced motion are unchanged; the
  Effort popover keeps its hover gate. Carried by
  `electron/src/renderer/loop-rest.ts`, installed by `main.tsx`; pin
  `electron/src/renderer/loop-rest.test.ts` — `lets a loop run about three
  seconds, then holds the first frame of its cycle`.
- Revised: 2026-10-28, owner: "the working indicators next to the name of the
  workspace no longer move"; chose "Loop as before" once a timer-stepped
  version measured no saving. Loops inside the sidebar (the working ticker, the
  dot by a folder's name and its CLI glyph, the Threads inbox's marks) never
  rest: they loop exactly as in 0.3.174, and still rest while Interface is
  behind another app or the sidebar is stowed. Loops outside the sidebar still
  rest after about three seconds. Pin `electron/src/renderer/loop-rest.test.ts`
  — `rests only loops outside the sidebar and leaves the sidebar's to their
  rules`.

## Threads: the TN Code look

- Settled: 2026-11-16. The owner asked: "For threads mode, I think I just
  want to plain old rob the T3 code GUI because it looks really good. Take it
  from the TN code branch that we made." TN Code is the owner's fork of T3
  Code (MIT), which they run with its Apple-style interface, the TN Code
  theme in dark, plain replies and the wide chat width. This replaces the
  Claude-app look below on the Threads surface.
- Look, on the Threads surface only (Code and Agent unchanged), in refined
  chrome; Bare CLI keeps its own face, corners and quoted prompts:
  - Type: the system face (SF) at macOS weights (400, 500, 600; the pane
    title 700), 14px replies and prompts, 13px rows and chips, 11px meta.
  - The list: captions as small mono labels in capitals with the count at the
    right; two-line 52px rows with a 9px corner, led by a 28px tile in the
    project's hue with the thread's initials in white (dim for No project);
    a soft fill on hover and the selection colour's wash when selected. A
    slim shelf row keeps the 8px hue square.
  - The pane head: the short title at 15px bold in the display face over one
    11px line; Changes is a capsule and the more glyph sits on a circle.
  - The transcript: a 72rem lane; your words in an 18px bubble with a tail,
    on the selection colour laid over the ground (10% light, 20% dark); a
    centred stamp over a prompt ("TODAY 4:12 PM", mono capitals, the day
    bolder) on the first prompt and then one an hour or more after the last;
    replies as plain page text; the work line a 12px caption with capsule
    chips; the changes card a grouped list on a soft fill under a hairline
    ring, a hairline between its files, Review a capsule; approvals and the
    dock on a 14px corner with capsule buttons.
  - The composer: a 22px card on the theme's surface under a hairline ring
    and a soft lift; a filled round "+"; the model, effort and access as
    borderless capsules (the model's provider wash is gone); Send and Stop
    as 32px circles.
  - Colour is the theme's own throughout: the two quiet label tiers are
    `--fg-muted` and `--fg-dim` (TN Code uses alphas of its ink; the theme's
    tiers keep each palette's measured contrast), a list row's second line
    and a read row's title stay in the dim tier as before, fills and
    separators are alphas of `--fg-strong`, and every corner and leading
    stays on the radius and leading ramps. In Bare CLI, which drops its
    shadows, the composer card keeps its bordered edge.
- Not copied: TN Code's floating glass sidebar and toolbar, its search pill
  and day sections, the inspector, and any T3 Code name, logo or brand asset.
- Carried by: `theme.css` (the `--tn-*` tokens), `components/ThreadTranscript.css`,
  `components/ThreadComposer.css`, `components/ThreadPaneHead.css`,
  `components/ThreadInbox.tsx` (`ThreadTile`), `components/ThreadTurnParts.tsx`
  (`ThreadStamp`), `thread-transcript.ts` (`threadStampKeys`,
  `threadStampParts`), `thread-inbox.ts` (`threadInitials`). The licence
  notice for what was adapted is `docs/THIRD_PARTY_NOTICES.md`.
- Markers: `--tn-bubble`, `--tn-tail`, `thread-inbox-tile`, `thread-stamp`, `--conv-measure:72rem`
- Verification: `thread-transcript.test.ts` ("stamps the first timed prompt,
  then one an hour or more after the last stamp"), `thread-inbox.test.ts`
  ("shows the first letters of the title's first two words, in capitals"),
  `ThreadInbox.test.ts`, `ThreadComposer.test.ts`, `thread-hue.test.ts`; seen
  in the hands-free app in Porcelain and Porcelain Night beside TN Code's own
  captures.

## Threads: the Claude-app look

- Replaced: 2026-11-16, owner. The Threads surface wears the TN Code look
  now; see "Threads: the TN Code look" above. The icon set (`ThreadGlyph`),
  the Pair (`ThinkingMark`) and the Changes panel's Review board stay as
  below; the spacing, corners, type and colours of the list, the pane head,
  the transcript and the composer are the new entry's.

- Settled: 2026-10-23. The owner asked to revamp Threads "in the image of
  Claude Code", then steered the canvas: the Claude app's Code mode rather
  than the CLI, an original thinking mark instead of the spark, T3 Code's
  roomier spacing kept in Interface's design language, slicker icons and a
  simpler working mark ("the best ones are simple"). They approved the canvas
  (https://claude.ai/artifact/3WjCfpoSjvK1XErohtFZv9, "This is good. Get it
  on dev") and chose each theme's own colours and the Pair.
- Look, on the Threads surface only (Code and Agent unchanged): every colour
  is the theme's own. The pane ground, cards and the composer on `--surface`,
  your words on `--surface-raised`, the accent for Send, Start, focus, the
  caret and the head's working line; a project's hue only as an 8px square
  (see [Threads: projects keep a hue, and the open thread wears it](#threads-projects-keep-a-hue-and-the-open-thread-wears-it)).
  - Icons: one set (`ThreadGlyph`) on a 16px grid with a 1.5 stroke, round
    ends and joins, in `currentColor`: read, edit, write, run, plan, search,
    web, agent, tool, pass, error, warning, changes, branch, pr, merge, copy,
    retry, fork, undo, access, add, send, stop, comment, preview, close, more,
    expand, open, back, down. A codicon stays only where the set has no word
    (Away's coffee among them).
  - The working mark is the Pair (`ThinkingMark`): two beads in the working
    colour that cross over and under. Still, they overlap with the far one at
    half strength: beside the New thread greeting at 36px, and everywhere
    under reduced motion. 16px in the status line, 14px in the Latest
    capsule, 13px in the inbox slot.
  - Roomy spacing: a 736px reading lane with 48px gutters from 640px (24
    below); a 52px pane head; 48px between turns; 32px step rows 2px apart;
    your bubble at 12×16 padding and a radius 4px over the composer's; the
    composer card at 11/7 of it (22px on the default radii); Send and Stop
    as 32px squares; 54px inbox rows. Bare CLI keeps its own measure,
    corners and rhythm. The pane head is two lines, 52px at least (since
    2026-11-15; see the revision below).
  - The pane head (revised 2026-11-15): the thread's short title on its own
    line, then one plain line in the dim tier: the hue square and project,
    "on <branch>", where edits land in words, the account, the pull request
    and lineage, each after a quiet dot. A quiet bordered Changes button with
    its totals and the more glyph stay on the right.
  - Buttons: a group's one deciding button is filled `--fg-strong` with
    `--surface` ink (Allow once, Commit…, the ship row's next step);
    secondary buttons carry a hairline edge; icon buttons are 28px squares.
  - The Changes panel follows the canvas's Review board: its header with the
    file count, totals, diffstat and Stacked / Split; file rows with a
    lower-case type badge, the directory dimmed and the name bold; a card
    per file with its hunk strips; a posted note on a sunken strip, "In the
    prompt".
- Revised: 2026-11-15, owner approved the thread-view mockup ("Go ahead and
  apply all of those changes"): (1) a thread with no title of its own (its
  title is still its first prompt) shows that prompt's first line with the
  white space collapsed, cut near 60 characters on a word with an ellipsis,
  one line that never wraps, the whole title in the tooltip, in the pane head
  and the inbox row alike (`thread-title.ts`); a title the owner set or Claude
  generated shows as it is, Rename still starts from the stored title, and
  generated titles stay off. (2) Under the title one plain line replaces the
  pills: the project with its hue square (a hollow ring and "No project" for a
  plain chat), "on main" (the recorded branch, else the one Rust last read the
  checkout on), and where edits land: "edits land in the project folder"
  (Local), "edits land in a separate worktree" (or "in the worktree <folder>"
  without a branch), "a chat, no files are edited" (No project). The live
  head's line, Needs you, Started by / Switched from and the Changes button
  keep their places. (3) The kept prompt is no longer a "Saved prompt · N
  attachments" strip across the pane: it is one line inside the composer
  card's top edge, `A message you wrote earlier was not sent: "<first
  words>"` (or "may not have finished"), its attachments counted when it has
  any, then Retry or Continue safely, Put it back (the message into the
  draft, its files and folders attached again through the drop path) and
  Discard (asks first, as Forget did); the words open the whole message and
  its previewable attachments (`KeptPromptLine`). It shows only when there is
  something to recover: a message delivered and answered draws no line (the
  mockup's "was kept" line on a finished thread was dropped the same day,
  since it repeated a message the transcript already shows answered). (4) The two blue bars at
  the title's ends were the title button's focus ring, clipped: the button ran
  the head's full height. It now wraps the title alone and draws the app's
  whole ring. Verified in the hands-free app (Porcelain Night, Porcelain, Bare
  CLI) and the `threads` lane (`thread-view-fixture.tsx`).
- Carried by: `components/ThreadGlyph.tsx` and `.css`,
  `components/ThinkingMark.tsx` and `.css`, `components/thread-look.ts`,
  `components/AgentTile.tsx` and `components/ThreadPaneHead.tsx` (the head),
  `thread-title.ts` (the short title), `components/PromptRecoveryPanel.tsx`
  (`KeptPromptLine`),
  `components/ThreadTranscript.css`, `components/ThreadLiveTurn.tsx`,
  `components/ThreadTurnParts.tsx`, `components/ThreadComposer.css`,
  `components/ThreadPaneHead.css`, `components/ThreadInbox.tsx`,
  `components/ThreadDiffPanel.tsx`, `components/ThreadShipBar.tsx`,
  `components/Composer.css` (`.composer-thread-card`),
  `components/ConversationRows.tsx` (the dock's tile).
- Markers: `thread-glyph`, `thinking-mark`, `--conv-measure:736px`
- Pin: `electron/src/renderer/components/ThreadGlyph.test.ts` — `draws every
  icon on the one 16px grid with a 1.5 round stroke, hidden from assistive
  tech`; `moves at the app's stepped rate, rests in the background and holds
  under reduced motion`.

## Live backgrounds: the sidebar is frosted glass

- Settled: 2026-11-16. Over a live background the owner found that the
  sidebar "looks a little weird": the picture showed through it more strongly
  than through the panes, so lines and shapes ran behind the workspace names
  and buttons. Shown four treatments in the hands-free app over Clay Cutouts
  and Kiln (solid, quiet glass, frosted, clear under the list:
  https://claude.ai/artifact/TZxp2cZzKreva7oJ1xCb84), the owner chose: "I
  quite like the frost glass look. Try to build it while being as battery
  friendly as possible."
- Look: in a theme file that draws a backdrop, the docked sidebar is tinted
  glass (the chrome colour at 56%, or at the theme's own panel fill when that
  is higher) over the picture blurred 26px and a little more saturated, so
  only soft colour shows behind the names. Stowed and brought back over the
  work (the peek, the narrow window's drawer) it is solid and no frost is
  drawn; under Reduce transparency likewise. Built-in palettes and theme
  files without a backdrop are unchanged.
- Battery: the blur is not a `backdrop-filter` on the sidebar, which blurs
  what is behind it again on every frame the backdrop moves. It is on a
  second, still copy of the art under the sidebar, which is brought up to the
  moving picture twice a second (once a second on the battery) and once when
  the backdrop comes to rest. Measured in the hands-free app with two terminal
  panes (`electron/scripts/handsfree-cost.mjs`, 8 s runs, of one core): over
  Kiln the plain glass sidebar 20%, this frost 21 to 22%, a `backdrop-filter`
  frost 23%; over Aurora 18%, 20% and 22%. The graphics process, where the
  blur is paid: 6%, 7% and 10 to 11%. Runs differ by a point or two, so the
  frost's own cost is between nothing and two points while the backdrop
  moves, half that on the battery, and nothing while it rests.
- Carried by: `electron/src/renderer/theme-look.ts` (`createThemeFrost`,
  `paintFrost`, `refreshFrost`, `frostRefreshDue`, `pairFrostAnimations`),
  `theme-shader.ts` (`copyTo`, the shader's picture into the copy) and
  `theme-look.css` (the box placed by the sidebar's own with anchor
  positioning, the blurred pane, the sidebar's fill).
- Markers: `.theme-backdrop-frost`, `--look-sidebar-fill`, `--interface-sidebar`
- Pin: `electron/src/renderer/theme-look.test.ts` — `frosts the sidebar from
  a still copy of the picture, not by blurring behind it on every frame`;
  `leaves the sidebar's frosted copy still between refreshes, then brings it
  up to the picture`.
