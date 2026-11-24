import { providerLabel } from "../provider-labels";
import { useMemo, useState } from "react";
import type {
  CollaborationApprovalAction,
  CollaborationEngine,
  CollaborationPeerAction,
  CollaborationStatusDto,
  CollaborationSubmitRequest,
} from "../../shared/ipc";
import type { AppSelectOption } from "./AppSelect";
import { ConfirmDialog } from "./ConfirmDialog";
import { FormSelect } from "./FormSelect";
import { messageFor, pretty } from "./panel-text";
import { productLabel } from "../product";

export interface CollaborationSessionOption {
  key: string;
  title: string;
  engine: string;
  status?: string;
}

export interface CollaborationPanelProps {
  statuses: readonly CollaborationStatusDto[];
  sessions: readonly CollaborationSessionOption[];
  selectedSessionKey?: string | null;
  loading?: boolean;
  error?: string | null;
  onRefresh: () => void;
  onSubmit: (input: CollaborationSubmitRequest) => Promise<void>;
  onApprove: (input: CollaborationApprovalAction) => Promise<void>;
  onDeny: (input: CollaborationApprovalAction) => Promise<void>;
  onAcceptPeer: (input: CollaborationPeerAction) => Promise<void>;
  onDiscardPeer: (input: CollaborationPeerAction) => Promise<void>;
  onCancel: (collaborationId: string) => Promise<void>;
  onOpenAgent: (sessionKey: string) => void;
}

const TARGETS: readonly { id: CollaborationEngine }[] = [
  { id: "claude" }, { id: "codex" }, { id: "acp:grok" },
];

function actionError(error: unknown): string {
  return messageFor(error, `${productLabel()} could not update this collaboration.`);
}

/**
 * The app's own pickers, with the hidden named input that keeps this form's
 * `new FormData(form)` reading exactly what it read from the platform <select>s
 * they replaced.
 */
const MODE_ROWS: AppSelectOption[] = [
  { value: "consult", label: "Consult, then review result" },
  { value: "handoff", label: "Handoff source ownership" },
];
const TARGET_ROWS: AppSelectOption[] = TARGETS.map(target =>
  ({ value: target.id, label: providerLabel(target.id) }));

/** An explicit source pick, remembered with the conversation it was made under. */
export interface CollaborationSourcePick { key: string; under: string }

/**
 * Which source agent the picker shows. Seeding once at mount is not enough: the
 * session list arrives after the first paint, and the selected conversation can
 * change under a mounted panel — both leave the picker with nothing selected and
 * the submit button dead. So the key is derived every render. A pick the user
 * made explicitly wins while its conversation is still the selected one and the
 * agent is still live; otherwise the panel falls back to the selected
 * conversation, then to the first live source.
 */
export function collaborationSourceKey(
  options: readonly string[],
  selectedSessionKey: string | null | undefined,
  pick: CollaborationSourcePick | null,
): string {
  const wanted = selectedSessionKey ?? "";
  if (pick !== null && pick.under === wanted && options.includes(pick.key)) return pick.key;
  return options.includes(wanted) ? wanted : options[0] ?? "";
}

function StatusCard({ status, pending, onApprove, onDeny, onAcceptPeer, onDiscardPeer,
    onCancel, onOpenAgent }: {
  status: CollaborationStatusDto;
  pending: boolean;
  onApprove: CollaborationPanelProps["onApprove"];
  onDeny: CollaborationPanelProps["onDeny"];
  onAcceptPeer: CollaborationPanelProps["onAcceptPeer"];
  onDiscardPeer: CollaborationPanelProps["onDiscardPeer"];
  onCancel: CollaborationPanelProps["onCancel"];
  onOpenAgent: CollaborationPanelProps["onOpenAgent"];
}) {
  const { receipt, approval, peer } = status;
  // Discarding is the one action on this card that destroys work: the peer's
  // result is thrown away and cannot be produced again without another run.
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  return <article className={`coord-card is-${receipt.state}`}>
    <header><div><span className="badge">{pretty(receipt.state)}</span>
      <strong>{pretty(receipt.mode)} · {providerLabel(receipt.sourceEngine)} → {providerLabel(receipt.targetEngine)}</strong></div>
      <small>revision {receipt.revision}</small></header>
    <div className="coord-meta">
      <span>{new Date(receipt.updatedAt).toLocaleString()}</span>
      <code>{receipt.collaborationId}</code>
    </div>
    {receipt.error !== undefined && <div className="coord-error" role="alert">
      <strong>{receipt.error.code}</strong><p>{receipt.error.safeNextAction}</p>
      <span>{receipt.error.retryable ? "A reviewed retry may be safe." : `${productLabel()} will not retry automatically.`}</span>
    </div>}
    {/* Approving a target launch is a user verdict; the voice assistant's
        ui_act refuses everything inside here (see voice-ui.ts). */}
    {approval !== undefined && <section className="coord-approval" aria-label="Collaboration approval"
      data-voice-guard="approval">
      <h4>Review exact target launch</h4>
      <p>Only the packet below is transferred. Provider internal state and omitted context are not copied. The source remains inspectable.</p>
      <dl><div><dt>Source</dt><dd>{approval.sourceTitle ?? approval.sourceSessionKey}</dd></div>
        <div><dt>Project</dt><dd><code>{approval.canonicalProjectRoot}</code></dd></div>
        <div><dt>Destination folder</dt><dd><code>{approval.canonicalCwd}</code></dd></div>
        <div><dt>Destination provider</dt><dd>{providerLabel(approval.targetEngine)}</dd></div>
        <div><dt>Source conversation</dt><dd><code>{approval.sourceSessionKey}</code></dd></div>
        <div><dt>Packet</dt><dd>{approval.packetBytes} bytes · {approval.omissions} omissions</dd></div>
        <div><dt>Expires</dt><dd>{new Date(approval.expiresAt).toLocaleString()}</dd></div></dl>
      <pre>{approval.packetPreview}</pre>
      <div className="coord-actions"><button type="button" className="btn btn-primary"
        disabled={pending} onClick={() => void onApprove({
          collaborationId: approval.collaborationId,
          approvalId: approval.approvalId,
          expectedRevision: approval.expectedRevision,
        })}>Approve target</button>
      <button type="button" className="btn" disabled={pending}
        onClick={() => void onDeny({
          collaborationId: approval.collaborationId,
          approvalId: approval.approvalId,
          expectedRevision: approval.expectedRevision,
        })}>Deny</button></div>
    </section>}
    {peer !== undefined && <section className="coord-peer" aria-label="Untrusted peer result">
      <h4>Untrusted peer result</h4><p>{peer.objective}</p><pre>{peer.body}</pre>
      <div className="coord-actions"><button type="button" className="btn btn-primary"
        disabled={pending || peer.state !== "awaiting-review"}
        onClick={() => void onAcceptPeer({
          collaborationId: peer.collaborationId,
          resultId: peer.resultId,
          resultDigest: peer.resultDigest,
          expectedRevision: peer.expectedRevision,
        })}>Accept into source</button>
      <button type="button" className="btn" disabled={pending}
        onClick={() => setConfirmDiscard(true)}>Discard</button>
      <button type="button" className="btn coord-link"
        onClick={() => onOpenAgent(peer.targetSessionKey)}>Open peer agent</button></div>
      <ConfirmDialog open={confirmDiscard} title="Discard this peer result?"
        description={`${peer.objective}\n\nThe untrusted result is thrown away. `
          + "Producing another one means running the collaboration again."}
        confirmLabel="Discard" onCancel={() => setConfirmDiscard(false)}
        onConfirm={() => {
          setConfirmDiscard(false);
          void onDiscardPeer({
            collaborationId: peer.collaborationId,
            resultId: peer.resultId,
            resultDigest: peer.resultDigest,
            expectedRevision: peer.expectedRevision,
          });
        }} />
    </section>}
    {receipt.cancellationSafe && <div className="coord-actions"><button type="button"
      className="btn" disabled={pending}
      onClick={() => void onCancel(receipt.collaborationId)}>Cancel safely</button></div>}
  </article>;
}

export function CollaborationPanel(props: CollaborationPanelProps) {
  const sourceOptions = useMemo(() => props.sessions.filter(session =>
    TARGETS.some(target => target.id === session.engine)), [props.sessions]);
  const [pick, setPick] = useState<CollaborationSourcePick | null>(null);
  const source = collaborationSourceKey(
    sourceOptions.map(row => row.key), props.selectedSessionKey, pick);
  const [pending, setPending] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [mode, setMode] = useState("consult");
  const [targetEngine, setTargetEngine] = useState("codex");
  const perform = async (operation: () => Promise<void>): Promise<void> => {
    setPending(true); setLocalError(null);
    try { await operation(); }
    catch (error) { setLocalError(actionError(error)); }
    finally { setPending(false); }
  };
  const wrap = <T,>(action: (input: T) => Promise<void>) =>
    (input: T): Promise<void> => perform(() => action(input));

  return <section className="coord-panel" aria-labelledby="coordination-title">
    <style>{COLLABORATION_PANEL_CSS}</style>
    <div className="coord-heading"><div><h3 id="coordination-title">Multi-agent coordination</h3>
      <p>Start a bounded, human-approved cross-provider consult or handoff. Peer output stays
        untrusted until you accept it.</p></div>
      <button type="button" className="btn" disabled={props.loading || pending}
        onClick={props.onRefresh}>{props.loading ? "Refreshing…" : "Refresh"}</button></div>
    {(props.error ?? localError) !== null && <div className="coord-error" role="alert">
      {props.error ?? localError}
    </div>}
    <form className="coord-form" onSubmit={event => {
      event.preventDefault();
      const data = new FormData(event.currentTarget);
      const objective = String(data.get("objective") ?? "").trim();
      if (source === "" || objective === "") return;
      void perform(() => props.onSubmit({
        sourceSessionKey: source as CollaborationSubmitRequest["sourceSessionKey"],
        mode: String(data.get("mode")) as CollaborationSubmitRequest["mode"],
        targetEngine: String(data.get("targetEngine")) as CollaborationEngine,
        objective,
        clientRequestId: globalThis.crypto.randomUUID(),
      }));
    }}>
      <FormSelect fieldClassName="coord-field" label="Source agent" value={source}
        placeholder="Select a live source" disabled={pending}
        onChange={value => setPick({ key: value, under: props.selectedSessionKey ?? "" })}
        options={sourceOptions.map(session => ({ value: session.key,
          label: `${session.title} · ${providerLabel(session.engine)}`
            + `${session.status === undefined ? "" : ` · ${session.status}`}` }))} />
      <FormSelect fieldClassName="coord-field" label="Mode" name="mode" value={mode}
        options={MODE_ROWS} onChange={setMode} disabled={pending} />
      <FormSelect fieldClassName="coord-field" label="Target" name="targetEngine"
        value={targetEngine} options={TARGET_ROWS} onChange={setTargetEngine}
        disabled={pending} />
      <label className="coord-objective"><span>Bounded objective</span>
        <textarea name="objective" rows={3} required maxLength={16_384} disabled={pending}
          placeholder="Ask for one concrete review, analysis, or transfer objective." /></label>
      <button type="submit" className="btn btn-primary" disabled={pending || source === ""}>
        {pending ? "Submitting…" : "Review request"}
      </button>
    </form>
    <div className="coord-list">
      {props.statuses.length === 0 ? <p className="coord-empty">Consult another agent or hand over a task. Requests appear here for review.</p>
        : props.statuses.map(status => <StatusCard key={status.receipt.collaborationId}
          status={status} pending={pending}
          onApprove={wrap(props.onApprove)} onDeny={wrap(props.onDeny)}
          onAcceptPeer={wrap(props.onAcceptPeer)} onDiscardPeer={wrap(props.onDiscardPeer)}
          onCancel={id => perform(() => props.onCancel(id))} onOpenAgent={props.onOpenAgent} />)}
    </div>
  </section>;
}

export const COLLABORATION_PANEL_CSS = `
.coord-panel { margin:18px 0; padding:16px; border:var(--border-w) solid var(--border);
  border-radius:var(--radius-container); background:var(--chrome-bg); }
.coord-heading { display:flex; align-items:start; justify-content:space-between; gap:16px; }
.coord-heading h3 { margin:0; font-size:var(--text-prose); font-weight:var(--wght-strong); }
.coord-heading p { max-width:64ch; margin:6px 0 0; color:var(--fg-muted); font-size:var(--text-ui); line-height:var(--lh-ui); }
.coord-heading > button { flex:none; }
.coord-empty { margin:0; color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.coord-link { border-color:transparent; }
.coord-panel :where(button,textarea):focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.coord-form { display:grid; grid-template-columns:minmax(180px,1fr) minmax(150px,.7fr)
  minmax(120px,.55fr); max-width:920px; gap:10px; align-items:end; margin:12px 0 16px; }
.coord-form label,.coord-field { display:flex; flex-direction:column; gap:5px; min-width:0;
  color:var(--fg-dim); }
.coord-form textarea { box-sizing:border-box; width:100%; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); padding:7px 9px; background:var(--input-bg); color:var(--fg); font:inherit; resize:vertical; min-height:80px; }
/* FormSelect owns the shared field shape. */
.coord-objective { grid-column:1 / -1; }
.coord-form > button { grid-column:1 / -1; justify-self:end; }
.coord-list { display:flex; flex-direction:column; gap:10px; }
.coord-card { padding:12px; border:var(--border-w) solid var(--border); border-radius:var(--radius-control);
  background:var(--editor-bg); }
.coord-card > header { display:flex; justify-content:space-between; gap:10px; align-items:center; }
.coord-card > header > div { display:flex; align-items:center; gap:8px; min-width:0; }
/* No .coord-badge: the coordination state wears theme.css's .badge, which
   carries the --badge-bg/--badge-fg pairing this rule used to re-declare. */
.coord-meta { display:flex; justify-content:space-between; gap:10px; color:var(--fg-dim);
  font-size:var(--text-ui); margin-top:7px; }
.coord-meta code { overflow:hidden; text-overflow:ellipsis; }
.coord-approval,.coord-peer,.coord-error { margin-top:12px; padding:11px;
  border:var(--border-w) solid var(--border); border-radius:var(--radius-control); }
.coord-error { color:var(--danger-fg); border-color:var(--border-strong); background:var(--danger-bg); }
.coord-approval pre,.coord-peer pre { max-height:260px; overflow:auto; padding:9px; white-space:pre-wrap;
  overflow-wrap:anywhere; background:var(--chrome-bg); border-radius:var(--radius-control); }
.coord-approval dl { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:7px 12px; }
.coord-approval dl div { min-width:0; }.coord-approval dt { color:var(--fg-dim); font-size:var(--text-meta); }
.coord-approval dd { margin:2px 0 0; overflow-wrap:anywhere; }
.coord-actions { display:flex; flex-wrap:wrap; gap:7px; margin-top:10px; }
@media (max-width:900px) { .coord-form { grid-template-columns:1fr 1fr; }
  .coord-objective { grid-column:1 / -1; }.coord-form > button { grid-column:1 / -1; }
  .coord-approval dl { grid-template-columns:1fr; } }
`;
