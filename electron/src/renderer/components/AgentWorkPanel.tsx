import { useEffect, useState } from "react";
import type { AgentWorkCommand, AgentWorkSpec, AgentWorkView } from "../../shared/agent-work";
import { keepAgentWork, startAgentWorkReads } from "../agent-reads";
import type { AgentRequestHandler } from "./AgentWorkspacePanel";

export function AgentWorkPanel({ agentId, mode, visible, onRequest, onMemoryChanged }: {
  agentId: string; mode: "tasks" | "memory" | "settings";
  /** The agent page is on screen; the view is read only then. */
  visible: boolean;
  onRequest: AgentRequestHandler; onMemoryChanged?: () => Promise<void>;
}) {
  const [work, setWork] = useState<AgentWorkView | null>(null);
  const [draft, setDraft] = useState<AgentWorkSpec | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [fact, setFact] = useState("");
  const [source, setSource] = useState("");
  const [expiry, setExpiry] = useState("");
  const [supersedes, setSupersedes] = useState("");
  const run = async (command: AgentWorkCommand): Promise<boolean> => {
    setBusy(true);
    const result = await onRequest({ kind: "work", agentId, command });
    setBusy(false);
    if (!result.ok) { setError(result.error); return false; }
    setError(null);
    const next = result.response.work;
    if (next !== undefined) setWork(current => keepAgentWork(current, next));
    if (command.action === "configure") setDraft(null);
    if (command.action === "restoreMemory") await onMemoryChanged?.();
    return true;
  };
  useEffect(() => startAgentWorkReads({
    visible, agentId, onRequest,
    onWork: next => { setWork(current => keepAgentWork(current, next)); },
    onError: setError,
  }), [agentId, visible]);
  if (work === null) return <p role="status">{error ?? "Loading task controls…"}</p>;
  return <div className="agent-work-controls">
    {error !== null && <p role="alert">{error}</p>}
    {mode === "settings" && <>
      <h2>Results and limits</h2>
      <p>Each new task snapshots these checks and limits. A completed turn stays unverified until its declared checks pass. Cost is unavailable; token limits use reported parent and child totals. Limits stop observed work; provider-native operations can finish before interruption.</p>
      {draft === null ? <>
        <p>{work.spec.budget.wallSeconds / 60} minute deadline · {work.spec.budget.maxActions} actions · {work.spec.artifacts.length} artifact checks</p>
        <button type="button" className="agent-dialog-button" onClick={() => { setDraft(structuredClone(work.spec)); }}>Edit task contract</button>
      </> : <form onSubmit={event => { event.preventDefault(); void run({ action: "configure", expectedRevision: work.revision, spec: draft }); }}>
        <label>Expected output<textarea className="agent-dialog-textarea" rows={3} value={draft.expectedOutput} onChange={e => { setDraft({ ...draft, expectedOutput: e.target.value }); }} /></label>
        <label>Deadline in minutes<input className="agent-dialog-input" type="number" min={1} max={1440} value={draft.budget.wallSeconds / 60} onChange={e => { const wallSeconds = Number(e.target.value) * 60; setDraft({ ...draft, budget: { ...draft.budget, wallSeconds, noProgressSeconds: Math.min(draft.budget.noProgressSeconds, wallSeconds) } }); }} /></label>
        <label>Maximum actions<input className="agent-dialog-input" type="number" min={1} max={2000} value={draft.budget.maxActions} onChange={e => { setDraft({ ...draft, budget: { ...draft.budget, maxActions: Number(e.target.value) } }); }} /></label>
        <label>No-progress timeout in minutes<input className="agent-dialog-input" type="number" min={1} max={1440} value={draft.budget.noProgressSeconds / 60} onChange={e => { setDraft({ ...draft, budget: { ...draft.budget, noProgressSeconds: Number(e.target.value) * 60 } }); }} /></label>
        <label>Reported token limit (optional)<input className="agent-dialog-input" type="number" min={1} value={draft.budget.maxReportedTokens ?? ""} onChange={e => { setDraft({ ...draft, budget: { ...draft.budget, maxReportedTokens: e.target.value === "" ? null : Number(e.target.value) } }); }} /></label>
        <label><input type="checkbox" checked={draft.allowOpaqueShellCommands} onChange={e => { setDraft({ ...draft, allowOpaqueShellCommands: e.target.checked }); }} />Allow scripts and commands whose effects Interface cannot determine</label>
        <p>Off by default: unknown programs and interpreter commands need your approval. Enabling this restores the broader risky-list policy; scripts may access files or services outside the declared folders.</p>
        <label><input type="checkbox" checked={draft.allowWebSearch === true} onChange={e => { if (e.target.checked) setDraft({ ...draft, allowWebSearch: true }); else { const { allowWebSearch: _search, ...rest } = draft; setDraft(rest); } }} />Search the web without asking</label>
        <p>Searches can carry what the agent has read to the search provider.</p>
        <h3>Selected-file consultations</h3>
        <p>Optional. This explicitly permits sending the chosen files to the chosen provider. Children have no tools. Parent review and unchanged source hashes are required before applying a proposal to an existing writable file.</p>
        <label><input type="checkbox" checked={draft.consultation !== undefined} onChange={e => { if (e.target.checked) setDraft({ ...draft, consultation: { model: "", files: [], writableFiles: [], maxChildren: 1 } }); else { const { consultation: _grant, ...rest } = draft; setDraft(rest); } }} />Enable consultation for new tasks</label>
        {draft.consultation !== undefined && <>
          <label>Exact provider/model<input className="agent-dialog-input" required placeholder="codex/model-id or claude/model-id" value={draft.consultation.model} onChange={e => { if (draft.consultation) setDraft({ ...draft, consultation: { ...draft.consultation, model: e.target.value } }); }} /></label>
          <label>Files to share (one path per line)<textarea className="agent-dialog-textarea" rows={3} required value={draft.consultation.files.join("\n")} onChange={e => { if (draft.consultation) setDraft({ ...draft, consultation: { ...draft.consultation, files: e.target.value.split("\n") } }); }} /></label>
          <label>Writable subset (optional, one path per line)<textarea className="agent-dialog-textarea" rows={3} value={draft.consultation.writableFiles.join("\n")} onChange={e => { if (draft.consultation) setDraft({ ...draft, consultation: { ...draft.consultation, writableFiles: e.target.value === "" ? [] : e.target.value.split("\n") } }); }} /></label>
          <label>Maximum consultations per task<input className="agent-dialog-input" type="number" min={1} max={4} value={draft.consultation.maxChildren} onChange={e => { if (draft.consultation) setDraft({ ...draft, consultation: { ...draft.consultation, maxChildren: Number(e.target.value) } }); }} /></label>
        </>}
        <h3>Artifact acceptance checks</h3>
        {draft.artifacts.map((check, index) => <div className="agent-workspace-row" key={index}>
          <label>Path<input className="agent-dialog-input" required value={check.path} onChange={e => { setDraft({ ...draft, artifacts: draft.artifacts.map((c, i) => i === index ? { ...c, path: e.target.value } : c) }); }} /></label>
          <label>Format<select className="agent-dialog-input" value={check.format} onChange={e => { setDraft({ ...draft, artifacts: draft.artifacts.map((c, i) => i === index ? { ...c, format: e.target.value as typeof check.format } : c) }); }}><option value="nonempty">Nonempty file</option><option value="json">Valid JSON</option><option value="text">UTF-8 text</option></select></label>
          <label>Must contain (optional)<input className="agent-dialog-input" value={check.contains ?? ""} onChange={e => { setDraft({ ...draft, artifacts: draft.artifacts.map((c, i) => i === index ? { ...c, contains: e.target.value || null } : c) }); }} /></label>
          <button type="button" className="agent-dialog-button" onClick={() => { setDraft({ ...draft, artifacts: draft.artifacts.filter((_, i) => i !== index) }); }}>Remove check</button>
        </div>)}
        <button type="button" className="agent-dialog-button" disabled={draft.artifacts.length >= 32} onClick={() => { setDraft({ ...draft, artifacts: [...draft.artifacts, { path: "", format: "nonempty", contains: null }] }); }}>Add artifact check</button>
        <h3>Required inputs</h3>
        {draft.inputs.map((input, index) => <div className="agent-workspace-row" key={index}>
          <label>Input path<input className="agent-dialog-input" required value={input.path} onChange={e => { setDraft({ ...draft, inputs: draft.inputs.map((v, i) => i === index ? { ...v, path: e.target.value } : v) }); }} /></label>
          <label>Maximum age in hours<input className="agent-dialog-input" type="number" min={1} value={input.maxAgeSeconds / 3600} onChange={e => { setDraft({ ...draft, inputs: draft.inputs.map((v, i) => i === index ? { ...v, maxAgeSeconds: Number(e.target.value) * 3600 } : v) }); }} /></label>
          <button type="button" className="agent-dialog-button" onClick={() => { setDraft({ ...draft, inputs: draft.inputs.filter((_, i) => i !== index) }); }}>Remove input</button>
        </div>)}
        <button type="button" className="agent-dialog-button" disabled={draft.inputs.length >= 32} onClick={() => { setDraft({ ...draft, inputs: [...draft.inputs, { path: "", maxAgeSeconds: 86400 }] }); }}>Add input</button>
        <div className="agent-workspace-actions">
          <button type="submit" className="agent-dialog-button agent-dialog-submit" disabled={busy}>Save contract</button>
          <button type="button" className="agent-dialog-button" onClick={() => { setDraft(null); }}>Cancel</button>
        </div>
      </form>}
    </>}
    {mode === "tasks" && <>
      <h2>Task results</h2><p>Verification checks exact artifact bytes. It does not establish research accuracy or overall task quality.</p>
      {(work.expiredApprovals ?? []).length > 0 && <details><summary>{work.expiredApprovals?.length} approvals expired when their provider session ended</summary>{work.expiredApprovals?.map(p => <p key={`${p.sessionKey}:${p.approvalId}`}>{p.tool}: {p.summary} · Expired; no answer will be replayed.</p>)}</details>}
      {(work.consultations ?? []).map(child => <details key={child.id} className="agent-workspace-row"><summary>{child.model} · {child.status} · {child.review ?? "Needs parent review"} · {child.elapsedMs === null ? "Time unknown" : `${(child.elapsedMs / 1000).toFixed(1)}s`}</summary><div className="agent-workspace-copy"><strong>{child.brief}</strong><pre>{child.report}</pre><small>{child.reportedTokens ?? "Unknown"} reported tokens · Result SHA-256 {child.resultSha256 ?? "Unresolved"}</small>{child.inputs.map(input => <small key={input.path}>{input.path} · {input.sha256}</small>)}{child.proposals.map(proposal => <details key={proposal.path}><summary>{proposal.path} · {proposal.appliedSha256 === null ? "Proposed" : "Applied"}</summary><pre>{proposal.text}</pre></details>)}</div></details>)}
      {work.tasks.length === 0 && <p>No bounded tasks yet.</p>}
      {work.tasks.slice().reverse().map(task => <div className="agent-workspace-row" key={task.id}>
        <div className="agent-workspace-copy"><strong>{task.goal}</strong>
          <small>Execution: {task.execution} · Verification: {task.verification} · {task.actions} actions · {task.reportedTokens ?? "Unknown"} reported tokens</small>
          <small>{task.detail}</small>{task.notes.map((note, index) => <small key={index}>{note}</small>)}<small>Next: {task.nextAction}</small>
          <small>Deadline: {new Date(task.deadlineAtMs).toLocaleString()}</small>
          {task.artifacts.map(artifact => <small key={artifact.path}>{artifact.path} · SHA-256 {artifact.sha256}</small>)}
        </div>
        <div className="agent-workspace-actions">
          {task.execution === "completed" && <button className="agent-dialog-button" type="button" disabled={busy} onClick={() => { void run({ action: "verify", taskId: task.id }); }}>Verify artifacts</button>}
          {(task.execution === "starting" || task.execution === "running") && <button className="agent-dialog-button" type="button" disabled={busy || task.stopRequested} onClick={() => { void run({ action: "stop", taskId: task.id }); }}>{task.stopRequested ? "Stopping…" : "Stop task"}</button>}
          {task.execution === "unknown" && <button className="agent-dialog-button" type="button" disabled={busy} onClick={() => { void run({ action: "acknowledge", taskId: task.id }); }}>Acknowledge after inspection</button>}
        </div>
      </div>)}
    </>}
    {mode === "memory" && <>
      <h2>Memory history</h2><p>File observations are not owner authorization. Restoring a revision requires idle work and a matching current file.</p>
      {work.memoryHistory.slice().reverse().map(revision => <details key={`${revision.sha256}:${revision.observedAtMs}`}>
        <summary>{new Date(revision.observedAtMs).toLocaleString()} · {revision.source}</summary><pre>{revision.text}</pre>
        <button type="button" className="agent-dialog-button" disabled={busy} onClick={() => { void onRequest({ kind: "readAgentFiles", agentId }).then(result => {
          const memory = result.ok ? result.response.files?.memory : null;
          if (memory) void run({ action: "restoreMemory", sha256: revision.sha256, expectedSha256: memory.sha256 });
          else setError("Current memory could not be read.");
        }); }}>Restore this revision</button>
      </details>)}
      <h2>Facts and preferences</h2>
      {work.facts.map(item => <div className="agent-workspace-row" key={item.id}><div className="agent-workspace-copy"><strong>{item.text}</strong><small>{item.state} · {item.source} · {new Date(item.observedAtMs).toLocaleString()}{item.expiresAtMs === null ? "" : ` · Expires ${new Date(item.expiresAtMs).toLocaleString()}`}</small></div>
        {item.state === "proposed" && <button className="agent-dialog-button" type="button" onClick={() => { void run({ action: "confirmFact", id: item.id }); }}>Confirm fact</button>}</div>)}
      <form onSubmit={e => { e.preventDefault(); void run({ action: "proposeFact", fact: { id: crypto.randomUUID(), text: fact, source, observedAtMs: 0, expiresAtMs: expiry === "" ? null : new Date(expiry).getTime(), supersedes: supersedes || null, state: "proposed" } }).then(ok => { if (ok) { setFact(""); setSource(""); } }); }}>
        <label>Fact or preference<input className="agent-dialog-input" required value={fact} onChange={e => { setFact(e.target.value); }} /></label>
        <label>Source<input className="agent-dialog-input" required value={source} onChange={e => { setSource(e.target.value); }} /></label>
        <label>Expires (optional)<input className="agent-dialog-input" type="datetime-local" value={expiry} onChange={e => { setExpiry(e.target.value); }} /></label>
        <label>Supersedes (optional)<select className="agent-dialog-input" value={supersedes} onChange={e => { setSupersedes(e.target.value); }}><option value="">None</option>{work.facts.filter(f => f.state !== "superseded").map(f => <option key={f.id} value={f.id}>{f.text}</option>)}</select></label>
        <button type="submit" className="agent-dialog-button" disabled={busy}>Add for review</button>
      </form>
    </>}
  </div>;
}
