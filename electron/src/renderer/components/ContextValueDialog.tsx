import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { useModalFocus } from "./ModalBehavior";
import "../codicons.css";

export interface ContextValueDialogProps {
  open: boolean;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}

/** Absolute HTTP(S) URLs only. The trimmed original is what submit preserves. */
export function isValidContextUrl(value: string): boolean {
  const candidate = value.trim();
  if (candidate === "") return false;
  try {
    const parsed = new URL(candidate);
    return (parsed.protocol === "http:" || parsed.protocol === "https:")
      && parsed.hostname.length > 0
      && parsed.username.length === 0
      && parsed.password.length === 0;
  } catch {
    return false;
  }
}

export const CONTEXT_VALUE_DIALOG_CSS = `
.context-value-backdrop {
  position:fixed; inset:0; z-index:1120; display:flex; align-items:center;
  justify-content:center; box-sizing:border-box; padding:18px;
  background:color-mix(in srgb, var(--chrome-bg) 70%, transparent);
}
.context-value-dialog {
  display:flex; flex-direction:column; box-sizing:border-box;
  width:min(520px, calc(100vw - 36px)); max-height:min(620px, calc(100vh - 36px));
  overflow:hidden; color:var(--fg); background:var(--widget-bg);
  border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-overlay); box-shadow:var(--overlay-shadow);
  font-family:var(--font-ui);
}
.context-value-header { display:flex; align-items:flex-start; gap:10px; padding:16px 16px 12px; }
.context-value-header-icon {
  display:inline-flex; flex:none; align-items:center; justify-content:center;
  width:28px; height:28px; color:var(--accent); background:var(--surface-raised);
  border-radius:var(--radius-control);
}
.context-value-heading { min-width:0; }
.context-value-title {
  margin:0; color:var(--fg-strong); font-size:var(--text-prose); font-weight:var(--wght-strong); line-height:var(--lh-tight);
}
.context-value-description {
  margin:3px 0 0; color:var(--fg-muted); font-size:var(--text-meta); line-height:var(--lh-ui);
}
.context-value-form { display:flex; flex-direction:column; min-height:0; }
.context-value-body { display:flex; flex-direction:column; gap:7px; min-height:0; padding:2px 16px 14px; }
.context-value-label { color:var(--fg-muted); font-size:var(--text-meta); font-weight:var(--wght-strong); line-height:var(--lh-ui); }
.context-value-input-wrap { position:relative; display:flex; align-items:center; }
.context-value-input-icon {
  position:absolute; left:8px; z-index:1; color:var(--input-placeholder); pointer-events:none;
}
.context-value-input {
  box-sizing:border-box; width:100%; height:var(--control-h); padding:0 9px 0 30px;
  color:var(--input-fg); background:var(--input-bg);
  border:var(--border-w) solid var(--input-border);
  border-radius:var(--radius-control); outline:none; font-family:var(--font-ui); font-size:var(--text-ui);
}
.context-value-input::placeholder { color:var(--input-placeholder); }
.context-value-input:hover { border-color:var(--border-strong); }
.context-value-input:focus { border-color:var(--focus); box-shadow:0 0 0 1px var(--focus); }
.context-value-input[aria-invalid="true"] { border-color:var(--danger-fg); }
.context-value-help {
  min-height:15px; margin:0; color:var(--fg-dim); font-size:var(--text-meta); line-height:var(--lh-ui);
}
.context-value-help.is-invalid { color:var(--danger-fg); }
.context-value-actions {
  display:flex; flex:none; justify-content:flex-end; gap:6px; padding:10px 12px;
  border-top:var(--border-w) solid var(--border);
}
.context-value-button {
  min-width:72px; height:var(--control-h); padding:0 10px; color:var(--fg);
  background:transparent; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font-family:var(--font-ui); font-size:var(--text-ui); cursor:pointer;
}
.context-value-button:hover:not(:disabled) { background:var(--row-hover); }
.context-value-button:focus-visible {
  outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.context-value-button:disabled { color:var(--fg-faint); cursor:default; }
.context-value-submit {
  color:var(--accent-fg); background:var(--accent); border-color:var(--accent); font-weight:var(--wght-strong);
}
.context-value-submit:hover:not(:disabled) {
  background:var(--accent-hover); border-color:var(--accent-hover);
}
.context-value-submit:disabled { color:var(--fg-faint); background:var(--surface-raised); }
`;

export function ContextValueDialog({ open, onSubmit, onCancel }: ContextValueDialogProps) {
  const baseId = useId();
  const titleId = `${baseId}-title`;
  const descriptionId = `${baseId}-description`;
  const inputId = `${baseId}-url-input`;
  const helpId = `${baseId}-url-help`;
  const dialogRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [urlValue, setUrlValue] = useState("");

  const urlReady = isValidContextUrl(urlValue);
  const urlInvalid = urlValue.trim() !== "" && !urlReady;
  const handleDialogKey = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => inputRef.current,
    onEscape: onCancel,
  });

  useEffect(() => {
    setUrlValue("");
    if (open) inputRef.current?.focus();
  }, [open]);

  if (!open) return null;

  const handleSubmit = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    const value = urlValue.trim();
    if (isValidContextUrl(value)) onSubmit(value);
  };

  const handleBackdrop = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (event.target === event.currentTarget) onCancel();
  };

  const surface = <>
    <style>{CONTEXT_VALUE_DIALOG_CSS}</style>
    <div className="context-value-backdrop" onClick={handleBackdrop}>
      <section ref={dialogRef} className="context-value-dialog is-floating" role="dialog"
        aria-modal="true" tabIndex={-1}
        aria-labelledby={titleId} aria-describedby={descriptionId}
        onKeyDown={handleDialogKey}>
        <header className="context-value-header">
          <span className="context-value-header-icon" aria-hidden="true">
            <span className="codicon codicon-link-external" />
          </span>
          <div className="context-value-heading">
            <h2 id={titleId} className="context-value-title">Add URL context</h2>
            <p id={descriptionId} className="context-value-description">
              Attach a web page so the agent can use it as prompt context.
            </p>
          </div>
        </header>
        <form className="context-value-form" onSubmit={handleSubmit}>
          <div className="context-value-body">
            <label className="context-value-label" htmlFor={inputId}>Web address</label>
            <div className="context-value-input-wrap">
              <span className="codicon codicon-link-external context-value-input-icon"
                aria-hidden="true" />
              <input ref={inputRef} id={inputId} className="context-value-input"
                type="url" inputMode="url" autoComplete="off" spellCheck={false}
                value={urlValue} placeholder="https://example.com/page"
                aria-describedby={helpId} aria-invalid={urlInvalid || undefined}
                onChange={event => setUrlValue(event.currentTarget.value)} />
            </div>
            <p id={helpId} className={`context-value-help${urlInvalid ? " is-invalid" : ""}`}>
              {urlInvalid
                ? "Enter a complete address beginning with http:// or https://."
                : "Only complete HTTP and HTTPS addresses can be attached."}
            </p>
          </div>
          <footer className="context-value-actions">
            <button type="button" className="context-value-button" onClick={onCancel}>
              Cancel
            </button>
            <button type="submit" className="context-value-button context-value-submit"
              disabled={!urlReady}>
              Add URL
            </button>
          </footer>
        </form>
      </section>
    </div>
  </>;

  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
