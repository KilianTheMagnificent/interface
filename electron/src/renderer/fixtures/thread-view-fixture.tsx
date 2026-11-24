import { KeptPromptLine, PendingLaunchRecoveryPanel, PromptRecoveryPanel } from "../components/PromptRecoveryPanel";
import type { PromptRecoveryReceipt } from "../../shared/recovery";
import { flushSync } from "react-dom";
import { useState } from "react";
import type { Root } from "react-dom/client";
import type { SessionSummary } from "../../shared/engine";
import type { ThreadDiff, ThreadTurn, ThreadsView } from "../../shared/threads";
import threadsFixture from "../../fixtures/threads-v34.json";
import { AgentTile } from "../components/AgentTile";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { Composer, type ComposerThreadCard } from "../components/Composer";
import { Conversation, type ConversationProps } from "../components/Conversation";
import { ThreadDiffPanel, type ThreadDiffLoad } from "../components/ThreadDiffPanel";
import { KEEP_FILES_LABEL, RESTORE_FILES_LABEL } from "../components/ThreadEditDialog";
import { ThreadDiffToggle, ThreadPaneHead, WORKTREE_WHERE_WORDS } from "../components/ThreadPaneHead";
import { SettingsPage } from "../components/SettingsPage";
import { DEFAULT_APP_PREFERENCES } from "../settings";
import type { ThreadSettings } from "../../shared/threads";
import type { ComposerLaunchForm, EngineChoices } from "../composer-model";
import type { ConvItem } from "../conversation-reducer";
import type { ThreadWorkspaceChoice } from "../thread-card";
import {
  appendDiffQuote, formatDiffQuote, SPLIT_DIFF_MIN_WIDTH, THREAD_DIFF_LAYOUT_KEY, type ThreadDiffChoice,
  type ThreadDiffQuote,
} from "../thread-diff";
import { conversationThread } from "../thread-pane";
import { threadPromptDigest } from "../thread-turns";
import { focusThreadDiffToggle, threadDiffToggleOf } from "../use-thread-pane";
import type { ConversationThread } from "../conversation-types";
import { assert, frames, noop, until, wait } from "./fixture-support";
import { threadReadFixture } from "./thread-read-fixture";

/**
 * The Threads view in a real Electron DOM: the New thread card in each mode
 * (No project, a project on Local, a project with a new worktree and a second
 * model Shift-chosen beside the first), and a worktree thread's pane in the
 * Threads layout — its branch chip, lineage line, folded work, changes cards,
 * Edit/Fork controls and the diff panel open beside it. Rust is a fake here;
 * the fixture checks what the renderer draws and the commands its controls send.
 */

const view = threadsFixture.view as unknown as ThreadsView;
const DIFF = threadsFixture.response.diff as unknown as ThreadDiff;
const ROOT = "/Users/owner/Developer/Projects/atlas";

const CLAUDE: EngineChoices = {
  models: ["claude-opus-5", "claude-sonnet-5"],
  modelOptions: [
    { value: "claude-opus-5", displayName: "Opus 5", supportsEffort: true, effortLevels: ["low", "high"] },
    { value: "claude-sonnet-5", displayName: "Sonnet 5", supportsEffort: true, effortLevels: ["low", "high"] },
  ],
  efforts: ["low", "high"], permissionModes: ["default", "acceptEdits", "auto", "bypassPermissions"],
  chat: true, settingSources: ["user", "project", "local"], toolDenyList: true,
};
const CODEX: EngineChoices = {
  models: ["gpt-6"],
  modelOptions: [{ value: "gpt-6", displayName: "GPT-6", supportsEffort: false, effortLevels: [] }],
  efforts: [], permissionModes: ["untrusted", "on-request", "never"],
};
const choicesFor = (engine: string): EngineChoices => engine === "codex" ? CODEX : CLAUDE;

function setLook(theme: string, chrome: string): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
}

/** A colour expression as `scope` resolves it, to compare with a painted colour. */
function probeColour(scope: HTMLElement, expression: string): string {
  const node = document.createElement("span");
  node.style.color = expression;
  scope.append(node);
  const colour = getComputedStyle(node).color;
  node.remove();
  return colour;
}

function button(label: string, scope: ParentNode = document): HTMLButtonElement {
  const found = [...scope.querySelectorAll<HTMLButtonElement>("button")].find(item =>
    item.textContent?.trim() === label || item.getAttribute("aria-label") === label);
  assert(found, `Missing button ${label}`);
  return found;
}

export type ThreadCardMode = "none" | "local" | "worktree";

function options(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('[role="option"]')];
}

/**
 * The New thread card in one mode. Local also walks the workspace drawer's
 * segments, "or chat without a project" and back through the headline's
 * picker; worktree starts two models and a ⌘Enter thread.
 */
export async function threadCardFixture(root: Root, theme: string, chrome: string,
    mode: ThreadCardMode): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  setLook(theme, chrome);
  const launches: { forms: readonly ComposerLaunchForm[]; background: boolean }[] = [];
  const workspaceChanges: ThreadWorkspaceChoice[] = [];
  function Harness() {
    const [project, setProject] = useState<string | null>(mode === "none" ? null : ROOT);
    const [workspace, setWorkspace] = useState<ThreadWorkspaceChoice>(mode === "worktree"
      ? { mode: "worktree", baseRef: "main" } : { mode: "local" });
    const thread: ComposerThreadCard = {
      projects: [{ root: ROOT, label: "atlas" }, { root: "/Users/owner/Developer/Projects/beacon", label: "beacon" }],
      project, onProjectChange: setProject, onChooseFolder: () => Promise.resolve(null),
      workspace,
      onWorkspaceChange: next => { workspaceChanges.push(next); setWorkspace(next); },
      branches: threadsFixture.response.branches,
      worktrees: [{ path: ROOT }, ...threadsFixture.response.worktrees],
      worktreeUnavailable: null, error: null, busy: false,
      onLaunch: (forms, _scope, _context, background) => {
        launches.push({ forms, background });
        return Promise.resolve(true);
      },
    };
    return (
      <div className="workspace-composer-panel" style={{ height: "100vh" }}>
        <style>{APP_SELECT_CSS}</style>
        <Composer open kind="chat" thread={thread}
          defaults={{ engine: "claude", cwd: "", prompt: mode === "none" ? "" : "Fix the flaky login test",
            model: "claude-opus-5", effort: "high", permissionMode: "default" }}
          engines={[{ id: "claude" }, { id: "codex" }]} choicesFor={choicesFor}
          onLaunch={noop} onBrowse={async () => null} onClose={noop} />
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  // The lane's first capture can precede the window's first painted frame.
  await document.fonts.ready;
  await frames(6);
  const card = document.querySelector<HTMLElement>(".composer-card.composer-thread-card");
  assert(card, "The New thread card did not mount");
  const prompt = card.querySelector<HTMLTextAreaElement>(".composer-ta")!;
  assert(document.activeElement === prompt, "Opening the card did not put the caret in the prompt");
  assert(prompt.rows === 3, `The prompt has ${prompt.rows} rows`);
  // One column, at most 700px wide, centred across and down the panel.
  const panel = card.getBoundingClientRect();
  const box = card.querySelector<HTMLElement>(".composer-box")!.getBoundingClientRect();
  assert(box.width <= 700, `The prompt card is ${box.width}px wide`);
  assert(Math.abs((box.left - panel.left) - (panel.right - box.right)) <= 2, "The prompt card is not centred");
  const headline = card.querySelector<HTMLElement>(".composer-thread-headline");
  assert(headline, "The card has no headline");
  // The Code mode look (owner, 2026-10-23): the question beside the still
  // Pair, then the chips with the project picker, then the prompt.
  const mark = headline.querySelector<SVGElement>(".thinking-mark.is-still");
  assert(mark !== null && mark.getBoundingClientRect().width === 36, "The greeting has no still thinking mark");
  const chipsRow = card.querySelector<HTMLElement>(".composer-thread-chips");
  assert(chipsRow !== null && chipsRow.querySelector(".composer-thread-project .app-select-trigger") !== null,
    "The project picker is not in the chips under the question");
  assert(headline.getBoundingClientRect().bottom <= chipsRow.getBoundingClientRect().top
    && chipsRow.getBoundingClientRect().bottom <= box.top, "The chips are not between the question and the prompt");
  const top = headline.getBoundingClientRect().top - panel.top;
  assert(top > panel.height / 6, `The column sits at the top of the panel (${Math.round(top)}px)`);
  // The footer: "+" and access on the left, the model chips and Start on the right.
  const footer = card.querySelector<HTMLElement>(".composer-control-secondary");
  const model = card.querySelector<HTMLElement>(".composer-model-picker .app-select-value");
  assert(model?.textContent === "Opus 5", `The model chip reads ${model?.textContent}`);
  assert(footer?.contains(model), "The model chip left the footer's right end");
  const drawer = card.querySelector<HTMLElement>(".composer-thread-drawer");
  const access = card.querySelector(".composer-access-picker");
  // The project keeps its hue only as its picker's square; No project's is a ring.
  const square = chipsRow.querySelector<HTMLElement>(".composer-thread-project .composer-thread-hue");
  assert(square !== null && square.getBoundingClientRect().width === 8, "The project picker has no hue square");
  const hue = [...square.classList].find(name => name.startsWith("project-hue-"));
  const aurora = card.querySelector<HTMLElement>(".composer-thread-aurora");
  const start = card.querySelector<HTMLElement>(".composer-submit")!;
  const scroller = card.querySelector<HTMLElement>(".composer-body")!;
  assert(scroller.scrollWidth <= scroller.clientWidth + 1, "The card's column scrolls sideways");
  // The prompt is the Threads composer's card; Start the accent square.
  assert(getComputedStyle(start).backgroundImage === "none"
    && getComputedStyle(card.querySelector(".composer-box")!).backgroundImage === "none",
  "The card still draws a gradient");
  assert(start.getBoundingClientRect().width === 32 && start.getBoundingClientRect().height === 32,
    "Start is not a 32px square");
  const report: Record<string, unknown> = {
    mode, headline: headline.textContent, cardWidth: Math.round(box.width), hue,
  };
  if (mode === "none") {
    assert(hue === "project-hue-none" && aurora === null, `No project wears ${hue} with an aurora`);
    assert(headline.textContent?.includes("What’s on your mind?"), "No project does not ask an open question");
    assert(drawer === null && access === null, "No project still shows folder controls");
    assert(card.textContent?.includes("Start chat"), "No project is not the chat card");
    assert(!card.textContent?.includes("or chat without a project"), "No project offers chat without a project");
    assert(prompt.placeholder === "Ask anything.", `No project's placeholder is ${prompt.placeholder}`);
    return report;
  }
  assert(headline.textContent?.includes("What should we build in"), "A project card does not ask what to build");
  assert(hue !== undefined && hue !== "project-hue-none", `A project's square wears ${hue}`);
  assert(aurora === null && !card.className.includes("project-hue-"), "A project card still wears its hue");
  assert(drawer !== null && access !== null, "A project card lacks its workspace or access mode");
  // The workspace sits in the chips beside the project, above the prompt.
  const workspace = drawer.getBoundingClientRect();
  assert(chipsRow.contains(drawer) && workspace.bottom <= box.top,
    "The workspace is not in the chips above the prompt");
  if (mode === "local") {
    assert(card.querySelector(".composer-branch-picker") === null, "Local shows a base branch picker");
    assert(button("Local checkout", drawer).getAttribute("aria-pressed") === "true", "Local checkout is not pressed");
    assert(drawer.textContent?.includes("Edits land in atlas itself."), "Local does not say where edits land");
    assert(drawer.querySelector(".composer-thread-branch-name")?.textContent === "main",
      "Local does not name the checked-out branch");
    report.drawer = drawer.textContent;
    // The segments drive the workspace choice; an existing worktree is a segment away.
    button("New worktree", card).click();
    await frames(2);
    assert(card.querySelector(".composer-branch-picker"), "New worktree offers no base branch");
    button("Local checkout", card).click();
    await frames(2);
    card.querySelector<HTMLButtonElement>(".composer-thread-existing .app-select-trigger")!.click();
    await frames();
    const hotfix = options().find(option => option.textContent?.includes("atlas-hotfix"));
    assert(hotfix, "The existing-worktree menu has no atlas-hotfix row");
    hotfix.click();
    await frames(2);
    assert(card.querySelector(".composer-thread-existing.is-on"), "The chosen existing worktree is not shown");
    button("Local checkout", card).click();
    await frames(2);
    const modes = workspaceChanges.map(change => change.mode).join(",");
    assert(modes === "worktree,local,existing,local", `The drawer sent ${modes}`);
    // "or chat without a project" is No project, with the caret back in the prompt…
    button("or chat without a project", card).click();
    await frames(3);
    assert(card.querySelector(".composer-thread-drawer") === null, "No project kept the drawer");
    assert(card.querySelector(".composer-thread-headline")?.textContent?.includes("What’s on your mind?"),
      "The link did not switch the card to No project");
    assert(document.activeElement === prompt, "The link left the caret off the prompt");
    // …and the headline's picker, its rows unchanged, brings the project back.
    card.querySelector<HTMLButtonElement>(".composer-thread-project .app-select-trigger")!.click();
    await frames();
    const rows = options().map(option => option.textContent ?? "");
    assert(["atlas", "beacon", "Choose folder…", "No project"].every(label =>
      rows.some(row => row.includes(label))), `The project picker's rows changed: ${rows.join(" | ")}`);
    options().find(option => option.textContent?.includes("atlas"))!.click();
    await frames(3);
    assert(card.querySelector(".composer-thread-drawer"), "Choosing atlas again did not bring the drawer back");
    prompt.focus({ focusVisible: false } as FocusOptions);
    await frames(2);
    report.workspaceChanges = workspaceChanges.map(change => change.mode);
    return report;
  }
  assert(card.querySelector(".composer-branch-picker"), "New worktree has no base branch picker");
  assert(button("New worktree", drawer).getAttribute("aria-pressed") === "true", "New worktree is not pressed");
  assert(drawer.textContent?.includes("A new branch in its own folder."), "New worktree does not explain itself");
  // Shift-click a second model: one thread per model, each in its own worktree.
  card.querySelector<HTMLButtonElement>(".composer-model-picker .app-select-trigger")!.click();
  await frames();
  const sonnet = options().find(option => option.textContent?.includes("GPT-6"));
  assert(sonnet, "The model menu has no GPT-6 row");
  sonnet.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
  await frames();
  assert(document.querySelector('[role="listbox"][aria-multiselectable="true"]'),
    "The model menu did not stay open as a multi-select");
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
  await frames();
  const extra = card.querySelector(".composer-thread-models");
  assert(extra !== null && extra.textContent?.includes("+1 model"), "The second model is not shown on the card");
  report.extraModels = extra.textContent;
  // ⌘Enter: background start, a fresh card on the same project and workspace.
  prompt.focus();
  prompt.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
  await frames(3);
  assert(launches.length === 1, `⌘Enter launched ${launches.length} times`);
  assert(launches[0].background, "⌘Enter did not start in the background");
  assert(launches[0].forms.length === 2, `Expected two models, got ${launches[0].forms.length}`);
  assert(launches[0].forms.map(form => form.engine).join(",") === "claude,codex",
    "The two forms are not one per model");
  assert(prompt.value === "", "The card kept the prompt after a background start");
  assert(card.querySelector(".composer-branch-picker"), "The fresh card lost its workspace mode");
  report.launch = { forms: launches[0].forms.length, background: launches[0].background };
  report.workspaceChanges = workspaceChanges.length;
  // Leave the worktree card on screen with its two models for the capture.
  card.querySelector<HTMLButtonElement>(".composer-model-picker .app-select-trigger")!.click();
  await frames();
  options().find(option => option.textContent?.includes("GPT-6"))!
    .dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
  await frames(2);
  return report;
}

export function clearThreadFixture(root: Root): void {
  flushSync(() => root.render(null));
}

const SENT = Date.parse("2026-10-06T14:15:00Z");
const turn = (ordinal: number, prompt: string, over: Partial<ThreadTurn> = {}): ThreadTurn => ({
  ordinal, sentAt: SENT + ordinal * 60_000, promptDigest: threadPromptDigest(prompt), hasStart: true,
  hasEnd: true, state: "done", ...over,
});

/**
 * Retry on a failed turn: drawn once, on the newest turn Rust recorded failed,
 * while the pane may send; one send per click with the prompt's own words,
 * held while it is on its way, offered again after a definite refusal, and
 * gone while the agent works. Both places it stands: under the prompt when the
 * turn wrote nothing, and in the turn's controls when it left an error.
 */
async function threadRetryChecks(root: Root): Promise<void> {
  const base = view.threads.find(thread => thread.workspace?.mode !== "worktree") ?? view.threads[0];
  const failedPrompt = "Run the migration on the staging copy";
  const thread = { ...base, turns: [
    turn(0, "Fix the flaky login test"),
    turn(1, failedPrompt, { state: "failed", hasEnd: false }),
  ] };
  const earlier: ConvItem[] = [
    { kind: "user", id: "retry-user-0", text: "Fix the flaky login test" },
    { kind: "text", id: "retry-text-1", text: "Fixed: the retry now backs off.", streaming: false },
    { kind: "user", id: "retry-user-2", text: failedPrompt },
  ];
  const sends: string[] = [];
  let settle: (refused: boolean) => void = noop;
  let setThinking: (value: boolean) => void = noop;
  function Harness({ items }: { items: ConvItem[] }) {
    const [thinking, setThinkingState] = useState(false);
    setThinking = setThinkingState;
    const props: ConversationProps = {
      items, live: true, thinking, focused: true, sessionKey: thread.key,
      config: { effort: null, models: [], canSetModel: false, effortLevels: [], showEffort: false,
        canSetEffort: false, pendingModel: false, pendingEffort: false, error: null },
      onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop,
      onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: new Set(),
      threadLayout: true,
      thread: conversationThread(thread, false),
      onThreadRecover: prompt => {
        sends.push(prompt);
        return new Promise<void>((resolve, reject) => {
          settle = refused => { if (refused) reject(new Error("The engine refused it.")); else resolve(); };
        });
      },
    };
    return <div className="tile-stack" style={{ width: 760, height: 640 }}><Conversation {...props} /></div>;
  }
  const retries = (): HTMLButtonElement[] => [...document.querySelectorAll<HTMLButtonElement>("button.thread-retry")];

  // The turn wrote nothing: Retry stands under its prompt.
  flushSync(() => root.render(<Harness items={earlier} />));
  await frames(3);
  assert(retries().length === 1, `Retry is drawn ${retries().length} times on a failed newest turn`);
  assert(retries()[0].closest(".thread-turn-foot") !== null, "Retry is not among the turn's controls");
  retries()[0].click();
  retries()[0]?.click();
  await frames(2);
  assert(sends.length === 1 && sends[0] === failedPrompt,
    `Retry sent ${sends.length} times: ${JSON.stringify(sends)}`);
  assert(retries().length === 1 && retries()[0].disabled && retries()[0].textContent?.includes("Continuing…") === true,
    "Retry is not held while its send is on its way");
  const prompts = [...document.querySelectorAll(".thread-user")].filter(node =>
    node.textContent?.includes(failedPrompt));
  assert(prompts.length === 1, `Recovery replayed the original prompt as an optimistic echo (${prompts.length} rows)`);
  settle(true);
  await frames(3);
  assert(retries().length === 1 && !retries()[0].disabled && retries()[0].textContent?.trim() === ""
    && retries()[0].getAttribute("aria-label")?.startsWith("Continue") === true,
  "A refused retry did not offer Retry again");
  flushSync(() => { setThinking(true); });
  await frames(2);
  assert(retries().length === 0, "Retry is offered while the agent works");
  flushSync(() => root.render(null));

  // The turn left an error: Retry joins the controls under it.
  flushSync(() => root.render(<Harness items={[...earlier,
    { kind: "notice", id: "retry-error", text: "The provider stopped: overloaded.", tone: "error" }]} />));
  await frames(3);
  assert(retries().length === 1, `Retry is drawn ${retries().length} times under the failed turn's error`);
  assert(retries()[0].closest(".thread-answer.is-latest") !== null, "Retry is not in the latest turn's controls");
  flushSync(() => root.render(null));
}

/** A worktree thread's pane: head, turn controls, the edit dialog and the diff panel. */
export async function threadPaneFixture(root: Root, theme: string, chrome: string): Promise<Record<string, unknown>> {
  const reads = await threadReadFixture(root, view,
    threadsFixture.response as unknown as import("../../shared/threads").ThreadControlResponse);
  flushSync(() => root.render(null));
  setLook(theme, chrome);
  await threadRetryChecks(root);
  await promptRecoveryChecks(root);
  const base = view.threads.find(thread => thread.workspace?.mode === "worktree")!;
  const thread = { ...base, lineage: { parent: "claude#parent", relation: "fork" as const, keptTurns: 1, at: 1 },
    turns: [
      turn(0, "Fix the flaky login test", { filesChanged: 3, added: 41, removed: 12 }),
      turn(1, "Now add a regression test for the retry path", { filesChanged: 1, added: 18, removed: 0 }),
    ],
    // The second turn's run, as Rust stamps it: it started when the turn was sent.
    lastRun: { startedAt: SENT + 60_000, endedAt: SENT + 101_000, outcome: "completed" as const } };
  const retry = `${base.workspace!.worktreePath!}/src/auth/retry.ts`;
  const items: ConvItem[] = [
    { kind: "user", id: "user-0", text: "Fix the flaky login test" },
    { kind: "text", id: "text-1", text: "The retry gives up after one attempt; I'll back it off.", streaming: false },
    { kind: "tool", id: "tool-read", tool: "Read", summary: retry, input: { file_path: retry },
      result: { ok: true, summary: "Read 40 lines" } },
    { kind: "tool", id: "tool-1", tool: "Edit", summary: retry, input: { file_path: retry },
      result: { ok: true, summary: "Edited" } },
    { kind: "diff", id: "diff-1", path: retry, callId: "tool-1", added: 8, removed: 1 },
    { kind: "tool", id: "tool-test", tool: "Bash", summary: "npm test -- login", input: { command: "npm test -- login" },
      result: { ok: false, summary: "login › retries: expected 5 attempts, saw 1" } },
    { kind: "text", id: "text-2", text: "The retry now backs off five times before failing.", streaming: false },
    { kind: "user", id: "user-3", text: "Now add a regression test for the retry path" },
    { kind: "text", id: "text-4", text: "Added `test/login.test.ts`.", streaming: false },
  ];
  const session: SessionSummary = {
    engine: "codex", sessionId: "019a3c5e-7d21-7c30-9f4e-2b6d8a1c4e90", title: "Fix the flaky login test",
    cwd: base.workspace!.worktreePath!, status: "idle", cloud: false,
    lastActivity: "2026-10-06T10:00:00Z", firstSeen: "2026-10-06T09:00:00Z", archived: false,
  };
  const diffs: string[] = [];
  // The renamed file carries two hunks and a long line, so the panel draws
  // its unchanged-line bands, numbers and wrapping as Rust would send them.
  const diff: ThreadDiff = { ...DIFF, files: [{ ...DIFF.files[0], patch: "@@ -12,5 +12,7 @@ export function retry\n"
    + " export async function retry(run: () => Promise<void>) {\n-  for (let i = 0; i < 3; i++) {\n"
    + "+  for (let attempt = 0; attempt < RETRIES; attempt++) {\n+    const wait = backoff(attempt);\n"
    + "+    await sleep(wait); // a long comment that has to wrap inside the narrow column rather than scroll sideways\n"
    + "     try { return await run(); } catch { continue; }\n   }\n }\n"
    + "@@ -40,3 +43,3 @@\n export const RETRY_LIMIT_MS = 10_000;\n-export const RETRIES = 3;\n+export const RETRIES = 5;\n"
    + " export default retry;\n" }, ...DIFF.files.slice(1)] };
  const quotes: ThreadDiffQuote[] = [];
  const undos: number[] = [];
  const edits: [number, boolean][] = [];
  const forks: number[] = [];
  const parentSelections: number[] = [];
  // What the pane tells the shell about its turns, for the diff panel's Undo.
  const rowReports: ReadonlyMap<number, string>[] = [];
  // App's pane lock, as the harness's: a fork holds the pane until released.
  const pane = { release: noop };
  function Harness() {
    const [choice, setChoice] = useState<ThreadDiffChoice>({ scope: "thread" });
    const [open, setOpen] = useState(true);
    const [busy, setBusy] = useState(false);
    pane.release = () => { setBusy(false); };
    const load: ThreadDiffLoad = { status: "ready", diff: { ...diff, scope: choice.scope } };
    const props: ConversationProps = {
      items, live: true, thinking: false, focused: true, sessionKey: thread.key,
      config: { effort: null, models: [], canSetModel: false, effortLevels: [], showEffort: false,
        canSetEffort: false, pendingModel: false, pendingEffort: false, error: null },
      onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop,
      onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: new Set(),
      pathRoot: session.cwd,
      threadLayout: true,
      thread: conversationThread(thread, busy),
      onThreadTurnDiff: ordinal => { diffs.push(String(ordinal)); setChoice({ scope: "turn", ordinal }); setOpen(true); },
      onThreadEditFrom: (ordinal, restoreFiles) => {
        edits.push([ordinal, restoreFiles]);
        return restoreFiles ? Promise.reject(new Error("Another session uses this worktree.")) : Promise.resolve();
      },
      onThreadForkAt: keep => { forks.push(keep); setBusy(true); },
      onThreadTurnRows: rowKeys => { rowReports.push(rowKeys); },
    };
    return (
      <div className={`app-grid no-sidebar${open ? " has-detail" : ""}`}>
        <style>{APP_SELECT_CSS}</style>
        <main className="workspace-shell" style={{ gridColumn: 2 }}>
          <div className="workspace-content">
            <div className="workspace-grid workspace-count-1">
              <AgentTile session={session} selected maximized={false} live engineLabel="Codex"
                projectLabel="atlas" projectRoot={thread.workspace?.projectRoot}
                onFocus={noop} onToggleMaximize={noop} onClose={noop}
                headDetail={<ThreadPaneHead workspace={thread.workspace} pullRequest={thread.pullRequest}
                  lineage={{ relation: "fork", title: "Plan the login retry",
                    onSelect: () => { parentSelections.push(1); } }} />}
                headActions={<ThreadDiffToggle open={open} onToggle={() => { setOpen(value => !value); }}
                  totals={{ added: 59, removed: 12 }} />}>
                <div className="tile-stack"><Conversation {...props} /></div>
              </AgentTile>
            </div>
          </div>
        </main>
        {open && <div className="detail-pane" style={{ gridColumn: 3 }}>
          <ThreadDiffPanel thread={thread} title={session.title} choice={choice} load={load}
            pathRoot={session.cwd} onChoose={setChoice} onRefresh={noop}
            onQuote={quote => { quotes.push(quote); }} turnUndo={ordinal => ({
              point: { label: "Undo · 3 files", description: "Restores 3 files." }, busy: false,
              onUndo: () => { undos.push(ordinal); } })}
            onClose={() => { setOpen(false); focusThreadDiffToggle(thread.key); }} />
        </div>}
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  await document.fonts.ready;
  await frames(3);
  // Each turn's prompt row, reported once: the diff panel's Undo reaches the
  // transcript's restore point through it.
  const reportedRows = rowReports.at(-1);
  assert(reportedRows !== undefined && [...reportedRows.keys()].join(",") === "0,1",
    `The pane reported turns ${reportedRows === undefined ? "never" : [...reportedRows.keys()].join(",")}`);
  const reportsAtMount = rowReports.length;
  const head = document.querySelector<HTMLElement>(".workspace-tile-head");
  assert(head !== null, "The pane head did not mount");
  // Owner, 2026-11-15: the title on its own line, then one plain line — the
  // project, "on <branch>", where edits land — in place of separate pills.
  const where = head.querySelector<HTMLElement>(".workspace-tile-where");
  assert(where !== null && where.textContent?.includes(`atlas·on ${base.workspace!.branch}·${WORKTREE_WHERE_WORDS}`),
    `The head's second line reads ${where?.textContent ?? "nothing"}`);
  assert(head.querySelector(".thread-head-place:not(.thread-head-account)") === null,
    "The head still draws the Local or Worktree chip");
  const headBox = head.getBoundingClientRect();
  assert(headBox.height >= 56 && headBox.height <= 80, `The pane head is ${headBox.height}px, not two lines`);
  const titleButton = head.querySelector<HTMLElement>(".workspace-tile-title")!;
  const titleBox = titleButton.getBoundingClientRect();
  const nameText = head.querySelector<HTMLElement>(".workspace-tile-name")!;
  assert(nameText.getBoundingClientRect().height < 30 && getComputedStyle(nameText).whiteSpace === "nowrap",
    "The title wraps");
  // The focus ring (the app's ring, offset 1px) fits inside the head, so
  // focus never draws the two clipped bars it did.
  assert(titleBox.top - headBox.top >= 4 && titleBox.bottom < where.getBoundingClientRect().top,
    `The title's ring has no room: title ${titleBox.top}-${titleBox.bottom}, head from ${headBox.top}`);
  titleButton.focus({ focusVisible: true } as FocusOptions);
  const ring = getComputedStyle(titleButton);
  assert(document.activeElement === titleButton && (!titleButton.matches(":focus-visible")
    || (ring.outlineStyle === "solid" && Number.parseFloat(ring.outlineWidth) >= 2)),
    `The focused title draws ${ring.outlineStyle} ${ring.outlineWidth}, not the app's ring`);
  titleButton.blur();
  // The pane's own commands sit behind one ⋯; the head has no access picker or provider mark.
  assert(head.querySelector(".thread-pane-menu-button") !== null, "The pane menu is missing");
  assert(head.querySelector(".provider-logo, .workspace-tile-access") === null,
    "The head still draws the provider mark or the access picker");
  assert(head.textContent?.includes("Forked from Plan the login retry"), "The lineage line is missing");
  // The pull request chip is a link the window's external-link handler opens.
  const pr = head.querySelector<HTMLAnchorElement>("a.thread-head-pr");
  assert(pr !== null && pr.textContent === `PR #${base.pullRequest!.number} · ${base.pullRequest!.state}`,
    `The pull request chip reads ${pr?.textContent ?? "nothing"}`);
  assert(pr.href === base.pullRequest!.url && pr.target === "_blank" && pr.rel.includes("noreferrer"),
    "The pull request chip is not a safe external link");
  assert(head.scrollWidth <= head.clientWidth + 1, "The pane head overflows");
  button("Forked from Plan the login retry", head).click();
  assert(parentSelections.length === 1, "The lineage line does not open the parent");
  const panel = document.querySelector<HTMLElement>(".tdiff");
  assert(panel, "The diff panel is not open");
  assert(panel.querySelectorAll(".tdiff-file").length === DIFF.files.length, "The diff panel lists the wrong files");
  assert(panel.textContent?.includes("Some files are listed without their changes"), "The truncated notice is missing");
  const grid = document.querySelector<HTMLElement>(".app-grid")!;
  assert(getComputedStyle(grid).gridTemplateColumns.split(" ").length === 3 || window.innerWidth <= 1080,
    "The diff panel is not the third column");
  // The Threads layout: the reading lane, the bare prompt bubble, the folded work.
  const conv = document.querySelector<HTMLElement>(".conv.is-thread-layout");
  assert(conv !== null, "The pane is not in the Threads layout");
  const column = conv.querySelector<HTMLElement>(".conv-column")!;
  // TN Code's wide chat width (the TN Code look, owner, 2026-11-16): 72rem.
  assert(chrome === "cli" || getComputedStyle(column).maxWidth === "1152px",
    `The reading lane is ${getComputedStyle(column).maxWidth}, not 1152px`);
  assert(conv.querySelector(".conv-turn-user .conv-turn-changes, .conv-turn-user button") === null,
    "A prompt bubble still carries a toolbar");
  const folds = [...conv.querySelectorAll<HTMLButtonElement>(".thread-fold")];
  assert(folds.length === 1, `Expected one work line, saw ${folds.length}`);
  const fold = folds[0];
  // Its name is the whole sentence; it draws the lead and a chip per kind of work.
  assert(fold.getAttribute("aria-expanded") === "false"
    && fold.getAttribute("aria-label") === "Worked · read 1 file, edited 1 file",
    `The work line is named ${fold.getAttribute("aria-label") ?? "nothing"}`);
  const chips = [...fold.querySelectorAll<HTMLElement>(".thread-work-chip")];
  assert(chips.map(chip => chip.textContent).join("|") === "read 1|edited 1"
    && chips.every(chip => chip.getBoundingClientRect().height === 18),
    `The work line's chips read ${chips.map(chip => chip.textContent).join("|")}`);
  // One neutral tally (the Threads look, owner, 2026-10-23): a count, never a colour.
  assert(getComputedStyle(chips[0]).color === getComputedStyle(chips[1]).color
    && getComputedStyle(chips[0]).backgroundColor === getComputedStyle(chips[1]).backgroundColor,
    "Read and edited chips are coloured by their kind");
  assert(!conv.textContent?.includes("gives up after one attempt"), "Folded work is drawn");
  assert(conv.textContent?.includes("The retry now backs off five times"), "The answer is folded away");
  // A failure is never folded: it stays below the line, its output open.
  const failure = conv.querySelector<HTMLElement>(".thread-failed");
  assert(failure !== null && failure.textContent?.includes("expected 5 attempts, saw 1")
    && (fold.compareDocumentPosition(failure) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
    "The failed test run is not shown, open, below the work line");
  fold.click();
  await frames(2);
  assert(fold.getAttribute("aria-expanded") === "true"
    && conv.textContent?.includes("gives up after one attempt"), "The work line did not open its work");
  fold.click();
  await frames(2);
  assert(fold.getAttribute("aria-expanded") === "false"
    && !conv.textContent?.includes("gives up after one attempt"), "The work line did not fold again");
  // What each turn changed, from Rust's measure; Review opens that turn in the panel.
  const cards = [...conv.querySelectorAll<HTMLElement>(".thread-changes")];
  assert(cards.length === 2 && cards[0].getAttribute("aria-label") === "3 files changed +41 −12"
    && cards[0].querySelector(".thread-file-path")?.textContent === "src/auth/retry.ts"
    && cards[0].textContent?.includes("+ 2 more"), "The first turn's changes card is wrong");
  const reviews = [...conv.querySelectorAll<HTMLButtonElement>(".thread-review")];
  assert(reviews.length === 2, `Expected two Review buttons, saw ${reviews.length}`);
  // The TN Code look (owner, 2026-11-16): in a project's tile the transcript
  // draws no hue — no aurora, your words in a bubble on the selection colour
  // laid over the ground, with its tail, Review a quiet capsule on a soft
  // fill — and the cards keep their file badges and five squares.
  assert(conv.closest(".project-hue-none") === null && getComputedStyle(conv).backgroundImage === "none",
    "The project's pane still draws an aurora");
  const bubbleBox = conv.querySelector<HTMLElement>(".conv-turn-user")!;
  const bubble = getComputedStyle(bubbleBox);
  assert(chrome === "cli" || (bubble.backgroundImage === "none"
    && bubble.backgroundColor === probeColour(conv, "var(--tn-bubble)")
    && bubble.borderTopLeftRadius === "18px"),
  "The prompt bubble is not an 18px bubble on the selection colour's wash");
  const tail = getComputedStyle(bubbleBox, "::before");
  assert(chrome === "cli" ? tail.content === "none"
    : tail.width === "24px" && tail.height === "20px"
      && tail.backgroundColor === bubble.backgroundColor,
  "The prompt bubble's tail is missing, or drawn in Bare CLI");
  assert(cards[0].querySelector(".file-badge")?.textContent === "TS"
    && cards.every(card => card.querySelector(".thread-changes-head .diffstat-bar") !== null),
    "The changes cards lack their file badges or five squares");
  assert(getComputedStyle(reviews[0]).backgroundColor === probeColour(conv, "var(--tn-fill)")
    && getComputedStyle(reviews[0]).boxShadow === "none",
    "Review is not a quiet capsule on a soft fill");
  assert(head.querySelector(".thread-head-changes .diffstat-bar") !== null,
    "The head's Changes button has no five squares");
  reviews[1].click();
  await frames();
  assert(diffs.join() === "1", "Review did not open its turn");
  assert(document.querySelector(".tdiff-scope .app-select-value")?.textContent?.startsWith("Turn 2 · "),
    "The panel did not switch to the turn");
  // An owned worktree thread's turn offers Undo beside its diff: the same
  // confirmation, and confirming hands that turn's ordinal to the restore.
  const undoTurn = document.querySelector<HTMLButtonElement>('.tdiff-head button[aria-label="Undo turn"]');
  assert(undoTurn !== null && undoTurn.title === "Restore files to before this message",
    "A worktree thread's diff offers no Undo turn");
  undoTurn.click();
  await frames();
  const undoDialog = document.querySelector<HTMLElement>(".confirm-dialog");
  assert(undoDialog !== null
    && undoDialog.querySelector(".confirm-dialog-title")?.textContent === "Restore files to before this message?"
    && undoDialog.querySelector(".confirm-dialog-description")?.textContent === "Restores 3 files.",
    "Undo turn did not ask before restoring");
  button("Restore files", undoDialog).click();
  await frames(2);
  assert(document.querySelector(".confirm-dialog") === null && undos.join() === "1",
    `Confirming did not restore turn 2's files (${undos.join()})`);
  await diffCommentChecks(quotes);
  await diffSplitChecks(quotes, grid, thread.key);
  // The turn's controls, once: always under the latest answer, on hover under the other.
  const feet = [...conv.querySelectorAll<HTMLElement>(".thread-turn-foot")];
  assert(feet.length === 2 && feet[1].classList.contains("is-latest")
    && !feet[0].classList.contains("is-latest"), "The turn controls are not once, under the latest answer");
  assert(getComputedStyle(feet[1]).opacity === "1" && getComputedStyle(feet[0]).opacity === "0",
    "An earlier turn's controls show without hover");
  assert(feet[1].querySelector("time")?.getAttribute("dateTime") === new Date(SENT + 101_000).toISOString(),
    "The latest answer does not say when it finished");
  assert(conv.querySelector(".conv-turn-end, .conv-turn-clock") === null,
    "The floating Fork row or the lone turn clock is still drawn");
  // Edit from here: under each prompt, the dialog, Rust's refusal for restoring the files, then keep them.
  const edit = [...document.querySelectorAll<HTMLButtonElement>(".thread-edit")];
  assert(edit.length === 2, "Edit from here is not under both prompts");
  const meta = edit[0].closest<HTMLElement>(".thread-user-meta")!;
  assert(getComputedStyle(meta).opacity === "0", "The line under a prompt shows without hover");
  edit[0].focus();
  for (let frame = 0; frame < 30 && getComputedStyle(meta).opacity !== "1"; frame += 1) await frames();
  assert(getComputedStyle(meta).opacity === "1", "Focus does not show the line under a prompt");
  edit[0].blur();
  // The first turn asks too: a new thread in the same workspace, files restored or not,
  // and this thread settled (the dialog says how to un-settle it if the card is closed).
  edit[0].click();
  await frames();
  const first = document.querySelector<HTMLElement>(".thread-edit-dialog");
  assert(first !== null && first.querySelector(".confirm-dialog-title")?.textContent === "Edit from here"
    && first.textContent?.includes("This thread is settled")
    && first.textContent.includes("Un-settle it from its menu"), "The first turn's edit did not ask");
  assert(!button(RESTORE_FILES_LABEL, first).disabled, "The first turn cannot restore its files");
  button("Cancel", first).click();
  await frames(2);
  assert(document.querySelector(".thread-edit-dialog") === null && edits.length === 0,
    "Cancelling the first turn's edit still edited");
  edit[1].click();
  await frames();
  const dialog = document.querySelector<HTMLElement>(".thread-edit-dialog");
  assert(dialog !== null && dialog.textContent?.includes("A new thread starts from just before this message")
    && !dialog.textContent.includes("Un-settle"), "The edit dialog did not open");
  button(RESTORE_FILES_LABEL, dialog).click();
  await frames(3);
  assert(dialog.textContent?.includes("Another session uses this worktree."), "Rust's refusal is not shown");
  // Refused: the option stays off, with Rust's sentence as its reason.
  const restore = button(RESTORE_FILES_LABEL, dialog);
  assert(restore.disabled && restore.title === "Another session uses this worktree.",
    "Restoring the files is still offered after Rust refused it");
  button(KEEP_FILES_LABEL, dialog).click();
  await frames(3);
  assert(document.querySelector(".thread-edit-dialog") === null, "The dialog stayed open after editing");
  assert(JSON.stringify(edits) === "[[1,true],[1,false]]", `Unexpected edits ${JSON.stringify(edits)}`);
  // Fork from here, in each finished turn's controls.
  const forkButtons = [...document.querySelectorAll<HTMLButtonElement>(".thread-fork")];
  assert(forkButtons.length === 2, `Expected two forks, saw ${forkButtons.length}`);
  forkButtons[0].click();
  await frames(2);
  assert(forks.join() === "1", "Fork did not keep the first turn");
  // Until that fork answers, the pane's Fork and Edit controls are off: a
  // double click is one fork.
  forkButtons[0].click();
  forkButtons[1].click();
  await frames();
  assert(forks.join() === "1", `A second click forked again: ${forks.join()}`);
  assert([...document.querySelectorAll<HTMLButtonElement>(".thread-fork, .thread-edit")]
    .every(control => control.disabled), "A busy pane left Fork or Edit live");
  pane.release();
  await frames(2);
  assert(!forkButtons[0].disabled && !edit[0].disabled, "The pane's controls stayed off after the answer");
  // Worktree threads have no Undo; Edit stands in for it.
  assert(document.querySelector(".thread-undo, .conv-turn-restore") === null, "A worktree thread shows Undo");
  // Esc in the panel closes it and hands focus back to the pane's Diff toggle.
  const scope = document.querySelector<HTMLButtonElement>(".tdiff .app-select-trigger")!;
  scope.focus();
  scope.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await frames(3);
  assert(document.querySelector(".tdiff") === null, "Esc did not close the diff panel");
  const toggle = threadDiffToggleOf(thread.key);
  assert(toggle !== null && document.activeElement === toggle,
    `Focus went to ${document.activeElement?.outerHTML.slice(0, 120) ?? "nothing"}, not the Diff toggle`);
  toggle.click();
  await frames(2);
  assert(document.querySelector(".tdiff") !== null, "The Diff toggle did not reopen the panel");
  toggle.blur();
  // Leave the panel on the Thread scope for the capture, with one line quoted
  // and a comment being written on another, as the owner would see it.
  await chooseDiffScope("Whole thread");
  assert(document.querySelector(".tdiff-scope .app-select-value")?.textContent === "Whole thread",
    "The scope menu did not choose Whole thread");
  assert(document.querySelector(".tdiff-inprompt") === null, "Quoted marks outlived their scope");
  await quoteLine(1, "Use the RETRIES constant here as well.");
  diffRow(6).click();
  await frames(2);
  typeComment("Also cover the fifth attempt in the test.");
  await frames(2);
  // Forks, scope changes and panel toggles re-rendered the pane many times;
  // the turns never moved, so the shell heard nothing new.
  assert(rowReports.length === reportsAtMount,
    `The pane re-reported its turns ${rowReports.length - reportsAtMount} times without a change`);
  return { branch: base.workspace!.branch, files: DIFF.files.length, edits: edits.length, forks: forks.length,
    quotes: quotes.length, reads };
}

/** The panel's nth selectable line, by its row (the whole row is the target). */
function diffRow(nth: number): HTMLElement {
  const lines = [...document.querySelectorAll<HTMLButtonElement>(".tdiff button.tdiff-ln")];
  const row = lines[nth]?.closest<HTMLElement>(".tdiff-dl");
  assert(row, `The diff panel has no selectable line ${nth}`);
  return row;
}

function typeComment(text: string): void {
  const input = document.querySelector<HTMLTextAreaElement>(".tdiff-comment-input");
  assert(input, "No comment box is open");
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function quoteLine(nth: number, comment: string): Promise<void> {
  diffRow(nth).click();
  await frames(2);
  typeComment(comment);
  await frames();
  document.querySelector(".tdiff-comment-input")!.dispatchEvent(new KeyboardEvent("keydown",
    { key: "Enter", metaKey: true, bubbles: true, cancelable: true }));
  await frames(2);
}

async function chooseDiffScope(label: string): Promise<void> {
  document.querySelector<HTMLButtonElement>(".tdiff .app-select-trigger")!.click();
  await frames(2);
  const option = [...document.querySelectorAll<HTMLElement>(".app-select-popover [role=\"option\"]")]
    .find(node => node.querySelector(".app-select-option-label")?.textContent === label);
  assert(option, `The scope menu has no ${label}`);
  option.click();
  await frames(3);
}

/**
 * The diff panel's line comments: a click selects a line and opens its comment
 * box, Shift-click extends it, ⌘↵ hands the quote over and marks the lines, Esc
 * cancels, and the keyboard reaches all of it.
 */
async function diffCommentChecks(quotes: ThreadDiffQuote[]): Promise<void> {
  const panel = document.querySelector<HTMLElement>(".tdiff")!;
  // Each hunk after the first lines of its file opens on a strip: its range,
  // its function and the unchanged lines above it.
  const strips = [...panel.querySelectorAll<HTMLElement>(".tdiff-hunk")];
  assert(strips.length === 2 && strips[0].textContent === "@@ -12,5 +12,7 @@export function retry11 unchanged lines"
    && strips[1].textContent === "@@ -40,3 +43,3 @@23 unchanged lines",
  `The hunk strips read ${strips.map(strip => strip.textContent).join(" | ")}`);
  const code = panel.querySelector<HTMLElement>(".tdiff-dl.is-add .tdiff-code")!;
  assert(getComputedStyle(code).whiteSpace === "pre-wrap", "Long diff lines do not wrap");
  // Badges, the diffstat, edge bars and highlighted code as spans.
  assert(panel.querySelector(".tdiff-fh .file-badge.kind-ts")?.textContent === "TS", "A file header has no type badge");
  assert(panel.querySelector(".tdiff-fh .tdiff-dir") !== null, "A file header's folders are not set apart");
  assert(panel.querySelector(".tdiff-head .diffstat-bar") !== null, "The totals have no diffstat");
  assert(getComputedStyle(code.parentElement!).boxShadow.includes("inset"), "An added row has no edge bar");
  assert(code.querySelector(".tdiff-tok-keyword") !== null && code.textContent === "  for (let attempt = 0; "
    + "attempt < RETRIES; attempt++) {", `The added line reads ${code.textContent ?? "nothing"}`);
  const body = panel.querySelector<HTMLElement>(".tdiff-body")!;
  assert(body.scrollWidth <= body.clientWidth + 1, "The diff body scrolls sideways");
  // The changed files above the diffs: the first marked; a row goes to its file.
  const fileRows = [...panel.querySelectorAll<HTMLButtonElement>(".tdiff-files .tdiff-frow")];
  const marked = (): number => fileRows.findIndex(row => row.getAttribute("aria-current") === "true");
  assert(fileRows.length === DIFF.files.length && marked() === 0 && fileRows[0].tabIndex === 0,
    `The changed files list has ${fileRows.length} rows, the ${marked()}th marked`);
  fileRows[1].click();
  await frames(3);
  const login = panel.querySelector<HTMLElement>('.tdiff-file[aria-label="Changes in test/login.test.ts"]')!;
  const seen = login.getBoundingClientRect().top - body.getBoundingClientRect().top;
  assert(marked() === 1 && fileRows[1].tabIndex === 0 && seen >= 0 && seen < body.clientHeight,
    `Choosing test/login.test.ts marked row ${marked()} and left its card ${seen}px down`);
  fileRows[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true, cancelable: true }));
  assert(document.activeElement === fileRows[0], "Home did not move to the first changed file");
  fileRows[0].click();
  await frames(3);
  assert(marked() === 0 && body.scrollTop === 0, `Back to the first file, the body sits at ${body.scrollTop}`);
  // A click selects the line and opens the comment box on it, focused.
  diffRow(2).click();
  await frames(2);
  let input = panel.querySelector<HTMLTextAreaElement>(".tdiff-comment-input");
  assert(input !== null && document.activeElement === input, "The comment box did not open focused");
  assert(panel.querySelectorAll(".tdiff-dl.is-selected").length === 1, "The clicked line is not selected");
  // Shift-click extends to a range inside the hunk.
  diffRow(4).dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
  await frames(2);
  assert(panel.querySelectorAll(".tdiff-dl.is-selected").length === 3, "Shift-click did not extend the range");
  assert(panel.querySelector(".tdiff-comment-label")?.textContent === "Comment on lines 13–15",
    `The comment box reads ${panel.querySelector(".tdiff-comment-label")?.textContent ?? "nothing"}`);
  // Esc cancels the comment and leaves the panel open, focus back on the line.
  input = panel.querySelector<HTMLTextAreaElement>(".tdiff-comment-input")!;
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await frames(2);
  assert(document.querySelector(".tdiff") !== null, "Esc in a comment closed the panel");
  assert(panel.querySelector(".tdiff-comment") === null, "Esc did not cancel the comment");
  assert(document.activeElement?.matches("button.tdiff-ln") === true, "Cancel did not hand focus back to the line");
  // ⌘↵ quotes the lines into the prompt and marks them.
  await quoteLine(2, "Name the attempt counter after RETRIES.");
  assert(quotes.length === 1, `Expected one quote, saw ${quotes.length}`);
  const quote = quotes[0];
  assert(quote.path === "src/auth/retry.ts" && quote.startLine === 13 && quote.side === "new"
    && quote.code.startsWith("+  for (let attempt"), `Unexpected quote ${JSON.stringify(quote)}`);
  assert(formatDiffQuote(quote).startsWith("src/auth/retry.ts:13\n```diff\n"), "The quote is not formatted");
  // The file's row says it gave the prompt a comment; the comment stays under its line.
  assert(panel.querySelector<HTMLElement>(".tdiff-frow .tdiff-inprompt")?.title
    === "1 comment on this file added to the prompt"
    && fileRows[0].getAttribute("aria-label")?.endsWith(", 1 comment in the prompt") === true,
  "The file's row does not say 1 comment in the prompt");
  const posted = panel.querySelector<HTMLElement>(".tdiff-posted");
  assert(posted !== null && posted.previousElementSibling?.querySelector('[aria-label="Added line 13"]') !== null
    && posted.querySelector(".tdiff-posted-text")?.textContent === "Comment on line 13: Name the attempt counter after RETRIES."
    && posted.querySelector(".tdiff-posted-tag")?.textContent === "In the prompt",
  `The sent comment reads ${posted?.textContent ?? "nothing"}`);
  assert(panel.querySelectorAll(".tdiff-dl.is-quoted").length === 1, "The quoted line is not marked");
  // The keyboard path: arrows walk the lines, Shift extends, Esc clears, Tab reaches the box.
  const start = document.activeElement as HTMLButtonElement;
  assert(start.matches("button.tdiff-ln") && start.tabIndex === 0, "The quoted line does not hold the tab stop");
  start.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", shiftKey: true, bubbles: true, cancelable: true }));
  await frames(2);
  assert(panel.querySelectorAll(".tdiff-dl.is-selected").length === 2, "Shift+ArrowDown did not select two lines");
  assert(document.activeElement?.matches("button.tdiff-ln") === true && document.activeElement !== start,
    "Shift+ArrowDown did not move to the next line");
  assert(panel.querySelector(".tdiff-comment-input") !== null, "A keyboard selection has no comment box");
  document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await frames(2);
  assert(document.querySelector(".tdiff") !== null && panel.querySelector(".tdiff-comment") === null,
    "Esc on a selected line did not just clear the selection");
}

/** Comments on whichever line `cell` draws and sends it with ⌘↵: the quote App would get. */
async function quoteCell(cell: HTMLElement, comment: string, quotes: ThreadDiffQuote[]): Promise<ThreadDiffQuote> {
  const before = quotes.length;
  cell.click();
  await frames(2);
  const row = cell.closest<HTMLElement>(".tdiff-sl");
  assert(row?.nextElementSibling?.classList.contains("tdiff-comment") === true,
    "The comment box is not under the split row it comments on");
  typeComment(comment);
  await frames();
  document.querySelector(".tdiff-comment-input")!.dispatchEvent(new KeyboardEvent("keydown",
    { key: "Enter", metaKey: true, bubbles: true, cancelable: true }));
  await frames(2);
  assert(quotes.length === before + 1, `Expected quote ${before + 1}, saw ${quotes.length}`);
  return quotes[quotes.length - 1];
}

/**
 * Stacked / Split: a narrow panel stays stacked whatever is chosen; a wide one
 * draws the old file on the left and the new on the right, row by row, and a
 * comment from either column quotes its own file's line into the draft. The
 * choice outlives the panel closing, and goes back to Stacked for the rest.
 */
async function diffSplitChecks(quotes: ThreadDiffQuote[], grid: HTMLElement, key: string): Promise<void> {
  const layout = (name: "Stacked" | "Split"): HTMLButtonElement => {
    const control = document.querySelector<HTMLButtonElement>(`.tdiff-layout-btn[aria-label="${name}"]`);
    assert(control, `The diff panel has no ${name} button`);
    return control;
  };
  const retry = (): HTMLElement => document.querySelector<HTMLElement>('.tdiff-file[aria-label="Changes in src/auth/retry.ts"]')!;
  const cellOf = (label: string): HTMLElement => {
    const cell = retry().querySelector(`.tdiff-sl button.tdiff-ln[aria-label="${label}"]`)?.closest<HTMLElement>(".tdiff-dl");
    assert(cell, `Split draws no ${label}`);
    return cell;
  };
  assert(document.querySelector(".tdiff-layout")?.getAttribute("role") === "group"
    && layout("Stacked").getAttribute("aria-pressed") === "true" && document.querySelector(".tdiff-sl") === null,
    "The diff panel does not open Stacked");
  // The fixture's 340px column has no room for two: Split is chosen and kept, the rows stay stacked.
  layout("Split").click();
  await frames(2);
  const panel = document.querySelector<HTMLElement>(".tdiff")!;
  assert(layout("Split").getAttribute("aria-pressed") === "true"
    && localStorage.getItem(THREAD_DIFF_LAYOUT_KEY) === "split", "Choosing Split was not pressed or kept");
  assert(panel.clientWidth < SPLIT_DIFF_MIN_WIDTH && document.querySelector(".tdiff-sl") === null
    && retry().querySelector(".tdiff-lines > .tdiff-dl.is-add") !== null,
    `A ${panel.clientWidth}px panel did not stay stacked`);
  // The Changes column at its widest (theme.css `.has-thread-diff`): two columns.
  grid.style.setProperty("--pane-detail", "560px");
  await until(() => document.querySelector(".tdiff-sl") !== null,
    () => `Split did not draw in a ${panel.clientWidth}px panel`);
  const rows = [...retry().querySelectorAll<HTMLElement>(".tdiff-sl")];
  // Two hunks of 7 and 3 rows; the first's 1 removal against 3 additions is one pair, then two empty lefts.
  assert(rows.length === 10 && retry().querySelectorAll(".tdiff-sl > .tdiff-dl.is-empty:first-child").length === 2,
    `Split drew ${rows.length} rows`);
  const [left, right] = [...rows[0].children].map(cell => cell.getBoundingClientRect());
  assert(rows[0].children.length === 2 && Math.abs(left.top - right.top) < 1 && left.right <= right.left + 1
    && Math.abs(left.width - right.width) <= 2, `A split row is not two equal columns: ${left.width} / ${right.width}`);
  // A context line is on both sides, each numbered by its own file.
  const limit = rows.find(row => row.textContent?.includes("RETRY_LIMIT_MS"))!;
  assert([...limit.querySelectorAll(".tdiff-ln")].map(number => number.textContent).join("|") === "40|43",
    `The context row reads ${[...limit.querySelectorAll(".tdiff-ln")].map(number => number.textContent).join("|")}`);
  const body = panel.querySelector<HTMLElement>(".tdiff-body")!;
  assert(body.scrollWidth <= body.clientWidth + 1, "The split diff scrolls sideways");
  const added = retry().querySelector<HTMLElement>(".tdiff-sl > .tdiff-dl.is-add .tdiff-code")!;
  assert(added.querySelector(".tdiff-tok-keyword") !== null
    && getComputedStyle(added.parentElement!).boxShadow.includes("inset"), "Split lost the rows' tokens or edge bar");
  // A comment from the right column quotes the new file's line…
  const fromRightCell = cellOf("Added line 14");
  assert(fromRightCell.parentElement!.lastElementChild === fromRightCell
    && fromRightCell.parentElement!.firstElementChild!.classList.contains("is-empty"),
    "Added line 14 is not in the right column beside an empty left");
  const fromRight = await quoteCell(fromRightCell, "Name the wait after the attempt.", quotes);
  assert(fromRight.path === "src/auth/retry.ts" && fromRight.startLine === 14 && fromRight.side === "new"
    && fromRight.code === "+    const wait = backoff(attempt);", `Unexpected quote ${JSON.stringify(fromRight)}`);
  assert(appendDiffQuote("Please tidy this up.", fromRight) === "Please tidy this up.\n\nsrc/auth/retry.ts:14\n"
    + "```diff\n+    const wait = backoff(attempt);\n```\nName the wait after the attempt.\n\n",
  "The right column's quote does not land in the draft as line 14");
  // …and one from the left column the old file's.
  const fromLeftCell = cellOf("Removed line 13");
  assert(fromLeftCell.parentElement!.firstElementChild === fromLeftCell, "Removed line 13 is not in the left column");
  const fromLeft = await quoteCell(fromLeftCell, "Keep the old bound in the test.", quotes);
  assert(fromLeft.startLine === 13 && fromLeft.side === "old" && fromLeft.code === "-  for (let i = 0; i < 3; i++) {"
    && formatDiffQuote(fromLeft).startsWith("src/auth/retry.ts:13 (removed)\n```diff\n"),
  `Unexpected quote ${JSON.stringify(fromLeft)}`);
  assert(cellOf("Removed line 13").classList.contains("is-quoted") && cellOf("Added line 14").classList.contains("is-quoted"),
    "The quoted split lines are not marked");
  // The keyboard reads row by row: from the removal across to the line that replaced it.
  const start = cellOf("Removed line 13").querySelector<HTMLButtonElement>("button.tdiff-ln")!;
  assert(document.activeElement === start, "Sending did not hand focus back to the left line");
  start.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
  await frames(2);
  assert(document.activeElement?.getAttribute("aria-label") === "Added line 13",
    `ArrowDown went to ${document.activeElement?.getAttribute("aria-label") ?? "nothing"}, not across the row`);
  // The choice outlives the panel: closed and opened again, it is still Split.
  const toggle = threadDiffToggleOf(key)!;
  toggle.click();
  await frames(2);
  assert(document.querySelector(".tdiff") === null, "The Diff toggle did not close the panel");
  toggle.click();
  await until(() => document.querySelector(".tdiff-sl") !== null, "Split was not remembered when the panel opened again");
  assert(layout("Split").getAttribute("aria-pressed") === "true", "The reopened panel does not press Split");
  toggle.blur();
  // Back to Stacked, kept, and the fixture's own column again for the checks that follow.
  layout("Stacked").click();
  await frames(2);
  assert(document.querySelector(".tdiff-sl") === null && localStorage.getItem(THREAD_DIFF_LAYOUT_KEY) === "stacked",
    "Choosing Stacked did not draw or keep it");
  grid.style.removeProperty("--pane-detail");
  await frames(2);
}

/**
 * Settings → Threads: the four settings Rust holds, each change sending the
 * whole object, and the merge switch resting while pull requests are not tracked.
 */
export async function threadSettingsFixture(root: Root, theme: string, chrome: string): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  setLook(theme, chrome);
  const sent: ThreadSettings[] = [];
  function Harness() {
    const [settings, setSettings] = useState<ThreadSettings>({ ...view.settings });
    return (
      <div style={{ height: "100vh" }}>
        <style>{APP_SELECT_CSS}</style>
        <SettingsPage appearance={{ preference: "dark", resolved: "dark" }} onAppearanceChange={noop}
          preferences={{ ...DEFAULT_APP_PREFERENCES }} onPreferencesChange={noop}
          engineHealth={[]} onRefreshProviders={noop}
          customizationApi={{ getDefaultDirectory: () => Promise.resolve("/fixture"),
            listCustomization: root => Promise.resolve({ root, entries: [], truncated: false }) }}
          threadSettings={settings}
          onThreadSettingsChange={next => { sent.push(next); setSettings(next); }}
          onClose={noop} />
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  await document.fonts.ready;
  await frames(3);
  button("Threads").click();
  await frames(2);
  const section = [...document.querySelectorAll<HTMLElement>(".settings-section")]
    .find(item => !item.hidden && item.querySelector(".settings-section-title")?.textContent === "Threads");
  assert(section, "Settings has no Threads page");
  const toggle = (label: string): HTMLInputElement => {
    const row = [...section.querySelectorAll<HTMLElement>(".settings-row")]
      .find(item => item.querySelector(".settings-row-label")?.textContent === label);
    const input = row?.querySelector<HTMLInputElement>('input[role="switch"]');
    assert(input, `Missing the ${label} switch`);
    return input;
  };
  assert(section.textContent?.includes("Uses a little of your Claude plan"), "Generated titles does not say it uses the Claude plan");
  toggle("Name threads automatically").click();
  await frames();
  assert(sent.at(-1)?.generatedTitles === true && sent.at(-1)?.autoSettleDays === view.settings.autoSettleDays,
    `Generated titles sent ${JSON.stringify(sent.at(-1))}`);
  toggle("Follow pull requests on GitHub").click();
  await frames();
  assert(sent.at(-1)?.trackPullRequests === false, `Follow pull requests sent ${JSON.stringify(sent.at(-1))}`);
  assert(toggle("Settle when the pull request merges").disabled,
    "Settle on merge stays live while pull requests are not tracked");
  toggle("Follow pull requests on GitHub").click();
  await frames();
  toggle("Settle when the pull request merges").click();
  await frames();
  assert(sent.at(-1)?.settleOnMerge === false && sent.at(-1)?.trackPullRequests === true,
    `Settle on merge sent ${JSON.stringify(sent.at(-1))}`);
  section.querySelector<HTMLButtonElement>(".app-select-trigger")!.click();
  await frames();
  const never = [...document.querySelectorAll<HTMLElement>('[role="option"]')]
    .find(option => option.textContent?.trim() === "Never");
  assert(never, "The idle-settle menu has no Never");
  const offered = [...document.querySelectorAll<HTMLElement>('[role="option"]')].map(option => option.textContent?.trim());
  assert(offered.join() === "Never,1 day,3 days,7 days,14 days,30 days", `Idle-settle choices were ${offered.join()}`);
  never.click();
  await frames(2);
  assert(sent.at(-1)?.autoSettleDays === 0, `Never sent ${JSON.stringify(sent.at(-1))}`);
  assert(sent.length === 5, `Expected five changes, saw ${sent.length}`);
  return { changes: sent.length };
}

/**
 * A Threads pane's live turn (owner, 2026-10-07) in a real DOM: the run under
 * its prompt — header words and clock, the trail, a running edit's peek, a
 * step opened in place, the waiting step, a steer — no working strip, the live
 * capsule once scrolled away, then the finish into the work line, the changes
 * card and the turn's controls, and a sent prompt rising into place. With
 * motion on (`prefers-reduced-motion: no-preference`) each beat of the finish
 * is seen in turn; reduced, the finished turn is there at once and nothing
 * rises or turns. The step's words never shimmer (cut for less motion).
 */
export type ThreadLivePose = "running" | "waiting" | "all";

export async function threadLiveFixture(root: Root, theme: string, chrome: string,
    pose: ThreadLivePose = "all"): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  setLook(theme, chrome);
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const base = view.threads.find(thread => thread.workspace?.mode === "worktree")!;
  const cwd = base.workspace!.worktreePath!;
  const file = (path: string): string => `${cwd}/${path}`;
  const started = Date.now() - 34_000;
  const first = "Fix the flaky login test";
  const second = "Now add a regression test for the retry path";
  const done = { ok: true, summary: "done" };
  // A long first answer, so the pane can be scrolled away from the live turn.
  const answer = Array.from({ length: 24 }, (_, index) =>
    `Step ${index + 1}: the retry now backs off before attempt ${index + 2}, and the test waits for it.`)
    .join("\n\n");
  const history: ConvItem[] = [
    { kind: "user", id: "user-0", text: first },
    { kind: "tool", id: "read-1", tool: "Read", summary: file("src/auth/retry.ts"),
      input: { file_path: file("src/auth/retry.ts") }, result: done },
    { kind: "text", id: "text-1", text: answer, streaming: false },
  ];
  const editInput = { file_path: file("test/login.test.ts"), old_string: "",
    new_string: "describe(\"retry\", () => {\n  it(\"backs off five times\", async () => {\n"
      + "    expect(await attempts()).toBe(5);\n  });\n});" };
  const editing: ConvItem = { kind: "tool", id: "edit-2", tool: "Edit", summary: file("test/login.test.ts"),
    input: editInput };
  const diff: ConvItem = { kind: "diff", id: "diff-2", path: file("test/login.test.ts"), callId: "edit-2",
    added: 5, removed: 0 };
  const running: ConvItem[] = [
    { kind: "user", id: "user-3", text: second },
    { kind: "tool", id: "read-2", tool: "Read", summary: file("test/login.test.ts"),
      input: { file_path: file("test/login.test.ts") }, result: done },
    { kind: "text", id: "text-4", text: "Adding the regression test.", streaming: false },
    editing, diff,
  ];
  const bash: ConvItem = { kind: "tool", id: "bash-5", tool: "Bash", summary: "npm test -- login",
    input: { command: "npm test -- login" } };
  const ask: Extract<ConvItem, { kind: "approval" }> = { kind: "approval", id: "ask-6", tool: "Bash",
    summary: "npm test -- login", input: { command: "npm test -- login" } };
  const turns = (state: "running" | "done"): ThreadTurn[] => [
    turn(0, first, { filesChanged: 1, added: 8, removed: 1 }),
    { ...turn(1, second), sentAt: started, state, hasEnd: state === "done",
      ...(state === "done" ? { filesChanged: 1, added: 5, removed: 0 } : {}) },
  ];
  interface Stage { items: ConvItem[]; thinking: boolean; thread: ConversationThread; active: Set<string> }
  const stage = (items: ConvItem[], state: "running" | "done", active: string[] = []): Stage => ({
    items, thinking: state === "running", active: new Set(active),
    thread: { turns: turns(state), project: true, worktree: true, revertFilesBlocked: null,
      lastRun: state === "running" ? { startedAt: started }
        : { startedAt: started, endedAt: Date.now(), outcome: "completed" } },
  });
  const control = { set: (_next: Stage): void => undefined };
  const sends: string[] = [];
  const session: SessionSummary = {
    engine: "claude", sessionId: "7b1f0c2e-1d4e-4c55-9a7e-5d2f1c0b9e61", title: first, cwd,
    status: "working", cloud: false, lastActivity: "2026-10-07T10:00:00Z", firstSeen: "2026-10-07T09:00:00Z",
    archived: false,
  };
  function Harness() {
    const [current, setCurrent] = useState<Stage>(() => stage([...history, ...running], "running"));
    control.set = next => { flushSync(() => { setCurrent(next); }); };
    const props: ConversationProps = {
      items: current.items, live: true, thinking: current.thinking, focused: true,
      sessionKey: base.key, turnStartedAt: current.thinking ? started : undefined,
      config: { effort: null, models: [], canSetModel: false, effortLevels: [], showEffort: false,
        canSetEffort: false, pendingModel: false, pendingEffort: false, error: null },
      onSend: text => { sends.push(text); }, onStop: noop, onSetModel: noop, onSetEffort: noop,
      onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: current.active,
      pathRoot: cwd, threadLayout: true, thread: current.thread,
      onThreadForkAt: noop,
    };
    return (
      <div className="app-grid no-sidebar">
        <style>{APP_SELECT_CSS}</style>
        <main className="workspace-shell" style={{ gridColumn: 2 }}>
          <div className="workspace-content">
            <div className="workspace-grid workspace-count-1">
              <AgentTile session={session} selected maximized={false} live engineLabel="Claude"
                projectLabel="atlas" projectRoot={base.workspace?.projectRoot}
                onFocus={noop} onToggleMaximize={noop} onClose={noop}>
                {/* A pane of a fixed height, so its transcript scrolls. */}
                <div className="tile-stack" style={{ display: "flex", flexDirection: "column", height: 620 }}>
                  <Conversation {...props} />
                </div>
              </AgentTile>
            </div>
          </div>
        </main>
      </div>
    );
  }
  flushSync(() => root.render(<Harness />));
  await document.fonts.ready;
  await frames(3);
  const conv = document.querySelector<HTMLElement>(".conv.is-thread-layout");
  assert(conv !== null, "The pane is not in the Threads layout");
  const list = conv.querySelector<HTMLElement>(".conv-list")!;
  const probe = (expression: string): string => {
    const node = document.createElement("span");
    node.style.color = expression;
    conv.append(node);
    const colour = getComputedStyle(node).color;
    node.remove();
    return colour;
  };
  const text = (selector: string, scope: ParentNode = conv): string =>
    scope.querySelector(selector)?.textContent ?? "";
  const report: Record<string, unknown> = { reduced };

  // ---- running: under the prompt, not above the composer -------------------
  const liveTurn = conv.querySelector<HTMLElement>(".thread-live");
  assert(liveTurn !== null, "The running turn has no live block");
  assert(conv.querySelector(".conv-working") === null, "The bottom working strip is still drawn");
  const prompts = [...conv.querySelectorAll<HTMLElement>(".thread-user")];
  assert(prompts.length === 2 && prompts[1].nextElementSibling === liveTurn,
    "The live turn is not right under its prompt");
  // The status line (the Threads look, owner, 2026-10-23): under the trail,
  // the Pair mark in the rail's column, the step's words and the clock.
  const head = liveTurn.querySelector<HTMLElement>(".thread-live-status")!;
  const trails = [...liveTurn.querySelectorAll<HTMLElement>(".thread-trail")];
  assert(head !== null && trails.length > 0
    && head.getBoundingClientRect().top >= trails[trails.length - 1].getBoundingClientRect().bottom,
  "The status line is not under the trail");
  assert(text(".thread-live-verb", head) === "Editing" && text(".thread-live-target", head) === "test/login.test.ts",
    `The status line reads ${head.textContent ?? ""}`);
  assert(/^0:3\d$/u.test(text(".thread-live-clock", head)), `The live clock reads ${text(".thread-live-clock", head)}`);
  const pair = head.querySelector<SVGElement>(".thread-live-mark .thinking-mark");
  assert(pair !== null && Math.round(pair.getBoundingClientRect().width) === 16
    && head.querySelector(".working-wave, .working-indicator") === null,
  "The status line has no Pair mark, or still draws the Wave");
  const markLeft = head.querySelector(".thread-live-mark")!.getBoundingClientRect().left;
  const nodeLeft = liveTurn.querySelector(".thread-step-node")!.getBoundingClientRect().left;
  assert(Math.abs(markLeft - nodeLeft) < 1, `The mark (${markLeft}) is not in the rail's column (${nodeLeft})`);
  assert(text('.conv-sr[role="status"]', liveTurn) === "Working", "The live region does not say Working");
  const verb = head.querySelector<HTMLElement>(".thread-live-verb")!;
  const still = getComputedStyle(verb);
  assert(still.animationName === "none" && still.color === probe("var(--fg)"),
    `The step's verb moves or is not the text ink (${still.animationName}, ${still.color})`);
  // The trail: a read done, the prose between, the edit running with no lines yet.
  const stepRows = [...liveTurn.querySelectorAll<HTMLElement>(".thread-step")];
  assert(stepRows.length === 2 && liveTurn.querySelectorAll(".thread-trail").length === 2,
    `Expected two steps in two trail segments, saw ${stepRows.length}`);
  assert(stepRows.every(row => Math.round(row.querySelector(".thread-step-row")!.getBoundingClientRect().height) === 32),
    "A trail row is not 32px");
  const [readStep, editStep] = stepRows;
  assert(readStep.querySelector(".thread-step-node.is-done") !== null
    && readStep.querySelector(".thread-step-row")?.getAttribute("title") === file("test/login.test.ts"),
  "The finished read is not a done step keeping its absolute path on the title");
  // Each step wears its tool's glyph on the rail; the running ring is on the right.
  assert(readStep.querySelector(".thread-step-node .thread-glyph") !== null
    && editStep.querySelector(".thread-step-node .thread-glyph") !== null, "A step has no tool glyph on the rail");
  assert(editStep.querySelector(".thread-step-node.is-running") !== null
    && editStep.querySelector(".thread-step-stat") === null, "The running edit shows lines or no ring");
  const ringNode = editStep.querySelector<HTMLElement>(".thread-step-end .thread-step-ring.is-running");
  assert(ringNode !== null && ringNode.getBoundingClientRect().left
    > editStep.querySelector(".thread-step-words")!.getBoundingClientRect().right - 1,
  "The running ring is not on the right of the row");
  const ring = getComputedStyle(ringNode);
  assert(reduced ? ring.animationName === "none" : ring.animationName === "thread-spin",
    `The running ring's motion is ${ring.animationName}`);
  const peek = [...editStep.querySelectorAll(".thread-peek-line")].map(line => line.textContent ?? "");
  assert(peek.length === 4 && peek[0] === "describe(\"retry\", () => {", `The edit's peek reads ${peek.join(" | ")}`);
  assert(liveTurn.textContent?.includes("Adding the regression test."), "The turn's prose is not in the trail");
  // A step opens its own detail in place, and folds again.
  const readButton = readStep.querySelector<HTMLButtonElement>(".thread-step-row")!;
  readButton.click();
  await frames(2);
  assert(readButton.getAttribute("aria-expanded") === "true"
    && readStep.querySelector(".thread-step-detail .conv-tool-card") !== null, "The read did not open in place");
  readButton.click();
  await frames(2);
  assert(readStep.querySelector(".thread-step-detail") === null, "The read did not fold again");
  report.running = { header: head.textContent, steps: stepRows.length };
  if (pose === "running") return report;

  // ---- the live capsule, scrolled away --------------------------------------
  list.scrollTop = 0;
  list.dispatchEvent(new Event("scroll"));
  await until(() => conv.querySelector(".conv-jump-latest.is-live") !== null,
    () => `No live capsule after scrolling away (${list.scrollHeight}/${list.clientHeight})`);
  const pill = conv.querySelector<HTMLElement>(".conv-jump-latest.is-live")!;
  assert(text(".thread-pill-line", pill) === "Editing login.test.ts" && pill.querySelector(".thinking-mark") !== null
    && /^0:\d\d$/u.test(text(".thread-live-clock", pill)), `The live capsule reads ${pill.textContent ?? ""}`);
  assert(!pill.textContent?.includes("Approval"), "The live capsule speaks of an approval");
  pill.click();
  await until(() => conv.querySelector(".conv-jump-latest") === null, "The live capsule did not jump back down");
  report.capsule = pill.textContent;

  // ---- waiting on the owner: the step waits, no card -----------------------
  control.set(stage([...history, ...running, bash, ask], "running", [ask.id]));
  await frames(2);
  assert(text(".thread-live-verb") === "Waiting for you"
    && conv.querySelector(".thread-live-status .thread-live-dot") !== null,
  `The waiting status line reads ${text(".thread-live-status")}`);
  assert(/^\d+:\d\d$/u.test(text(".thread-live-clock")), `The waiting clock reads ${text(".thread-live-clock")}`);
  const waiting = conv.querySelector<HTMLElement>(".thread-step.is-waiting");
  assert(waiting !== null && text(".thread-step-meta", waiting) === "needs your OK"
    && text(".thread-step-verb", waiting) === "Run", `The waiting step reads ${waiting?.textContent ?? "nothing"}`);
  assert(getComputedStyle(waiting.querySelector(".thread-step-meta")!).color === probe("var(--thread-waiting)"),
    "The waiting note is not the app's amber");
  assert(list.querySelector(".conv-approval") === null, "The transcript draws an approval card");
  assert(reduced || getComputedStyle(waiting).animationName === "thread-rise", "A new step does not rise in");
  assert(text('.conv-sr[role="status"]', conv.querySelector(".thread-live")!) === "Waiting for your approval",
    "The live region does not say it waits for approval");
  if (pose === "waiting") return report;

  // ---- a steer, and the answered request -----------------------------------
  const steered = [...history, ...running, { ...bash, result: done }, { ...ask, resolved: "allow" as const },
    { kind: "user" as const, id: "steer-7", text: "Use five retries, not three" },
    { kind: "text" as const, id: "text-8", text: "Switching to five.", streaming: false }];
  control.set(stage(steered, "running"));
  await frames(2);
  const steer = conv.querySelector<HTMLElement>(".thread-steer");
  assert(steer !== null && text(".thread-steer-label", steer) === "You steered"
    && text(".thread-steer-text", steer) === "Use five retries, not three", "The steer is not drawn in the trail");
  assert(conv.querySelectorAll(".thread-user").length === 2, "The steer opened a turn of its own");
  assert([...conv.querySelectorAll(".thread-step-prefix")].some(node => node.textContent === "Allowed"),
    "The answered request is not a quiet Allowed row");

  // ---- the finish -----------------------------------------------------------
  const finished = [...history, running[0], running[1], running[2], { ...editing, result: done }, diff,
    ...steered.slice(history.length + running.length),
    { kind: "text" as const, id: "text-9", text: "The regression test passes.", streaming: false }];
  control.set(stage(finished, "done"));
  if (!reduced) {
    await frames(1);
    assert(conv.querySelector(".thread-live.is-done .thread-live-check") !== null,
      "The finish does not begin with the done check");
    assert(text(".thread-live-verb") === "Done" && conv.querySelector(".thread-live-clock.is-done") !== null,
      "The finished status line does not freeze its clock in done blue");
    await until(() => conv.querySelector(".thread-live.is-collapsing > .thread-fold") !== null,
      "The trail does not collapse into the work line", { timeoutMs: 1500, pollMs: 10 });
    await until(() => conv.querySelector(".thread-live") === null
      && conv.querySelector(".thread-changes.is-arriving") !== null
      && conv.querySelector(".thread-turn-foot.is-arriving") !== null,
    "The changes card and the turn's controls do not arrive", { timeoutMs: 1500, pollMs: 10 });
    await until(() => conv.querySelector(".is-arriving") === null, "The finish never settles",
      { timeoutMs: 1500, pollMs: 20 });
  } else {
    await frames(2);
  }
  assert(conv.querySelector(".thread-live") === null, "The finished turn still draws its live block");
  const folds = [...conv.querySelectorAll<HTMLElement>(".thread-fold")];
  assert(folds.length === 2 && folds[1].getAttribute("aria-label")?.includes("steered once") === true,
    `The finished turn's work line reads ${folds[1]?.getAttribute("aria-label") ?? "nothing"}`);
  assert(conv.querySelectorAll(".thread-changes").length === 2
    && conv.querySelector(".thread-turn-foot.is-latest") !== null, "The finished turn lacks its card or its foot");
  assert(conv.textContent?.includes("The regression test passes.")
    && !conv.textContent.includes("Use five retries, not three"), "The finished turn did not fold its work");
  report.finished = folds[1].getAttribute("aria-label");
  // The work line opens its rows in place and folds them again; with motion
  // the rows play in and out, reduced they are simply there and gone.
  folds[1].click();
  await frames(1);
  assert(folds[1].getAttribute("aria-expanded") === "true"
    && conv.textContent.includes("Use five retries, not three"), "The finished work line did not open");
  assert(reduced === (conv.querySelector(".thread-reveal.is-opening") === null),
    "The opening work line's motion does not follow the reader's preference");
  await until(() => conv.querySelector(".thread-reveal") === null, "The work line never finished opening");
  folds[1].click();
  await frames(1);
  assert(folds[1].getAttribute("aria-expanded") === "false", "The work line did not say it folds");
  if (!reduced) {
    assert(conv.querySelector(".thread-reveal.is-closing") !== null, "The closing work line does not play out");
  }
  await until(() => !conv.textContent?.includes("Use five retries, not three"), "The work line did not fold again");

  // ---- your next message rises into place ----------------------------------
  const field = conv.querySelector<HTMLTextAreaElement>(".thread-composer-input");
  assert(field !== null, "The Threads composer is missing");
  flushSync(() => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(field, "Run it once more");
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
  flushSync(() => {
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  });
  await frames(1);
  assert(sends.length === 1, `The composer sent ${sends.length} times`);
  const sent = conv.querySelectorAll<HTMLElement>(".thread-user");
  assert(sent.length === 3, `Expected the sent prompt as a third turn, saw ${sent.length}`);
  if (reduced) {
    assert(conv.querySelector(".thread-user.is-sent") === null, "Reduced motion still raises the sent prompt");
  } else {
    assert(sent[2].classList.contains("is-sent") && getComputedStyle(sent[2]).animationName === "thread-sent",
      "The sent prompt does not rise into place");
    await wait(450);
    assert(!sent[2].classList.contains("is-sent"), "The sent prompt kept its rise");
  }
  return report;
}

/**
 * A MEASUREMENT, not a look (T3 reuse pass, 2026-10-09): a Threads pane over a
 * long history — 300 finished turns, 1,800 rows, about the longest session on
 * record — opened, streamed into and scrolled in a real Electron DOM. It
 * reports what each costs, so a decision about windowing the transcript rests
 * on numbers. The ceilings only catch a pathological regression; they are
 * several times what a quiet machine measures, because the gate shares it.
 */
export async function threadLongTranscriptFixture(root: Root, theme: string,
    chrome: string): Promise<Record<string, unknown>> {
  flushSync(() => root.render(null));
  setLook(theme, chrome);
  const TURNS = 300;
  const STREAM_UPDATES = 40;
  const base = view.threads.find(thread => thread.workspace?.mode !== "worktree") ?? view.threads[0];
  const past: ConvItem[] = [];
  const turns: ThreadTurn[] = [];
  for (let n = 0; n < TURNS; n += 1) {
    const prompt = `Turn ${n}: tighten the retry back-off and rerun the login tests`;
    const file = `${ROOT}/src/module-${n % 40}.ts`;
    past.push(
      { kind: "user", id: `long-u${n}`, text: prompt },
      { kind: "tool", id: `long-read${n}`, tool: "Read", summary: file, input: { file_path: file },
        result: { ok: true, summary: "Read 120 lines" } },
      { kind: "tool", id: `long-edit${n}`, tool: "Edit", summary: file, input: { file_path: file },
        result: { ok: true, summary: "Edited" } },
      { kind: "diff", id: `long-diff${n}`, path: file, callId: `long-edit${n}`, added: 6, removed: 2 },
      { kind: "tool", id: `long-bash${n}`, tool: "Bash", summary: "npm test -- login",
        input: { command: "npm test -- login" }, result: { ok: true, summary: "12 passed" } },
      { kind: "text", id: `long-a${n}`, streaming: false, text: `The back-off now doubles from 200 ms `
        + `(turn ${n}).\n\n- \`retry.ts\` waits before each attempt\n- the login test covers five `
        + "attempts\n\n```ts\nexport const RETRIES = 5;\nexport const backoff = (attempt: number) => "
        + "200 * 2 ** attempt;\n```\n\nAll 12 login tests pass." },
    );
    turns.push(turn(n, prompt, { filesChanged: 1, added: 6, removed: 2 }));
  }
  const livePrompt = "Now make the back-off configurable per call site";
  const runningTurns = [...turns, turn(TURNS, livePrompt, { state: "running", hasEnd: false })];
  let setLive: (live: { items: ConvItem[]; thinking: boolean }) => void = noop;
  function Harness() {
    const [live, setLiveState] = useState<{ items: ConvItem[]; thinking: boolean }>(
      { items: [], thinking: false });
    setLive = setLiveState;
    const props: ConversationProps = {
      items: live.items, live: true, thinking: live.thinking, focused: true, sessionKey: base.key,
      history: { status: "ready", items: past },
      config: { effort: null, models: [], canSetModel: false, effortLevels: [], showEffort: false,
        canSetEffort: false, pendingModel: false, pendingEffort: false, error: null },
      onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop,
      onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: new Set(),
      threadLayout: true, pathRoot: ROOT,
      thread: conversationThread({ ...base, turns: live.thinking ? runningTurns : turns }, false),
    };
    return <div className="tile-stack" style={{ width: 880, height: 780 }}><Conversation {...props} /></div>;
  }
  const time = (run: () => void): number => {
    const started = performance.now();
    run();
    return performance.now() - started;
  };
  const sorted = (values: number[]): number[] => [...values].sort((a, b) => a - b);
  const round = (value: number): number => Math.round(value * 10) / 10;

  // Opening the session: the commit, then the first painted frame.
  const openStarted = performance.now();
  const openCommitMs = time(() => { flushSync(() => root.render(<Harness />)); });
  await frames(2);
  const openFrameMs = performance.now() - openStarted;
  const pane = document.querySelector<HTMLElement>(".conv.is-thread-layout");
  assert(pane !== null, "The long thread did not mount in the Threads layout");
  const list = pane.querySelector<HTMLElement>(".conv-list");
  assert(list !== null, "The long thread has no scrolling list");
  const turnsDrawn = pane.querySelectorAll(".thread-user").length;
  assert(turnsDrawn === TURNS, `The long thread draws ${turnsDrawn} of ${TURNS} turns`);
  const elements = pane.querySelectorAll("*").length;

  // A turn streaming in at the end: each delta is one commit of the whole pane.
  const streamed: ConvItem[] = [{ kind: "user", id: "long-live-user", text: livePrompt }];
  let answer = "";
  const deltas: number[] = [];
  for (let update = 0; update < STREAM_UPDATES; update += 1) {
    answer += `Sentence ${update} of the answer explains one more part of the change. `;
    const items: ConvItem[] = [...streamed, { kind: "text", id: "long-live-text", text: answer, streaming: true }];
    deltas.push(time(() => { flushSync(() => { setLive({ items, thinking: true }); }); }));
    await frames(1);
  }
  const streaming = sorted(deltas);

  // Scrolling through it: the layout each jump forces.
  const scrolls: number[] = [];
  for (const at of [0.75, 0.5, 0.25, 0, 1]) {
    scrolls.push(time(() => {
      list.scrollTop = (list.scrollHeight - list.clientHeight) * at;
      void list.getBoundingClientRect().height;
    }));
    await frames(1);
  }

  // Leaving the session and coming back to it.
  flushSync(() => root.render(null));
  await frames(1);
  const reopenCommitMs = time(() => { flushSync(() => root.render(<Harness />)); });
  await frames(2);

  const report = {
    turns: TURNS, rows: past.length, elements,
    openCommitMs: round(openCommitMs), openFrameMs: round(openFrameMs),
    reopenCommitMs: round(reopenCommitMs),
    streamMedianMs: round(streaming[Math.floor(streaming.length / 2)]),
    streamP95Ms: round(streaming[Math.floor(streaming.length * 0.95)]),
    streamMaxMs: round(streaming[streaming.length - 1]),
    scrollMaxMs: round(Math.max(...scrolls)),
  };
  console.log(`thread long transcript: ${JSON.stringify(report)}`);
  assert(openFrameMs < 6000, `Opening ${TURNS} turns took ${round(openFrameMs)} ms`);
  assert(report.streamP95Ms < 400, `A streamed delta into ${TURNS} turns took ${report.streamP95Ms} ms (p95)`);
  assert(report.scrollMaxMs < 1000, `A scroll through ${TURNS} turns forced ${report.scrollMaxMs} ms of layout`);
  return report;
}

async function promptRecoveryChecks(root: Root): Promise<void> {
  const receipt: PromptRecoveryReceipt = { id: "receipt-1", key: "claude#recover", at: 1, delivery: "unknown", serialized: "serialized original",
    input: { text: "Publish the report", context: [{ id: "image", kind: "file", label: "Original sketch", value: "/private/images/sketch.png" }] } };
  const calls: string[] = [];
  const read = async () => ({ version: 1 as const, receipt, prepared: null });
  const recover = async (_key: string, mode: string, id: string) => { calls.push(`${mode}:${id}`); };
  flushSync(() => root.render(<PromptRecoveryPanel sessionKey={receipt.key} wake={1} interrupted={false} disabled={false} read={read} recover={recover} />));
  await until(() => document.querySelector(".prompt-recovery") !== null, "Recovery card did not render");
  assert(document.body.textContent!.includes("Continue safely") && !document.body.textContent!.includes("Retry preserved prompt"), "Unknown delivery exposed replay");
  assert(document.body.textContent!.includes("Original sketch"), "Recovery lost the attachment association");
  document.querySelector<HTMLButtonElement>(".prompt-recovery button")!.click(); await frames(3);
  assert(calls[0] === "continue:receipt-1", "Recovery did not name the current durable receipt");
  receipt.delivery = "unsent";
  flushSync(() => root.render(<PromptRecoveryPanel sessionKey={receipt.key} wake={2} interrupted={false} disabled={false} read={read} recover={recover} />));
  await until(() => document.body.textContent!.includes("Retry preserved prompt"), "Unsent receipt did not offer retry");
  document.querySelector<HTMLButtonElement>(".prompt-recovery button")!.click(); await frames(3);
  assert(calls[1] === "retry:receipt-1", "Definitely-unsent recovery did not use Retry");
  receipt.delivery = "accepted";
  const forget = async (key: string, id: string) => { calls.push(`forget:${key}:${id}`); };
  flushSync(() => root.render(<PromptRecoveryPanel sessionKey={receipt.key} wake={3} interrupted={false} disabled={false} read={read} recover={recover} forget={forget} />));
  await frames(3);
  assert(!document.querySelector(".prompt-recovery strong"), "Completed receipt showed a prominent interruption warning");
  document.querySelector<HTMLDetailsElement>(".prompt-recovery details")!.open = true;
  document.querySelector<HTMLButtonElement>(".prompt-recovery button")!.click(); await frames(2);
  assert(!calls.some(call => call.startsWith("forget:")), "Forget skipped confirmation");
  [...document.querySelectorAll<HTMLButtonElement>(".prompt-recovery button")].find(button => button.textContent === "Confirm forget")!.click(); await frames(3);
  assert(calls.includes("forget:claude#recover:receipt-1"), "Completed receipt could not be forgotten");
  const pending = { ...receipt, key: "pending-launch#test", delivery: "unknown" as const };
  const readPending = async (_key: string, list = false) => ({ version: 1 as const, receipt: list ? null : pending, prepared: null,
    pending: list ? [{ key: pending.key, id: pending.id, preview: pending.input.text, delivery: pending.delivery }] : [] });
  flushSync(() => root.render(<PendingLaunchRecoveryPanel wake={4} read={readPending} forget={forget} restore={async () => { calls.push("restore"); }} />));
  await until(() => document.querySelector(".prompt-recovery select option[value='pending-launch#test']") !== null, "Orphaned launch was not listed after reload");
  const choice = document.querySelector<HTMLSelectElement>(".prompt-recovery select")!;
  choice.value = pending.key; choice.dispatchEvent(new Event("change", { bubbles: true })); await frames(4);
  assert(document.body.textContent!.includes("Original sketch"), "Orphaned launch lost its preserved attachments");
  assert(!document.body.textContent!.includes("Restore unsent prompt") && !document.body.textContent!.includes("Continue safely"), "Unknown launch exposed an automatic replay action");
  const accepted = { ...pending, delivery: "accepted" as const };
  const readAccepted = async (_key: string, list = false) => ({ version: 1 as const, receipt: list ? null : accepted, prepared: null,
    pending: list ? [{ key: accepted.key, id: accepted.id, preview: accepted.input.text, delivery: accepted.delivery, sessionKey: "claude#already-created" }] : [] });
  flushSync(() => root.render(<PendingLaunchRecoveryPanel wake={5} read={readAccepted} forget={forget} restore={async () => { throw new Error("must not restore accepted"); }} openSession={key => { calls.push(`open:${key}`); }} />));
  await until(() => document.body.textContent!.includes("Open started thread"), "Bound launch lost its actual session association");
  [...document.querySelectorAll<HTMLButtonElement>(".prompt-recovery button")].find(button => button.textContent === "Open started thread")!.click(); await frames(2);
  assert(calls.includes("open:claude#already-created"), "Accepted launch could not open the existing thread");
  assert(!document.body.textContent!.includes("Restore unsent prompt"), "Accepted launch invited duplicate work");
  const unsent = { ...pending, delivery: "unsent" as const };
  const readUnsent = async (_key: string, list = false) => ({ version: 1 as const, receipt: list ? null : unsent, prepared: null,
    pending: list ? [{ key: unsent.key, id: unsent.id, preview: unsent.input.text, delivery: unsent.delivery }] : [] });
  flushSync(() => root.render(<PendingLaunchRecoveryPanel wake={6} read={readUnsent} forget={forget} restore={async () => { throw new Error("Original attachment is no longer safe to read; nothing was sent."); }} />));
  await until(() => document.body.textContent!.includes("Restore unsent prompt"), "Unsent launch could not be explicitly restored");
  [...document.querySelectorAll<HTMLButtonElement>(".prompt-recovery button")].find(button => button.textContent === "Restore unsent prompt to composer")!.click();
  await until(() => document.body.textContent!.includes("Original attachment is no longer safe"), "Restore authorization failure was hidden");
  assert(document.body.textContent!.includes("Original sketch"), "Failed restore discarded the preserved attachment");
  await keptPromptLineChecks(root);


}

/**
 * The kept prompt inside the composer's top edge (owner, 2026-11-15): one
 * line with the message's first words, its attachments counted, Retry or
 * Continue safely, Put it back and Discard; only when there is something to
 * recover, never for a message delivered and answered.
 */
async function keptPromptLineChecks(root: Root): Promise<void> {
  const receipt: PromptRecoveryReceipt = { id: "kept-1", key: "claude#kept", at: 1, delivery: "accepted", serialized: "s",
    input: { text: "Add a rollback step to the release checklist, then re-run the dry run against staging", context: [
      { id: "plan", kind: "file", label: "plan.md", value: "/private/atlas/plan.md" },
      { id: "src", kind: "folder", label: "src", value: "/private/atlas/src" }] } };
  const calls: string[] = [];
  let reads = 0;
  const read = async () => { reads++; return { version: 1 as const, receipt, prepared: null }; };
  const forget = async (key: string, id: string) => { calls.push(`forget:${key}:${id}`); };
  const recover = async (_key: string, mode: string, id: string) => { calls.push(`${mode}:${id}`); };
  const restore = (kept: PromptRecoveryReceipt) => { calls.push(`restore:${kept.id}`); };
  const paint = (wake: number, interrupted = false): void => {
    flushSync(() => root.render(<div className="thread-composer" style={{ width: 640 }}>
      <KeptPromptLine sessionKey={receipt.key} wake={wake} interrupted={interrupted} disabled={false}
        read={read} recover={recover} forget={forget} restore={restore} />
    </div>));
  };
  // Delivered and answered: the receipt is read, and no line is drawn.
  paint(1);
  await until(() => reads > 0, "The kept prompt was never read");
  await frames(3);
  assert(document.querySelector(".kept-prompt") === null, "A delivered, answered message still drew the kept line");
  // Not sent: the line, its count, Retry, Put it back and Discard.
  receipt.delivery = "unsent";
  paint(2);
  await until(() => document.querySelector(".kept-prompt") !== null, "A message that was not sent drew no line");
  const line = document.querySelector<HTMLElement>(".kept-prompt-line")!;
  assert(line.textContent === 'A message you wrote earlier was not sent: "Add a rollback step to the release…"· 2 attachmentsRetryPut it backDiscard',
    `The kept line reads ${line.textContent ?? "nothing"}`);
  assert(line.getBoundingClientRect().height < 24, "The kept prompt wraps onto a second line");
  assert(document.querySelector(".kept-prompt")!.closest(".thread-composer") !== null, "The line is not inside the composer");
  button("Retry", line).click(); await frames(3);
  assert(calls.includes("retry:kept-1"), "Retry did not name the kept receipt");
  document.querySelector<HTMLButtonElement>(".kept-prompt-words")!.click(); await frames(2);
  assert(document.querySelector(".kept-prompt-more pre")?.textContent === receipt.input.text
    && document.body.textContent!.includes("plan.md · file"), "The words did not open the whole message and its attachments");
  // Discard asks first, as Forget did.
  button("Discard", document.querySelector<HTMLElement>(".kept-prompt-line")!).click(); await frames(2);
  assert(!calls.some(call => call.startsWith("forget:")), "Discard skipped its confirmation");
  button("Keep it").click(); await frames(2);
  assert(document.querySelector(".kept-prompt-confirm") === null, "Keep it left the question open");
  button("Discard", document.querySelector<HTMLElement>(".kept-prompt-line")!).click(); await frames(2);
  button("Discard", document.querySelector<HTMLElement>(".kept-prompt-confirm")!).click(); await frames(3);
  assert(calls.includes("forget:claude#kept:kept-1"), "Discard did not forget the kept message");
  assert(document.querySelector(".kept-prompt") === null, "The line stayed after Discard");
  // May not have finished (a delivered message whose turn was interrupted):
  // Continue safely, and Put it back hands the receipt to the composer.
  receipt.id = "kept-2";
  receipt.delivery = "accepted";
  paint(3, true);
  await until(() => document.body.textContent!.includes("may not have finished"), "An interrupted message drew no line");
  button("Continue safely").click(); await frames(3);
  assert(calls.includes("continue:kept-2"), "Continue safely did not name the kept receipt");
  button("Put it back").click(); await frames(3);
  assert(calls.includes("restore:kept-2"), "Put it back did not restore the kept message");
  assert(document.querySelector(".kept-prompt") === null, "The line stayed after Put it back");
  // Outcome unknown: it may not have finished either.
  receipt.id = "kept-3";
  receipt.delivery = "unknown";
  paint(4);
  await until(() => document.body.textContent!.includes("may not have finished"), "An unknown outcome drew no line");
  flushSync(() => root.render(null));
}
