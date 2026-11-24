import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import { attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE, serializeTerminalWorkspaceState } from "../terminal-workspace";
import { assert, noop, wait } from "./fixture-support";

/** Cold saved panes, rather than the already-running registry used by the
 * interaction fixture. A host startup refusal must survive sibling saves and
 * navigating away while the bounded reopen is in flight. No live providers. */
export async function terminalReopenFixture(root: Root): Promise<unknown> {
  const reports = [];
  for (const scenario of ["sibling-save", "navigate", "refused", "roster-error"] as const) {
    const navigate = scenario === "navigate";
    const failure = scenario === "refused" || scenario === "roster-error";
    const stamp = "2026-09-24T00:00:00Z";
    const terminals: TerminalSummary[] = Array.from({ length: 4 }, (_, index) => ({
      id: `00000000-0000-4000-8000-${String(981 + index).padStart(12, "0")}`,
      generation: 1, title: `Saved Claude ${index}`, root: "/fixture/reopen", cwd: "/fixture/reopen",
      state: "running", size: { columns: 80, rows: 24 },
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: `00000000-0000-4000-8001-${String(981 + index).padStart(12, "0")}`, resume: true },
      createdAt: stamp, updatedAt: stamp,
    }));
    const record: TerminalWorkspaceRecord = { id: "reopen-workspace", name: "Saved agents",
      createdAt: stamp, updatedAt: stamp, terminals,
      layout: serializeTerminalWorkspaceState(attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE,
        terminals.map(t => t.id), i => `reopen-pane-${i}`, i => `reopen-split-${i}`)) };
    let records: readonly TerminalWorkspaceRecord[] = [record, { ...record, id: "other-workspace",
      name: "Other", terminals: [], layout: serializeTerminalWorkspaceState(EMPTY_TERMINAL_WORKSPACE_STATE) }];
    const calls = new Map<string, number>();
    const errors: string[] = [];
    let switcher: TerminalWorkspaceSwitcher | null = null;
    let receive: Parameters<TerminalRendererApi["onTerminalData"]>[0] | undefined;
    const unexpected = (): never => { throw new Error("Cold reopen attempted an unrelated operation"); };
    const api: TerminalRendererApi = {
      listTerminals: async () => ({ revision: 1, terminals: [] }),
      listTerminalLaunchProfiles: async () => [{ id: "claude", label: "Claude Code", available: true }],
      getTerminalWorkspaces: async () => {
        if (scenario === "roster-error" && calls.size > 0) throw new Error("Host roster unavailable");
        return [...records];
      },
      setTerminalWorkspaces: async value => { records = value; return value; },
      onTerminalData: callback => { receive = callback; return noop; },
      onTerminalLifecycle: () => noop, onTerminalDataFailure: () => noop,
      sendTerminalInput: noop,
      resizeTerminal: async request => ({ ...terminals.find(t => t.id === request.id)!, size: request.size }),
      createTerminal: async request => {
        const count = (calls.get(request.id) ?? 0) + 1;
        calls.set(request.id, count);
        const terminal = terminals.find(t => t.id === request.id)!;
        assert(request.launchSession?.id === terminal.launchSession?.id && request.launchSession?.resume,
          "A retry changed the saved conversation identity");
        // One successful sibling writes a fresh workspace object while the
        // other three are backing off. Navigating away is a second invalidator.
        if ((count === 1 || failure) && (navigate || request.id !== terminals[0].id)) {
          throw new Error("terminal-authority-unavailable");
        }
        return terminal;
      },
      closeTerminal: unexpected, dismissTerminal: unexpected, pathForFile: unexpected,
      pickDirectory: unexpected, savePastedImages: unexpected, startAdversarialReview: unexpected,
    };
    const until = async (ready: () => boolean, message: string): Promise<void> => {
      const deadline = performance.now() + 4000;
      while (!ready()) {
        assert(performance.now() < deadline,
          `${message}; attempts=${JSON.stringify([...calls.values()])}; errors=${errors.join("; ")}`);
        await wait(10);
      }
    };
    const readyPanes = (): number => document.querySelectorAll(".terminal-view[data-terminal-ready]").length;
    try {
      localStorage.setItem("interface.integrated-terminals.workspace.v1", record.id);
      flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
        <IntegratedTerminalCenter api={api} activeCwd="/fixture/reopen"
          visible onSwitcherChange={value => { switcher = value; }} onError={message => errors.push(message)} />
      </div>));
      await until(() => calls.size === 4 && switcher !== null, "Cold saved panes did not attempt to reopen");
      if (failure) {
        await until(() => document.querySelectorAll(".terminal-reopen-failure").length === 3,
          "Refused saved panes stayed blank instead of showing the reason");
        const expected = scenario === "roster-error" ? "Host roster unavailable" : "terminal-authority-unavailable";
        assert(errors.length === 3 && errors.every(error => error.includes(expected)),
          `Missing reopen failures: ${errors.join("; ")}`);
        assert(document.querySelectorAll(".terminal-reopen-failure-reason").length === 3,
          "Failed panes did not render their reasons");
        reports.push({ scenario, attempts: [...calls.values()], visibleFailures: 3 });
        continue;
      }
      if (navigate) flushSync(() => switcher!.onSelect("other-workspace"));
      await until(() => [...calls.values()].filter(count => count === 2).length === (navigate ? 4 : 3),
        "A workspace update abandoned the saved panes");
      if (navigate) flushSync(() => switcher!.onSelect(record.id));
      await until(() => readyPanes() === 4, "The resumed panes did not mount xterm");
      for (const terminal of terminals) {
        receive!({ kind: "data", terminalId: terminal.id, generation: 1, sequence: "0",
          data: new TextEncoder().encode(`Restored ${terminal.title}\r\n`) }, noop);
      }
      await until(() => [...document.querySelectorAll(".xterm-accessibility-tree")]
        .filter(node => node.textContent?.includes("Restored Saved Claude")).length === 4,
      "The resumed panes did not render output");
      assert(errors.length === 0, errors.join("; "));
      assert([...calls.values()].every(count => count <= 2), "Returning to the workspace launched a duplicate");
      reports.push({ scenario, attempts: [...calls.values()], readyPanes: readyPanes(), outputVisible: true });
    } finally {
      flushSync(() => root.render(null));
      localStorage.removeItem("interface.integrated-terminals.workspace.v1");
    }
  }
  return reports;
}
