import { useId, useRef } from "react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { createPortal } from "react-dom";
import { CONTEXT_VALUE_DIALOG_CSS } from "./ContextValueDialog";
import { useModalFocus } from "./ModalBehavior";
import "../codicons.css";

/**
 * The two pickers composer references open (T3 section 9), on the URL
 * dialog's surface: a list of threads, each one button. Plain text only:
 * titles and terminal output are drawn as text nodes, never as markup.
 */

export const COMPOSER_REFERENCE_DIALOG_CSS = `
.composer-ref-list { display:flex; flex-direction:column; gap:2px; max-height:320px; overflow-y:auto;
  margin:0; padding:0; list-style:none; }
.composer-ref-choice { display:flex; flex-direction:column; align-items:flex-start; gap:1px; width:100%;
  box-sizing:border-box; padding:7px 10px; color:var(--fg); text-align:left; background:transparent;
  border:0; border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); cursor:pointer; }
.composer-ref-choice:hover { background:var(--row-hover); }
.composer-ref-choice:focus-visible { outline:var(--focus-ring-w) solid var(--focus);
  outline-offset:calc(-1 * var(--focus-ring-offset)); }
.composer-ref-choice-meta { color:var(--fg-dim); font-size:var(--text-meta); }
.composer-ref-empty { margin:0; color:var(--fg-muted); font-size:var(--text-meta); }
.composer-ref-excerpt { max-height:160px; overflow:auto; margin:0; padding:8px 10px;
  color:var(--fg-muted); background:var(--input-bg); border:var(--border-w) solid var(--input-border);
  border-radius:var(--radius-control); font-family:var(--font-mono); font-size:var(--text-meta);
  white-space:pre-wrap; overflow-wrap:anywhere; }
`;

export interface ComposerReferenceTarget {
  key: string;
  title: string;
  /** One quiet line under the title ("Codex · 3h"). */
  meta?: string;
}

function ReferenceDialog({ open, icon, title, description, children, targets, empty, onChoose,
  onCancel }: {
  open: boolean;
  icon: string;
  title: string;
  description: string;
  children?: ReactNode;
  targets: readonly ComposerReferenceTarget[];
  empty: string;
  onChoose: (key: string) => void;
  onCancel: () => void;
}) {
  const baseId = useId();
  const titleId = `${baseId}-title`;
  const descriptionId = `${baseId}-description`;
  const dialogRef = useRef<HTMLElement>(null);
  const handleDialogKey = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => dialogRef.current?.querySelector<HTMLElement>(".composer-ref-choice")
      ?? dialogRef.current,
    onEscape: onCancel,
  });
  if (!open) return null;
  const handleBackdrop = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (event.target === event.currentTarget) onCancel();
  };
  const surface = <>
    <style>{CONTEXT_VALUE_DIALOG_CSS}</style>
    <style>{COMPOSER_REFERENCE_DIALOG_CSS}</style>
    <div className="context-value-backdrop" onClick={handleBackdrop}>
      <section ref={dialogRef} className="context-value-dialog is-floating" role="dialog"
        aria-modal="true" tabIndex={-1} aria-labelledby={titleId} aria-describedby={descriptionId}
        onKeyDown={handleDialogKey}>
        <header className="context-value-header">
          <span className="context-value-header-icon" aria-hidden="true">
            <span className={`codicon codicon-${icon}`} />
          </span>
          <div className="context-value-heading">
            <h2 id={titleId} className="context-value-title">{title}</h2>
            <p id={descriptionId} className="context-value-description">{description}</p>
          </div>
        </header>
        <div className="context-value-body">
          {children}
          {targets.length === 0 ? <p className="composer-ref-empty">{empty}</p> : (
            <ul className="composer-ref-list" aria-label={title}>
              {targets.map(target => (
                <li key={target.key}>
                  <button type="button" className="composer-ref-choice"
                    onClick={() => { onChoose(target.key); }}>
                    <span>{target.title}</span>
                    {target.meta !== undefined
                      && <span className="composer-ref-choice-meta">{target.meta}</span>}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <footer className="context-value-actions">
          <button type="button" className="context-value-button" onClick={onCancel}>Cancel</button>
        </footer>
      </section>
    </div>
  </>;
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}

/** The context picker's Thread kind: the project's other threads. */
export function ThreadReferenceDialog({ open, choices, onChoose, onCancel }: {
  open: boolean;
  choices: readonly ComposerReferenceTarget[];
  onChoose: (key: string) => void;
  onCancel: () => void;
}) {
  return <ReferenceDialog open={open} icon="comment-discussion" title="Refer to a thread"
    description="Its title, key and latest exchange go with your next message."
    targets={choices} empty="This project has no other threads yet."
    onChoose={onChoose} onCancel={onCancel} />;
}

/** "Send selection to thread…" from a terminal pane: the thread whose draft gets it. */
export function TerminalSelectionDialog({ open, paneTitle, selection, targets, onChoose, onCancel }: {
  open: boolean;
  paneTitle: string;
  selection: string;
  targets: readonly ComposerReferenceTarget[];
  onChoose: (key: string) => void;
  onCancel: () => void;
}) {
  // The end is what matters in terminal output; the draft keeps up to 8,000
  // characters of it, and this preview shows its last lines.
  const preview = selection.replace(/\r\n?/gu, "\n").trimEnd().split("\n").slice(-12).join("\n");
  return <ReferenceDialog open={open} icon="terminal" title="Send selection to thread"
    description={`From "${paneTitle}". It goes into the thread's draft; nothing is sent.`}
    targets={targets} empty="No threads are open." onChoose={onChoose} onCancel={onCancel}>
    <pre className="composer-ref-excerpt" aria-label="Selected terminal text">{preview}</pre>
  </ReferenceDialog>;
}
