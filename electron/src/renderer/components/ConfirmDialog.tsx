import { useId, useRef } from "react";
import type { MouseEvent as ReactMouseEvent, RefObject } from "react";
import { createPortal } from "react-dom";
import { useModalFocus } from "./ModalBehavior";
import "../codicons.css";

export type ConfirmDialogTone = "danger" | "default";

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: string;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: ConfirmDialogTone;
  /** Keep the modal visible while an accepted operation hands off to the OS. */
  pending?: boolean;
  /** Optional logical destination when the control that opened the dialog disappears. */
  returnFocusRef?: RefObject<HTMLElement | null>;
  onConfirm: () => void;
  onCancel: () => void;
  /**
   * Escape and a press on the backdrop, when they must mean something other
   * than the Cancel button (folder trust: Not now records an answer, closing
   * records none). Absent: they cancel.
   */
  onDismiss?: () => void;
}

export const CONFIRM_DIALOG_CSS = `
.confirm-dialog-backdrop { position:fixed; inset:0; z-index:1200; display:flex;
  align-items:center; justify-content:center; padding:16px;
  background:color-mix(in srgb, var(--chrome-bg) 68%, transparent); }
.confirm-dialog { display:flex; flex-direction:column; box-sizing:border-box;
  width:min(440px, calc(100vw - 32px)); max-height:calc(100vh - 32px); overflow:auto;
  color:var(--fg); background:var(--widget-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui); }
.confirm-dialog-copy { padding:16px; }
.confirm-dialog-title { margin:0; color:var(--fg-strong); font-size:var(--text-prose);
  font-weight:var(--wght-strong); line-height:var(--lh-ui); }
.confirm-dialog-description { margin:8px 0 0; color:var(--fg-muted); font-size:var(--text-ui);
  line-height:var(--lh-ui); white-space:pre-wrap; overflow-wrap:anywhere; }
.confirm-dialog-actions { display:flex; justify-content:flex-end; gap:6px; padding:10px 12px;
  border-top:var(--border-w) solid var(--border); }
.confirm-dialog-pending { display:flex; align-items:center; gap:8px; padding:10px 16px;
  color:var(--fg-muted); border-top:var(--border-w) solid var(--border); font-size:var(--text-ui); }
.confirm-dialog-button { box-sizing:border-box; min-width:72px; min-height:28px; padding:0 10px;
  color:var(--fg); background:transparent; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); cursor:pointer; }
.confirm-dialog-button:not(:disabled):hover { background:var(--row-hover); }
.confirm-dialog-button:disabled { color:var(--fg-faint); cursor:default; }
.confirm-dialog-confirm { color:var(--accent-fg); background:var(--accent);
  border-color:var(--accent); font-weight:var(--wght-strong); }
.confirm-dialog-confirm:not(:disabled):hover { background:var(--accent-hover); border-color:var(--accent-hover); }
.confirm-dialog-confirm:disabled { color:var(--fg-faint); background:var(--surface-raised); border-color:var(--border); }
/* Louder than the Cancel beside it, and louder than the ordinary confirm. The
   hover used to change nothing a reader could see — same background, same
   border, same colour, plus an inset shadow that only doubled the border it
   already had — so the destructive button was the one control on the dialog
   with no hover response at all. A tint, a doubled danger edge and the strong
   weight; NOT a solid --danger-fg fill, which would have to clear contrast in
   thirteen themes. Same recipe .btn-danger and the queue's Clear all take,
   including the hover: it THICKENS THE RING rather than stepping the fill
   toward --danger-fg, because the label is --danger-fg and every percent of
   that mix walks the background toward the text — 18% put nine of thirteen
   theme blocks under 4.5:1. See Conversation.tsx's .btn-danger for the table. */
.confirm-dialog-danger { color:var(--danger-fg); background:var(--danger-bg);
  border-color:var(--danger-fg); font-weight:var(--wght-strong);
  box-shadow:inset 0 0 0 var(--border-w) var(--danger-fg); }
.confirm-dialog-danger:not(:disabled):hover {
  background:var(--danger-bg); box-shadow:inset 0 0 0 2px var(--danger-fg); }
@media (max-width:360px) {
  .confirm-dialog-actions { display:grid; grid-template-columns:1fr; }
  .confirm-dialog-button { width:100%; }
}
`;

export function ConfirmDialog({
  open, title, description, confirmLabel = "Confirm", cancelLabel = "Cancel",
  tone = "danger", pending = false, returnFocusRef, onConfirm, onCancel, onDismiss = onCancel,
}: ConfirmDialogProps) {
  const baseId = useId();
  const titleId = `${baseId}-title`;
  const descriptionId = `${baseId}-description`;
  const dialogRef = useRef<HTMLElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const handleKey = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => pending ? dialogRef.current : cancelRef.current,
    onEscape: pending ? () => undefined : onDismiss,
    escapeEnabled: !pending,
    returnFocusRef,
  });

  if (!open) return null;

  const handleBackdrop = (event: ReactMouseEvent<HTMLDivElement>): void => {
    // Only a press that starts and ends on the backdrop cancels. A drag that
    // begins inside the dialog must not turn destructive merely because the
    // pointer was released outside it.
    if (!pending && event.target === event.currentTarget) onDismiss();
  };

  const surface = <>
    <style>{CONFIRM_DIALOG_CSS}</style>
    <div className="confirm-dialog-backdrop" onClick={handleBackdrop}>
      <section ref={dialogRef} className="confirm-dialog is-floating" role="alertdialog" aria-modal="true"
        data-voice-guard="approval"
        tabIndex={-1} aria-busy={pending || undefined}
        aria-labelledby={titleId} aria-describedby={descriptionId} onKeyDown={handleKey}>
        <div className="confirm-dialog-copy">
          <h2 id={titleId} className="confirm-dialog-title">{title}</h2>
          <p id={descriptionId} className="confirm-dialog-description">{description}</p>
        </div>
        {pending ? (
          <div className="confirm-dialog-pending" role="status" aria-live="polite">
            <span className="codicon codicon-loading codicon-modifier-spin" aria-hidden="true" />
            <span>Keep Interface open while macOS prepares the restart.</span>
          </div>
        ) : (
          <div className="confirm-dialog-actions">
            <button ref={cancelRef} type="button" className="confirm-dialog-button"
              onClick={onCancel}>{cancelLabel}</button>
            <button ref={confirmRef} type="button"
              className={tone === "danger"
                ? "confirm-dialog-button confirm-dialog-danger"
                : "confirm-dialog-button confirm-dialog-confirm"}
              onClick={onConfirm}>{confirmLabel}</button>
          </div>
        )}
      </section>
    </div>
  </>;
  // The desktop app always takes the portal branch. Returning the identical
  // surface during server rendering keeps its ARIA contract inspectable in the
  // project's dependency-free node tests and is harmless to a renderer that is
  // never hydrated from server HTML.
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
