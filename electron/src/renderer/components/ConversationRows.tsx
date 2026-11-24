import {
  foldEditedFileRows, isShellTool, mergeFileChangeRows, repeatedToolLabel,
  type ConversationDisplayRow,
} from "../conversation-display";
import { VisualReplyCard } from "./VisualReply";
import { Icon, StateGlyph } from "./glyphs";
import "./Conversation.css";
import {
  memo, useCallback, useContext, useEffect, useId, useLayoutEffect, useMemo, useRef, useState,
  useSyncExternalStore,
} from "react";
import type { ReactElement, ReactNode, RefObject } from "react";
import { equalValue } from "../stable-values";
import type { AgentQuestion } from "../../shared/questions";
import {
  APPROVAL_ALLOW_KEYS, APPROVAL_ALLOW_KEYSHORTCUTS, APPROVAL_DENY_KEYS,
  APPROVAL_DENY_KEYSHORTCUTS, APPROVAL_KEY_ATTR, type ApprovalKeyVerdict, approvalVerdictKey,
} from "../approval-keys";
import { compactSeamLabel } from "../conversation-reducer";
import {
  commandRunFailureLabel, editedFilesLabel, editedFilesStat, fileChangeTargetText,
  fileChangeVerbWord, fileChangeVerbs, findSubagentItem, subagentGlyph, subagentRowDescription,
  subagentStatusWord, toolActivityGroupName, toolActivityName, toolGlyph, toolInputBlocks,
  toolRowSummaryText, workGroupParts, workGroupPartsLabel, turnChangesLabel,
} from "../tool-activity";
import { displayPath, shortenSummaryPaths } from "../display-paths";
import {
  mountedPace, paceArrival, paceFrame, prefersReducedMotion, settledPace,
} from "../transcript-navigation";
import { transcriptChildKey } from "../transcript-search";
import { transcriptDensityOpensItems } from "../transcript-density";
import type { ConvItem } from "../conversation-reducer";
import type {
  FileChangeOutcome, FileChangeVerb, RunState, TurnChangeSummary, WorkGroupPart,
} from "../tool-activity";
import type { TranscriptDensity } from "../transcript-density";
import { diffStatLabel, formatDiffStat, type DiffStat } from "../diff-stats";
import { formatTurnElapsed } from "../session-activity";
import { doneLineText, workingLineText } from "../session-status";
import { Markdown } from "../markdown";
import {
  threadDockCommand, threadDockDiff, threadDockFiles, threadDockKind, threadDockMeta,
  threadDockQuestionTitle, threadDockTitle, type ThreadApproval, type ThreadDockKind,
} from "../thread-frame";
import { threadStepLine, threadStepWords } from "../thread-live";
import { PaneVisibleContext } from "./ConversationPaneSurface";
import { FileBadge } from "./ProjectHueParts";
import { ThreadGlyph, type ThreadGlyphName } from "./ThreadGlyph";
import "../theme.css";

import type {
  SessionConfigView, ThreadTurnControl, TurnRestorePoint,
} from "../conversation-types";
import {
  extractPromptTurnAttachments, resolveTurnImageAttachmentPath, type PromptTurnAttachment,
} from "../prompt-context";
import type { ConversationProps } from "./Conversation";

/**
 * The transcript's rows: every item renderer (prose, user turns, tool and file
 * rows, folded runs, subagent cards and grids, approval and question cards),
 * their memo boundaries and comparators, and the small pieces the pane shares
 * (the turn clock, the document chrome). `ConversationTranscript` lays
 * them out; nothing here owns pane state.
 */

export interface FindRenderContext {
  active: boolean;
  matches: ReadonlySet<string>;
  currentKey: string | null;
  register: (key: string, node: HTMLDivElement | null) => void;
}

/**
 * The key a button answers to, printed on its own face. Hidden from the
 * accessibility tree on purpose: the button already carries the same binding as
 * aria-keyshortcuts, and a screen reader reading "Allow ⌘ ↩" as part of the
 * name is worse than reading "Allow" with a shortcut announced as one.
 *
 * The sidebar row prints the identical chip off the identical
 * constants, so the two surfaces cannot teach different keys.
 */
function KeyHint({ keys, on }: { keys: readonly string[]; on: boolean }) {
  if (!on) return null;
  return <span className="kbd-hint" aria-hidden="true">{keys.join("")}</span>;
}

/**
 * The other two marks of the chord: the attribute the pane's listener finds the
 * button by, and the announced binding. Both are absent together with the chip
 * on a pane that does not own the chord — an unfocused tile must not advertise,
 * or answer to, a key that would land on its neighbour.
 */
function approvalKeyProps(keyed: boolean, verdict: ApprovalKeyVerdict):
    Record<string, string> {
  if (!keyed) return {};
  return {
    [APPROVAL_KEY_ATTR]: verdict,
    "aria-keyshortcuts": verdict === "allow"
      ? APPROVAL_ALLOW_KEYSHORTCUTS : APPROVAL_DENY_KEYSHORTCUTS,
  };
}

// The shape of a transcript, drawn while the real one is being read. Every bar
// is one of the three things this pane actually renders, at the size it renders
// it: a user card on the right, prose lines, and 22px tool rows. It is entirely
// decorative — the status line above it is what a screen reader hears — so it
// is hidden from the accessibility tree rather than announced twice.
export function TranscriptSkeleton() {
  return (
    <div className="conv-skeleton" aria-hidden="true">
      <div className="conv-skel-user" />
      <div className="conv-skel-line" />
      <div className="conv-skel-line conv-skel-line-b" />
      <div className="conv-skel-line conv-skel-line-c" />
      <div className="conv-skel-group">
        <div className="conv-skel-row" />
        <div className="conv-skel-row" />
        <div className="conv-skel-row" />
      </div>
      <div className="conv-skel-line" />
      <div className="conv-skel-line conv-skel-line-c" />
    </div>
  );
}

/**
 * ONE COLLAPSIBLE THING'S FOLD, under the pane's density dial.
 *
 * The card owns its fold — clicking it must still work, and must still stick —
 * but the dial owns where that fold STARTS. PRESSING the dial re-seeds every
 * card on screen, which is what turns a three-way switch into collapse-all and
 * expand-all without a second pair of buttons for them.
 *
 * Pressing, not changing. The seed is the step AND the press that set it, so
 * re-pressing the step already showing re-seeds too. Without that the headline
 * action is unreachable from the state every session starts in: the shipped
 * default is Balanced, "Balanced" is what a user who has opened four cards
 * wants to press to shut them, and a value-only seed makes that click inert.
 *
 * The re-seed happens during render, using React's documented "adjust state
 * when a prop changes": the pass is thrown away and re-run immediately, so the
 * DOM is committed once and no card is ever painted at the old fold under the
 * new dial. An effect would paint the stale frame first, and across forty rows
 * that is a visible flash of the thing the user just asked to hide.
 */
function useDensityFold(
  density: TranscriptDensity, epoch: number, openByDefault = false,
): [boolean, () => void] {
  const wanted = transcriptDensityOpensItems(density) || openByDefault;
  const [open, setOpen] = useState(wanted);
  // `openByDefault` is part of the seed, so a call that fails while it is on
  // screen opens the way it would have opened had it failed before mounting.
  const seed = `${density}#${String(epoch)}${openByDefault ? "#open" : ""}`;
  const [seenSeed, setSeenSeed] = useState(seed);
  if (seenSeed !== seed) {
    setSeenSeed(seed);
    setOpen(wanted);
  }
  return [open, () => { setOpen(current => !current); }];
}

// One disclosure affordance, used by every collapsible thing in the pane.
function Chevron({ open, className }: { open: boolean; className?: string }) {
  const extra = className === undefined ? "" : ` ${className}`;
  return <Icon name="chevron-down"
    className={`conv-chev${open ? " conv-chev-open" : ""}${extra}`} />;
}

const readDocumentChrome = (): "cli" | "refined" =>
  typeof document !== "undefined" && document.documentElement.dataset.chrome === "cli"
    ? "cli" : "refined";

// ONE observer on data-chrome for the whole document, however many panes and
// approval cards read it: installed by the first subscriber, removed with the
// last. A static render (no document) reads "refined".
const chromeListeners = new Set<() => void>();
let chromeObserver: MutationObserver | null = null;

function subscribeDocumentChrome(listener: () => void): () => void {
  chromeListeners.add(listener);
  if (chromeObserver === null && typeof document !== "undefined") {
    chromeObserver = new MutationObserver(() => {
      for (const notify of chromeListeners) notify();
    });
    chromeObserver.observe(document.documentElement,
      { attributes: true, attributeFilter: ["data-chrome"] });
  }
  return () => {
    chromeListeners.delete(listener);
    if (chromeListeners.size > 0 || chromeObserver === null) return;
    chromeObserver.disconnect();
    chromeObserver = null;
  };
}

export function useDocumentChrome(): "cli" | "refined" {
  return useSyncExternalStore(subscribeDocumentChrome, readDocumentChrome, readDocumentChrome);
}

// Takes the whole prop bag rather than the three callbacks it uses directly: a
// subagent's children are rendered by this same function, so it has to be able
// to hand itself down intact.
// The return type is annotated on purpose. Without it TS infers `… | undefined`
// from a switch with no default, and then a NEW ConvItem member compiles clean
// and renders nothing at all — which is exactly how the user turn shipped
// invisible: the item kind existed, the engines emitted it, and the switch below
// silently returned undefined for it. The `never` arm turns that into a build
// failure at the one place that has to change.
type ConversationItemProps = {
  item: ConvItem;
  findKey: string;
  find: FindRenderContext;
  /** The pane's dial. Every collapsible thing below re-seeds its fold from it. */
  density: TranscriptDensity;
  /** Which press of that dial this is. See useDensityFold. */
  densityEpoch: number;
  /**
   * The subagent whose children this item is one of, when it is nested. It is
   * the only way a nested card can name its parent, and naming it is what makes
   * the fold structure a TREE for a screen reader rather than a flat run of
   * identical "… subagent" landmarks at four different depths.
   */
  parentSubagent?: string;
  /**
   * The paths of the diff rows drawn DIRECTLY under this one, when it is a tool
   * row that produced any. It is what lets the row stop re-listing the files
   * its own children already name (see toolRowSummaryText). Absent means "no
   * diff rows follow", which is the honest default for a row whose siblings are
   * not known — the members of a folded run and the children of a subagent card
   * both take that path, and both keep their summaries in full.
   */
  followingDiffPaths?: readonly string[];
  /**
   * What this row's turn changed, when it is a user turn that produced diffs.
   * Absent is "nothing this window saw", and nothing is what gets drawn.
   */
  turnChanges?: TurnChangeSummary;
  /** Open the restore confirmation. Absent means this pane cannot restore. */
  onRequestRestore?: (turnKey: string) => void;
  /**
   * The Rust thread turn this user row is, and what its thread allows. Absent
   * (every non-thread pane, and a row that maps to no turn) draws no thread
   * control at all.
   */
  threadTurn?: ThreadTurnControl;
  /** A thread control on this row was pressed. Stable; required with `threadTurn`. */
  onThreadTurn?: (action: "diff" | "edit", turnKey: string) => void;
  /** Session state, threaded by identity instead of spread. */
  all: ConversationItemContext;
};

/**
 * EVERY prop a transcript row reads, and nothing else.
 *
 * The row comparator selects the fields relevant to that row kind. Approval
 * membership and restore/subagent selection are compared per row, so changing
 * one row never invalidates the settled transcript. Add new reads here and to
 * sameConversationItemProps together to avoid stale memoized output — and,
 * for a read inside a run, file-change or grid row, to sameWorkRowProps.
 */
export type ConversationItemContext = Pick<ConversationProps,
  "activeApprovalIds" | "approvals" | "focused" | "live"
  | "onApprove" | "onDeny" | "onPreviewAttachment" | "onSelectSubagent" | "onSignIn" | "pathRoot" | "restoringTurnKey"
  | "selectedSubagentKey" | "sessionKey" | "turnRestorePoints" | "visualFrames"> & {
    config: Pick<SessionConfigView, "engineLabel">;
    keyedApprovalId: string | null;
    /**
     * A Threads pane's layout (owner, 2026-10-06): a failed call opens its
     * output, and an approval card names its question and puts Allow last.
     */
    threadLayout?: boolean;
    /** Active approvals the agent policy is still deciding (see approvalGate). */
    checkingApprovalIds: ReadonlySet<string>;
    /**
     * T3 section 9: put a reply's selected text in this pane's draft as a
     * quote. Absent (a pane with no composer of its own) offers no Quote.
     */
    onQuoteReply?: (text: string) => void;
  };

/** How long the turn ending on this row took, or nothing where it is not known. */
export function TurnClock({ ms, cliChrome }: { ms?: number; cliChrome: boolean }): ReactElement | null {
  if (ms === undefined) return null;
  return (
    <div className="conv-turn-clock">
      {cliChrome ? doneLineText(ms) : formatTurnElapsed(ms)}
    </div>
  );
}

/**
 * The only once-per-second state in a live conversation.
 *
 * Keeping the timer in this leaf means its tick updates two short text nodes;
 * it cannot reconcile the transcript rows, composer, approvals, or subagents in
 * Conversation above it. The fixed start remains owned by the shell/parent.
 */
export function LiveTurnElapsed({ startedAt, cliChrome, label }: {
  startedAt: number;
  cliChrome: boolean;
  label: string;
}): ReactElement {
  const [elapsedMs, setElapsedMs] = useState(() => Math.max(0, Date.now() - startedAt));
  useEffect(() => {
    const tick = (): void => { setElapsedMs(Math.max(0, Date.now() - startedAt)); };
    tick();
    const id = window.setInterval(tick, 1_000);
    return () => window.clearInterval(id);
  }, [startedAt]);
  if (cliChrome) {
    return (
      <span className="conv-working-label" aria-hidden="true">
        {/* tokens: undefined — the app has no per-turn figure. See
            workingLineText for the measurement that does not exist. */}
        {workingLineText(elapsedMs, undefined, label)}
      </span>
    );
  }
  return (
    <span className="conv-working-elapsed" aria-hidden="true"
      title={`${formatTurnElapsed(elapsedMs)} elapsed`}>
      {formatTurnElapsed(elapsedMs)}
    </span>
  );
}

/**
 * The find highlight one row carries: the class string plus the two attributes
 * that go with it. Rows own one key or several — a merged file change stands
 * where a run of rows used to — and a hit on any of them lights the same row.
 */
function findItemClass(keys: readonly string[], find: FindRenderContext): {
  matched: boolean; current: boolean; className: string;
} {
  const matched = find.active && keys.some(key => find.matches.has(key));
  const current = matched && keys.some(key => find.currentKey === key);
  return {
    matched,
    current,
    className: current
      ? "conv-find-item conv-find-hit conv-find-current"
      : matched ? "conv-find-item conv-find-hit" : "conv-find-item",
  };
}

/**
 * MEMOIZED, because this is the transcript's per-row boundary: a thousand-row
 * session re-ran a thousand of these on every streamed token. Every prop it
 * takes is now stable across a frame that changed none of them — `all` is the
 * pane's one useMemo'd ConversationItemContext, `find` is memoized beside it,
 * and `followingDiffPaths`/`turnChanges` come out of maps keyed on `rows`.
 *
 * `Item` below is deliberately NOT wrapped: it stays a plain function because
 * this file's own tests call it directly.
 */
export const SearchableItem = memo(function SearchableItem(
  props: ConversationItemProps,
): ReactElement {
  const { matched, current, className } = findItemClass([props.findKey], props.find);
  return (
    <div
      ref={node => props.find.register(props.findKey, node)}
      className={className}
      data-transcript-match={matched ? "true" : undefined}
      data-current-match={current ? "true" : undefined}
      aria-current={current ? "true" : undefined}
    >
      <Item {...props} />
    </div>
  );
}, sameConversationItemProps);

/** Every own prop identical, read without allocating: the settled row's case
 * on a streamed frame, which the deep comparison below would also answer true. */
function sameOwnProps(previous: object, next: object): boolean {
  const before = previous as Record<string, unknown>;
  const after = next as Record<string, unknown>;
  let count = 0;
  for (const key in after) {
    count += 1;
    if (!Object.is(before[key], after[key]) || !Object.hasOwn(before, key)) return false;
  }
  for (const _key in before) count -= 1;
  return count === 0;
}

/** Compare only state a row (or its descendants) can actually read. */
export function sameConversationItemProps(previous: ConversationItemProps, next: ConversationItemProps): boolean {
  if (sameOwnProps(previous, next)) return true;
  const { all: a, find: af, ...ap } = previous;
  const { all: b, find: bf, ...bp } = next;
  if (!equalValue(ap, bp) || a.sessionKey !== b.sessionKey) return false;
  if (next.item.kind === "subagent") return equalValue(a, b) && equalValue(af, bf);
  const key = next.findKey;
  if (af.register !== bf.register
      || !equalValue(findItemClass([key], af), findItemClass([key], bf))) return false;
  switch (next.item.kind) {
    case "text": return a.live === b.live && a.threadLayout === b.threadLayout
      && a.onPreviewAttachment === b.onPreviewAttachment && a.onQuoteReply === b.onQuoteReply;
    case "tool": return a.pathRoot === b.pathRoot && a.threadLayout === b.threadLayout;
    case "diff": return a.pathRoot === b.pathRoot;
    case "user": return (a.restoringTurnKey != null) === (b.restoringTurnKey != null)
      && (a.restoringTurnKey === key) === (b.restoringTurnKey === key)
      && a.onPreviewAttachment === b.onPreviewAttachment
      && equalValue(a.turnRestorePoints?.get(key), b.turnRestorePoints?.get(key));
    case "approval": {
      const id = next.item.id;
      const verdictKey = approvalVerdictKey(b.sessionKey ?? "", id);
      return a.onApprove === b.onApprove && a.onDeny === b.onDeny
        && a.config.engineLabel === b.config.engineLabel && a.threadLayout === b.threadLayout
        && (a.live && a.activeApprovalIds.has(id)) === (b.live && b.activeApprovalIds.has(id))
        && (a.focused === true && a.keyedApprovalId === id) === (b.focused === true && b.keyedApprovalId === id)
        && a.checkingApprovalIds.has(id) === b.checkingApprovalIds.has(id)
        && equalValue(a.approvals[verdictKey], b.approvals[verdictKey]);
    }
    case "notice": return next.item.signIn !== true || a.onSignIn === b.onSignIn;
    case "peer": case "compact": return true;
    case "visual": return a.visualFrames === b.visualFrames;
  }
}

/**
 * THE RUN-LEVEL ROWS READ ALMOST NOTHING FROM `all`. A folded run, a merged
 * file change and every row they draw inside themselves are tool and diff
 * rows, which read `pathRoot` alone (sameConversationItemProps' tool/diff
 * case); a subagent grid's tiles add the selection. So an approval verdict, a
 * focus change or a restore point elsewhere in the pane leaves them alone.
 * Every other prop is compared by identity, exactly as memo() would. A new
 * read of `all` in ToolRun, FileChangeRow or SubagentGrid belongs here too.
 */
function sameExceptContext(previous: object, next: object): boolean {
  const before = previous as Record<string, unknown>;
  const after = next as Record<string, unknown>;
  let count = 0;
  for (const key in after) {
    count += 1;
    if (key !== "all" && !(Object.hasOwn(before, key) && Object.is(before[key], after[key]))) return false;
  }
  for (const _key in before) count -= 1;
  return count === 0;
}

export function sameWorkRowProps<P extends { all: ConversationItemContext }>(previous: P, next: P): boolean {
  return sameExceptContext(previous, next) && (previous.all === next.all
    || (previous.all.sessionKey === next.all.sessionKey
      && previous.all.pathRoot === next.all.pathRoot));
}

export function sameSubagentGridProps<P extends { all: ConversationItemContext }>(previous: P, next: P): boolean {
  return sameWorkRowProps(previous, next) && (previous.all === next.all
    || (previous.all.onSelectSubagent === next.all.onSelectSubagent
      && previous.all.selectedSubagentKey === next.all.selectedSubagentKey));
}

function expandWorkGroup(
  run: Extract<ConversationDisplayRow, { kind: "tool-run" }>,
): ConversationDisplayRow[] {
  const source = run.members ?? run.rows;
  return foldEditedFileRows(mergeFileChangeRows(
    source.map(row => ({ kind: "row" as const, row })),
  ));
}

function WorkGroupText({ parts }: { parts: readonly WorkGroupPart[] }): ReactElement {
  return (
    <>
      {parts.map((part, index) => {
        const stat = part.added === undefined
          ? null : { added: part.added, removed: part.removed ?? 0 };
        return (
          <span key={`${part.text}-${String(index)}`}>
            {index > 0 ? ", " : ""}
            {part.text}
            <DiffStatText stat={stat} addClass="conv-work-add" delClass="conv-work-del" />
          </span>
        );
      })}
    </>
  );
}

/**
 * DECLINED IS NOT FAILED. itemOk collapses both to ok:false because neither is
 * a success, and the result carries no structured status — so the word is read
 * back out of the result summary, by exact match on the one status string the
 * codex mapper puts there and nothing looser.
 *
 * ONE rule, shared by the folded run line and the row it folds away, because
 * those are the same claim about the same call and must never disagree. A patch
 * the USER refused read "failed" while folded and "declined" once opened, which
 * is the fold telling them their own decision was an error — and compact, where
 * a lone edit now folds too, sends far more patches down that path.
 */
function isDeclinedResult(result: { ok: boolean; summary: string } | undefined): boolean {
  return result !== undefined && !result.ok
    && result.summary.trim().toLowerCase() === "declined";
}

export const ToolRun = memo(function ToolRun({ run, all, find, density, densityEpoch }: {
  run: Extract<ConversationDisplayRow, { kind: "tool-run" }>;
  all: ConversationItemContext;
  find: FindRenderContext;
  density: TranscriptDensity;
  densityEpoch: number;
}) {
  const [open, toggleOpen] = useDensityFold(density, densityEpoch);
  const findKeys = [
    ...run.rows.map(row => row.key),
    ...(run.members ?? []).map(row => row.key),
  ];
  const forceOpen = find.active && findKeys.some(key => find.currentKey === key);
  const shownOpen = open || forceOpen;
  // Only an open run draws its members, so only an open run expands them: runs
  // are folded by default at balanced and compact.
  const expanded = useMemo(() => shownOpen ? expandWorkGroup(run) : null, [run, shownOpen]);
  const commands = run.tool === "command"
    || run.rows.every(row => isShellTool(row.item.tool));
  const running = run.rows.some(row => row.item.result === undefined);
  const name = toolActivityGroupName(run.tool);
  const kind = toolGlyph(run.tool);
  const mixed = run.tool === "work";
  // A FINISHED run of one keeps its command. Shell rows fold regardless of
  // count, so the fold was replacing "Bash · npx vitest run" with "Ran 1
  // command" — thirteen characters that name nothing, from a line that had
  // named the one thing the user came here to read. Folding is supposed to
  // remove repetition, not information. While it is still RUNNING the label
  // stays "Running command", because in-flight is the fact that outranks the
  // text and the busy strip below already names the live tool.
  const single = commands && !mixed && run.rows.length === 1 && !running
    ? toolRowSummaryText(run.rows[0].item, { root: all.pathRoot })
    : "";
  const tools = run.rows.map(row => row.item);
  const parts = workGroupParts({
    tools,
    diffs: run.diffs,
    running,
    root: all.pathRoot,
  });
  const label = single !== "" ? single
    : mixed || commands
    ? workGroupPartsLabel(parts)
    : repeatedToolLabel(run.tool, name, run.rows.length, running);
  // Success stays wordless, exactly as it is on a tool row. A FAILURE does not:
  // a folded run said "Ran 1 command" in the same dim grey whether the command
  // passed or blew up, so the one state worth surfacing was the one the fold
  // hid. isShellToolRow folds on the tool name alone and deliberately still
  // does — the run is real either way; what it owes the reader is a verdict.
  // A REFUSAL IS NOT A FAILURE, and the folded line is where that distinction
  // was being lost: commandRunFailureLabel knows one word, so a patch the user
  // declined reported itself as broken. Counted apart and worded apart, in the
  // same slot, with the same glyph FileChangeRow uses for both.
  const declined = run.rows.filter(row => isDeclinedResult(row.item.result)).length;
  const failed = run.rows.filter(row => row.item.result?.ok === false).length - declined;
  const failure = [
    commandRunFailureLabel(failed, run.rows.length),
    declined === 0 ? ""
      : run.rows.length === 1 ? "declined" : `${String(declined)} declined`,
  ].filter(word => word !== "").join(", ");
  const exactTools = [...new Set(run.rows.map(row => row.item.tool))].join(", ");
  const firstResult = run.rows[0]?.item.result?.summary.trim().split("\n")[0] ?? "";
  const statusLabel = failure === "" ? label : `${label}, ${failure}`;
  const accessibleLabel = `${statusLabel}. Tools: ${exactTools}`;
  return (
    <details className={commands || mixed
      ? "conv-tool-run conv-tool-run-commands" : "conv-tool-run"}
      open={shownOpen}
      onToggle={event => {
        // The current search hit owns this disclosure while it is selected.
        // Native <details> otherwise lets the summary close its DOM state
        // without a React render, leaving the selected child mounted but
        // invisible behind a closed parent.
        if (forceOpen) {
          if (!event.currentTarget.open) event.currentTarget.open = true;
          return;
        }
        if (event.currentTarget.open !== open) toggleOpen();
      }}>
      <summary className="conv-tool-run-summary" aria-label={accessibleLabel}
        title={`${exactTools} · ${run.rows.length} ${run.rows.length === 1 ? "call" : "calls"}`}
        onClick={event => { if (forceOpen) event.preventDefault(); }}>
        {!commands && !mixed && kind !== null && <Icon name={kind} className="conv-kind" />}
        <span className="conv-shrink">
          {single !== "" || parts.length === 0 ? label : <WorkGroupText parts={parts} />}
        </span>
        {/* What the first call in this run came back with. In refined chrome it
            is hidden by the pane's own rule; in CLI it is the ⎿ line under the
            bullet, which is the whole point of a folded run being readable
            without opening it. */}
        {firstResult !== "" && (
          <span className="conv-sum conv-run-result">{firstResult}</span>
        )}
        {/* No "completed" word. The label already reads "6 Read calls"; a
            trailing status that is the same on every finished run is the same
            noise the tool rows just shed. The chevron is the affordance. */}
        {failure !== "" && (
          <span className="conv-state conv-inline conv-state-fail">
            <StateGlyph state="failed" />
            {failure}
          </span>
        )}
        <Chevron open={shownOpen} />
      </summary>
      {expanded !== null && (
        <div className="conv-tool-run-items">
          {expanded.map(entry => {
            if (entry.kind === "file-change") {
              return <FileChangeRow key={entry.key} display={entry} all={all}
                find={find} density={density} densityEpoch={densityEpoch} />;
            }
            if (entry.kind === "edited-files") {
              return <EditedFilesRow key={entry.key} run={entry} find={find}
                pathRoot={all.pathRoot} />;
            }
            if (entry.kind === "tool-run") {
              return entry.rows.map(row => (
                <SearchableItem key={row.key} item={row.item} all={all}
                  findKey={row.key} find={find} density={density}
                  densityEpoch={densityEpoch} />
              ));
            }
            if (entry.kind === "subagent-grid") return null;
            const row = entry.row;
            if (row.kind !== "item") return null;
            return <SearchableItem key={row.key} item={row.item} all={all}
              findKey={row.key} find={find} density={density}
              densityEpoch={densityEpoch} />;
          })}
        </div>
      )}
    </details>
  );
}, sameWorkRowProps);

/**
 * "+12 −4" the way every diff tool draws a diffstat: the added half on
 * --diff-added, the removed half on --diff-removed. Both halves used to print
 * in --fg-muted, which spent the only quantitative fact on these rows on the
 * same grey as the words beside it.
 *
 * THE SILENCE RULE STAYS IN formatDiffStat and is re-read here rather than
 * re-implemented, so an unknown or net-zero stat still prints nothing at all
 * and this cannot start claiming "+0 −0" where the string version stayed quiet.
 *
 * The leading space is for the inline caller (EditedFilesRow's label). Inside
 * .conv-inline, which is a flex container, a whitespace-only anonymous item is
 * not rendered at all and the 4px gap separates the halves instead — so one
 * component serves both without either caller owning a separator.
 *
 * Nothing here is an accessible name: both callers keep the sentence they had.
 *
 * The folded work line paints the same two halves in its own pair of classes,
 * so they are a parameter rather than a second copy of this component.
 */
function DiffStatText({ stat, addClass = "conv-stat-add", delClass = "conv-stat-del" }: {
  stat: DiffStat | null;
  addClass?: string;
  delClass?: string;
}): ReactElement | null {
  if (stat === null || formatDiffStat(stat) === "") return null;
  return (
    <>
      {" "}
      <span className={addClass}>+{stat.added}</span>
      {" "}
      <span className={delClass}>−{stat.removed}</span>
    </>
  );
}

export const EditedFilesRow = memo(function EditedFilesRow({ run, find, pathRoot }: {
  run: Extract<ConversationDisplayRow, { kind: "edited-files" }>;
  find: FindRenderContext;
  pathRoot?: string;
}): ReactElement {
  const stat = editedFilesStat(run.items);
  // One string for the row's name, two painted spans for the eye. The null
  // argument asks the same helper for the file-count half alone, so the
  // visible row and the label cannot drift into saying different numbers.
  const label = editedFilesLabel(run.paths.length, stat);
  const title = run.paths.map(path => displayPath(path, pathRoot)).join("\n");
  return (
    <div {...findKeysProps(run.keys, find)} className="conv-find-item">
    <div className="conv-card">
      <div className="conv-row conv-diffrow" title={title} aria-label={label}>
        <Icon name="git-compare" className="conv-kind" />
        <span className="conv-sum">
          {editedFilesLabel(run.paths.length, null)}
          <DiffStatText stat={stat} />
        </span>
      </div>
    </div>
    </div>
  );
});

/**
 * THE WRAPPER PROPS THAT KEEP A COLLAPSED ROW FINDABLE UNDER EVERY KEY IT
 * ABSORBED. A find hit lands on a KEY, and a row standing where a tool row and
 * its diff rows used to stand has to answer to all of them or the counter
 * reports a match that highlights nothing and scrolls nowhere.
 *
 * The keys are ALSO printed as an attribute, and that is not decoration: a ref
 * never runs in a static render, so a registration wired to the wrong array is
 * a defect no markup assertion can see — which is exactly how it went unpinned
 * here before. One argument feeds both the registration and the attribute, so
 * the attribute is evidence about the registration rather than a second thing
 * to keep in step with it.
 */
export function findKeysProps(keys: readonly string[], find: FindRenderContext): {
  ref: (node: HTMLDivElement | null) => void;
  "data-find-keys": string;
} {
  return {
    ref: (node: HTMLDivElement | null) => {
      for (const key of keys) find.register(key, node);
    },
    "data-find-keys": keys.join(" "),
  };
}

/**
 * A FILE CHANGE, ONCE. See mergeFileChangeRows for why the pair exists at all;
 * this is the row that carries both halves' facts:
 *
 *   verb   the kind of change (fileChangeVerbWord), in the tense the OUTCOME
 *          earns — a refused patch says "Delete", never "Deleted"
 *   path   one file shortened against the root, or "N files", or — when the
 *          kinds differ and the verb has to go neutral — the breakdown that
 *          says a deletion happened without anyone hovering
 *   stat   +added −removed, on every state
 *   state  running… / failed / declined; success stays wordless as it does on
 *          every other row in this pane
 *
 * THE ROW STATES THE CHANGE IN EVERY STATE AND OPENS NOTHING. The editor it
 * used to navigate to is gone, so the row is a plain element carrying those
 * facts; the disclosure beside it is the only control here, and it stays a
 * SIBLING of the row rather than a child of it — the same two elements, the
 * same way round, as the subagent cell above. It was a sibling because a button
 * inside a button is invalid HTML and the inner one is unreachable by keyboard;
 * now that only the fold is pressable, the arrangement is what keeps the fold
 * reachable without the row wrapping it.
 *
 * Every state still spells its counts. Sizing the decision is the row's whole
 * job — a failed, refused or still-running patch is exactly the one a reader
 * needs measured, so nothing here is withheld until the patch settles.
 *
 * Distinguishability is carried by the WORD and its colour, and by nothing
 * else. Chevron DIRECTION carries none of it, whatever it may look like in
 * source: .conv-chev's closed state is rotate(-90deg), so a folded row's
 * chevron-down renders pointing right, identical to the navigator's.
 *
 * A CHEVRON ONLY WHERE THERE IS A FOLD. With a body, the fold button draws it;
 * with none, the slot stays empty. The static right chevron that used to fill
 * it read exactly like the collapsed fold two rows above and opened nothing,
 * and what the row IS — a file change — is already carried by the git-compare
 * glyph at its head.
 */
export const FileChangeRow = memo(function FileChangeRow({ display, all, find, density, densityEpoch }: {
  display: Extract<ConversationDisplayRow, { kind: "file-change" }>;
  all: ConversationItemContext;
  find: FindRenderContext;
  density: TranscriptDensity;
  densityEpoch: number;
}): ReactElement {
  const [open, toggleOpen] = useDensityFold(density, densityEpoch);
  const bodyId = useId();
  const item = display.row.item;
  const diffPaths = display.items.map(diff => diff.path);
  const paths = [...new Set(diffPaths)];
  const verbByPath = fileChangeVerbs(item, diffPaths);
  const verbs: FileChangeVerb[] = paths.map(path => verbByPath.get(path) ?? "unknown");
  const done = item.result;
  const outcome: FileChangeOutcome = done === undefined ? "running"
    : done.ok ? "ok" : "refused";
  const verb = fileChangeVerbWord(verbs, outcome);
  const target = fileChangeTargetText(verbs);
  const shown = target !== "" ? target : displayPath(paths[0] ?? "", all.pathRoot);
  const stat = editedFilesStat(display.items);
  // DECLINED is not FAILED — see isDeclinedResult, which the folded run line
  // reads too so the two can never word one refusal differently.
  const refused = isDeclinedResult(done);
  const state = outcome === "running" ? "running…" : refused ? "declined" : "failed";
  // EVERY path, absolute, one per line, each with the kind that was read for it
  // — built from the diff rows and NOT from item.summary. The engine cuts that
  // summary at 120 characters mid-path, so a three-file patch's third filename
  // existed nowhere on the row: not in the text, not in the title, not in the
  // accessible name. The tool's own identifier is here and in the body because
  // the visible row gave its name slot to the verb.
  const title = [...paths.map(path => {
    const kind = verbByPath.get(path);
    return kind === undefined || kind === "unknown" ? path : `${kind} ${path}`;
  }), `tool ${item.tool}`].join("\n");
  const label = [verb, shown, diffStatLabel(stat), `tool ${item.tool}`,
    outcome === "ok" ? "completed" : outcome === "running" ? "running" : state,
  ].filter(Boolean).join(", ");
  const { matched, current, className: findClass } = findItemClass(display.keys, find);
  // A settled, successful patch has a body only where the dial asks for one: at
  // the shipped default this row is one line with one glyph, which is what the
  // merge was for. Everything else always has one, because the reason a patch
  // failed, was refused, or is still being asked about is the row's whole point
  // and lives nowhere else.
  const hasBody = outcome !== "ok" || transcriptDensityOpensItems(density);
  const shownOpen = hasBody
    && (open || (find.active && display.keys.some(key => find.currentKey === key)));
  return (
    <div {...findKeysProps(display.keys, find)}
      className={findClass}
      data-transcript-match={matched ? "true" : undefined}
      data-current-match={current ? "true" : undefined}
      aria-current={current ? "true" : undefined}
    >
      <div className="conv-card conv-tool-card">
        <div className="conv-filehead">
          <div className="conv-row conv-diffrow conv-filerow"
            title={title} aria-label={label}>
            <Icon name="git-compare" className="conv-kind" />
            <span className="conv-name">{verb}</span>
            <span className="conv-sum conv-path">{shown}</span>
            <span className={outcome === "ok" ? "conv-state conv-inline"
              : outcome === "running" ? "conv-state conv-inline conv-state-run"
                : "conv-state conv-inline conv-state-fail"}>
              <DiffStatText stat={stat} />
              {outcome === "ok"
                ? <span className="conv-sr">done</span>
                : <>
                    <StateGlyph state={outcome === "running" ? "running" : "failed"} />
                    {state}
                  </>}
            </span>
          </div>
          {hasBody && (
            <button type="button" className="conv-filefold" aria-expanded={shownOpen}
              aria-controls={shownOpen ? bodyId : undefined}
              aria-label={`${shownOpen ? "Hide" : "Show"} the result of `
                + `${verb.toLowerCase()} ${shown}`}
              onClick={toggleOpen}>
              <Chevron open={shownOpen} />
            </button>
          )}
        </div>
        {shownOpen && (
          <div id={bodyId}>
            <div className="conv-tool-id">
              <span className="conv-tool-id-label">Tool</span>
              {item.tool}
            </div>
            <ToolInput input={item.input} summary={item.summary} />
            {done && <pre className="conv-pre" tabIndex={0} role="region"
              aria-label="Tool output">{done.summary}</pre>}
          </div>
        )}
      </div>
    </div>
  );
}, sameWorkRowProps);

/**
 * The reader's `matchMedia`, or nothing where there is no DOM to ask — this
 * module is rendered to static markup in its own tests. A plain read, on the
 * render path: the preference is cheap to ask for and a listener would buy
 * nothing, since a reader who changes it mid-stream is answered by the next
 * frame of a stream that is already committing one.
 */
export function mediaQuery(): ((query: string) => { matches: boolean }) | undefined {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return undefined;
  return query => window.matchMedia(query);
}

/** The clock pacing reads, where there is one: static-markup tests have none to need. */
function paceNow(): number {
  return typeof performance === "undefined" ? 0 : performance.now();
}

/**
 * How much of `text` to paint, whether it is still being painted in, and
 * whether this paint is a paced step (what a Threads pane settles in).
 *
 * The whole string is in state the moment it lands — this paces only what is
 * DRAWN, at the speed paceArrival sets from how the provider has been sending,
 * advanced each animation frame, so display granularity stops being whatever
 * the provider happened to flush.
 *
 * `pacing` false is the pass-through: a pane that is not live, is covered, or
 * whose reader asked for less motion paints `text` by identity, so `Markdown`'s
 * memo keeps protecting every completed message exactly as before. A block is
 * painted whole when it first appears (see mountedPace); one that stops
 * streaming while still behind finishes flowing in (see paceArrival) instead
 * of snapping.
 *
 * A paced step is a pace with a speed: paceArrival and paceFrame keep one,
 * and every way of painting text whole leaves it at zero — mountedPace,
 * settledPace, and so every snap paceArrival makes (a flood, a replacement,
 * the ceiling).
 */
function useRevealedText(text: string, streaming: boolean,
    pacing: boolean): [string, boolean, boolean] {
  const [pace, setPace] = useState(() => mountedPace(text.length, paceNow(), streaming && pacing));
  const [seen, setSeen] = useState({ text, streaming });
  // ADJUSTED DURING RENDER, the same idiom as the pane's session-switch reset:
  // the commit that brings a new tail sets its speed, so the next frame already
  // paints at it. A pane that is not pacing holds the count at full, so a later
  // stream on this same item resumes from the end of what is read, not zero.
  let current = pace;
  if (seen.text !== text || seen.streaming !== streaming) {
    current = pacing && (streaming || pace.shown < pace.length)
      ? paceArrival(pace, text.length, paceNow(), streaming)
      : settledPace(text.length);
    setSeen({ text, streaming });
    setPace(current);
  } else if (!pacing && pace.shown < text.length) {
    current = settledPace(text.length);
    setPace(current);
  }
  const behind = pacing && current.shown < text.length;
  useEffect(() => {
    // No idle rAF: this app spends frames on real activity only. It runs while
    // a tail is behind, because a provider that pauses mid-message commits
    // nothing more — without it the rest of the sentence would sit hidden.
    if (!behind) return;
    const frame = requestAnimationFrame(() => {
      setPace(previous => paceFrame(previous, paceNow()));
    });
    return () => { cancelAnimationFrame(frame); };
  }, [behind, current]);
  const shown = Math.floor(current.shown);
  return [behind && shown < text.length ? text.slice(0, shown) : text, behind,
    pacing && current.speed > 0];
}

/**
 * A block of assistant prose, and the only item in the transcript that holds
 * state of its own. It is split out from `Item` for exactly that reason: `Item`
 * is called as a plain function by this file's own tests, so it has to stay one.
 *
 * Only a tail that arrived in a LIVE pane on screen is paced, and the caret
 * stays until it is painted in. A settled block, a background pane, a covered
 * pane and a reader who asked for less motion all take the identity path
 * through the hook and are drawn precisely as they were before it existed.
 *
 * In a Threads pane each paced step's new words settle in (Markdown's
 * `settleIn`); text painted whole never does. Markdown also places a Threads
 * pane's caret, inline after the last painted word when the last block is
 * prose. Every other pane passes neither and draws the caret below the text.
 */
/**
 * The reply's own selection, as plain text, and where its end is drawn
 * inside `block`; null when nothing in this block is selected. A selection
 * that starts or ends outside the block is not this reply's (the same rule
 * the output preview's capture uses).
 */
function blockSelection(block: HTMLElement): { text: string; top: number; left: number } | null {
  const selection = window.getSelection();
  if (selection === null || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  if (!block.contains(range.startContainer) || !block.contains(range.endContainer)) return null;
  const text = selection.toString();
  if (text.trim() === "") return null;
  const rects = range.getClientRects();
  const last = rects.length > 0 ? rects[rects.length - 1] : range.getBoundingClientRect();
  const frame = block.getBoundingClientRect();
  return { text, top: last.bottom - frame.top + 4, left: Math.max(0, last.right - frame.left - 28) };
}

/**
 * Quote (T3 section 9): a small button by a selection inside one reply. It
 * keeps the selection (pointer down is prevented) and hands its plain text
 * to the pane, which quotes it into the draft.
 */
function useReplyQuote(onQuote: ((text: string) => void) | undefined) {
  const block = useRef<HTMLDivElement>(null);
  const [anchor, setAnchor] = useState<{ text: string; top: number; left: number } | null>(null);
  const look = useCallback((): void => {
    if (onQuote === undefined || block.current === null) return;
    setAnchor(blockSelection(block.current));
  }, [onQuote]);
  useEffect(() => {
    if (anchor === null) return undefined;
    // A selection that collapses or moves out of this reply takes the button.
    const changed = (): void => {
      if (block.current === null || blockSelection(block.current) === null) setAnchor(null);
    };
    document.addEventListener("selectionchange", changed);
    return () => { document.removeEventListener("selectionchange", changed); };
  }, [anchor]);
  const button = anchor === null || onQuote === undefined ? null : (
    <button type="button" className="conv-quote-button" style={{ top: anchor.top, left: anchor.left }}
      title="Quote in the draft" aria-label="Quote the selection in the draft"
      onPointerDown={event => { event.preventDefault(); }}
      onMouseDown={event => { event.preventDefault(); }}
      onClick={() => {
        onQuote(anchor.text);
        window.getSelection()?.removeAllRanges();
        setAnchor(null);
      }}>
      Quote
    </button>
  );
  return { block, look, button, quoting: button !== null };
}

function TextBlock({ item, live, threadLayout, onPreviewAttachment, onQuote }: {
  item: Extract<ConvItem, { kind: "text" }>; live: boolean; threadLayout: boolean;
  onPreviewAttachment?: (attachment: PromptTurnAttachment) => void;
  onQuote?: (text: string) => void;
}): ReactElement {
  // A covered pane paces nothing, and the frame that shows it again snaps: what
  // streamed while nobody could see it is not replayed. The ref is the
  // visibility of the last COMMITTED frame, which is what makes the showing
  // frame, and only that one, the snap.
  const visible = useContext(PaneVisibleContext);
  const coveredAtCommit = useRef(!visible);
  useLayoutEffect(() => { coveredAtCommit.current = !visible; });
  // ponytail: the preference is read per render, not subscribed to. A reader who
  // changes it mid-stream is honoured on the next frame of a stream that is
  // already committing one, and on the next render everywhere else; a listener
  // would buy only the idle case, at the cost of a subscription per text item.
  const [revealed, behind, paced] = useRevealedText(
    item.text,
    item.streaming,
    live && visible && !coveredAtCommit.current && !prefersReducedMotion(mediaQuery()),
  );
  const openFile = useCallback((path: string) => {
    const slash = path.lastIndexOf("/");
    const label = slash >= 0 && slash < path.length - 1 ? path.slice(slash + 1) : path;
    onPreviewAttachment?.({ kind: "file", label, path });
  }, [onPreviewAttachment]);
  const caret = item.streaming || behind;
  const quote = useReplyQuote(onQuote);
  return (
    <div ref={quote.block} className={quote.quoting ? "conv-text is-quoting" : "conv-text"}
      onMouseUp={quote.look} onKeyUp={quote.look}>
      <Markdown text={revealed} settleIn={threadLayout ? paced : undefined}
        caret={threadLayout ? caret : undefined}
        onOpenFile={onPreviewAttachment === undefined ? undefined : openFile} />
      {!threadLayout && caret && <span className="conv-caret" aria-hidden="true">▍</span>}
      {quote.button}
    </div>
  );
}

function Item(props: ConversationItemProps): ReactElement | null {
  const { item, all } = props;
  const { onApprove, onDeny } = all;
  switch (item.kind) {
    case "text":
      return <TextBlock item={item} live={all.live} threadLayout={all.threadLayout === true}
        onPreviewAttachment={all.onPreviewAttachment} onQuote={all.onQuoteReply} />;
    // The user's own words: a contained block, not flat prose, and captioned
    // because on a session that is entirely user turns the shape alone names
    // nobody. Replayed history and this window's live echo both land here.
    case "user":
      return <UserTurn item={item} turnKey={props.findKey}
        point={all.turnRestorePoints?.get(props.findKey)}
        // A restore already running anywhere in this pane disables every
        // control: two rollbacks of one working tree at once is not a state to
        // offer, and the one in flight is the only one that can be described.
        busy={all.restoringTurnKey != null}
        restoring={all.restoringTurnKey === props.findKey}
        onRequestRestore={props.onRequestRestore}
        changes={props.turnChanges}
        threadTurn={props.threadTurn}
        onThreadTurn={props.onThreadTurn}
        onPreviewAttachment={all.onPreviewAttachment} />;
    case "tool":
      return <ToolCard item={item} density={props.density}
        densityEpoch={props.densityEpoch} pathRoot={all.pathRoot}
        diffPaths={props.followingDiffPaths}
        // A Threads pane shows a failure's output unfolded: it stays out of
        // the turn's work line precisely so it can be read.
        openFailed={all.threadLayout === true && item.result?.ok === false
          && !isDeclinedResult(item.result)}
        forceOpen={props.find.active && props.find.currentKey === props.findKey} />;
    case "approval": {
      // A Threads pane docks every request it holds open above its composer
      // (owner, 2026-10-07, "a live turn"); the transcript never draws one
      // there, so the card is in exactly one place.
      if (all.threadLayout === true && all.live && all.activeApprovalIds.has(item.id)) return null;
      // `keyed` is the WHOLE truth about the chord, in one expression: the pane
      // owns it (four visible tiles can each be blocked, and only the focused
      // one answers), AND this is the card that will receive it (a pane can
      // hold two outstanding requests, and the listener takes the last). The
      // chip, the attribute the listener finds the button by, and the announced
      // binding all ride on it, so no card can advertise a key that would land
      // somewhere else. The `&&` short-circuits: an unfocused pane never scans.
      const keyed = all.focused === true
        && all.keyedApprovalId === item.id;
      const answer = all.approvals[approvalVerdictKey(all.sessionKey ?? "", item.id)];
      // A verdict already sent keeps its card: the host drops a request from
      // the owner's list once answered, which must not read as "checking".
      if (all.live && all.checkingApprovalIds.has(item.id)
          && (answer === undefined || answer.error !== undefined) && item.resolved === undefined) {
        return <CheckingApprovalCard item={item} />;
      }
      return item.questions !== undefined
        ? <QuestionCard item={item} onApprove={onApprove} onDeny={onDeny}
            keyed={keyed}
            active={all.live && all.activeApprovalIds.has(item.id)}
            answer={all.approvals[approvalVerdictKey(all.sessionKey ?? "", item.id)]} />
        : <ApprovalCard item={item} onApprove={onApprove} onDeny={onDeny}
            keyed={keyed} engineLabel={all.config.engineLabel}
            threadLayout={all.threadLayout === true}
            active={all.live && all.activeApprovalIds.has(item.id)}
            answer={all.approvals[approvalVerdictKey(all.sessionKey ?? "", item.id)]} />;
    }
    // A produced diff is a row like the tool rows around it, and it has to LOOK
    // like one: the same columns in the same order, ending in the same
    // right-aligned slot. It is now borderless like all of them (see
    // .conv-diffrow above), and it has shed both of its constant words. "Diff"
    // said what the git-compare glyph beside it already said, on a row whose
    // path is the only field that varies; "Open" never changed on any row ever
    // rendered, which is the definition of a word carrying no information — the
    // same reasoning that took "done" off a successful tool row. The trailing
    // chevron went the same way and for the same reason: a right chevron is
    // what a COLLAPSED fold looks like two rows above (.conv-chev's closed
    // state is rotate(-90deg)), so on a row that opens nothing it promised a
    // click that cannot happen. The right-hand slot stays empty.
    //
    // The row is not a control and names nothing to open: the shortened path is
    // its visible text and there is no accessible name to restore the dropped
    // words to. The whole absolute path stays on `title`, and the find index
    // still holds it raw.
    case "diff": {
      const shown = displayPath(item.path, all.pathRoot);
      return (
        <div className="conv-card">
          <div className="conv-row conv-diffrow" title={item.path}>
            <Icon name="git-compare" className="conv-kind" />
            <span className="conv-sum conv-path">{shown}</span>
          </div>
        </div>
      );
    }
    case "notice":
      return (
        <p className={item.tone === "error" ? "conv-notice conv-notice-fail"
          : "conv-notice conv-notice-info"}
          role={item.tone === "error" ? "alert" : "status"}>
          <Icon name={item.tone === "error" ? "error" : "info"}
            className={item.tone === "error" ? "conv-glyph-fail" : "conv-glyph-idle"} />
          <span>{item.text}</span>
          {item.signIn === true && all.onSignIn !== undefined && (
            <button type="button" className="conv-notice-action" onClick={all.onSignIn}>
              Sign in
            </button>
          )}
        </p>
      );
    case "compact":
      return (
        <p className={item.ok === false ? "conv-compact is-failed" : "conv-compact"}
          role="status">
          <span>{compactSeamLabel(item)}</span>
        </p>
      );
    // What another agent answered when this session consulted one. Never flat
    // prose like an assistant turn: the source and the untrusted mark are what
    // stop a reply from a different provider from reading as this agent's own
    // words, which is the relabelling `untrusted` is carried through the
    // reducer to prevent. The objective is on the same line because "which
    // question is this the answer to" is the first thing the row has to say.
    case "peer":
      return (
        <div className="conv-peer">
          <p className="conv-peer-head">
            <Icon name="comment-discussion" className="conv-kind" />
            <span>Consulted {item.sourceEngine}</span>
            <span className="conv-peer-untrusted">untrusted</span>
            {item.objective !== "" && (
              <span className="conv-peer-objective" title={item.objective}>
                · {item.objective}
              </span>
            )}
          </p>
          <Markdown text={item.text} />
        </div>
      );
    // A page the agent showed (T3 section 8), framed and sandboxed by the card.
    case "visual":
      return <VisualReplyCard item={item} sessionKey={all.sessionKey} loader={all.visualFrames} />;
    case "subagent":
      return <SubagentCard item={item} all={props} />;
    default: {
      // Unreachable while every ConvItem member has a case above, and a compile
      // error the moment one does not: only `never` is assignable to `never`.
      const unhandled: never = item;
      return unhandled;
    }
  }
}

export const SubagentGrid = memo(function SubagentGrid({ run, all, find }: {
  run: Extract<ConversationDisplayRow, { kind: "subagent-grid" }>;
  all: ConversationItemContext;
  find: FindRenderContext;
}): ReactElement {
  return (
    <div className="sub-grid" role="list" aria-label="Subagents">
      {run.rows.map(row => (
        <div key={row.key} role="listitem" {...findKeysProps([row.key], find)}
          className="conv-find-item">
          <SubagentTile item={row.item} all={all} />
        </div>
      ))}
    </div>
  );
}, sameSubagentGridProps);

function SubagentTile({ item, all }: {
  item: Extract<ConvItem, { kind: "subagent" }>;
  all: ConversationItemContext;
}): ReactElement {
  const running = item.status === "running";
  const desc = subagentRowDescription(item.subagentType, item.description, all.pathRoot);
  const selected = all.onSelectSubagent !== undefined
    && all.selectedSubagentKey === item.id;
  const spoken = subagentStatusWord(item.status);
  const onSelect = all.onSelectSubagent;
  return (
    <button type="button"
      className={selected ? "sub-tile is-selected" : "sub-tile"}
      data-agent-key={item.id} data-subagent-status={item.status}
      // The square is fixed and its description already flexes to fill whatever
      // the type row and the state line leave; a fourth line would clamp that
      // description mid-line for a field most tiles do not carry. So the model
      // goes where the rest of what the square cannot hold already goes.
      title={[item.subagentType,
        item.description.trim() === "" ? null : item.description,
        item.model === undefined ? null : `model: ${item.model}`]
        .filter(part => part !== null).join(" · ")}
      aria-label={`${item.subagentType} subagent`}
      aria-current={selected ? "true" : undefined}
      disabled={onSelect === undefined}
      onClick={event => { onSelect?.(item.id, event.detail === 0); }}>
      <span className="sub-tile-top">
        <Icon name="robot" className="conv-kind" />
        <span className="sub-tile-type">{item.subagentType}</span>
      </span>
      {desc !== "" && <span className="sub-tile-desc">{desc}</span>}
      <span className={`sub-tile-state${running ? " is-run" : ""}${
        item.status === "failed" ? " is-fail" : ""}`}
        role="status" aria-label={spoken}>
        <StateGlyph state={subagentGlyph(item.status)} />
        {item.status === "done" ? <span className="conv-sr">{spoken}</span>
          : running ? "running…" : item.status}
      </span>
    </button>
  );
}

// A spawned subagent, drawn where its spawn was as ONE row in the same language
// as the tool rows around it: no border, no fill, no second line. Its whole
// transcript stays folded behind the row by default even while running, so
// several parallel agents remain scannable instead of burying the parent's own
// thread.
//
// Two controls, never nested (a button inside a button is invalid HTML and the
// inner one is unreachable): a chevron that unfolds the nested transcript in
// place, drawn only when there IS one, and the row itself, which hands the
// subagent to whatever detail surface the caller owns. With no such surface
// wired the row falls back to the chevron's job, which is what this cell did
// before the surface existed.
function SubagentCard({ item, all: row }: {
  item: Extract<ConvItem, { kind: "subagent" }>;
  all: ConversationItemProps;
}) {
  const { all, find, findKey, density, densityEpoch } = row;
  const [open, toggleOpen] = useDensityFold(density, densityEpoch);
  const panelId = useId();
  const running = item.status === "running";
  const empty = item.children.length === 0;
  // A current result nested under this card must be visible and scrollable even
  // if the user left the subagent folded. Moving away restores its prior fold.
  const forcedOpen = find.active
    && find.currentKey?.startsWith(`${findKey}/`) === true;
  // The detail panel can target a nested spawn while its ancestors are folded.
  // Open only that path so Reveal has a real DOM row to scroll/focus; when the
  // selection moves away, the user's prior fold state returns unchanged.
  const selectedDescendant = all.selectedSubagentKey !== undefined
    && findSubagentItem(item.children, all.selectedSubagentKey) !== undefined;
  // A card with nothing under it yet — the ordinary state between the spawn and
  // the subagent's first event — is not open, whatever the dial seeded into it.
  // Four signals ride on this one flag (the card's outline, the caret, the
  // button's aria-expanded and the panel itself); the panel was always withheld
  // while empty, so at Detailed the other three told the user, and a screen
  // reader, that a disclosure holding nothing was expanded.
  const shownOpen = (open || forcedOpen || selectedDescendant) && !empty;

  // The glyph is one half of the status; this word is the other, because a
  // 12px icon is not an accessible name. "ended" is deliberately the flattest
  // word available: the stream stopped without this subagent reporting back,
  // which is not a success and not a failure — so it takes the neutral dim
  // tier, a stopped glyph rather than a check or an error, and no pulse. The
  // full sentence rides on `title` rather than in the row, because at the real
  // pane width (420px) a 25-character status squeezes the description out of a
  // single row entirely.
  const state = running ? "running…" : item.status;
  const stateTitle = item.status === "ended"
    ? "ended — the stream stopped before this subagent reported a result"
    : undefined;
  const spokenState = item.status === "ended"
    ? "Ended: the stream stopped before this subagent reported a result"
    : subagentStatusWord(item.status);
  const stateClass = running ? "sub-state conv-inline conv-nowrap sub-state-run"
    : item.status === "failed" ? "sub-state conv-inline conv-nowrap sub-state-failed"
    : "sub-state conv-inline conv-nowrap";
  const glyph = subagentGlyph(item.status);

  const tools = item.toolCount === 1 ? "1 tool" : `${item.toolCount} tools`;
  // Delegated work is work. Counted separately from tools because a nested
  // spawn is its own card rather than one of this card's calls (see
  // conversation-reducer.ts), and stated FIRST because an agent that split its task
  // across three others has said more about itself with that than with the one
  // Read it did on the way. Without it an orchestrator's collapsed cell read
  // "starting…" over three finished children.
  const kids = item.subagentCount === 1 ? "1 subagent" : `${item.subagentCount} subagents`;
  // The absolute paths inside a reported summary get the same treatment every
  // tool row's summary already gets. This line was the one place `pathRoot` was
  // threaded in and then ignored, so a finished cell spent its whole slot on a
  // 69-character path whose last two segments were the only new information.
  const last = item.lastActivity === undefined
    ? undefined : shortenSummaryPaths(item.lastActivity, all.pathRoot);
  const activity = [
    item.subagentCount > 0 ? kids : null,
    item.toolCount > 0 ? tools : null,
    // Unlabelled, like the counts either side of it: a model id reads as one
    // and nothing else in this slot could be mistaken for it.
    item.model === undefined ? null : item.model,
    last !== undefined ? `last: ${last}` : null,
  ].filter(part => part !== null).join(" · ");

  // WHAT THE DESCRIPTION SLOT PRINTS, which is no longer what the engine sent.
  // "" means the slot is not drawn at all: Codex derives a spawn's type and its
  // description from one string, so every one of those cells printed its own
  // name twice, the second copy behind a namespace prefix. The raw value stays
  // on the row's title and in the find corpus.
  const desc = subagentRowDescription(
    item.subagentType, item.description, all.pathRoot);
  // A spawn that named a model is running something OTHER than what the parent
  // runs, which is the one thing about it a reader cannot infer from the row —
  // so it joins the counts in the summary slot above rather than taking a slot
  // of its own, and the labelled form rides the title below for the width where
  // that slot clips. Absent means it inherits the parent's model, and the row
  // says nothing at all. The panel prints either case in full.
  // The type LEADS the title. .conv-name's contract (:585) is that a name may
  // ellipsise because the row's title keeps the whole of it reachable, and the
  // type is the one field that tells two spawns of the same task apart — with
  // description alone, two clipped siblings had identical hover text.
  const rowTitle = [
    item.subagentType,
    item.description.trim() === "" ? null : item.description,
    item.model === undefined ? null : `model: ${item.model}`,
  ].filter(part => part !== null).join(" · ");

  // The tree, spoken. The indent says "nested" to someone looking at the pane;
  // to a screen reader four cards at four depths were four identically shaped
  // landmarks, so the parent that spawned this one is named in the card's own
  // accessible name.
  const parent = row.parentSubagent;
  const cardLabel = parent === undefined
    ? `${item.subagentType} subagent`
    : `${item.subagentType} subagent, spawned by ${parent}`;

  // Handing this subagent to the caller's detail surface is what the row does
  // WHEN THERE IS ONE. `event.detail === 0` is a keyboard activation (Enter or
  // Space synthesise a click with no click count); the caller uses it to decide
  // whether to move focus, because a pointer user is still reading the
  // transcript and a keyboard user has just asked to go somewhere.
  const onSelect = all.onSelectSubagent;
  const selected = onSelect !== undefined && all.selectedSubagentKey === item.id;
  const activate = onSelect === undefined
    ? toggleOpen
    : (event: { detail: number }) => onSelect(item.id, event.detail === 0);

  return (
    <section className={`sub sub-${item.status}${shownOpen ? " sub-open" : ""}`
      + (selected ? " is-selected" : "")}
      data-subagent-status={item.status} data-subagent-parent={parent}
      data-agent-key={item.id} aria-label={cardLabel}>
      <div className={`sub-head${empty ? " sub-head-flat" : ""}`}>
        {/* The fold is its own control, and it is absent — not disabled —
            when there is nothing under this agent yet. A disabled chevron was
            still a tab stop and still held its 16px, on every cell between a
            spawn and its first event, which is most cells on screen. */}
        {!empty && (
          <button type="button" className="sub-fold" aria-expanded={shownOpen}
            aria-controls={shownOpen ? panelId : undefined}
            aria-label={`${shownOpen ? "Hide" : "Show"} nested activity of ${item.subagentType}`}
            onClick={toggleOpen}>
            <Chevron open={shownOpen} className="sub-caret" />
          </button>
        )}
        {/* The row opens the detail surface. It is inert only when there is
            neither a surface to open nor anything to unfold — a button that
            cannot do either is worse than no button. */}
        <button type="button" className="sub-row" title={rowTitle}
          aria-current={selected ? "true" : undefined}
          disabled={onSelect === undefined && empty} onClick={activate}>
          <Icon name="robot" className="conv-kind" />
          <span className="sub-type">{item.subagentType}</span>
          {desc !== "" && <span className="sub-desc">{desc}</span>}
          {/* Counts and the newest thing this agent said, in the slot a tool
              row gives its summary. Empty means no slot: "starting…" beside a
              "running…" that already said it was one word occupying a whole
              second line. */}
          {activity !== "" && <span className="conv-sum">{activity}</span>}
          <span className={stateClass} title={stateTitle} role="status"
            aria-live="polite" aria-atomic="true" aria-label={spokenState}>
            <StateGlyph state={glyph} />
            {/* SUCCESS IS QUIET, the rule every tool row already follows: the
                check glyph is the whole message, and the word survives for
                assistive tech. running / failed / ended keep theirs, because
                those do inform. */}
            {item.status === "done" ? <span className="conv-sr">{state}</span> : state}
          </span>
        </button>
      </div>

      {/* The same renderers the main thread uses — a nested tool card, diff or
          approval behaves exactly as it does at the top level, including the
          Allow/Deny wiring, because `all` is this pane's own prop bag. */}
      {shownOpen && (
        <div id={panelId} className="sub-kids">
          {item.children.map((child, index) => (
            // parentSubagent is set AFTER the spread on purpose: `all` already
            // carries this card's own parent, and a grandchild must name the
            // agent that spawned it rather than the one two levels up.
            // onRequestRestore is dropped for the same class of reason — a user
            // turn in here is the parent agent's prompt to this subagent, not
            // something the human typed, so there is no checkpoint before it.
            // turnChanges goes with it: whatever this agent wrote is already
            // counted on the human turn that spawned it.
            <SearchableItem key={child.id} {...row} item={child}
              findKey={transcriptChildKey(findKey, index)}
              parentSubagent={item.subagentType}
              turnChanges={undefined}
              onRequestRestore={undefined}
              threadTurn={undefined}
              onThreadTurn={undefined} />
          ))}
        </div>
      )}
    </section>
  );
}

/**
 * The user's own words, and — where the caller says a checkpoint stands behind
 * them — the one click that undoes what the agent did next.
 *
 * Rollback used to live only in the Review Center: open a panel, find the
 * checkpoint by its timestamp, decide whether that is the right one. The
 * mistake is visible HERE, at the message that caused it, so the control is
 * here too. It is drawn only for a turn the caller supplied a restore point
 * for, so a transcript nobody can roll back is byte-for-byte what it was.
 *
 * Beside it, and on the same reasoning, the turn's own diffstat: `+62 −11 ·
 * 4 files` for what this prompt changed on disk, derived here rather than
 * supplied, because the pane already holds every diff it is counting. It is
 * ORIENTATION and nothing else: it opens nothing and never did anything but
 * state the delta, so it is a line of text beside a control rather than a
 * second control.
 */
function UserTurn({ item, turnKey, point, busy, restoring, onRequestRestore,
  changes, threadTurn, onThreadTurn, onPreviewAttachment }: {
  item: Extract<ConvItem, { kind: "user" }>;
  turnKey: string;
  point?: TurnRestorePoint;
  busy: boolean;
  restoring: boolean;
  onRequestRestore?: (turnKey: string) => void;
  changes?: TurnChangeSummary;
  threadTurn?: ThreadTurnControl;
  onThreadTurn?: (action: "diff" | "edit", turnKey: string) => void;
  onPreviewAttachment?: (attachment: PromptTurnAttachment) => void;
}) {
  const offered = point !== undefined && onRequestRestore !== undefined;
  // Re-read rather than re-implemented, so an unknown or net-zero stat keeps
  // exactly the silence DiffStatText keeps and the separator cannot survive
  // the counts it separates.
  const delta = changes === undefined ? "" : formatDiffStat(changes.stat);
  // A thread turn's change summary is Rust's measure of it and OPENS that
  // turn's diff, so it is a control rather than the static chip below.
  const threadDiff = threadTurn !== undefined && threadTurn.diff && onThreadTurn !== undefined;
  const threadEdit = threadTurn !== undefined && threadTurn.edit && onThreadTurn !== undefined;
  const threadChange = threadTurn?.change || delta;
  const resolvedAttachments = onPreviewAttachment === undefined
    ? [] : extractPromptTurnAttachments(item.text, item.attachments);
  const shownImagePaths = new Set<string>();
  const imageTargets = (item.images ?? []).map((image, index) => {
    const path = resolveTurnImageAttachmentPath(image, index, resolvedAttachments);
    if (path !== undefined) shownImagePaths.add(path);
    return { image, path };
  });
  const fileAttachments = resolvedAttachments.filter(attachment =>
    !shownImagePaths.has(attachment.path));
  return (
    <div className="conv-turn-user">
      <div className="conv-turn-head">
        <span className="conv-turn-who">You</span>
        {threadDiff && (
          <button type="button" className="conv-turn-changes is-action"
            aria-label={threadChange === ""
              ? "Show the changes from this message"
              : `Show the changes from this message: ${threadChange}`}
            title="Show this turn’s changes"
            onClick={() => { onThreadTurn("diff", turnKey); }}>
            <Icon name="git-compare" sm />
            {threadChange === "" ? "Changes" : threadChange}
          </button>
        )}
        {/* THE LINE DELTA, AND NOTHING ELSE. The file count this chip used to
            print came from the diff paths this window saw; the Undo control on
            the same row prints one counted from the files its checkpoint holds.
            Two numbers, two sources, one row — so the row could say "2 files"
            beside "Restores 3 files" and be right twice, which reads as a bug
            in whichever one you trust less. The count stays with the control
            that can act on it, and the chip keeps the delta nothing else says.
            With no delta to state the chip has nothing left to say, so it is
            not drawn at all rather than drawn empty. */}
        {!threadDiff && changes !== undefined && delta !== "" && (
          // There is no lane left to open, and the fact the chip states is true
          // either way — so it stays, as the statement it always was.
          <span className="conv-turn-changes"
            aria-label={turnChangesLabel(changes)}
            title={turnChangesLabel(changes)}>
            <DiffStatText stat={changes.stat} />
          </span>
        )}
        {offered && (
          <button type="button" className="conv-turn-restore"
            // Disabled by ANY restore in flight, not just this one — see the
            // call site. `restoring` is the one that can say what it is doing.
            disabled={busy}
            aria-label="Restore files to before this message"
            title="Restore files to before this message"
            onClick={() => { onRequestRestore(turnKey); }}>
            <Icon name="refresh" sm />
            {restoring ? "Restoring…" : point.label ?? "Restore"}
          </button>
        )}
        {threadEdit && (
          <button type="button" className="conv-turn-restore conv-turn-edit"
            aria-label="Edit from this message: start a new thread from here"
            title="Edit from here"
            disabled={threadTurn.busy}
            onClick={() => { onThreadTurn("edit", turnKey); }}>
            <Icon name="edit" sm />
            Edit from here
          </button>
        )}
      </div>
      {/* Above the words, the way every chat surface puts the picture above the
          question about it. Drawn for a turn this window just sent AND for a
          replayed one whose engine store kept the bytes — the same markup for
          both, which is what makes a screenshot survive a reload unchanged.
          Every `src` was validated at the mapper or the composer boundary
          (isPromptImagePreviewUrl), so nothing here re-checks it. */}
      {item.images !== undefined && item.images.length > 0 && (
        <div className="conv-turn-images">
          {imageTargets.map(({ image, path }, index) => onPreviewAttachment === undefined ? (
            <img key={`${index}:${image.label}`} className="conv-turn-image"
              src={image.src} alt={`Attached image ${image.label}`} />
          ) : (
            <img key={`${index}:${image.label}`} className="conv-turn-image is-clickable"
              src={image.src} alt={`Attached image ${image.label}`}
              role="button" tabIndex={0} title={`Preview ${image.label}`}
              onClick={() => {
                onPreviewAttachment({
                  kind: "file", label: image.label,
                  path: path ?? image.label, previewUrl: image.src,
                });
              }}
              onKeyDown={event => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
                onPreviewAttachment({
                  kind: "file", label: image.label,
                  path: path ?? image.label, previewUrl: image.src,
                });
              }} />
          ))}
        </div>
      )}
      {onPreviewAttachment !== undefined && fileAttachments.length > 0 && (
        <div className="conv-turn-attachments" role="group" aria-label="Attached files">
          {fileAttachments.map(attachment => (
            <span key={`${attachment.kind}:${attachment.path}`}
              role="button" tabIndex={0}
              className="conv-turn-attachment"
              title={attachment.path}
              aria-label={`Preview ${attachment.kind} ${attachment.label}`}
              onClick={() => { onPreviewAttachment(attachment); }}
              onKeyDown={event => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
                onPreviewAttachment(attachment);
              }}>
              <Icon name={attachment.kind === "folder" ? "folder" : "file"} sm />
              <span className="conv-turn-attachment-label">{attachment.label}</span>
            </span>
          ))}
        </div>
      )}
      {/* The words carry a class of their own so a chrome can mark them —
          CLI hangs Claude Code's `›` here, because the turn's own first child
          is the head row above. An inline span inherits every type and colour
          rule the turn already set, so the shipped appearance is unchanged. */}
      <span className="conv-turn-text">{item.text}</span>
    </div>
  );
}

function ToolCard({ item, forceOpen, density, densityEpoch, pathRoot, diffPaths, openFailed }: {
  item: Extract<ConvItem, { kind: "tool" }>;
  forceOpen: boolean;
  density: TranscriptDensity;
  densityEpoch: number;
  pathRoot?: string;
  diffPaths?: readonly string[];
  /** Start unfolded because the call failed (a Threads pane). */
  openFailed?: boolean;
}) {
  const [open, toggleOpen] = useDensityFold(density, densityEpoch, openFailed === true);
  // Tool payload/result text participates in find even while the card is folded.
  // Reveal it for the current result; moving away restores the user's own fold.
  const shownOpen = open || forceOpen;
  const done = item.result;
  const state = !done ? "running…" : done.ok ? "done" : "failed";
  const stateClass = !done ? "conv-state conv-inline conv-state-run"
    : done.ok ? "conv-state conv-inline" : "conv-state conv-inline conv-state-fail";
  const glyph: RunState = !done ? "running" : done.ok ? "ok" : "failed";
  const kind = toolGlyph(item.tool);
  const visibleName = toolActivityName(item.tool);
  // What the middle column PRINTS, which is no longer what the engine sent.
  // toolRowSummaryText drops a summary that is the tool's own name again,
  // collapses a re-listing of the files the diff rows below already name, and
  // shortens absolute paths against the session root. "" means the slot is not
  // drawn at all rather than drawn empty. The RAW string stays on `title` below
  // and in the find corpus, so nothing here is unreachable.
  const summary = toolRowSummaryText(item, { root: pathRoot, diffPaths });
  const accessibleState = !done ? "running" : done.ok ? "completed" : "failed";
  const accessibleLabel = [visibleName, `tool ${item.tool}`, summary, accessibleState]
    .filter(Boolean).join(", ");
  return (
    <div className="conv-card conv-tool-card">
      {/* The calm method label is visible; the provider's exact identifier is
          retained in the accessible name, title, search index and open body. */}
      <button type="button" className="conv-row conv-tool-row" aria-expanded={shownOpen}
        aria-label={accessibleLabel}
        title={item.summary.trim() !== "" ? item.summary : item.tool}
        onClick={toggleOpen}>
        {kind !== null && <Icon name={kind} className="conv-kind" />}
        <span className="conv-name">{visibleName}</span>
        {summary !== "" && <span className="conv-sum">{summary}</span>}
        {/* SUCCESS IS QUIET. A finished-and-fine call printed a check glyph AND
            the word "done" on every row — two elements saying the one thing
            that carries no information, on the majority of rows in a busy
            transcript. Running and failed keep both glyph and word because
            those DO inform; success survives only in the accessible name and
            the screen-reader fallback below. */}
        {done?.ok === true
          ? <span className="conv-sr">{state}</span>
          : <span className={stateClass}>
              <StateGlyph state={glyph} />
              {state}
            </span>}
        <Chevron open={shownOpen} />
      </button>
      {shownOpen && (
        <>
          {visibleName !== item.tool && (
            <div className="conv-tool-id">
              <span className="conv-tool-id-label">Tool</span>
              {item.tool}
            </div>
          )}
          <ToolInput input={item.input} summary={item.summary} />
          {done && <pre className="conv-pre" tabIndex={0} role="region"
            aria-label="Tool output">{done.summary}</pre>}
        </>
      )}
    </div>
  );
}

/**
 * The arguments of a call, unfolded. A shell command is the thing the user came
 * here to read, and JSON.stringify hands it over as one line with every newline
 * spelled \n — so the body of a call is printed as the text it is. Leftover
 * keys stay off the card once a named body was extracted. toolInputBlocks
 * decides which is which; this only draws the answer.
 */
function ToolInput({ input, summary }: { input: unknown; summary?: string }) {
  const blocks = toolInputBlocks(input, summary ?? "");
  if (blocks.length === 0) return null;
  return (
    <>
      {blocks.map((block, index) => (
        <div key={block.label ?? `rest-${index}`} className="conv-input">
          {block.label !== undefined && (
            <div className="conv-input-label">{block.label}</div>
          )}
          <pre className="conv-pre" tabIndex={0} role="region"
            aria-label={block.label ?? "Tool input"}>{block.text}</pre>
        </div>
      ))}
    </>
  );
}

/**
 * THE VERDICT IS THE CALLER'S STATE, NOT THIS CARD'S. `answer` comes from the
 * one map above both surfaces that can answer an approval (see
 * ConversationProps.approvals), which is what makes answering on another surface
 * settle the request here too. This card held a private `useState` instead, and
 * a private latch in each surface is exactly how a user allowed in one place,
 * came here, was offered Allow/Deny again, clicked Deny, and read "Denied" for
 * a command the engine had already run.
 *
 * And it says SENT, not settled. What this
 * window knows is that it called `respond`; the engine dropping the request is
 * what settles it. A send that failed puts the buttons back and prints why,
 * because a card reading "Allowed" over an agent that is still blocked is the
 * failure this whole channel exists to prevent.
 */
export function toggleQuestionOption(
  selected: readonly string[],
  label: string,
  multi: boolean,
): { selected: string[]; clearOther: boolean } {
  if (!multi) {
    if (selected[0] === label) return { selected: [...selected], clearOther: false };
    return { selected: [label], clearOther: label !== "Other" };
  }
  const wasSelected = selected.includes(label);
  return {
    selected: wasSelected
      ? selected.filter(item => item !== label) : [...selected, label],
    clearOther: label === "Other" && wasSelected,
  };
}

export function questionAnswersReady(
  questions: readonly AgentQuestion[],
  picks: Readonly<Record<string, readonly string[]>>,
  other: Readonly<Record<string, string>>,
): boolean {
  if (questions.length === 0) return false;
  return questions.every(question => {
    const selected = picks[question.question] ?? [];
    if (selected.length === 0) return false;
    return !selected.includes("Other") || (other[question.question]?.trim() ?? "").length > 0;
  });
}

/**
 * The verdict state both blocking cards keep. A failed send is not a verdict:
 * the request is demonstrably still outstanding, so the controls come back.
 * `item.resolved` is still honoured for a future producer that fills it in; the
 * reducer never does. Call `settle()` before dispatching a verdict — the card
 * then takes focus back when ITS OWN send lands, and a verdict arriving from
 * anywhere else leaves the caret where the reader put it.
 *
 * A DOCKED card (a Threads pane, owner 2026-10-07) can be answered from the
 * composer under it — an empty field's ⌘↵ / ⌘⌫ click its buttons — and the
 * caret then stays in the composer: the card takes focus back only when focus
 * was inside it at the click, which is the case the settle exists for (the
 * clicked button disappears).
 */
function useApprovalVerdict(
  item: Extract<ConvItem, { kind: "approval" }>,
  answer?: { verdict: "allow" | "deny"; error?: string },
  docked = false,
): {
  cardRef: RefObject<HTMLDivElement | null>;
  failed: string | undefined;
  sent: "allow" | "deny" | undefined;
  settle: () => void;
} {
  const cardRef = useRef<HTMLDivElement>(null);
  const settleFocus = useRef(false);
  const failed = answer?.error;
  const sent = failed === undefined ? answer?.verdict ?? item.resolved : undefined;
  useEffect(() => {
    if (sent === undefined || !settleFocus.current) return;
    settleFocus.current = false;
    cardRef.current?.focus();
  }, [sent]);
  return { cardRef, failed, sent, settle: () => {
    settleFocus.current = !docked
      || cardRef.current?.contains(document.activeElement) === true;
  } };
}

/**
 * Where a docked card is drawn and what its footnote says. Present only for
 * the Threads pane's dock; every other card is the transcript's.
 */
export interface ApprovalDock {
  /** Paths in the card are shown relative to this, as the transcript shows them. */
  pathRoot?: string;
  /** The thread's project, for "in website · waiting 0:42"; absent says the clock alone. */
  projectLabel?: string;
  /** When the dock first drew this request (epoch ms), for its waiting clock. */
  since: number;
}

/** The wrapper both blocking cards render: one focus target, one alert role. */
export function ApprovalShell({ cardRef, active, sent, label, docked, children }: {
  cardRef: RefObject<HTMLDivElement | null>;
  active: boolean;
  sent: "allow" | "deny" | undefined;
  label: string;
  /** Docked above a Threads composer: the dock's card, not the transcript's. */
  docked?: boolean;
  children: ReactNode;
}) {
  if (docked === true) {
    // The same focus target, alert and voice guard, in the dock's card.
    const pending = active && !sent;
    return (
      <div ref={cardRef} tabIndex={-1}
        className={pending ? "thread-dock-card is-pending" : "thread-dock-card"}
        role={pending ? "alert" : undefined}
        aria-label={pending ? label : undefined}
        data-voice-guard="approval">
        {children}
      </div>
    );
  }
  return (
    <div ref={cardRef} tabIndex={-1}
      className={sent || !active ? "conv-approval" : "conv-approval conv-approval-pending"}
      role={active && !sent ? "alert" : undefined}
      aria-label={active && !sent ? label : undefined}
      // The voice assistant's ui_act refuses everything inside this card. It
      // may read the request; the verdict is the user's (see voice-ui.ts).
      data-voice-guard="approval">
      {children}
    </div>
  );
}

/**
 * The docked card's tile (owner, 2026-10-23, the Threads look): what the
 * request is about, in the Threads icons. Amber while it blocks the turn; a
 * sent or checking card's tile drops to the dim tier.
 */
const DOCK_GLYPH = {
  command: "run", edit: "edit", write: "write", other: "access",
} as const satisfies Record<ThreadDockKind, ThreadGlyphName>;

function DockTile({ glyph, live }: { glyph: ThreadGlyphName; live: boolean }): ReactElement {
  return (
    <span className={live ? "thread-dock-tile thread-dock-glyph" : "thread-dock-tile conv-glyph-idle"}
      aria-hidden="true">
      <ThreadGlyph name={glyph} />
    </span>
  );
}

/**
 * The docked card's "in website · waiting 0:42", ticking once a second in this
 * leaf alone. Hidden from the accessibility tree: the card is an alert, and a
 * live region re-announces every tick of a clock inside it.
 */
function DockWaiting({ since, project }: { since: number; project?: string }): ReactElement {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => { setNow(Date.now()); }, 1_000);
    return () => window.clearInterval(id);
  }, []);
  return (
    <span className="thread-dock-meta" aria-hidden="true">
      {threadDockMeta(project, now - since)}
    </span>
  );
}

/**
 * What a docked request asks, drawn for the decision: a command as the shell
 * line it is, an edit as its file and a small diff, a Write as its file and
 * the start of what it writes. The file's path always shows and the raw
 * "content" band never does; anything else is the request's own input.
 */
function DockedRequestBody({ item, pathRoot }: { item: ThreadApproval; pathRoot?: string }) {
  const kind = threadDockKind(item.tool);
  if (kind === "command") {
    const command = threadDockCommand(item.input);
    if (command !== null) {
      return (
        <pre className="thread-dock-command" tabIndex={0} role="region" aria-label="Command">
          <span className="thread-dock-prompt" aria-hidden="true">$ </span>{command}
        </pre>
      );
    }
  }
  if (kind === "edit" || kind === "write") {
    const files = threadDockFiles(item, pathRoot);
    if (files.length > 0) {
      const diff = threadDockDiff(item.input, kind);
      return (
        <>
          <ul className="thread-dock-files" aria-label={files.length === 1 ? "File" : "Files"}>
            {files.map(file => (
              <li key={file.path} className="thread-dock-file" title={file.path}>
                <FileBadge path={file.path} />
                <span className="thread-dock-path">{file.shown}</span>
                {file.stat !== undefined && (
                  <span className="thread-dock-stat">
                    {file.stat.added !== undefined && (
                      <span className="thread-dock-added">+{file.stat.added}</span>
                    )}
                    {file.stat.removed !== undefined && (
                      <span className="thread-dock-removed">−{file.stat.removed}</span>
                    )}
                  </span>
                )}
              </li>
            ))}
          </ul>
          {diff !== null && (
            <div className="thread-dock-diff" tabIndex={0} role="region"
              aria-label={kind === "write" ? "Start of the file" : "Proposed change"}>
              {diff.lines.map((line, index) => (
                <div key={index}
                  className={line.sign === "+" ? "thread-dock-line is-added" : "thread-dock-line is-removed"}>
                  <span className="thread-dock-sign">{line.sign}</span>
                  <span className="thread-dock-code">{line.text === "" ? " " : line.text}</span>
                </div>
              ))}
              {diff.more > 0 && (
                <div className="thread-dock-more">+{diff.more} more {diff.more === 1 ? "line" : "lines"}</div>
              )}
            </div>
          )}
        </>
      );
    }
  }
  return <ToolInput input={item.input} summary={item.summary} />;
}

export function QuestionCard({ item, onApprove, onDeny, answer, active, keyed, dock }: {
  item: Extract<ConvItem, { kind: "approval" }>;
  onApprove: ConversationProps["onApprove"];
  onDeny: ConversationProps["onDeny"];
  answer?: { verdict: "allow" | "deny"; error?: string };
  active: boolean;
  /** This pane owns the approval chord, so its buttons may print it. */
  keyed: boolean;
  /** Docked above a Threads composer: the same questions, in the dock's card. */
  dock?: ApprovalDock;
}) {
  const uid = useId();
  const { cardRef, failed, sent, settle } = useApprovalVerdict(item, answer, dock !== undefined);
  const questions = item.questions ?? [];
  const [picks, setPicks] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});

  function toggle(question: string, label: string, multi: boolean): void {
    const result = toggleQuestionOption(picks[question] ?? [], label, multi);
    setPicks(current => ({ ...current, [question]: result.selected }));
    if (result.clearOther) setOther(current => ({ ...current, [question]: "" }));
  }

  function submit(): void {
    if (!questionAnswersReady(questions, picks, other)) return;
    const answers: Record<string, string> = {};
    for (const question of questions) {
      const selected = picks[question.question] ?? [];
      const otherText = other[question.question]?.trim() ?? "";
      const labels = selected.map(label =>
        label === "Other" && otherText.length > 0 ? otherText : label);
      if (labels.length > 0) answers[question.question] = labels.join(", ");
    }
    if (Object.keys(answers).length === 0) return;
    settle();
    onApprove(item.id, answers);
  }

  const ready = questionAnswersReady(questions, picks, other);

  const body = (
      <div className="conv-q">
        {questions.length === 0 && (
          <div className="conv-verdict-err" role="alert">
            <Icon name="warning" sm className="conv-glyph-wait" />
            <span>Question details are unavailable. Skip this request and ask the agent to try again.</span>
          </div>
        )}
        {questions.map((question, questionIndex) => {
          const selected = picks[question.question] ?? [];
          return (
            <div key={question.question} className="conv-q-block">
              <div className="conv-q-head">{question.header}</div>
              <p className="conv-q-text">{question.question}</p>
              <div className="conv-q-opts" role={question.multiSelect ? "group" : "radiogroup"}
                aria-label={question.question}>
                {[...question.options, { label: "Other", description: "Write your own answer" }]
                  .map((option, optionIndex) => {
                    const on = selected.includes(option.label);
                    const inputId = `${uid}-q-${questionIndex}-option-${optionIndex}`;
                    return (
                      <div key={option.label} className="conv-q-choice">
                        <label htmlFor={inputId}
                          className={on ? "conv-q-opt is-on" : "conv-q-opt"}>
                          <input id={inputId} className="conv-q-native"
                            type={question.multiSelect ? "checkbox" : "radio"}
                            name={question.multiSelect ? undefined : `${uid}-q-${questionIndex}`}
                            checked={on}
                            disabled={!active || sent !== undefined}
                            onChange={() => { toggle(question.question, option.label,
                              question.multiSelect); }} />
                          <span className="conv-q-opt-copy">
                          <span className="conv-q-opt-label">{option.label}</span>
                          {option.description !== "" && (
                            <span className="conv-q-opt-desc">{option.description}</span>
                          )}
                          {option.preview !== undefined && option.preview !== "" && (
                            <pre className="conv-q-preview">{option.preview}</pre>
                          )}
                          </span>
                        </label>
                        {option.label === "Other" && on && (
                          <input className="conv-q-other" value={other[question.question] ?? ""}
                            aria-label={`Other answer for ${question.header}`}
                            placeholder="Your answer"
                            disabled={!active || sent !== undefined}
                            onChange={event => {
                              setOther(current => ({
                                ...current, [question.question]: event.target.value,
                              }));
                            }} />
                        )}
                      </div>
                    );
                  })}
              </div>
            </div>
          );
        })}
      </div>
  );
  const failedLine = failed !== undefined && (
    <div className="conv-verdict-err" role="alert">
      <Icon name="error" sm className="conv-glyph-fail" />
      <span>{failed}</span>
    </div>
  );
  const verdict = sent ? (
    <div className="conv-verdict" role="status">
      {sent === "allow"
        ? <><Icon name="check" sm className="conv-glyph-ok" />Answers sent</>
        : <><Icon name="close" sm className="conv-glyph-fail" />Skipped</>}
    </div>
  ) : !active ? (
    <div className="conv-verdict">Request closed</div>
  ) : null;
  const skip = (className: string): ReactElement => (
    <button type="button" className={className}
      {...approvalKeyProps(keyed, "deny")}
      onClick={() => {
        settle();
        onDeny(item.id, "Skipped the questions");
      }}>
      <Icon name="close" />
      Skip
      <KeyHint keys={APPROVAL_DENY_KEYS} on={keyed} />
    </button>
  );
  const sendAnswers = (
    <button type="button" className="btn btn-primary" disabled={!ready}
      {...approvalKeyProps(keyed, "allow")}
      onClick={submit}>
      <Icon name="check" />
      Send answers
      <KeyHint keys={APPROVAL_ALLOW_KEYS} on={keyed} />
    </button>
  );

  if (dock !== undefined) {
    // The dock's order: Skip first and Send answers last, the primary at the
    // reading end of the row, as the docked approval puts Allow once.
    return (
      <ApprovalShell cardRef={cardRef} active={active} sent={sent} docked
        label="The agent is asking a question">
        <div className="thread-dock-head">
          <DockTile glyph="comment" live={!sent} />
          <span className="thread-dock-title">{threadDockQuestionTitle(questions.length)}</span>
        </div>
        <div className="thread-dock-body">{body}</div>
        {failedLine}
        <div className="thread-dock-foot">
          {verdict ?? (
            <>
              <DockWaiting since={dock.since} project={dock.projectLabel} />
              <div className="thread-dock-acts">
                {skip("btn thread-dock-ghost")}
                {sendAnswers}
              </div>
            </>
          )}
        </div>
      </ApprovalShell>
    );
  }

  return (
    <ApprovalShell cardRef={cardRef} active={active} sent={sent}
      label="The agent is asking a question">
      <div className="conv-head">
        <Icon name="comment"
          className={sent ? "conv-glyph-idle" : "conv-glyph-wait"} />
        <span className="conv-name">Question</span>
        <span className="conv-sum">{item.tool}</span>
      </div>
      {body}
      {failedLine}
      {verdict ?? (
        <div className="conv-acts">
          {sendAnswers}
          {skip("btn btn-danger")}
        </div>
      )}
    </ApprovalShell>
  );
}

/** An agent's request while Interface's approval policy decides: what it asks, no verdict. */
export function CheckingApprovalCard({ item, dock }: {
  item: Extract<ConvItem, { kind: "approval" }>;
  /** Docked above a Threads composer. Not amber: nothing waits on the owner yet. */
  dock?: ApprovalDock;
}) {
  if (dock !== undefined) {
    return (
      <div tabIndex={-1} className="thread-dock-card is-checking" data-voice-guard="approval">
        <div className="thread-dock-head">
          <DockTile glyph="access" live={false} />
          <span className="thread-dock-title">Checking with Interface&apos;s approval policy…</span>
          {item.questions === undefined && (
            <span className="thread-dock-sum" title={item.summary}>
              {threadStepLine(threadStepWords(item, "waiting", dock.pathRoot))}
            </span>
          )}
        </div>
        {item.questions === undefined && (
          <div className="thread-dock-body is-last">
            <DockedRequestBody item={item} pathRoot={dock.pathRoot} />
          </div>
        )}
      </div>
    );
  }
  return (
    <div tabIndex={-1} className="conv-approval" data-voice-guard="approval">
      <div className="conv-head">
        <Icon name="shield" className="conv-glyph-idle" />
        <span className="conv-name">Checking with Interface&apos;s approval policy…</span>
        <span className="conv-sum" title={item.summary}>{item.tool} — {item.summary}</span>
      </div>
      {item.questions === undefined && <ToolInput input={item.input} summary={item.summary} />}
    </div>
  );
}

export function ApprovalCard({ item, onApprove, onDeny, answer, active, keyed,
    engineLabel, threadLayout, dock }: {
  item: Extract<ConvItem, { kind: "approval" }>;
  onApprove: ConversationProps["onApprove"];
  onDeny: ConversationProps["onDeny"];
  answer?: { verdict: "allow" | "deny"; error?: string };
  active: boolean;
  /** This pane owns the approval chord, so its buttons may print it. */
  keyed: boolean;
  /** The pane's provider, for CLI's "Claude Code wants to run" head. */
  engineLabel?: string;
  /**
   * A Threads pane (owner, 2026-10-06): the card asks its question ("Run this
   * command?" for a shell call), and Allow once is the last, primary button.
   * The same two verdicts, the same handlers and the same keys.
   */
  threadLayout?: boolean;
  /**
   * Docked above a Threads composer (owner, 2026-10-07, "a live turn"): the
   * question by kind, the request drawn for the decision, a waiting clock,
   * then Deny and Allow once with their keys. Same verdicts, same handlers.
   */
  dock?: ApprovalDock;
}) {
  const { cardRef, failed, sent, settle } = useApprovalVerdict(item, answer, dock !== undefined);
  // Claude Code names the ENGINE that is asking, not the tool it wants; refined
  // chrome keeps the card's own heading and puts the tool in the summary. Two
  // sentences for one state, so the wording is gated where the look is.
  const cliChrome = useDocumentChrome() === "cli";
  const asking = (engineLabel ?? "").trim() || item.tool;
  const denyButtonRef = useRef<HTMLButtonElement>(null);
  const [denying, setDenying] = useState(false);
  const [note, setNote] = useState("");

  function confirmDeny() {
    settle();
    onDeny(item.id, note.trim() || undefined);
  }

  function cancelDeny() {
    setDenying(false);
    requestAnimationFrame(() => { denyButtonRef.current?.focus(); });
  }

  const allowButton = (label: string): ReactElement => (
    <button type="button" className={dock === undefined ? "btn btn-primary" : "btn btn-primary thread-dock-allow"}
      {...approvalKeyProps(keyed, "allow")}
      onClick={() => {
        settle();
        onApprove(item.id);
      }}>
      <Icon name="check" />
      {label}
      <KeyHint keys={APPROVAL_ALLOW_KEYS} on={keyed} />
    </button>
  );
  const denyButton = (
    <button ref={denyButtonRef} type="button"
      className={dock === undefined ? "btn btn-danger" : "btn thread-dock-ghost"}
      {...approvalKeyProps(keyed, "deny")}
      onClick={() => setDenying(true)}>
      <Icon name="close" />
      Deny
      <KeyHint keys={APPROVAL_DENY_KEYS} on={keyed} />
    </button>
  );
  const failedLine = failed !== undefined && (
    <div className="conv-verdict-err" role="alert">
      <Icon name="error" sm className="conv-glyph-fail" />
      <span>{failed}</span>
    </div>
  );
  // What replaces the two verdict buttons: the verdict sent, a closed
  // request, or Deny's reason field.
  const settled = sent ? (
    <div className="conv-verdict" role="status">
      {sent === "allow"
        ? <><Icon name="check" sm className="conv-glyph-ok" />Allow sent</>
        : <><Icon name="close" sm className="conv-glyph-fail" />Deny sent</>}
    </div>
  ) : !active ? (
    <div className="conv-verdict">Request closed</div>
  ) : denying ? (
    <div className={dock === undefined ? "conv-acts" : "thread-dock-acts is-denying"}>
      <input
        className="conv-note"
        value={note}
        autoFocus
        aria-label="Reason for denying"
        placeholder="Reason (optional)"
        onChange={e => setNote(e.target.value)}
        onKeyDown={e => {
          if (e.key === "Enter") confirmDeny();
          else if (e.key === "Escape") {
            e.preventDefault();
            cancelDeny();
          }
        }}
      />
      <button type="button" className="btn btn-danger" onClick={confirmDeny}>
        <Icon name="close" />
        Confirm deny
      </button>
      <button type="button" className="btn" onClick={cancelDeny}>
        Cancel
      </button>
    </div>
  ) : null;

  if (dock !== undefined) {
    return (
      <ApprovalShell cardRef={cardRef} active={active} sent={sent} docked label="Approval required">
        <div className="thread-dock-head">
          {/* Amber while it blocks the turn; the settled card drops to the dim tier. */}
          <DockTile glyph={DOCK_GLYPH[threadDockKind(item.tool)]} live={!sent} />
          <span className="thread-dock-title">{threadDockTitle(item)}</span>
        </div>
        <div className="thread-dock-body">
          <DockedRequestBody item={item} pathRoot={dock.pathRoot} />
        </div>
        {failedLine}
        <div className="thread-dock-foot">
          {!sent && active && !denying && (
            <DockWaiting since={dock.since} project={dock.projectLabel} />
          )}
          {settled ?? (
            // Deny first and Allow once last, the primary at the reading end
            // of the row; the DOM order is the order on screen.
            <div className="thread-dock-acts">
              {denyButton}
              {allowButton("Allow once")}
            </div>
          )}
        </div>
      </ApprovalShell>
    );
  }

  return (
    <ApprovalShell cardRef={cardRef} active={active} sent={sent} label="Approval required">
      <div className="conv-head">
        {/* Amber while it is blocking; the settled card drops to the dim tier. */}
        <Icon name="warning"
          className={sent ? "conv-glyph-idle" : "conv-glyph-wait"} />
        {cliChrome ? (
          <>
            <span className="conv-name">{asking} wants to run</span>
            <span className="conv-sum" title={item.summary}>{item.summary}</span>
          </>
        ) : threadLayout === true && isShellTool(item.tool) ? (
          <span className="conv-name">Run this command?</span>
        ) : (
          <>
            <span className="conv-name">Approval needed</span>
            <span className="conv-sum" title={item.summary}>{item.tool} — {item.summary}</span>
          </>
        )}
      </div>
      {/* The same treatment the tool rows get, and it matters more here: this is
          the command the user is being asked to allow, and an escaped one-line
          JSON blob is the worst possible form for the one decision that blocks
          the agent. */}
      <ToolInput input={item.input} summary={item.summary} />

      {failedLine}

      {settled ?? (threadLayout === true ? (
        // Deny first and Allow once last, the primary at the reading end of
        // the row; the order in the DOM is the order on screen, so Tab walks
        // it the way it is read.
        <div className="conv-acts">
          {denyButton}
          <span className="conv-acts-gap" />
          {allowButton("Allow once")}
        </div>
      ) : (
        <div className="conv-acts">
          {allowButton("Allow")}
          {denyButton}
        </div>
      ))}
    </ApprovalShell>
  );
}
