import "./ThreadPaneHead.css";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import type { EngineId } from "../../shared/engine";
import type { CollaborationEngine } from "../../shared/ipc";
import type { LineageRelation, ThreadPullRequest, ThreadWorkspace } from "../../shared/threads";
import type { ThreadProviderOffers } from "../thread-opinion";
import { homeRelativePath, lastSegment } from "../display-paths";
import { threadWorkspaceLabel } from "../thread-pane";
import { APP_SELECT_DISMISS_EVENT } from "./AppSelect";
import { DiffstatBar } from "./ProjectHueParts";
import { useTerminalMenuPopover } from "./TerminalMenu";
import { ThreadGlyph } from "./ThreadGlyph";

/**
 * What a Threads pane's head says under the title, after the project: where
 * the thread works (its branch and where its edits land — or that its
 * worktree is gone and comes back with the next message), its pull request,
 * and where it was forked or edited from. Rust says all of it; the head only
 * words it.
 */
export interface ThreadPaneHeadProps {
  workspace?: ThreadWorkspace;
  /**
   * The branch Rust last read the thread's checkout on (its ship read,
   * protocol 37), named when the workspace recorded none: a Local thread
   * works on whatever the project has checked out.
   */
  checkoutBranch?: string | undefined;
  /** The pull request Rust found for the thread's branch. */
  pullRequest?: ThreadPullRequest;
  /** The thread this one was forked or edited from, when it is known. */
  lineage?: { relation: LineageRelation; title: string; onSelect: () => void };
  /**
   * The provider this thread ran on before its current one (protocol 59,
   * Switch to): "Switched from Claude Code" where a continued thread says
   * "Continued from …". Absent for a thread never switched.
   */
  switchedFrom?: string;
  /**
   * Protocol 60: the provider account it runs on ("Work", "Default", or
   * "account removed"); null or absent when its provider has one account.
   */
  account?: string | null;
  /**
   * Protocol 65: the client name of the agent outside Interface that started
   * this thread through `iface mcp`; absent for every other thread.
   */
  startedFromOutside?: string;
}

/** "Started from outside (claude-code)": a thread an outside agent started. */
export function startedFromOutsideWords(client: string): string {
  return `Started from outside (${client})`;
}

export const MISSING_WORKTREE_TEXT = "worktree removed — recreated on next message";

/** "PR #42 · open": the chip's words, and the hover card's. */
export function threadPullRequestLabel(pullRequest: Pick<ThreadPullRequest, "number" | "state">): string {
  return `PR #${pullRequest.number} · ${pullRequest.state}`;
}

/**
 * The pull request's address as a link, or undefined: only a web address is
 * one. The link opens through the window's external-link handler (a
 * `target="_blank"` anchor, as transcript links do), which checks it again.
 */
export function pullRequestHref(url: string): string | undefined {
  const value = url.trim();
  if (value === "" || /[\u0000-\u001f\u007f]/u.test(value)) return undefined;
  try {
    const parsed = new URL(value);
    return (parsed.protocol === "https:" || parsed.protocol === "http:") && parsed.hostname !== ""
      && parsed.username === "" && parsed.password === "" ? parsed.href : undefined;
  } catch {
    return undefined;
  }
}

function PullRequestChip({ pullRequest }: { pullRequest: ThreadPullRequest }) {
  const label = threadPullRequestLabel(pullRequest);
  const href = pullRequestHref(pullRequest.url);
  const className = `thread-head-pr is-${pullRequest.state}`;
  const content = <><ThreadGlyph name="pr" size={12} /><span>{label}</span></>;
  return href === undefined
    ? <span className={className} title={label}>{content}</span>
    : <a className={className} href={href} target="_blank" rel="noreferrer noopener"
      title={`Open pull request #${pullRequest.number} (${pullRequest.state})`}
      onClick={event => { event.stopPropagation(); }}>{content}</a>;
}

/**
 * "Forked from ", "Edited from ", "Continued from " a thread on another
 * provider, or "Started by " the thread whose agent started this one
 * (team tools, protocol 57).
 */
export function lineageWords(relation: LineageRelation): string {
  switch (relation) {
    case "edit": return "Edited from ";
    case "handoff": return "Continued from ";
    case "fork": return "Forked from ";
    case "child": return "Started by ";
  }
}

/**
 * Second opinion and Switch to (review B4; Switch to since protocol 59): the
 * other providers a thread can ask or move to, and why they wait (a turn
 * runs), for the pane's ⋯ and the inbox row's menu.
 */
export interface ThreadProviderBridges {
  offers: ThreadProviderOffers;
  /** Why neither applies right now (a turn is running); null when they do. */
  refusal: string | null;
  onSecondOpinion: (engine: CollaborationEngine) => void;
  onSwitchTo: (engine: EngineId) => void;
}

/** The workspace's words in one (the Changes panel's and a tooltip's): "main · Local". */
export function threadHeadWorkspaceLabel(workspace: ThreadWorkspace | undefined): string | null {
  if (workspace?.mode === "worktree" && (workspace.branch ?? "").trim() === ""
      && workspace.worktreePath !== undefined) {
    return `${lastSegment(workspace.worktreePath)} · Worktree`;
  }
  return threadWorkspaceLabel(workspace);
}

export const LOCAL_WHERE_WORDS = "edits land in the project folder";
export const WORKTREE_WHERE_WORDS = "edits land in a separate worktree";
export const CHAT_WHERE_WORDS = "a chat, no files are edited";

/**
 * Where the thread works, in plain words (owner, 2026-11-15): the branch as
 * "on main" when Rust recorded one, then where its edits land: the project
 * folder for a Local thread, a separate worktree (named by its folder when
 * there is no branch) for a worktree thread, and none for a plain chat.
 */
export function threadHeadWhere(workspace: ThreadWorkspace | undefined, checkoutBranch?: string):
    { branch: string | null; where: string } | null {
  if (workspace === undefined) return null;
  if (workspace.mode === "none") return { branch: null, where: CHAT_WHERE_WORDS };
  const recorded = workspace.branch?.trim() ?? "";
  const branch = recorded !== "" ? recorded : checkoutBranch?.trim() ?? "";
  if (workspace.mode === "worktree") {
    if (branch === "" && workspace.worktreePath !== undefined) {
      return { branch: null, where: `edits land in the worktree ${lastSegment(workspace.worktreePath)}` };
    }
    return { branch: branch === "" ? null : branch, where: WORKTREE_WHERE_WORDS };
  }
  return { branch: branch === "" ? null : branch, where: LOCAL_WHERE_WORDS };
}

/** The quiet dot between two parts of the head's line. */
function Dot() {
  return <span className="thread-head-dot" aria-hidden="true">·</span>;
}

/**
 * The second line of a Threads pane's head, after the project (owner,
 * 2026-11-15): one plain line, each part after a quiet dot — the branch
 * ("on main"), where edits land, the provider account, the pull request,
 * and where the thread came from. A plain chat says it edits no files.
 */
export function ThreadPaneHead({
  workspace, checkoutBranch, pullRequest, lineage, switchedFrom, account, startedFromOutside,
}: ThreadPaneHeadProps) {
  const worktree = workspace?.mode === "worktree" ? workspace : undefined;
  const label = threadHeadWorkspaceLabel(workspace);
  const where = threadHeadWhere(workspace, checkoutBranch);
  const accountLabel = account ?? null;
  if (where === null && pullRequest === undefined && lineage === undefined
    && switchedFrom === undefined && accountLabel === null
    && startedFromOutside === undefined) return null;
  const tooltip = label === null ? undefined : worktree?.worktreePath === undefined ? label
    : `${label} · ${homeRelativePath(worktree.worktreePath)}`;
  return (
    <span className="thread-head">
      {worktree?.missing === true ? <>
        <Dot />
        <span className="thread-head-branch is-missing"
          title={worktree.worktreePath === undefined ? MISSING_WORKTREE_TEXT
            : `${homeRelativePath(worktree.worktreePath)} is gone. It is recreated from its branch on the next message.`}>
          {MISSING_WORKTREE_TEXT}
        </span>
      </> : where !== null && <>
        {where.branch !== null && <>
          <Dot />
          <span className="thread-head-branch" title={tooltip}>
            on <span className="thread-head-branch-name">{where.branch}</span>
          </span>
        </>}
        <Dot />
        <span className="thread-head-where" title={tooltip}>{where.where}</span>
      </>}
      {accountLabel !== null && <>
        <Dot />
        <span className="thread-head-place thread-head-account" title={`Provider account: ${accountLabel}`}>
          {accountLabel}
        </span>
      </>}
      {pullRequest !== undefined && <><Dot /><PullRequestChip pullRequest={pullRequest} /></>}
      {lineage !== undefined && <>
        <Dot />
        <button type="button" className="thread-head-lineage"
          title={`Open ${lineage.title}`}
          onClick={event => { event.stopPropagation(); lineage.onSelect(); }}>
          {lineageWords(lineage.relation)}
          <span className="thread-head-parent">{lineage.title}</span>
        </button>
      </>}
      {switchedFrom !== undefined && <>
        <Dot />
        <span className="thread-head-lineage" data-thread-switched=""
          title={`This thread ran on ${switchedFrom} before; its whole conversation is here.`}>
          Switched from <span className="thread-head-parent">{switchedFrom}</span>
        </span>
      </>}
      {startedFromOutside !== undefined && <>
        <Dot />
        <span className="thread-head-lineage" data-thread-outside=""
          title={`${startedFromOutside}, an agent outside Interface, started this thread. It asks you before it acts.`}>
          {startedFromOutsideWords(startedFromOutside)}
        </span>
      </>}
    </span>
  );
}

/**
 * The pane head's Changes button (⌘D): the changes glyph, the word, the
 * thread's totals and their five squares, so how much the thread changed
 * reads before the panel opens. Focus comes back to it when the panel closes.
 * Unknown totals print nothing, never a zero.
 */
export function ThreadDiffToggle({ open, onToggle, totals }: {
  open: boolean;
  onToggle: () => void;
  totals?: { added: number; removed: number } | null;
}) {
  return (
    <button type="button" data-thread-diff-toggle=""
      className={open ? "thread-head-changes is-active" : "thread-head-changes"}
      aria-pressed={open}
      aria-label={open ? "Hide changes" : "Show changes"}
      title={open ? "Hide changes (⌘D)" : "Changes (⌘D)"}
      onClick={onToggle}>
      <ThreadGlyph name="changes" size={14} />
      <span>Changes</span>
      {totals !== undefined && totals !== null && <>
        <span className="thread-head-changes-stat" aria-hidden="true">
          <span className="thread-head-added">+{totals.added}</span>
          <span className="thread-head-removed">−{totals.removed}</span>
        </span>
        <DiffstatBar added={totals.added} removed={totals.removed} />
      </>}
    </button>
  );
}

/** The ⋯ menu's rows: split, maximize or restore, close. Exported for static tests. */
export function ThreadPaneMenuItems({ title, maximized, onSplit, splitDisabled, splitLabel,
    onOpenInTerminal, terminalRefusal = null, providers, onSchedule, onSideQuestion, choose, onToggleMaximize,
    onClose }: {
  title: string;
  maximized: boolean;
  onSplit?: () => void;
  splitDisabled?: boolean;
  splitLabel: string;
  /** Open in terminal (review C1): resume this thread's session in a Code pane. */
  onOpenInTerminal?: () => void;
  /** Why Open in terminal does not apply now; null when it does. */
  terminalRefusal?: string | null;
  /** Second opinion from … and Switch to … (review B4, protocol 59). */
  providers?: ThreadProviderBridges;
  /** Schedule a prompt… (protocol 62); absent outside Threads. */
  onSchedule?: () => void;
  /** Side question… (protocol 67): starts `/btw ` in the composer; absent where none can be asked. */
  onSideQuestion?: () => void;
  choose: (action: () => void) => void;
  onToggleMaximize: () => void;
  onClose: () => void;
}) {
  return <>
    {onSplit !== undefined && (
      <button type="button" className="terminal-menu-launch" role="menuitem"
        disabled={splitDisabled === true}
        aria-label={splitDisabled === true ? `${splitLabel} — workspace is full (4 panes)` : splitLabel}
        title={splitDisabled === true ? "Workspace is full (4 panes)" : undefined}
        onClick={() => { choose(onSplit); }}>
        <span className="codicon codicon-add" aria-hidden="true" />
        <span>New thread beside this one</span>
      </button>
    )}
    {onOpenInTerminal !== undefined && (
      <button type="button" className="terminal-menu-launch" role="menuitem"
        disabled={terminalRefusal !== null}
        title={terminalRefusal ?? "Resume this session in a Code pane. The thread stops driving it while the pane runs."}
        onClick={() => { choose(onOpenInTerminal); }}>
        <span className="codicon codicon-terminal" aria-hidden="true" />
        <span>Open in terminal</span>
      </button>
    )}
    {providers?.offers.opinion.map(choice => (
      <button key={`opinion-${choice.engine}`} type="button" className="terminal-menu-launch"
        role="menuitem" data-thread-opinion={choice.engine} disabled={providers.refusal !== null}
        title={providers.refusal ?? `${choice.label} reads this thread's latest answer and its changes, then answers here. It can't change files.`}
        onClick={() => { choose(() => { providers.onSecondOpinion(choice.engine); }); }}>
        <span className="codicon codicon-comment-discussion" aria-hidden="true" />
        <span>Second opinion from {choice.label}</span>
      </button>
    ))}
    {providers?.offers.switchTo.map(choice => (
      <button key={`switch-${choice.engine}`} type="button" className="terminal-menu-launch"
        role="menuitem" data-thread-switch={choice.engine} disabled={providers.refusal !== null}
        title={providers.refusal ?? `Carry on in this thread on ${choice.label}. It is told where things stand first.`}
        onClick={() => { choose(() => { providers.onSwitchTo(choice.engine); }); }}>
        <span className="codicon codicon-arrow-swap" aria-hidden="true" />
        <span>Switch to {choice.label}</span>
      </button>
    ))}
    {onSchedule !== undefined && (
      <button type="button" className="terminal-menu-launch" role="menuitem" data-thread-schedule-open=""
        title="Send this thread a prompt later, once or on a repeat."
        onClick={() => { choose(onSchedule); }}>
        <span className="codicon codicon-watch" aria-hidden="true" />
        <span>Schedule a prompt…</span>
      </button>
    )}
    {onSideQuestion !== undefined && (
      <button type="button" className="terminal-menu-launch" role="menuitem" data-thread-side-question=""
        title="Ask something without adding it to this thread. The answer shows on the right."
        onClick={() => { choose(onSideQuestion); }}>
        <span className="codicon codicon-comment" aria-hidden="true" />
        <span>Side question…</span>
      </button>
    )}
    <button type="button" className="terminal-menu-launch" role="menuitem"
      aria-label={maximized ? "Restore agent pane" : "Maximize agent pane"}
      onClick={() => { choose(onToggleMaximize); }}>
      <span className={`codicon codicon-${maximized ? "collapse-diagonal" : "expand-diagonal"}`}
        aria-hidden="true" />
      <span>{maximized ? "Restore" : "Maximize"}</span>
      <kbd className="workspace-menu-shortcut">⌘⇧M</kbd>
    </button>
    <div className="terminal-pane-menu-divider" role="separator" />
    <button type="button" className="terminal-menu-launch" role="menuitem"
      aria-label={`Close ${title} pane`}
      onClick={() => { choose(onClose); }}>
      <span className="codicon codicon-close" aria-hidden="true" />
      <span>Close pane</span>
    </button>
  </>;
}

/**
 * The pane's own commands behind one ⋯ (redesign, 2026-10-06): a new thread
 * beside this one, maximize, close. The head keeps what is about the thread;
 * these are about the pane, and maximize keeps its shortcut.
 */
export function ThreadPaneMenu({ title, maximized, onSplit, splitDisabled, splitLabel,
    onOpenInTerminal, terminalRefusal, providers, onSchedule, onSideQuestion, onToggleMaximize, onClose }: {
  title: string;
  maximized: boolean;
  onSplit?: () => void;
  splitDisabled?: boolean;
  splitLabel: string;
  onOpenInTerminal?: () => void;
  terminalRefusal?: string | null;
  providers?: ThreadProviderBridges;
  onSchedule?: () => void;
  onSideQuestion?: () => void;
  onToggleMaximize: () => void;
  onClose: () => void;
}) {
  const [open, setOpen] = useState(false);
  const { triggerRef, menuRef, box, onMenuKey } = useTerminalMenuPopover({
    open, onOpenChange: setOpen, minWidth: 220, align: "end",
  });
  useEffect(() => {
    const dismiss = (): void => { setOpen(false); };
    window.addEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
    return () => window.removeEventListener(APP_SELECT_DISMISS_EVENT, dismiss);
  }, []);
  const choose = (action: () => void): void => {
    setOpen(false);
    triggerRef.current?.focus();
    action();
  };
  const panel = open ? (
    <div ref={menuRef} className="terminal-menu-panel is-floating thread-pane-menu"
      role="menu" aria-label={`${title} pane`} onKeyDown={onMenuKey}
      style={box === null ? { visibility: "hidden", top: 0, left: 0 }
        : { top: box.top, left: box.left, width: box.width, maxWidth: "calc(100vw - 16px)", maxHeight: box.maxHeight }}>
      <ThreadPaneMenuItems title={title} maximized={maximized} onSplit={onSplit}
        splitDisabled={splitDisabled} splitLabel={splitLabel} choose={choose}
        onOpenInTerminal={onOpenInTerminal} terminalRefusal={terminalRefusal}
        providers={providers} onSchedule={onSchedule} onSideQuestion={onSideQuestion}
        onToggleMaximize={onToggleMaximize} onClose={onClose} />
    </div>
  ) : null;
  return <>
    <button ref={triggerRef} type="button" className="workspace-icon-btn thread-pane-menu-button"
      aria-label={`${title} pane actions`} title="Pane actions"
      aria-haspopup="menu" aria-expanded={open}
      onClick={() => { setOpen(!open); }}>
      <ThreadGlyph name="more" />
    </button>
    {panel !== null && (typeof document === "undefined" ? panel : createPortal(panel, document.body))}
  </>;
}
