import { useEffect, useId, useState } from "react";
import type { ThreadSecretRequest } from "../../shared/thread-secrets";
import { SECRET_CARD_NOTE } from "../thread-secrets";

/* ============================================================================
   THE PRIVATE SECRET CARD (protocol 67, T3 section 12)

   An agent asked for a secret through `request_secret`. The card sits in the
   pane's dock, above the composer, where an approval waits: who asks, for
   what and why, a password field, Give and Decline. The value lives in this
   field only until Give sends it (Rust writes it to one owner-only file and
   answers names only); the field is cleared as it goes, and nothing of it is
   kept in a draft or shown again.

   PRESENTATIONAL. Props in, callbacks out: no `window.iface`, no store.
   ========================================================================= */

export const THREAD_SECRET_CSS = `
.thread-secret-card { display:flex; flex-direction:column; gap:8px; margin:0; padding:12px 14px;
  color:var(--fg); background:var(--surface); border-radius:var(--radius-container);
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); font-family:var(--font-ui);
  font-size:var(--text-body); line-height:var(--lh-ui); }
.thread-secret-ask { margin:0; overflow-wrap:anywhere; }
.thread-secret-ask code { font-family:var(--font-mono); font-size:var(--text-ui); }
.thread-secret-row { display:flex; flex-wrap:wrap; align-items:center; gap:8px; }
.thread-secret-input { flex:1 1 200px; min-width:0; box-sizing:border-box; height:30px; padding:0 10px;
  color:var(--input-fg); background:var(--surface); border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  font:inherit; font-family:var(--font-mono); }
.thread-secret-input:focus { outline:none; box-shadow:inset 0 0 0 var(--border-w) var(--focus), 0 0 0 var(--border-w) var(--focus); }
.thread-secret-btn { flex:none; height:30px; padding:0 12px; color:var(--fg); background:transparent; border:0;
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong); border-radius:var(--radius-container);
  font:inherit; font-weight:var(--wght-medium); cursor:pointer; }
.thread-secret-btn.is-primary { color:var(--surface); background:var(--fg-strong); box-shadow:none; }
.thread-secret-btn:disabled { color:var(--fg-faint); background:transparent;
  box-shadow:inset 0 0 0 var(--border-w) var(--border); cursor:default; }
.thread-secret-btn:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.thread-secret-note { margin:0; color:var(--fg-dim); font-size:var(--text-ui); }
.thread-secret-error { margin:0; color:var(--danger-fg); font-size:var(--text-ui); }
.thread-secret-sr { position:absolute; width:1px; height:1px; margin:-1px; padding:0; overflow:hidden;
  clip-path:inset(50%); white-space:nowrap; border:0; }
`;

export function ThreadSecretCard({ request, onGive, onDecline }: {
  request: ThreadSecretRequest;
  /** Sends the value once; answers why it was not taken, or null. */
  onGive: (id: string, value: string) => Promise<string | null>;
  onDecline: (id: string) => void;
}) {
  const fieldId = useId();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A new request starts empty.
  useEffect(() => { setValue(""); setError(null); setBusy(false); }, [request.id]);
  // In the dock's own frame: the composer's measure, above it.
  return (
    <div className="thread-dock">
    <form className="thread-secret-card" role="alert" aria-label={`${request.agent} asks for a secret`}
      data-thread-secret={request.id} data-voice-guard="secret" onSubmit={event => {
        event.preventDefault();
        if (busy || value === "") return;
        const given = value;
        setValue("");
        setBusy(true);
        setError(null);
        void onGive(request.id, given).then(refusal => {
          setBusy(false);
          setError(refusal);
        });
      }}>
      <style>{THREAD_SECRET_CSS}</style>
      <p className="thread-secret-ask">
        {request.agent} asks for <code>{request.name}</code>: {request.reason}
      </p>
      <div className="thread-secret-row">
        <label className="thread-secret-sr" htmlFor={fieldId}>Value for {request.name}</label>
        <input id={fieldId} className="thread-secret-input" type="password" value={value} disabled={busy}
          autoComplete="off" spellCheck={false} placeholder={request.name}
          onChange={event => { setValue(event.target.value); }} />
        <button type="submit" className="thread-secret-btn is-primary" disabled={busy || value === ""}>
          {busy ? "Giving…" : "Give"}
        </button>
        <button type="button" className="thread-secret-btn" disabled={busy}
          onClick={() => { setValue(""); onDecline(request.id); }}>Decline</button>
      </div>
      <p className="thread-secret-note">{SECRET_CARD_NOTE}</p>
      {error !== null && <p className="thread-secret-error" role="alert">{error}</p>}
    </form>
    </div>
  );
}
