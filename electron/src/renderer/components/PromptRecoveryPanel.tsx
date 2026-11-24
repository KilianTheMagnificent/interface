import { useEffect, useState } from "react";
import type { PromptRecoveryReceipt, RecoveryMode, RecoveryResponse } from "../../shared/recovery";
import { ipcMessage } from "../ipc-message";
import { shortPromptTitle } from "../thread-title";
export function PromptRecoveryPanel({ sessionKey, wake, interrupted, disabled, read, recover, forget, pending = false, restore, preview }: {
  sessionKey: string; wake: number; interrupted: boolean; disabled: boolean;
  read: (key: string) => Promise<RecoveryResponse>;
  recover?: (key: string, mode: RecoveryMode, id: string) => Promise<void>;
  forget?: (key: string, id: string) => Promise<void>;
  pending?: boolean;
  restore?: (receipt: PromptRecoveryReceipt) => Promise<void>;
  preview?: (item: PromptRecoveryReceipt["input"]["context"][number]) => void;
}) {
  const [confirmForget, setConfirmForget] = useState(false);
  const [receipt, setReceipt] = useState<PromptRecoveryReceipt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void read(sessionKey).then(value => { if (active) { setReceipt(value.receipt); setError(null); } })
      .catch(failure => { if (active) setError(ipcMessage(failure)); });
    return () => { active = false; };
  }, [sessionKey, wake, interrupted, read]);
  if (!receipt) return null;
  const actionable = receipt.delivery !== "accepted" || interrupted;
  const mode = receipt.delivery === "unsent" ? "retry" : "continue";
  return <section className="prompt-recovery" aria-label="Recover previous prompt">
    {actionable && <strong>{pending ? "Saved thread launch" : mode === "retry" ? "Previous prompt was not sent" : "Continue from the current state"}</strong>}
    {actionable && <p>{pending ? "Inspect existing threads before recreating work whose launch outcome is unknown. The original prompt and attachments are saved below." : mode === "retry" ? "Retry uses the preserved prompt and attachments. Their access is checked again before sending."
      : "Work may already have happened. Continue asks the agent to inspect what completed and work on the remainder; it does not replay the original request."}</p>}
    <details><summary>Saved prompt · {receipt.input.context.length} attachments</summary>
      <pre>{receipt.input.text}</pre>
      <ul>{receipt.input.context.map(item => {
        const canPreview = preview !== undefined
          && (item.kind === "file" || item.kind === "folder" || (item.kind === "diff" && item.safeContent !== undefined));
        return <li key={item.id} title={item.value}>
          {canPreview
            ? <button type="button" onClick={() => { preview(item); }}>{item.label} · {item.kind}</button>
            : <>{item.label} · {item.kind}</>}
        </li>;
      })}</ul>
      {forget && <div>
        {confirmForget ? <>
          <p>{pending ? "Inspect existing threads first. Forgetting removes this launch's duplicate protection and saved attachments." : "Forget the saved recovery prompt and attachment references? The conversation remains available."}</p>
          <button type="button" disabled={disabled || busy} onClick={() => {
            setBusy(true); setError(null);
            void forget(sessionKey, receipt.id).then(() => { setReceipt(null); setConfirmForget(false); })
              .catch(failure => { setError(ipcMessage(failure)); }).finally(() => { setBusy(false); });
          }}>Confirm forget</button>
          <button type="button" disabled={busy} onClick={() => { setConfirmForget(false); }}>Keep recovery</button>
        </> : <button type="button" disabled={disabled || busy} onClick={() => { setConfirmForget(true); }}>Forget saved recovery</button>}
      </div>}
    </details>
    {error && <p role="alert">{error}</p>}
    {actionable && !pending && recover && <button type="button" disabled={disabled || busy} onClick={() => {
      setBusy(true); setError(null);
      void recover(sessionKey, mode, receipt.id).then(() => read(sessionKey)).then(value => { setReceipt(value.receipt); })
        .catch(failure => { setError(ipcMessage(failure)); }).finally(() => { setBusy(false); });
    }}>{busy ? "Sending…" : mode === "retry" ? "Retry preserved prompt" : "Continue safely"}</button>}
    {pending && receipt.delivery === "unsent" && restore && <button type="button" disabled={disabled || busy} onClick={() => {
      setBusy(true); setError(null);
      void restore(receipt).catch(failure => { setError(ipcMessage(failure)); }).finally(() => { setBusy(false); });
    }}>Restore unsent prompt to composer</button>}
  </section>;
}


/** How many characters of the kept message the line quotes. */
const KEPT_WORDS_CHARS = 40;

/**
 * Whether the kept message has anything to recover, so the line shows: one
 * that was not sent, or whose outcome is unknown, or whose turn failed or was
 * interrupted. A message delivered and answered draws no line; its receipt
 * stays stored as it is (coordinator decision, 2026-11-15).
 */
export function keptPromptShows(delivery: PromptRecoveryReceipt["delivery"], interrupted: boolean): boolean {
  return delivery !== "accepted" || interrupted;
}

/**
 * What the kept-prompt line says before the message's first words: not sent,
 * or sent with an outcome nobody knows (or a turn that did not finish).
 */
export function keptPromptLead(delivery: PromptRecoveryReceipt["delivery"]): string {
  return delivery === "unsent" ? "A message you wrote earlier was not sent"
    : "A message you wrote earlier may not have finished";
}

/**
 * The draft after Put it back: the kept message alone in an empty box, or
 * after what is already typed there, a blank line between, so nothing the
 * owner wrote is lost.
 */
export function keptPromptDraft(current: string, kept: string): string {
  const typed = current.replace(/\s+$/u, "");
  return typed === "" ? kept : `${typed}\n\n${kept}`;
}

/** The kept attachments Put it back can attach again: files and folders, by path. */
export function keptPromptPaths(context: PromptRecoveryReceipt["input"]["context"]):
    { path: string; directory?: boolean }[] {
  return context.filter(item => item.kind === "file" || item.kind === "folder")
    .map(item => (item.kind === "folder" ? { path: item.value, directory: true } : { path: item.value }));
}

/** "1 attachment", "3 attachments". */
export function attachmentCount(count: number): string {
  return `${count} attachment${count === 1 ? "" : "s"}`;
}

/**
 * The kept prompt inside the composer's top edge (owner, 2026-11-15): one
 * line, `A message you wrote earlier was not sent: "<its first words>"` (or
 * "may not have finished"), how many attachments it holds when it holds any,
 * then Retry or Continue safely, Put it back and Discard. Only a message with
 * something to recover shows it (`keptPromptShows`): a delivered, answered
 * message draws nothing. The words open the whole message, its
 * attachments (each previewable) and what Retry or Continue does; Discard
 * asks first, as Forget did. The receipt and its actions are the
 * PromptRecoveryPanel's: Rust keeps it, the line only words it.
 */
export function KeptPromptLine({ sessionKey, wake, interrupted, disabled, read, recover, forget, restore, preview }: {
  sessionKey: string; wake: number; interrupted: boolean; disabled: boolean;
  read: (key: string) => Promise<RecoveryResponse>;
  recover?: (key: string, mode: RecoveryMode, id: string) => Promise<void>;
  forget?: (key: string, id: string) => Promise<void>;
  /** Put it back: the message (and what attachments can be) into this composer. */
  restore?: (receipt: PromptRecoveryReceipt) => Promise<void> | void;
  preview?: (item: PromptRecoveryReceipt["input"]["context"][number]) => void;
}) {
  const [receipt, setReceipt] = useState<PromptRecoveryReceipt | null>(null);
  const [open, setOpen] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  // The receipt put back into the composer: the line steps aside for it
  // until Rust keeps another message.
  const [putBack, setPutBack] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void read(sessionKey).then(value => { if (active) { setReceipt(value.receipt); setError(null); } })
      .catch(failure => { if (active) setError(ipcMessage(failure)); });
    return () => { active = false; };
  }, [sessionKey, wake, interrupted, read]);
  if (!receipt || receipt.id === putBack || !keptPromptShows(receipt.delivery, interrupted)) return null;
  const mode = receipt.delivery === "unsent" ? "retry" : "continue";
  const attachments = receipt.input.context;
  const words = shortPromptTitle(receipt.input.text, KEPT_WORDS_CHARS);
  const explanation = mode === "retry"
    ? "Retry uses the preserved prompt and attachments. Their access is checked again before sending."
    : "Work may already have happened. Continue asks the agent to inspect what completed and work on the remainder; it does not replay the original request.";
  return <section className="kept-prompt" aria-label="Recover previous prompt" data-delivery={receipt.delivery}>
    <div className="kept-prompt-line">
      <button type="button" className="kept-prompt-words" aria-expanded={open}
        title={open ? "Hide the kept message" : receipt.input.text}
        onClick={() => { setOpen(!open); }}>
        <span className="kept-prompt-text">{keptPromptLead(receipt.delivery)}: "{words}"</span>
        {attachments.length > 0 && <span className="kept-prompt-count">· {attachmentCount(attachments.length)}</span>}
      </button>
      <span className="kept-prompt-actions">
        {recover && <button type="button" className="kept-prompt-action" disabled={disabled || busy}
          title={explanation}
          onClick={() => {
            setBusy(true); setError(null);
            void recover(sessionKey, mode, receipt.id).then(() => read(sessionKey)).then(value => { setReceipt(value.receipt); })
              .catch(failure => { setError(ipcMessage(failure)); }).finally(() => { setBusy(false); });
          }}>{busy ? "Sending…" : mode === "retry" ? "Retry" : "Continue safely"}</button>}
        {restore && <button type="button" className="kept-prompt-action" disabled={busy}
          title="Put this message back in the box below, to edit or send again"
          onClick={() => {
            setError(null);
            void Promise.resolve(restore(receipt)).then(() => { setPutBack(receipt.id); setOpen(false); setConfirmDiscard(false); })
              .catch(failure => { setError(ipcMessage(failure)); });
          }}>Put it back</button>}
        {forget && <button type="button" className="kept-prompt-action" disabled={disabled || busy || confirmDiscard}
          onClick={() => { setConfirmDiscard(true); }}>Discard</button>}
      </span>
    </div>
    {confirmDiscard && forget && <div className="kept-prompt-confirm" role="group" aria-label="Discard the kept message">
      <span>Discard this message{attachments.length > 0 ? " and its attachments" : ""}? The conversation stays.</span>
      <span className="kept-prompt-actions">
        <button type="button" className="kept-prompt-action is-danger" disabled={disabled || busy} onClick={() => {
          setBusy(true); setError(null);
          void forget(sessionKey, receipt.id).then(() => { setReceipt(null); setConfirmDiscard(false); })
            .catch(failure => { setError(ipcMessage(failure)); }).finally(() => { setBusy(false); });
        }}>Discard</button>
        <button type="button" className="kept-prompt-action" disabled={busy}
          onClick={() => { setConfirmDiscard(false); }}>Keep it</button>
      </span>
    </div>}
    {open && <div className="kept-prompt-more">
      <p>{explanation}</p>
      <pre>{receipt.input.text}</pre>
      {attachments.length > 0 && <ul>{attachments.map(item => {
        const canPreview = preview !== undefined
          && (item.kind === "file" || item.kind === "folder" || (item.kind === "diff" && item.safeContent !== undefined));
        return <li key={item.id} title={item.value}>
          {canPreview
            ? <button type="button" className="kept-prompt-action" onClick={() => { preview(item); }}>{item.label} · {item.kind}</button>
            : <>{item.label} · {item.kind}</>}
        </li>;
      })}</ul>}
    </div>}
    {error && <p role="alert" className="kept-prompt-error">{error}</p>}
  </section>;
}

export function PendingLaunchRecoveryPanel({ wake, read, forget, restore, openSession, preview }: {
  wake: number;
  read: (key: string, list?: boolean) => Promise<RecoveryResponse>;
  forget: (key: string, id: string) => Promise<void>;
  restore: (receipt: PromptRecoveryReceipt) => Promise<void>;
  openSession?: (key: string) => void;
  preview?: (item: PromptRecoveryReceipt["input"]["context"][number]) => void;
}) {
  const [rows, setRows] = useState<NonNullable<RecoveryResponse["pending"]>>([]);
  const [selected, setSelected] = useState("");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void read("pending-launches", true).then(result => { if (active) { setRows(result.pending ?? []); setError(null); } })
      .catch(failure => { if (active) setError(ipcMessage(failure)); });
    return () => { active = false; };
  }, [read, wake]);
  if (rows.length === 0 && !error) return null;
  return <details className="prompt-recovery"><summary>Saved thread launches ({rows.length})</summary>
    {error && <p role="alert">{error}</p>}
    <label>Inspect launch <select value={selected} onChange={event => { setSelected(event.target.value); }}>
      <option value="">Choose a saved launch</option>
      {rows.map(row => <option key={row.key} value={row.key}>{row.delivery === "unsent" ? "Not sent" : row.delivery === "accepted" ? "Started" : "Outcome unknown"} · {row.preview}</option>)}
    </select></label>
    {rows.filter(row => row.key === selected && row.sessionKey !== undefined).map(row => <p key={row.key}>
      Started thread: {row.sessionKey} {openSession && <button type="button" onClick={() => { openSession(row.sessionKey!); }}>Open started thread</button>}
    </p>)}
    {selected && rows.some(row => row.key === selected) && <PromptRecoveryPanel key={selected} sessionKey={selected} wake={wake}
      interrupted={false} disabled={false} pending read={read} restore={restore} preview={preview}
      forget={async (key, id) => { await forget(key, id); setRows(current => current.filter(row => row.key !== key)); setSelected(""); }} />}
  </details>;
}
