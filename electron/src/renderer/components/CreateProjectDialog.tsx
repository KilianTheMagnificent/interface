import { useId, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import type { ProjectsFolderView } from "../../shared/projects";
import { ipcMessage } from "../ipc-message";
import { CONFIRM_DIALOG_CSS } from "./ConfirmDialog";
import { useModalFocus } from "./ModalBehavior";

interface CreateProjectDialogProps {
  onCreate: (name: string) => Promise<void>;
  onCancel: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
  /** Where the project goes (Rust's Projects folder); null while it is read. */
  folder?: ProjectsFolderView | null;
  /** Change…: the native picker, after which Rust keeps the new Projects folder. */
  onChangeFolder?: () => Promise<void>;
}

/** The path line: the Projects folder as Rust shows it, then the folder New project makes. */
export function createProjectPathLine(folder: ProjectsFolderView | null | undefined, name: string): string {
  return `${folder?.display ?? "Projects folder"}/${name.trim() || "project-name"}`;
}

export function CreateProjectDialog({ onCreate, onCancel, returnFocusRef, folder, onChangeFolder }:
    CreateProjectDialogProps) {
  const id = useId();
  const dialogRef = useRef<HTMLFormElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const inFlight = useRef(false);
  const [name, setName] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);
  const changeFolder = async (): Promise<void> => {
    if (onChangeFolder === undefined || inFlight.current) return;
    inFlight.current = true;
    setChanging(true);
    setError(null);
    try {
      await onChangeFolder();
    } catch (reason: unknown) {
      setError(ipcMessage(reason));
    } finally {
      inFlight.current = false;
      setChanging(false);
    }
  };
  const handleKey = useModalFocus({
    open: true, containerRef: dialogRef, initialFocus: () => inputRef.current,
    onEscape: onCancel, escapeEnabled: !pending && !changing, returnFocusRef,
  });
  const submit = async (): Promise<void> => {
    if (inFlight.current || name.trim() === "") return;
    inFlight.current = true;
    setPending(true);
    setError(null);
    try {
      await onCreate(name.trim());
    } catch (reason: unknown) {
      setError(ipcMessage(reason));
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  };
  const surface = <>
    <style>{CONFIRM_DIALOG_CSS}{`
      .create-project-field { display:flex; flex-direction:column; gap:8px; margin-top:16px;
        color:var(--fg); font-size:var(--text-ui); }
      .create-project-input { box-sizing:border-box; width:100%; padding:8px 10px; font:inherit;
        color:var(--fg); background:var(--input-bg); border:var(--border-w) solid var(--border-strong);
        border-radius:var(--radius-control); }
      .create-project-where { display:flex; align-items:baseline; gap:8px; margin:8px 0 0; }
      .create-project-path { flex:1; min-width:0; margin:0; overflow-wrap:anywhere; color:var(--fg-muted);
        font-size:var(--text-ui); line-height:var(--lh-ui); }
      .create-project-change { flex:none; padding:0; font:inherit; color:var(--accent-text);
        background:none; border:0; cursor:pointer; }
      .create-project-change:disabled { color:var(--fg-muted); cursor:default; }
      .create-project-error { color:var(--danger-fg); }
    `}</style>
    <div className="confirm-dialog-backdrop" onClick={event => {
      if (!inFlight.current && event.target === event.currentTarget) onCancel();
    }}>
      <form ref={dialogRef} className="confirm-dialog create-project-dialog is-floating" role="dialog"
        aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}
        aria-busy={pending || undefined} tabIndex={-1} onKeyDown={handleKey}
        onSubmit={event => { event.preventDefault(); void submit(); }}>
        <div className="confirm-dialog-copy">
          <h2 id={`${id}-title`} className="confirm-dialog-title">New project</h2>
          <p id={`${id}-description`} className="confirm-dialog-description">
            Create a folder in your Projects folder, then choose panes for your workspace.
          </p>
          <label className="create-project-field" htmlFor={`${id}-name`}>Project name
            <input ref={inputRef} id={`${id}-name`} className="create-project-input" value={name}
              placeholder="My project" autoComplete="off" spellCheck={false} disabled={pending || changing}
              aria-invalid={error !== null || undefined} aria-describedby={`${id}-path${error ? ` ${id}-error` : ""}`}
              onChange={event => { setName(event.target.value); setError(null); }} />
          </label>
          <div className="create-project-where">
            <p id={`${id}-path`} className="create-project-path">{createProjectPathLine(folder, name)}</p>
            {onChangeFolder !== undefined && (
              <button type="button" className="create-project-change" disabled={pending || changing}
                aria-label="Change the Projects folder" title={folder?.path}
                onClick={() => { void changeFolder(); }}>
                {changing ? "Choosing…" : "Change…"}
              </button>
            )}
          </div>
          {error && <p id={`${id}-error`} className="confirm-dialog-description create-project-error" role="alert">{error}</p>}
        </div>
        <div className="confirm-dialog-actions">
          <button type="button" className="confirm-dialog-button" disabled={pending} onClick={onCancel}>Cancel</button>
          <button type="submit" className="confirm-dialog-button confirm-dialog-confirm"
            disabled={pending || changing || name.trim() === ""}>
            {pending ? "Creating…" : "Create project"}
          </button>
        </div>
      </form>
    </div>
  </>;
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
