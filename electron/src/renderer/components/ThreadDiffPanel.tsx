import { Fragment, memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, ReactNode } from "react";
import type { DiffFile, DiffFileStatus, ThreadDiff, ThreadView } from "../../shared/threads";
import type { TurnRestorePoint } from "../conversation-types";
import { displayPath } from "../display-paths";
import { isPreviewableOutput } from "../output-preview";
import {
  diffFileName, diffPathParts, diffQuoteFor, diffQuoteLines, diffRowLine, diffStatusWord,
  diffSummaryText, diffTotals, filePatchState, parsePatch, parseThreadDiffLayout, shownThreadDiffLayout,
  splitDiffFits, splitHunkRows, THREAD_DIFF_LAYOUT_KEY, threadDiffChoiceFromValue,
  threadDiffScopeLabel, threadDiffScopes, threadDiffScopeValue, TRUNCATED_SENTENCE,
  turnUndoAllowed, unchangedLinesText,
  type DiffHunk, type DiffRow, type ParsedPatch, type SplitCell, type ThreadDiffChoice, type ThreadDiffLayout,
  type ThreadDiffQuote,
} from "../thread-diff";
import { syntaxLanguageFor, tokenizeLine, type SyntaxToken } from "../syntax-tokens";
import { AppSelect, type AppSelectOption } from "./AppSelect";
import { ConfirmDialog } from "./ConfirmDialog";
import { Icon } from "./glyphs";
import { DiffstatBar, FileBadge } from "./ProjectHueParts";
import { ThreadGlyph } from "./ThreadGlyph";
import { ThreadShipBar, type ThreadShipBarProps } from "./ThreadShipBar";
import "../codicons.css";
import "../theme.css";

/* ============================================================================
   THE THREAD DIFF PANEL — the right column beside a thread's pane

   The same slot, width and push behaviour as the subagent detail column
   (App renders it inside `.detail-pane`): what a thread changed is read beside
   the transcript that changed it, not over it. Rust answers every diff
   (`turnDiff`, `threadDiff`); this panel only draws the answer it is handed.

   Threads look (owner, 2026-10-23, canvas board "Review"): a head that names
   the panel and holds its tools, then the scope and its totals; a list of the
   changed files (the one in view marked, a click goes to it); and every file
   as a card on the pane's ground, its header sticky, each hunk under a sunken
   strip, its patch read into hunks (thread-diff.ts `parsePatch`) with the
   unchanged lines before each counted. With `onQuote`, a line (Shift for a
   range) takes a comment that goes into the prompt draft; hovering a line
   shows the small accent + that says so. A sent comment stays under its lines
   as a note, "In the prompt", and the quoted lines stay marked, until the
   panel moves to another thread or scope.

   Stacked / Split (owner, 2026-10-09): Split draws each hunk as the old file
   on the left and the new on the right (thread-diff.ts `splitHunkRows`), with
   the same rows, colours and tokens in two columns. Every cell keeps its
   parsed row's index, so selecting, quoting and marking are the stacked
   rules unchanged. The choice is this viewer's, kept in localStorage; a panel
   narrower than `SPLIT_DIFF_MIN_WIDTH` draws Stacked whatever was chosen.

   Colour: theme tokens only, no project hue. Each file leads with its type
   badge and reads folders dimmer than the name; the totals end in the
   five-square diffstat; added and removed rows keep their tints and the 3px
   bar at the left edge; and each line's code is lightly highlighted
   (syntax-tokens.ts, one line at a time), drawn as spans — never as HTML.

   PRESENTATIONAL. Props in, callbacks out: no `window.iface`, no store.
   ========================================================================= */

export type ThreadDiffLoad =
  | { status: "loading" }
  | { status: "ready"; diff: ThreadDiff }
  | { status: "error"; message: string };

/**
 * A turn's Undo, offered beside its diff: one control, one confirmation. A
 * local thread restores through its per-turn checkpoint (App's
 * `restoreTurnCheckpoint`, the transcript Undo's path); a worktree thread
 * through Rust's git restore of the turn's start checkpoint, files only.
 */
export interface ThreadTurnUndo {
  /** The pane's restore point for the turn: its confirmation's sentence. */
  point: TurnRestorePoint;
  /** A restore is already running in this pane: the button waits for it. */
  busy: boolean;
  /** Confirmed: restore the files to before the turn. */
  onUndo: () => void;
}

export interface ThreadDiffPanelProps {
  thread: ThreadView;
  /** The thread's title, for the panel's name. */
  title: string;
  choice: ThreadDiffChoice;
  load: ThreadDiffLoad;
  /** Paths are shown against this root when they are absolute. */
  pathRoot?: string;
  /** Move focus into the panel when it opens (keyboard activation). */
  focusOnOpen?: boolean;
  onChoose: (choice: ThreadDiffChoice) => void;
  onRefresh: () => void;
  onClose: () => void;
  /**
   * A commented line or range for the prompt draft (`formatDiffQuote` writes
   * it). Absent, lines are not selectable.
   */
  onQuote?: (quote: ThreadDiffQuote) => void;
  /**
   * The Undo a turn offers, or null where the pane offers none for it (no
   * restore point). Asked only for a Turn scope on a thread that keeps Undo
   * (`turnUndoAllowed`: a local thread, or a worktree the thread created).
   */
  turnUndo?: (ordinal: number) => ThreadTurnUndo | null;
  /**
   * Shipping this thread's work (protocol 37): commit, branch, push and pull
   * request, checks, review comments and merge, at the panel's foot. Absent,
   * the panel has no ship section.
   */
  ship?: ThreadShipBarProps;
  /** Preview a PDF, image or Markdown file beside the thread (review B5). */
  onPreview?: (path: string) => void;
  onBrowseOutputs?: () => void;
}

/* Corners derive from the radius tokens, so the CLI chrome squares them all:
   cards 12, buttons 8, icon buttons and rows 7, small squares 5. */
export const THREAD_DIFF_PANEL_CSS = `
.tdiff { --tdiff-x:24px; display:flex; flex-direction:column; min-width:0; height:100%; overflow:hidden;
  container-type:inline-size;
  color:var(--fg); background:var(--surface);
  border-left:var(--border-w) solid var(--border); font-family:var(--font-ui); font-size:var(--text-ui); }
@container (max-width: 479px) { .tdiff > * { --tdiff-x:16px; } }
.tdiff:focus { outline:none; }
.tdiff:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
.tdiff-head { display:flex; flex:none; flex-direction:column; gap:4px; padding:10px 12px 8px var(--tdiff-x);
  border-bottom:var(--border-w) solid var(--border); }
.tdiff-head-row { display:flex; flex-wrap:wrap; align-items:center; gap:6px 10px; min-width:0; min-height:30px; }
.tdiff-title { flex:none; margin:0; color:var(--fg-strong); font-size:var(--text-label);
  font-weight:var(--wght-medium); line-height:var(--lh-tight); }
.tdiff-tools { display:flex; flex-wrap:wrap; align-items:center; justify-content:flex-end; gap:6px 10px;
  margin-left:auto; }
.tdiff-scope { flex:0 1 auto; min-width:0; margin-left:-9px; }
.tdiff .tdiff-scope .app-select-trigger { height:28px; padding:0 7px 0 9px; color:var(--fg-dim);
  border-radius:var(--radius-container); font-size:var(--text-body); }
.tdiff .tdiff-scope .app-select-trigger:hover:not(:disabled),
.tdiff .tdiff-scope .app-select-trigger[aria-expanded="true"] { color:var(--fg-strong); background:var(--row-hover); }
.tdiff .tdiff-scope .app-select-value { color:var(--fg); }
.tdiff-total { display:inline-flex; flex:none; align-items:center; gap:8px; color:var(--fg-dim);
  font-size:var(--text-ui); font-variant-numeric:tabular-nums; white-space:nowrap; }
.tdiff-iconbtn { display:inline-flex; flex:none; align-items:center; justify-content:center; box-sizing:border-box;
  width:28px; height:28px; padding:0; color:var(--fg-dim); background:transparent; border:0;
  border-radius:calc(var(--radius-control) * 1.75); cursor:pointer; }
.tdiff-iconbtn:hover:not(:disabled) { color:var(--fg-strong); background:var(--row-hover); }
.tdiff-iconbtn:disabled { color:var(--fg-faint); cursor:default; }
.tdiff-iconbtn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
/* Stacked / Split: two pressed-or-not words in one quiet tray. */
.tdiff-layout { display:inline-flex; flex:none; gap:2px; padding:2px; background:var(--row-hover);
  border-radius:var(--radius-container); }
.tdiff-layout-btn { display:inline-flex; align-items:center; box-sizing:border-box; height:24px;
  padding:0 12px; color:var(--fg-dim); background:transparent; border:0;
  border-radius:calc(var(--radius-control) * 1.5); font:inherit; font-size:var(--text-ui);
  white-space:nowrap; cursor:pointer; }
.tdiff-layout-btn:not([aria-pressed="true"]):hover { color:var(--fg-strong); }
/* Its own hairline: in a theme whose hover wash is its surface (Catppuccin) the tray goes. */
.tdiff-layout-btn[aria-pressed="true"] { color:var(--fg-strong); background:var(--surface);
  box-shadow:inset 0 0 0 var(--border-w) var(--border), var(--composer-lift); }
.tdiff-layout-btn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
@container (max-width: 419px) { .tdiff-layout-btn { padding:0 8px; } }
/* The changed files: one row each, the file in view marked; a click goes to it. */
.tdiff-files { display:flex; flex:none; flex-direction:column; gap:2px; max-height:32%; overflow-y:auto;
  margin:0; padding:8px calc(var(--tdiff-x) - 10px); list-style:none;
  border-bottom:var(--border-w) solid var(--border); }
.tdiff-files > li { min-width:0; }
.tdiff-frow { display:grid; grid-template-columns:32px minmax(0, 1fr) auto; gap:10px; align-items:center;
  box-sizing:border-box; width:100%; padding:7px 10px; color:var(--fg-dim); background:transparent; border:0;
  border-radius:calc(var(--radius-control) * 1.75); font:inherit; font-size:var(--text-body);
  line-height:var(--lh-tight); text-align:left; cursor:pointer; }
.tdiff-frow:hover { background:var(--row-hover); }
/* The hairline keeps the mark where a theme's selection is a lift to its
   surface colour (Light's is #fff, the panel's own ground). */
.tdiff-frow[aria-current="true"] { background:var(--row-selected); box-shadow:inset 0 0 0 var(--border-w) var(--border); }
.tdiff-frow:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
.tdiff-frow-path { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;
  direction:rtl; text-align:left; }
.tdiff-frow-path > bdi { direction:ltr; unicode-bidi:isolate; }
.tdiff-name { color:var(--fg); }
.tdiff-frow[aria-current="true"] .tdiff-name { color:var(--fg-strong); font-weight:var(--wght-medium); }
.tdiff-frow-meta { display:inline-flex; align-items:center; gap:6px; font-size:var(--text-ui); white-space:nowrap; }
.tdiff-inprompt { display:inline-flex; flex:none; color:var(--accent); }
/* The badge as the canvas draws it: a little taller, lower case, one width. */
.tdiff .file-badge { min-width:32px; height:18px; padding:0 3px; border-radius:calc(var(--radius-control) * 1.25);
  font-weight:var(--wght-medium); text-transform:lowercase; }
.tdiff-body { --tdiff-top:20px; position:relative; display:flex; flex:1 1 auto; flex-direction:column; gap:16px;
  min-height:0; overflow-y:auto; padding:var(--tdiff-top) var(--tdiff-x) 24px; background:var(--editor-bg); }
.tdiff-body > * { flex:none; }
.tdiff-body:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
.tdiff-note { display:flex; align-items:center; gap:6px; margin:0; padding:0 2px; color:var(--fg-dim);
  font-size:var(--text-ui); line-height:var(--lh-ui); }
.tdiff-note.tdiff-error { padding:10px 12px; color:var(--danger-fg); background:var(--danger-bg);
  border-radius:var(--radius-container); }
.tdiff-file .tdiff-note { padding:12px 16px; }
/* A file: a card on the pane's ground. clip, not hidden, so the header stays
   sticky; it sticks at the scroller's edge, not inside its top padding. */
.tdiff-file { overflow:clip; background:var(--surface); border:var(--border-w) solid var(--border);
  border-radius:calc(var(--radius-control) * 3); }
.tdiff-fh { position:sticky; top:calc(-1 * var(--tdiff-top)); z-index:1; display:flex; align-items:center; gap:8px;
  box-sizing:border-box; min-height:40px; padding:0 12px 0 0; background:var(--surface); }
.tdiff-file:not(.is-collapsed) > .tdiff-fh { border-bottom:var(--border-w) solid var(--border); }
.tdiff-fh-toggle { display:flex; flex:1 1 auto; align-self:stretch; align-items:center; gap:8px; box-sizing:border-box;
  min-width:0; padding:0 8px 0 14px; color:var(--fg-dim); background:transparent; border:0;
  border-radius:var(--radius-none); font:inherit; font-size:var(--text-ui); text-align:left; cursor:pointer; }
.tdiff-fh-toggle:hover { background:var(--row-hover); }
.tdiff-fh-toggle:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
.tdiff-fh-chevron { color:var(--fg-dim); }
.tdiff-path { flex:0 1 auto; min-width:0; overflow:hidden; color:var(--fg);
  text-overflow:ellipsis; white-space:nowrap; direction:rtl; text-align:left; }
.tdiff-file:not(.is-collapsed) .tdiff-path { font-weight:var(--wght-medium); }
.tdiff-path > bdi { direction:ltr; unicode-bidi:isolate; }
.tdiff-dir, .tdiff-suffix { color:var(--fg-dim); font-weight:var(--wght-regular); }
.tdiff-stat { flex:none; margin-left:auto; font-size:var(--text-ui); font-variant-numeric:tabular-nums; white-space:nowrap; }
.tdiff-add { color:var(--diff-added); }
.tdiff-del { color:var(--diff-removed); }
/* A hunk: its range and the function it sits in, on a sunken strip, and how
   many unchanged lines the patch leaves out above it. */
.tdiff-hunk { display:flex; align-items:center; gap:10px; min-height:24px; box-sizing:border-box;
  margin-bottom:2px; padding:3px 12px 3px 16px; color:var(--fg-dim); background:var(--editor-bg);
  font-family:var(--font-mono); font-size:var(--text-meta); line-height:var(--lh-tight); }
.tdiff-lines > :not(.tdiff-hunk) + .tdiff-hunk { margin-top:2px; }
.tdiff-hunk-range { flex:none; white-space:pre; }
.tdiff-hunk-section { flex:0 1 auto; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:pre; }
.tdiff-hunk-skip { flex:none; margin-left:auto; font-family:var(--font-ui); white-space:nowrap; }
.tdiff-header { margin:0; padding:12px 16px; color:var(--fg-dim); font-family:var(--font-mono);
  font-size:var(--text-ui); line-height:var(--lh-ui); white-space:pre-wrap; overflow-wrap:break-word; }
.tdiff-lines { content-visibility:auto; contain-intrinsic-size:auto 480px; padding-bottom:6px; }
.tdiff-lines > :first-child:not(.tdiff-hunk) { margin-top:2px; }
.tdiff-dl { position:relative; display:grid; grid-template-columns:44px 14px minmax(0, 1fr); align-items:start;
  color:var(--fg); font-family:var(--font-mono); font-size:var(--text-ui); line-height:var(--lh-prose); }
.tdiff-ln { box-sizing:border-box; align-self:stretch; margin:0; padding:1px 10px 0 0;
  color:var(--fg-dim); font-variant-numeric:tabular-nums; text-align:right; user-select:none; }
button.tdiff-ln { position:relative; display:flex; align-items:flex-start; justify-content:flex-end;
  background:transparent; border:0; border-radius:var(--radius-none); font:inherit; cursor:pointer; }
.tdiff-ln:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:calc(-1 * var(--focus-ring-offset)); }
/* The comment +: a small accent square at a quotable line's edge, on hover. */
.tdiff-cplus { position:absolute; top:3px; left:4px; display:inline-flex; align-items:center; justify-content:center;
  width:16px; height:16px; color:var(--accent-fg); background:var(--accent);
  border-radius:calc(var(--radius-control) * 1.25); visibility:hidden; }
.tdiff-cplus > .thread-glyph { stroke-width:2.4px; }
.tdiff-lines.is-quotable .tdiff-dl:hover .tdiff-cplus, .tdiff-ln:focus-visible .tdiff-cplus { visibility:visible; }
.tdiff-mk { padding-top:1px; color:var(--fg-faint); user-select:none; }
.tdiff-code { min-width:0; padding:1px 12px 1px 0; white-space:pre-wrap; overflow-wrap:break-word;
  word-break:break-word; tab-size:4; }
/* The 3px bar at the left edge says added or removed without the marker
   column; a quoted or selected line's focus bar replaces it below. */
.tdiff-dl.is-add { background:var(--diff-added-bg); box-shadow:inset 3px 0 0 var(--diff-added); }
.tdiff-dl.is-add .tdiff-mk { color:var(--diff-added); }
.tdiff-dl.is-del { background:var(--diff-removed-bg); box-shadow:inset 3px 0 0 var(--diff-removed); }
.tdiff-dl.is-del .tdiff-mk { color:var(--diff-removed); }
.tdiff-dl.is-note { color:var(--fg-dim); font-style:italic; }
/* Split: each row is the old file's line beside the new file's, every cell a
   .tdiff-dl of its own, so the tints, edge bars, tokens and selection read
   exactly as stacked. A side with no line is a faint hatch. */
.tdiff-sl { display:grid; grid-template-columns:minmax(0, 1fr) minmax(0, 1fr); }
.tdiff-sl > .tdiff-dl + .tdiff-dl { border-left:var(--border-w) solid var(--border); }
.tdiff-dl.is-empty { background-image:repeating-linear-gradient(-45deg, transparent 0 5px,
  color-mix(in srgb, var(--fg) 6%, transparent) 5px 6px); }
.tdiff-lines.is-quotable .tdiff-dl:not(.is-note):not(.is-empty) { cursor:pointer; }
.tdiff-lines.is-quotable .tdiff-dl.is-context:hover { background:var(--row-hover); }
.tdiff-dl.is-quoted { box-shadow:inset 3px 0 0 color-mix(in srgb, var(--focus) 55%, transparent); }
.tdiff-dl.is-selected { background:color-mix(in srgb, var(--focus) 20%, transparent);
  box-shadow:inset 3px 0 0 var(--focus); }
/* Light syntax colours: light-dark() pairs, each 4.5:1 on the card and on
   the added and removed tints in both kinds of theme (ThreadDiffPanel.test.ts
   measures them). Comments take --fg-dim: --fg-faint is not text. The dark
   function and number shades are a step lighter than they were on the old
   chrome ground (#82aaff, #f78c6c): Catppuccin's card is light for a dark theme. */
.tdiff-tok-keyword { color:light-dark(#7a3fc0, #c9a9f5); }
.tdiff-tok-function { color:light-dark(#2e5cc9, #8ab0ff); }
.tdiff-tok-type { color:light-dark(#0e6f60, #7fd1c8); }
.tdiff-tok-string { color:light-dark(#87580e, #e6c07b); }
.tdiff-tok-number, .tdiff-tok-literal { color:light-dark(#9e4928, #f9977a); }
.tdiff-tok-comment { color:var(--fg-dim); }
.tdiff-tok-punctuation { color:var(--fg-muted); }
/* The comment being written, and the ones sent: strips on the sunken ground
   across the card, under the lines they are about. */
.tdiff-comment, .tdiff-posted { display:flex; align-items:flex-start; gap:12px; padding:12px 16px;
  background:var(--editor-bg); border-top:var(--border-w) solid var(--border);
  border-bottom:var(--border-w) solid var(--border); font-family:var(--font-ui); }
.tdiff-comment + .tdiff-posted, .tdiff-posted + .tdiff-posted { border-top:0; }
.tdiff-comment-glyph { margin-top:9px; color:var(--accent); }
.tdiff-comment-box { flex:1 1 auto; min-width:0; }
.tdiff-comment-input { display:block; box-sizing:border-box; width:100%; min-height:60px; margin:0;
  padding:8px 10px; color:var(--input-fg); background:var(--surface); border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  outline:none; resize:vertical; font:inherit; font-size:var(--text-body); line-height:var(--lh-ui); }
.tdiff-comment-input:focus { box-shadow:inset 0 0 0 var(--border-w) var(--focus), 0 0 0 var(--border-w) var(--focus); }
.tdiff-comment-input::placeholder { color:var(--input-placeholder); }
.tdiff-comment-foot { display:flex; flex-wrap:wrap; align-items:center; gap:8px; margin-top:10px; }
.tdiff-comment-label { flex:1 1 auto; color:var(--fg-dim); font-size:var(--text-ui); white-space:nowrap; }
.tdiff-comment-actions { display:flex; gap:8px; margin-left:auto; }
.tdiff-posted { font-size:var(--text-body); line-height:var(--lh-prose); }
.tdiff-posted-glyph { margin-top:3px; color:var(--accent); }
.tdiff-posted-text { flex:1 1 auto; min-width:0; margin:0; color:var(--fg); white-space:pre-wrap;
  overflow-wrap:anywhere; }
.tdiff-posted-text.is-bare { color:var(--fg-dim); }
.tdiff-posted-tag { display:inline-flex; flex:none; align-items:center; box-sizing:border-box; height:20px;
  padding:0 8px; color:var(--accent-text); background:color-mix(in srgb, var(--accent) 14%, transparent);
  border-radius:var(--radius-pill); font-size:var(--text-meta); font-weight:var(--wght-medium);
  line-height:var(--lh-tight); white-space:nowrap; }
/* Buttons: secondary on a hairline edge; the one deciding button filled in ink. */
.tdiff-btn { display:inline-flex; flex:none; align-items:center; gap:8px; box-sizing:border-box;
  height:30px; padding:0 12px; color:var(--fg); background:transparent; border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  font:inherit; font-size:var(--text-body); font-weight:var(--wght-medium); white-space:nowrap; cursor:pointer; }
.tdiff-btn:hover { color:var(--fg-strong); background:var(--row-hover); }
.tdiff-btn.is-primary { color:var(--surface); background:var(--fg-strong); box-shadow:none; }
.tdiff-btn.is-primary:hover { color:var(--surface); background:color-mix(in srgb, var(--fg-strong) 86%, var(--surface)); }
.tdiff-btn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.tdiff-preview { height:24px; padding:0 8px; gap:6px; color:var(--fg-dim);
  box-shadow:inset 0 0 0 var(--border-w) var(--border); font-size:var(--text-ui); font-weight:var(--wght-regular); }
.tdiff-preview:hover { color:var(--fg-strong); }
.tdiff-kbd { padding:0 4px; color:inherit;
  border:var(--border-w) solid color-mix(in srgb, currentColor 35%, transparent);
  border-radius:var(--radius-control); font-family:var(--font-mono); font-size:var(--text-meta);
  font-weight:var(--wght-regular); line-height:var(--lh-tight); }
.tdiff-sr { position:absolute; width:1px; height:1px; margin:-1px; padding:0; overflow:hidden;
  clip-path:inset(50%); white-space:nowrap; border:0; }
`;

/** A line of one file: its hunk and its row in that hunk. */
interface LineAt { path: string; hunk: number; row: number }

/** A selected line or range, and the comment being written under it. */
interface LineSelection extends LineAt {
  /** The thread and scope it was made in; another target drops it. */
  target: string;
  /** Where a Shift-extension started; `row` is where it ends. */
  anchor: number;
  draft: string;
}

/** A comment that went into the prompt, drawn under the line it ends on. */
interface PostedNote {
  /** The last quoted line's `lineMark`: the note sits under that line. */
  at: string;
  /** "line 14", "lines 13–15 (removed)". */
  lines: string;
  comment: string;
}

/** What one file has given the prompt: how many comments, their lines and their words. */
interface FileMarks { quotes: number; lines: ReadonlySet<string>; notes: readonly PostedNote[] }

/** One file of the panel's target, by path. */
interface FileAt { target: string; path: string }

interface LineHandlers {
  press: (at: LineAt, extend: boolean) => void;
  key: (event: KeyboardEvent<HTMLButtonElement>, at: LineAt) => void;
  focus: (at: LineAt) => void;
  toggle: (path: string) => void;
  draft: (text: string) => void;
  cancel: () => void;
  submit: () => void;
}

const NO_FILES: readonly DiffFile[] = [];
const NO_PATHS: ReadonlySet<string> = new Set();
const NO_MARKS: ReadonlyMap<string, FileMarks> = new Map();
const NO_NOTES: ReadonlyMap<string, readonly PostedNote[]> = new Map();
const MARKER: Record<DiffRow["kind"], string> = { add: "+", del: "−", context: "", note: "" };
const KIND_WORD: Record<DiffRow["kind"], string> = { add: "Added", del: "Removed", context: "", note: "" };
/** What a file's row and header say after its path: a new file is "new", a deleted one "deleted". */
const STATUS_SUFFIX: Partial<Record<DiffFileStatus, string>> = { added: "new", deleted: "deleted" };
const LIST_KEYS = new Set(["ArrowDown", "ArrowUp", "Home", "End"]);

/** "Added line 21", "Removed line 14", "Line 13". */
function lineName(row: DiffRow): string {
  const line = diffRowLine(row) ?? "";
  return KIND_WORD[row.kind] === "" ? `Line ${line}` : `${KIND_WORD[row.kind]} line ${line}`;
}

/** A quoted line's identity across re-reads of the same scope: its side and number. */
function lineMark(row: DiffRow): string {
  return row.kind === "del" ? `-${row.oldLine}` : `+${row.newLine}`;
}

function selectable(row: DiffRow | undefined): boolean {
  return row !== undefined && row.kind !== "note";
}

/** "1 comment", "3 comments". */
function commentCount(count: number): string {
  return `${count} ${count === 1 ? "comment" : "comments"}`;
}

/** A changed-file row's name: its path, how it changed, its counts and what it gave the prompt. */
function fileRowLabel(file: DiffFile, quotes: number): string {
  const parts = [diffFileName(file)];
  if (file.status !== "modified") parts.push(diffStatusWord(file.status).toLowerCase());
  if (!file.binary && (file.added > 0 || file.removed > 0)) parts.push(`+${file.added} −${file.removed}`);
  if (quotes > 0) parts.push(`${commentCount(quotes)} in the prompt`);
  return parts.join(", ");
}

/** "@@ -12,5 +12,7 @@", as git heads the hunk. */
function hunkRange(hunk: DiffHunk): string {
  return `@@ -${hunk.oldStart},${hunk.oldCount} +${hunk.newStart},${hunk.newCount} @@`;
}

/** Where a line is drawn: a stacked row, or the old (left) or new (right) column of a split row. */
type CellSide = "stacked" | "old" | "new";

/** The number a cell shows: a stacked row its own side's, a split column its file's. */
function cellLine(row: DiffRow, side: CellSide): number | null {
  if (side === "old") return row.oldLine;
  return side === "new" ? row.newLine : diffRowLine(row);
}

/* localStorage never throws here: a blocked or missing store just means the
   panel opens Stacked and the choice lasts this run only. */
function readLayout(): ThreadDiffLayout {
  try {
    return parseThreadDiffLayout(window.localStorage.getItem(THREAD_DIFF_LAYOUT_KEY));
  } catch {
    return parseThreadDiffLayout(null);
  }
}

function writeLayout(layout: ThreadDiffLayout): void {
  try {
    window.localStorage.setItem(THREAD_DIFF_LAYOUT_KEY, layout);
  } catch {
    // Storage refused: the choice holds for this run only.
  }
}

function rowsOf(files: readonly DiffFile[], parsed: readonly (ParsedPatch | null)[],
    path: string, hunk: number): readonly DiffRow[] | undefined {
  return parsed[files.findIndex(file => file.path === path)]?.hunks[hunk]?.rows;
}

function selectionSpan(selection: LineSelection): [number, number] {
  return [Math.min(selection.anchor, selection.row), Math.max(selection.anchor, selection.row)];
}

function lineButton(root: HTMLElement, files: readonly DiffFile[], at: LineAt): HTMLButtonElement | null {
  const index = files.findIndex(file => file.path === at.path);
  return root.querySelector<HTMLButtonElement>(`[data-line="${index}:${at.hunk}:${at.row}"]`);
}

/** How far the body's first card rests below its top: the body's own padding. */
function bodyRest(body: HTMLElement): number {
  return Number.parseFloat(getComputedStyle(body).paddingTop);
}

function FileStat({ file }: { file: DiffFile }) {
  if (file.binary || (file.added === 0 && file.removed === 0)) return null;
  return (
    <span className="tdiff-stat">
      <span className="tdiff-add">+{file.added}</span>{" "}
      <span className="tdiff-del">−{file.removed}</span>
    </span>
  );
}

/** A line's code as spans by kind; plain runs stay text. Never HTML. */
function CodeText({ tokens }: { tokens: readonly SyntaxToken[] }) {
  return <>{tokens.map((token, index) => token.kind === "plain" ? token.text
    : <span key={index} className={`tdiff-tok-${token.kind}`}>{token.text}</span>)}</>;
}

function CommentBox({ lines, draft, handlers }: { lines: string; draft: string; handlers: LineHandlers }) {
  const id = useId();
  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      handlers.cancel();
    } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      event.stopPropagation();
      handlers.submit();
    }
  }
  return (
    <div className="tdiff-comment">
      <ThreadGlyph name="comment" size={15} className="tdiff-comment-glyph" />
      <div className="tdiff-comment-box">
        <textarea id={id} className="tdiff-comment-input" rows={2} value={draft}
          placeholder="What should change here?"
          onChange={event => { handlers.draft(event.target.value); }} onKeyDown={handleKeyDown} />
        <div className="tdiff-comment-foot">
          <label className="tdiff-comment-label" htmlFor={id}>Comment on {lines}</label>
          <span className="tdiff-comment-actions">
            <button type="button" className="tdiff-btn" onClick={handlers.cancel}>Cancel</button>
            <button type="button" className="tdiff-btn is-primary" aria-keyshortcuts="Meta+Enter"
              onClick={handlers.submit}>
              Add to prompt<kbd className="tdiff-kbd" aria-hidden="true">⌘↵</kbd>
            </button>
          </span>
        </div>
      </div>
    </div>
  );
}

/** A comment that went into the prompt, under the line it ends on. */
function PostedComment({ note }: { note: PostedNote }) {
  const bare = note.comment === "";
  return (
    <div className="tdiff-posted" role="note" title={`Comment on ${note.lines}, added to the prompt`}>
      <ThreadGlyph name="comment" size={15} className="tdiff-posted-glyph" />
      <p className={`tdiff-posted-text${bare ? " is-bare" : ""}`}>
        <span className="tdiff-sr">Comment on {note.lines}: </span>
        {bare ? "Quoted without a comment." : note.comment}
      </p>
      <span className="tdiff-posted-tag">In the prompt</span>
    </div>
  );
}

interface DiffFileProps {
  file: DiffFile;
  index: number;
  parsed: ParsedPatch | null;
  pathRoot?: string;
  collapsed: boolean;
  marks: FileMarks | undefined;
  /** The selection when it is in this file. */
  selection: LineSelection | null;
  /** `hunk:row` of the line that holds the panel's one line tab stop, when it is here. */
  roving: string | null;
  quotable: boolean;
  /** The layout drawn (the viewer's choice, Stacked when the panel is too narrow for Split). */
  layout: ThreadDiffLayout;
  handlers: LineHandlers;
  /** Preview this file beside the thread (review B5); absent, there is none. */
  onPreview?: (path: string) => void;
}

/** A split row's side with no line: a filler that is not a line, not a target and not read aloud. */
const EMPTY_CELL = <div className="tdiff-dl is-empty" aria-hidden="true" />;

/** One file: its sticky header, then why there is no patch, or its hunks. */
const DiffFileView = memo(function DiffFileView({
  file, index, parsed, pathRoot, collapsed, marks, selection, roving, quotable, layout, handlers,
  onPreview,
}: DiffFileProps) {
  const state = filePatchState(file);
  const [from, to] = selection === null ? [-1, -1] : selectionSpan(selection);
  const selectedRows = selection === null ? undefined : parsed?.hunks[selection.hunk]?.rows;
  const preview = selectedRows === undefined ? null
    : diffQuoteFor(file.path, selectedRows.slice(from, to + 1), "");
  // Each line's colours, read once per patch: a selection redraws the rows, not the reading.
  const language = syntaxLanguageFor(file.path);
  const tokens = useMemo(() => parsed?.hunks.map(hunk => hunk.rows.map(row =>
    row.kind === "note" ? null : tokenizeLine(row.text, language))) ?? [], [parsed, language]);
  // The side-by-side pairing, read once per patch and only when it is drawn.
  const split = useMemo(() => layout === "split" ? parsed?.hunks.map(hunk => splitHunkRows(hunk.rows)) ?? []
    : null, [layout, parsed]);
  // The sent comments by the line they sit under.
  const notesAt = useMemo(() => {
    if (marks === undefined || marks.notes.length === 0) return NO_NOTES;
    const map = new Map<string, PostedNote[]>();
    for (const note of marks.notes) map.set(note.at, [...map.get(note.at) ?? [], note]);
    return map;
  }, [marks]);
  const { dir, name } = diffPathParts(displayPath(diffFileName(file), pathRoot));
  const suffix = STATUS_SUFFIX[file.status];

  /**
   * One line, stacked or in a split column. `r` is the row's index in its
   * hunk wherever it is drawn: the selection, the quote and the marks are the
   * same line's in either layout. A context line's left copy in Split is not
   * a button, so the keyboard meets each line once, on its right.
   */
  function lineCell(row: DiffRow, h: number, r: number, side: CellSide): ReactNode {
    const at: LineAt = { path: file.path, hunk: h, row: r };
    const canSelect = quotable && selectable(row);
    const isButton = canSelect && !(side === "old" && row.kind === "context");
    const selected = selection !== null && selection.hunk === h && r >= from && r <= to;
    const quoted = selectable(row) && marks?.lines.has(lineMark(row)) === true;
    const className = `tdiff-dl is-${row.kind}${selected ? " is-selected" : ""}`
      + `${quoted ? " is-quoted" : ""}`;
    return (
      <div className={className}
        onMouseDown={canSelect ? (event: MouseEvent) => {
          // Shift extends the line selection, not the text selection.
          if (event.shiftKey) event.preventDefault();
        } : undefined}
        onClick={canSelect ? (event: MouseEvent) => {
          // A drag that selected code to copy is not a line pick.
          const text = window.getSelection();
          if (!event.shiftKey && text !== null && !text.isCollapsed) return;
          handlers.press(at, event.shiftKey);
        } : undefined}>
        {isButton ? (
          <button type="button" className="tdiff-ln" data-line={`${index}:${h}:${r}`}
            tabIndex={roving === `${h}:${r}` ? 0 : -1}
            aria-label={lineName(row)} aria-pressed={selected}
            title="Comment on this line (Shift-click for a range)"
            onFocus={() => { handlers.focus(at); }}
            onKeyDown={event => { handlers.key(event, at); }}>
            <span className="tdiff-cplus" aria-hidden="true"><ThreadGlyph name="add" size={10} /></span>
            {cellLine(row, side)}
          </button>
        ) : (
          <span className="tdiff-ln">
            {KIND_WORD[row.kind] !== "" && <span className="tdiff-sr">{KIND_WORD[row.kind]} </span>}
            {cellLine(row, side)}
          </span>
        )}
        <span className="tdiff-mk" aria-hidden="true">{MARKER[row.kind]}</span>
        <span className="tdiff-code">
          {tokens[h]?.[r] == null ? row.text : <CodeText tokens={tokens[h][r]!} />}
        </span>
      </div>
    );
  }

  /** The comment box under a selection: the lines it quotes, and the draft. */
  function commentBox(): ReactNode {
    if (selection === null || preview === null) return null;
    return (
      <CommentBox handlers={handlers} draft={selection.draft}
        lines={`${diffQuoteLines(preview)}${preview.side === "old" ? " (removed)" : ""}`} />
    );
  }

  /** The sent comments under these rows (a split row's two sides), in the order they were sent. */
  function postedUnder(rows: readonly (DiffRow | undefined)[]): ReactNode {
    if (notesAt.size === 0) return null;
    const marksHere = [...new Set(rows.filter(selectable).map(row => lineMark(row!)))];
    const notes = marksHere.flatMap(mark => notesAt.get(mark) ?? []);
    return notes.map((note, k) => <PostedComment key={`${note.at}:${k}`} note={note} />);
  }

  /** A split column's cell, or the filler where that side has no line. */
  function splitCell(cell: SplitCell | null, h: number, side: "old" | "new"): ReactNode {
    return cell === null ? EMPTY_CELL : lineCell(cell.row, h, cell.index, side);
  }

  function body(): ReactNode {
    if (state.kind !== "patch" || parsed === null) {
      return <p className="tdiff-note">{state.kind === "patch" ? "No line changes." : state.sentence}</p>;
    }
    if (parsed.hunks.length === 0) return <pre className="tdiff-header">{parsed.header.join("\n")}</pre>;
    const inSelection = (cell: SplitCell | null): boolean => cell !== null && cell.index >= from && cell.index <= to;
    return (
      <div className={`tdiff-lines${quotable ? " is-quotable" : ""}`}>
        {parsed.hunks.map((hunk, h) => {
          const pairs = split?.[h];
          // Split draws the comment box under the last row that holds a selected line.
          let boxAt = -1;
          if (pairs !== undefined && selection !== null && selection.hunk === h) {
            pairs.forEach((pair, k) => { if (inSelection(pair.left) || inSelection(pair.right)) boxAt = k; });
          }
          return (
            <Fragment key={h}>
              {(hunk.skippedBefore > 0 || hunk.section !== "") && (
                <div className="tdiff-hunk">
                  <span className="tdiff-hunk-range">{hunkRange(hunk)}</span>
                  {hunk.section !== "" && <span className="tdiff-hunk-section">{hunk.section}</span>}
                  {hunk.skippedBefore > 0 && (
                    <span className="tdiff-hunk-skip">{unchangedLinesText(hunk.skippedBefore)}</span>
                  )}
                </div>
              )}
              {pairs === undefined ? hunk.rows.map((row, r) => (
                <Fragment key={r}>
                  {lineCell(row, h, r, "stacked")}
                  {selection !== null && selection.hunk === h && r === to && commentBox()}
                  {postedUnder([row])}
                </Fragment>
              )) : pairs.map((pair, k) => (
                <Fragment key={k}>
                  <div className="tdiff-sl">
                    {splitCell(pair.left, h, "old")}
                    {splitCell(pair.right, h, "new")}
                  </div>
                  {k === boxAt && commentBox()}
                  {postedUnder([pair.left?.row, pair.right?.row])}
                </Fragment>
              ))}
            </Fragment>
          );
        })}
      </div>
    );
  }

  return (
    <section className={`tdiff-file${collapsed ? " is-collapsed" : ""}`} data-file={index}
      aria-label={`Changes in ${file.path}`}>
      <div className="tdiff-fh">
        <button type="button" className="tdiff-fh-toggle" aria-expanded={!collapsed}
          title={diffFileName(file)} onClick={() => { handlers.toggle(file.path); }}>
          <ThreadGlyph name={collapsed ? "open" : "expand"} size={14} className="tdiff-fh-chevron" />
          <span className="tdiff-sr">{diffStatusWord(file.status)} </span>
          <FileBadge path={file.path} />
          <span className="tdiff-path">
            <bdi>{dir !== "" && <span className="tdiff-dir">{dir}</span>}{name}
              {suffix !== undefined && <span className="tdiff-suffix" aria-hidden="true"> · {suffix}</span>}</bdi>
          </span>
          <FileStat file={file} />
        </button>
        {onPreview !== undefined && file.status !== "deleted" && isPreviewableOutput(file.path) && (
          <button type="button" className="tdiff-btn tdiff-preview" aria-label={`Preview ${file.path}`}
            title="Preview this file beside the thread" onClick={() => { onPreview(file.path); }}>
            <ThreadGlyph name="preview" size={14} />Preview
          </button>
        )}
      </div>
      {!collapsed && body()}
    </section>
  );
});

/** One thread's changes, by turn or across the thread, beside its pane. */
export function ThreadDiffPanel({
  thread, title, choice, load, pathRoot, focusOnOpen, onChoose, onRefresh, onClose, onQuote, turnUndo,
  ship, onPreview, onBrowseOutputs,
}: ThreadDiffPanelProps) {
  const rootRef = useRef<HTMLElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const diff = load.status === "ready" ? load.diff : null;
  const files = diff?.files ?? NO_FILES;
  const parsed = useMemo(() => files.map(file => {
    const state = filePatchState(file);
    return state.kind === "patch" ? parsePatch(state.text) : null;
  }), [files]);
  const quotable = onQuote !== undefined;
  // What the panel is looking at. Selections, collapsed files and the marks of
  // what was quoted belong to one target and are dropped on the next.
  const target = `${thread.key}\n${threadDiffScopeValue(choice)}`;
  const [selectionState, setSelection] = useState<LineSelection | null>(null);
  const [activeState, setActive] = useState<(LineAt & { target: string }) | null>(null);
  const [collapsedState, setCollapsed] = useState<{ target: string; paths: ReadonlySet<string> }>(
    { target: "", paths: NO_PATHS });
  const [marksState, setMarks] = useState<{ target: string; files: ReadonlyMap<string, FileMarks> }>(
    { target: "", files: NO_MARKS });
  // The file in view (the list marks it) and the list row holding its one tab stop.
  const [currentState, setCurrent] = useState<FileAt | null>(null);
  const [listStopState, setListStop] = useState<FileAt | null>(null);
  const [confirmingUndo, setConfirmingUndo] = useState<number | null>(null);
  // Stacked or Split, this viewer's choice across reloads; drawn Split only
  // while the panel is wide enough (null: not measured yet, as on the server).
  const [chosenLayout, setChosenLayout] = useState<ThreadDiffLayout>(readLayout);
  const [splitFits, setSplitFits] = useState<boolean | null>(null);
  const layout = shownThreadDiffLayout(chosenLayout, splitFits);
  const chooseLayout = (next: ThreadDiffLayout): void => {
    setChosenLayout(next);
    writeLayout(next);
  };
  const collapsed = collapsedState.target === target ? collapsedState.paths : NO_PATHS;
  const marks = marksState.target === target ? marksState.files : NO_MARKS;
  const known = (at: FileAt | null): string | null =>
    at !== null && at.target === target && files.some(file => file.path === at.path) ? at.path : null;
  const current = known(currentState) ?? files[0]?.path ?? null;
  const listStop = known(listStopState) ?? current;

  // A selection whose lines are no longer drawn is no selection.
  const selection = useMemo(() => {
    const live = selectionState;
    if (!quotable || live === null || live.target !== target || collapsed.has(live.path)) return null;
    const rows = rowsOf(files, parsed, live.path, live.hunk);
    return rows !== undefined && rows[live.anchor] !== undefined && rows[live.row] !== undefined ? live : null;
  }, [collapsed, files, parsed, quotable, selectionState, target]);

  // One tab stop for every line: the last one focused, else the first drawn.
  const roving = useMemo((): LineAt | null => {
    if (!quotable) return null;
    const active = activeState;
    if (active !== null && active.target === target && !collapsed.has(active.path)
        && selectable(rowsOf(files, parsed, active.path, active.hunk)?.[active.row])) return active;
    for (let index = 0; index < files.length; index += 1) {
      if (collapsed.has(files[index].path)) continue;
      const hunks = parsed[index]?.hunks ?? [];
      for (let hunk = 0; hunk < hunks.length; hunk += 1) {
        const row = hunks[hunk].rows.findIndex(selectable);
        if (row >= 0) return { path: files[index].path, hunk, row };
      }
    }
    return null;
  }, [activeState, collapsed, files, parsed, quotable, target]);

  const latest = useRef({ target, files, parsed, selection, onQuote });
  latest.current = { target, files, parsed, selection, onQuote };
  // Focus that has to wait for the render a state change makes: the comment
  // box once it exists, or the line it was about once it has gone.
  const pendingFocus = useRef<"comment" | LineAt | null>(null);
  // A file the list asked for, scrolled to once its card is drawn (and open).
  const pendingScroll = useRef<string | null>(null);
  // Until then, and for a moment after, scrolling does not move the list's mark.
  const spyHeld = useRef(0);
  useLayoutEffect(() => {
    const want = pendingFocus.current;
    const root = rootRef.current;
    if (want !== null && root !== null) {
      pendingFocus.current = null;
      if (want === "comment") root.querySelector<HTMLTextAreaElement>(".tdiff-comment-input")?.focus();
      else lineButton(root, latest.current.files, want)?.focus();
    }
    const path = pendingScroll.current;
    const body = bodyRef.current;
    if (path !== null && body !== null) {
      pendingScroll.current = null;
      const index = latest.current.files.findIndex(file => file.path === path);
      const card = body.querySelector<HTMLElement>(`.tdiff-file[data-file="${index}"]`);
      if (card !== null) {
        spyHeld.current = performance.now() + 250;
        // The first file is the top, with any note above it still in view.
        body.scrollTop = index === 0 ? 0 : card.offsetTop - bodyRest(body);
      }
    }
  });

  // The list marks the file at the top of the scroller: the last card whose top
  // has reached where the first one rests, else the first. Read once a frame
  // while scrolling.
  useEffect(() => {
    const body = bodyRef.current;
    if (body === null) return;
    let frame = 0;
    const read = (): void => {
      frame = 0;
      if (performance.now() < spyHeld.current) return;
      const line = body.scrollTop + bodyRest(body) + 1;
      let found = latest.current.files[0]?.path ?? null;
      for (const card of body.querySelectorAll<HTMLElement>(":scope > .tdiff-file")) {
        if (card.offsetTop > line) break;
        found = latest.current.files[Number(card.dataset.file)]?.path ?? found;
      }
      if (found === null) return;
      const now = latest.current.target;
      const path = found;
      setCurrent(at => at !== null && at.target === now && at.path === path ? at : { target: now, path });
    };
    const onScroll = (): void => { if (frame === 0) frame = requestAnimationFrame(read); };
    body.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      body.removeEventListener("scroll", onScroll);
      if (frame !== 0) cancelAnimationFrame(frame);
    };
  }, []);

  const handlers = useMemo((): LineHandlers => {
    const fresh = (at: LineAt, draft: string): LineSelection =>
      ({ ...at, target: latest.current.target, anchor: at.row, draft });
    return {
      press(at, extend) {
        const now = latest.current.target;
        setSelection(current => {
          const live = current !== null && current.target === now ? current : null;
          if (extend && live !== null && live.path === at.path && live.hunk === at.hunk) {
            return { ...live, row: at.row };
          }
          return fresh(at, live?.draft ?? "");
        });
        setActive({ ...at, target: now });
        pendingFocus.current = "comment";
      },
      key(event, at) {
        const root = rootRef.current;
        if (event.key === "Escape") {
          // A line selection goes first; with none, Escape closes the panel.
          if (latest.current.selection === null) return;
          event.preventDefault();
          event.stopPropagation();
          setSelection(null);
          return;
        }
        if ((event.key !== "ArrowDown" && event.key !== "ArrowUp") || root === null) return;
        event.preventDefault();
        const buttons = [...root.querySelectorAll<HTMLButtonElement>("button.tdiff-ln[data-line]")];
        const next = buttons[buttons.indexOf(event.currentTarget) + (event.key === "ArrowDown" ? 1 : -1)];
        if (next === undefined) return;
        if (event.shiftKey) {
          const [index, hunk, row] = (next.dataset.line ?? "").split(":").map(Number);
          const path = latest.current.files[index]?.path;
          const now = latest.current.target;
          if (path !== undefined) {
            setSelection(current => {
              const live = current !== null && current.target === now && current.path === at.path
                && current.hunk === at.hunk ? current
                : fresh(at, current !== null && current.target === now ? current.draft : "");
              // A range stays inside one hunk: the lines between hunks are not in the patch.
              return path === live.path && hunk === live.hunk ? { ...live, row } : live;
            });
          }
        }
        next.focus();
      },
      focus(at) {
        const now = latest.current.target;
        setActive(current => current !== null && current.target === now && current.path === at.path
          && current.hunk === at.hunk && current.row === at.row ? current : { ...at, target: now });
      },
      toggle(path) {
        const now = latest.current.target;
        setCollapsed(current => {
          const paths = new Set(current.target === now ? current.paths : []);
          if (paths.has(path)) paths.delete(path);
          else paths.add(path);
          return { target: now, paths };
        });
      },
      draft(text) {
        setSelection(current => current === null ? null : { ...current, draft: text });
      },
      cancel() {
        const live = latest.current.selection;
        if (live !== null) pendingFocus.current = { path: live.path, hunk: live.hunk, row: live.anchor };
        setSelection(null);
      },
      submit() {
        const { selection: live, files: shown, parsed: patches, onQuote: quote, target: now } = latest.current;
        if (live === null || quote === undefined) return;
        const rows = rowsOf(shown, patches, live.path, live.hunk) ?? [];
        const [from, to] = selectionSpan(live);
        const quoted = rows.slice(from, to + 1);
        const made = diffQuoteFor(live.path, quoted, live.draft);
        if (made === null) return;
        quote(made);
        const note: PostedNote = { at: lineMark(rows[to]), comment: made.comment,
          lines: `${diffQuoteLines(made)}${made.side === "old" ? " (removed)" : ""}` };
        setMarks(current => {
          const next = new Map(current.target === now ? current.files : NO_MARKS);
          const before = next.get(live.path);
          const lines = new Set(before?.lines ?? []);
          for (const row of quoted) if (selectable(row)) lines.add(lineMark(row));
          next.set(live.path, { quotes: (before?.quotes ?? 0) + 1, lines, notes: [...before?.notes ?? [], note] });
          return { target: now, files: next };
        });
        pendingFocus.current = { path: live.path, hunk: live.hunk, row: live.anchor };
        setSelection(null);
      },
    };
  }, []);

  useEffect(() => {
    if (focusOnOpen === true) rootRef.current?.focus();
  }, [focusOnOpen]);

  // The panel's width decides whether Split fits. Measured before the first
  // paint, so a narrow panel never flashes two columns; kept only as fits /
  // does not fit, so dragging the window redraws nothing until that flips.
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (root === null) return;
    const measure = (): void => { setSplitFits(splitDiffFits(root.clientWidth)); };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => { observer.disconnect(); };
  }, []);

  function handleKeyDown(event: KeyboardEvent<HTMLElement>): void {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    onClose();
  }

  /** A changed-file row: mark the file, open it if it was folded, and scroll its card to the top. */
  function goToFile(path: string): void {
    setCurrent({ target, path });
    setListStop({ target, path });
    if (collapsed.has(path)) handlers.toggle(path);
    pendingScroll.current = path;
  }

  function handleListKeyDown(event: KeyboardEvent<HTMLUListElement>): void {
    if (!LIST_KEYS.has(event.key)) return;
    const rows = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button.tdiff-frow")];
    const at = rows.indexOf(event.target as HTMLButtonElement);
    if (at < 0) return;
    event.preventDefault();
    const step = event.key === "ArrowDown" ? at + 1 : event.key === "ArrowUp" ? at - 1
      : event.key === "Home" ? 0 : rows.length - 1;
    rows[Math.min(rows.length - 1, Math.max(0, step))]?.focus();
  }

  const options = threadDiffScopes(thread, choice).map((entry): AppSelectOption => ({
    value: entry.value,
    label: entry.label,
    triggerLabel: threadDiffScopeLabel(entry),
    group: entry.choice.scope === "turn" ? "Turns" : "Across the thread",
    ...(entry.time === null ? {} : { meta: entry.time }),
    ...(entry.disabled === null ? {} : { disabled: true, description: entry.disabled }),
  }));
  const totals = diff === null ? null : diffTotals(diff);
  const undoFor = (ordinal: number | null): ThreadTurnUndo | null =>
    ordinal === null || turnUndo === undefined || !turnUndoAllowed(thread) ? null : turnUndo(ordinal);
  const undo = undoFor(choice.scope === "turn" ? choice.ordinal : null);
  // A restore point that has gone takes its dialog down with it.
  const confirming = undoFor(confirmingUndo);

  return (
    <aside ref={rootRef} className="tdiff" role="complementary"
      aria-label={`Changes in ${title}`} tabIndex={-1} onKeyDown={handleKeyDown}>
      <style>{THREAD_DIFF_PANEL_CSS}</style>
      <header className="tdiff-head">
        <div className="tdiff-head-row">
          <h2 className="tdiff-title">Changes</h2>
          <span className="tdiff-tools">
            {onBrowseOutputs && <button type="button" className="tdiff-iconbtn" aria-label="Browse outputs"
              title="Browse outputs" onClick={onBrowseOutputs}><ThreadGlyph name="preview" size={15} /></button>}
            {/* The pressed button is the viewer's choice; a narrow panel draws Stacked either way. */}
            <span className="tdiff-layout" role="group" aria-label="Diff layout">
              <button type="button" className="tdiff-layout-btn" aria-label="Stacked"
                aria-pressed={chosenLayout === "stacked"} title="Stacked: one column, removed lines above added"
                onClick={() => { chooseLayout("stacked"); }}>
                Stacked
              </button>
              <button type="button" className="tdiff-layout-btn" aria-label="Split"
                aria-pressed={chosenLayout === "split"}
                title="Split: the old file on the left, the new on the right (a narrow panel stays stacked)"
                onClick={() => { chooseLayout("split"); }}>
                Split
              </button>
            </span>
            {undo !== null && choice.scope === "turn" && (
              <button type="button" className="tdiff-iconbtn" disabled={undo.busy}
                aria-label="Undo turn" title="Restore files to before this message"
                onClick={() => { setConfirmingUndo(choice.ordinal); }}>
                <ThreadGlyph name="undo" size={15} />
              </button>
            )}
            <button type="button" className="tdiff-iconbtn" onClick={onRefresh}
              title="Refresh" aria-label="Refresh changes">
              <ThreadGlyph name="retry" size={15} />
            </button>
            <button type="button" className="tdiff-iconbtn" onClick={onClose}
              title="Close (Esc)" aria-label="Close changes">
              <ThreadGlyph name="close" size={14} />
            </button>
          </span>
        </div>
        <div className="tdiff-head-row">
          <AppSelect className="tdiff-scope" label="Diff scope" value={threadDiffScopeValue(choice)}
            options={options}
            onChange={value => {
              const chosen = threadDiffChoiceFromValue(value);
              if (chosen !== null) onChoose(chosen);
            }} />
          {diff !== null && totals !== null && (
            <span className="tdiff-total" role="status">
              {files.length > 0 && (
                <span aria-hidden="true">{files.length} {files.length === 1 ? "file" : "files"}</span>
              )}
              {(totals.added > 0 || totals.removed > 0) && <>
                <span aria-hidden="true">
                  <span className="tdiff-add">+{totals.added}</span>{" "}
                  <span className="tdiff-del">−{totals.removed}</span>
                </span>
                <DiffstatBar added={totals.added} removed={totals.removed} />
              </>}
              <span className="tdiff-sr">{diffSummaryText(diff)}</span>
            </span>
          )}
        </div>
      </header>

      {files.length > 0 && (
        <ul className="tdiff-files" aria-label="Changed files" onKeyDown={handleListKeyDown}>
          {files.map(file => {
            const { dir, name } = diffPathParts(displayPath(diffFileName(file), pathRoot));
            const quotes = marks.get(file.path)?.quotes ?? 0;
            const suffix = STATUS_SUFFIX[file.status];
            return (
              <li key={file.path}>
                <button type="button" className="tdiff-frow" tabIndex={listStop === file.path ? 0 : -1}
                  aria-current={current === file.path ? "true" : undefined}
                  aria-label={fileRowLabel(file, quotes)} title={diffFileName(file)}
                  onFocus={() => { setListStop({ target, path: file.path }); }}
                  onClick={() => { goToFile(file.path); }}>
                  <FileBadge path={file.path} />
                  <span className="tdiff-frow-path">
                    <bdi>{dir !== "" && <span className="tdiff-dir">{dir}</span>}<span className="tdiff-name">{name}</span>
                      {suffix !== undefined && <span className="tdiff-suffix"> · {suffix}</span>}</bdi>
                  </span>
                  <span className="tdiff-frow-meta">
                    {quotes > 0 && (
                      <span className="tdiff-inprompt"
                        title={`${commentCount(quotes)} on this file added to the prompt`}>
                        <ThreadGlyph name="comment" size={12} />
                      </span>
                    )}
                    <FileStat file={file} />
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <div ref={bodyRef} className="tdiff-body" role="region" aria-label="Diffs" tabIndex={0}
        aria-busy={load.status === "loading" || undefined}>
        {load.status === "loading" && (
          <p className="tdiff-note" role="status">
            <Icon name="loading" sm className="codicon-modifier-spin" /> Loading changes…
          </p>
        )}
        {load.status === "error" && (
          <p className="tdiff-note tdiff-error" role="alert">{load.message}</p>
        )}
        {diff !== null && <>
          {diff.truncated && <p className="tdiff-note">{TRUNCATED_SENTENCE}</p>}
          {files.length === 0 && <p className="tdiff-note">No changes</p>}
          {files.map((file, index) => (
            <DiffFileView key={file.path} file={file} index={index} parsed={parsed[index]}
              pathRoot={pathRoot} collapsed={collapsed.has(file.path)} marks={marks.get(file.path)}
              selection={selection?.path === file.path ? selection : null}
              roving={roving?.path === file.path ? `${roving.hunk}:${roving.row}` : null}
              quotable={quotable} layout={layout} handlers={handlers} onPreview={onPreview} />
          ))}
        </>}
      </div>

      {ship !== undefined && <ThreadShipBar {...ship} />}

      {/* One restore, one way to confirm it, whichever engine restores the files. */}
      {confirming !== null && (
        <ConfirmDialog
          open
          title="Restore files to before this message?"
          description={confirming.point.description}
          confirmLabel="Restore files"
          tone="danger"
          onConfirm={() => {
            setConfirmingUndo(null);
            confirming.onUndo();
          }}
          onCancel={() => { setConfirmingUndo(null); }}
        />
      )}
    </aside>
  );
}
