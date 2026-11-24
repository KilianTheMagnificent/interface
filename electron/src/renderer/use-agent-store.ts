import { useCallback, useEffect, useRef, useState } from "react";
import {
  emptyAgentControlResponse, type AgentControlResponse, type AgentMutation, type AgentRequest,
} from "../shared/agents";
import type { IfaceApi } from "../shared/ipc";
import { readAgentProfiles } from "./agent-profiles";
import { agentRequestIsRead } from "./agent-reads";
import type { AgentRequestResult } from "./components/AgentWorkspacePanel";
import { ipcMessage, withoutIpcErrorShape } from "./ipc-message";
import { equalValue } from "./stable-values";
import { startVisiblePolling } from "./visible-polling";

/**
 * The Rust agent store as App holds it: the last acknowledged state, the
 * serialized mutation and request queue, and the read that polls every 5 s
 * while the window shows (and on approval pushes, through `agentReload`).
 * Moved out of App as it was, like useUpdater. Reads do not overlap, and an
 * unchanged answer keeps the current immutable DTO.
 */
export function useAgentStore(api: Pick<IfaceApi, "agentControl">) {
  const [agentStore, setAgentStore] = useState<AgentControlResponse>(emptyAgentControlResponse);
  const agentStateRef = useRef(agentStore);
  const agentQueue = useRef<Promise<unknown>>(Promise.resolve());
  // The poller's own read, reachable from the live-event listener.
  const agentReload = useRef<() => void>(() => undefined);
  const [agentError, setAgentError] = useState<string | null>(null);
  const [agentReady, setAgentReady] = useState(false);
  const [agentPending, setAgentPending] = useState(false);
  const acceptAgentState = useCallback((answer: AgentControlResponse) => {
    // Files, decisions and a new thread's key answer one request; they belong
    // to its caller, not to the store every other read replaces.
    const { calendar: _calendar, work: _work, files: _files, decisions: _decisions, sessionKey: _sessionKey, ...next } = answer;
    if (next.revision < agentStateRef.current.revision) return;
    if (equalValue(agentStateRef.current, next)) return;
    agentStateRef.current = next; setAgentStore(next);
  }, []);
  const mutateAgent = useCallback((command: AgentMutation): Promise<boolean> => {
    const pending = agentQueue.current.then(async () => {
      setAgentPending(true);
      try {
        const next = await api.agentControl({ ...command, expectedRevision: agentStateRef.current.revision });
        acceptAgentState(next); setAgentError(null); return true;
      } catch (error) {
        setAgentError(error instanceof Error ? withoutIpcErrorShape(error.message) : "Agent changes could not be saved.");
        // Reload a conflict, but never replay a command or a provider effect.
        try { acceptAgentState(await api.agentControl({ kind: "read" })); } catch { /* retain last acknowledged state */ }
        return false;
      } finally { setAgentPending(false); }
    });
    agentQueue.current = pending.catch(() => false);
    return pending;
  }, [api, acceptAgentState]);
  /**
   * Host-only and read-side agent commands, serialized with the mutations. A
   * refusal comes back to the page control that asked, which shows it there;
   * it never touches the shared error, so no later read can wipe it.
   */
  const requestAgent = useCallback((command: AgentRequest): Promise<AgentRequestResult> => {
    // A read keeps its place in the queue but leaves the page's controls enabled.
    const busy = !agentRequestIsRead(command);
    const pending = agentQueue.current.then(async (): Promise<AgentRequestResult> => {
      if (busy) setAgentPending(true);
      try {
        const next = await api.agentControl(command);
        acceptAgentState(next); return { ok: true, response: next };
      } catch (error) {
        // Reload, but never replay a command or a provider effect.
        try { acceptAgentState(await api.agentControl({ kind: "read" })); } catch { /* retain last acknowledged state */ }
        return { ok: false, error: ipcMessage(error) };
      } finally { if (busy) setAgentPending(false); }
    });
    agentQueue.current = pending.catch(() => null);
    return pending;
  }, [api, acceptAgentState]);

  useEffect(() => {
    let active = true;
    let loading = false;
    let again = false;
    const load = async (): Promise<void> => {
      if (!active) return;
      // A read asked for mid-read runs right after it: the one in flight may
      // have left before the approval it is being asked about reached Rust.
      if (loading) { again = true; return; }
      loading = true;
      try {
        let next = await api.agentControl({ kind: "read" });
        if (next.revision === 0) {
          const legacy = readAgentProfiles(localStorage);
          next = await api.agentControl({ kind: "importLegacy", ...legacy });
        }
        if (active) { acceptAgentState(next); setAgentReady(true); }
      } catch (error) {
        if (active) setAgentError(error instanceof Error ? withoutIpcErrorShape(error.message) : "Agent storage is unavailable.");
      } finally {
        loading = false;
        if (again) { again = false; void load(); }
      }
    };
    // Approval-driven reloads run hidden too: OS notifications depend on them.
    agentReload.current = () => { void load(); };
    // The periodic refresh reads only while the window shows, and on return.
    const polling = startVisiblePolling(load, 5000);
    if (document.hidden) void load();
    return () => {
      active = false;
      agentReload.current = () => undefined;
      polling();
    };
  }, [api, acceptAgentState]);

  return {
    agentStore, agentStateRef, agentReload, agentError, setAgentError, agentReady, agentPending,
    mutateAgent, requestAgent,
  };
}
