import { AgentCalendarPanel } from "./AgentCalendarPanel";
import { AgentWorkPanel } from "./AgentWorkPanel";
import { useEffect, useReducer, useRef, useState, type ReactNode } from "react";
import {
  agentPermissionMode, type AgentControlResponse, type AgentDecision, type AgentDecisionKind, type AgentFileText,
  type AgentFiles, type AgentMutation, type AgentPermissionMode, type AgentProfile, type AgentRequest,
  type AgentRoutine, type AgentStatus,
} from "../../shared/agents";
import type { EngineProbe } from "../../shared/ipc";
import { newAgentId } from "../agent-profiles";
import { equalValue } from "../stable-values";
import { AGENT_PERMISSION_HELP, AGENT_PERMISSION_LABELS } from "./AgentMenu";

export { AGENT_PERMISSION_HELP };

export type AgentWorkspaceView = "thread" | "memory" | "routines" | "runs" | "computer";
/** A page request's answer, or the host's refusal in words for the control that asked. */
export type AgentRequestResult =
  | { ok: true; response: AgentControlResponse }
  | { ok: false; error: string };
export type AgentRequestHandler = (command: AgentRequest) => Promise<AgentRequestResult>;
export interface AgentWorkspacePanelProps {
  agent: AgentProfile; response: AgentControlResponse; pending: boolean; error: string | null;
  /** The conversation App mounts for `shownSessionKey`. */
  children: ReactNode;
  /** The session App has focused on the Agent surface, whatever it is: the thread, a run, an older task. */
  shownSessionKey: string | null;
  /** The Agent surface is on screen and the window is visible, so the owner can see this page. */
  visible: boolean;
  /** The newest message the owner sent in the current thread, by echo id. */
  threadLastSendId?: string;
  onCommand: (command: AgentMutation) => Promise<boolean>;
  /** Host-only and read-side commands. A refusal comes back for the page to show where it happened. */
  onRequest: AgentRequestHandler;
  onOpenTask: (key: string) => void;
  /** Mount a past thread (read-only) under the Thread tab. */
  onOpenThread: (sessionKey: string) => void;
  /** Show this agent's thread: its current session, or the first-message form when it has none. */
  onShowThread: () => void;
  onPickDirectory: () => Promise<string | null>;
  /** The tab a fresh page opens on. Thread unless a caller needs another. */
  initialView?: AgentWorkspaceView;
  provider?: EngineProbe;
}

export const AGENT_MEMORY_CONFLICT = "MEMORY.md changed since you opened it. Reloaded.";
export const AGENT_MEMORY_TOO_LARGE = "Memory is larger than 256 KiB";
/** The host refuses a larger save; checked here first so nothing is sent. */
export const AGENT_MEMORY_MAX_BYTES = 256 * 1024;
export const AGENT_ASKS_BEFORE = "Requests approval for recognized risky actions. Scripts and provider-native tools can bypass this policy; it is not a security boundary.";
export const AGENT_PROVIDER_NOTE = "Claude uses tool approval hooks. Codex uses a verified workspace sandbox; external MCP servers, connectors, browser, computer use and native delegation are disabled. Interface task tools are attached only to active scoped work. Other providers cannot start gated agents.";
export const AGENT_PAST_THREAD = "This is a past conversation. It's read-only.";
const AGENT_INTERRUPTED = "Interface restarted while this agent was working. Check the conversation before continuing.";
export const AGENT_DECISION_LABELS: Readonly<Record<AgentDecisionKind, string>> = {
  allowed: "Allowed", asked: "Asked you", ownerAllowed: "You allowed", ownerDenied: "You denied",
  failedClosed: "Refused on error",
};
const TABS: readonly AgentWorkspaceView[] = ["thread", "memory", "routines", "runs", "computer"];

/** What a control last said: a refusal (alert) or a quiet confirmation. */
export interface AgentMessage { text: string; alert: boolean }
export type AgentControl = "send" | "save" | "recreate" | "instructions" | "reveal" | "files" | "decisions" | "folders";
export type AgentMessages = Partial<Record<AgentControl, AgentMessage>>;

/**
 * What the Thread tab draws. Whatever session App focused wins — a run opened
 * from the tray, an older task, a past thread — and the page moves to the
 * current thread only when the owner asks (the Thread tab, or the roster).
 */
export function agentThreadPane(currentKey: string | undefined, shownSessionKey: string | null):
    "current" | "other" | "first-message" | "open-current" {
  if (shownSessionKey !== null && shownSessionKey !== currentKey) return "other";
  if (currentKey === undefined) return "first-message";
  return shownSessionKey === currentKey ? "current" : "open-current";
}

/**
 * Reading happens when the owner has the thread itself open and can see it.
 * Never behind another surface, and never while the Thread tab shows a run, an
 * older task, a past thread or only an Open thread button: those do not open
 * the thread, and reading would clear needs-attention for one nobody saw.
 */
export function shouldMarkRead({ view, pane, visible, status, interrupted }: {
  view: AgentWorkspaceView; pane: ReturnType<typeof agentThreadPane>; visible: boolean;
  status: AgentStatus | undefined; interrupted: boolean;
}): boolean {
  return view === "thread" && pane === "current" && visible && (status === "unread" || interrupted);
}

/**
 * The restart notice outlives the read that clears `interrupted`: once seen it
 * stays until the owner dismisses it or sends a message in that thread. Per
 * thread key; a later restart of the same thread latches it again.
 */
export type RestartLatches = Map<string, { open: boolean; sendId: string | undefined; cleared: boolean }>;
export function restartNoticeOpen(latches: RestartLatches, key: string, interrupted: boolean,
    sendId: string | undefined): boolean {
  let latch = latches.get(key);
  if (interrupted && (latch === undefined || latch.cleared)) {
    latch = { open: true, sendId, cleared: false };
    latches.set(key, latch);
  } else if (!interrupted && latch !== undefined) {
    latch.cleared = true;
  }
  if (latch === undefined) return false;
  if (latch.open && latch.sendId !== sendId) latch.open = false;
  // Nothing left to remember: closed, and no restart still being reported.
  if (!latch.open && latch.cleared) latches.delete(key);
  return latch.open;
}
export function dismissRestartNotice(latches: RestartLatches, key: string): void {
  const latch = latches.get(key);
  if (latch === undefined) return;
  latch.open = false;
  if (latch.cleared) latches.delete(key);
}
// Kept beside the component rather than in it: switching agents remounts the
// page, and a notice the owner has not acted on must survive that.
const restartLatches: RestartLatches = new Map();

/** The first message creates the thread. Null when there is nothing to send. */
export async function sendFirstThreadMessage(agentId: string, draft: string, onRequest: AgentRequestHandler,
    requestId: string): Promise<AgentRequestResult | null> {
  const text = draft.trim();
  if (text === "") return null;
  return await onRequest({ kind: "startAgentThread", agentId, text, requestId });
}

/**
 * Save MEMORY.md against the hash it was opened with. A refused save is never
 * replayed: the file is read again, and a different hash on disk is the
 * conflict the host refused on, so the page shows the file as it now is. Any
 * other refusal keeps the draft (`files: null`) and says the host's reason.
 */
export async function saveMemoryDraft(agentId: string, loaded: AgentFileText, text: string,
    onRequest: AgentRequestHandler): Promise<{ files: AgentFiles | null; notice: AgentMessage | null }> {
  if (new TextEncoder().encode(text).byteLength > AGENT_MEMORY_MAX_BYTES) {
    return { files: null, notice: { text: AGENT_MEMORY_TOO_LARGE, alert: true } };
  }
  const saved = await onRequest({ kind: "saveAgentMemory", agentId, expectedSha256: loaded.sha256, text });
  const reread = saved.ok && saved.response.files !== undefined ? null
    : await onRequest({ kind: "readAgentFiles", agentId });
  const files = saved.ok && saved.response.files !== undefined ? saved.response.files
    : reread?.ok === true ? reread.response.files ?? null : null;
  if (saved.ok) return { files, notice: { text: "MEMORY.md saved.", alert: false } };
  return files !== null && files.memory?.sha256 !== loaded.sha256
    ? { files, notice: { text: AGENT_MEMORY_CONFLICT, alert: true } }
    : { files: null, notice: { text: saved.error, alert: true } };
}

/** A control's own line, directly under it; refusals are alerts. */
function ControlMessage({ message }: { message: AgentMessage | undefined }) {
  if (message === undefined) return null;
  return <p role={message.alert ? "alert" : "status"}
    className={message.alert ? "agent-control-message is-alert" : "agent-control-message"}>{message.text}</p>;
}

const PAST_THREAD_CSS = `
.agent-past-thread {display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px;padding:12px 16px;color:var(--fg-muted);font-family:var(--font-ui);font-size:var(--text-ui);line-height:var(--lh-ui);border-top:var(--border-w) solid var(--border)}
.agent-past-thread p {flex:1;min-width:180px;margin:0}
`;

/** Stands where the composer would be: a past thread takes no messages, resumes nothing and answers nothing. */
export function AgentPastThreadNotice({ hasCurrent, onBack }: { hasCurrent: boolean; onBack: () => void }) {
  return <div className="agent-past-thread" role="status">
    <style>{PAST_THREAD_CSS}</style>
    <p>{AGENT_PAST_THREAD}</p>
    <button type="button" className="btn" onClick={onBack}>{hasCurrent ? "Back to current conversation" : "Back to conversation"}</button>
  </div>;
}

const CSS = `
.agent-workspace {display:flex;flex-direction:column;min-height:0;overflow:hidden;font-family:var(--font-ui)}
.agent-workspace > .agent-task {flex:1;min-height:0;height:auto}
.agent-workspace-nav {display:flex;flex:none;gap:14px;padding:0 18px;border-bottom:var(--border-w) solid var(--border);overflow:auto}
.agent-workspace-nav button {font:inherit;font-size:var(--text-ui);color:var(--fg-muted);background:transparent;border:0;border-radius:0;height:32px;padding:0 2px;cursor:pointer;white-space:nowrap}
.agent-workspace-nav button:hover {color:var(--fg-strong)}
/* A horizontal strip selects with the underline, as theme.css states. */
.agent-workspace-nav button[aria-pressed=true] {color:var(--fg-strong);box-shadow:inset 0 -2px 0 var(--selection-accent)}
.agent-switch {display:inline-flex;align-items:center;gap:6px;color:var(--fg-muted);font-size:var(--text-ui);cursor:pointer}
.agent-switch input {appearance:none;width:28px;height:16px;margin:0;border-radius:var(--radius-pill);background:var(--surface-raised);box-shadow:inset 0 0 0 var(--border-w) var(--border-strong);position:relative;cursor:pointer}
.agent-switch input::after {content:"";position:absolute;top:3px;left:3px;width:10px;height:10px;border-radius:50%;background:var(--fg-muted);transition:transform var(--motion-fast) var(--ease-out)}
.agent-switch input:checked {background:var(--accent);box-shadow:none}
.agent-switch input:checked::after {background:var(--accent-fg);transform:translateX(12px)}
.agent-workspace-icon {display:inline-grid;place-items:center;width:var(--control-h);height:var(--control-h);padding:0;color:var(--fg-muted);background:transparent;border:0;border-radius:var(--radius-control);cursor:pointer}
.agent-workspace-icon:hover:not(:disabled) {color:var(--fg-strong);background:var(--row-hover)}
.agent-workspace-body {flex:1;min-height:0;overflow:auto;padding:24px;max-width:900px;width:100%;box-sizing:border-box;align-self:center}
.agent-workspace-body h2 {font-size:var(--text-prose);font-weight:var(--wght-strong);margin:0 0 8px;color:var(--fg-strong)}
.agent-workspace-body p {color:var(--fg-muted);line-height:var(--lh-ui);font-size:var(--text-ui);margin:0 0 18px}
.agent-workspace-row {display:flex;flex-wrap:wrap;align-items:center;gap:12px;padding:16px 0;border-bottom:var(--border-w) solid var(--border)}
.agent-workspace-copy {flex:1;min-width:180px;overflow-wrap:anywhere}.agent-workspace-copy strong {font-weight:var(--wght-medium);color:var(--fg-strong)}
.agent-workspace-copy small {display:block;color:var(--fg-muted);font-size:var(--text-meta);margin-top:5px;line-height:var(--lh-ui)}
.agent-work-controls {margin:0 0 24px}.agent-work-controls form,.agent-calendar-controls form {display:grid;gap:12px}.agent-work-controls label {display:grid;gap:5px}.agent-work-controls label:has(> input[type=checkbox]) {display:flex;align-items:center;gap:8px}.agent-work-controls input[type=checkbox] {flex:none;width:auto;margin:0}.agent-work-controls pre {white-space:pre-wrap;max-height:280px;overflow:auto}.agent-readiness {border:var(--border-w) solid var(--border);border-radius:var(--radius-control);padding:12px;margin-bottom:18px}.agent-readiness p:last-child {margin-bottom:0}
.agent-calendar-controls {margin-top:8px}.agent-calendar-controls > summary {cursor:pointer;color:var(--fg);font-size:var(--text-meta);font-weight:var(--wght-medium);padding:4px 0}.agent-calendar-controls > summary:hover {color:var(--fg-strong)}.agent-calendar-controls > label {display:grid;gap:6px;margin:12px 0;color:var(--fg-muted);font-size:var(--text-meta)}.agent-calendar-controls > label:has(input[type=checkbox]) {display:flex;align-items:center;gap:8px}.agent-calendar-controls input[type=checkbox] {margin:0}.agent-calendar-controls fieldset {margin:12px 0;padding:10px 12px;border:var(--border-w) solid var(--border);border-radius:var(--radius-control)}.agent-calendar-controls legend {padding:0 4px;color:var(--fg-muted);font-size:var(--text-meta)}.agent-calendar-controls fieldset label {display:inline-flex;align-items:center;gap:5px;margin-right:12px;color:var(--fg);font-size:var(--text-ui)}.agent-calendar-controls ul {margin:8px 0 0;padding-left:18px;color:var(--fg-muted);font-size:var(--text-meta);font-variant-numeric:tabular-nums}
.agent-workspace-actions {display:flex;flex-wrap:wrap;gap:6px}
.agent-workspace button:focus-visible, .agent-workspace summary:focus-visible {outline:2px solid var(--focus);outline-offset:2px}
.agent-routine-form {display:flex;flex-direction:column;gap:14px;margin-top:22px;padding:20px;background:var(--widget-bg);border:var(--border-w) solid var(--border);border-radius:var(--radius-overlay)}
.agent-routine-form label {display:flex;flex-direction:column;gap:6px;color:var(--fg-muted);font-size:var(--text-meta)}
.agent-routine-form .agent-routine-check {flex-direction:row;align-items:center}
.agent-routine-fields {display:flex;flex-wrap:wrap;gap:14px}.agent-routine-fields > label {flex:1;min-width:180px}
.agent-workspace-empty {margin-top:24px;padding:24px 0;border-top:var(--border-w) solid var(--border)}
.agent-workspace-error {position:fixed;bottom:20px;left:50%;transform:translateX(-50%);max-width:600px;z-index:1300;background:var(--widget-bg);color:var(--fg);padding:12px 18px;border:1px solid var(--border-strong);border-radius:var(--radius-overlay);box-shadow:var(--overlay-shadow)}
/* Thread: the conversation fills the page under one quiet action row. */
.agent-workspace > .agent-thread-bar {display:flex;flex:none;flex-wrap:wrap;align-items:center;gap:6px 10px;padding:6px 18px;border-bottom:var(--border-w) solid var(--border)}
.agent-thread-notice {flex:1;min-width:200px;margin:0;color:var(--fg);font-size:var(--text-ui);line-height:var(--lh-ui)}
.agent-thread-actions {display:flex;gap:6px;margin-left:auto}
.agent-thread-past {position:relative}
.agent-thread-past > summary {display:inline-flex;align-items:center;list-style:none}
.agent-thread-past > summary::-webkit-details-marker {display:none}
.agent-thread-past-menu {position:absolute;right:0;top:calc(100% + 4px);z-index:30;display:flex;flex-direction:column;min-width:220px;padding:4px;background:var(--widget-bg);border:var(--border-w) solid var(--border-strong);border-radius:var(--radius-overlay);box-shadow:var(--overlay-shadow)}
.agent-thread-past-menu button {font:inherit;font-size:var(--text-ui);text-align:left;color:var(--fg);background:transparent;border:0;border-radius:var(--radius-control);padding:6px 8px;cursor:pointer}
.agent-thread-past-menu button:hover {background:var(--row-hover)}
.agent-thread-first, .agent-memory-form {display:flex;flex-direction:column;gap:10px}
.agent-thread-first label, .agent-memory-form label {color:var(--fg-muted);font-size:var(--text-meta)}
.agent-memory-form {margin:16px 0 12px}
.agent-memory-compare {display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;margin:10px 0}
.agent-memory-compare pre {margin:6px 0 0;padding:10px;max-height:220px;overflow:auto;white-space:pre-wrap;overflow-wrap:anywhere;font-family:var(--font-mono);font-size:var(--text-meta);color:var(--fg);background:var(--widget-bg);border:var(--border-w) solid var(--border);border-radius:var(--radius-control)}
.agent-decisions {list-style:none;margin:0;padding:0}
.agent-decision {display:flex;flex-wrap:wrap;align-items:baseline;gap:4px 12px;padding:10px 0;border-bottom:var(--border-w) solid var(--border);font-size:var(--text-ui)}
.agent-decision time {flex:none;color:var(--fg-dim);font-size:var(--text-meta);font-variant-numeric:tabular-nums}
.agent-decision-tool {flex:none;color:var(--fg-strong);font-family:var(--font-mono);font-size:var(--text-meta)}
.agent-decision-summary {flex:1;min-width:140px;color:var(--fg-muted);overflow-wrap:anywhere}
.agent-workspace-body .agent-control-message {flex-basis:100%;margin:6px 0 0;color:var(--fg-muted);font-size:var(--text-ui)}
.agent-workspace-body .agent-control-message.is-alert {color:var(--status-failed)}
.agent-thread-dismiss {display:inline-grid;place-items:center;width:var(--control-h);height:var(--control-h);padding:0;color:var(--fg-muted);background:transparent;border:0;border-radius:var(--radius-control);cursor:pointer}
.agent-thread-dismiss:hover {color:var(--fg-strong);background:var(--row-hover)}
@media(max-width:600px){.agent-workspace-body{padding:16px}.agent-workspace-nav{padding:8px}.agent-routine-form{padding:14px}.agent-workspace > .agent-thread-bar{padding:6px 8px}}
`;
const repeatLabel = (minutes: number | null): string => minutes === null ? "Once" : minutes % 60 === 0 ? `Every ${minutes / 60} hours` : `Every ${minutes} minutes`;
const time = (value: number): string => new Date(value).toLocaleString();
function localInput(value: number): string {
  const date = new Date(value);
  return new Date(value - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}
const tabLabel = (tab: AgentWorkspaceView, routines: number, runs: number): string =>
  tab === "thread" ? "Conversation" : tab === "memory" ? "Memory"
    : tab === "routines" ? `Routines${routines ? ` · ${routines}` : ""}`
    : tab === "runs" ? `Runs${runs ? ` · ${runs}` : ""}` : "Computer";

/** A tab's read: still on its way, refused (the reason is beside it), or the answer. */
export type AgentRead<T> = T | "loading" | "unavailable";

export interface AgentMemoryViewProps {
  agent: AgentProfile; files: AgentRead<AgentFiles>; draft: string; messages: AgentMessages; pending: boolean;
  onDraft: (text: string) => void; onSave: () => void; onReveal: () => void; onRecreate: () => void;
  onUseInterfaceInstructions: () => void;
}

export function AgentMemoryView({ agent, files, draft, messages, pending, onDraft, onSave, onReveal,
    onRecreate, onUseInterfaceInstructions }: AgentMemoryViewProps) {
  const memoryId = `agent-memory-${agent.id}`;
  return <>
    <h2>Memory</h2>
    <p>{agent.name} keeps its own notes in MEMORY.md, in its folder. What it does is edited in the agent form.</p>
    {files === "loading" ? <p role="status">Loading MEMORY.md…</p>
      : files === "unavailable" ? <><p role="status">MEMORY.md could not be read.</p><ControlMessage message={messages.files} /></> : <>
      <div className="agent-workspace-row">
        <div className="agent-workspace-copy"><strong>Folder</strong>
          <small>{files.home ?? `Created when ${agent.name} gets its first message.`}</small></div>
        <div className="agent-workspace-actions">
          {!files.exists && files.home
            ? <><button type="button" className="agent-dialog-button" disabled={pending} onClick={onRecreate}>Recreate folder</button><ControlMessage message={messages.recreate} /></>
            : <><button type="button" className="agent-dialog-button" disabled={pending} onClick={onReveal}>Show in Finder</button><ControlMessage message={messages.reveal} /></>}
        </div>
      </div>
      <ControlMessage message={messages.files} />
      {!files.exists && files.home && <p role="status">This folder is missing. {agent.name} will not start until it is recreated.</p>}
      {files.instructionsConflict && files.instructions && <div className="agent-workspace-row">
        <div className="agent-workspace-copy"><strong>AGENTS.md and CLAUDE.md were edited outside Interface</strong>
          <div className="agent-memory-compare">
            <div><small>In the folder</small><pre>{files.instructions.text}</pre></div>
            <div><small>{"Interface's version"}</small><pre>{agent.instructions}</pre></div>
          </div>
          <div className="agent-workspace-actions">
            <button type="button" className="agent-dialog-button" disabled={pending}
              onClick={onUseInterfaceInstructions}>{"Use Interface's version"}</button>
            <ControlMessage message={messages.instructions} />
          </div>
        </div>
      </div>}
      {files.memory !== null && (files.memory.tooLarge
        ? <p role="status">MEMORY.md is larger than 256 KiB, so it is read-only here. Open it with Show in Finder.</p>
        : <form className="agent-memory-form" onSubmit={event => { event.preventDefault(); onSave(); }}>
          <label htmlFor={memoryId}>MEMORY.md</label>
          <textarea id={memoryId} className="agent-dialog-textarea" rows={14} value={draft}
            onChange={event => { onDraft(event.currentTarget.value); }} />
          <div className="agent-workspace-actions">
            <button type="submit" className="agent-dialog-button agent-dialog-submit"
              disabled={pending || draft === files.memory.text}>Save memory</button>
            <ControlMessage message={messages.save} />
          </div>
        </form>)}
    </>}
  </>;
}

export interface AgentComputerViewProps {
  agent: AgentProfile; decisions: AgentRead<readonly AgentDecision[]>; pending: boolean; messages: AgentMessages;
  onAddFolder: () => void; onRemoveFolder: (folder: string) => void; onReveal: () => void;
  /** Gated when absent. */
  permission?: AgentPermissionMode;
}

export function AgentComputerView({ agent, decisions, pending, messages, onAddFolder, onRemoveFolder,
    onReveal, permission = "gated" }: AgentComputerViewProps) {
  const folders = agent.folders ?? [];
  const newest = typeof decisions === "string" ? [] : decisions.slice().sort((a, b) => b.atMs - a.atMs);
  return <>
    <h2>Computer</h2>
    <p>{agent.name} works on this Mac, in its own folder and the folders you grant it.</p>
    <div className="agent-workspace-row">
      <div className="agent-workspace-copy"><strong>Home folder</strong>
        <small>{agent.home ?? `Created when ${agent.name} gets its first message.`}</small></div>
      <div className="agent-workspace-actions">
        <button type="button" className="agent-dialog-button" disabled={pending} onClick={onReveal}>Show in Finder</button>
        <ControlMessage message={messages.reveal} />
      </div>
    </div>
    <div className="agent-workspace-row">
      <div className="agent-workspace-copy"><strong>Granted folders</strong>
        <small>{folders.length === 0 ? "None yet. It works only in its home folder." : `${folders.length} of 16`}</small></div>
      <div className="agent-workspace-actions">
        <button type="button" className="agent-dialog-button" disabled={pending || folders.length >= 16}
          onClick={onAddFolder}>Add folder</button>
        <ControlMessage message={messages.folders} />
      </div>
    </div>
    {folders.map(folder => <div className="agent-workspace-row" key={folder}>
      <div className="agent-workspace-copy"><small>{folder}</small></div>
      <div className="agent-workspace-actions">
        <button type="button" className="agent-dialog-button" aria-label={`Remove ${folder}`} disabled={pending}
          onClick={() => { onRemoveFolder(folder); }}>Remove</button>
      </div>
    </div>)}
    <div className="agent-workspace-row">
      {permission === "gated"
        ? <div className="agent-workspace-copy"><strong>Approval gate</strong>
          <small>{AGENT_ASKS_BEFORE}</small><small>{AGENT_PROVIDER_NOTE}</small></div>
        : <div className="agent-workspace-copy"><strong>{`Permissions: ${AGENT_PERMISSION_LABELS[permission]}`}</strong>
          <small>{AGENT_PERMISSION_HELP[permission]}</small></div>}
    </div>
    <h2 style={{ marginTop: 24 }}>Recent decisions</h2>
    {decisions === "loading" ? <p role="status">Loading decisions…</p>
      : decisions === "unavailable" ? <><p role="status">Recent decisions could not be read.</p><ControlMessage message={messages.decisions} /></>
      : newest.length === 0 ? <p>No decisions yet.</p>
      : <ol className="agent-decisions">{newest.map((row, index) => (
        <li className="agent-decision" key={`${row.atMs}:${row.approvalId}:${index}`}>
          <time dateTime={new Date(row.atMs).toISOString()}>{time(row.atMs)}</time>
          <span className="agent-decision-tool">{row.tool}</span>
          <span className="agent-decision-summary" title={row.reason || row.summary}>{row.summary}</span>
          <span className="badge badge-quiet" data-decision={row.decision}>{AGENT_DECISION_LABELS[row.decision]}</span>
        </li>))}</ol>}
  </>;
}

export function AgentWorkspacePanel({ agent, response, pending, error, children, shownSessionKey,
    visible, threadLastSendId, onCommand, onRequest, onOpenTask, onOpenThread, onShowThread, onPickDirectory,
    initialView = "thread", provider }: AgentWorkspacePanelProps) {
  const [view, setView] = useState<AgentWorkspaceView>(initialView);
  const [editing, setEditing] = useState<AgentRoutine | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  // One id per message until the host creates the thread, so a retry after an
  // unknown outcome is the same request and never a second thread.
  const threadRequestId = useRef<string | null>(null);
  const [files, setFiles] = useState<AgentRead<AgentFiles>>("loading");
  const [memoryDraft, setMemoryDraft] = useState("");
  const [decisions, setDecisions] = useState<AgentRead<readonly AgentDecision[]>>("loading");
  // Each control keeps its own last word. A follow-up read touches only its
  // own slot, so it can never clear the refusal of the action before it.
  const [messages, setMessages] = useState<AgentMessages>({});
  const say = (control: AgentControl, message: AgentMessage | null): void => {
    setMessages(current => {
      if (message === null) {
        if (current[control] === undefined) return current;
        const { [control]: _gone, ...rest } = current;
        return rest;
      }
      return { ...current, [control]: message };
    });
  };
  const refusal = (result: AgentRequestResult): AgentMessage | null =>
    result.ok ? null : { text: result.error, alert: true };
  const [, rerender] = useReducer((count: number) => count + 1, 0);
  const pastMenu = useRef<HTMLDetailsElement>(null);
  const routines = response.routines.filter(row => row.agentId === agent.id);
  const runs = response.runs.filter(row => row.agentId === agent.id).slice().reverse();
  const blocked = runs.some(row => ["queued", "starting", "running", "waiting", "unknown"].includes(row.status));
  const thread = response.threads?.[agent.id];
  const current = thread?.current;
  const permission = agentPermissionMode(response, agent.id);
  const currentKey = current?.sessionKey;
  const past = (thread?.past ?? []).slice().sort((a, b) => b.startedAtMs - a.startedAtMs);
  const pane = agentThreadPane(currentKey, shownSessionKey);
  const restartOpen = current !== undefined
    && restartNoticeOpen(restartLatches, current.sessionKey, current.interrupted, threadLastSendId);

  // Opening the thread reads it: once per activity, and only while the owner
  // can see that thread — never from behind Code, a hidden window, or another
  // session on the Thread tab.
  const lastActivityAtMs = thread?.lastActivityAtMs ?? 0;
  const needsRead = shouldMarkRead({ view, pane, visible, status: response.statuses[agent.id],
    interrupted: current?.interrupted === true });
  const markedAt = useRef<number | null>(null);
  useEffect(() => {
    if (!needsRead || markedAt.current === lastActivityAtMs) return;
    markedAt.current = lastActivityAtMs;
    void onRequest({ kind: "markRead", agentId: agent.id });
  }, [needsRead, lastActivityAtMs]);

  const applyFiles = (next: AgentFiles): void => { setFiles(next); setMemoryDraft(next.memory?.text ?? ""); };
  const loadFiles = async (): Promise<void> => {
    const next = await onRequest({ kind: "readAgentFiles", agentId: agent.id });
    say("files", refusal(next));
    if (next.ok && next.response.files !== undefined) applyFiles(next.response.files);
    else setFiles(loaded => loaded === "loading" ? "unavailable" : loaded);
  };
  useEffect(() => {
    if (typeof document === "undefined") return;
    const onPointerDown = (event: MouseEvent): void => {
      if (pastMenu.current?.open && event.target instanceof Node && !pastMenu.current.contains(event.target)) {
        pastMenu.current.open = false;
      }
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape" && pastMenu.current?.open) {
        pastMenu.current.open = false;
      }
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  useEffect(() => {
    if (view === "memory") void loadFiles();
    // Memory drafts are read only on entry so polling cannot replace an edit.
  }, [view]);
  useEffect(() => {
    if (view !== "computer" || !visible) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async (): Promise<void> => {
      const next = await onRequest({ kind: "readAgentDecisions", agentId: agent.id, limit: 50 });
      if (!active) return;
      say("decisions", refusal(next));
      // Keep the list on screen while nothing in it changed; an outcome can
      // still settle on an older row, so compare every row.
      const rows = next.ok ? next.response.decisions ?? [] : "unavailable";
      setDecisions(current => equalValue(current, rows) ? current : rows);
      timer = setTimeout(() => { void refresh(); }, 3000);
    };
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, [view, visible, agent.id]);

  const reveal = (): void => {
    void onRequest({ kind: "revealAgentHome", agentId: agent.id }).then(next => {
      say("reveal", refusal(next));
      if (next.ok && view === "memory" && (typeof files === "string" || files.home === null)) void loadFiles();
    });
  };
  const sendFirst = async (): Promise<void> => {
    if (sending || pending || draft.trim() === "") return;
    threadRequestId.current ??= newAgentId();
    setSending(true);
    const result = await sendFirstThreadMessage(agent.id, draft, onRequest, threadRequestId.current);
    setSending(false);
    if (result === null) return;
    say("send", refusal(result));
    if (result.ok) { setDraft(""); threadRequestId.current = null; }
  };
  const folders = agent.folders ?? [];
  const addFolder = (): void => {
    say("folders", null);
    void onPickDirectory().then(folder => {
      if (folder && !folders.includes(folder)) {
        void onCommand({ kind: "saveAgentFolders", agentId: agent.id, folders: [...folders, folder] });
      }
    }).catch(() => { say("folders", { text: "Folder picker could not open.", alert: true }); });
  };
  const addRoutine = (): void => {
    setFormError(null);
    const startAtMs = Date.now() + 3_600_000;
    setEditing({ id: newAgentId(), agentId: agent.id, name: "", prompt: "", cwd: "",
      enabled: false, startAtMs, intervalMinutes: 1440, nextRunAtMs: null });
  };
  const composerId = `agent-thread-first-${agent.id}`;
  return <section className="agent-workspace" aria-label={`${agent.name} workspace`}>
    <style>{CSS}</style>
    <nav className="agent-workspace-nav" aria-label="Agent views">
      {TABS.map(tab => <button type="button" key={tab}
        aria-pressed={view === tab} onClick={() => {
          setView(tab);
          // The one place a tab moves the pane: the owner asked for the thread.
          if (tab === "thread") onShowThread();
        }}>
        {tabLabel(tab, routines.length, runs.length)}
      </button>)}
    </nav>
    {view === "thread" && (pane === "current" || pane === "other") ? <>
      {pane === "current" && current !== undefined && <div className="agent-thread-bar">
        {permission !== "gated" && <span className="badge badge-quiet agent-permission-badge"
          data-permission={permission} title={AGENT_PERMISSION_HELP[permission]}>
          {AGENT_PERMISSION_LABELS[permission]}</span>}
        {restartOpen && <>
          <p className="agent-thread-notice" role="status">{AGENT_INTERRUPTED}</p>
          <button type="button" className="agent-thread-dismiss" aria-label="Dismiss restart notice" title="Dismiss"
            onClick={() => { dismissRestartNotice(restartLatches, current.sessionKey); rerender(); }}>
            <span className="codicon codicon-close" aria-hidden="true" />
          </button>
        </>}
        <span className="agent-thread-actions">
          {past.length > 0 && <details className="agent-thread-past" ref={pastMenu}>
            <summary className="agent-dialog-button">Past conversations</summary>
            <div className="agent-thread-past-menu is-floating" role="menu" aria-label="Past conversations">
              {past.map(row => <button type="button" role="menuitem" key={row.sessionKey} onClick={() => {
                if (pastMenu.current !== null) pastMenu.current.open = false;
                onOpenThread(row.sessionKey);
              }}>{`Started ${time(row.startedAtMs)}`}</button>)}
            </div>
          </details>}
          <button type="button" className="agent-dialog-button" disabled={pending}
            onClick={() => {
              // The old thread becomes a past one; show the first-message form, not it.
              void onCommand({ kind: "newThread", agentId: agent.id }).then(done => { if (done) onShowThread(); });
            }}>New conversation</button>
        </span>
      </div>}
      {children}
    </> : <div className="agent-workspace-body">
      {error !== null && editing === null && <p role="alert">{error}</p>}
      {view === "thread" && pane === "open-current" && <>
        <h2>{`${agent.name}'s conversation`}</h2>
        <p>{agent.name} keeps one ongoing conversation.</p>
        <button type="button" className="agent-dialog-button agent-dialog-submit" onClick={onShowThread}>Open conversation</button>
      </>}
      {view === "thread" && pane === "first-message" && <form className="agent-thread-first" onSubmit={event => { event.preventDefault(); void sendFirst(); }}>
        <h2>{`Start a conversation with ${agent.name}`}</h2>
        <p>{agent.name} keeps one ongoing conversation and works from its home folder. {AGENT_ASKS_BEFORE}</p>
        <AgentReadiness agent={agent} provider={provider} />
        <label htmlFor={composerId}>{`Message ${agent.name}`}</label>
        <textarea id={composerId} className="agent-dialog-textarea" rows={5} maxLength={8000} value={draft}
          onChange={event => { setDraft(event.currentTarget.value); }}
          onKeyDown={event => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); void sendFirst(); }
          }} />
        <div className="agent-workspace-actions">
          <button type="submit" className="agent-dialog-button agent-dialog-submit"
            disabled={pending || sending || draft.trim() === ""}>Send</button>
          <ControlMessage message={messages.send} />
        </div>
      </form>}
      {view === "memory" && <AgentMemoryView agent={agent} files={files} draft={memoryDraft} messages={messages}
        pending={pending} onDraft={setMemoryDraft} onReveal={reveal}
        onSave={() => {
          const loaded = typeof files === "string" ? null : files.memory;
          if (loaded === null || loaded.tooLarge) return;
          void saveMemoryDraft(agent.id, loaded, memoryDraft, onRequest).then(result => {
            if (result.files !== null) applyFiles(result.files);
            say("save", result.notice);
          });
        }}
        onRecreate={() => {
          void onRequest({ kind: "recreateAgentHome", agentId: agent.id }).then(result => {
            say("recreate", refusal(result));
            return loadFiles();
          });
        }}
        onUseInterfaceInstructions={() => {
          const onDisk = typeof files === "string" ? null : files.instructions;
          if (onDisk === null) return;
          void onRequest({ kind: "overwriteAgentInstructionsFile", agentId: agent.id, expectedSha256: onDisk.sha256 })
            .then(result => { say("instructions", refusal(result)); return loadFiles(); });
        }} />}
      {view === "memory" && <AgentWorkPanel agentId={agent.id} mode="memory" visible={visible} onRequest={onRequest} onMemoryChanged={loadFiles} />}
      {view === "computer" && <AgentComputerView agent={agent} decisions={decisions} pending={pending}
        permission={permission}
        messages={messages} onAddFolder={addFolder} onReveal={reveal}
        onRemoveFolder={folder => {
          void onCommand({ kind: "saveAgentFolders", agentId: agent.id, folders: folders.filter(row => row !== folder) });
        }} />}
      {view === "computer" && <AgentReadiness agent={agent} provider={provider} />}
      {view === "computer" && <AgentWorkPanel agentId={agent.id} mode="settings" visible={visible} onRequest={onRequest} />}
      {view === "runs" && <AgentWorkPanel agentId={agent.id} mode="tasks" visible={visible} onRequest={onRequest} />}
      {view === "routines" && <>
        <h2>Work on a schedule</h2><p>Give {agent.name} a repeatable task. Save it paused while setting up the provider, then enable it when ready.</p>
        <button type="button" className="agent-dialog-button agent-dialog-submit" disabled={pending} onClick={addRoutine}>New routine</button>
        {routines.length === 0 && !editing && <div className="agent-workspace-empty"><p>No routines yet. Start with a report, a repository check, or a recurring research task.</p></div>}
        {routines.map(row => <div className="agent-workspace-row" key={row.id}>
          <div className="agent-workspace-copy"><strong>{row.name}</strong><small>{row.enabled ? row.nextRunAtMs === null ? "Schedule complete" : `Next ${time(row.nextRunAtMs)}` : "Paused"}</small><AgentCalendarPanel elapsedLabel={repeatLabel(row.intervalMinutes)} agentId={agent.id} routineId={row.id} revision={response.revision} onRequest={onRequest} /></div>
          <div className="agent-workspace-actions">
            <button type="button" className="agent-dialog-button" disabled={pending || blocked} onClick={() => {
              void onCommand({ kind: "runNow", routineId: row.id, requestId: newAgentId() }).then(ok => { if (ok) setView("runs"); });
            }}>Run now</button>
            <label className="agent-switch">
              <input type="checkbox" role="switch" checked={row.enabled} disabled={pending}
                aria-label={row.enabled ? `Pause ${row.name}` : `Enable ${row.name}`}
                onChange={() => { void onCommand({ kind: "saveRoutine", routine: { ...row, enabled: !row.enabled } }); }} />
              {row.enabled ? "On" : "Off"}
            </label>
            <button type="button" className="agent-workspace-icon" disabled={pending} aria-label={`Edit ${row.name}`} title="Edit"
              onClick={() => { setEditing(row); setFormError(null); }}>
              <span className="codicon codicon-edit" aria-hidden="true" />
            </button>
          </div>
        </div>)}
        {editing && <form className="agent-routine-form" onSubmit={event => {
          event.preventDefault(); setFormError(null);
          if (!Number.isFinite(editing.startAtMs)) { setFormError("Choose a valid start time."); return; }
          void onCommand({ kind: "saveRoutine", routine: editing }).then(ok => { if (ok) setEditing(null); });
        }}>
          <h2>{routines.some(row => row.id === editing.id) ? "Edit routine" : "New routine"}</h2>
          <label>Name<input className="agent-dialog-input" required maxLength={80} value={editing.name} onChange={event => { setEditing({ ...editing, name: event.target.value }); }} /></label>
          <label>Task<textarea className="agent-dialog-textarea" required maxLength={8000} rows={4} value={editing.prompt} onChange={event => { setEditing({ ...editing, prompt: event.target.value }); }} /></label>
          <label>Working folder<input className="agent-dialog-input" placeholder="Agent home (prepared when saved)" value={editing.cwd} onChange={event => { setEditing({ ...editing, cwd: event.target.value }); }} /></label>
          <div className="agent-workspace-actions">
            <button type="button" className="agent-dialog-button" onClick={() => { setEditing({ ...editing, cwd: "" }); }}>Use agent home</button>
            <button type="button" className="agent-dialog-button" onClick={() => { void onPickDirectory().then(folder => { if (folder) setEditing(current => current ? { ...current, cwd: folder } : null); }).catch(() => { setFormError("Folder picker could not open."); }); }}>Choose folder…</button>
          </div>
          <div className="agent-routine-fields">
            <label>Start · local time<input className="agent-dialog-input" type="datetime-local" required value={Number.isFinite(editing.startAtMs) ? localInput(editing.startAtMs) : ""} onChange={event => { setEditing({ ...editing, startAtMs: new Date(event.target.value).getTime() }); }} /></label>
            <label>Repeat<select className="agent-dialog-input" value={editing.intervalMinutes ?? "once"} onChange={event => { setEditing({ ...editing, intervalMinutes: event.target.value === "once" ? null : Number(event.target.value) }); }}>
              <option value="once">Once</option><option value="60">Every hour</option><option value="360">Every 6 hours</option><option value="1440">Every 24 hours</option><option value="10080">Every 7 days</option>
            </select></label>
          </div>
          <label className="agent-routine-check"><input type="checkbox" checked={editing.enabled} onChange={event => { setEditing({ ...editing, enabled: event.target.checked }); }} />Enable schedule</label>
          <p>Repeats use elapsed time, including across daylight saving changes. One run per agent at a time; approvals may require you to open the task.</p>
          {(formError || error) && <p role="alert">{formError || error}</p>}
          <div className="agent-workspace-actions"><button className="agent-dialog-button agent-dialog-submit" disabled={pending} type="submit">{pending ? "Saving…" : "Save routine"}</button>
            <button className="agent-dialog-button" type="button" onClick={() => { setEditing(null); }}>Cancel</button>
            {routines.some(row => row.id === editing.id) && <button className="agent-dialog-button" disabled={pending} type="button" onClick={() => { void onCommand({ kind: "removeRoutine", id: editing.id }).then(ok => { if (ok) setEditing(null); }); }}>Delete routine</button>}
          </div>
        </form>}
      </>}
      {view === "runs" && <><h2>Run history</h2><p>Routine runs and one-off tasks, with a link to the provider’s task. Completed turns still need result review.</p>
        {runs.length === 0 && <div className="agent-workspace-empty"><p>No runs yet. Save a routine, then choose Run now to test it.</p></div>}
        {runs.map(row => <div className="agent-workspace-row" key={row.id}>
          <div className="agent-workspace-copy"><strong>{row.name}</strong><small>{row.status === "succeeded" ? "Turn completed" : row.status} · {time(row.scheduledAtMs)}</small>{row.detail && <small>{row.detail}</small>}</div>
          <div className="agent-workspace-actions">{row.sessionKey && <button className="agent-dialog-button" type="button" onClick={() => { if (row.sessionKey) onOpenTask(row.sessionKey); setView("thread"); }}>Open task</button>}
            {row.status === "unknown" && <button className="agent-dialog-button" disabled={pending} type="button" onClick={() => { void onCommand({ kind: "acknowledgeRun", id: row.id }); }}>I reviewed this run</button>}
          </div>
        </div>)}
      </>}
    </div>}
  </section>;
}

function AgentReadiness({ agent, provider }: { agent: AgentProfile; provider?: EngineProbe }) {
  const supported = agent.engine === "claude" || agent.engine === "codex";
  return <aside className="agent-readiness" aria-label="Provider readiness">
    <strong>{agent.engine} · {provider?.info?.version ?? "Version not reported"}</strong>
    <p>{!supported ? "This provider cannot enforce the Agent approval gate. Choose Claude or Codex before starting." : provider === undefined ? "Provider readiness has not been checked yet." : provider.ok ? "Provider executable is available. Authentication and task-tool attachment are checked at launch." : provider.error ?? "Provider is unavailable."}</p>
    <p>Requested model: {agent.model || "Provider default"}. Effective model is reported by the running conversation. {agent.folders?.length ?? 0} additional folders. {AGENT_PROVIDER_NOTE}</p>
  </aside>;
}
