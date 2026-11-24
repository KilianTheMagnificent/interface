import type { WorkflowCommand, WorkflowView } from "../shared/workflows";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SessionSummary } from "../shared/engine";
import type {
  BranchList, ThreadCommand, ThreadControlResponse, ThreadsView, WorktreeEntry,
} from "../shared/threads";
import type { ComposerThreadCard } from "./components/Composer";
import type { ComposerLaunchForm, ScopeOptions } from "./composer-model";
import { askBeforeFirstSend, type FolderTrustChoice } from "./folder-trust";
import { ipcMessage } from "./ipc-message";
import type { PromptContextItem } from "./prompt-context";
import { equalValue } from "./stable-values";
import {
  checkedThreadWorkspace, EMPTY_THREAD_CARD_MEMORY, initialThreadProject, launchThreadCard, parseThreadCardMemory,
  recentThreadProject, rememberedThreadWorkspace, rememberThreadProject, rememberThreadWorkspace,
  serializeThreadCardMemory, THREAD_CARD_STORAGE_KEY, threadCardOpening, threadCardProjects,
  threadExtrasFailureText,
  type ThreadCardMemory, type ThreadCardSeed, type ThreadLaunchOutcome, type ThreadLaunchPort,
  type ThreadWorkspaceChoice,
} from "./thread-card";

function readMemory(): ThreadCardMemory {
  try {
    return parseThreadCardMemory(window.localStorage.getItem(THREAD_CARD_STORAGE_KEY));
  } catch {
    return EMPTY_THREAD_CARD_MEMORY;
  }
}

function writeMemory(memory: ThreadCardMemory): void {
  try {
    window.localStorage.setItem(THREAD_CARD_STORAGE_KEY, serializeThreadCardMemory(memory));
  } catch {
    // Quota, privacy mode: the card simply forgets, as the sidebar does.
  }
}

/** What Rust listed for one project: branches and worktrees, or why there are none. */
interface ProjectLists {
  branches: BranchList | null;
  worktrees: readonly WorktreeEntry[] | null;
  unavailable: string | null;
}

const UNKNOWN_LISTS: ProjectLists = { branches: null, worktrees: null, unavailable: null };

export interface ThreadCardOptions {
  request: (command: ThreadCommand) => Promise<ThreadControlResponse>;
  view: ThreadsView | null;
  sessions: readonly SessionSummary[];
  pinnedProjects: readonly string[];
  /** The New thread card is open on the Threads surface. */
  open: boolean;
  /**
   * The inbox scope's project (null: No project), which a plain opening
   * starts on; undefined (All projects) leaves it to the last one used.
   */
  scopeProject?: string | null;
  pickDirectory: (preferredPath?: string) => Promise<string | null>;
  /**
   * The app's launch path, plus the lock it shares with every other launch.
   * `hold` takes it for the first thread's prepare → launch → bind (false: a
   * launch is already running) and `release` gives it back; the other
   * models start after it, unlocked. `report` puts an extra model's failure
   * on the app's action error line, since the card it came from is gone.
   */
  port: Pick<ThreadLaunchPort, "launch" | "warn" | "captureContext"> & {
    hold(): boolean;
    release(): void;
    report(message: string): void;
    /**
     * Protocol 72: shows "Trust this folder?" for `folder` and answers how
     * the owner left it. Asked before a new Claude thread's first prompt
     * when nobody has answered for its folder yet (folder-trust.ts).
     */
    askFolderTrust(folder: string): Promise<FolderTrustChoice>;
  };
}

/**
 * The New thread card's state for App: the project and workspace it is on,
 * Rust's branch and worktree lists for that project, what it remembers, and
 * the launch. Everything it shows is drawn by Composer; everything it decides
 * is in thread-card.ts.
 */
export function useThreadCard(options: ThreadCardOptions): {
  card: ComposerThreadCard;
  /**
   * Open the next card on this project: in `workspace` (Edit from turn 1), or
   * in the workspace last used there (a pane's split +). `note` is shown in
   * that card while it is open.
   */
  seed: (project: string | null, workspace?: ThreadWorkspaceChoice, note?: string) => void;
} {
  const { request, view, sessions, pinnedProjects, open, scopeProject } = options;
  const [memory, setMemory] = useState(readMemory);
  const [project, setProject] = useState<string | null>(() => initialThreadProject(memory, scopeProject));
  const [workspace, setWorkspace] = useState<ThreadWorkspaceChoice>(() =>
    project === null ? { mode: "local" } : rememberedThreadWorkspace(memory, project));
  const [lists, setLists] = useState<Readonly<Record<string, ProjectLists>>>({});
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [workflow, setWorkflow] = useState<WorkflowView | null>(null);
  const [workflowError, setWorkflowError] = useState<string | null>(null);
  const seeded = useRef<ThreadCardSeed | null>(null);
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const workflowRequest = useCallback(async (command: WorkflowCommand): Promise<WorkflowView> => {
    const answer = await request({ kind: "workflow", command });
    if (answer.workflow === undefined) throw new Error("Workflow state was not returned.");
    setWorkflow(answer.workflow);
    setWorkflowError(null);
    if (command.action === "retrySetup") {
      const refreshed = await request({ kind: "listWorktrees", projectRoot: command.projectRoot });
      setLists(current => ({
        ...current,
        [command.projectRoot]: {
          ...(current[command.projectRoot] ?? UNKNOWN_LISTS),
          worktrees: refreshed.worktrees ?? [],
        },
      }));
    }
    return answer.workflow;
  }, [request]);
  useEffect(() => {
    if (!open || project === null) return;
    let current = true;
    const read = (): void => { void request({ kind: "workflow", command: { action: "read", projectRoot: project } })
      .then(answer => { if (current) { setWorkflow(answer.workflow ?? null); setWorkflowError(null); } })
      .catch((error: unknown) => { if (current) setWorkflowError(ipcMessage(error)); }); };
    read();
    const timer = busy ? window.setInterval(read, 1000) : undefined;
    return () => { current = false; window.clearInterval(timer); };
  }, [busy, open, project, request]);

  // Every opening is a fresh card: on a seeded project (Edit from turn 1, a
  // pane's split +), else the inbox scope's project, else the last one
  // launched from; in the workspace mode last used there.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      const opening = threadCardOpening(memory, seeded.current, scopeProject, root => lists[root] ?? {},
        recentThreadProject(view?.threads ?? []));
      seeded.current = null;
      setProject(opening.project);
      setWorkspace(opening.workspace);
      setError(null);
      setNote(opening.note ?? null);
    }
  }

  // Rust's lists for the chosen project, read once per opening.
  const readLists = useCallback((root: string) => {
    void Promise.allSettled([
      request({ kind: "listWorktrees", projectRoot: root }),
      request({ kind: "listBranches", projectRoot: root }),
    ]).then(([worktrees, branches]) => {
      setLists(current => ({
        ...current,
        [root]: {
          worktrees: worktrees.status === "fulfilled" ? worktrees.value.worktrees ?? [] : [],
          branches: branches.status === "fulfilled" ? branches.value.branches ?? null : null,
          unavailable: branches.status === "rejected" ? ipcMessage(branches.reason) : null,
        },
      }));
    });
  }, [request]);
  useEffect(() => {
    if (open && project !== null) readLists(project);
  }, [open, project, readLists]);

  const known = project === null ? UNKNOWN_LISTS : lists[project] ?? UNKNOWN_LISTS;
  // A remembered existing worktree or base branch that Rust no longer lists
  // falls back once the lists arrive.
  useEffect(() => {
    if (project === null || known === UNKNOWN_LISTS) return;
    setWorkspace(current => {
      const checked = checkedThreadWorkspace(current, known);
      return equalValue(checked, current) ? current : checked;
    });
  }, [known, project]);

  const changeProject = useCallback((next: string | null) => {
    setProject(next);
    setWorkspace(next === null ? { mode: "local" }
      : rememberedThreadWorkspace(memory, next, lists[next] ?? {}));
    setError(null);
  }, [lists, memory]);
  const changeWorkspace = useCallback((next: ThreadWorkspaceChoice) => {
    setWorkspace(next);
    setError(null);
  }, []);
  const chooseFolder = useCallback(() =>
    optionsRef.current.pickDirectory(project ?? undefined), [project]);

  const launch = useCallback(async (forms: readonly ComposerLaunchForm[], scope: ScopeOptions,
      context: readonly PromptContextItem[], background: boolean): Promise<boolean> => {
    const { port } = optionsRef.current;
    if (!port.hold()) return false;
    setBusy(true);
    setError(null);
    let outcome: ThreadLaunchOutcome;
    try {
      // The folder's own settings load only once it is trusted: ask before
      // the first prompt goes out. Closing the question keeps the draft.
      const sends = await askBeforeFirstSend({ request, ask: folder => port.askFolderTrust(folder) },
        project, forms);
      if (!sends) return false;
      outcome = await launchThreadCard({ request, launch: port.launch, warn: port.warn, captureContext: port.captureContext },
        { project, workspace, forms, scope, context, background });
    } finally {
      // The lock covers the first thread only: the other models start in
      // the background while the owner moves on, never freezing navigation.
      setBusy(false);
      port.release();
    }
    if (outcome.status === "prepare-failed") setError(outcome.message);
    if (outcome.status !== "launched") return false;
    // A card left open (⌘Enter) is a fresh one: what led to it is done.
    setNote(null);
    setMemory(current => {
      let next = rememberThreadProject(current, project);
      if (project !== null) next = rememberThreadWorkspace(next, project, workspace);
      writeMemory(next);
      return next;
    });
    // A new worktree now exists (one per model): a card still open lists
    // them again now, and the next opening does otherwise.
    const forgetLists = (): void => {
      if (project === null || workspace.mode !== "worktree") return;
      if (optionsRef.current.open) {
        readLists(project);
        return;
      }
      setLists(current => {
        const { [project]: _stale, ...rest } = current;
        return rest;
      });
    };
    forgetLists();
    void outcome.extras.then(extras => {
      if (extras.keys.length > 0) forgetLists();
      const failed = threadExtrasFailureText(extras.failures);
      if (failed !== null) optionsRef.current.port.report(failed);
    });
    return true;
  }, [project, readLists, request, workspace]);

  const projects = useMemo(() => threadCardProjects({
    view, sessions, pinned: pinnedProjects, current: project,
  }), [pinnedProjects, project, sessions, view]);

  const card = useMemo<ComposerThreadCard>(() => ({
    projects,
    project,
    onProjectChange: changeProject,
    onChooseFolder: chooseFolder,
    workspace,
    onWorkspaceChange: changeWorkspace,
    branches: known.branches,
    worktrees: known.worktrees,
    worktreeUnavailable: known.unavailable,
    error,
    note,
    busy,
    onLaunch: launch,
    workflow: { view: workflow?.projectRoot === project ? workflow : null, error: workflowError, request: workflowRequest },
  }), [busy, changeProject, changeWorkspace, chooseFolder, error, known, launch, note, project,
    projects, workspace, workflow, workflowError, workflowRequest]);

  // Applied at the next opening; an open card (reseeded in place by its
  // caller) takes it at once.
  const openRef = useRef(open);
  openRef.current = open;
  const memoryRef = useRef(memory);
  memoryRef.current = memory;
  const listsRef = useRef(lists);
  listsRef.current = lists;
  const seed = useCallback((nextProject: string | null, nextWorkspace?: ThreadWorkspaceChoice,
      nextNote?: string) => {
    const next: ThreadCardSeed = {
      project: nextProject,
      ...(nextWorkspace === undefined ? {} : { workspace: nextWorkspace }),
      ...(nextNote === undefined ? {} : { note: nextNote }),
    };
    if (!openRef.current) {
      seeded.current = next;
      return;
    }
    const opening = threadCardOpening(memoryRef.current, next, undefined,
      root => listsRef.current[root] ?? {});
    setProject(opening.project);
    setWorkspace(opening.workspace);
    setError(null);
    setNote(opening.note ?? null);
  }, []);

  return { card, seed };
}
