import { useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { SessionSummary } from "../../shared/engine";
import type { TitleSource } from "../../shared/threads";
import { projectHueClass } from "../project-hue";
import { NO_PROJECT_LABEL } from "../thread-inbox";
import { threadDisplayTitle } from "../thread-title";
import { threadChangesCue, type ThreadChangesCue } from "../thread-frame";
import "./ProjectHue.css";
import { ProjectTile } from "./ProjectHueParts";
import { ThreadPaneMenu, type ThreadProviderBridges } from "./ThreadPaneHead";
import { sessionStatusWord } from "./status-words";

export interface AgentTileProps {
  session: SessionSummary;
  selected: boolean;
  maximized: boolean;
  live: boolean;
  /** Probe-owned display name; the stable engine id still routes the session. */
  engineLabel: string;
  /** The project the thread belongs to, drawn under its title; absent for a plain chat. */
  projectLabel?: string;
  /**
   * Where the thread's title came from (Rust's record): a title still its
   * first prompt is shown short (`threadDisplayTitle`); absent when unknown.
   */
  titleSource?: TitleSource;
  /**
   * The project's folder: the pane takes its hue (`project-hue.ts`), and
   * everything inside — bubbles, composer, Review — reads `--project-h`.
   */
  projectRoot?: string | null;
  /** Starts a New Chat in this tile's project and opens it beside this pane. */
  onSplit?: () => void;
  /** True at the four-pane ceiling: the control stays, and says why. */
  splitDisabled?: boolean;
  /** Open in terminal (review C1): a Threads pane resumes its session in Code. */
  onOpenInTerminal?: () => void;
  /** Why Open in terminal does not apply now; null when it does. */
  terminalRefusal?: string | null;
  /** Second opinion from … and Continue with … (review B4). */
  providers?: ThreadProviderBridges;
  /** Schedule a prompt… (protocol 62): opens the schedule dialog for this thread. */
  onSchedule?: () => void;
  /** Side question… (protocol 67): starts `/btw ` in this thread's composer. */
  onSideQuestion?: () => void;
  /** Drawn under the title, after the project: where the thread works, its pull request and lineage. */
  headDetail?: ReactNode;
  /** Drawn first among the head's actions: a Threads pane's Changes button. */
  headActions?: ReactNode;
  onFocus: () => void;
  onToggleMaximize: () => void;
  onClose: () => void;
  children: ReactNode;
}

/**
 * The cue for the Changes chip, read from what the chip printed this render.
 * The chip is the head's own child (App's ThreadDiffToggle in `headActions`),
 * and its totals text is the one fact the cue needs, so the tile reads it
 * where it is drawn rather than asking the shell for a second copy.
 */
function useChangesCue(head: { current: HTMLElement | null }): ThreadChangesCue {
  const seen = useRef<string | null | undefined>(undefined);
  const current = useRef<ThreadChangesCue>("");
  const [cue, setCue] = useState<ThreadChangesCue>("");
  useLayoutEffect(() => {
    const stat = head.current?.querySelector(".thread-head-changes-stat")?.textContent ?? null;
    const next = threadChangesCue(seen.current, stat, current.current);
    seen.current = stat;
    if (next === current.current) return;
    current.current = next;
    setCue(next);
  });
  return cue;
}

/**
 * One independently mounted thread conversation in the Threads grid. The tile
 * is deliberately only chrome: Conversation continues to own its draft, scroll
 * pin and transcript state, so rearranging or focusing panes cannot move text
 * from one provider session to another.
 *
 * The head (owner, 2026-11-15) is two lines: the thread's short title, then
 * one plain line — the project's hue square and name, then where the thread
 * works and where it came from (`headDetail`) — with the Changes button and
 * one ⋯ for the pane's own commands on the right. The provider's mark and the
 * access mode live in the composer's chips, so the mark appears once per pane.
 *
 * The head is live (owner, 2026-10-07, "a live turn"): a working thread runs
 * a hairline along its bottom edge, a thread waiting on its owner holds a
 * still amber one and a "Needs you" pill before Changes (both from the
 * data-status the tile carries, in theme.css), and the Changes chip pops when
 * its totals arrive and flashes when they change. The state stays in the
 * accessible names; the pill is drawn, not said twice.
 */
export function AgentTile({
  session, selected, maximized, live, engineLabel, projectLabel, titleSource, projectRoot, onSplit, splitDisabled,
  onOpenInTerminal, terminalRefusal, providers, onSchedule, onSideQuestion, headDetail, headActions, onFocus, onToggleMaximize, onClose,
  children,
}: AgentTileProps) {
  const statusLabel = sessionStatusWord(session.status, live);
  const splitLabel = session.kind === "chat" || session.kind === "voice"
    ? "Open a new chat pane" : "Open a new pane in this project";
  const head = useRef<HTMLElement>(null);
  const changesCue = useChangesCue(head);
  const hueClass = projectHueClass(projectRoot);
  const shortTitle = threadDisplayTitle(session.title, titleSource, session.titleFromSummary);
  const project = projectLabel !== undefined && projectLabel !== "" ? projectLabel : null;
  return (
    <section
      className={`workspace-tile workspace-agent${selected ? " is-selected" : ""}${
        maximized ? " is-maximized" : ""}${hueClass === "" ? "" : ` ${hueClass}`}`}
      data-agent-key={`${session.engine}#${session.sessionId}`}
      data-status={session.status}
      data-live={live || undefined}
      aria-label={`${session.title} — ${engineLabel} — ${statusLabel}`}
      onPointerDown={onFocus}
      onFocusCapture={onFocus}
    >
      <header ref={head} className="workspace-tile-head"
        data-changes-cue={changesCue === "" ? undefined : changesCue}>
        <div className="workspace-tile-heading">
          {/* One line, never wrapping: the short title, the whole one in the
              tooltip. Focusing a pane lands here, so it wears the app's ring. */}
          <button
            type="button"
            className="workspace-tile-title"
            title={session.title}
            aria-label={`Focus ${session.title}, ${engineLabel}, ${statusLabel}`}
            onClick={onFocus}
          >
            <span className="workspace-tile-name">{shortTitle}</span>
          </button>
          <div className="workspace-tile-where">
            <ProjectTile root={project === null ? null : projectRoot} name={project ?? NO_PROJECT_LABEL} large />
            <span className="workspace-tile-project">{project ?? NO_PROJECT_LABEL}</span>
            {headDetail}
          </div>
        </div>
        <div className="workspace-tile-actions">
          {/* Blocked on the owner, said before Changes. The tile's and the
              title's accessible names already carry the state. */}
          {session.status === "waiting" && (
            <span className="workspace-tile-needs" aria-hidden="true">Needs you</span>
          )}
          {headActions}
          <ThreadPaneMenu title={session.title} maximized={maximized}
            onSplit={onSplit} splitDisabled={splitDisabled} splitLabel={splitLabel}
            onOpenInTerminal={onOpenInTerminal} terminalRefusal={terminalRefusal}
            providers={providers} onSchedule={onSchedule} onSideQuestion={onSideQuestion}
            onToggleMaximize={onToggleMaximize} onClose={onClose} />
        </div>
      </header>
      <div className="workspace-tile-body">{children}</div>
    </section>
  );
}
