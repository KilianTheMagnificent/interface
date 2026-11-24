import {
  Component, createRef, type DragEvent as ReactDragEvent, type ReactNode,
  memo, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
} from "react";
import { createPortal } from "react-dom";
import type { EngineId, SessionKey } from "../../shared/engine";
import type { CollaborationEngine } from "../../shared/ipc";
import type { ThreadCommand, ThreadSection } from "../../shared/threads";
import type { DiffStat } from "../diff-stats";
import { productLabel } from "../product";
import { projectHueClass } from "../project-hue";
import type { ThreadLiveFacts } from "../session-activity";
import { threadBridgeWait, type ThreadProviderOffers } from "../thread-opinion";
import { sidebarRowMeta } from "../sidebar-view";
import {
  adjacentThreadKey, AWAY_ROW_WORDS, DEFAULT_THREAD_FOLDS, isThreadSubmenu, parseThreadFolds, SETTLED_PAGE_SIZE,
  IN_MESSAGES_LABEL, snoozeWakeLabel, THREAD_MENU_CHORDS, THREAD_META_LINE, THREAD_SECTION_LABEL, threadChord,
  threadDropBeforeKey, threadDropCommands,
  threadDropUndo, threadInboxOrder, threadMarkWord, threadMenuEntries, threadRowAccessibleName,
  threadRowLine, threadRowSlot, threadRowTrail, threadScopeFor, threadScopeOptions,
  threadScopeRoot, threadSectionAccessibleName, threadSectionFolded, threadSectionRows,
  threadActionLeavesSection, threadFocusAfterRemoval, threadInitials, threadPullRequestFact, threadShelfSlot,
  threadSidebarRow, threadTerminalRefusal, threadTitleTone, threadTracksPullRequest, threadUndo,
  threadUndoCommands, threadUndoMessage,
  type ThreadDropTarget, type ThreadInboxRow, type ThreadInboxSection, type ThreadMark,
  type ThreadMenuCommand, type ThreadMenuEntry, type ThreadRowLine, type ThreadRowSlot, type ThreadRowTrail,
  type ThreadSectionFolds, type ThreadShelfSlot, type ThreadTeammate, type ThreadUndo, type ThreadUndoAction,
} from "../thread-inbox";
import type { ThreadInboxController } from "../use-thread-inbox";
import { useCommittedActions } from "../use-stable-value";
import { AppSelect, positionAppSelect, type AppSelectPosition } from "./AppSelect";
import { ConfirmDialog } from "./ConfirmDialog";
import { ProductName } from "./ProductName";
// The `.project-hue-N` classes the hue square reads.
import "./ProjectHue.css";
import { ProviderLogo } from "./ProviderLogo";
import {
  positionSessionPeek, SessionPeek, type SessionPeekFacts, type SessionPeekPosition,
} from "./SessionPeek";
import type { SidebarLifecycle } from "./Sidebar";
import { ThinkingMark } from "./ThinkingMark";
import { ThreadGlyph } from "./ThreadGlyph";
import {
  FOREGROUND_DIALOG_SELECTOR, menuKeyNav, roveMenuTabIndex, SESSION_PEEK_HOVER_MS,
  SESSION_PEEK_LEAVE_MS, sessionConfirmDialog, type SessionConfirmKind, useAnchoredOverlay,
} from "./sidebar-controls";

/*
 * The Threads sidebar (owner, 2026-10-06): one flat inbox across projects in
 * four sections drawn by the Sidebar on the Threads surface only. Pinned and
 * Active sit at the top under their captions, in two-line rows; Snoozed
 * and Settled are shelves pushed to the bottom, in slim rows. Rust decides
 * every section, order and unread flag; this draws
 * `ThreadInboxController.model` and sends commands.
 */

/** The fold state is a renderer preference, like the project folds. */
export const THREAD_FOLDS_KEY = "interface.threads.folds";
/** What a thread row carries while it is dragged. */
export const THREAD_DRAG_MIME = "application/x-interface-thread";
/** How long the Undo toast stays. */
export const THREAD_UNDO_MS = 6000;

// The Threads look (owner, 2026-11-16, in the image of TN Code, their fork of
// T3 Code with its Apple-style interface; the Code mode look of 2026-10-23
// before it): section captions as small mono labels with the count at the
// right, two-line rounded rows led by a 28px tile in the project's hue with
// the thread's initials, the selection colour as the selected row's wash, and
// the shelves as slim folds under one hairline. A row's line-one slot says
// ONE thing and gives its box to Snooze and Settle on hover or keyboard
// focus, so nothing shifts. The measurements are adapted from TN Code's
// sidebar sheets (see ThreadTranscript.css); the --tn-* tokens are in
// theme.css.
const THREAD_INBOX_CSS = `
.sidebar-section-label:has(+ .thread-scope-picker) { flex: none; }
.sidebar .thread-scope-picker { flex: 0 1 auto; min-width: 0; max-width: 132px; margin: 0 auto 0 4px; }
/* The system face at macOS weights, in refined chrome; a console keeps its own. */
:where(:root:not([data-chrome="cli"])) .thread-inbox {
  --font-ui: var(--tn-font); --wght-regular: 400; --wght-medium: 500; --wght-strong: 600;
  font-family: var(--font-ui);
}
.thread-inbox-section { display: flex; flex: none; flex-direction: column; min-width: 0; }
/* A caption 12px under what is above it: a small mono label in capitals. The
   whole caption is the fold: its chevron shows on hover and keyboard focus,
   and stays while folded. */
.thread-inbox-heading {
  margin: 12px 0 2px; font-family: var(--tn-font-mono); font-size: var(--text-meta);
  font-weight: var(--wght-medium); letter-spacing: var(--track-caps); text-transform: uppercase;
}
/* The first sits closer, under the header row. */
.thread-inbox-section:first-child > .thread-inbox-heading { margin-top: 6px; }
.thread-inbox-fold {
  display: flex; align-items: center; gap: 4px; width: 100%; min-width: 0; height: 22px; padding: 0 12px;
  color: var(--tn-label-3); font: inherit; letter-spacing: inherit; line-height: var(--lh-tight);
  text-align: left; text-transform: inherit;
  background: transparent; border: 0; border-radius: calc(var(--radius-container) + 1px); cursor: pointer;
}
.thread-inbox-fold:hover { color: var(--fg-strong); }
.thread-inbox-fold:focus-visible { outline: var(--focus-ring-w) solid var(--focus); outline-offset: calc(-1 * var(--focus-ring-offset)); }
.thread-inbox-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.thread-inbox-chevron { opacity: 0; transition: transform var(--motion-base) var(--ease-out), opacity var(--motion-fast) linear; }
.thread-inbox-fold:hover .thread-inbox-chevron, .thread-inbox-fold:focus-visible .thread-inbox-chevron,
.thread-inbox-chevron.is-folded { opacity: 1; }
.thread-inbox-chevron.is-folded { transform: rotate(-90deg); }
.thread-inbox-fold-end { display: inline-flex; flex: none; align-items: center; gap: 6px; margin-left: auto; }
.thread-inbox-count { font-weight: var(--wght-regular); font-variant-numeric: tabular-nums; }
/* In messages (protocol 56): a caption like Active's that does not fold. */
.thread-inbox-fold.thread-inbox-found-head { box-sizing: border-box; cursor: default; }
.thread-inbox-fold.thread-inbox-found-head:hover { color: var(--fg-dim); }
/* The shelves (Snoozed, Settled, From terminals) sit at the foot of the list
   under one hairline, 18px clear of the rows above it: slim folds, "Snoozed ·
   2", with the chevron at the right that turns down while open. */
.thread-inbox-shelves {
  position: relative; display: flex; flex: none; flex-direction: column; gap: 2px; min-width: 0;
  margin-top: auto; padding-top: 27px;
}
.thread-inbox-shelves::before {
  content: ""; position: absolute; top: 18px; right: 0; left: 0; height: var(--border-w); background: var(--border);
}
.thread-inbox-shelf-head {
  margin: 0; font-family: var(--font-ui); font-size: var(--text-meta); font-weight: var(--wght-strong);
  letter-spacing: 0; text-transform: none;
}
.thread-inbox-shelf-head + .session-items { margin-top: 2px; }
.thread-inbox-fold.thread-inbox-shelf { height: 30px; border-radius: calc(var(--radius-container) + 1px); }
.thread-inbox-fold.thread-inbox-shelf[aria-expanded="true"] { color: var(--tn-label-2); }
.thread-inbox-fold.thread-inbox-shelf:hover { color: var(--fg-strong); background: var(--tn-fill); }
.thread-inbox-shelf .thread-inbox-chevron { opacity: 1; }
.thread-inbox-shelf .thread-inbox-chevron.is-open { transform: rotate(90deg); }
/* Two-line rounded rows, 52px tall and 2px apart, led by the thread's tile:
   a soft fill on hover, the selection colour's wash when selected. */
.thread-inbox .session-items > li + li { margin-top: 2px; }
.sidebar .session-row.thread-inbox-row {
  position: relative;
  display: grid; grid-template-columns: minmax(0, 1fr) auto; grid-template-rows: 18px 14px;
  column-gap: 6px; row-gap: 2px; align-content: center; align-items: center;
  height: 52px; padding: 0 10px 0 46px; font-size: var(--text-body); font-weight: var(--wght-medium);
  line-height: var(--lh-tight); border-radius: calc(var(--radius-container) + 1px);
}
.sidebar .session-row.thread-inbox-row.is-slim {
  display: flex; gap: 8px; height: 32px; padding-left: 12px; font-size: var(--text-body);
}
.sidebar .thread-inbox .session-row.thread-inbox-row:hover:not(.is-selected) { background: var(--tn-fill); }
.sidebar .thread-inbox .session-row.thread-inbox-row.is-selected { background: var(--tn-row-selected); }
/* The thread's tile: 28px, in its project's hue under a soft gloss, with the
   thread's initials in white; a plain chat's is the dim tier. */
.thread-inbox-tile {
  position: absolute; top: 50%; left: 9px; display: grid; place-items: center; box-sizing: border-box;
  width: 28px; height: 28px; margin-top: -14px; color: var(--surface); border-radius: var(--radius-container);
  background: linear-gradient(180deg, color-mix(in srgb, var(--project-h) 82%, white), var(--project-h));
  box-shadow: inset 0 0 0 .5px color-mix(in srgb, black 12%, transparent),
    inset 0 1px 0 color-mix(in srgb, white 28%, transparent);
  font-size: var(--text-meta); font-weight: var(--wght-heavy); line-height: 1;
}
.thread-inbox-tile:not(.project-hue-none) { color: white; }
.thread-inbox-tile.project-hue-none {
  background: linear-gradient(180deg, color-mix(in srgb, var(--fg-dim) 82%, white), var(--fg-dim));
}
.thread-inbox-title { grid-column: 1; grid-row: 1; color: var(--fg-strong); font-weight: var(--wght-medium); }
.thread-inbox-away { margin-inline-end: 4px; color: var(--fg-dim); vertical-align: -2px; }
.thread-inbox-row.is-unread .thread-inbox-title { font-weight: var(--wght-strong); }
.thread-inbox-row.is-read .thread-inbox-title { color: var(--tn-label-3); }
.thread-inbox-row.is-slim .thread-inbox-title { color: var(--tn-label-2); font-weight: var(--wght-regular); }
.thread-inbox-row.is-selected .thread-inbox-title { color: var(--fg-strong); }
.thread-inbox-slot {
  grid-column: 2; grid-row: 1; display: inline-flex; flex: none; align-items: center; justify-content: flex-end;
  gap: 6px; min-width: 40px; font-size: var(--text-meta); font-weight: var(--wght-regular);
  font-variant-numeric: tabular-nums; white-space: nowrap;
}
.thread-inbox-row.is-slim .thread-inbox-slot { min-width: 24px; }
.thread-inbox-slot.is-age, .thread-inbox-slot.is-wake, .thread-inbox-slot.is-working { color: var(--tn-label-3); }
.thread-inbox-slot.is-woke { color: var(--fg-muted); }
/* Working: no pill, so it never reads like Needs you beside it. The Pair at
   13px, then the run's clock in the slot's own figures. It rests with the
   sidebar hidden. */
.app-grid.no-sidebar > .sidebar .thinking-mark circle { animation-play-state: paused; }
/* Unread: the done blue. It pops in when a row already on screen finishes unread. */
.thread-inbox-unread { flex: none; width: 7px; height: 7px; border-radius: 50%; background: var(--status-done); }
.thread-inbox-unread.is-arriving { animation: thread-inbox-pop 240ms cubic-bezier(.34, 1.56, .64, 1); }
@keyframes thread-inbox-pop { 0% { transform: scale(.3); } 60% { transform: scale(1.18); } 100% { transform: scale(1); } }
.thread-inbox-pill {
  display: inline-flex; flex: none; align-items: center; height: 18px; padding: 0 8px; box-sizing: border-box;
  border-radius: var(--radius-pill); font-size: var(--text-meta); font-weight: var(--wght-strong); line-height: 1;
  white-space: nowrap;
}
/* Needs you is the one filled pill: the amber, lifted to a light tone where a
   paper palette's amber is a deep text colour, under a deep ink of its own hue
   (4.5:1 in every palette; ThreadInbox.test.ts measures it). It rings twice
   as it arrives, then holds still. */
.thread-inbox-pill.is-needs-you {
  --thread-inbox-waiting: var(--status-waiting);
  --thread-inbox-amber: oklch(from var(--thread-inbox-waiting) max(l, 0.8) c h);
  color: oklch(from var(--thread-inbox-waiting) 0.27 calc(c * 0.7) h);
  background: var(--thread-inbox-amber);
  animation: thread-inbox-ring 1.1s var(--ease-out) 2;
}
@keyframes thread-inbox-ring {
  from { box-shadow: 0 0 0 0 color-mix(in srgb, var(--thread-inbox-amber) 60%, transparent); }
  to { box-shadow: 0 0 0 6px transparent; }
}
/* Failed is an outline in the failed colour, so it differs from Needs you by
   shape as well as hue. Its words lean a fifth toward the strongest tier to
   keep 4.5:1 on a hovered or selected row. */
.thread-inbox-pill.is-failed {
  color: color-mix(in srgb, var(--status-failed) 80%, var(--fg-strong)); background: transparent;
  border: var(--border-w) solid var(--status-failed); font-weight: var(--wght-medium);
}
/* A folded caption's or a slim row's one mark: Needs you a dot in the
   sidebar's amber (.session-wait), Failed the error glyph, Working the Pair. */
.thread-inbox-mark { display: inline-flex; flex: none; align-items: center; justify-content: center; }
.thread-inbox-mark-dot { flex: none; width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
.thread-inbox-woke { color: var(--fg-muted); }
.thread-inbox-meta {
  grid-column: 1; grid-row: 2; display: flex; align-items: center; gap: 6px; min-width: 0; overflow: hidden;
  color: var(--tn-label-3); font-size: var(--text-meta); font-weight: var(--wght-regular); white-space: nowrap;
}
/* The one place a project's hue survives (owner, 2026-10-23): an 8px square
   before its words, a hollow ring for No project; half strength on a shelf. */
.thread-inbox-hue {
  flex: none; width: 8px; height: 8px; border-radius: calc(var(--radius-control) - 1.5px); background: var(--project-h);
}
.thread-inbox-hue.project-hue-none { border-radius: 50%; background: transparent; box-shadow: inset 0 0 0 1.5px var(--fg-faint); }
.thread-inbox-row.is-slim .thread-inbox-hue { opacity: 0.5; }
/* Line two's words: project · branch · the pull request or the diff, or a
   live thread's own line in their place, all in the dim tier (the slot says
   the status). A live line that ends fades out over the project line. */
.thread-inbox-words {
  position: relative; display: flex; flex: 1 1 auto; align-items: center; gap: 4px; min-width: 0; overflow: hidden;
}
.thread-inbox-words.is-entering > :not(.thread-inbox-leaving) { animation: thread-inbox-fade-in var(--motion-base) linear; }
.thread-inbox-leaving {
  position: absolute; inset: 0; display: flex; align-items: center; gap: 4px; min-width: 0; pointer-events: none;
  animation: thread-inbox-fade-out var(--motion-base) linear forwards;
}
@keyframes thread-inbox-fade-in { from { opacity: 0; } }
@keyframes thread-inbox-fade-out { to { opacity: 0; } }
.thread-inbox-line { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* Reduced motion: nothing rings, pops or fades. */
@media (prefers-reduced-motion: reduce) {
  .thread-inbox-pill.is-needs-you, .thread-inbox-unread.is-arriving, .thread-inbox-leaving,
  .thread-inbox-words.is-entering > * { animation: none; }
}
.thread-inbox-sep { flex: none; }
.thread-inbox-project { flex: 0 0 auto; max-width: 60%; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.thread-inbox-branch { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.thread-inbox-diff { display: inline-flex; flex: none; gap: 4px; font-variant-numeric: tabular-nums; }
.thread-inbox-added { color: var(--diff-added); }
.thread-inbox-removed { color: var(--diff-removed); }
/* The pull request: its glyph and number in its state's colour, the ink
   leaning 15% toward the strongest tier so it keeps 4.5:1 on any row state
   (ThreadInbox.test.ts measures it). */
.thread-inbox-pr {
  --thread-inbox-pr: var(--fg-dim);
  display: inline-flex; flex: none; align-items: center; gap: 3px;
  color: color-mix(in srgb, var(--thread-inbox-pr) 85%, var(--fg-strong)); font-variant-numeric: tabular-nums;
}
.thread-inbox-pr.is-open { --thread-inbox-pr: var(--diff-added); }
.thread-inbox-pr.is-merged { --thread-inbox-pr: var(--review-accent); }
/* The provider's identity ends line two: its own mark, in its brand colour. */
.thread-inbox-trail { grid-column: 2; grid-row: 2; display: inline-flex; align-items: center; justify-content: flex-end; }
.thread-inbox-provider { flex: none; }
.thread-inbox-actions {
  position: absolute; z-index: 1; top: 6px; right: 6px; display: flex;
  opacity: 0; transition: opacity var(--motion-fast) linear;
}
.thread-inbox-item.is-slim .thread-inbox-actions { top: 5px; }
/* The hover actions' own hover is a wash over whatever fill the row has. */
.thread-inbox-action {
  display: inline-flex; align-items: center; justify-content: center; width: 24px; height: 24px; padding: 0;
  color: var(--tn-label-2); background: transparent; border: 0; border-radius: 50%;
  cursor: pointer;
}
.thread-inbox-action:hover, .thread-inbox-action[aria-expanded="true"] {
  color: var(--fg-strong); background: color-mix(in srgb, var(--fg-strong) 8%, transparent);
}
.thread-inbox-action:focus-visible { outline: var(--focus-ring-w) solid var(--focus); outline-offset: calc(-1 * var(--focus-ring-offset)); }
.thread-inbox-item:hover .thread-inbox-actions,
.thread-inbox-item:has(:focus-visible) .thread-inbox-actions,
.thread-inbox-item:has(.thread-inbox-action[aria-expanded="true"]) .thread-inbox-actions { opacity: 1; }
.thread-inbox-item:hover .thread-inbox-slot,
.thread-inbox-item:has(:focus-visible) .thread-inbox-slot,
.thread-inbox-item:has(.thread-inbox-action[aria-expanded="true"]) .thread-inbox-slot { visibility: hidden; }
.thread-inbox-item.is-full > .session-rename { height: 52px; }
.thread-inbox-item.is-slim > .session-rename { height: 32px; }
.thread-inbox-item.is-dragging { opacity: 0.45; }
.thread-inbox-item.is-drop-before::after,
.thread-inbox-item.is-drop-after::after {
  content: ""; position: absolute; z-index: 1; left: 4px; right: 4px; height: 2px;
  background: var(--accent); pointer-events: none;
}
.thread-inbox-item.is-drop-before::after { top: -1px; }
.thread-inbox-item.is-drop-after::after { bottom: -1px; }
.thread-inbox-section.is-drop-zone > .session-items {
  background: color-mix(in srgb, var(--accent) 8%, transparent); border-radius: var(--radius-container);
}
.thread-inbox-drop-hint { padding: 6px 8px; color: var(--fg-dim); font-size: var(--text-ui); list-style: none; }
.thread-inbox .session-show-more { padding-left: 0; }
.thread-inbox-toast {
  display: flex; flex: none; align-items: center; gap: 8px; margin: 0 8px 6px; padding: 4px 4px 4px 10px;
  color: var(--fg); background: var(--surface-raised); border: var(--border-w) solid var(--border-strong);
  border-radius: var(--radius-container); font-size: var(--text-ui);
}
.thread-inbox-toast-text { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.thread-inbox-toast-undo {
  display: inline-flex; flex: none; align-items: center; gap: 6px; height: 24px; padding: 0 8px;
  color: var(--fg-strong); background: transparent;
  border: var(--border-w) solid var(--border-strong); border-radius: var(--radius-control);
  font: inherit; cursor: pointer;
}
.thread-inbox-toast-undo:hover { background: var(--row-hover); }
.thread-inbox-toast-undo:focus-visible { outline: var(--focus-ring-w) solid var(--focus); outline-offset: var(--focus-ring-offset); }
.row-menu-item.thread-menu-item { display: flex; align-items: center; gap: 8px; }
.thread-menu-trail { margin-left: auto; color: var(--fg-dim); }
.thread-menu-key {
  margin-left: auto; padding-left: 12px; color: var(--fg-dim);
  font-family: var(--font-mono); font-size: var(--text-meta);
}
.thread-inbox-toast-undo > .thread-menu-key { margin-left: 0; padding-left: 0; }
.thread-menu-heading {
  padding: 6px 10px 4px; color: var(--fg-dim); font-size: var(--text-meta); font-weight: var(--wght-strong);
  letter-spacing: var(--track-caps); text-transform: uppercase;
}
.thread-menu-check { flex: none; width: var(--icon-size-sm); }
`;

function readFolds(): ThreadSectionFolds {
  try {
    return parseThreadFolds(localStorage.getItem(THREAD_FOLDS_KEY));
  } catch {
    return DEFAULT_THREAD_FOLDS;
  }
}

function writeFolds(folds: ThreadSectionFolds): void {
  try {
    localStorage.setItem(THREAD_FOLDS_KEY, JSON.stringify(folds));
  } catch {
    // Storage refused: the fold holds for this run only.
  }
}

/** A submenu's items: plain commands, or the radio pair Auto-settle offers. */
const SUBMENU_ITEMS = '[role="menuitem"]:not(:disabled), [role="menuitemradio"]:not(:disabled)';

/**
 * Focus a placed menu's item once it is visible. Placement removes the
 * menu's visibility:hidden, but the item can still compute hidden for a frame,
 * and a hidden element refuses focus. Bounded, and cancelled with the menu.
 */
function focusWhenShown(find: () => HTMLElement | null | undefined): () => void {
  const deadline = performance.now() + 1000;
  let frame = 0;
  const attempt = (): void => {
    const item = find();
    if (item !== null && item !== undefined && getComputedStyle(item).visibility === "visible") item.focus();
    else if (performance.now() < deadline) frame = requestAnimationFrame(attempt);
  };
  attempt();
  return () => { cancelAnimationFrame(frame); };
}

/** Snoozed, Settled and From terminals are shelves at the foot of the list. */
function isShelf(section: ThreadSection): boolean {
  return section === "snoozed" || section === "settled" || section === "fromTerminals";
}

function isThreadDrag(transfer: DataTransfer | null): boolean {
  return transfer !== null && Array.from(transfer.types).includes(THREAD_DRAG_MIME);
}

function editingText(element: Element | null): boolean {
  return element instanceof HTMLElement && (element.isContentEditable
    || element.matches("input, textarea, select"));
}

/**
 * A submenu beside the item that opened it: to the right of the menu when it
 * fits, else to its left; level with the item, clamped to the window.
 */
export function positionThreadSubmenu(anchor: { top: number; left: number; right: number },
    size: { width: number; height: number },
    viewport: { width: number; height: number }): { top: number; left: number } {
  const gutter = 8;
  const fitsRight = anchor.right + 4 + size.width <= viewport.width - gutter;
  const left = fitsRight ? anchor.right + 4 : Math.max(gutter, anchor.left - 4 - size.width);
  const top = Math.max(gutter, Math.min(anchor.top - 4, viewport.height - gutter - size.height));
  return { top, left };
}

/* ---- glyphs ----------------------------------------------------------------
   The Threads icon set (ThreadGlyph) has no word for an alarm clock or the sun
   up, so the inbox draws those two in its grammar: the 16px grid, a 1.5 stroke,
   round ends and joins, `color` inherited. Snooze and Back are the alarm clock,
   Wake now the sun. Decorative: the control or row that owns one carries the
   words. */

const INBOX_GLYPHS = {
  alarm: <><circle cx="8" cy="8.75" r="5" /><path d="M8 6.25v2.5l1.5 1.5M3.25 2.25 1.75 3.75M12.75 2.25l1.5 1.5" /></>,
  wake: <><circle cx="8" cy="8" r="2.75" /><path d="M8 1.75v1.5M8 12.75v1.5M1.75 8h1.5M12.75 8h1.5M3.58 3.58l1.06 1.06M11.36 11.36l1.06 1.06M3.58 12.42l1.06-1.06M11.36 4.64l1.06-1.06" /></>,
} as const;

function InboxGlyph({ name, size }: { name: keyof typeof INBOX_GLYPHS; size: number }): ReactNode {
  return (
    <svg className="thread-glyph" width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor"
      strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"
      data-inbox-glyph={name}>
      {INBOX_GLYPHS[name]}
    </svg>
  );
}

/**
 * A status mark's glyph and its colour class, for a slim row's slot or a
 * folded caption: Needs you a dot in the sidebar's amber, Working the Pair,
 * Failed the error glyph, Back the alarm clock.
 */
function markGlyph(mark: Exclude<ThreadMark, "unread" | "none">): { glyph: ReactNode; tone: string | null } {
  switch (mark) {
    case "needs-you": return { glyph: <span className="thread-inbox-mark-dot" />, tone: "session-wait" };
    case "working": return { glyph: <ThinkingMark size={13} />, tone: null };
    case "failed": return { glyph: <ThreadGlyph name="error" size={14} />, tone: "session-fail" };
    case "woke": return { glyph: <InboxGlyph name="alarm" size={13} />, tone: "thread-inbox-woke" };
  }
}

/** The one status mark a folded caption or a slim row draws, as a glyph. */
export function ThreadMarkGlyph({ mark }: { mark: ThreadMark }): ReactNode {
  const word = threadMarkWord(mark);
  if (mark === "none") return null;
  if (mark === "unread") {
    return <span className="dot dot-done thread-inbox-mark" role="img"
      aria-label={word} title={word} data-thread-mark={mark} />;
  }
  const { glyph, tone } = markGlyph(mark);
  return <span className={tone === null ? "thread-inbox-mark" : `thread-inbox-mark ${tone}`} role="img"
    aria-label={word} title={word} data-thread-mark={mark}>{glyph}</span>;
}

/** The header's project scope: All projects, each project, No project. */
export function ThreadScopePicker({ inbox }: { inbox: ThreadInboxController }): ReactNode {
  const { model, setScope } = inbox;
  const options = useMemo(() => threadScopeOptions(model), [model]);
  return <AppSelect compact className="thread-scope-picker" label="Project scope"
    value={model.scope} options={options} onChange={setScope} />;
}

/** A slim row's project hue: a small square, or a hollow ring for No project. */
function ProjectHueSquare({ root }: { root: string | null }): ReactNode {
  return <span className={`thread-inbox-hue ${projectHueClass(root)}`} aria-hidden="true" />;
}

/** A full row's tile: the thread's initials on its project's hue (TN Code's chat tiles). */
function ThreadTile({ root, title }: { root: string | null; title: string }): ReactNode {
  return (
    <span className={`thread-inbox-tile ${projectHueClass(root)}`} aria-hidden="true">
      {threadInitials(title)}
    </span>
  );
}

/**
 * Unread: the done blue. `arriving` pops it in: the row was on screen when its
 * run ended unread.
 */
function UnreadDot({ arriving = false }: { arriving?: boolean }): ReactNode {
  return <span className={arriving ? "thread-inbox-unread is-arriving" : "thread-inbox-unread"} />;
}

/**
 * Line one's right-hand slot on a two-line row. Hidden, never removed, under
 * the hover actions (owner, 2026-10-07, "Inbox under load"; Threads look,
 * 2026-10-23): Working is the Pair and the run's clock, no pill; Needs you is
 * the one filled pill; Failed an outline pill; Back stays a muted word and the
 * age plain text, after the unread dot.
 */
function RowSlot({ slot, mark, arriving }: { slot: ThreadRowSlot; mark: ThreadMark; arriving: boolean }): ReactNode {
  if (slot.kind === "age") {
    return (
      <span className="thread-inbox-slot is-age" data-thread-mark={mark}
        title={slot.unread ? threadMarkWord("unread") : undefined}>
        {slot.unread && <UnreadDot arriving={arriving} />}
        {slot.age}
      </span>
    );
  }
  if (slot.mark === "working") {
    return (
      <span className="thread-inbox-slot is-working" data-thread-mark={mark} title={threadMarkWord("working")}>
        <ThinkingMark size={13} />
        {slot.elapsed !== undefined && <span className="thread-inbox-clock">{slot.elapsed}</span>}
      </span>
    );
  }
  if (slot.mark === "woke") {
    return (
      <span className="thread-inbox-slot is-woke" data-thread-mark={mark} title={threadMarkWord("woke")}>
        <InboxGlyph name="alarm" size={13} />{slot.word}
      </span>
    );
  }
  return (
    <span className={`thread-inbox-slot is-${slot.mark}`} data-thread-mark={mark} title={threadMarkWord(slot.mark)}>
      <span className={`thread-inbox-pill is-${slot.mark}`}>{slot.word}</span>
    </span>
  );
}

/** The dot between line two's words. */
function Sep(): ReactNode {
  return <span className="thread-inbox-sep" aria-hidden="true">·</span>;
}

/**
 * Line two's words after the hue square: project · branch · the pull request
 * or the diff (`threadRowTrail`), or a live thread's own line in their place
 * (`threadRowLine`), which takes the pull request and the diff with it.
 */
function RowWords({ row, line, trail }: { row: ThreadInboxRow; line: ThreadRowLine; trail: ThreadRowTrail | null }): ReactNode {
  if (line.kind !== "meta") {
    return <span className={`thread-inbox-line is-${line.kind}`} title={line.text}>{line.text}</span>;
  }
  return (
    <>
      <span className="thread-inbox-project" title={row.projectRoot ?? undefined}>{row.projectLabel}</span>
      {row.branch !== undefined && (
        <><Sep /><span className="thread-inbox-branch" title={row.branch}>{row.branch}</span></>
      )}
      {trail?.kind === "pr" && (
        <><Sep /><span className={`thread-inbox-pr is-${trail.state}`} title={`Pull request #${trail.number} · ${trail.state}`}>
          <ThreadGlyph name={trail.state === "merged" ? "merge" : "pr"} size={12} />#{trail.number}
        </span></>
      )}
      {trail?.kind === "diff" && (
        <><Sep /><span className="thread-inbox-diff">
          <span className="thread-inbox-added">+{trail.added}</span>
          <span className="thread-inbox-removed">−{trail.removed}</span>
        </span></>
      )}
    </>
  );
}

function motionAllowed(): boolean {
  return typeof matchMedia !== "function" || !matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** How long a live line takes to fade out over the project line (--motion-base, and a frame). */
const THREAD_LINE_LEAVE_MS = 220;

/**
 * What arrived on a row already on screen since its last paint: the unread
 * dot (a run ended unread), and a live line ending, which fades out over the
 * project line coming back. Nothing arrives on a row's first paint, so opening
 * the app or a fold never pops or fades anything.
 */
function useRowArrivals(mark: ThreadMark, line: ThreadRowLine): {
  readonly arriving: boolean; readonly leaving: ThreadRowLine | null;
} {
  const seen = useRef<{ mark: ThreadMark; line: ThreadRowLine } | null>(null);
  const [arriving, setArriving] = useState(false);
  const [leaving, setLeaving] = useState<ThreadRowLine | null>(null);
  useLayoutEffect(() => {
    const before = seen.current;
    seen.current = { mark, line };
    if (before === null) return;
    if (before.mark !== "unread" && mark === "unread") setArriving(true);
    if (before.line.kind !== "meta" && line.kind === "meta" && motionAllowed()) setLeaving(before.line);
  }, [mark, line]);
  useEffect(() => {
    if (leaving === null) return;
    const timer = window.setTimeout(() => { setLeaving(null); }, THREAD_LINE_LEAVE_MS);
    return () => { window.clearTimeout(timer); };
  }, [leaving]);
  return { arriving, leaving };
}

/** A slim row's right edge: a live status glyph, the wake time, or the age. */
function ShelfSlot({ slot, mark }: { slot: ThreadShelfSlot; mark: ThreadMark }): ReactNode {
  if (slot.kind === "status") {
    const { glyph, tone } = markGlyph(slot.mark);
    return <span className={tone === null ? `thread-inbox-slot is-${slot.mark}` : `thread-inbox-slot is-${slot.mark} ${tone}`}
      data-thread-mark={mark} title={slot.word}>{glyph}</span>;
  }
  if (slot.kind === "wake") {
    return <span className="thread-inbox-slot is-wake" data-thread-mark={mark} title={slot.title}>{slot.label}</span>;
  }
  return (
    <span className="thread-inbox-slot is-age" data-thread-mark={mark}>
      {slot.unread && <UnreadDot />}
      {slot.age}
    </span>
  );
}

/** Line two's right edge: the provider's own mark, in its brand colour. */
function RowTrail({ engine }: { engine: string }): ReactNode {
  return (
    <span className="thread-inbox-trail">
      <ProviderLogo engine={engine} label={engine} size={12} className="thread-inbox-provider" />
    </span>
  );
}

/** One committed object whose members never change identity (useCommittedActions). */
type ThreadRowActions = {
  readonly activate: (key: SessionKey, split: boolean) => void;
  readonly toggleMenu: (key: SessionKey, anchor: DOMRect, trigger: HTMLElement) => void;
  readonly toggleSnooze: (key: SessionKey, trigger: HTMLElement) => void;
  /** The hover actions that need no menu: Settle, Wake, Un-settle, Continue as thread. */
  readonly quick: (key: SessionKey, kind: "settle" | "unsnooze" | "unsettle" | "continueFromTerminal") => void;
  readonly peekEnter: (key: SessionKey, element: HTMLElement) => void;
  readonly peekLeave: () => void;
  readonly peekToggle: (key: SessionKey, element: HTMLElement) => void;
  readonly peekHide: () => void;
  readonly dragStart: (key: SessionKey, transfer: DataTransfer) => void;
  readonly dragEnd: () => void;
  readonly setRenameDraft: (draft: string) => void;
  readonly commitRename: (key: SessionKey) => void;
  readonly cancelRename: () => void;
};

interface ThreadRowProps {
  readonly row: ThreadInboxRow;
  /** Line one's slot (two-line rows) or the slim row's right edge. */
  readonly slot: ThreadRowSlot | ThreadShelfSlot;
  readonly trail: ThreadRowTrail | null;
  /** Line two's words on a two-line row: the project line, or a live thread's own. */
  readonly line: ThreadRowLine;
  readonly snoozeOpen: boolean;
  readonly renaming: boolean;
  readonly renameDraft: string;
  readonly peeking: boolean;
  readonly dragging: boolean;
  readonly dropEdge: "before" | "after" | null;
  /** Snoozed rows say when they come back. */
  readonly wake: string | null;
  readonly uid: string;
  readonly actions: ThreadRowActions;
}

/** Plain objects equal field by field, one level of nesting deep (a row's diff and pull request). */
function sameFields(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  return keys.length === Object.keys(right).length && keys.every(key => Object.is(left[key], right[key])
    || (typeof left[key] === "object" && sameFields(left[key], right[key])));
}

function sameThreadRow(a: ThreadRowProps, b: ThreadRowProps): boolean {
  const names = Object.keys(a) as (keyof ThreadRowProps)[];
  return names.every(name => Object.is(a[name], b[name])
    || ((name === "row" || name === "slot" || name === "trail" || name === "line") && sameFields(a[name], b[name])));
}

/**
 * One thread, memoized so a streamed token redraws no row and the working
 * clock redraws only the row whose words changed. Pinned and Active rows are
 * two lines; Snoozed, Settled and From terminals rows are one slim line.
 */
const ThreadRow = memo(function ThreadRow({ row, slot, trail, line, snoozeOpen, renaming, renameDraft, peeking,
    dragging, dropEdge, wake, uid, actions }: ThreadRowProps): ReactNode {
  const slim = isShelf(row.section);
  const { arriving, leaving } = useRowArrivals(row.mark, line);
  const classes = ["session-item", "thread-inbox-item", slim ? "is-slim" : "is-full"];
  if (dragging) classes.push("is-dragging");
  if (dropEdge !== null) classes.push(`is-drop-${dropEdge}`);
  const rowClasses = ["session-row", "thread-inbox-row", `is-${threadTitleTone(row)}`];
  if (slim) rowClasses.push("is-slim");
  if (row.selected) rowClasses.push("is-selected");
  const title = (
    <span className="session-title thread-inbox-title" title={wake === null ? row.title : `${row.title} · ${wake}`}>
      {/* An Away loop's thread (review B3): Away's own coffee, in the dim tier. */}
      {row.away === true && <span className="codicon codicon-sm codicon-coffee thread-inbox-away"
        aria-hidden="true" title={AWAY_ROW_WORDS} />}
      {/* The short title (owner, 2026-11-15); the whole one is the tooltip. */}
      {row.displayTitle}
    </span>
  );
  return (
    <li className={classes.join(" ")} data-thread-key={row.key} data-thread-section={row.section}>
      {renaming ? (
        <input className="session-rename" value={renameDraft} aria-label={`Rename ${row.title}`} autoFocus
          onChange={event => { actions.setRenameDraft(event.target.value); }}
          // Blur cancels: a rename writes the engine's store, and a click
          // elsewhere must not be a silent write.
          onBlur={actions.cancelRename}
          onKeyDown={event => {
            if (event.key === "Enter") {
              event.preventDefault();
              actions.commitRename(row.key);
            } else if (event.key === "Escape") {
              event.preventDefault();
              actions.cancelRename();
            }
          }} />
      ) : (
        <>
          <button type="button" draggable className={rowClasses.join(" ")}
            aria-label={threadRowAccessibleName(row, slot, trail, line)}
            aria-current={row.selected || undefined}
            aria-describedby={peeking ? `${uid}-thread-peek` : undefined}
            onPointerEnter={event => {
              if (event.pointerType === "touch") return;
              actions.peekEnter(row.key, event.currentTarget);
            }}
            onPointerLeave={actions.peekLeave}
            onBlur={actions.peekLeave}
            onClick={event => { actions.activate(row.key, event.metaKey || event.ctrlKey); }}
            onContextMenu={event => {
              event.preventDefault();
              actions.toggleMenu(row.key, new DOMRect(event.clientX, event.clientY, 0, 0), event.currentTarget);
            }}
            onKeyDown={event => {
              // Space is the preview, Enter the thread, as on session rows; the
              // menu key and ⇧F10 open the row menu.
              if (event.key === " ") {
                event.preventDefault();
                actions.peekToggle(row.key, event.currentTarget);
              } else if (event.key === "Escape" && peeking) {
                event.preventDefault();
                actions.peekHide();
              } else if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
                event.preventDefault();
                actions.toggleMenu(row.key, event.currentTarget.getBoundingClientRect(), event.currentTarget);
              }
            }}
            onDragStart={event => { actions.dragStart(row.key, event.dataTransfer); }}
            onDragEnd={actions.dragEnd}>
            {slim ? (
              <>
                <ProjectHueSquare root={row.projectRoot} />
                {title}
                <ShelfSlot slot={slot as ThreadShelfSlot} mark={row.mark} />
              </>
            ) : (
              <>
                <ThreadTile root={row.projectRoot} title={row.displayTitle} />
                {title}
                <RowSlot slot={slot as ThreadRowSlot} mark={row.mark} arriving={arriving} />
                <span className="thread-inbox-meta">
                  <span className={leaving === null ? "thread-inbox-words" : "thread-inbox-words is-entering"}>
                    <RowWords row={row} line={line} trail={trail} />
                    {leaving !== null && (
                      <span className="thread-inbox-leaving" aria-hidden="true">
                        <RowWords row={row} line={leaving} trail={trail} />
                      </span>
                    )}
                  </span>
                </span>
                <RowTrail engine={row.engine} />
              </>
            )}
          </button>
          <span className="thread-inbox-actions">
            {row.section === "snoozed" ? (
              <button type="button" className="thread-inbox-action" data-thread-action="unsnooze"
                aria-label={`Wake ${row.title}`} title="Wake now"
                onClick={() => { actions.quick(row.key, "unsnooze"); }}>
                <InboxGlyph name="wake" size={15} />
              </button>
            ) : row.section === "settled" ? (
              <button type="button" className="thread-inbox-action" data-thread-action="unsettle"
                aria-label={`Un-settle ${row.title}`} aria-keyshortcuts={THREAD_MENU_CHORDS.unsettle.keys}
                title={`Un-settle (${THREAD_MENU_CHORDS.unsettle.hint})`}
                onClick={() => { actions.quick(row.key, "unsettle"); }}>
                <ThreadGlyph name="undo" size={15} />
              </button>
            ) : row.section === "fromTerminals" ? (
              <button type="button" className="thread-inbox-action" data-thread-action="continueFromTerminal"
                aria-label={`Continue ${row.title} as thread`} title="Continue as thread"
                onClick={() => { actions.quick(row.key, "continueFromTerminal"); }}>
                <ThreadGlyph name="undo" size={15} />
              </button>
            ) : (
              <>
                <button type="button" className="thread-inbox-action" data-thread-action="snooze"
                  data-thread-menu-trigger="" aria-label={`Snooze ${row.title}`} title="Snooze"
                  aria-haspopup="menu" aria-expanded={snoozeOpen}
                  aria-controls={snoozeOpen ? `${uid}-thread-menu` : undefined}
                  onClick={event => { actions.toggleSnooze(row.key, event.currentTarget); }}>
                  <InboxGlyph name="alarm" size={15} />
                </button>
                <button type="button" className="thread-inbox-action" data-thread-action="settle"
                  aria-label={`Settle ${row.title}`} aria-keyshortcuts={THREAD_MENU_CHORDS.settle.keys}
                  title={`Settle (${THREAD_MENU_CHORDS.settle.hint})`}
                  onClick={() => { actions.quick(row.key, "settle"); }}>
                  <ThreadGlyph name="pass" size={15} />
                </button>
              </>
            )}
          </span>
        </>
      )}
    </li>
  );
}, sameThreadRow);

/** A row's slide to its new place (owner, 2026-10-07): transform only. */
const THREAD_FLOAT_MS = 240;
const THREAD_FLOAT_EASE = "cubic-bezier(.16, 1, .3, 1)";

interface ThreadRowsMotionProps {
  readonly id: string;
  /** The section's keys in Rust's order, joined. */
  readonly rustOrder: string;
  /** The keys as drawn, joined. */
  readonly drawnOrder: string;
  readonly children: ReactNode;
}

/**
 * A section's list. When the only change is a row floating to the top
 * because it needs you, or settling back once answered (the drawn order moved
 * and Rust's did not), each row that moved slides from where it was drawn:
 * measured before React moves the nodes (a layout effect runs too late),
 * played after, transform only. Anything else lands at once, and reduced
 * motion, a hidden window or a hidden sidebar skip the slide. Presentation
 * only: no order is kept here.
 */
class ThreadRowsMotion extends Component<ThreadRowsMotionProps> {
  private list = createRef<HTMLUListElement>();
  private playing = new Set<() => void>();
  componentWillUnmount(): void { this.stop(); }
  private stop(): void {
    for (const cleanup of [...this.playing]) cleanup();
  }
  private rows(): [string, HTMLElement][] {
    return [...this.list.current?.children ?? []].flatMap(node => node instanceof HTMLElement
      && node.dataset.threadKey !== undefined ? [[node.dataset.threadKey, node] as [string, HTMLElement]] : []);
  }
  getSnapshotBeforeUpdate(previous: ThreadRowsMotionProps): Map<string, number> | null {
    if (previous.drawnOrder === this.props.drawnOrder || previous.rustOrder !== this.props.rustOrder) return null;
    const list = this.list.current;
    if (list === null || typeof list.animate !== "function" || document.hidden || !motionAllowed()
      || list.getClientRects().length === 0) return null;
    // Where each row is drawn now, mid-slide included, before the slide in flight is dropped.
    const tops = new Map(this.rows().map(([key, node]) => [key, node.getBoundingClientRect().top]));
    this.stop();
    return tops;
  }
  componentDidUpdate(_previous: ThreadRowsMotionProps, _state: unknown, before: Map<string, number> | null): void {
    if (before === null) return;
    for (const [key, node] of this.rows()) {
      const from = before.get(key);
      if (from === undefined) continue;
      const dy = from - node.getBoundingClientRect().top;
      if (Math.abs(dy) < 0.5) continue;
      const animation = node.animate([{ transform: `translateY(${dy}px)` }, { transform: "none" }],
        { duration: THREAD_FLOAT_MS, easing: THREAD_FLOAT_EASE });
      animation.id = "thread-inbox-float";
      // Occlusion can suspend animation events; no row stays displaced past the bound.
      const timer = window.setTimeout(() => { cleanup(); }, THREAD_FLOAT_MS + 100);
      const cleanup = (): void => {
        this.playing.delete(cleanup);
        window.clearTimeout(timer);
        animation.onfinish = animation.oncancel = null;
        animation.cancel();
      };
      animation.onfinish = animation.oncancel = cleanup;
      this.playing.add(cleanup);
    }
  }
  render(): ReactNode {
    return <ul ref={this.list} id={this.props.id} className="session-items">{this.props.children}</ul>;
  }
}

/** A drop's place: the section, the key it goes before, and the row edge drawn. */
interface ThreadDrop extends ThreadDropTarget {
  readonly over?: { readonly key: SessionKey; readonly edge: "before" | "after" };
}

interface ThreadMenuState {
  readonly key: SessionKey;
  /** The row's whole menu, or the Snooze presets its hover button opens. */
  readonly kind: "row" | "snooze";
  /** The open submenu, and whether the keyboard opened it (then it takes focus). */
  readonly submenu: { readonly id: ThreadSubmenuId; readonly focus: boolean } | null;
}

/** The row menu's submenus: Rust's (Snooze, Auto-settle) and Hand to teammate. */
type ThreadSubmenuId = "snooze" | "autoSettle" | "teammate";

export interface ThreadInboxProps {
  readonly inbox: ThreadInboxController;
  readonly onSelect: (key: SessionKey) => void;
  readonly onOpenInNewPane?: (key: SessionKey) => void;
  /** Open in terminal (review C1): resume the thread's session in a Code pane. */
  readonly onOpenInTerminal?: (key: SessionKey) => void;
  /** Hand to teammate (review C1): the teammates offered, and the hand-off. */
  readonly teammates?: readonly ThreadTeammate[];
  readonly onHandToTeammate?: (key: SessionKey, agentId: string) => void;
  /**
   * Second opinion from … and Switch to … (review B4; Switch to since
   * protocol 59): the other providers a row's thread can ask or move to, and
   * the two actions.
   */
  readonly offersFor?: (engine: string, project: boolean) => ThreadProviderOffers;
  readonly onSecondOpinion?: (key: SessionKey, engine: CollaborationEngine) => void;
  readonly onSwitchTo?: (key: SessionKey, engine: EngineId) => void;
  /**
   * The session's local hide, named Archive here; Undo is its unarchive.
   * Resolves whether it archived: Undo is offered only for an archive that
   * happened (an answer of nothing offers none).
   */
  readonly onArchive: (key: SessionKey) => void | Promise<boolean>;
  readonly lifecycle?: SidebarLifecycle;
  readonly diffStats: Record<string, DiffStat>;
  readonly peeks: Record<string, SessionPeekFacts>;
  /**
   * Session key -> what its live log says (`threadLiveFacts`): the step it is
   * on, what it asks, why it failed. Line two words the row's from it; absent
   * keys keep the project line.
   */
  readonly live?: Readonly<Record<string, ThreadLiveFacts>>;
  /** The Sidebar's copy for an inbox with nothing in any scope. */
  readonly emptyText: string;
  readonly emptyHint: string | null;
  readonly filterQuery: string;
  readonly onSearchTranscripts?: (query: string) => void;
  /** The clock for snooze presets and Undo; fixtures pin it. */
  readonly now?: () => number;
}

const NO_THREAD_LIVE: Readonly<Record<string, ThreadLiveFacts>> = {};
const NO_TEAMMATES: readonly ThreadTeammate[] = [];

/** The inbox, between the Sidebar's header and its footer. */
export function ThreadInbox({ inbox, onSelect, onOpenInNewPane, onOpenInTerminal, teammates = NO_TEAMMATES,
    onHandToTeammate, offersFor, onSecondOpinion, onSwitchTo, onArchive, lifecycle, diffStats,
    peeks, live = NO_THREAD_LIVE, emptyText, emptyHint, filterQuery, onSearchTranscripts,
    now = Date.now }: ThreadInboxProps) {
  const { model } = inbox;
  const uid = useId();
  const listRef = useRef<HTMLDivElement>(null);
  const [folds, setFolds] = useState<ThreadSectionFolds>(readFolds);
  const [settledShown, setSettledShown] = useState(SETTLED_PAGE_SIZE);
  const [fromTerminalsShown, setFromTerminalsShown] = useState(SETTLED_PAGE_SIZE);
  const [menu, setMenu] = useState<ThreadMenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuAnchor = useRef<DOMRect | null>(null);
  const [menuBox, setMenuBox] = useState<AppSelectPosition | null>(null);
  const submenuRef = useRef<HTMLDivElement>(null);
  const submenuAnchor = useRef<DOMRect | null>(null);
  const [submenuBox, setSubmenuBox] = useState<{ top: number; left: number } | null>(null);
  // Where focus goes back to when a menu closes: the row, or the Snooze button.
  const returnFocus = useRef<HTMLElement | null>(null);
  const [renameFor, setRenameFor] = useState<SessionKey | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [confirm, setConfirm] = useState<{ key: SessionKey; kind: SessionConfirmKind } | null>(null);
  const [peekFor, setPeekFor] = useState<SessionKey | null>(null);
  const [peekBox, setPeekBox] = useState<SessionPeekPosition | null>(null);
  const peekRef = useRef<HTMLDivElement>(null);
  const peekAnchor = useRef<DOMRect | null>(null);
  const peekTimer = useRef<number | null>(null);
  const peekShowing = useRef(false);
  const [dragKey, setDragKey] = useState<SessionKey | null>(null);
  const [drop, setDrop] = useState<ThreadDrop | null>(null);
  const [toast, setToast] = useState<ThreadUndo | null>(null);
  const toastTimer = useRef<number | null>(null);

  const rowByKey = useMemo(() => new Map(model.sections.flatMap(section =>
    section.rows.map(row => [row.key, row] as const))), [model]);
  const order = useMemo(() => threadInboxOrder(model, folds, settledShown, fromTerminalsShown),
    [folds, fromTerminalsShown, model, settledShown]);
  const selectedKey = useMemo(() => model.sections.flatMap(section => section.rows)
    .find(row => row.selected)?.key ?? null, [model]);

  const menuRow = menu === null ? null : rowByKey.get(menu.key) ?? null;
  const peekRow = peekFor === null ? null : rowByKey.get(peekFor) ?? null;
  const confirmRow = confirm === null ? null : rowByKey.get(confirm.key) ?? null;

  /* ---- actions ---------------------------------------------------------- */

  const clearToastTimer = (): void => {
    if (toastTimer.current === null) return;
    window.clearTimeout(toastTimer.current);
    toastTimer.current = null;
  };
  useEffect(() => clearToastTimer, []);

  const offerUndo = (undo: ThreadUndo): void => {
    clearToastTimer();
    setToast(undo);
    toastTimer.current = window.setTimeout(() => {
      toastTimer.current = null;
      setToast(null);
    }, THREAD_UNDO_MS);
  };

  const runUndo = (): void => {
    if (toast === null) return;
    clearToastTimer();
    setToast(null);
    if (toast.action === "archive") inbox.unarchive(toast.key);
    else void inbox.send(threadUndoCommands(toast, now()));
  };

  /** Send one action's commands; settle and snooze then offer Undo. */
  const act = (row: ThreadInboxRow, commands: readonly ThreadCommand[], undoAction?: ThreadUndoAction): void => {
    if (commands.length === 0) return;
    // What was true BEFORE the action, read now: the push that answers it
    // replaces the model.
    const undo = undoAction === undefined ? null : threadUndo(undoAction, row, model);
    void inbox.send(commands).then(sent => { if (sent && undo !== null) offerUndo(undo); });
  };

  const archive = (row: ThreadInboxRow): void => {
    const undo = threadUndo("archive", row, model);
    // Undo waits for the archive's answer: offered sooner, it could unarchive
    // before the archive lands; a refused archive has nothing to undo.
    void Promise.resolve(onArchive(row.key)).then(archived => {
      if (archived === true) offerUndo(undo);
    });
  };

  const rowButton = (key: SessionKey): HTMLElement | null =>
    [...listRef.current?.querySelectorAll<HTMLElement>(".thread-inbox-item") ?? []]
      .find(node => node.dataset.threadKey === key)?.querySelector<HTMLElement>(".thread-inbox-row") ?? null;

  const restoreFocus = (key: SessionKey): void => {
    requestAnimationFrame(() => {
      const trigger = returnFocus.current;
      if (trigger?.isConnected === true
        && trigger.closest<HTMLElement>(".thread-inbox-item")?.dataset.threadKey === key) {
        trigger.focus();
        return;
      }
      const row = rowButton(key);
      if (row !== null) {
        row.focus();
        return;
      }
      listRef.current?.querySelector<HTMLElement>(".thread-inbox-row.is-selected, .thread-inbox-row")?.focus();
    });
  };

  const closeMenu = (focusKey?: SessionKey): void => {
    setMenu(null);
    if (focusKey !== undefined) restoreFocus(focusKey);
  };

  /**
   * Close the menu of a row that is leaving the list (Archive): focus goes
   * to the row that takes its place, not to a trigger about to disappear.
   */
  const closeMenuInPlaceOf = (key: SessionKey): void => {
    setMenu(null);
    focusInPlaceOf(key);
  };

  /**
   * Close the menu after `undo`'s action. Settle and snooze move the row
   * out of its section too, so focus goes where Archive's does: left on the
   * vanishing trigger it fell to the page, and ⌘Z never reached Undo.
   */
  const closeMenuAfter = (row: ThreadInboxRow, undo: ThreadUndoAction | undefined): void => {
    if (threadActionLeavesSection(undo, row.section)) closeMenuInPlaceOf(row.key);
    else closeMenu(row.key);
  };

  /** Focus the row that takes `key`'s place in the list. */
  const focusInPlaceOf = (key: SessionKey): void => {
    const neighbour = threadFocusAfterRemoval(order, key);
    if (neighbour === null) {
      restoreFocus(key);
      return;
    }
    requestAnimationFrame(() => {
      const row = rowButton(neighbour);
      if (row !== null) row.focus();
      else restoreFocus(key);
    });
  };

  /* ---- the hover card --------------------------------------------------- */

  const clearPeekTimer = (): void => {
    if (peekTimer.current === null) return;
    window.clearTimeout(peekTimer.current);
    peekTimer.current = null;
  };
  const showPeek = (key: SessionKey, anchor: DOMRect): void => {
    clearPeekTimer();
    peekAnchor.current = anchor;
    peekShowing.current = true;
    setPeekBox(null);
    setPeekFor(key);
  };
  const hidePeek = (): void => {
    clearPeekTimer();
    peekShowing.current = false;
    setPeekFor(null);
  };
  useEffect(() => clearPeekTimer, []);
  useAnchoredOverlay(peekFor, peekRef, peekAnchor, setPeekBox, positionSessionPeek);
  useEffect(() => {
    if (peekFor === null) return;
    const onKey = (event: KeyboardEvent): void => { if (event.key === "Escape") hidePeek(); };
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("keydown", onKey); };
  }, [peekFor]);
  useEffect(() => {
    // A scroll moves the row out from under its card.
    const onScroll = (event: Event): void => {
      // Only a scroll that can move a row counts: the list's own, or an ancestor's.
      const target = event.target;
      const list = listRef.current;
      if (!(target instanceof Node) || list === null
        || (!list.contains(target) && !target.contains(list))) return;
      clearPeekTimer();
      peekShowing.current = false;
      setPeekFor(null);
    };
    window.addEventListener("scroll", onScroll, true);
    return () => { window.removeEventListener("scroll", onScroll, true); };
  }, []);

  /* ---- the row menu ------------------------------------------------------ */

  useAnchoredOverlay(menu === null ? null : `${menu.kind}:${menu.key}`, menuRef, menuAnchor, setMenuBox,
    positionAppSelect, 200);
  useAnchoredOverlay(menu?.submenu?.id ?? null, submenuRef, submenuAnchor, setSubmenuBox,
    positionThreadSubmenu, 160);

  useEffect(() => {
    if (menu === null || menuBox === null || menu.submenu !== null) return;
    if (menuRef.current?.contains(document.activeElement) === true) return;
    return focusWhenShown(() => menuRef.current?.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)'));
  }, [menu, menuBox]);

  const focusSubmenu = (): void => {
    submenuRef.current?.querySelector<HTMLButtonElement>(SUBMENU_ITEMS)?.focus();
  };
  useEffect(() => {
    if (menu?.submenu?.focus !== true || submenuBox === null) return;
    return focusWhenShown(() => submenuRef.current?.querySelector<HTMLElement>(SUBMENU_ITEMS));
  }, [menu, submenuBox]);

  useEffect(() => {
    if (menu === null) return;
    const onPointer = (event: PointerEvent): void => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target) === true || submenuRef.current?.contains(target) === true) return;
      // The Snooze button toggles its own menu on click.
      if (target instanceof Element && target.closest(".thread-inbox-item [data-thread-menu-trigger]") !== null) return;
      setMenu(null);
    };
    window.addEventListener("pointerdown", onPointer);
    return () => { window.removeEventListener("pointerdown", onPointer); };
  }, [menu]);

  const openSubmenu = (id: ThreadSubmenuId, item: HTMLElement, focus: boolean): void => {
    if (menu === null) return;
    if (menu.submenu?.id === id) {
      // Already open: the keyboard moves into it; a second hover does nothing.
      if (focus && menu.submenu.focus) focusSubmenu();
      else if (focus) setMenu({ key: menu.key, kind: menu.kind, submenu: { id, focus } });
      return;
    }
    submenuAnchor.current = item.getBoundingClientRect();
    setSubmenuBox(null);
    setMenu({ key: menu.key, kind: menu.kind, submenu: { id, focus } });
  };

  const closeSubmenu = (): void => {
    if (menu === null || menu.submenu === null) return;
    const id = menu.submenu.id;
    setMenu({ key: menu.key, kind: menu.kind, submenu: null });
    requestAnimationFrame(() => {
      menuRef.current?.querySelector<HTMLElement>(`[data-thread-submenu="${id}"]`)?.focus();
    });
  };

  /* ---- drag and drop ------------------------------------------------------ */

  /**
   * Where a drag over `inbox` would drop. The line is drawn on the row under
   * the pointer; the place it names is in Rust's order (`threadDropBeforeKey`),
   * since a row that needs you floats only on screen.
   */
  const dragOver = (event: ReactDragEvent<HTMLElement>, inboxSection: ThreadInboxSection, header: boolean): void => {
    const section = inboxSection.section;
    if (dragKey === null || !isThreadDrag(event.dataTransfer)
      || section === "snoozed" || section === "fromTerminals") return;
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = "move";
    let next: ThreadDrop = { section };
    if (section !== "settled") {
      const item = header || !(event.target instanceof Element) ? null
        : event.target.closest<HTMLElement>("[data-thread-key]");
      const over = item?.dataset.threadKey;
      const first = inboxSection.rows[0];
      const drawnFirst = inboxSection.drawn[0];
      if (header && first !== undefined && drawnFirst !== undefined) {
        next = { section, beforeKey: first.key, over: { key: drawnFirst.key, edge: "before" } };
      } else if (item !== null && item !== undefined && over !== undefined) {
        const rect = item.getBoundingClientRect();
        const before = event.clientY < rect.top + rect.height / 2;
        const beforeKey = threadDropBeforeKey(inboxSection.rows.map(row => row.key), over, before);
        next = beforeKey === undefined
          ? { section, over: { key: over, edge: before ? "before" : "after" } }
          : { section, beforeKey, over: { key: over, edge: before ? "before" : "after" } };
      }
    }
    setDrop(current => current !== null && current.section === next.section
      && current.beforeKey === next.beforeKey && current.over?.key === next.over?.key
      && current.over?.edge === next.over?.edge ? current : next);
  };

  const dropOn = (event: ReactDragEvent<HTMLElement>): void => {
    if (dragKey === null || drop === null || !isThreadDrag(event.dataTransfer)) return;
    event.preventDefault();
    event.stopPropagation();
    const row = rowByKey.get(dragKey);
    setDragKey(null);
    setDrop(null);
    if (row === undefined) return;
    const commands = threadDropCommands(row, drop);
    act(row, commands, threadDropUndo(commands));
  };

  /* ---- rename, confirm ---------------------------------------------------- */

  const commitRename = (key: SessionKey): void => {
    const row = rowByKey.get(key);
    const title = renameDraft.trim();
    setRenameFor(null);
    restoreFocus(key);
    // An empty box is a cancel; an unchanged title is not worth a write.
    if (row === undefined || title === "" || title === row.title || lifecycle === undefined) return;
    lifecycle.onRename(key, title);
  };

  const rowActions = useCommittedActions<ThreadRowActions>({
    activate: (key, split) => {
      setMenu(null);
      hidePeek();
      if (split && onOpenInNewPane !== undefined) onOpenInNewPane(key);
      else onSelect(key);
    },
    toggleMenu: (key, anchor, trigger) => {
      if (menu?.key === key && menu.kind === "row") {
        setMenu(null);
        return;
      }
      hidePeek();
      returnFocus.current = trigger;
      menuAnchor.current = anchor;
      setMenuBox(null);
      setMenu({ key, kind: "row", submenu: null });
    },
    toggleSnooze: (key, trigger) => {
      if (menu?.key === key && menu.kind === "snooze") {
        setMenu(null);
        return;
      }
      hidePeek();
      returnFocus.current = trigger;
      menuAnchor.current = trigger.getBoundingClientRect();
      setMenuBox(null);
      setMenu({ key, kind: "snooze", submenu: null });
    },
    quick: (key, kind) => {
      const row = rowByKey.get(key);
      if (row === undefined) return;
      setMenu(null);
      hidePeek();
      // Each takes the row out of its section: focus takes its place, so ⌘Z
      // still finds the sidebar focused.
      focusInPlaceOf(key);
      if (kind === "settle") act(row, [{ kind: "settle", key }], "settle");
      else act(row, [{ kind, key }]);
    },
    peekEnter: (key, element) => {
      clearPeekTimer();
      const anchor = element.getBoundingClientRect();
      if (peekShowing.current) {
        showPeek(key, anchor);
        return;
      }
      peekTimer.current = window.setTimeout(() => {
        peekTimer.current = null;
        showPeek(key, anchor);
      }, SESSION_PEEK_HOVER_MS);
    },
    peekLeave: () => {
      clearPeekTimer();
      peekTimer.current = window.setTimeout(() => {
        peekTimer.current = null;
        hidePeek();
      }, SESSION_PEEK_LEAVE_MS);
    },
    peekToggle: (key, element) => {
      if (peekFor === key) hidePeek();
      else showPeek(key, element.getBoundingClientRect());
    },
    peekHide: hidePeek,
    dragStart: (key, transfer) => {
      transfer.effectAllowed = "move";
      transfer.setData(THREAD_DRAG_MIME, key);
      hidePeek();
      setMenu(null);
      setDragKey(key);
    },
    dragEnd: () => {
      setDragKey(null);
      setDrop(null);
    },
    setRenameDraft,
    commitRename,
    cancelRename: () => { setRenameFor(null); },
  });

  /* ---- keyboard ----------------------------------------------------------- */

  // The window chords read this render's facts through a ref, so the listener
  // is registered once per shortcuts change rather than once per render.
  const chordHandler = useRef<(event: KeyboardEvent) => void>(() => undefined);
  useLayoutEffect(() => {
    chordHandler.current = (event: KeyboardEvent): void => {
      const chord = threadChord(event);
      if (chord === null || event.defaultPrevented) return;
      if (document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target !== null && target.closest(".terminal-center") !== null) return;
      if (chord === "undo") {
        // ⌘Z is the sidebar's only while it has focus and nothing is being typed.
        const active = document.activeElement;
        if (toast === null || active === null || active.closest(".sidebar") === null
          || editingText(active)) return;
        event.preventDefault();
        runUndo();
        return;
      }
      if (chord === "previous" || chord === "next") {
        event.preventDefault();
        const key = adjacentThreadKey(order, selectedKey, chord);
        if (key !== null) onSelect(key);
        return;
      }
      // Settle and pin act on the row holding focus, else the open thread.
      const focused = document.activeElement?.closest<HTMLElement>(".thread-inbox-item")?.dataset.threadKey;
      const row = rowByKey.get(focused ?? selectedKey ?? "");
      if (row === undefined) return;
      event.preventDefault();
      if (chord === "settle") {
        if (row.section === "settled") act(row, [{ kind: "unsettle", key: row.key }]);
        else {
          // The focused row leaves for Settled: focus takes its place, so ⌘Z still undoes.
          if (focused === row.key) focusInPlaceOf(row.key);
          act(row, [{ kind: "settle", key: row.key }], "settle");
        }
      } else {
        act(row, [row.pinned && row.section !== "settled"
          ? { kind: "unpin", key: row.key } : { kind: "pin", key: row.key }]);
      }
    };
  });
  useEffect(() => {
    if (!inbox.shortcuts) return;
    const onKey = (event: KeyboardEvent): void => { chordHandler.current(event); };
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("keydown", onKey); };
  }, [inbox.shortcuts]);

  // The rows' clock. A working row's elapsed time moves each second; ages
  // move by the minute, so half a minute keeps them honest. Only rows whose
  // words changed redraw (ThreadRow is memoized on what it draws).
  const ticking = model.sections.some(section => section.rows.some(row =>
    row.mark === "working" && row.runStartedAt !== undefined));
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => { setTick(tick => tick + 1); }, ticking ? 1000 : 30_000);
    return () => { window.clearInterval(timer); };
  }, [ticking]);

  const setFold = (section: ThreadSection, folded: boolean): void => {
    const next = { ...folds, [section]: folded };
    writeFolds(next);
    setFolds(next);
  };

  /* ---- render ------------------------------------------------------------- */

  const scopeRoot = threadScopeRoot(model.scope);
  const scopeName = scopeRoot === undefined ? ""
    : scopeRoot === null ? "No project"
      : model.projects.find(project => threadScopeFor(project.root) === model.scope)?.name ?? scopeRoot;
  // While a thread is dragged, the drop sections show even when empty.
  const shownSections: ThreadInboxSection[] = dragKey === null ? [...model.sections]
    : (["pinned", "active", "snoozed", "settled"] as const).flatMap(section =>
      model.sections.find(item => item.section === section)
        ?? (section === "snoozed" ? [] : [{ section, label: THREAD_SECTION_LABEL[section], rows: [], drawn: [],
          mark: "none" as const }]));
  const nowMs = now();

  const renderRows = (section: ThreadInboxSection, rows: readonly ThreadInboxRow[], listId: string): ReactNode => (
    <ThreadRowsMotion id={listId} rustOrder={section.rows.map(row => row.key).join("\n")}
      drawnOrder={rows.map(row => row.key).join("\n")}>
      {rows.map(row => {
        const slim = isShelf(row.section);
        return (
          <ThreadRow key={row.key} row={row}
            slot={slim ? threadShelfSlot(row, nowMs) : threadRowSlot(row, nowMs)}
            trail={slim ? null : threadRowTrail(row)}
            line={slim ? THREAD_META_LINE : threadRowLine(row, live[row.key])}
            snoozeOpen={menu?.key === row.key && menu.kind === "snooze"}
            renaming={renameFor === row.key && lifecycle !== undefined}
            renameDraft={renameFor === row.key ? renameDraft : ""}
            peeking={peekFor === row.key} dragging={dragKey === row.key}
            dropEdge={drop?.over?.key === row.key ? drop.over.edge : null}
            wake={row.section === "snoozed" && row.snoozedUntil !== undefined
              ? snoozeWakeLabel(row.snoozedUntil, nowMs) : null}
            uid={uid} actions={rowActions} />
        );
      })}
      {rows.length === 0 && (
        <li className="thread-inbox-drop-hint">
          {section.section === "pinned" ? "Drop here to pin" : section.section === "settled"
            ? "Drop here to settle" : "Drop here to make active"}
        </li>
      )}
      {rows.length < section.rows.length && (
        <li className="session-show-more">
          <button type="button" className="session-show-more-button"
            aria-label={`Show ${Math.min(SETTLED_PAGE_SIZE, section.rows.length - rows.length)} more ${
              section.section === "fromTerminals" ? "terminal sessions" : "settled threads"}`}
            onClick={() => {
              if (section.section === "fromTerminals") setFromTerminalsShown(shown => shown + SETTLED_PAGE_SIZE);
              else setSettledShown(shown => shown + SETTLED_PAGE_SIZE);
            }}>
            Show more
          </button>
        </li>
      )}
    </ThreadRowsMotion>
  );

  /**
   * Pinned and Active: a caption in sentence case with the count at the right
   * (the folded section's highest mark before it), then two-line rows.
   */
  const renderSection = (section: ThreadInboxSection): ReactNode => {
    const folded = threadSectionFolded(model, folds, section.section) && dragKey === null;
    const rows = threadSectionRows(model, section, settledShown, fromTerminalsShown);
    const headId = `${uid}-${section.section}`;
    const listId = `${headId}-rows`;
    const dropping = drop?.section === section.section;
    return (
      <section key={section.section} className={dropping ? "thread-inbox-section is-drop-zone" : "thread-inbox-section"}
        data-thread-section={section.section} aria-labelledby={headId}
        onDragOver={event => { dragOver(event, section, false); }}
        onDrop={dropOn}>
        <h3 className="thread-inbox-heading"
          onDragOver={event => { dragOver(event, section, true); }}>
          <button type="button" id={headId} className="thread-inbox-fold"
            aria-expanded={!folded} aria-controls={folded ? undefined : listId}
            aria-label={threadSectionAccessibleName(section, folded)}
            onClick={() => { setFold(section.section, !folds[section.section]); }}>
            <span className="thread-inbox-label">{section.label}</span>
            <ThreadGlyph name="expand" size={12}
              className={folded ? "thread-inbox-chevron is-folded" : "thread-inbox-chevron"} />
            <span className="thread-inbox-fold-end">
              {folded && section.mark !== "none" && (
                <span aria-hidden="true"><ThreadMarkGlyph mark={section.mark} /></span>
              )}
              {section.rows.length > 0 && <span className="thread-inbox-count" aria-hidden="true">{section.rows.length}</span>}
            </span>
          </button>
        </h3>
        {!folded && renderRows(section, rows, listId)}
      </section>
    );
  };

  /**
   * Snoozed, Settled and From terminals: shelves at the foot of the list under
   * one hairline, each a slim fold ("Settled · 14", the chevron at the right),
   * then slim rows. Folded by default.
   */
  const renderShelf = (section: ThreadInboxSection): ReactNode => {
    const folded = threadSectionFolded(model, folds, section.section) && dragKey === null;
    const rows = threadSectionRows(model, section, settledShown, fromTerminalsShown);
    const headId = `${uid}-${section.section}`;
    const listId = `${headId}-rows`;
    const dropping = drop?.section === section.section;
    return (
      <section key={section.section}
        className={dropping ? "thread-inbox-section thread-inbox-shelf-section is-drop-zone"
          : "thread-inbox-section thread-inbox-shelf-section"}
        data-thread-section={section.section} aria-labelledby={headId}
        onDragOver={event => { dragOver(event, section, false); }}
        onDrop={dropOn}>
        <h3 className="thread-inbox-shelf-head"
          onDragOver={event => { dragOver(event, section, true); }}>
          <button type="button" id={headId} className="thread-inbox-fold thread-inbox-shelf"
            aria-expanded={!folded} aria-controls={folded ? undefined : listId}
            aria-label={threadSectionAccessibleName(section, folded)}
            onClick={() => { setFold(section.section, !folds[section.section]); }}>
            <span className="thread-inbox-label">{section.label}</span>
            {section.rows.length > 0 && <span className="thread-inbox-count" aria-hidden="true">· {section.rows.length}</span>}
            <span className="thread-inbox-fold-end">
              {folded && section.mark !== "none" && (
                <span aria-hidden="true"><ThreadMarkGlyph mark={section.mark} /></span>
              )}
              <ThreadGlyph name="open" size={14} className={folded ? "thread-inbox-chevron" : "thread-inbox-chevron is-open"} />
            </span>
          </button>
        </h3>
        {!folded && renderRows(section, rows, listId)}
      </section>
    );
  };

  const renderEntry = (row: ThreadInboxRow, entry: ThreadMenuEntry): ReactNode => {
    if (isThreadSubmenu(entry)) {
      const open = menu?.submenu?.id === entry.id;
      return (
        <button key={entry.id} type="button" className="row-menu-item thread-menu-item" role="menuitem"
          tabIndex={-1} aria-haspopup="menu" aria-expanded={open} data-thread-submenu={entry.id}
          aria-controls={open ? `${uid}-thread-submenu` : undefined}
          onPointerEnter={event => { openSubmenu(entry.id, event.currentTarget, false); }}
          onClick={event => { openSubmenu(entry.id, event.currentTarget, true); }}
          onKeyDown={event => {
            if (event.key !== "ArrowRight" && event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            event.stopPropagation();
            openSubmenu(entry.id, event.currentTarget, true);
          }}>
          {entry.label}
          <ThreadGlyph name="open" size={14} className="thread-menu-trail" />
        </button>
      );
    }
    const chord = THREAD_MENU_CHORDS[entry.id];
    return (
      <button key={entry.id} type="button" className="row-menu-item thread-menu-item" role="menuitem" tabIndex={-1}
        data-thread-command={entry.id} aria-keyshortcuts={chord?.keys}
        onPointerEnter={() => {
          if (menu?.submenu !== null && menu !== null) setMenu({ key: menu.key, kind: menu.kind, submenu: null });
        }}
        onClick={() => {
          closeMenuAfter(row, entry.undo);
          act(row, entry.commands, entry.undo);
        }}>
        {entry.label}
        {chord !== undefined && <span className="thread-menu-key" aria-hidden="true">{chord.hint}</span>}
      </button>
    );
  };

  /**
   * Threads found by their messages (protocol 56), under their caption after
   * Pinned and Active: ordinary rows, the excerpt in the project's place.
   */
  const renderFound = (rows: readonly ThreadInboxRow[]): ReactNode => {
    const headId = `${uid}-in-messages`;
    const found: ThreadInboxSection = { section: "active", label: IN_MESSAGES_LABEL, rows, drawn: rows, mark: "none" };
    return (
      <section key="inMessages" className="thread-inbox-section thread-inbox-found" data-thread-found=""
        aria-labelledby={headId}>
        <h3 className="thread-inbox-heading">
          <span id={headId} className="thread-inbox-fold thread-inbox-found-head">
            <span className="thread-inbox-label">{IN_MESSAGES_LABEL}</span>
            <span className="thread-inbox-fold-end">
              <span className="thread-inbox-count" aria-hidden="true">{rows.length}</span>
            </span>
          </span>
        </h3>
        {renderRows(found, rows, `${headId}-rows`)}
      </section>
    );
  };

  const empty = model.total === 0 && model.inMessages.length === 0 && dragKey === null;
  return (
    <>
      <style>{THREAD_INBOX_CSS}</style>
      {empty ? (
        <div className="empty thread-inbox-empty">
          {scopeRoot !== undefined && model.unscoped > 0 ? (
            <>
              <p className="empty-text">No threads in {scopeName}.</p>
              <button type="button" className="sidebar-search-handoff"
                onClick={() => { inbox.setScope("all"); }}>
                Show all projects
              </button>
            </>
          ) : (
            <>
              <p className="empty-text">{emptyText}</p>
              {emptyHint !== null && <p className="empty-hint">{emptyHint}</p>}
            </>
          )}
          {onSearchTranscripts !== undefined && filterQuery !== "" && (
            <button type="button" className="sidebar-search-handoff"
              onClick={() => { onSearchTranscripts(filterQuery); }}>
              <ThreadGlyph name="search" size={14} />
              <span>Search transcripts for “{filterQuery}”</span>
            </button>
          )}
        </div>
      ) : (
        <div ref={listRef} className="session-list session-list-flat thread-inbox" aria-label="Threads">
          {shownSections.filter(section => !isShelf(section.section)).map(renderSection)}
          {model.inMessages.length > 0 && renderFound(model.inMessages)}
          {shownSections.some(section => isShelf(section.section)) && (
            <div className="thread-inbox-shelves">
              {shownSections.filter(section => isShelf(section.section)).map(renderShelf)}
            </div>
          )}
        </div>
      )}

      {toast !== null && (
        <div className="thread-inbox-toast" role="status">
          <span className="thread-inbox-toast-text" title={threadUndoMessage(toast)}>{threadUndoMessage(toast)}</span>
          <button type="button" className="thread-inbox-toast-undo" title="Undo (⌘Z)" aria-keyshortcuts="Meta+Z"
            onClick={runUndo}>
            Undo<span className="thread-menu-key" aria-hidden="true">⌘Z</span>
          </button>
        </div>
      )}

      {peekRow !== null && renameFor === null && (() => {
        const meta = sidebarRowMeta(threadSidebarRow(peekRow), diffStats[peekRow.key]);
        const card = (
          <SessionPeek cardRef={peekRef} id={`${uid}-thread-peek`} title={peekRow.title}
            badge={peekRow.status} status={meta.status} age={meta.age} diff={meta.diff}
            worktree={meta.worktree} pullRequest={threadPullRequestFact(model.records.get(peekRow.key))}
            facts={peeks[peekRow.key] ?? {}} box={peekBox} />
        );
        return createPortal(card, document.body);
      })()}

      {menuRow !== null && renameFor === null && (() => {
        const { lead, tail } = threadMenuEntries(menuRow, nowMs, {
          refreshPullRequest: threadTracksPullRequest(model.records.get(menuRow.key), model.settings),
          sectionKeys: model.sections.find(section => section.section === menuRow.section)?.rows
            .map(row => row.key),
        });
        const can = (op: Parameters<SidebarLifecycle["can"]>[1]): boolean =>
          lifecycle?.can(menuRow.engine, op) === true;
        if (menu?.kind === "snooze") {
          const snooze = lead.find((entry): entry is Extract<ThreadMenuEntry, { items: unknown }> =>
            isThreadSubmenu(entry) && entry.id === "snooze");
          if (snooze === undefined) return null;
          const snoozeNode = (
            <div ref={menuRef} id={`${uid}-thread-menu`} className="row-menu thread-menu thread-snooze-menu is-floating"
              role="menu" aria-label={`Snooze ${menuRow.title}`}
              onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}
              onKeyDown={event => { menuKeyNav(event, '[role="menuitem"]:not(:disabled)', () => { closeMenu(menuRow.key); }); }}
              style={menuBox === null ? { visibility: "hidden", top: 0, left: 0 } : { top: menuBox.top, left: menuBox.left }}>
              <div className="thread-menu-heading" aria-hidden="true">Snooze until</div>
              {snooze.items.map(item => (
                <button key={item.id} type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  data-thread-command={item.id}
                  onClick={() => {
                    closeMenuAfter(menuRow, item.undo);
                    act(menuRow, item.commands, item.undo);
                  }}>
                  {item.label}
                </button>
              ))}
            </div>
          );
          return createPortal(snoozeNode, document.body);
        }
        const submenu = menu?.submenu === null || menu?.submenu === undefined ? null
          : [...lead, ...tail].find((entry): entry is Extract<ThreadMenuEntry, { items: unknown }> =>
            isThreadSubmenu(entry) && entry.id === menu.submenu?.id) ?? null;
        const menuNode = (
          <div ref={menuRef} id={`${uid}-thread-menu`} className="row-menu thread-menu is-floating" role="menu"
            aria-label={`Actions for ${menuRow.title}`}
            onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}
            onKeyDown={event => {
              if (event.target instanceof Element && submenuRef.current?.contains(event.target) === true) return;
              menuKeyNav(event, '[role="menuitem"]:not(:disabled)', () => { closeMenu(menuRow.key); });
            }}
            style={menuBox === null ? { visibility: "hidden", top: 0, left: 0 } : { top: menuBox.top, left: menuBox.left }}>
            {onOpenInNewPane !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                onClick={() => { closeMenu(menuRow.key); onOpenInNewPane(menuRow.key); }}>
                Open in new pane
              </button>
            )}
            {onOpenInTerminal !== undefined && (() => {
              const refusal = threadTerminalRefusal(menuRow);
              return (
                <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  data-thread-bridge="terminal" disabled={refusal !== null}
                  title={refusal ?? "Resume this session in a Code pane. The thread stops driving it while the pane runs."}
                  onClick={() => { closeMenu(menuRow.key); onOpenInTerminal(menuRow.key); }}>
                  Open in terminal
                </button>
              );
            })()}
            {onHandToTeammate !== undefined && teammates.length > 0 && (
              <button type="button" className="row-menu-item thread-menu-item" role="menuitem" tabIndex={-1}
                aria-haspopup="menu" aria-expanded={menu?.submenu?.id === "teammate"} data-thread-submenu="teammate"
                aria-controls={menu?.submenu?.id === "teammate" ? `${uid}-thread-submenu` : undefined}
                onPointerEnter={event => { openSubmenu("teammate", event.currentTarget, false); }}
                onClick={event => { openSubmenu("teammate", event.currentTarget, true); }}
                onKeyDown={event => {
                  if (event.key !== "ArrowRight" && event.key !== "Enter" && event.key !== " ") return;
                  event.preventDefault();
                  event.stopPropagation();
                  openSubmenu("teammate", event.currentTarget, true);
                }}>
                Hand to teammate
                <ThreadGlyph name="open" size={14} className="thread-menu-trail" />
              </button>
            )}
            {offersFor !== undefined && (() => {
              const offers = offersFor(menuRow.engine, menuRow.projectRoot !== null);
              const wait = threadBridgeWait(menuRow.status);
              return <>
                {onSecondOpinion !== undefined && offers.opinion.map(choice => (
                  <button key={`opinion-${choice.engine}`} type="button" className="row-menu-item"
                    role="menuitem" tabIndex={-1} data-thread-opinion={choice.engine} disabled={wait !== null}
                    title={wait ?? `${choice.label} reads this thread's latest answer and its changes, then answers in the thread. It can't change files.`}
                    onClick={() => { closeMenu(menuRow.key); onSecondOpinion(menuRow.key, choice.engine); }}>
                    Second opinion from {choice.label}
                  </button>
                ))}
                {onSwitchTo !== undefined && offers.switchTo.map(choice => (
                  <button key={`switch-${choice.engine}`} type="button" className="row-menu-item"
                    role="menuitem" tabIndex={-1} data-thread-switch={choice.engine} disabled={wait !== null}
                    title={wait ?? `Carry on in this thread on ${choice.label}. It is told where things stand first.`}
                    onClick={() => { closeMenu(menuRow.key); onSwitchTo(menuRow.key, choice.engine); }}>
                    Switch to {choice.label}
                  </button>
                ))}
              </>;
            })()}
            {lead.map(entry => renderEntry(menuRow, entry))}
            {can("renameSession") && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                title="Retitles the session in the engine's own store, so the CLI sees it too."
                onClick={() => {
                  setMenu(null);
                  setRenameDraft(menuRow.title);
                  setRenameFor(menuRow.key);
                }}>
                Rename…
              </button>
            )}
            {tail.map(entry => renderEntry(menuRow, entry))}
            {can("forkSession") && lifecycle !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                onClick={() => { closeMenu(menuRow.key); lifecycle.onFork(menuRow.key); }}>
                Fork
              </button>
            )}
            {can("adoptSession") && lifecycle !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                onClick={() => { closeMenu(menuRow.key); lifecycle.onAdopt(menuRow.key, menuRow.external); }}>
                {menuRow.external ? <>Add to <ProductName /></> : <>Remove from <ProductName /></>}
              </button>
            )}
            <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1} data-thread-command="archive"
              title={`Archive this thread in ${productLabel()}. It stays in the engine's own store, and Undo or Library > Archived puts it back.`}
              onClick={() => { closeMenuInPlaceOf(menuRow.key); archive(menuRow); }}>
              Archive
            </button>
            {can("setArchived") && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                onClick={() => { setMenu(null); setConfirm({ key: menuRow.key, kind: "engine" }); }}>
                Archive in {menuRow.engine}…
              </button>
            )}
            {can("deleteSession") && (
              <button type="button" className="row-menu-item row-menu-danger" role="menuitem" tabIndex={-1}
                onClick={() => { setMenu(null); setConfirm({ key: menuRow.key, kind: "delete" }); }}>
                Delete…
              </button>
            )}
          </div>
        );
        const teammateMenu = menu?.submenu?.id === "teammate" && onHandToTeammate !== undefined;
        const submenuNode = teammateMenu ? (
          <div ref={submenuRef} id={`${uid}-thread-submenu`} className="row-menu thread-submenu is-floating"
            role="menu" aria-label="Hand to teammate"
            onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}
            onKeyDown={event => {
              if (event.key === "ArrowLeft") {
                event.preventDefault();
                closeSubmenu();
                return;
              }
              menuKeyNav(event, SUBMENU_ITEMS, closeSubmenu);
            }}
            style={submenuBox === null ? { visibility: "hidden", top: 0, left: 0 } : { top: submenuBox.top, left: submenuBox.left }}>
            {teammates.map(teammate => (
              <button key={teammate.id} type="button" className="row-menu-item thread-menu-item" role="menuitem"
                tabIndex={-1} data-thread-teammate={teammate.id}
                onClick={() => { closeMenu(menuRow.key); onHandToTeammate(menuRow.key, teammate.id); }}>
                {teammate.name}
              </button>
            ))}
          </div>
        ) : submenu === null ? null : (
          <div ref={submenuRef} id={`${uid}-thread-submenu`} className="row-menu thread-submenu is-floating"
            role="menu" aria-label={submenu.label}
            onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}
            onKeyDown={event => {
              if (event.key === "ArrowLeft") {
                event.preventDefault();
                closeSubmenu();
                return;
              }
              menuKeyNav(event, SUBMENU_ITEMS, closeSubmenu);
            }}
            style={submenuBox === null ? { visibility: "hidden", top: 0, left: 0 } : { top: submenuBox.top, left: submenuBox.left }}>
            {submenu.items.map((item: ThreadMenuCommand) => (
              <button key={item.id} type="button" className="row-menu-item thread-menu-item"
                role={item.checked === undefined ? "menuitem" : "menuitemradio"}
                tabIndex={-1} data-thread-command={item.id}
                aria-checked={item.checked}
                onClick={() => {
                  closeMenuAfter(menuRow, item.undo);
                  act(menuRow, item.commands, item.undo);
                }}>
                {item.checked === true && <ThreadGlyph name="pass" size={12} className="thread-menu-check" />}
                {item.checked === false && <span className="thread-menu-check" aria-hidden="true" />}
                {item.label}
              </button>
            ))}
          </div>
        );
        return <>{createPortal(menuNode, document.body)}{submenuNode !== null && createPortal(submenuNode, document.body)}</>;
      })()}

      {confirm !== null && confirmRow !== null && (
        <ConfirmDialog open
          {...sessionConfirmDialog(confirmRow, confirm.kind, lifecycle?.liveKeys.has(confirmRow.key) === true)}
          onConfirm={() => {
            const { key, kind } = confirm;
            setConfirm(null);
            if (lifecycle === undefined) return;
            if (kind === "engine") lifecycle.onEngineArchive(key);
            else lifecycle.onDelete(key);
          }}
          onCancel={() => {
            const key = confirm.key;
            setConfirm(null);
            restoreFocus(key);
          }} />
      )}
    </>
  );
}
