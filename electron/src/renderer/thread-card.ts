import type { SessionKind, SessionSummary } from "../shared/engine";
import type {
  BranchList, PrepareWorkspaceMode, ThreadCommand, ThreadControlResponse, ThreadsView,
  ThreadView, WorktreeEntry,
} from "../shared/threads";
import { asRecord } from "./helpers";
import { composerKindEngines, type ComposerLaunchForm, type EngineChoices,
  type EngineOption, type ScopeOptions } from "./composer-model";
import { lastSegment } from "./display-paths";
import { ipcMessage } from "./ipc-message";
import type { PromptContextItem } from "./prompt-context";

/**
 * The New thread card's pure model (the Threads surface's Composer).
 *
 * The card picks a project first. "No project" is today's chat card exactly:
 * kind `chat`, chat-capable engines only, no tools. A project makes it a
 * coding session (kind `code`) with every engine, the access mode and a
 * workspace: the project's own checkout (Local, the default), a new git
 * worktree, or a worktree that already exists. Rust prepares the workspace
 * (`prepareWorkspace`), the ordinary launch path starts the session in the
 * folder Rust answered, and `bind` ties the two together. Nothing here decides
 * where a worktree goes or whether one may be created; Rust does.
 */

/** The workspace control's value. */
export type ThreadWorkspaceChoice =
  | { mode: "local" }
  | { mode: "worktree"; baseRef?: string }
  | { mode: "existing"; worktreePath: string };

export const LOCAL_WORKSPACE: ThreadWorkspaceChoice = { mode: "local" };

/** The card's kind for a project choice: no project is a plain chat. */
export function threadCardKind(project: string | null): SessionKind {
  return project === null ? "chat" : "code";
}

/** No project offers the chat engines only; a project offers every engine. */
export function threadCardEngines(project: string | null, engines: readonly EngineOption[],
    choicesFor: (engine: string) => EngineChoices): readonly EngineOption[] {
  return composerKindEngines(threadCardKind(project), engines, choicesFor);
}

/** Several models at once start one thread each, which only a new worktree per thread keeps apart. */
export function threadCardAllowsSeveralModels(project: string | null,
    workspace: ThreadWorkspaceChoice): boolean {
  return project !== null && workspace.mode === "worktree";
}

/* ---- the card's words -------------------------------------------------------- */

/**
 * The card's headline around its project picker, and its prompt's
 * placeholder. A project asks what to build in it ("What should we build in
 * [atlas ▾]?"); No project asks an open question and the picker, reading
 * "No project", sits beside it.
 */
export interface ThreadCardWording {
  /** The words before the project picker. */
  lead: string;
  /** After the picker: "?" closes the question; "" when `lead` is a whole sentence. */
  trail: string;
  placeholder: string;
}

export function threadCardWording(project: string | null): ThreadCardWording {
  return project === null
    ? { lead: "What’s on your mind?", trail: "", placeholder: "Ask anything." }
    : { lead: "What should we build in", trail: "?",
        placeholder: "Describe the task. @ mentions a file, / runs a command." };
}

/** The workspace drawer's one line: where this thread's edits land. */
export function threadWorkspaceLine(choice: ThreadWorkspaceChoice, projectLabel: string): string {
  if (choice.mode === "local") return `Edits land in ${projectLabel} itself.`;
  if (choice.mode === "worktree") return "A new branch in its own folder.";
  return "Edits land in a worktree that already exists.";
}

/* ---- the project list ------------------------------------------------------ */

export interface ThreadCardProject {
  root: string;
  /** The folder's name, with its parent when two recent projects share one. */
  label: string;
}

/** A Rust-made thread worktree, never a project in its own right. */
export function isThreadWorktreePath(path: string): boolean {
  return /[\\/]\.interface-worktrees[\\/]/u.test(path);
}

const MAX_RECENT_PROJECTS = 12;

/**
 * Recent projects, newest first: the project roots of known threads, the
 * projects sessions ran in, and the owner's pinned projects. A worktree a
 * thread made is never offered as a project of its own. `current` is always
 * kept, so the picker never loses the value it shows.
 */
export function threadCardProjects(input: {
  view: ThreadsView | null;
  sessions: readonly SessionSummary[];
  pinned: readonly string[];
  current: string | null;
  limit?: number;
}): ThreadCardProject[] {
  const seen = new Map<string, number>();
  const note = (root: string | undefined, at: number): void => {
    const path = root?.trim() ?? "";
    if (path === "" || isThreadWorktreePath(path)) return;
    seen.set(path, Math.max(seen.get(path) ?? Number.NEGATIVE_INFINITY, at));
  };
  for (const thread of input.view?.threads ?? []) {
    const workspace = thread.workspace;
    if (workspace === undefined || workspace.mode === "none") continue;
    note(workspace.projectRoot, Math.max(thread.createdAt, thread.lastRun?.startedAt ?? 0));
  }
  for (const session of input.sessions) {
    const at = Date.parse(session.lastActivity);
    note(session.worktreeHome ?? session.projectRoot, Number.isFinite(at) ? at : 0);
  }
  const recent = [...seen].sort((a, b) => b[1] - a[1]).map(([root]) => root);
  const pinned = input.pinned.filter(root => root.trim() !== "" && !isThreadWorktreePath(root));
  const ordered: string[] = [];
  const add = (root: string): void => { if (!ordered.includes(root)) ordered.push(root); };
  pinned.forEach(add);
  recent.slice(0, input.limit ?? MAX_RECENT_PROJECTS).forEach(add);
  if (input.current !== null && input.current !== "") add(input.current);
  const names = new Map<string, number>();
  for (const root of ordered) {
    const name = lastSegment(root);
    names.set(name, (names.get(name) ?? 0) + 1);
  }
  return ordered.map(root => {
    const name = lastSegment(root) || root;
    if ((names.get(lastSegment(root)) ?? 0) < 2) return { root, label: name };
    const trimmed = root.replace(/[\\/]+$/u, "");
    const parent = lastSegment(trimmed.slice(0, Math.max(0, trimmed.length - name.length - 1)));
    return { root, label: parent === "" ? name : `${name} (${parent})` };
  });
}

/* ---- what the card remembers ------------------------------------------------
   A renderer preference, like the sidebar's collapse memory: the last project
   the card launched in, and the last workspace mode used for each project.
   Every read tolerates corrupt or foreign data; every write may fail. */

export const THREAD_CARD_STORAGE_KEY = "interface.threadCard.v1";
const MAX_REMEMBERED_PROJECTS = 64;

/** A workspace a project may default to: Local or New worktree, never one that already exists. */
export type RememberedThreadWorkspace = Exclude<ThreadWorkspaceChoice, { mode: "existing" }>;

export interface ThreadCardMemory {
  /** Absent: never launched from the card. null: "No project" was last. */
  lastProject?: string | null;
  /** Project root → the last workspace used there, newest first. */
  workspaces: Record<string, RememberedThreadWorkspace>;
}

export const EMPTY_THREAD_CARD_MEMORY: ThreadCardMemory = { workspaces: {} };

/** A stored choice; an existing worktree an earlier build remembered reads as none. */
function parseChoice(value: unknown): RememberedThreadWorkspace | null {
  const record = asRecord(value);
  if (record === null) return null;
  if (record.mode === "local") return { mode: "local" };
  if (record.mode === "worktree") {
    return typeof record.baseRef === "string" && record.baseRef.trim() !== ""
      ? { mode: "worktree", baseRef: record.baseRef } : { mode: "worktree" };
  }
  return null;
}

export function parseThreadCardMemory(raw: string | null | undefined): ThreadCardMemory {
  if (raw === null || raw === undefined || raw === "") return EMPTY_THREAD_CARD_MEMORY;
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { return EMPTY_THREAD_CARD_MEMORY; }
  const record = asRecord(parsed);
  if (record === null || record.version !== 1) return EMPTY_THREAD_CARD_MEMORY;
  const workspaces: Record<string, RememberedThreadWorkspace> = {};
  for (const [root, value] of Object.entries(asRecord(record.workspaces) ?? {})) {
    const choice = parseChoice(value);
    if (choice !== null && root.trim() !== "") workspaces[root] = choice;
  }
  const lastProject = record.lastProject === null ? null
    : typeof record.lastProject === "string" && record.lastProject.trim() !== ""
      ? record.lastProject : undefined;
  return lastProject === undefined ? { workspaces } : { lastProject, workspaces };
}

export function serializeThreadCardMemory(memory: ThreadCardMemory): string {
  return JSON.stringify({ version: 1, lastProject: memory.lastProject,
    workspaces: memory.workspaces });
}

/**
 * Remember `choice` for `root` as the newest entry, keeping the map bounded.
 * Only Local and New worktree become a project's default: an existing
 * worktree (Edit from the first message seeds one) is a one-off, so the
 * project keeps the mode it had.
 */
export function rememberThreadWorkspace(memory: ThreadCardMemory, root: string,
    choice: ThreadWorkspaceChoice): ThreadCardMemory {
  if (choice.mode === "existing") return memory;
  const rest = Object.entries(memory.workspaces).filter(([key]) => key !== root)
    .slice(0, MAX_REMEMBERED_PROJECTS - 1);
  return { ...memory, workspaces: Object.fromEntries([[root, choice], ...rest]) };
}

export function rememberThreadProject(memory: ThreadCardMemory,
    project: string | null): ThreadCardMemory {
  return memory.lastProject === project ? memory : { ...memory, lastProject: project };
}

/** What Rust listed for a project, as far as it is known yet. */
export interface ThreadProjectLists {
  worktrees?: readonly WorktreeEntry[] | null;
  branches?: BranchList | null;
  /** Rust's reason the project has no worktrees (not a git repository), or null. */
  unavailable?: string | null;
}

/**
 * `choice`, still valid against what Rust listed: an existing worktree that
 * is gone falls back to a new worktree, so work meant to be isolated never
 * lands in the project's own checkout; any worktree in a folder that is not
 * a git repository falls back to Local (there is no other); a base branch
 * that no longer exists falls back to the current branch. Unknown lists
 * change nothing.
 */
export function checkedThreadWorkspace(choice: ThreadWorkspaceChoice,
    known: ThreadProjectLists = {}): ThreadWorkspaceChoice {
  if (choice.mode !== "local" && known.unavailable !== undefined && known.unavailable !== null) {
    return LOCAL_WORKSPACE;
  }
  if (choice.mode === "existing" && known.worktrees !== undefined && known.worktrees !== null
      && !known.worktrees.some(entry => entry.path === choice.worktreePath)) {
    return { mode: "worktree" };
  }
  if (choice.mode === "worktree" && choice.baseRef !== undefined
      && known.branches !== undefined && known.branches !== null
      && !known.branches.branches.includes(choice.baseRef)) {
    return { mode: "worktree" };
  }
  return choice;
}

/** The workspace a project opens on: what was used there last, still valid; Local otherwise. */
export function rememberedThreadWorkspace(memory: ThreadCardMemory, root: string,
    known: ThreadProjectLists = {}): ThreadWorkspaceChoice {
  const choice = memory.workspaces[root];
  return choice === undefined ? LOCAL_WORKSPACE : checkedThreadWorkspace(choice, known);
}

/** The project the card opens on: an explicit one, else the last used. */
export function initialThreadProject(memory: ThreadCardMemory,
    preferred?: string | null, recent?: string): string | null {
  if (preferred !== undefined) return preferred;
  if (memory.lastProject === undefined) return recent ?? null;
  return memory.lastProject;
}

/**
 * The project of the thread used last (visited, else created), for a card
 * that has never launched anything: its threads came from elsewhere (a
 * fork, a split, the CLI), and "No project" is the one answer that is
 * certainly not what the owner was working on.
 */
export function recentThreadProject(threads: readonly Pick<ThreadView,
    "createdAt" | "lastVisitedAt" | "workspace">[]): string | undefined {
  let best: { at: number; root: string } | undefined;
  for (const thread of threads) {
    const workspace = thread.workspace;
    if (workspace === undefined || workspace.mode === "none") continue;
    const root = workspace.projectRoot;
    if (root.trim() === "") continue;
    const at = Math.max(thread.createdAt, thread.lastVisitedAt ?? 0);
    if (best === undefined || at > best.at) best = { at, root };
  }
  return best?.root;
}

/** A project (and maybe a workspace) the next card opening is asked to start on. */
export interface ThreadCardSeed {
  project: string | null;
  /** Absent: the workspace last used in that project. */
  workspace?: ThreadWorkspaceChoice;
  /** A status line the card shows while it is open (Edit from the first message). */
  note?: string;
}

/**
 * Where a fresh card opens. A seed wins (Edit from turn 1, a pane's split +,
 * a project's own +); else the inbox scope's project (null is No project;
 * undefined, All projects, says nothing); else the last project launched
 * from. The workspace is the seed's, else the one last used in that project,
 * checked against what Rust listed for it.
 */
export function threadCardOpening(memory: ThreadCardMemory, seed: ThreadCardSeed | null,
    scopeProject: string | null | undefined,
    known: (root: string) => ThreadProjectLists = () => ({}),
    recent?: string): ThreadCardSeed & {
      workspace: ThreadWorkspaceChoice;
    } {
  const project = seed !== null ? seed.project : initialThreadProject(memory, scopeProject, recent);
  const workspace = seed?.workspace ?? (project === null ? LOCAL_WORKSPACE
    : rememberedThreadWorkspace(memory, project, known(project)));
  return seed?.note === undefined ? { project, workspace } : { project, workspace, note: seed.note };
}

/* ---- the launch sequence ---------------------------------------------------- */

export interface ThreadLaunchPort {
  captureContext?(items: readonly PromptContextItem[]): () => void;
  request(command: ThreadCommand): Promise<ThreadControlResponse>;
  /**
   * The app's existing launch path (context reauthorization, echo, permission
   * handling). Resolves with the new session's key and the prompt the
   * provider was given, or null when the launch failed — the app has already
   * said why. `background` starts it without switching to it. `extra` is
   * another model's thread, started after the first one while the owner has
   * moved on: it takes no launch lock, touches nothing on the card, and
   * rejects with the reason instead of showing it.
   */
  launch(form: ComposerLaunchForm, scope: ScopeOptions,
    context: readonly PromptContextItem[], options: { background: boolean; extra?: boolean }):
    Promise<{ key: string; prompt: string } | null>;
  /** A problem after the session started; nothing is undone for it. */
  warn?(message: string): void;
}

export interface ThreadLaunchPlan {
  project: string | null;
  workspace: ThreadWorkspaceChoice;
  /** One form per model, the first the primary. Several only for a new worktree each. */
  forms: readonly ComposerLaunchForm[];
  scope: ScopeOptions;
  context: readonly PromptContextItem[];
  /** Start without switching to it (⌘Enter). Extra models always start that way. */
  background: boolean;
}

/** One extra model whose thread did not start, and why. */
export interface ThreadExtraFailure { model: string; message: string }

/** What became of the extra models once each was tried. */
export interface ThreadExtrasResult { keys: string[]; failures: ThreadExtraFailure[] }

export type ThreadLaunchOutcome =
  /**
   * The first model's thread started. With several models, `extras`
   * settles once every other model's thread has started or failed; it never
   * rejects.
   */
  | { status: "launched"; key: string; extras: Promise<ThreadExtrasResult> }
  /** Rust refused the workspace; nothing was started. */
  | { status: "prepare-failed"; message: string }
  /** The launch failed after its workspace was prepared; the workspace was discarded. */
  | { status: "launch-failed" };

function prepareCommand(project: string, workspace: ThreadWorkspaceChoice,
    hint: string): ThreadCommand {
  const mode: PrepareWorkspaceMode = workspace.mode;
  return {
    kind: "prepareWorkspace", projectRoot: project, mode, hint,
    ...(workspace.mode === "worktree" && workspace.baseRef !== undefined
      ? { baseRef: workspace.baseRef } : {}),
    ...(workspace.mode === "existing" ? { worktreePath: workspace.worktreePath } : {}),
  };
}

type ThreadStart =
  | { status: "started"; key: string }
  | { status: "prepare-failed"; message: string }
  | { status: "launch-failed"; message: string };

/** Prepare → launch → bind for one model. Never rejects. */
async function startThread(port: ThreadLaunchPort, plan: ThreadLaunchPlan, form: ComposerLaunchForm,
    options: { background: boolean; extra: boolean }): Promise<ThreadStart> {
  const bind = async (command: Extract<ThreadCommand, { kind: "bind" }>): Promise<void> => {
    try {
      await port.request(command);
    } catch (err: unknown) {
      port.warn?.(`The thread started, but its record was not saved: ${ipcMessage(err)}`);
    }
  };
  // A launch the app answered null for has already said why on the card;
  // an extra one rejects with its reason, which is kept for the report.
  const launchAs = async (launchForm: ComposerLaunchForm):
      Promise<{ key: string; prompt: string } | string> => {
    try {
      return await port.launch(launchForm, plan.scope, plan.context, options)
        ?? "The session did not start.";
    } catch (err: unknown) {
      return ipcMessage(err);
    }
  };
  if (plan.project === null) {
    const launched = await launchAs({ ...form, kind: "chat" });
    if (typeof launched === "string") return { status: "launch-failed", message: launched };
    await bind({ kind: "bind", key: launched.key, prompt: launched.prompt });
    return { status: "started", key: launched.key };
  }
  let prepared;
  try {
    const answer = await port.request(prepareCommand(plan.project, plan.workspace, form.prompt));
    prepared = answer.prepared;
    if (answer.setup !== undefined && answer.setup.state !== "ready") {
      return { status: "prepare-failed", message: answer.setup.message };
    }
  } catch (err: unknown) {
    return { status: "prepare-failed", message: ipcMessage(err) };
  }
  if (prepared === undefined) {
    return { status: "prepare-failed", message: "The workspace could not be prepared." };
  }
  const launched = await launchAs({ ...form, kind: "code", cwd: prepared.cwd, worktree: false });
  if (typeof launched === "string") {
    await port.request({ kind: "discardWorkspace", token: prepared.token }).catch(() => undefined);
    return { status: "launch-failed", message: launched };
  }
  await bind({ kind: "bind", key: launched.key, token: prepared.token,
    prompt: launched.prompt, titleSource: "prompt" });
  return { status: "started", key: launched.key };
}

/** The extra models, one after another; a failure is noted and the next is tried. */
async function startExtras(port: ThreadLaunchPort, plan: ThreadLaunchPlan,
    forms: readonly ComposerLaunchForm[]): Promise<ThreadExtrasResult> {
  const result: ThreadExtrasResult = { keys: [], failures: [] };
  for (const form of forms) {
    const started = await startThread(port, plan, form, { background: true, extra: true });
    if (started.status === "started") result.keys.push(started.key);
    else result.failures.push({ model: form.model.trim() || form.engine, message: started.message });
  }
  return result;
}

/**
 * Prepare → launch → bind, once per model. A refused `prepareWorkspace`
 * starts nothing and hands back Rust's sentence for the card (the draft
 * stays). A launch that fails after a workspace was prepared sends
 * `discardWorkspace` so a just-made worktree does not linger. A `bind` that
 * fails is reported but undoes nothing: the session is already running, and
 * Rust seeds a record for it from the registry either way.
 *
 * The answer comes once the first model's thread has started: the other
 * models (several only with a new worktree each) then start in the
 * background, without the launch lock, and `extras` says which did not.
 *
 * Plain chat has no workspace: launch → `bind {key, prompt}`.
 */
export async function launchThreadCard(port: ThreadLaunchPort,
    plan: ThreadLaunchPlan): Promise<ThreadLaunchOutcome> {
  const [primary, ...others] = plan.forms;
  if (primary === undefined) return { status: "launch-failed" };
  const releaseContext = port.captureContext?.(plan.context) ?? (() => {});
  let handedToExtras = false;
  try {
    const first = await startThread(port, plan, primary, { background: plan.background, extra: false });
    if (first.status === "prepare-failed") return { status: "prepare-failed", message: first.message };
    if (first.status === "launch-failed") return { status: "launch-failed" };
    const extras = threadCardAllowsSeveralModels(plan.project, plan.workspace) ? others : [];
    const pendingExtras = startExtras(port, plan, extras).finally(releaseContext);
    handedToExtras = true;
    return { status: "launched", key: first.key, extras: pendingExtras };
  } finally { if (!handedToExtras) releaseContext(); }
}

/** How much of the prompt a fan-out thread's name keeps before its model. */
export const THREAD_FAN_OUT_PROMPT_CHARS = 60;

/**
 * The name of one thread of a multi-model fan-out: the prompt's start, then
 * the model, so the threads that share a prompt read apart in the inbox.
 */
export function threadFanOutName(prompt: string, model: string): string {
  const head = [...prompt.replace(/\s+/gu, " ").trim()].slice(0, THREAD_FAN_OUT_PROMPT_CHARS)
    .join("").trimEnd();
  return `${head} · ${model}`;
}

/**
 * A fan-out's forms, each named for its model (`label`, e.g. the catalog's
 * display name). One form is not a fan-out, and its prompt stays its title.
 */
export function threadFanOutForms(forms: readonly ComposerLaunchForm[],
    label: (form: ComposerLaunchForm) => string): ComposerLaunchForm[] {
  if (forms.length < 2) return [...forms];
  return forms.map(form => ({ ...form, name: threadFanOutName(form.prompt, label(form)) }));
}

/** The action error for extra models that did not start, or null when all did. */
export function threadExtrasFailureText(failures: readonly ThreadExtraFailure[]): string | null {
  if (failures.length === 0) return null;
  if (failures.length === 1) {
    return `The ${failures[0].model} thread did not start: ${failures[0].message}`;
  }
  return `${failures.length} threads did not start. `
    + failures.map(failure => `${failure.model}: ${failure.message}`).join(" · ");
}

/**
 * The card's account (protocol 60) on each form it starts: the forms on the
 * chosen provider run on `account`; any other provider's form (a Shift-added
 * model) on its own default. `account` null = no account control was shown,
 * and every form keeps its provider's default.
 */
export function threadFormsOnAccount(forms: readonly ComposerLaunchForm[], engine: string,
    account: string | null): ComposerLaunchForm[] {
  return forms.map(form => {
    const { account: _carried, ...rest } = form;
    return account !== null && form.engine === engine ? { ...rest, account } : rest;
  });
}
