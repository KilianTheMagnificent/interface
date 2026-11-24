import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { parseProviderCommandLike } from "../../shared/provider-commands";
import {
  DEFAULT_STEERING_QUEUE_LIMIT,
  normalizeSteeringQueue,
  steeringQueueItemName,
  steeringQueueKeyAction,
  steeringQueueLimit,
  type SteeringQueueItem,
  type SteeringQueueMove,
} from "../steering-queue";
import { logicalListFocusIndex } from "./ModalBehavior";
import "../codicons.css";

export const STEERING_QUEUE_CSS = `
.steering-queue { display:flex; flex-direction:column; min-width:0;
  color:var(--fg); background:var(--surface); border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-container); font-family:var(--font-ui); }
.steering-queue-head { display:flex; align-items:center; gap:6px; min-height:30px; padding:3px 6px 3px 8px;
  border-bottom:var(--border-w) solid var(--border); }
.steering-queue-title { margin:0; color:var(--fg-muted); font-size:var(--text-meta); font-weight:var(--wght-strong); }
.steering-queue-count { color:var(--fg-dim); font-size:var(--text-meta); font-variant-numeric:tabular-nums; }
.steering-queue-spacer { flex:1 1 auto; }
.steering-queue-clear,
.steering-queue-iconbtn { display:inline-flex; flex:none; align-items:center; justify-content:center;
  height:var(--control-h); padding:0 6px; color:var(--fg-muted); background:transparent;
  border:0; border-radius:var(--radius-control); font:inherit; font-size:var(--text-meta); cursor:pointer; }
.steering-queue-iconbtn { width:var(--control-h); padding:0; }
.steering-queue-clear:not(:disabled):hover,
.steering-queue-iconbtn:not(:disabled):hover { color:var(--fg); background:var(--row-hover); }
.steering-queue-clear:disabled,
.steering-queue-iconbtn:disabled { color:var(--fg-faint); background:transparent; cursor:default; }
.steering-queue-list { display:flex; flex-direction:column; gap:0; margin:0; padding:3px; list-style:none; }
.steering-queue-item { display:flex; align-items:flex-start; gap:6px; min-width:0; padding:5px;
  border-radius:var(--radius-control); }
.steering-queue-item:hover,
.steering-queue-item:focus-visible { background:var(--row-hover); }
.steering-queue-index { display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:18px; height:18px; color:var(--fg-dim); font-size:var(--text-meta); font-variant-numeric:tabular-nums; }
.steering-queue-summary { display:flex; flex:1 1 auto; flex-direction:column; gap:2px; min-width:0; }
.steering-queue-copy { min-width:0; margin:1px 0 0; overflow:hidden;
  color:var(--fg); font-size:var(--text-ui); line-height:var(--lh-ui); white-space:pre-wrap; overflow-wrap:anywhere;
  display:-webkit-box; -webkit-box-orient:vertical; -webkit-line-clamp:3; }
.steering-queue-context { display:inline-flex; align-items:center; gap:3px; width:max-content;
  color:var(--fg-dim); font-size:var(--text-meta); line-height:var(--lh-tight); white-space:nowrap; }
/* The two tokens this rule used to name first — --warning-fg and --warning-bg —
   are declared nowhere in the app, so the fallbacks were always what rendered
   and the var() chain only hid that. Written plainly now. Deliberately NOT the
   --status-waiting amber it looked like it wanted: "queued", "paused" and "in
   flight" are pending, not an agent blocked on you, which is the one sentence
   that hue is reserved for (status-waiting-allowlist.test.ts). */
.steering-queue-state { display:inline-flex; align-items:center; width:max-content; padding:0 4px;
  color:var(--fg-muted); background:var(--row-hover);
  border-radius:var(--radius-control); font-size:var(--text-meta); line-height:var(--lh-ui); white-space:nowrap; }
.steering-queue-actions { display:flex; flex:none; align-items:center; gap:1px; }
.steering-queue-send { color:var(--accent); }
.steering-queue-remove:hover:not(:disabled) { color:var(--danger-fg); }
.steering-queue-edit { display:flex; flex:1 1 auto; flex-direction:column; gap:5px; min-width:0; }
.steering-queue-textarea { box-sizing:border-box; width:100%; min-height:64px; resize:vertical;
  padding:6px 8px; color:var(--input-fg); background:var(--input-bg);
  border:var(--border-w) solid var(--focus); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-ui); line-height:var(--lh-ui); }
.steering-queue-edit-actions,
.steering-queue-clear-confirm { display:flex; align-items:center; justify-content:flex-end; gap:5px; }
.steering-queue-button { height:var(--control-h); padding:0 8px; color:var(--fg);
  background:transparent; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-meta); cursor:pointer; }
.steering-queue-button:hover:not(:disabled) { background:var(--row-hover); }
.steering-queue-button:disabled { color:var(--fg-faint); cursor:default; }
.steering-queue-button-primary { color:var(--accent-fg); background:var(--accent); border-color:var(--accent); }
.steering-queue-button-primary:hover:not(:disabled) { background:var(--accent-hover); border-color:var(--accent-hover); }
/* Clear all removes every queued prompt and used to be byte-identical to the
   Cancel next to it. The app's destructive recipe, restated here rather than
   reached for across sheets: this component's own .steering-queue-button rules
   would otherwise outrank a shared class on hover. Same declarations as
   .btn-danger (Conversation) and .confirm-dialog-danger, hover included: it
   thickens the ring instead of stepping the fill toward --danger-fg, which
   would walk the background toward the label's own colour. */
.steering-queue-button-danger { color:var(--danger-fg); background:var(--danger-bg);
  border-color:var(--danger-fg); font-weight:var(--wght-strong);
  box-shadow:inset 0 0 0 var(--border-w) var(--danger-fg); }
.steering-queue-button-danger:hover:not(:disabled) {
  background:var(--danger-bg); box-shadow:inset 0 0 0 2px var(--danger-fg); }
.steering-queue-clear-confirm { justify-content:flex-start; padding:6px 8px;
  color:var(--fg-muted); border-top:var(--border-w) solid var(--border); font-size:var(--text-meta); }
.steering-queue-clear-confirm span:first-child { flex:1 1 auto; }
.steering-queue-empty { margin:0; padding:10px; color:var(--fg-dim); font-size:var(--text-ui); }
.steering-queue-sr { position:absolute; width:1px; height:1px; margin:-1px; padding:0;
  overflow:hidden; clip-path:inset(50%); white-space:nowrap; border:0; }
/* Text actions and the next-up caret stay in the markup so CLI chrome can
   drop the icons without a second component. Refined hides both. */
.steering-queue-cli-act,
.steering-queue-caret { display:none; }
/* THE COMPACT CARD (a Threads pane, owner 2026-10-07): what waits behind the
   turn, said quietly — a dashed edge on a recessed surface, a "QUEUED n"
   micro-label, one line per prompt with Send now · Edit · Remove on the row
   and the reorder arrows on hover or focus. Every action, key and name is the
   full card's. The element in each selector outweighs the status sink's own
   reset of the card. */
section.steering-queue.is-compact { background:var(--input-bg);
  border:var(--border-w) dashed var(--border-strong); border-radius:var(--radius-container); }
.steering-queue.is-compact .steering-queue-head { min-height:24px; padding:4px 6px 0 12px; border-bottom:0; }
.steering-queue.is-compact .steering-queue-title { color:var(--fg-dim); font-size:var(--text-meta);
  text-transform:uppercase; letter-spacing:var(--track-caps); }
.steering-queue.is-compact .steering-queue-count { color:var(--fg-dim); }
.steering-queue.is-compact .steering-queue-list { padding:2px 4px 4px; }
.steering-queue.is-compact .steering-queue-item { align-items:center; min-height:28px; padding:2px 4px 2px 8px; }
.steering-queue.is-compact .steering-queue-index { display:none; }
.steering-queue.is-compact .steering-queue-summary { flex-direction:row; align-items:center; gap:6px; }
.steering-queue.is-compact .steering-queue-copy { display:block; flex:0 1 auto; margin:0;
  white-space:nowrap; text-overflow:ellipsis; overflow:hidden; }
.steering-queue.is-compact .steering-queue-move { display:none; }
.steering-queue.is-compact .steering-queue-item:hover .steering-queue-move,
.steering-queue.is-compact .steering-queue-item:focus-within .steering-queue-move { display:inline-flex; }
.steering-queue-word { display:inline-flex; flex:none; align-items:center; height:24px; padding:0 6px;
  color:var(--fg-muted); background:transparent; border:0; border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-meta); white-space:nowrap; cursor:pointer; }
.steering-queue-word:not(:disabled):hover { color:var(--fg); background:var(--row-hover); }
.steering-queue-word:disabled { color:var(--fg-faint); cursor:default; }
.steering-queue-word.steering-queue-send:not(:disabled) { color:var(--fg); }
.steering-queue-dot { flex:none; color:var(--fg-faint); font-size:var(--text-meta); }
`;

/** CLI-only marks. Refined paints the codicon beside them; CLI hides the icon. */
const CLI_QUEUE_ACT = {
  up: "↑",
  down: "↓",
  edit: "~",
  now: "▶",
  rm: "×",
} as const;

export interface SteeringQueueProps {
  items: readonly SteeringQueueItem[];
  onMove: (id: string, move: SteeringQueueMove) => boolean | void;
  onEdit: (id: string, text: string) => boolean | void;
  onRemove: (id: string) => boolean | void;
  /** Caller removes the row only after its provider/backend accepts Send now. */
  onSendNow: (id: string) => void;
  onClear: () => boolean | void;
  maxItems?: number;
  disabled?: boolean;
  sendingId?: string | null;
  label?: string;
  /**
   * The compact card a Threads pane draws (owner, 2026-10-07): one line per
   * prompt, Send now · Edit · Remove in words on the row, the reorder arrows
   * on hover or focus, Clear only past one prompt. The same actions, keys and
   * accessible names as the full card.
   */
  compact?: boolean;
  /**
   * Owning session key when mounted inside a reusable pane, so switching
   * sessions resets local editor state and releases the prior session's lock.
   */
  sessionKey?: string;
  /**
   * A row's editor opened (`true`) or closed (`false`), however it closed:
   * Save, Cancel, the row leaving, or this queue unmounting. The caller holds
   * automatic delivery at a row while its editor is open.
   */
  onEditingChange?: (id: string, editing: boolean) => void;
}

type QueueFocusAction = "edit" | "remove";
type PendingQueueFocus = {
  id: string;
  index: number;
  action: QueueFocusAction;
  waitFor: "edit-close" | "removal";
};

const queueActionRefKey = (id: string, action: QueueFocusAction): string => `${id}:${action}`;

export function SteeringQueue({ items, onMove, onEdit, onRemove, onSendNow, onClear,
    maxItems = DEFAULT_STEERING_QUEUE_LIMIT, disabled = false, sendingId = null,
    label = "Steering queue", compact = false, sessionKey, onEditingChange }: SteeringQueueProps) {
  const uid = useId();
  const editRef = useRef<HTMLTextAreaElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const clearRef = useRef<HTMLButtonElement>(null);
  const clearCancelRef = useRef<HTMLButtonElement>(null);
  const rowRefs = useRef<Map<string, HTMLLIElement>>(new Map());
  const actionRefs = useRef<Map<string, HTMLButtonElement>>(new Map());
  const pendingFocus = useRef<PendingQueueFocus | null>(null);
  const editingIndex = useRef(-1);
  const [seenSessionKey, setSeenSessionKey] = useState(sessionKey);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const [announcement, setAnnouncement] = useState("");
  if (seenSessionKey !== sessionKey) {
    setSeenSessionKey(sessionKey);
    if (editingId !== null) {
      setEditingId(null);
      setDraft("");
    }
    if (confirmClear) setConfirmClear(false);
  }
  const limit = steeringQueueLimit(maxItems);
  // Hits on every parent re-render: Conversation memoizes the array it passes.
  const queue = useMemo(() => normalizeSteeringQueue(items, limit), [items, limit]);
  const queueIds = queue.map(item => item.id).join("\u0000");
  const editingChange = useRef(onEditingChange);
  editingChange.current = onEditingChange;

  // Tell the caller which row is open, keyed on the row alone so a new
  // callback identity never reopens or closes it, while cleanup closes on the
  // callback that opened it even if the pane has switched sessions.
  useEffect(() => {
    if (editingId === null) return;
    const notify = editingChange.current;
    notify?.(editingId, true);
    return () => { notify?.(editingId, false); };
  }, [editingId]);

  useEffect(() => {
    if (editingId === null) return;
    if (!queue.some(item => item.id === editingId)) {
      pendingFocus.current = {
        id: editingId,
        index: Math.max(editingIndex.current, 0),
        action: "edit",
        waitFor: "removal",
      };
      setEditingId(null);
      setDraft("");
      return;
    }
    editRef.current?.focus();
    editRef.current?.select();
  }, [editingId, queueIds]);

  useEffect(() => {
    if (queue.length === 0) setConfirmClear(false);
  }, [queue.length]);

  useEffect(() => {
    const request = pendingFocus.current;
    if (request === null) return;
    const exactIndex = queue.findIndex(item => item.id === request.id);
    if (request.waitFor === "edit-close" && editingId !== null) return;
    if (request.waitFor === "removal" && exactIndex >= 0) return;

    const targetIndex = exactIndex >= 0
      ? exactIndex : logicalListFocusIndex(request.index, queue.length);
    const targetId = targetIndex >= 0 ? queue[targetIndex]?.id : undefined;
    const preferred = targetId === undefined ? undefined
      : actionRefs.current.get(queueActionRefKey(targetId, request.action));
    const row = targetId === undefined ? undefined : rowRefs.current.get(targetId);
    if (preferred?.isConnected && !preferred.disabled) preferred.focus();
    else if (row?.isConnected) row.focus();
    else titleRef.current?.focus();
    pendingFocus.current = null;
  }, [editingId, queueIds]);

  useEffect(() => {
    if (confirmClear) clearCancelRef.current?.focus();
  }, [confirmClear]);

  const rememberAction = (id: string, action: QueueFocusAction,
      node: HTMLButtonElement | null): void => {
    const key = queueActionRefKey(id, action);
    if (node === null) actionRefs.current.delete(key); else actionRefs.current.set(key, node);
  };

  const beginEdit = (item: SteeringQueueItem, index: number): void => {
    setConfirmClear(false);
    editingIndex.current = index;
    setEditingId(item.id);
    setDraft(item.text);
  };

  const finishEdit = (item: SteeringQueueItem, index: number): void => {
    pendingFocus.current = { id: item.id, index, action: "edit", waitFor: "edit-close" };
    setEditingId(null);
    setDraft("");
  };

  const cancelEdit = (item: SteeringQueueItem, index: number): void => {
    setAnnouncement(`Canceled editing queued prompt ${index + 1}.`);
    finishEdit(item, index);
  };

  const saveEdit = (item: SteeringQueueItem, index: number): void => {
    const text = draft.trim();
    if (text === "" || parseProviderCommandLike(text) !== null) return;
    if (onEdit(item.id, text) === false) return;
    setAnnouncement(`Updated queued prompt ${steeringQueueItemName(item)}.`);
    finishEdit(item, index);
  };

  const moveItem = (item: SteeringQueueItem, index: number, move: SteeringQueueMove): void => {
    if (onMove(item.id, move) === false) return;
    const destination = move === "up" ? index
      : move === "down" ? index + 2
      : move === "first" ? 1 : queue.length;
    setAnnouncement(`Moved queued prompt to position ${destination} of ${queue.length}.`);
  };

  const handleRowKey = (event: KeyboardEvent<HTMLLIElement>, item: SteeringQueueItem,
      index: number): void => {
    if (disabled || sendingId === item.id) return;
    if (event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement) return;
    const action = steeringQueueKeyAction(index, queue.length, event.key, event.altKey);
    if (action === null) return;
    event.preventDefault();
    event.stopPropagation();
    moveItem(item, index, action.move);
  };

  // The row's three actions. The full card draws each as an icon button; the
  // compact card says it in a word. Names, handlers and focus memory are one.
  const actionClass = (extra = ""): string =>
    `${compact ? "steering-queue-word" : "steering-queue-iconbtn"}${extra}`;
  const editButton = (item: SteeringQueueItem, index: number, sending: boolean) => (
    <button type="button" className={actionClass()}
      ref={node => { rememberAction(item.id, "edit", node); }}
      disabled={disabled || sending}
      aria-label={`Edit queued prompt ${index + 1}`}
      title="Edit"
      onClick={() => beginEdit(item, index)}>
      {compact ? "Edit" : <>
        {/* The pencil, not the cog: a cog is settings, and this
            opens the queued prompt for editing. */}
        <span className="codicon codicon-sm codicon-edit" aria-hidden="true" />
        <span className="steering-queue-cli-act" aria-hidden="true">{CLI_QUEUE_ACT.edit}</span>
      </>}
    </button>
  );
  const sendNowButton = (item: SteeringQueueItem, index: number, sending: boolean) => (
    <button type="button" className={actionClass(" steering-queue-send")}
      disabled={disabled || sending}
      aria-label={sending
        ? `Sending queued prompt ${index + 1}`
        : `Send queued prompt ${index + 1} now`}
      title="Send now"
      onClick={() => {
        onSendNow(item.id);
        setAnnouncement(`Sending queued prompt ${index + 1} now.`);
      }}>
      {compact ? (sending ? "Sending…" : "Send now") : <>
        {/* `send`, the same mark the composer's Send wears: this
            row delivers the prompt now. Steer keeps `play`. */}
        <span className={`codicon codicon-sm codicon-${sending ? "loading codicon-modifier-spin" : "send"}`}
          aria-hidden="true" />
        <span className="steering-queue-cli-act" aria-hidden="true">{CLI_QUEUE_ACT.now}</span>
      </>}
    </button>
  );
  const removeButton = (item: SteeringQueueItem, index: number, sending: boolean) => (
    <button type="button"
      className={actionClass(" steering-queue-remove")}
      ref={node => { rememberAction(item.id, "remove", node); }}
      disabled={disabled || sending}
      aria-label={`Remove queued prompt ${index + 1}`}
      title="Remove"
      onClick={() => {
        pendingFocus.current = {
          id: item.id, index, action: "remove", waitFor: "removal",
        };
        if (onRemove(item.id) === false) {
          pendingFocus.current = null;
          return;
        }
        setAnnouncement(`Removed queued prompt ${index + 1}.`);
      }}>
      {compact ? "Remove" : <>
        <span className="codicon codicon-sm codicon-trash" aria-hidden="true" />
        <span className="steering-queue-cli-act" aria-hidden="true">{CLI_QUEUE_ACT.rm}</span>
      </>}
    </button>
  );

  return (
    <section className={compact ? "steering-queue is-compact" : "steering-queue"}
      aria-labelledby={`${uid}-title`}>
      <style>{STEERING_QUEUE_CSS}</style>
      <header className="steering-queue-head">
        <h3 ref={titleRef} id={`${uid}-title`} className="steering-queue-title"
          tabIndex={-1}>
          {compact ? <>
            <span aria-hidden="true">Queued</span>
            <span className="steering-queue-sr">{label}</span>
          </> : label}
        </h3>
        {compact
          ? <span className="steering-queue-count" title={`${queue.length} of ${limit}`}>{queue.length}</span>
          : <span className="steering-queue-count">{queue.length}/{limit}</span>}
        <span className="steering-queue-spacer" />
        {queue.length > (compact ? 1 : 0) && (
          <button ref={clearRef} type="button" className="steering-queue-clear" disabled={disabled}
            aria-expanded={confirmClear}
            aria-controls={confirmClear ? `${uid}-clear` : undefined}
            onClick={() => setConfirmClear(value => !value)}>
            Clear
          </button>
        )}
      </header>

      {queue.length === 0 ? (
        <p className="steering-queue-empty" role="status">No queued prompts.</p>
      ) : (
        <ol className="steering-queue-list" aria-label="Prompts waiting to be sent">
          {queue.map((item, index) => {
            const name = steeringQueueItemName(item);
            const editing = editingId === item.id;
            const sending = sendingId === item.id;
            const contextCount = item.context?.length ?? 0;
            const deliveryCopy = item.deliveryState === "inflight"
              ? "Delivery outcome unknown" : item.deliveryState === "paused" ? "Paused" : null;
            return (
              <li key={item.id} ref={node => {
                  if (node === null) rowRefs.current.delete(item.id);
                  else rowRefs.current.set(item.id, node);
                }}
                className="steering-queue-item" tabIndex={editing ? -1 : 0}
                aria-label={`Queued prompt ${index + 1} of ${queue.length}: ${name}`}
                aria-busy={sending || undefined}
                aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown Alt+Home Alt+End"
                onKeyDown={event => handleRowKey(event, item, index)}>
                <span className="steering-queue-caret" aria-hidden="true">{index === 0 ? ">" : " "}</span>
                <span className="steering-queue-index" aria-hidden="true">{index + 1}</span>
                {editing ? (
                  <div className="steering-queue-edit">
                    <label className="steering-queue-sr" htmlFor={`${uid}-edit-${item.id}`}>
                      Edit queued prompt {index + 1}
                    </label>
                    <textarea ref={editRef} id={`${uid}-edit-${item.id}`}
                      className="steering-queue-textarea" value={draft} disabled={disabled}
                      onChange={event => setDraft(event.target.value)}
                      onKeyDown={event => {
                        if (event.key === "Escape") {
                          event.preventDefault();
                          event.stopPropagation();
                          cancelEdit(item, index);
                        } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                          event.preventDefault();
                          event.stopPropagation();
                          saveEdit(item, index);
                        }
                      }} />
                    <div className="steering-queue-edit-actions">
                      <span className="steering-queue-sr">Command or Control plus Enter saves.</span>
                      <button type="button" className="steering-queue-button"
                        onClick={() => cancelEdit(item, index)}>Cancel</button>
                      <button type="button"
                        className="steering-queue-button steering-queue-button-primary"
                        disabled={disabled || draft.trim() === ""
                          || parseProviderCommandLike(draft) !== null}
                        onClick={() => saveEdit(item, index)}>Save</button>
                    </div>
                  </div>
                ) : (
                  <>
                    <div className="steering-queue-summary">
                      <p className="steering-queue-copy" title={item.text}>{item.text}</p>
                      {contextCount > 0 && (
                        <span className="steering-queue-context"
                          aria-label={`${contextCount} attached context ${contextCount === 1 ? "item" : "items"}`}>
                          <span className="codicon codicon-sm codicon-files" aria-hidden="true" />
                          {contextCount} context
                        </span>
                      )}
                      {deliveryCopy !== null && (
                        <span className="steering-queue-state">{deliveryCopy}</span>
                      )}
                    </div>
                    <div className="steering-queue-actions">
                      <button type="button" className="steering-queue-iconbtn steering-queue-move"
                        disabled={disabled || sending || index === 0}
                        aria-label={`Move queued prompt ${index + 1} earlier`}
                        title="Move earlier"
                        onClick={() => moveItem(item, index, "up")}>
                        <span className="codicon codicon-sm codicon-chevron-up" aria-hidden="true" />
                        <span className="steering-queue-cli-act" aria-hidden="true">{CLI_QUEUE_ACT.up}</span>
                      </button>
                      <button type="button" className="steering-queue-iconbtn steering-queue-move"
                        disabled={disabled || sending || index === queue.length - 1}
                        aria-label={`Move queued prompt ${index + 1} later`}
                        title="Move later"
                        onClick={() => moveItem(item, index, "down")}>
                        <span className="codicon codicon-sm codicon-chevron-down" aria-hidden="true" />
                        <span className="steering-queue-cli-act" aria-hidden="true">{CLI_QUEUE_ACT.down}</span>
                      </button>
                      {compact ? <>
                        {/* The compact row says its three actions in words, in
                            the order they are reached for; each keeps the full
                            card's accessible name. */}
                        {sendNowButton(item, index, sending)}
                        <span className="steering-queue-dot" aria-hidden="true">·</span>
                        {editButton(item, index, sending)}
                        <span className="steering-queue-dot" aria-hidden="true">·</span>
                        {removeButton(item, index, sending)}
                      </> : <>
                        {editButton(item, index, sending)}
                        {sendNowButton(item, index, sending)}
                        {removeButton(item, index, sending)}
                      </>}
                    </div>
                  </>
                )}
              </li>
            );
          })}
        </ol>
      )}

      {confirmClear && queue.length > 0 && (
        <div id={`${uid}-clear`} className="steering-queue-clear-confirm"
          role="group" aria-label="Confirm clear steering queue">
          <span>Remove all {queue.length} queued prompts?</span>
          <button ref={clearCancelRef} type="button" className="steering-queue-button"
            onClick={() => {
              setConfirmClear(false);
              queueMicrotask(() => clearRef.current?.focus());
            }}>Cancel</button>
          <button type="button" className="steering-queue-button steering-queue-button-danger"
            disabled={disabled} onClick={() => {
              if (onClear() === false) return;
              setConfirmClear(false);
              setAnnouncement(`Cleared ${queue.length} queued prompts.`);
              queueMicrotask(() => titleRef.current?.focus());
            }}>Clear all</button>
        </div>
      )}

      <div className="steering-queue-sr" role="status" aria-live="polite"
        aria-atomic="true">{announcement}</div>
    </section>
  );
}
