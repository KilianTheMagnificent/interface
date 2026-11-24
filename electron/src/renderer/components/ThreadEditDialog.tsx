import { useId, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { CONFIRM_DIALOG_CSS } from "./ConfirmDialog";
import { useModalFocus } from "./ModalBehavior";
import { ipcMessage } from "../ipc-message";
import "../codicons.css";

/**
 * "Edit from here" on a thread turn: start a new thread from the moment
 * before this message, with the message back in its composer to edit, and
 * settle this one.
 *
 * Two ways to do it. "Keep the current files" leaves the files as they are.
 * "Restore the files to before this message" also puts them back — offered
 * only to a thread that made its own worktree (`revertFilesBlocked` says why
 * not otherwise). Rust still checks it and may refuse; its sentence is then
 * shown here and the option stays off with that sentence as its reason.
 *
 * The app's ConfirmDialog look (its CSS), with a third button.
 */
export interface ThreadEditDialogProps {
  /** Turn ordinals count from 0. */
  ordinal: number;
  /** Why restoring the files is not offered, or null. */
  revertFilesBlocked: string | null;
  /** Rejects with the message to show when Rust refuses. */
  onChoose: (restoreFiles: boolean) => Promise<void>;
  onCancel: () => void;
}

export const KEEP_FILES_LABEL = "Keep the current files";
export const RESTORE_FILES_LABEL = "Restore the files to before this message";

/** What editing from a turn does: a new thread from the earlier message, and this one settled. */
export function threadEditDescription(): string {
  return "A new thread starts from just before this message, with the message in its "
    + "composer to change. This thread is settled; nothing in it is deleted.";
}

/**
 * The first message has nothing before it to fork from: the New thread card
 * opens instead, and this thread is settled before it does.
 */
export function threadEditCardNote(ordinal: number): string | null {
  return ordinal === 0
    ? "The New thread card opens on the same workspace. If you close it without starting, "
      + "this thread stays settled; Un-settle it from its menu in the sidebar."
    : null;
}

export function ThreadEditDialog({ ordinal, revertFilesBlocked, onChoose, onCancel }: ThreadEditDialogProps) {
  const baseId = useId();
  const dialogRef = useRef<HTMLElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const [pending, setPending] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Rust's reason the files were not restored: the option stays off with it.
  const [restoreRefused, setRestoreRefused] = useState<string | null>(null);
  const busy = pending !== null;
  const restoreBlocked = revertFilesBlocked ?? restoreRefused;
  const cardNote = threadEditCardNote(ordinal);
  const handleKey = useModalFocus({
    open: true,
    containerRef: dialogRef,
    initialFocus: () => cancelRef.current,
    onEscape: busy ? () => undefined : onCancel,
    escapeEnabled: !busy,
  });

  const choose = (restoreFiles: boolean): void => {
    if (busy) return;
    setPending(restoreFiles);
    setError(null);
    onChoose(restoreFiles).then(() => { setPending(null); }, (err: unknown) => {
      const message = ipcMessage(err);
      setPending(null);
      setError(message);
      if (restoreFiles) setRestoreRefused(message);
    });
  };
  const handleBackdrop = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (!busy && event.target === event.currentTarget) onCancel();
  };

  const surface = <>
    <style>{CONFIRM_DIALOG_CSS}</style>
    <div className="confirm-dialog-backdrop" onClick={handleBackdrop}>
      <section ref={dialogRef} className="confirm-dialog thread-edit-dialog is-floating"
        role="dialog" aria-modal="true" tabIndex={-1} aria-busy={busy || undefined}
        aria-labelledby={`${baseId}-title`} aria-describedby={`${baseId}-description`}
        onKeyDown={handleKey}>
        <div className="confirm-dialog-copy">
          <h2 id={`${baseId}-title`} className="confirm-dialog-title">Edit from here</h2>
          <p id={`${baseId}-description`} className="confirm-dialog-description">
            {threadEditDescription()}
          </p>
          {cardNote !== null && <p className="confirm-dialog-description">{cardNote}</p>}
          {revertFilesBlocked !== null && (
            <p className="confirm-dialog-description">{revertFilesBlocked}</p>
          )}
          {error !== null && (
            <p className="confirm-dialog-description thread-edit-error" role="alert">{error}</p>
          )}
        </div>
        <div className="confirm-dialog-actions">
          <button ref={cancelRef} type="button" className="confirm-dialog-button"
            disabled={busy} onClick={onCancel}>Cancel</button>
          <button type="button" className="confirm-dialog-button confirm-dialog-confirm"
            disabled={busy} onClick={() => { choose(false); }}>
            {pending === false ? "Starting…" : KEEP_FILES_LABEL}
          </button>
          <button type="button" className="confirm-dialog-button confirm-dialog-danger"
            disabled={busy || restoreBlocked !== null}
            title={restoreBlocked ?? "Put the files back to how they were before this message"}
            onClick={() => { choose(true); }}>
            {pending === true ? "Restoring…" : RESTORE_FILES_LABEL}
          </button>
        </div>
      </section>
    </div>
  </>;
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
