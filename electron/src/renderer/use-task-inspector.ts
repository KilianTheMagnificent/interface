import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ComponentProps } from "react";
import type { SessionKey } from "../shared/engine";
import type {
  AttentionTarget as DurableAttentionTarget, HarnessCommand, HarnessView, TerminalServerLink,
} from "../shared/harness";
import type { IfaceApi } from "../shared/ipc";
import { harnessAttentionEvents, type AttentionRow } from "./attention-tray";
import type { TaskInspector } from "./components/TaskInspector";
import { ipcMessage } from "./ipc-message";
import { equalValue } from "./stable-values";

export type TaskInspectorApi = Pick<IfaceApi,
  "harnessControl" | "inspectTerminalServers" | "openWorkspaceTool">;

/** The checkout, and the session or exact terminal process, an inspection is about. */
export interface TaskTarget { root: string; target: DurableAttentionTarget }

export type TaskInspectorKind = "read" | "context" | "presets";

/** What the inspector needs from App. */
export type TaskInspectorHost = {
  /** Replace the tray's event rows with the harness's attention history. */
  replaceAttentionEvents: (events: AttentionRow[]) => void;
  reportActionError: (message: string) => void;
  /** The focused terminal pane or session, for an inspector opened without a target. */
  focusedTarget: () => TaskTarget | null;
  /** The exact terminal process is still running. */
  terminalRunning: (terminalId: string, generation: number) => boolean;
  selectSession: (key: SessionKey) => void;
  /** Append a follow-up to a conversation's draft. */
  appendConversationDraft: (key: SessionKey, text: string) => void;
  /** A fresh composer on `root`, set to work in a new worktree. */
  openIsolatedComposer: (root: string) => void;
  /** Show `url` in a Browser pane beside this terminal; false when it cannot. */
  openBrowserPane: (terminalId: string, url: string) => boolean;
};

type TaskInspectorProps = ComponentProps<typeof TaskInspector>;

/**
 * The harness (Rust-owned task history, results, drafts and attention) and
 * the task inspector over it. `request` serializes every harness command on
 * one lane; `openInspector` opens it on a target; `inspectorProps` is the
 * open inspector's props. App binds `host` once its callbacks exist.
 */
export function useTaskInspector(api: TaskInspectorApi,
    hostRef: { readonly current: TaskInspectorHost | null }) {
  const [view, setView] = useState<HarnessView | null>(null);
  const [isOpen, setIsOpen] = useState(false);
  const [servers, setServers] = useState<TerminalServerLink[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lane = useRef<Promise<void>>(Promise.resolve());
  // Bumped on every open and close, so a late answer for an inspection the
  // user has left updates the shared state but not the inspector's result.
  const epoch = useRef(0);
  const target = useRef<TaskTarget | null>(null);
  const host = useCallback((): TaskInspectorHost => {
    if (hostRef.current === null) throw new Error("The task inspector ran before App bound it.");
    return hostRef.current;
  }, [hostRef]);

  const request = useCallback((command: HarnessCommand): Promise<void> => {
    const requestEpoch = epoch.current;
    const operation = lane.current.then(async () => {
      const answer = await api.harnessControl(command);
      // The read every window focus makes usually answers what is held
      // already; an equal view or history keeps its identity and renders nothing.
      setView(current => {
        if (current !== null && current.state.revision > answer.state.revision) return current;
        const next = { ...current, ...(requestEpoch === epoch.current ? answer : { version: answer.version, state: answer.state, catalog: answer.catalog }) };
        return current !== null && equalValue(current, next) ? current : next;
      });
      if (answer.result && requestEpoch === epoch.current) target.current = { root: answer.result.root, target: answer.result.target };
      host().replaceAttentionEvents(harnessAttentionEvents(answer.state));
    });
    lane.current = operation.catch((failure: unknown) => {
      setError(ipcMessage(failure));
      if (command.kind === "recordAttention") host().reportActionError("Attention history could not be saved. " + ipcMessage(failure));
    });
    return operation;
  }, [api, host]);

  useEffect(() => {
    void request({ kind: "read" }).catch(() => undefined);
    const refresh = (): void => { void request({ kind: "read" }).catch(() => undefined); };
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [request]);

  /** Busy while one inspector action runs. */
  const busyWhile = useCallback((run: Promise<void>): void => {
    setBusy(true);
    void run.catch(() => undefined).finally(() => setBusy(false));
  }, []);

  const openInspector = useCallback((kind: TaskInspectorKind, explicit?: TaskTarget) => {
    epoch.current += 1;
    const selected = explicit ?? host().focusedTarget();
    target.current = selected; setServers([]);
    setIsOpen(true); setError(null);
    setView(current => current === null ? null : { version: 1, state: current.state, catalog: current.catalog });
    let command: HarnessCommand;
    if (kind === "read") command = { kind: "read" };
    else if (selected === null) { setError("Select a session or terminal in an open project first."); return; }
    else command = kind === "context" ? { kind, root: selected.root, query: "" } : { kind, root: selected.root };
    busyWhile(request(command));
  }, [busyWhile, host, request]);

  const actions = useMemo(() => ({
    onOpenSession: (key: string) => { setIsOpen(false); host().selectSession(key); },
    onFindServers: () => {
      const selected = target.current;
      if (selected?.target.kind !== "terminal") { setError("Select a running terminal to discover its servers."); return; }
      setBusy(true);
      void api.inspectTerminalServers({ id: selected.target.terminalId, generation: selected.target.generation }).then(setServers)
        .catch((failure: unknown) => setError(ipcMessage(failure))).finally(() => setBusy(false));
    },
    onOpenServer: (server: TerminalServerLink) => {
      const selected = target.current;
      if (selected?.target.kind !== "terminal") return;
      const { terminalId } = selected.target;
      void api.inspectTerminalServers({ id: terminalId, generation: selected.target.generation }).then(current => {
        if (!current.some(item => item.processId === server.processId && item.url === server.url)) throw new Error("That owned listener is no longer available.");
        // In the app, beside the terminal serving it; the default browser only
        // when there is no terminal centre to hold a Browser pane.
        if (host().openBrowserPane(terminalId, server.url)) { epoch.current += 1; setIsOpen(false); return; }
        return api.openWorkspaceTool({ version: 1, root: selected.root, tool: "browser", url: server.url });
      }).catch((failure: unknown) => setError(ipcMessage(failure)));
    },
    onClose: () => { epoch.current += 1; setIsOpen(false); },
    onCapture: (objective: string) => {
      const selected = target.current;
      if (!selected) { setError("Select a task in an open project before capturing a result."); return; }
      busyWhile(request({ kind: "captureResult", id: crypto.randomUUID(), objective, ...selected }));
    },
    onRunChecks: (id: string, selected: string[]) => { busyWhile(request({ kind: "runChecks", id, selected })); },
    onInspect: (id: string) => { busyWhile(request({ kind: "inspectResult", id })); },
    onContext: (query: string) => { const selected = target.current; if (selected) void request({ kind: "context", root: selected.root, query }).catch(() => undefined); },
    // The one palette row opens the dialog on results; these load its other
    // two parts in place, for the checkout the dialog is about.
    onShowContext: () => {
      const selected = target.current;
      if (!selected) { setError("Select a session or terminal in an open project first."); return; }
      busyWhile(request({ kind: "context", root: selected.root, query: "" }));
    },
    onShowPresets: () => {
      const selected = target.current;
      if (!selected) { setError("Select a session or terminal in an open project first."); return; }
      busyWhile(request({ kind: "presets", root: selected.root }));
    },
    onAttach: (text: string) => {
      const selected = target.current;
      if (selected?.target.kind === "terminal") {
        const exact = selected.target;
        if (!host().terminalRunning(exact.terminalId, exact.generation)) { setError("The original terminal process is no longer available."); return; }
        void api.harnessControl({ kind: "readDraft", target: exact }).then(draftView => {
          if (!draftView.draft) throw new Error("Saved draft unavailable.");
          return api.harnessControl({ kind: "saveDraft", target: exact, expectedRevision: draftView.draft.revision, text: [draftView.draft.text, text].filter(Boolean).join("\n\n") });
        }).then(() => setError("Attached to this terminal's saved follow-up draft. Local conflicting edits are preserved; reload the draft if needed.")).catch((failure: unknown) => setError(ipcMessage(failure)));
        return;
      }
      if (selected?.target.kind !== "session") { setError("Select a conversation before attaching a follow-up."); return; }
      const key = selected.target.key;
      host().appendConversationDraft(key, text);
      setIsOpen(false); host().selectSession(key);
    },
    onPreset: (isolate: boolean) => {
      const selected = target.current; if (!selected) return;
      if (isolate) { setIsOpen(false); host().openIsolatedComposer(selected.root); }
      else { void request({ kind: "context", root: selected.root, query: "" }).catch(() => undefined); }
    },
  }), [api, busyWhile, host, request]);

  const inspectorProps: TaskInspectorProps = useMemo(() => ({
    ...actions, view, busy, error, servers,
  }), [actions, busy, error, servers, view]);

  return { view, isOpen, request, openInspector, inspectorProps };
}
