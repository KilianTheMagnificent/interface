import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { createPortal } from "react-dom";
import { useModalFocus } from "./ModalBehavior";

export interface TerminalRenameDialogProps {
  readonly open: boolean;
  readonly title: string;
  /** Which object is being renamed. The same dialog serves both, and a literal
      heading told a user renaming a workspace that they were renaming a
      terminal. */
  readonly heading?: string;
  readonly onSubmit: (title: string) => void;
  readonly onCancel: () => void;
}

const TERMINAL_RENAME_DIALOG_CSS = `
.terminal-rename-backdrop { position:fixed; inset:0; z-index:1250; display:flex;
  align-items:center; justify-content:center; padding:18px;
  background:color-mix(in srgb, var(--chrome-bg) 70%, transparent); }
.terminal-rename-dialog { display:flex; flex-direction:column; box-sizing:border-box;
  width:min(440px, calc(100vw - 36px)); color:var(--fg); background:var(--widget-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui); }
.terminal-rename-copy { display:flex; flex-direction:column; gap:8px; padding:16px; }
.terminal-rename-title { margin:0; color:var(--fg-strong); font-size:var(--text-prose);
  font-weight:var(--wght-strong); }
.terminal-rename-label { color:var(--fg-muted); font-size:var(--text-meta); }
.terminal-rename-input { box-sizing:border-box; width:100%; height:var(--control-h);
  padding:0 9px; color:var(--input-fg); background:var(--input-bg);
  border:var(--border-w) solid var(--input-border); border-radius:var(--radius-control);
  outline:none; font-family:var(--font-ui); font-size:var(--text-ui); }
.terminal-rename-input:focus { border-color:var(--focus); box-shadow:0 0 0 1px var(--focus); }
.terminal-rename-actions { display:flex; justify-content:flex-end; gap:6px; padding:10px 12px;
  border-top:var(--border-w) solid var(--border); }
.terminal-rename-button { min-width:72px; height:var(--control-h); padding:0 10px;
  color:var(--fg); background:transparent; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font-family:var(--font-ui); font-size:var(--text-ui); cursor:pointer; }
.terminal-rename-button:hover:not(:disabled) { background:var(--row-hover); }
.terminal-rename-button:disabled { color:var(--fg-faint); cursor:default; }
.terminal-rename-submit { color:var(--accent-fg); background:var(--accent);
  border-color:var(--accent); font-weight:var(--wght-strong); }
`;

export function TerminalRenameDialog({ open, title, heading = "Rename terminal",
    onSubmit, onCancel }: TerminalRenameDialogProps) {
  const baseId = useId();
  const dialogRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(title);
  const handleKeyDown = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => inputRef.current,
    onEscape: onCancel,
  });

  useEffect(() => {
    if (!open) return;
    setValue(title);
    window.requestAnimationFrame(() => { inputRef.current?.select(); });
  }, [open, title]);

  if (!open) return null;
  const normalized = value.trim();
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (normalized !== "") onSubmit(normalized);
  };
  const surface = <>
    <style>{TERMINAL_RENAME_DIALOG_CSS}</style>
    <div className="terminal-rename-backdrop" onPointerDown={event => {
      if (event.target === event.currentTarget) onCancel();
    }}>
      <section ref={dialogRef} className="terminal-rename-dialog is-floating" role="dialog" aria-modal="true"
        aria-labelledby={`${baseId}-title`} tabIndex={-1} onKeyDown={handleKeyDown}>
        <form onSubmit={submit}>
          <div className="terminal-rename-copy">
            <h2 id={`${baseId}-title`} className="terminal-rename-title">{heading}</h2>
            <label className="terminal-rename-label" htmlFor={`${baseId}-input`}>Name</label>
            <input ref={inputRef} id={`${baseId}-input`} className="terminal-rename-input"
              value={value} maxLength={120} autoComplete="off"
              onChange={event => { setValue(event.currentTarget.value); }} />
          </div>
          <div className="terminal-rename-actions">
            <button type="button" className="terminal-rename-button" onClick={onCancel}>Cancel</button>
            <button type="submit" className="terminal-rename-button terminal-rename-submit"
              disabled={normalized === ""}>Rename</button>
          </div>
        </form>
      </section>
    </div>
  </>;
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
