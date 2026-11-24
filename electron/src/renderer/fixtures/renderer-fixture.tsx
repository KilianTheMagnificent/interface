import { commitSelectionFixture } from "./commit-selection-fixture";
import { workflowsFixture } from "./workflows-fixture";
import { customizationFixture } from "./customization-fixture";
import { outputPreviewFixture, clearOutputPreviewFixture } from "./output-preview-fixture";
import {
  browserPointChoose, browserPointComment, browserPointFixture, browserPointGuest, browserPointPress, browserPointState,
  clearBrowserPointFixture,
} from "./browser-point-fixture";
import {
  browserChromeClose, browserChromeFindType, browserChromeFixture, browserChromeFlip, browserChromeKey, browserChromeSelect,
  browserChromeState, clearBrowserChromeFixture,
} from "./browser-chrome-fixture";
import {
  visualPreviewFixture, visualReplyFixture, visualReplyPress, visualReplyState,
} from "./visual-reply-fixture";
import { conversationSurfaceInteractions, markdownStreamingInteractions } from "./conversation-surface-fixture";
import { awayFixture } from "./away-fixture";
import {
  clearThreadFixture, threadCardFixture, threadLiveFixture, threadLongTranscriptFixture, threadPaneFixture,
  threadSettingsFixture, type ThreadCardMode, type ThreadLivePose,
} from "./thread-view-fixture";
import {
  clearThreadComposerFixture, threadComposerFixture, type ThreadComposerPose,
} from "./thread-composer-fixture";
import {
  clearComposerReferencesFixture, composerReferencesFixture,
} from "./composer-references-fixture";
import { harnessFixture } from "./harness-fixture";
import { workspaceLauncherFixture } from "./workspace-launcher-fixture";
import { clearThreadOpinionFixture, threadOpinionFixture } from "./thread-opinion-fixture";
import { workspaceHierarchyFixture } from "./workspace-hierarchy-fixture";
import { terminalPaneMenuFixture } from "./terminal-pane-menu-fixture";
import { workspaceToolbarFixture } from "./workspace-toolbar-fixture";
import { cornerNoticesFixture } from "./corner-notices-fixture";
import { sidebarPeekFixture, sidebarPeekState, sidebarPeekTrack } from "./sidebar-peek-fixture";
import { workingIndicatorFixture } from "./working-indicator-fixture";
import { createCliPaneAppearanceFixture } from "./cli-pane-appearance-fixture";
import { createTranslucencyFixture, sidebarGlassScrollFixture, readableSurfaceFrames, materialOverlayProbe } from "./translucency-fixture";
import { createRoot } from "react-dom/client";
import { isResolvedTheme, THEME_PREFERENCES, themeKind } from "../../shared/appearance";
import { createTerminalThemeFixture, terminalCaptureMatches, themeStructure } from "./theme-consistency-fixture";
import { terminalRedrawFixture } from "./terminal-redraw-fixture";
import { terminalCollapseFixture } from "./terminal-collapse-fixture";
import { terminalPaneMotionFixture } from "./terminal-pane-motion-fixture";
import { terminalPaneSwapFixture } from "./terminal-pane-swap-fixture";
import { terminalInteractionFixture } from "./terminal-interaction-fixture";
import { terminalExitHandoffFixture, terminalHandoffSwitchFixture } from "./terminal-exit-handoff-fixture";
import { terminalReopenFixture } from "./terminal-reopen-fixture";
import { terminalPreservationFixture } from "./terminal-preservation-fixture";
import { terminalWorkspaceSwitchFixture } from "./terminal-workspace-switch-fixture";
import { terminalCursorFixture } from "./terminal-cursor-fixture";
import { terminalCaretFixture } from "./terminal-caret-fixture";
import { terminalProgramCopyFixture } from "./terminal-program-copy-fixture";
import { createKeyboardInputFixture } from "./keyboard-input-fixture";
import { agentComputerVisual, agentMemoryVisual, agentRoutineVisual, agentProfileVisual, agentRosterVisual, agentWorkspaceInteractions, agentWorkspaceVisual } from "./agent-workspace-fixture";
import { flushSync } from "react-dom";
import { useCallback, useRef, useState } from "react";
import { Conversation, type ConversationProps } from "../components/Conversation";
import { SearchableItem } from "../components/ConversationRows";
import { ProjectedConversation } from "../components/ProjectedConversation";
import { Composer } from "../components/Composer";
import { AgentTile } from "../components/AgentTile";
import { sessionConfigFor } from "../app-session-projection";
import type { SessionSummary } from "../../shared/engine";
import { APP_SELECT_CSS } from "../components/AppSelect";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import { IntegratedTerminalCenter, type IntegratedTerminalCenterHandle, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import { AttentionTray as FixtureAttentionTray } from "../components/AttentionTray";
import { AcpProfilesEditor } from "../components/AcpProfilesEditor";
import { EMPTY_ATTENTION, attentionRows, clearEvents, dismissEvent, recordEvent, type AttentionRow } from "../attention-tray";
import { useTerminalNotifications } from "../terminal-notifications";
import { Sidebar } from "../components/Sidebar";
import { sessionCategoryFixture } from "./session-category-fixture";
import { sidebarReferenceFixture } from "./sidebar-reference-fixture";
import { sidebarRowFixture } from "./sidebar-row-fixture";
import { threadInboxFixture } from "./thread-inbox-fixture";
import { conversationRenderFixture } from "./conversation-render-fixture";
import { commandPaletteFixture } from "./command-palette-fixture";
import { agentOverviewFixture, clearAgentOverviewFixture } from "./agent-overview-fixture";
import { buildSidebarView } from "../sidebar-view";
import { TURN_CLOCK_MIN_MS } from "../session-activity";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import { TERMINAL_FAST_MODE_KEY, type TerminalSummary, type TerminalWorkspaceRecord } from "../../shared/ipc";
import { EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, serializeTerminalWorkspaceState } from "../terminal-workspace";
import { createConversationProjection } from "../conversation-projection";
import { createTerminalActivityMonitor, feedTerminalActivity, isTerminalFocusReport,
  setTerminalActivityProfile, type TerminalActivitySummary } from "../terminal-activity";
import type { ConvItem } from "../conversation-reducer";
import type { EngineEvent } from "../../shared/engine";
import "../theme.css";
import "../codicons.css";
import { assert, frames as settled, noop, wait } from "./fixture-support";

const root = createRoot(document.getElementById("root")!);
const base: ConversationProps = {
  items: [], live: true, thinking: false, focused: true, sessionKey: "codex#fixture",
  config: { effort: null, models: [], canSetModel: false, effortLevels: [], showEffort: false,
    canSetEffort: false, pendingModel: false, pendingEffort: false, error: null },
  onSend: noop, onStop: noop, onSetModel: noop, onSetEffort: noop,
  onApprove: noop, onDeny: noop, approvals: {}, activeApprovalIds: new Set(),
};

async function rowMemoization(): Promise<Record<string, number>> {
  const counts = new Map<string, number>();
  const find = { active: false, matches: new Set<string>(), currentKey: null,
    register: (key: string, node: HTMLDivElement | null) => { if (node) counts.set(key, (counts.get(key) ?? 0) + 1); } };
  const events: EngineEvent[] = Array.from({ length: 1000 }, (_, index) => ({ kind: "user.message", text: `Prompt ${index}` }));
  events.push({ kind: "message.delta", text: "Streaming" });
  const project = createConversationProjection();
  const render = (): void => {
    const all = { ...base, keyedApprovalId: null, activeApprovalIds: new Set<string>(), checkingApprovalIds: new Set<string>() };
    flushSync(() => root.render(<>{project(events).map(item => <SearchableItem key={item.id}
      item={item} findKey={item.id} find={find} all={all} density="balanced" densityEpoch={0} />)}</>));
  };
  render();
  const before = new Map(counts);
  const started = performance.now();
  for (let index = 0; index < 10; index++) { events.push({ kind: "message.delta", text: " more" }); render(); }
  await settled();
  const changed = [...counts].filter(([key, count]) => count !== before.get(key));
  assert(changed.length === 1, `Streaming rerendered ${changed.length} rows, expected the tail only`);
  return { rows: counts.size, streamingUpdates: 10, rowsRerendered: changed.length, durationMs: performance.now() - started };
}

async function conversationInteractions(): Promise<void> {
  const events: EngineEvent[] = Array.from({ length: 80 }, (_, index) => ({ kind: "user.message", text: `Historical prompt ${index}` }));
  events.push({ kind: "message.delta", text: "Final answer" });
  const render = (key: string, log: EngineEvent[]): void => {
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex", flexDirection: "column" }}>
      <ProjectedConversation {...base} sessionKey={key} events={log} />
    </div>));
  };
  render("codex#A", events);
  await settled();
  const log = document.querySelector<HTMLElement>('[role="log"]')!;
  assert(log.scrollHeight > log.clientHeight, "Long transcript did not create a scrollable log");
  log.focus();
  log.dispatchEvent(new KeyboardEvent("keydown", { key: "f", metaKey: true, bubbles: true }));
  await settled();
  const input = document.querySelector<HTMLInputElement>('.conv-findfield');
  assert(input !== null && document.activeElement === input, "Find did not take focus");
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "prompt 17");
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await settled();
  assert(document.querySelector('[data-current-match="true"]')?.textContent?.includes("prompt 17"), "Find did not highlight the matching prompt");
  render("codex#B", [{ kind: "user.message", text: "Only session B" }]);
  render("codex#C", [{ kind: "user.message", text: "Only session C" }]);
  await settled();
  assert(!document.querySelector('.conv-findfield'), "Find state crossed a session switch");
  assert(document.querySelector('[role="log"]')?.textContent?.includes("Only session C"), "Late session content won a rapid switch");
  assert(!document.querySelector('[role="log"]')?.textContent?.includes("session B"), "Previous session survived a switch");

  let approved = "";
  const approval: ConvItem = { kind: "approval", id: "approval", tool: "Bash", summary: "Fixture only", input: {} };
  flushSync(() => root.render(<Conversation {...base} items={[approval]} activeApprovalIds={new Set(["approval"])}
    onApprove={id => { approved = id; }} />));
  await settled();
  const allow = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.trim().startsWith("Allow"));
  assert(allow && !allow.disabled, "Active approval has no enabled Allow button");
  allow.click();
  await settled();
  assert(approved === "approval", "Approval did not reach the injected callback");
}

async function chatInteractions(): Promise<void> {
  const defaults = { engine: "claude", cwd: "", prompt: "Chat fixture", model: "haiku",
    effort: "", permissionMode: "auto", worktree: true };
  let launched: { kind?: string; permissionMode: string; worktree?: boolean } | undefined;
  const choicesFor = (engine: string) => ({ models: ["haiku"], efforts: [],
    permissionModes: ["default", "auto"], chat: engine === "claude" });
  const renderComposer = (engines: { id: string; unavailable?: string }[]): void => {
    flushSync(() => root.render(<Composer key={JSON.stringify(engines)} open kind="chat"
      defaults={defaults} engines={engines} choicesFor={choicesFor}
      onLaunch={form => { launched = form; }} onBrowse={async () => null} onClose={noop} />));
  };
  for (const engines of [[{ id: "codex" }], [{ id: "claude", unavailable: "offline" }]]) {
    renderComposer(engines);
    await settled();
    const submit = document.querySelector<HTMLButtonElement>(".composer-submit")!;
    assert(submit.disabled, "Chat offered a provider that cannot launch it");
    document.querySelector("textarea")!.dispatchEvent(new KeyboardEvent("keydown", {
      key: "Enter", bubbles: true,
    }));
    await settled();
    assert(launched === undefined, "Enter bypassed unavailable Chat provider guard");
    assert(document.querySelector('.composer-notice[role="status"]')?.textContent?.includes("Claude provider"),
      "Unavailable Chat has no actionable explanation");
  }
  renderComposer([{ id: "claude" }, { id: "codex" }]);
  await settled();
  document.querySelector<HTMLButtonElement>(".composer-submit")!.click();
  assert(launched?.kind === "chat" && launched.permissionMode === "" && launched.worktree === false,
    "Chat launch inherited hidden workspace settings");

  const summary = { engine: "claude", sessionId: "chat-fixture", kind: "chat", title: "Chat fixture",
    cwd: "/private-chat-home", status: "idle", model: "haiku" } as SessionSummary;
  const config = sessionConfigFor("claude#chat-fixture", summary, [], [], "Claude", {}, {}, {},
    [{ engine: "claude", model: "haiku", effort: "default", permissionMode: "auto" }],
    ["default", "auto"], [], "auto", false, false);
  let sends = 0;
  let stops = 0;
  const renderConversation = (busy = false): void => {
    flushSync(() => root.render(<AgentTile session={summary} selected maximized={false} live
      engineLabel="Claude" onFocus={noop} onClose={noop} onToggleMaximize={noop}>
      <Conversation {...base} sessionKey="claude#chat-fixture" config={config}
        queueWhileBusy={busy} thinking={busy} onQueue={() => false}
        onSend={async () => { sends++; throw new Error("Fixture definitely unsent"); }}
        onStop={() => { stops++; }} />
    </AgentTile>));
  };
  renderConversation();
  await settled();
  assert(!document.querySelector('[aria-label^="Permission mode"]'), "Chat rendered a permission control");
  assert(!document.querySelector('[aria-label^="Add prompt context"]'), "Chat offered unreadable project references");
  const prompt = document.querySelector<HTMLTextAreaElement>("textarea")!;
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(prompt, "Keep my words");
  prompt.dispatchEvent(new Event("input", { bubbles: true }));
  await settled();
  prompt.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await settled();
  assert(sends === 1 && prompt.value === "Keep my words", "Refused Chat send lost its draft or sent twice");
  renderConversation(true);
  await settled();
  prompt.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await settled();
  assert(sends === 1 && prompt.value === "Keep my words", "Refused queue lost the draft or sent directly");
  const stop = document.querySelector<HTMLButtonElement>('button[aria-label="Stop"]');
  assert(stop, "Working Chat has no Stop control");
  stop.click();
  assert(stops === 1, "Chat Stop did not reach its owner");
}

/** A turn's clock belongs to its session: an in-place switch from a working
 * session to an idle one must not stamp "Done in" on the idle one's rows. */
async function turnClockSwitch(): Promise<void> {
  const items = (text: string): ConvItem[] => [{ kind: "text", id: "reply", text, streaming: false }];
  const render = (key: string, thinking: boolean, text: string): void => {
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex", flexDirection: "column" }}>
      <Conversation {...base} sessionKey={key} thinking={thinking} items={items(text)} />
    </div>));
  };
  // The control: the same session finishing in place does draw its clock.
  render("codex#clock-a", true, "Working on A");
  await wait(TURN_CLOCK_MIN_MS + 100);
  render("codex#clock-a", false, "Finished A");
  await settled();
  assert(document.querySelector(".conv-turn-clock") !== null, "A finished turn drew no clock");
  render("codex#clock-a", true, "Working on A again");
  await wait(TURN_CLOCK_MIN_MS + 100);
  render("codex#clock-b", false, "Idle B");
  await settled();
  assert(document.querySelector(".conv-turn-clock") === null,
    "An in-place switch stamped the previous session's turn clock on the new session");
  flushSync(() => root.render(null));
}

/** Editing a provider field back to its saved value keeps the same input. */
async function acpProfileFocus(): Promise<void> {
  const profiles = [{ slug: "local-agent", displayName: "Local Agent",
    executablePath: "/opt/homebrew/bin/local-agent", args: ["acp"] }];
  flushSync(() => root.render(<AcpProfilesEditor profiles={profiles} onSave={noop} />));
  await settled();
  const field = document.querySelector<HTMLInputElement>(".acp-profile-input")!;
  const status = (): string => document.querySelector(".acp-profiles-status")?.textContent ?? "";
  const type = async (value: string): Promise<void> => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
    await settled();
  };
  field.focus();
  await type("local-agentx");
  assert(status() === "Unsaved provider roster changes.", `Edit did not mark the roster unsaved: ${status()}`);
  await type("local-agent");
  assert(status() === "Provider roster is saved.", `Saved value did not settle the roster: ${status()}`);
  assert(field.isConnected && document.activeElement === field,
    "Editing a field back to its saved value remounted it and dropped the focus");
  flushSync(() => root.render(null));
}

async function terminalBacklog(): Promise<Record<string, number>> {
  let terminal: TerminalViewHandle | null = null;
  flushSync(() => root.render(<div style={{ height: 300 }}><TerminalView terminalId="fixture" active
    onData={noop} onReady={handle => { terminal = handle; handle?.completeSetup(); }} /></div>));
  await settled();
  const handle = terminal as TerminalViewHandle | null;
  assert(handle, "Terminal did not mount");
  let outstanding = 0;
  let peak = 0;
  const chunk = new TextEncoder().encode("fixture output\r\n".repeat(256));
  const started = performance.now();
  await new Promise<void>(resolve => {
    for (let index = 0; index < 512; index++) {
      outstanding += chunk.byteLength;
      peak = Math.max(peak, outstanding);
      handle.write(chunk, () => { outstanding -= chunk.byteLength; if (outstanding === 0) resolve(); });
    }
  });
  assert(handle.readLines().some(line => line.includes("fixture output")), "Terminal lost the test output");
  return { peakOutstandingBytes: peak, remainingBytes: outstanding, drainMs: performance.now() - started };
}

async function terminalFocusActivity(): Promise<number> {
  const monitor = createTerminalActivityMonitor();
  setTerminalActivityProfile(monitor, "codex");
  const encoder = new TextEncoder();
  let terminal: TerminalViewHandle | null = null;
  let focusReports = 0;
  const repaint = encoder.encode("\x1b[1;1H› Ask Codex to do anything");
  flushSync(() => root.render(<div style={{ height: 300 }}>
    <button id="blur-terminal">Other pane</button>
    <TerminalView terminalId="focus-fixture" active onReady={handle => {
      terminal = handle; handle?.completeSetup();
    }} onData={data => {
      assert(isTerminalFocusReport(encoder.encode(data)), "Focus emitted ordinary input");
      focusReports += 1;
      feedTerminalActivity(monitor, repaint);
      assert(monitor.activity === "idle", "Focus redraw flashed working");
      terminal?.write(repaint);
    }} />
  </div>));
  await settled();
  const handle = terminal as TerminalViewHandle | null;
  assert(handle, "Focus terminal did not mount");
  await new Promise<void>(resolve => handle.write(encoder.encode("\x1b[?1004h\x1b]0;Respond to greeting\x07"), resolve));
  for (let index = 0; index < 10; index++) {
    document.getElementById("blur-terminal")!.focus();
    handle.focus();
    await settled();
  }
  assert(focusReports >= 10, `xterm reported ${focusReports} focus transitions; active=${document.activeElement?.outerHTML.slice(0, 150)}`);
  return focusReports;
}

async function workspaceSelection(): Promise<Record<string, number>> {
  const timestamp = "2026-09-06T00:00:00.000Z";
  const terminals: TerminalSummary[] = Array.from({ length: 12 }, (_, index) => ({
    id: `00000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
    generation: 1, title: `Shell ${index + 1}`, root: "/fixture/sidebar",
    cwd: "/fixture/sidebar", state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: index === 0 ? "codex" : index === 1 ? "claude" : index === 2 ? "opencode" : "shell" },
    createdAt: timestamp, updatedAt: timestamp,
  }));
  let saved: TerminalWorkspaceRecord[] = terminals.map((terminal, index) => ({
    id: `workspace-${index + 1}`, name: `Workspace ${index + 1}`,
    createdAt: timestamp, updatedAt: timestamp, terminals: [terminal],
    layout: serializeTerminalWorkspaceState(openTerminalTab(
      EMPTY_TERMINAL_WORKSPACE_STATE, `pane-${index + 1}`, terminal.id)),
  }));
  const expectedOrder = saved.map(record => record.id).join(",");
  const errors: string[] = [];
  let receiveData: Parameters<TerminalRendererApi["onTerminalData"]>[0] | undefined;
  let receiveLifecycle: Parameters<TerminalRendererApi["onTerminalLifecycle"]>[0] | undefined;
  const fastInputs: { id: string; generation: number; data: string }[] = [];
  terminals[0].fastMode = { version: 1, model: "gpt-6-astra", enabled: false, ready: true };
  const notifications: { title: string; activate?: () => void }[] = [];
  let notificationsAllowed = true;
  const notify = (title: string, _body: string, activate?: () => void): void => {
    if (!notificationsAllowed) return;
    notifications.push({ title, activate });
  };
  let writes = 0;
  const unexpected = (): never => { throw new Error("Workspace selection attempted an unrelated API call"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals }),
    listTerminalLaunchProfiles: async () => [],
    getTerminalWorkspaces: async () => structuredClone(saved),
    setTerminalWorkspaces: async records => {
      assert(records.map(record => record.id).join(",") === expectedOrder,
        "Workspace selection reordered the persisted roster");
      saved = structuredClone(records); writes++;
      return structuredClone(saved);
    },
    onTerminalLifecycle: callback => { receiveLifecycle = callback; return noop; },
    onTerminalData: listener => { receiveData = listener; return () => { receiveData = undefined; }; },
    onTerminalDataFailure: () => noop,
    sendTerminalInput: (id, generation, data) => {
      const text = new TextDecoder().decode(data);
      if (text === TERMINAL_FAST_MODE_KEY) fastInputs.push({ id, generation, data: text });
    },
    resizeTerminal: async input => ({ ...terminals.find(terminal => terminal.id === input.id)!, size: input.size }),
    closeTerminal: unexpected, createTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  function WorkspaceFixture() {
    const [switcher, setSwitcher] = useState<TerminalWorkspaceSwitcher | null>(null);
    const [summaries, setSummaries] = useState<readonly TerminalActivitySummary[]>([]);
    const [focused, setFocused] = useState<string | null>(null);
    const [attention, setAttention] = useState(EMPTY_ATTENTION);
    const [open, setOpen] = useState(false);
    const center = useRef<IntegratedTerminalCenterHandle>(null);
    const record = useCallback((event: Omit<AttentionRow, "live">) => {
      setAttention(current => recordEvent(current, event));
    }, []);
    const activate = useCallback((id: string) => { center.current?.focusTerminal(id); }, []);
    useTerminalNotifications({ terminals: summaries, visible: true, focusedTerminalId: focused,
      workspaces: switcher?.records, onEvent: record, onActivate: activate, notify });
    const events = attentionRows(attention, []);
    const sidebar = buildSidebarView([], null, new Set(), false, false, "", {
      terminals: summaries, registryTerminals: summaries, projectSessions: [],
      selectedTerminalId: focused,
    });
    return <><FixtureAttentionTray rows={events} unread={events.length} open={open} onOpenChange={setOpen}
      now={Date.now()} onClearAll={() => { setAttention(clearEvents); }} onActivate={row => {
        if (row.target.kind === "terminal") activate(row.target.terminalId);
        setAttention(current => dismissEvent(current, row.id));
      }} /><div style={{ height: 240, display: "flex" }}>
      <Sidebar groups={sidebar.groups} terminalProjects={sidebar.terminalProjects}
        rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
        onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop}
        terminalWorkspaces={switcher ?? undefined} terminalsActive />
      <IntegratedTerminalCenter api={api} ref={center} activeCwd="/fixture/sidebar"
        visible onError={message => { errors.push(message); }} onSwitcherChange={setSwitcher}
        onTerminalsChange={setSummaries} onFocusedTerminalChange={setFocused} />
    </div></>;
  }
  const rows = (): HTMLButtonElement[] => [...document.querySelectorAll<HTMLButtonElement>(
    ".sidebar [data-terminal-workspace-id]")];
  const mount = async (): Promise<void> => {
    flushSync(() => root.render(<WorkspaceFixture />));
    const deadline = performance.now() + 3000;
    while (rows().length !== terminals.length) {
      assert(performance.now() < deadline,
        `Workspace sidebar failed to load (${rows().length} rows): ${errors.join("; ")}`);
      await settled();
    }
    await settled();
  };
  const assertOrder = (): void => {
    assert(rows().map(row => row.dataset.terminalWorkspaceId).join(",") === expectedOrder,
      "Clicking a workspace moved its sidebar row");
  };
  await mount();
  flushSync(() => rows()[0]!.click());
  await settled();
  const bolt = (): HTMLButtonElement | null => document.querySelector(".terminal-fast-mode");
  assert(bolt() !== null, "Supported Codex model did not expose its Fast bolt");
  assert(Boolean(bolt()!.nextElementSibling?.querySelector(".codicon-ellipsis")),
    "Fast bolt is not beside the pane menu");
  assert(bolt()!.getAttribute("aria-pressed") === "false", "Fast starts unconfirmed on");
  flushSync(() => bolt()!.click());
  assert(bolt()!.disabled && bolt()!.getAttribute("aria-pressed") === "false",
    "Pending Fast click must wait for the CLI acknowledgement");
  flushSync(() => bolt()!.click());
  assert(fastInputs.length === 1 && fastInputs[0].id === terminals[0].id
    && fastInputs[0].generation === 1 && fastInputs[0].data === TERMINAL_FAST_MODE_KEY,
    "Fast action was duplicated or sent to the wrong terminal generation");
  let revision = 1;
  const publishFast = async (mode: TerminalSummary["fastMode"]): Promise<void> => {
    const terminal = { ...terminals[0], fastMode: mode,
      updatedAt: `2026-09-06T00:00:${String(++revision).padStart(2, "0")}.000Z` };
    flushSync(() => receiveLifecycle!({ kind: "upsert", revision, terminal }));
    await settled();
  };
  await publishFast({ version: 1, model: "gpt-6-astra", enabled: true, ready: true });
  assert(!bolt()!.disabled && bolt()!.getAttribute("aria-pressed") === "true",
    "Fast bolt did not follow the acknowledged state");
  await publishFast({ version: 1, model: "gpt-6-astra", enabled: false, ready: false });
  assert(bolt()!.disabled && bolt()!.getAttribute("aria-pressed") === "false",
    "Fast bolt must follow manual /fast changes and disable while busy");
  await publishFast(undefined);
  assert(bolt() === null, "Unsupported model retained a stale Fast control");
  const notice = (): HTMLElement | null => document.querySelector(".terminal-startup-download");
  assert(notice() === null, "A pane with local startup files showed a download notice");
  const publishDownload = async (download: TerminalSummary["startupDownload"]): Promise<void> => {
    const terminal = { ...terminals[0], fastMode: undefined, startupDownload: download,
      updatedAt: `2026-09-06T00:00:${String(++revision).padStart(2, "0")}.000Z` };
    flushSync(() => receiveLifecycle!({ kind: "upsert", revision, terminal }));
    await settled();
  };
  await publishDownload({ version: 1, files: [".git/HEAD", "CLAUDE.md"], remaining: 4 });
  assert(notice()?.textContent === "Waiting for cloud download: .git/HEAD, CLAUDE.md and 2 more",
    "A pane waiting on cloud-only startup files did not say so");
  assert(notice()!.getAttribute("role") === "status" && /Keep Downloaded/u.test(notice()!.title),
    "The download notice does not announce itself or explain the wait");
  await publishDownload(undefined);
  assert(notice() === null, "The download notice outlived the download");
  terminals[0].fastMode = undefined;
  assertOrder();
  const list = document.querySelector<HTMLElement>(".sidebar .session-list")!;
  assert(list.scrollHeight > list.clientHeight, "Workspace fixture did not exercise sidebar scrolling");
  list.scrollTop = list.scrollHeight;
  const scrollTop = list.scrollTop;
  const selections = [11, 9, 10, 11, 10, 9];
  for (const index of selections) {
    flushSync(() => rows()[index]!.click());
    await settled();
    assertOrder();
    assert(rows()[index]!.getAttribute("aria-current") === "page", "Clicked workspace was not selected");
    assert(Math.abs(list.scrollTop - scrollTop) < 1,
      `Workspace ${index + 1} moved sidebar scroll position from ${scrollTop} to ${list.scrollTop}`);
  }
  // Unmount flushes the real persistence queue; mount reads that saved roster
  // and the active-workspace preference, as reopening the surface does.
  flushSync(() => root.render(null));
  await settled();
  await mount();
  assertOrder();
  assert(rows()[9]!.getAttribute("aria-current") === "page", "Workspace selection did not survive remount");
  const emit = async (index: number, output: string): Promise<void> => {
    assert(receiveData, "Terminal data listener is missing");
    receiveData({ kind: "data", terminalId: terminals[index]!.id, generation: 1,
      sequence: "0", data: new TextEncoder().encode(output) }, noop);
    await settled(); await settled();
  };
  const codexDone = "\x1b]9;Agent turn complete\x07";
  const claudeTurn = "\x1b]2;interface-claude-activity:working\x07"
    + "\x1b]2;interface-claude-activity:idle\x07";
  await emit(0, codexDone);
  assert(notifications.length === 1 && notifications[0]!.title.endsWith("agent finished"),
    "Background Codex completion did not send a notification");
  await emit(0, codexDone);
  assert(notifications.length === 1, "Duplicate completion sent another notification");
  await emit(1, claudeTurn);
  assert(Number(notifications.length) === 2, "Batched Claude completion was lost");
  notifications[0]!.activate?.();
  await settled(); await settled();
  assert(rows()[0]!.getAttribute("aria-current") === "page", "Notification opened the wrong workspace");
  assertOrder();
  await emit(0, "\x1b]2;⠋ Working\x07" + codexDone);
  assert(Number(notifications.length) === 2, "A watched terminal completion notified");
  flushSync(() => rows()[9]!.click());
  await settled();
  assert(Number(notifications.length) === 2, "Switching away re-notified an already watched completion");
  document.querySelector<HTMLButtonElement>(".attention-tray-button")!.click();
  await settled(); await settled();
  const completedRows = [...document.querySelectorAll<HTMLButtonElement>(".attention-tray-row")];
  assert(completedRows.length === 2 && completedRows.every(row => row.textContent?.includes("Finished")),
    "Terminal completions did not remain in the bell menu");
  const claudeRow = completedRows.find(row => row.textContent?.includes("Workspace 2"));
  assert(claudeRow, "Completion lost its workspace name");
  claudeRow.click();
  await settled(); await settled();
  assert(rows()[1]!.getAttribute("aria-current") === "page", "Bell entry opened the wrong workspace");
  notificationsAllowed = false;
  await emit(0, "\x1b]2;⠋ Working again\x07" + codexDone);
  assert(Number(notifications.length) === 2, "Denied notification sink still delivered an alert");
  document.querySelector<HTMLButtonElement>(".attention-tray-button")!.click();
  await settled(); await settled();
  assert(document.querySelectorAll(".attention-tray-row").length === 2,
    "Unavailable OS notifications hid the completion from the bell menu");
  assert(errors.length === 0, errors.join("; "));
  assert(writes > 0, "Workspace fixture did not exercise persistence");
  flushSync(() => root.render(null));
  await settled();
  notificationsAllowed = true;
  await mount();
  assert(Number(notifications.length) === 2, "Remount fabricated completion notifications");
  flushSync(() => rows()[2]!.click());
  await settled();
  for (const [signal, kind, label] of [
    ["working", "working", "Working"],
    ["waiting", "waiting", "Needs attention"],
    ["working", "working", "Working"],
    ["complete", "running", "Idle"],
  ]) {
    await emit(2, `\x1b]2;interface-terminal-activity:v1:${signal}\x07`);
    // Rust reads the same bytes and says whether the pane needs you
    // (host protocol v44); the pane shows that answer, not its own.
    const at = `2026-09-06T00:01:${String(++revision).padStart(2, "0")}.000Z`;
    const terminal = { ...terminals[2]!, updatedAt: at,
      ...(signal === "waiting" ? { attention: { version: 1 as const, since: at } } : {}) };
    flushSync(() => receiveLifecycle!({ kind: "upsert", revision, terminal }));
    await settled(); await settled();
    assert(document.querySelector(`.terminal-tab-status.is-${kind}`),
      `OpenCode pane did not project ${signal}`);
    const activePane = document.querySelector('.terminal-tab[aria-selected="true"]');
    assert(activePane?.textContent?.includes(label!), `OpenCode pane did not announce ${label}`);
    const workspaceRow = document.querySelector('[data-terminal-workspace-id="workspace-3"]');
    assert(workspaceRow?.querySelector('[role="img"]')?.getAttribute("aria-label") === label,
      `OpenCode workspace did not announce ${label}`);
  }
  flushSync(() => root.render(null));
  return { workspaces: terminals.length, selections: selections.length, writes, completionNotifications: notifications.length };
}

async function screenshotFixture(theme: string, chrome: string): Promise<unknown> {
  assert(isResolvedTheme(theme), `Unknown fixture theme ${theme}`);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  // Reconcile MutationObserver-driven chrome state before recording layout.
  await settled();
  const items: ConvItem[] = [
    { kind: "user", id: "user", text: "Review the workspace and explain the changes." },
    { kind: "text", id: "answer", text: "The implementation keeps **storage and delivery guarantees** intact.\n\n- Stable transcript rows\n- Bounded terminal replay\n- Incremental voice streaming", streaming: false },
    { kind: "tool", id: "tool", tool: "Read", input: { path: "/fixture/a-very-long-directory-name/source-file.rs" }, summary: "Read source-file.rs", result: { ok: true, summary: "Read complete" } },
  ];
  flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
    <style>{APP_SELECT_CSS}</style>
    {[0, 1].map(index => <div key={index} style={{ width: "50%", minWidth: 0, display: "flex", flexDirection: "column" }}>
      <Conversation {...base} focused={index === 0} sessionKey={`codex#${index}`} items={items} />
      <Composer open defaults={{ engine: "codex", cwd: "/fixture/a-very-long-repository-name", prompt: "",
        model: "fixture-model", effort: "", permissionMode: "default" }}
        engines={[{ id: "codex" }]} choicesFor={() => ({ models: ["fixture-model"], efforts: [], permissionModes: ["default"] })}
        onLaunch={noop} onBrowse={async () => null} onClose={noop} />
    </div>)}
  </div>));
  await settled();
  await document.fonts.ready;
  // Chromium can defer style invalidation in a hidden window past rAF. Poll
  // computed values until the requested theme and chrome have actually landed.
  const deadline = performance.now() + 3000;
  while (true) {
    const style = getComputedStyle(document.querySelector<HTMLElement>(".conv-text")!);
    const hex = getComputedStyle(document.documentElement).getPropertyValue("--fg").trim();
    const expectedColor = `rgb(${[1, 3, 5].map(index => parseInt(hex.slice(index, index + 2), 16)).join(", ")})`;
    if (style.color === expectedColor && style.fontSize === (chrome === "cli" ? "19px" : "15px")) break;
    assert(performance.now() < deadline, `Theme or chrome failed to settle: ${theme}/${chrome}, font=${style.fontSize}, color=${style.color}`);
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  const prompt = document.querySelector<HTMLTextAreaElement>('.composer-box textarea');
  assert(prompt, "Composer prompt did not mount");
  prompt.focus({ focusVisible: true } as FocusOptions);
  assert(document.activeElement === prompt, "Keyboard focus did not reach the Composer");
  // Include the same focused state in every capture after focus selectors and
  // chrome-observer effects have painted, not the preceding frame's styles.
  await settled();
  // Reduced motion still runs every property change as a 0.01ms transition
  // (theme.css shortens transition-duration; the property stays `all`), and a
  // hidden window starts one only at its next compositor frame. Measured
  // before that under load, the focus ring read as its 0px first frame and
  // the first theme of a lane differed from the rest. Wait for them to end.
  await Promise.allSettled(document.getAnimations()
    .filter(animation => animation.effect?.getComputedTiming().endTime !== Infinity)
    .map(animation => animation.finished));
  assert(matchMedia('(prefers-reduced-motion: reduce)').matches, "Reduced-motion fixture is not active");
  for (const card of document.querySelectorAll<HTMLElement>('.composer-card')) {
    const bounds = card.getBoundingClientRect();
    const submit = card.querySelector<HTMLElement>('.composer-submit')!.getBoundingClientRect();
    assert(submit.left >= bounds.left && submit.right <= bounds.right, "Narrow Composer clipped its launch button");
  }
  const measurements = [...document.querySelectorAll<HTMLElement>('.conv-text,.conv-user,.conv-composer,.composer-card,.composer-box')].map(element => {
    const style = getComputedStyle(element);
    const { width, height } = element.getBoundingClientRect();
    return { className: element.className, width, height, color: style.color, background: style.backgroundColor, fontSize: style.fontSize };
  });
  return { measurements, structure: themeStructure(".conv,.conv-text,.conv-user,.conv-composer,.composer-card,.composer-intro,.composer-intro h1,.composer-box,.composer-submit,.is-floating") };
}

declare global {
  interface Window {
    keyboardInputFixture: ReturnType<typeof createKeyboardInputFixture>;
    awayFixture(): Promise<unknown>;
    commitSelectionFixture(): Promise<unknown>;
    workflowsFixture(theme: string): Promise<unknown>;
    customizationFixture(): Promise<unknown>;
    threadCardFixture(theme: string, chrome: string, mode: ThreadCardMode): Promise<unknown>;
    threadPaneFixture(theme: string, chrome: string): Promise<unknown>;
    threadOpinionFixture(theme: string, chrome: string, pose?: "pane" | "menu"): Promise<unknown>;
    clearThreadOpinionFixture(): void;
    threadLongTranscriptFixture(theme: string, chrome: string): Promise<unknown>;
    threadLiveFixture(theme: string, chrome: string, pose?: ThreadLivePose): Promise<unknown>;
    threadSettingsFixture(theme: string, chrome: string): Promise<unknown>;
    clearThreadFixture(): void;
    threadComposerFixture(theme: string, chrome: string, pose: ThreadComposerPose): Promise<unknown>;
    clearThreadComposerFixture(): void;
    composerReferencesFixture(theme: string): Promise<unknown>;
    clearComposerReferencesFixture(): void;
    terminalPaneMotionFixture(hold?: boolean): Promise<unknown>;
    terminalPaneSwapFixture(holdPreview?: boolean): Promise<unknown>;
    sidebarReferenceFixture(theme: import("../../shared/appearance").ResolvedTheme, chrome: string, width: number): Promise<unknown>;
    threadInboxFixture(theme: import("../../shared/appearance").ResolvedTheme, chrome: string, width: number): Promise<unknown>;
    workingIndicatorFixture(theme: import("../../shared/appearance").ResolvedTheme, chrome: string, engine?: string): Promise<unknown>;
    terminalPaneMenuFixture(): Promise<unknown>;
    workspaceHierarchyFixture(theme: string, chrome: string, count?: number, sidebarSlideOnly?: boolean): Promise<unknown>;
    workspaceLauncherFixture(theme: string, chrome: string): Promise<unknown>;
    workspaceToolbarFixture(theme: string, chrome: string): Promise<unknown>;
    cornerNoticesFixture(theme: import("../../shared/appearance").ResolvedTheme, chrome: string): Promise<unknown>;
    sidebarPeekFixture(): void;
    sidebarPeekState(): unknown;
    sidebarPeekTrack(count: number): Promise<number[]>;
    outputPreviewFixture(theme: string, chrome: string): Promise<unknown>;
    clearOutputPreviewFixture(): void;
    browserPointFixture(url: string): Promise<unknown>;
    browserPointGuest(code: string): Promise<unknown>;
    browserPointPress(name: string): Promise<void>;
    browserPointChoose(label: string, option: string): Promise<void>;
    browserPointComment(text: string): Promise<void>;
    browserPointState(): Promise<unknown>;
    browserChromeFixture(url: string): Promise<unknown>;
    browserChromeState(): Promise<unknown>;
    browserChromeFindType(text: string): Promise<void>;
    browserChromeKey(target: "find" | "address", key: string, modifiers?: { meta?: boolean; shift?: boolean }): Promise<void>;
    browserChromeSelect(id: string): Promise<void>;
    browserChromeFlip(): Promise<string>;
    browserChromeClose(id: string): Promise<void>;
    clearBrowserChromeFixture(): void;
    visualReplyFixture(theme: "dark" | "light", html: string): Promise<unknown>;
    visualReplyState(): unknown;
    visualReplyPress(words: string, inOverlay?: boolean): Promise<void>;
    visualPreviewFixture(theme: "dark" | "light", html: string): Promise<unknown>;
    clearBrowserPointFixture(): void;
    agentOverviewFixture(theme: string, chrome: string, tab?: "agents" | "browser"): Promise<unknown>;
    clearAgentOverviewFixture(): void;
    runRendererFixtures(): Promise<unknown>;
    runAgentFixtures(): Promise<unknown>;
    harnessVisual(): Promise<unknown>;
    terminalInteractionFixture(): Promise<unknown>;
    terminalReopenFixture(): Promise<unknown>;
    terminalExitHandoffFixture(): Promise<unknown>;
    terminalHandoffSwitchFixture(): Promise<unknown>;
    shellRenderFixture(): Promise<unknown>;
    terminalCaretFixture(): Promise<unknown>;
    terminalPreservationFixture(): Promise<unknown>;
    screenshotFixture(theme: string, chrome: string): Promise<unknown>;
    themeNames: string[];
    terminalThemeFixture(theme: string): Promise<unknown>;
    cliPaneAppearanceFixture(style: "terminal" | "clean", theme?: string, chrome?: string): Promise<unknown>;
    translucencyFixture(theme: string, terminal: number, sidebar: number, chrome?: string, settleMaterials?: boolean): Promise<unknown>;
    sidebarGlassScrollFixture(): Promise<unknown>;
    readableSurfaceFrames(): Promise<unknown>;
    materialOverlayProbe(show: boolean): Promise<void>;
    terminalCaptureMatches: typeof terminalCaptureMatches;
    agentRoutineVisual(theme: string): Promise<unknown>;
    agentWorkspaceVisual(theme: string): Promise<unknown>;
    agentProfileVisual(theme: string): Promise<unknown>;
    agentRosterVisual(theme: string): Promise<unknown>;
    agentMemoryVisual(theme: string): Promise<unknown>;
    agentComputerVisual(theme: string): Promise<unknown>;
  }
}
window.workingIndicatorFixture = (theme, chrome, engine) => workingIndicatorFixture(root, base, theme, chrome, engine);
window.keyboardInputFixture = createKeyboardInputFixture(root);
window.sidebarReferenceFixture = (theme, chrome, width) => sidebarReferenceFixture(root, theme, chrome, width);
window.threadInboxFixture = (theme, chrome, width) => threadInboxFixture(root, theme, chrome, width);
window.terminalPaneMenuFixture = () => terminalPaneMenuFixture(root);
window.terminalPaneMotionFixture = hold => terminalPaneMotionFixture(root, hold);
window.terminalPaneSwapFixture = holdPreview => terminalPaneSwapFixture(root, holdPreview);
window.workspaceLauncherFixture = (theme, chrome) => workspaceLauncherFixture(root, theme, chrome);
window.workspaceToolbarFixture = (theme, chrome) => workspaceToolbarFixture(root, theme, chrome);
window.cornerNoticesFixture = (theme, chrome) => cornerNoticesFixture(root, theme, chrome);
window.sidebarPeekFixture = () => { sidebarPeekFixture(root); };
window.sidebarPeekState = sidebarPeekState;
window.sidebarPeekTrack = sidebarPeekTrack;
window.agentOverviewFixture = (theme, chrome, tab) => agentOverviewFixture(root, theme, chrome, tab);
window.clearAgentOverviewFixture = () => { clearAgentOverviewFixture(root); };
window.harnessVisual = () => harnessFixture(root, true);
window.browserPointFixture = url => browserPointFixture(root, url);
window.browserPointGuest = browserPointGuest;
window.browserPointPress = browserPointPress;
window.browserPointChoose = browserPointChoose;
window.browserPointComment = browserPointComment;
window.browserPointState = browserPointState;
window.browserChromeFixture = url => browserChromeFixture(root, url);
window.browserChromeState = browserChromeState;
window.browserChromeFindType = browserChromeFindType;
window.browserChromeKey = browserChromeKey;
window.browserChromeSelect = browserChromeSelect;
window.browserChromeFlip = browserChromeFlip;
window.browserChromeClose = browserChromeClose;
window.clearBrowserChromeFixture = () => { clearBrowserChromeFixture(root); };
window.visualReplyFixture = (theme, html) => visualReplyFixture(root, theme, html);
window.visualReplyState = visualReplyState;
window.visualReplyPress = visualReplyPress;
window.visualPreviewFixture = (theme, html) => visualPreviewFixture(root, theme, html);
window.clearBrowserPointFixture = () => { clearBrowserPointFixture(root); };
window.runRendererFixtures = async () => {
  const harness = await harnessFixture(root);
  const conversations = await conversationSurfaceInteractions(root, base);
  const markdown = await markdownStreamingInteractions(root);
  const rows = await rowMemoization();
  await conversationInteractions();
  await chatInteractions();
  await turnClockSwitch();
  await acpProfileFocus();
  const terminal = await terminalBacklog();
  const focusReports = await terminalFocusActivity();
  const workspaces = await workspaceSelection();
  const categories = await sessionCategoryFixture(root);
  const terminalRedraw = await terminalRedrawFixture(root);
  const terminalCollapse = await terminalCollapseFixture(root);
  const terminalPaneSwap = await terminalPaneSwapFixture(root);
  return { harness, conversations, markdown, rows, interactions: "passed", chat: "passed",
    turnClockSwitch: "passed", acpProfileFocus: "passed", terminal, focusReports, workspaces, categories, terminalRedraw, terminalCollapse, terminalPaneSwap };
};
// Their own lane (interactions-agents): with the fixtures above they took most
// of a minute in one process.
window.runAgentFixtures = async () => {
  const agentWorkspace = await agentWorkspaceInteractions(root, base);
  return { agentWorkspace };
};
window.screenshotFixture = screenshotFixture;
window.themeNames = ["dark", ...THEME_PREFERENCES.filter(theme => theme !== "system" && theme !== "dark")];
window.terminalThemeFixture = createTerminalThemeFixture(root);
window.translucencyFixture = createTranslucencyFixture(root);
window.sidebarGlassScrollFixture = sidebarGlassScrollFixture;
window.readableSurfaceFrames = readableSurfaceFrames;
window.materialOverlayProbe = materialOverlayProbe;
window.terminalCaptureMatches = terminalCaptureMatches;
window.agentWorkspaceVisual = theme => agentWorkspaceVisual(root, base, theme);
window.agentProfileVisual = theme => agentProfileVisual(root, theme);
window.agentRosterVisual = theme => agentRosterVisual(root, base, theme);
window.agentMemoryVisual = theme => agentMemoryVisual(root, theme);
window.agentComputerVisual = theme => agentComputerVisual(root, theme);

let terminalPortResult: Promise<Record<string, number>>;
async function beginTerminalPortFixture(): Promise<void> {
  const noisy = "00000000-0000-4000-8000-000000000001";
  const sibling = "00000000-0000-4000-8000-000000000002";
  const handles = new Map<string, TerminalViewHandle>();
  flushSync(() => root.render(<div style={{ height: 360, display: "flex" }}>
    {[noisy, sibling].map(id => <div key={id} style={{ width: "50%", minWidth: 0 }}>
      <TerminalView terminalId={id} active onData={noop} onReady={handle => {
        if (handle) { handles.set(id, handle); handle.completeSetup(); }
        else handles.delete(id);
      }} />
    </div>)}
  </div>));
  await settled();
  assert(handles.size === 2, "Two terminal views did not mount");
  let frames = 0;
  let received = 0;
  let checksum = 0;
  let eof = false;
  let siblingDone = false;
  let resolve!: (value: Record<string, number>) => void;
  terminalPortResult = new Promise(done => { resolve = done; });
  const finish = (): void => {
    if (frames !== 512 || !eof || !siblingDone) return;
    assert(handles.get(noisy)!.readLines().some(line => line.includes("frame:511")), "xterm lost the final frame");
    remove();
    resolve({ frames, checksum });
  };
  const remove = window.iface.onTerminalData((event, complete) => {
    const handle = handles.get(event.terminalId);
    assert(handle, "Output crossed terminal identity");
    if (event.kind === "eof") { eof = true; complete(); finish(); return; }
    if (event.terminalId === sibling) {
      handle.write(event.data, () => {
        complete();
        siblingDone = true;
        window.iface.sendTerminalInput(sibling, 1, new TextEncoder().encode("ping"));
        finish();
      });
      return;
    }
    assert(BigInt(event.sequence) === BigInt(received++), "Terminal bytes arrived out of order");
    for (const byte of event.data) checksum = (checksum + byte) >>> 0;
    // A deliberately slow consumer: includes the real context bridge,
    // MessagePort, renderer scheduling and xterm's completion callback.
    setTimeout(() => handle.write(event.data, () => { frames++; complete(); finish(); }), 20);
  });
}

declare global {
  interface Window {
    beginTerminalPortFixture(): Promise<void>;
    finishTerminalPortFixture(): Promise<Record<string, number>>;
  }
}
window.beginTerminalPortFixture = beginTerminalPortFixture;
window.terminalCaretFixture = () => terminalCaretFixture(root);
window.terminalReopenFixture = () => terminalReopenFixture(root);
window.terminalExitHandoffFixture = () => terminalExitHandoffFixture(root);
window.terminalHandoffSwitchFixture = () => terminalHandoffSwitchFixture(root);
// Its own lane: the interactions lane already runs close to its deadline.
window.shellRenderFixture = async () => ({
  sidebarRows: await sidebarRowFixture(root), commandPalette: await commandPaletteFixture(root),
  conversation: await conversationRenderFixture(root),
});
window.terminalInteractionFixture = async () => ({
  ...await terminalInteractionFixture(root), cursor: await terminalCursorFixture(root),
  caret: await terminalCaretFixture(root),
  programCopy: await terminalProgramCopyFixture(root),
  preservation: await terminalPreservationFixture(root),
  workspaceSwitching: await terminalWorkspaceSwitchFixture(root, 4, true),
  workspaceSwitchingEightPanes: await terminalWorkspaceSwitchFixture(root, 8),
});
window.terminalPreservationFixture = async () => ({
  preservation: await terminalPreservationFixture(root),
  workspaceSwitching: await terminalWorkspaceSwitchFixture(root),
  workspaceSwitchingEightPanes: await terminalWorkspaceSwitchFixture(root, 8),
});
window.finishTerminalPortFixture = () => terminalPortResult;

window.cliPaneAppearanceFixture = createCliPaneAppearanceFixture(root);

window.agentRoutineVisual = theme => agentRoutineVisual(root, theme);

window.workspaceHierarchyFixture = (theme, chrome, count, sidebarSlideOnly) => workspaceHierarchyFixture(root, theme, chrome, count, sidebarSlideOnly);


window.awayFixture = () => awayFixture(root);
window.workflowsFixture = theme => workflowsFixture(root, theme);
window.customizationFixture = () => customizationFixture(root);

window.threadCardFixture = (theme, chrome, mode) => threadCardFixture(root, theme, chrome, mode);
window.threadPaneFixture = (theme, chrome) => threadPaneFixture(root, theme, chrome);
window.threadOpinionFixture = (theme, chrome, pose) => threadOpinionFixture(root, theme, chrome, pose);
window.clearThreadOpinionFixture = () => clearThreadOpinionFixture(root);
window.threadLongTranscriptFixture = (theme, chrome) => threadLongTranscriptFixture(root, theme, chrome);
window.threadLiveFixture = (theme, chrome, pose) => threadLiveFixture(root, theme, chrome, pose);
window.threadSettingsFixture = (theme, chrome) => threadSettingsFixture(root, theme, chrome);
window.clearThreadFixture = () => clearThreadFixture(root);
window.threadComposerFixture = (theme, chrome, pose) => threadComposerFixture(root, theme, chrome, pose);
window.clearThreadComposerFixture = () => clearThreadComposerFixture(root);
window.composerReferencesFixture = theme => composerReferencesFixture(root, theme);
window.clearComposerReferencesFixture = () => clearComposerReferencesFixture(root);

window.outputPreviewFixture = (theme, chrome) => outputPreviewFixture(root, theme, chrome);
window.clearOutputPreviewFixture = () => clearOutputPreviewFixture(root);

window.commitSelectionFixture = () => commitSelectionFixture(root);
