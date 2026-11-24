import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type {
  TerminalLifecycleEvent, TerminalSummary, TerminalWorkspaceRecord,
} from "../../shared/ipc";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import {
  attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE, parseTerminalWorkspaceState,
  serializeTerminalWorkspaceState, terminalPaneForTerminal,
} from "../terminal-workspace";
import { assert, noop, until, wait } from "./fixture-support";

/** One agent pane, a parked shell elsewhere, and the shell Rust hands the agent. */
function handoffCast(base: number, stamp: string) {
  const id = (n: number): string => `00000000-0000-4000-8000-0000000013${(base + n).toString().padStart(2, "0")}`;
  const agent: TerminalSummary = {
    id: id(1), generation: 1, title: "Codex update",
    root: "/fixture/handoff", cwd: "/fixture/handoff", state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "codex" },
    launchSession: { version: 1, id: "01a09b91-1f48-73d0-a37e-4334492620ff", resume: true },
    createdAt: stamp, updatedAt: stamp,
  };
  // Protocol 50: Rust starts this shell itself and names the agent it replaces.
  const shell: TerminalSummary = {
    id: id(2), generation: 1, title: "Shell", root: agent.root,
    cwd: agent.cwd, state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "shell" }, replaces: agent.id, createdAt: stamp, updatedAt: stamp,
  };
  const parked: TerminalSummary = {
    id: id(3), generation: 1, title: "Elsewhere", root: "/fixture/elsewhere",
    cwd: "/fixture/elsewhere", state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "shell" }, createdAt: stamp, updatedAt: stamp,
  };
  const pane = (terminal: TerminalSummary, name: string): string => serializeTerminalWorkspaceState(
    attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE, [terminal.id],
      () => `${name}-pane`, () => `${name}-split`));
  const records: readonly TerminalWorkspaceRecord[] = [
    { id: "handoff-agent", name: "Agent", createdAt: stamp, updatedAt: stamp, terminals: [agent],
      layout: pane(agent, "agent") },
    { id: "handoff-other", name: "Other", createdAt: stamp, updatedAt: stamp, terminals: [parked],
      layout: pane(parked, "other") },
  ];
  return { agent, shell, parked, records };
}

/** A fake host that never expects the renderer to start or end a terminal. */
function handoffHost(known: TerminalSummary[], initial: readonly TerminalWorkspaceRecord[]) {
  const state = {
    records: initial,
    calls: [] as string[],
    lifecycle: undefined as ((event: TerminalLifecycleEvent) => void) | undefined,
  };
  const unexpected = (): never => { throw new Error("Exit hand-off attempted an unrelated operation"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals: [...known] }),
    listTerminalLaunchProfiles: async () => [],
    getTerminalWorkspaces: async () => [...state.records],
    setTerminalWorkspaces: async value => { state.records = value; return value; },
    onTerminalData: () => noop, onTerminalDataFailure: () => noop,
    onTerminalLifecycle: callback => { state.lifecycle = callback; return noop; },
    sendTerminalInput: noop,
    resizeTerminal: async request => ({ ...known.find(t => t.id === request.id)!, size: request.size }),
    createTerminal: async request => { state.calls.push(`create ${request.cwd}`); return unexpected(); },
    dismissTerminal: async id => { state.calls.push(`dismiss ${id}`); return { id, removed: true }; },
    closeTerminal: unexpected, pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const saved = (workspaceId: string) => {
    const record = state.records.find(candidate => candidate.id === workspaceId);
    return {
      filed: record?.terminals.map(entry => entry.id) ?? [],
      paneOf: (terminalId: string) => terminalPaneForTerminal(
        parseTerminalWorkspaceState(record?.layout ?? null), terminalId)?.id,
    };
  };
  return { api, state, saved };
}

const inView = (id: string): boolean => document.querySelector(`[data-terminal-id="${id}:1"]`) !== null;

/**
 * An agent CLI that ends on its own — Codex finishing an update is the case —
 * gets its pane back as a shell from Rust, while its workspace is parked. The
 * renderer starts and dismisses nothing: it moves the parked workspace's saved
 * tab to the shell at once, so the workspace never reads as stopped, and the
 * shell is there when the workspace is shown. A non-zero exit gets no shell
 * from Rust and keeps its tab.
 */
export async function terminalExitHandoffFixture(root: Root): Promise<unknown> {
  const reports = [];
  for (const exit of [{ code: 0 }, { code: 3 }]) {
    const handsOff = exit.code === 0;
    const { agent, shell, parked, records } = handoffCast(0, "2026-10-01T00:00:00Z");
    const known = [agent, parked];
    const { api, state, saved } = handoffHost(known, records);
    const errors: string[] = [];
    let switcher: TerminalWorkspaceSwitcher | null = null;
    try {
      localStorage.setItem("interface.integrated-terminals.workspace.v1", "handoff-other");
      flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
        <IntegratedTerminalCenter api={api} activeCwd="/fixture/elsewhere"
          visible onSwitcherChange={value => { switcher = value; }} onError={message => errors.push(message)} />
      </div>));
      await until(() => switcher !== null && state.lifecycle !== undefined && inView(parked.id),
        () => `The other workspace did not load: ${errors.join("; ")}`);
      assert(!inView(agent.id), "The agent's workspace was shown before it was selected");

      // The agent ends while its workspace is parked; Rust decides the rest.
      state.lifecycle!({ kind: "upsert", revision: 2, terminal: {
        ...agent, state: "exited", exit, updatedAt: "2026-10-01T00:01:00Z" } });
      if (handsOff) {
        known.push(shell);
        state.lifecycle!({ kind: "upsert", revision: 3, terminal: shell });
        state.lifecycle!({ kind: "removed", revision: 4, id: agent.id, generation: 1 });
        await until(() => saved("handoff-agent").paneOf(shell.id) === "agent-pane"
            && saved("handoff-agent").filed.includes(shell.id)
            && !saved("handoff-agent").filed.includes(agent.id),
          () => `The parked workspace did not take Rust's shell: ${JSON.stringify(state.records)}`,
          { timeoutMs: 4000 });
        assert(saved("handoff-agent").paneOf(agent.id) === undefined,
          "The parked workspace still lays out the ended agent");
      }
      await wait(150);
      flushSync(() => switcher!.onSelect("handoff-agent"));
      if (handsOff) {
        await until(() => inView(shell.id),
          () => `A parked agent's pane did not show Rust's shell: ${errors.join("; ")}`, { timeoutMs: 4000 });
        assert(!inView(agent.id), "The ended agent kept its tab beside the shell");
      } else {
        await until(() => inView(agent.id), "A non-zero exit lost its tab");
      }
      // Revisiting changes nothing, and the renderer never started or ended a terminal.
      flushSync(() => switcher!.onSelect("handoff-other"));
      await wait(100);
      flushSync(() => switcher!.onSelect("handoff-agent"));
      await wait(300);
      assert(state.calls.length === 0, `The renderer started or ended terminals: ${state.calls.join(", ")}`);
      assert(errors.length === 0, errors.join("; "));
      reports.push({ exitCode: exit.code, shellShown: inView(shell.id), calls: state.calls.length });
    } finally {
      flushSync(() => root.render(null));
      localStorage.removeItem("interface.integrated-terminals.workspace.v1");
    }
  }
  return reports;
}

/**
 * The agent ends while its workspace is shown, and the owner selects another
 * workspace before Rust's shell arrives. The shell still takes the agent's
 * slot in the agent's workspace; the workspace selected meanwhile is left
 * exactly as it was. Then the window reloads having missed the moment: a
 * fresh renderer that finds Rust's shell and a saved layout still naming the
 * agent makes the same move on load.
 */
export async function terminalHandoffSwitchFixture(root: Root): Promise<unknown> {
  const { agent, shell, parked, records } = handoffCast(10, "2026-10-03T00:00:00Z");
  const known = [agent, parked];
  const { api, state, saved } = handoffHost(known, records);
  const errors: string[] = [];
  let switcher: TerminalWorkspaceSwitcher | null = null;
  try {
    localStorage.setItem("interface.integrated-terminals.workspace.v1", "handoff-agent");
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
      <IntegratedTerminalCenter api={api} activeCwd="/fixture/handoff"
        visible onSwitcherChange={value => { switcher = value; }} onError={message => errors.push(message)} />
    </div>));
    await until(() => switcher !== null && state.lifecycle !== undefined && inView(agent.id),
      () => `The agent's workspace did not load: ${errors.join("; ")}`);

    state.lifecycle!({ kind: "upsert", revision: 2, terminal: {
      ...agent, state: "exited", exit: { code: 0 }, updatedAt: "2026-10-03T00:01:00Z" } });
    flushSync(() => switcher!.onSelect("handoff-other"));
    await until(() => inView(parked.id), "The other workspace did not open");

    known.splice(known.indexOf(agent), 1, shell);
    state.lifecycle!({ kind: "upsert", revision: 3, terminal: shell });
    state.lifecycle!({ kind: "removed", revision: 4, id: agent.id, generation: 1 });
    await until(() => saved("handoff-agent").paneOf(shell.id) === "agent-pane",
      () => `The shell did not take the agent's slot: ${JSON.stringify(state.records)}`);
    await wait(150);
    assert(!inView(shell.id), "The shell opened in the workspace selected while it started");
    assert(inView(parked.id), "The workspace selected meanwhile lost its own pane");
    const other = saved("handoff-other");
    assert(other.paneOf(shell.id) === undefined && !other.filed.includes(shell.id),
      "The workspace selected meanwhile saved the shell");
    assert(other.paneOf(parked.id) === "other-pane", "The workspace selected meanwhile lost its layout");

    flushSync(() => switcher!.onSelect("handoff-agent"));
    await until(() => inView(shell.id), () => `The agent's workspace did not show its shell: ${errors.join("; ")}`);
    assert(!inView(agent.id), "The ended agent kept its tab beside the shell");
    await until(() => saved("handoff-agent").filed.includes(shell.id)
        && !saved("handoff-agent").filed.includes(agent.id),
      () => `The agent's workspace did not file the shell in the agent's place: ${JSON.stringify(state.records)}`);
    assert(state.calls.length === 0, `The renderer started or ended terminals: ${state.calls.join(", ")}`);
    assert(errors.length === 0, errors.join("; "));
  } finally {
    flushSync(() => root.render(null));
    localStorage.removeItem("interface.integrated-terminals.workspace.v1");
  }

  // A reload that missed the moment: Rust's shell is live, the saved roster still names the agent.
  const reload = handoffCast(20, "2026-10-03T00:00:00Z");
  const reloadHost = handoffHost([reload.shell, reload.parked], reload.records);
  let reloadSwitcher: TerminalWorkspaceSwitcher | null = null;
  try {
    localStorage.setItem("interface.integrated-terminals.workspace.v1", "handoff-agent");
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
      <IntegratedTerminalCenter api={reloadHost.api} activeCwd="/fixture/handoff"
        visible onSwitcherChange={value => { reloadSwitcher = value; }} onError={message => errors.push(message)} />
    </div>));
    await until(() => reloadSwitcher !== null && inView(reload.shell.id),
      () => `A reloaded window did not move the tab to Rust's shell: ${errors.join("; ")}`
        + ` ${JSON.stringify(reloadHost.state.records)}`, { timeoutMs: 4000 });
    assert(reloadHost.saved("handoff-agent").filed.includes(reload.shell.id)
      && !reloadHost.saved("handoff-agent").filed.includes(reload.agent.id),
      "A reloaded window kept the ended agent's saved entry");
    assert(reloadHost.state.calls.length === 0,
      `A reloaded window started or ended terminals: ${reloadHost.state.calls.join(", ")}`);
    assert(errors.length === 0, errors.join("; "));
    return { scenario: "switched-while-shell-starts-then-reloaded", calls: state.calls.length };
  } finally {
    flushSync(() => root.render(null));
    localStorage.removeItem("interface.integrated-terminals.workspace.v1");
  }
}
