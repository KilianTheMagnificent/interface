import {
  AgentPastThreadNotice, AgentWorkspacePanel, type AgentRequestHandler, type AgentRequestResult,
} from "../components/AgentWorkspacePanel";
import { agentApprovalGateFor } from "../agent-roster";
import {
  emptyAgentControlResponse, type AgentControlResponse, type AgentDecision, type AgentFiles, type AgentMutation,
  type AgentRequest, type AgentRoutine,
} from "../../shared/agents";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { AgentHeader, AgentMenu, AgentProfileDialog, AGENT_MENU_CSS } from "../components/AgentMenu";
import { Sidebar } from "../components/Sidebar";
import { AGENT_COLORS } from "../agent-profiles";
import { AgentTask } from "../components/AgentTask";
import { Conversation, type ConversationProps } from "../components/Conversation";
import { seedLaunchForm } from "../composer-model";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { parseAgentProfiles, serializeAgentProfiles, type AgentProfile } from "../agent-profiles";
import type { ConvItem } from "../conversation-reducer";
import type { EngineEvent, ModelOption, SessionSummary } from "../../shared/engine";
import { assert, frames as settled, noop } from "./fixture-support";

// The fixture's transcripts are items; AgentTask reads them as stored history.
const NO_EVENTS: EngineEvent[] = [];
async function assertReachable(element: HTMLElement, message: string): Promise<void> {
  // The hidden measuring pass can take more than two hidden-window frames to
  // invalidate inherited visibility on nested option labels in Chromium.
  const labels = [element, ...element.querySelectorAll<HTMLElement>('span')];
  const deadline = performance.now() + 1000;
  while (labels.some(row => getComputedStyle(row).visibility !== "visible") && performance.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  const rect = element.getBoundingClientRect();
  const hit = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
  const menu = element.closest<HTMLElement>('.app-select-popover');
  assert(labels.every(row => getComputedStyle(row).visibility === "visible")
    && rect.width > 0 && rect.height > 0 && hit !== null && element.contains(hit),
    `${message}: ${JSON.stringify({ rect: rect.toJSON(), hit: hit?.className,
      menu: menu?.className, zIndex: menu && getComputedStyle(menu).zIndex })}`);
}
const catalog: ModelOption[] = [
  { value: "reasoner", displayName: "Reasoner", supportsEffort: true, effortLevels: ["low", "high"] },
  { value: "fast", displayName: "Fast", supportsEffort: false, effortLevels: [] },
];
const profile: AgentProfile = { id: "fixture-agent", name: "Research assistant", engine: "codex",
  model: "reasoner", effort: "low", color: "teal", instructions: "Cite primary sources." };
const session: SessionSummary = { engine: "codex", sessionId: "agent-fixture", title: "Prepare the launch briefing",
  cwd: "/fixture/research", status: "working", cloud: false, model: "reasoner", archived: false,
  lastActivity: "2026-09-07T12:00:00Z", firstSeen: "2026-09-07T12:00:00Z", kind: "agent" };
/* A teammate: its own folder, two granted folders and one thread whose last
   request is still with Interface's approval policy. */
const home = "/fixture/agents/Research assistant";
const teammate: AgentProfile = { ...profile, home, folders: ["/fixture/research", "/fixture/site"] };
const threadKey = "codex#agent-fixture";
const teammateResponse = (over: Partial<AgentControlResponse> = {}): AgentControlResponse => ({
  ...emptyAgentControlResponse(), agents: [teammate],
  threads: { [teammate.id]: {
    current: { sessionKey: threadKey, startedAtMs: 1788958900000, status: "waiting", interrupted: false },
    past: [{ sessionKey: "codex#agent-fixture-earlier", startedAtMs: 1788958000000, endedAtMs: 1788958500000 }],
    lastReadAtMs: 1788958900000, lastActivityAtMs: 1788958900000,
  } },
  statuses: { [teammate.id]: "working" },
  ...over,
});
const answered = (over: Partial<AgentControlResponse> = {}): AgentRequestResult =>
  ({ ok: true, response: teammateResponse(over) });
const refusedWith = (error: string): AgentRequestResult => ({ ok: false, error });
/** The props App supplies to every page, set to the thread on screen and a visible surface. */
const pageProps = {
  pending: false, error: null, shownSessionKey: threadKey, visible: true, onOpenTask: noop, onOpenThread: noop,
  onShowThread: noop, onCommand: async () => true, onPickDirectory: async () => null,
  onRequest: (async () => answered()) as AgentRequestHandler,
};
const teammateFiles = (text: string, sha256: string): AgentFiles => ({
  home, exists: true, markerOk: true, instructionsConflict: false,
  instructions: { text: profile.instructions, sha256: "instructions-sha", tooLarge: false },
  memory: { text, sha256, tooLarge: false },
});
const decisionRow = (over: Partial<AgentDecision>): AgentDecision => ({ atMs: 1788958910000, sessionKey: threadKey,
  approvalId: "decision", tool: "Bash", summary: "Run the build", decision: "allowed", reason: "", outcome: "sent",
  facts: { kind: "shell", command: "npm run build", cwd: "/fixture/research" }, ...over });
const decisions: AgentDecision[] = [
  decisionRow({ approvalId: "d1", summary: "Read the research brief", tool: "Read", facts: { kind: "read", paths: ["/fixture/research/brief.md"] } }),
  decisionRow({ approvalId: "d2", atMs: 1788958920000, summary: "Search the product documentation", tool: "WebSearch", decision: "asked", facts: { kind: "webSearch" }, outcome: "pending" }),
  decisionRow({ approvalId: "d3", atMs: 1788958930000, summary: "Push the briefing branch", decision: "ownerAllowed" }),
  decisionRow({ approvalId: "d4", atMs: 1788958940000, summary: "Delete the old export", decision: "ownerDenied", outcome: "unsent" }),
  decisionRow({ approvalId: "d5", atMs: 1788958950000, summary: "Call an unrecognized tool", tool: "mcp__docs__publish", decision: "failedClosed", facts: { kind: "unknown", tool: "publish" }, outcome: "unsent" }),
];
const checkingApproval: ConvItem = { kind: "approval", id: "checking-fixture", tool: "Bash",
  summary: "npm run build", input: { command: "npm run build" } };
async function waitFor<T>(read: () => T | null | undefined | false, message: string): Promise<T> {
  const deadline = performance.now() + 2000;
  for (;;) {
    const value = read();
    if (value) return value;
    assert(performance.now() < deadline, message);
    await new Promise(resolve => setTimeout(resolve, 20));
  }
}
function typeInto(field: HTMLInputElement | HTMLTextAreaElement, text: string): void {
  const prototype = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(field, text);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}
const items: ConvItem[] = [
  { kind: "user", id: "request", text: "Compare the two agent interfaces and prepare a brief with sources." },
  { kind: "text", id: "answer", text: "I found the official design documentation for both products.\n\nThe briefing will separate **model choice**, **tool access**, and **persistent agent identity** so each decision has clear evidence.", streaming: false },
  { kind: "tool", id: "read", tool: "Read", input: { path: "/fixture/research/brief.md" }, summary: "Read research/brief.md", result: { ok: true, summary: "Read complete" } },
];

async function choose(label: string, text: string): Promise<void> {
  const trigger = document.querySelector<HTMLButtonElement>(`button[aria-label^="${label}:"]`);
  assert(trigger, `Missing ${label} picker`);
  trigger.scrollIntoView({ block: "nearest" });
  trigger.click(); await settled();
  const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')]
    .find(row => !row.hidden && row.textContent?.includes(text));
  assert(option, `Missing ${label} option ${text}`);
  await assertReachable(option, `${label} option ${text} is covered by another surface`);
  option.click(); await settled();
}

export async function agentWorkspaceInteractions(root: Root, base: ConversationProps): Promise<unknown> {
  let saved: AgentProfile | null = null;
  const mount = (initial: AgentProfile): void => flushSync(() => root.render(<>
    <style>{APP_SELECT_CSS}</style>
    <AgentProfileDialog key={initial.id} open initial={initial}
      engines={[{ id: "codex", label: "Codex", available: true }, { id: "acp:local", label: "Local", available: true }]}
      modelsFor={engine => engine === "codex" ? catalog : []} effortLevelsFor={() => ["low", "high"]}
      onSubmit={value => { saved = value; }} onCancel={noop} />
  </>));
  mount(profile); await settled();
  // Name, what it does and the icon come first; provider settings wait behind
  // Advanced, and the nine shapes fit without scrolling sideways.
  const advanced = document.querySelector<HTMLDetailsElement>('.agent-dialog-advanced');
  assert(advanced && !advanced.open, "Provider settings should wait behind Advanced");
  const dialogBox = document.querySelector<HTMLElement>('.agent-dialog')!;
  assert(dialogBox.scrollWidth <= dialogBox.clientWidth, "Agent icon choices overflow the dialog");
  document.querySelector<HTMLElement>('.agent-dialog-advanced-summary')!.click(); await settled();
  assert(advanced.open, "Advanced did not open");
  assert(document.querySelector('[aria-label="Model: Reasoner"]'), "Saved catalog model was not selected");
  await choose("Model", "Fast");
  assert(!document.querySelector('[aria-label^="Effort:"]'), "Effort axis leaked onto a model without effort");
  await choose("Model", "Reasoner");
  await choose("Effort", "high");
  document.querySelector<HTMLFormElement>('form')!.requestSubmit(); await settled();
  const selected = saved as AgentProfile | null;
  assert(selected?.model === "reasoner" && selected.effort === "high", "Saved model/effort did not match selection");
  const restored = parseAgentProfiles(serializeAgentProfiles({ agents: [selected], assignments: {} })).agents[0];
  assert(restored.model === selected.model && restored.effort === selected.effort, "Profile round trip lost model identity");
  const launch = seedLaunchForm({ engine: restored.engine, model: restored.model, effort: restored.effort,
    cwd: "/fixture", prompt: "Fixture task", permissionMode: "default" },
    { models: catalog.map(row => row.value), modelOptions: catalog, efforts: ["low", "high"], permissionModes: ["default"] });
  assert(launch.model === "reasoner" && launch.effort === "high", "Launch seeding changed the profile model/effort");
  await choose("Provider", "Local");
  assert(document.querySelector('[aria-label="Model: Choose at launch"]'), "Provider switch carried another provider's model");
  assert(document.querySelector('[aria-label="Effort: Default"]'), "Provider switch carried another provider's effort");
  await choose("Model", "Custom model ID");
  const custom = document.querySelector<HTMLInputElement>('input[placeholder="Model ID from your provider"]');
  assert(custom, "Custom model input missing");
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(custom, "local/model-v2");
  custom.dispatchEvent(new Event("input", { bubbles: true })); await settled();
  document.querySelector<HTMLFormElement>('form')!.requestSubmit(); await settled();
  const customSaved = saved as AgentProfile | null;
  assert(customSaved?.model === "local/model-v2" && customSaved.engine === "acp:local", "Custom model was silently replaced");
  mount({ ...customSaved, id: "reopened-custom" }); await settled();
  assert(document.querySelector<HTMLInputElement>('input[placeholder="Model ID from your provider"]')?.value === "local/model-v2", "Unlisted custom model disappeared on reopen");

  await agentRosterVisual(root, base, "light");
  const rows = [...document.querySelectorAll<HTMLButtonElement>('.agent-menu-row')];
  const order = rows.map(row => row.title);
  rows[2]!.focus();
  rows[2]!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  assert(document.activeElement === rows[3], "Agent roster arrow navigation failed");
  // The box opens from the header's filter glyph, like Tasks' (owner, 2026-09-28).
  document.querySelector<HTMLButtonElement>('button[aria-label="Filter agents"]')!.click(); await settled();
  const search = document.querySelector<HTMLInputElement>('input[aria-label="Search agents"]')!;
  assert(document.activeElement === search, "Opening agent search did not focus its box");
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(search, "Career");
  search.dispatchEvent(new Event("input", { bubbles: true })); await settled();
  assert(document.querySelectorAll('.agent-menu-row').length === 1
    && document.querySelector('.agent-menu-row')?.getAttribute('title') === "Career", "Agent search did not filter names");
  search.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); await settled();
  assert([...document.querySelectorAll<HTMLButtonElement>('.agent-menu-row')].map(row => row.title).join() === order.join(),
    "Searching agents changed their saved order");
  search.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); await settled();
  assert(!document.querySelector('input[aria-label="Search agents"]'), "Escape on an empty agent search left the box open");

  await agentWorkspaceVisual(root, base, "dark");
  const conversation = document.querySelector('.agent-task-conversation .conv');
  const inspector = document.querySelector<HTMLElement>('.agent-task-inspector');
  assert(conversation && inspector && !inspector.hidden, "Inspector or conversation missing");
  document.querySelector<HTMLButtonElement>('.agent-task-inspect')!.click(); await settled();
  assert(inspector.hidden && document.querySelector('.agent-task-conversation .conv') === conversation,
    "Closing inspector destroyed or hid the conversation");
  const textarea = document.querySelector<HTMLTextAreaElement>('.conv-composer textarea');
  assert(textarea && textarea.getBoundingClientRect().height > 0, "Composer is not visible");
  const checkingCard = [...document.querySelectorAll<HTMLElement>('.conv-approval')]
    .find(card => card.textContent?.includes("Checking with Interface's approval policy…"));
  assert(checkingCard && checkingCard.querySelector('button') === null && !checkingCard.querySelector('[data-approval-key]'),
    "An approval the policy is still checking offered verdict controls");
  assert([...document.querySelectorAll('.agent-thread-bar button')].some(button => button.textContent === "New conversation"),
    "The conversation lost its New conversation action");

  let approved = false;
  const approval: ConvItem = { kind: "approval", id: "approve-fixture", tool: "Write", summary: "Write the briefing", input: {} };
  flushSync(() => root.render(<div style={{ height: "100vh" }}><AgentTask session={{ ...session, status: "waiting" }}
    events={NO_EVENTS} history={{ items: [approval] }} engineLabel="Codex" live turnStartedAt={null} now={0}
    onStop={noop} onOpenDiff={noop}>
    <Conversation {...base} items={[approval]} activeApprovalIds={new Set([approval.id])}
      onApprove={() => { approved = true; }} />
  </AgentTask></div>)); await settled();
  const allow = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.trim().startsWith("Allow"));
  assert(allow && allow.getBoundingClientRect().height > 0, "Approval is hidden behind a disclosure");
  allow.click(); await settled(); assert(approved, "Approval did not reach the session callback");
  await agentRoutineVisual(root, "light");
  const buttons = (): HTMLButtonElement[] => [...document.querySelectorAll<HTMLButtonElement>('button')];
  buttons().find(button => button.textContent === "New routine")!.click(); await settled();
  const form = document.querySelector<HTMLFormElement>('.agent-routine-form');
  assert(form, "Routine form missing");
  const enabled = form.querySelector<HTMLInputElement>('input[type=checkbox]');
  assert(enabled && !enabled.checked, "New routine must start paused");
  const cwd = [...form.querySelectorAll('label')].find(label => label.textContent?.startsWith("Working folder"))
    ?.querySelector('input');
  assert(cwd?.value === "" && cwd.placeholder.includes("Agent home"), "A new routine must let Rust resolve the agent home on save");
  buttons().find(button => button.textContent?.startsWith("Runs"))!.click(); await settled();
  assert(document.querySelector('.agent-workspace-body')?.textContent?.includes("No runs yet"), "Run empty state missing");

  // The first message creates the thread, keeps a refused draft, and retries as the same request.
  const requests: AgentRequest[] = [];
  flushSync(() => root.render(<div className="agent-surface-stack" style={{ height: "100vh" }}>
    <style>{AGENT_MENU_CSS}</style>
    <AgentWorkspacePanel key="first-message" agent={teammate} response={{ ...emptyAgentControlResponse(), agents: [teammate] }}
      {...pageProps} shownSessionKey={null}
      onRequest={async command => { requests.push(command); return refusedWith("Codex is not signed in."); }}>
      <p>Thread conversation</p>
    </AgentWorkspacePanel>
  </div>)); await settled();
  const first = document.querySelector<HTMLTextAreaElement>(`#agent-thread-first-${teammate.id}`);
  assert(first && document.querySelector(`label[for="agent-thread-first-${teammate.id}"]`)?.textContent
    === `Message ${teammate.name}`, "The first-message composer lost its label");
  typeInto(first, "Summarize the launch plan."); await settled();
  const send = document.querySelector<HTMLButtonElement>('.agent-thread-first button[type=submit]')!;
  send.click(); await settled();
  await waitFor(() => requests.length === 1 && !send.disabled, "Send did not reach the host");
  const started = requests[0];
  assert(started?.kind === "startAgentThread" && started.agentId === teammate.id
    && started.text === "Summarize the launch plan.", "Send did not start the thread with the typed text");
  assert(first.value === "Summarize the launch plan.", "A refused first message lost its draft");
  assert(document.querySelector('.agent-thread-first [role="alert"]')?.textContent === "Codex is not signed in.",
    "A refused first message did not say why beside Send");
  send.click(); await settled();
  await waitFor(() => requests.length === 2, "Retrying the first message did not reach the host");
  const retried = requests[1];
  assert(retried?.kind === "startAgentThread" && retried.requestId === started.requestId,
    "Retrying an unanswered first message minted a second request");

  const memory = await agentMemoryVisual(root, "light");
  const computer = await agentComputerVisual(root, "dark");
  const shownSessions = await agentShownSessions(root, base);
  const restartNotice = await agentRestartNotice(root);
  flushSync(() => root.render(null));
  return { catalogSelection: true, customModel: true, providerReset: true, profileRoundTrip: true,
    visibleApproval: true, stableConversation: true, searchableRoster: true, rosterKeyboard: true,
    checkingApproval: true, firstMessage: true, memory, computer, shownSessions, restartNotice };
}

export async function agentRosterVisual(root: Root, base: ConversationProps, theme: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  const names = ["Research assistant", "Inbox triage", "Career", "School helper", "Release manager", "Writing partner"];
  const profiles = AGENT_COLORS.map((color, i) => ({ ...profile, color, id: `roster-${i}`, name: names[i]!,
    instructions: ["Find primary sources and prepare a brief", "Summarize the messages that need attention", "Prepare for the next opportunity", "Plan the week ahead", "Check the next release", "Help shape the first draft"][i]! }));
  const latestTasks = Object.fromEntries(profiles.slice(0, 4).map((agent, i) => [agent.id, { ...session,
    title: agent.instructions, lastActivity: `2026-09-07T${String(12 - i).padStart(2, "0")}:00:00Z` }]));
  // One agent in each status, and the first one previewing its thread.
  const statuses = { [profiles[0]!.id]: "working", [profiles[1]!.id]: "needsAttention", [profiles[2]!.id]: "unread" } as const;
  const rosterThreadKey = "codex#roster-thread";
  const threads = { [profiles[0]!.id]: { current: { sessionKey: rosterThreadKey, startedAtMs: 1788958900000,
    status: "working" as const, interrupted: false }, past: [], lastReadAtMs: 0, lastActivityAtMs: 1788958900000 } };
  let selected = profiles[3]!;
  const render = (): void => flushSync(() => root.render(<div style={{ display: "flex", height: "100vh" }}>
    <style>{`.agent-roster-fixture-sidebar > .sidebar { width:100%; min-width:0; }`}</style>
    <div className="agent-roster-fixture-sidebar" style={{ width: innerWidth < 800 ? 240 : 294, height: "100%", flex: "none", display: "flex" }}>
      <Sidebar groups={[]} rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
        importable={0} onToggleFilters={noop} onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop}
        sectionLabel="Tasks" newLabel="New task" zeroStateText="Your tasks appear here."
        agentMenu={<AgentMenu agents={profiles} selectedId={selected.id} latestTasks={latestTasks}
          statuses={statuses} threads={threads} threadPreviews={{ [rosterThreadKey]: "Drafted the launch brief with sources" }}
          now={Date.parse("2026-09-07T12:30:00Z")} onSelect={id => {
            selected = profiles.find(agent => agent.id === id)!; render();
          }} onCreate={noop} />} />
    </div>
    <div className="agent-surface-stack" style={{ flex: 1, minWidth: 0 }}>
      <AgentHeader profile={selected} engineLabel="Codex" workingCount={1} onEdit={noop} onDelete={noop} />
      <AgentTask session={session} events={NO_EVENTS} history={{ items }} engineLabel="Codex" live
        turnStartedAt={null} now={0} onStop={noop} onOpenDiff={noop}>
        <Conversation {...base} items={items} />
      </AgentTask>
    </div>
  </div>));
  render(); await settled(); await document.fonts.ready;
  const first = document.querySelector<HTMLElement>('.agent-menu-row')!;
  await assertReachable(first, "Agent row is covered");
  const marks = [...document.querySelectorAll<HTMLElement>('.agent-menu-row .agent-menu-status')];
  assert(marks.map(mark => `${mark.closest<HTMLElement>('.agent-menu-row')?.title}:${mark.dataset.status}`).join()
    === "Research assistant:working,Inbox triage:needsAttention,Career:unread", "Roster status marks are missing or misplaced");
  for (const mark of marks) {
    const box = mark.getBoundingClientRect();
    assert(box.width > 0 && box.height > 0 && mark.getAttribute("aria-label"), `${mark.dataset.status} mark has no size or label`);
  }
  assert(getComputedStyle(marks[2]!).backgroundColor !== "rgba(0, 0, 0, 0)", "The unread mark lost the tray's dot");
  assert(first.querySelector('.agent-menu-row-preview')?.textContent === "Drafted the launch brief with sources",
    "The roster did not preview the agent's thread");
  const before = [...document.querySelectorAll<HTMLButtonElement>('.agent-menu-row')].map(row => row.title).join();
  first.click(); await settled();
  assert(document.querySelector('.agent-header-name')?.textContent === "Research assistant", "Agent selection did not reach its destination");
  assert([...document.querySelectorAll<HTMLButtonElement>('.agent-menu-row')].map(row => row.title).join() === before,
    "Selecting an agent moved it in the roster");
  assert(document.documentElement.scrollWidth <= innerWidth, "Agent roster layout overflows horizontally");
  // Let inherited SVG paint and the selection wash reach the screenshot too;
  // textContent can update a hidden Chromium frame before its computed fill.
  const deadline = performance.now() + 1000;
  const avatar = document.querySelector<SVGPathElement>('.agent-surface-head svg path')!;
  while (getComputedStyle(avatar).fill !== "rgb(71, 155, 145)" && performance.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert(getComputedStyle(avatar).fill === "rgb(71, 155, 145)", "Selected agent avatar did not repaint");
  await settled();
  return { width: innerWidth, characters: document.querySelectorAll('.agent-menu-row svg').length, stableOrder: true };
}

export async function agentWorkspaceVisual(root: Root, base: ConversationProps, theme: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  flushSync(() => root.render(<div style={{ height: "100vh", display: "flex", flexDirection: "column" }}>
    <style>{AGENT_MENU_CSS}</style><style>{APP_SELECT_CSS}</style>
    <AgentHeader profile={profile} engineLabel="Codex" workingCount={1} onEdit={noop} onDelete={noop} />
    <AgentWorkspacePanel agent={teammate} response={teammateResponse()} {...pageProps}>
    <AgentTask key={theme} session={session} events={NO_EVENTS} engineLabel="Codex" live
      history={{ items: [...items, checkingApproval],
        plan: [{ content: "Compare official documentation", priority: "medium", status: "completed" }, { content: "Write the briefing", priority: "medium", status: "in_progress" }] }}
      commands={[{ name: "research", description: "Find primary sources and prepare a cited brief." }]}
      turnStartedAt={null} now={0} onStop={noop} onOpenDiff={noop}>
      <Conversation {...base} items={[...items, checkingApproval]} activeApprovalIds={new Set([checkingApproval.id])}
        approvalGate={() => "checking"} />
    </AgentTask>
    </AgentWorkspacePanel>
  </div>)); await settled();
  const toggle = document.querySelector<HTMLButtonElement>('.agent-task-inspect')!;
  if (toggle.getAttribute('aria-expanded') !== 'true') { toggle.click(); await settled(); }
  await document.fonts.ready;
  const prompt = document.querySelector<HTMLElement>('.conv-composer')!.getBoundingClientRect();
  const task = document.querySelector<HTMLElement>('.agent-task')!.getBoundingClientRect();
  assert(prompt.height > 0 && prompt.bottom <= innerHeight + 1, "Agent composer clipped vertically");
  assert(task.left >= 0 && task.right <= innerWidth + 1, "Agent task overflows viewport");
  assert(document.documentElement.scrollWidth <= innerWidth, "Agent workspace has horizontal overflow");
  const inspector = getComputedStyle(document.querySelector('.agent-task-inspector')!);
  return { width: innerWidth, promptHeight: prompt.height, inspectorVisible: true,
    inspectorBorder: { width: inspector.borderTopWidth, color: inspector.borderTopColor } };
}

export async function agentProfileVisual(root: Root, theme: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  flushSync(() => root.render(<><style>{APP_SELECT_CSS}</style>
    <AgentProfileDialog key={theme} open initial={profile}
      engines={[{ id: "codex", label: "Codex", available: true }]}
      modelsFor={() => catalog} effortLevelsFor={() => ["low", "high"]}
      onSubmit={noop} onCancel={noop} />
  </>));
  await settled(); await document.fonts.ready;
  const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!.getBoundingClientRect();
  assert(dialog.left >= 0 && dialog.right <= innerWidth && dialog.top >= 0 && dialog.bottom <= innerHeight,
    "Agent profile dialog clipped by viewport");
  document.querySelector<HTMLElement>('.agent-dialog-advanced-summary')!.click(); await settled();
  const modelTrigger = document.querySelector<HTMLButtonElement>('[aria-label="Model: Reasoner"]')!;
  modelTrigger.scrollIntoView({ block: "nearest" });
  modelTrigger.click();
  await settled();
  const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')]
    .find(row => row.textContent?.includes("Fast"));
  assert(option, "Model catalog did not open");
  await assertReachable(option, "Model catalog is covered by the dialog");
  return { width: innerWidth, height: dialog.height, catalogOpen: true };
}

export async function agentRoutineVisual(root: Root, theme: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  const routine: AgentRoutine = { id: "brief", agentId: profile.id, name: "Morning research brief",
    prompt: "Prepare a brief with primary sources.", cwd: "/fixture/research", enabled: false,
    startAtMs: 1788958800000, intervalMinutes: 1440, nextRunAtMs: 1788958800000 };
  let command: AgentMutation | null = null;
  flushSync(() => root.render(<div className="agent-surface-stack" style={{ height: "100vh" }}>
    <style>{AGENT_MENU_CSS}</style>
    <AgentHeader profile={profile} engineLabel="Codex" workingCount={0} onEdit={noop} onDelete={noop} />
    <AgentWorkspacePanel key={`routine-${theme}-${innerWidth}`} agent={teammate}
      response={{ ...emptyAgentControlResponse(), agents: [teammate], routines: [routine] }} {...pageProps}
      shownSessionKey={null} onCommand={async next => { command = next; return true; }}
      onPickDirectory={async () => "/fixture/research"}>
      <p>Task conversation</p>
    </AgentWorkspacePanel>
  </div>)); await settled();
  [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.startsWith("Routines"))!.click(); await settled();
  // Enable is a switch on the row (owner, 2026-09-12); its label carries the verb.
  const enable = document.querySelector<HTMLInputElement>('.agent-switch input[role="switch"]')!;
  assert(enable && !enable.checked && enable.getAttribute("aria-label")?.startsWith("Enable "), "Paused routine lost its Enable switch");
  await assertReachable(enable.closest<HTMLElement>('.agent-switch')!, "Routine controls are covered");
  enable.click(); await settled();
  const received = command as AgentMutation | null;
  assert(received?.kind === "saveRoutine" && received.routine.enabled, "Enable did not emit the exact routine command");
  assert(document.documentElement.scrollWidth <= innerWidth, "Routine workspace overflows horizontally");
  return { width: innerWidth, pausedDraft: true, enableCommand: true };
}

/** Memory: MEMORY.md changes on disk while it is open, so the save is refused and the page reloads it. */
export async function agentMemoryVisual(root: Root, theme: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  const disk = { text: "Cite primary sources in every brief.", sha256: "memory-1", refuse: null as string | null };
  const requests: AgentRequest[] = [];
  flushSync(() => root.render(<div className="agent-surface-stack" style={{ height: "100vh" }}>
    <style>{AGENT_MENU_CSS}</style>
    <AgentHeader profile={teammate} engineLabel="Codex" workingCount={0} onEdit={noop} onDelete={noop} />
    <AgentWorkspacePanel key={`memory-${theme}-${innerWidth}`} agent={teammate} response={teammateResponse()}
      {...pageProps}
      onRequest={async command => {
        requests.push(command);
        if (command.kind === "readAgentFiles") return answered({ files: teammateFiles(disk.text, disk.sha256) });
        if (command.kind === "saveAgentMemory") {
          if (disk.refuse !== null) return refusedWith(disk.refuse);
          return command.expectedSha256 === disk.sha256 ? answered()
            : refusedWith("MEMORY.md changed since it was opened.");
        }
        return answered();
      }}>
      <p>Thread conversation</p>
    </AgentWorkspacePanel>
  </div>)); await settled();
  [...document.querySelectorAll<HTMLButtonElement>('.agent-workspace-nav button')]
    .find(button => button.textContent === "Memory")!.click();
  const editor = await waitFor(() => document.querySelector<HTMLTextAreaElement>(`#agent-memory-${teammate.id}`),
    "MEMORY.md did not load");
  assert(editor.value === disk.text, "MEMORY.md opened with the wrong text");
  assert(document.querySelector('.agent-workspace-body')?.textContent?.includes(home), "Memory does not show the agent's folder");
  // The agent writes its own note while the owner is typing.
  disk.text = "Cite primary sources in every brief.\nThe owner prefers short summaries.";
  disk.sha256 = "memory-2";
  typeInto(editor, "Cite primary sources in every brief.\nMy edit."); await settled();
  [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === "Save memory")!.click();
  await waitFor(() => document.querySelector('.agent-workspace-body')?.textContent
    ?.includes("MEMORY.md changed since you opened it. Reloaded."), "A conflicting save did not reload and say so");
  assert(editor.value === disk.text, "The conflict did not reload MEMORY.md as it now is");
  const save = requests.find(request => request.kind === "saveAgentMemory");
  assert(save?.kind === "saveAgentMemory" && save.expectedSha256 === "memory-1", "Save did not carry the hash it opened with");
  assert(requests.filter(request => request.kind === "saveAgentMemory").length === 1, "A refused save was replayed");
  // A refusal that is not a conflict keeps its reason and the draft through the follow-up read.
  disk.refuse = "MEMORY.md could not be written: the disk is full.";
  typeInto(editor, `${disk.text}\nAnother edit.`); await settled();
  const readsBefore = requests.filter(request => request.kind === "readAgentFiles").length;
  [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === "Save memory")!.click();
  await waitFor(() => requests.filter(request => request.kind === "readAgentFiles").length === readsBefore + 1,
    "A refused save did not re-read MEMORY.md");
  await settled(); await settled();
  const saveAlert = document.querySelector('.agent-memory-form [role="alert"]');
  assert(saveAlert?.textContent === disk.refuse, "A non-conflict save refusal vanished after the follow-up read");
  assert(editor.value === `${disk.text}\nAnother edit.`, "A non-conflict save refusal lost the draft");
  // A draft over 256 KiB is refused before anything is sent.
  const savesBefore = requests.filter(request => request.kind === "saveAgentMemory").length;
  typeInto(editor, "€".repeat(87_382)); await settled();
  [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === "Save memory")!.click();
  await waitFor(() => document.querySelector('.agent-memory-form [role="alert"]')?.textContent
    === "Memory is larger than 256 KiB", "An oversized draft was not refused before sending");
  assert(requests.filter(request => request.kind === "saveAgentMemory").length === savesBefore,
    "An oversized draft reached the host");
  typeInto(editor, disk.text); await settled();
  assert(document.documentElement.scrollWidth <= innerWidth, "Memory overflows horizontally");
  return { width: innerWidth, conflictReloaded: true, refusalKept: true, oversizeRefused: true };
}

/** Computer: the home folder, granted folders with add and remove, what it asks before, and recent decisions. */
export async function agentComputerVisual(root: Root, theme: string): Promise<unknown> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = "refined";
  const commands: AgentMutation[] = [];
  flushSync(() => root.render(<div className="agent-surface-stack" style={{ height: "100vh" }}>
    <style>{AGENT_MENU_CSS}</style>
    <AgentHeader profile={teammate} engineLabel="Codex" workingCount={0} onEdit={noop} onDelete={noop} />
    <AgentWorkspacePanel key={`computer-${theme}-${innerWidth}`} agent={teammate} response={teammateResponse()}
      initialView="computer" {...pageProps}
      onCommand={async command => { commands.push(command); return true; }}
      onRequest={async command => command.kind === "readAgentDecisions" && command.limit === 50
        ? answered({ decisions }) : answered()}
      onPickDirectory={async () => "/fixture/docs"}>
      <p>Thread conversation</p>
    </AgentWorkspacePanel>
  </div>)); await settled();
  await waitFor(() => document.querySelectorAll('.agent-decision').length === decisions.length, "Recent decisions did not load");
  const body = document.querySelector('.agent-workspace-body')!.textContent ?? "";
  for (const text of [home, "/fixture/research", "/fixture/site", "Allowed", "Asked you", "You allowed", "You denied",
    "Refused on error",
    "Requests approval for recognized risky actions.",
    "Other providers cannot start gated agents."]) {
    assert(body.includes(text), `Computer is missing: ${text}`);
  }
  [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === "Add folder")!.click();
  await waitFor(() => commands.length === 1, "Add folder sent nothing");
  const added = commands[0];
  assert(added?.kind === "saveAgentFolders" && added.folders.join() === "/fixture/research,/fixture/site,/fixture/docs",
    "Add folder did not save the picked folder beside the granted ones");
  document.querySelector<HTMLButtonElement>('button[aria-label="Remove /fixture/site"]')!.click();
  await waitFor(() => commands.length === 2, "Remove sent nothing");
  const removed = commands[1];
  assert(removed?.kind === "saveAgentFolders" && removed.folders.join() === "/fixture/research",
    "Remove did not save the remaining folders");
  assert(document.documentElement.scrollWidth <= innerWidth, "Computer overflows horizontally");
  return { width: innerWidth, folders: teammate.folders?.length, decisions: decisions.length };
}

/**
 * The page shows whatever session App focused. A run's pending approval opened
 * from the tray and an older task opened from navigation stay on screen through
 * a remount, and only the Thread tab asks for the thread. A past thread is
 * read-only with its way back.
 */
export async function agentShownSessions(root: Root, base: ConversationProps): Promise<unknown> {
  document.documentElement.dataset.theme = "light";
  document.documentElement.dataset.chrome = "refined";
  const runKey = "codex#run-fixture";
  const runApproval: ConvItem = { kind: "approval", id: "run-ask", tool: "Bash", summary: "git push origin brief",
    input: { command: "git push origin brief" } };
  const state = teammateResponse({
    runs: [{ id: "run", routineId: "brief", agentId: teammate.id, name: "Morning brief", scheduledAtMs: 0,
      updatedAtMs: 0, status: "waiting", sessionKey: runKey, detail: "", prompt: "", cwd: home, began: true,
      profile: teammate }],
    pendingApprovals: [{ agentId: teammate.id, sessionKey: runKey, approvalId: runApproval.id, tool: "Bash",
      summary: runApproval.summary, reason: "risky:git-push", askedAtMs: 1 }],
  });
  const asks = { thread: 0, past: 0, approved: "" };
  // Read through a call: a narrowed property would make the later count unreadable to the checker.
  const threadAsks = (): number => asks.thread;
  const mount = (panelKey: string, response: AgentControlResponse, shown: string): void => flushSync(() => root.render(
    <div className="agent-surface-stack" style={{ height: "100vh" }}>
      <style>{AGENT_MENU_CSS}</style>
      <AgentWorkspacePanel key={panelKey} agent={teammate} response={response} {...pageProps} shownSessionKey={shown}
        onShowThread={() => { asks.thread += 1; }} onOpenThread={() => { asks.past += 1; }}>
        <div className="shown-session-fixture" data-key={shown} style={{ height: "60vh" }}>
          <Conversation {...base} sessionKey={shown} items={shown === runKey ? [runApproval] : items}
            activeApprovalIds={new Set(shown === runKey ? [runApproval.id] : [])}
            approvalGate={agentApprovalGateFor(response, shown, null)}
            onApprove={id => { asks.approved = id; }} />
        </div>
      </AgentWorkspacePanel>
    </div>));

  // The tray selects the run's session; the owner-follow remounts the page for its agent.
  mount("tray-before", state, runKey); await settled();
  mount("tray-remounted", state, runKey); await settled(); await settled();
  assert(document.querySelector(`.shown-session-fixture[data-key="${runKey}"]`), "The tray's run session was not shown");
  assert(!document.querySelector(`#agent-thread-first-${teammate.id}`), "The run session was replaced by the first-message form");
  assert(threadAsks() === 0, "Mounting the page jumped to the thread over the run the tray opened");
  const allow = [...document.querySelectorAll<HTMLButtonElement>('.shown-session-fixture button')]
    .find(button => button.textContent?.trim().startsWith("Allow"));
  assert(allow, "The run's owner approval lost its Allow control");
  allow.click(); await settled();
  assert(asks.approved === runApproval.id, "Allow on the run's approval did not answer it");

  // An older task from Tasks navigation, for an agent with no thread.
  const noThread = { ...teammateResponse(), threads: {} };
  mount("older-task", noThread, "codex#older-task"); await settled(); await settled();
  assert(document.querySelector('.shown-session-fixture[data-key="codex#older-task"]'), "The older task was not shown");
  assert(!document.querySelector(`#agent-thread-first-${teammate.id}`), "The older task was replaced by the first-message form");
  assert(threadAsks() === 0, "The older task was replaced by a jump to the thread");
  [...document.querySelectorAll<HTMLButtonElement>('.agent-workspace-nav button')]
    .find(button => button.textContent === "Conversation")!.click(); await settled();
  assert(threadAsks() === 1, "The Conversation tab did not ask for the conversation");

  // A past thread: no composer, no verdicts, and the way back.
  const back = { clicks: 0 };
  flushSync(() => root.render(<div style={{ height: "100vh" }}>
    <Conversation {...base} sessionKey="codex#agent-fixture-earlier" items={[...items, runApproval]}
      activeApprovalIds={new Set()} composer={<AgentPastThreadNotice hasCurrent onBack={() => { back.clicks += 1; }} />} />
  </div>)); await settled();
  assert(!document.querySelector('.conv-composer textarea'), "A past thread still offers a composer");
  assert(![...document.querySelectorAll('button')].some(button => button.textContent?.trim().startsWith("Allow")),
    "A past thread still offers Allow");
  const notice = document.querySelector<HTMLElement>('.agent-past-thread');
  assert(notice && notice.textContent?.includes("This is a past conversation. It's read-only."), "The past-conversation notice is missing");
  const backButton = notice.querySelector<HTMLButtonElement>('button')!;
  assert(backButton.textContent === "Back to current conversation" && backButton.getBoundingClientRect().height > 0,
    "The past-thread notice has no visible way back");
  backButton.click(); await settled();
  assert(back.clicks === 1, "Back to current thread did nothing");
  return { trayRun: true, olderTask: true, pastThreadReadOnly: true, pastOpens: asks.past };
}

/**
 * The restart notice waits for the owner, and a thread is read only when the
 * owner opens it: never from behind another surface, and never while the
 * Thread tab shows something else restored at launch.
 */
export async function agentRestartNotice(root: Root): Promise<unknown> {
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  const key = "codex#restart-fixture";
  const requests: AgentRequest[] = [];
  const state = (interrupted: boolean): AgentControlResponse => teammateResponse({
    threads: { [teammate.id]: { current: { sessionKey: key, startedAtMs: 1788958900000, status: "idle", interrupted },
      past: [{ sessionKey: "codex#restart-earlier", startedAtMs: 1788958000000, endedAtMs: 1788958500000 }],
      lastReadAtMs: 0, lastActivityAtMs: 1788958900000 } },
    statuses: { [teammate.id]: interrupted ? "needsAttention" : "idle" },
  });
  const mount = (interrupted: boolean, visible: boolean, shown: string | null = key): void => flushSync(() => root.render(
    <div className="agent-surface-stack" style={{ height: "100vh" }}>
      <style>{AGENT_MENU_CSS}</style>
      <AgentWorkspacePanel key="restart" agent={teammate} response={state(interrupted)} {...pageProps}
        shownSessionKey={shown} visible={visible}
        onRequest={async command => { requests.push(command); return answered(); }}>
        <p>Thread conversation</p>
      </AgentWorkspacePanel>
    </div>));
  const markReads = (): number => requests.filter(request => request.kind === "markRead").length;
  const noticeShown = (): boolean => [...document.querySelectorAll('.agent-thread-notice')]
    .some(node => node.textContent?.startsWith("Interface restarted while this agent was working."));

  // Launch restored another focus for this agent: a run, an older task, a past
  // thread, or nothing (only an Open thread button). The page is on Thread and
  // visible, but the owner has not opened the thread.
  for (const restored of ["codex#run-restored", "codex#older-restored", "codex#restart-earlier", null]) {
    mount(true, true, restored); await settled(); await settled();
    assert(markReads() === 0, `markRead was sent while the Thread tab showed ${restored ?? "only Open thread"}`);
  }
  // The owner is on Code: the page is mounted but hidden.
  mount(true, false); await settled(); await settled();
  assert(markReads() === 0, "markRead was sent while the Agent surface was hidden");
  // The owner opens the thread.
  mount(true, true); await settled();
  await waitFor(() => requests.filter(request => request.kind === "markRead").length === 1,
    "markRead was not sent once the owner could see the thread");
  // The read clears `interrupted`; the notice stays.
  mount(false, true); await settled(); await settled();
  assert(noticeShown(), "The restart notice vanished with the read that cleared interrupted");
  assert(requests.filter(request => request.kind === "markRead").length === 1, "markRead was sent more than once");
  document.querySelector<HTMLButtonElement>('button[aria-label="Dismiss restart notice"]')!.click(); await settled();
  assert(!noticeShown(), "Dismiss did not close the restart notice");
  return { hiddenRead: false, restoredFocusRead: false, latched: true, dismissed: true };
}
