import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent } from "react";
import { createPortal } from "react-dom";
import type { TerminalLaunchProfile } from "../../shared/ipc";
import { AdversarialReviewIcon } from "./AdversarialReviewIcon";
import { useModalFocus } from "./ModalBehavior";
import {
  terminalLauncherAdversarialPair,
  type TerminalAdversarialReviewLaunchRequest,
  type TerminalAdversarialReviewPair,
  type TerminalLauncherNotice,
} from "./TerminalLauncher";

/**
 * Adversarial review, out of the terminal launcher.
 *
 * It was a mode of "New terminal" that opened no terminal: it reserves two
 * agent chat panes and leaves the Code surface entirely. As a mode it forced
 * the count row, the agent grid and the summary section to each carry a second
 * meaning, and clicking a number silently threw the mode away. As its own
 * dialog it needs one input the launcher does not have — an objective — and
 * says up front where it lands.
 */
export interface AdversarialReviewDialogProps {
  readonly open: boolean;
  readonly profiles: readonly TerminalLaunchProfile[];
  /** The launcher's current working folder. Shown, never edited here. */
  readonly cwd: string | null;
  readonly launching: boolean;
  readonly notice: TerminalLauncherNotice | null;
  readonly onStart: (request: TerminalAdversarialReviewLaunchRequest) => void;
  readonly onCancel: () => void;
}

/**
 * The lineup is fixed, not chosen: review is cross-provider by construction and
 * only Claude Code and Codex have a reviewer workflow. It is both of them or
 * neither — each is the other's reviewer — so naming Claude the builder is the
 * whole decision, and losing either one leaves no lineup at all.
 */
export function adversarialReviewLineup(
    profiles: readonly TerminalLaunchProfile[]): TerminalAdversarialReviewPair | null {
  return terminalLauncherAdversarialPair(profiles, "claude");
}

export const ADVERSARIAL_REVIEW_DIALOG_CSS = `
.adversarial-review-backdrop { position:fixed; inset:0; z-index:1250; display:flex;
  align-items:center; justify-content:center; padding:18px;
  background:color-mix(in srgb, var(--chrome-bg) 70%, transparent); }
.adversarial-review-dialog { display:flex; flex-direction:column; box-sizing:border-box;
  width:min(460px, calc(100vw - 36px)); max-height:calc(100vh - 36px); overflow:auto;
  color:var(--fg); background:var(--widget-bg);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-overlay);
  box-shadow:var(--overlay-shadow); font-family:var(--font-ui); }
.adversarial-review-copy { display:flex; flex-direction:column; gap:10px; padding:16px; }
.adversarial-review-title { margin:0; color:var(--fg-strong); font-size:var(--text-prose);
  font-weight:var(--wght-strong); line-height:var(--lh-tight); }
.adversarial-review-lede { margin:0; color:var(--fg-muted); font-size:var(--text-ui);
  line-height:var(--lh-ui); }
.adversarial-review-icon { display:block; flex:none; }
/* The lineup, as one line: who builds, who reviews. It is fixed, so it is a
   statement rather than a control. */
.adversarial-review-lineup { display:flex; align-items:center; gap:8px; min-width:0;
  padding:8px 10px; color:var(--fg); background:var(--surface);
  border:var(--border-w) solid var(--border); border-radius:var(--radius-container);
  font-size:var(--text-ui); }
.adversarial-review-lineup > .adversarial-review-icon { color:var(--review-accent); }
.adversarial-review-field { display:flex; flex-direction:column; gap:6px; min-width:0; }
.adversarial-review-label { display:flex; justify-content:space-between; gap:12px;
  color:var(--fg-muted); font-size:var(--text-meta); font-weight:var(--wght-strong);
  letter-spacing:var(--track-caps); text-transform:uppercase; }
.adversarial-review-label > span:last-child { color:var(--fg-dim); }
.adversarial-review-objective { box-sizing:border-box; width:100%; min-height:76px;
  padding:9px 11px; resize:vertical; color:var(--input-fg); background:var(--input-bg);
  border:var(--border-w) solid var(--input-border); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-ui); line-height:var(--lh-ui); }
.adversarial-review-objective::placeholder { color:var(--fg-faint); }
.adversarial-review-objective:disabled { color:var(--fg-faint); cursor:default; }
.adversarial-review-objective:focus-visible { outline:var(--focus-ring-w) solid var(--focus);
  outline-offset:calc(-1 * var(--focus-ring-offset)); }
.adversarial-review-folder { min-width:0; overflow:hidden; color:var(--fg-dim);
  font-family:var(--font-mono); font-size:var(--text-meta); text-overflow:ellipsis;
  white-space:nowrap; }
.adversarial-review-help { margin:0; color:var(--fg-dim); font-size:var(--text-meta);
  line-height:var(--lh-ui); }
/* The one sentence explaining why the submit is off. NOT amber — nothing is
   blocked on the user's answer to an agent — and not the same --fg-dim/11px as
   the help copy directly above it, which is how the launcher's version of this
   line hid in plain sight. One tier up and one weight up. */
.adversarial-review-blocked { margin:0; color:var(--fg-muted);
  font-size:var(--text-meta); font-weight:var(--wght-strong); line-height:var(--lh-ui); }
.adversarial-review-notice { padding:9px 11px; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font-size:var(--text-ui); line-height:var(--lh-ui); }
.adversarial-review-notice.is-error { color:var(--danger-fg); background:var(--danger-bg); }
.adversarial-review-notice.is-partial { color:var(--fg); background:var(--row-selected-blur); }
.adversarial-review-actions { display:flex; justify-content:flex-end; gap:6px; padding:10px 12px;
  border-top:var(--border-w) solid var(--border); }
.adversarial-review-button { box-sizing:border-box; min-width:72px; height:var(--control-h);
  padding:0 10px; color:var(--fg); background:transparent;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-ui); cursor:pointer; }
.adversarial-review-button:hover:not(:disabled) { background:var(--row-hover); }
.adversarial-review-button:disabled { color:var(--fg-faint); cursor:default; }
.adversarial-review-submit { color:var(--accent-fg); background:var(--accent);
  border-color:var(--accent); font-weight:var(--wght-strong); }
.adversarial-review-submit:hover:not(:disabled) { background:var(--accent-hover);
  border-color:var(--accent-hover); }
.adversarial-review-submit:disabled { background:var(--surface-raised); border-color:var(--border); }
`;

export function AdversarialReviewDialog({ open, profiles, cwd, launching, notice,
    onStart, onCancel }: AdversarialReviewDialogProps) {
  const baseId = `${useId()}-adversarial-review`;
  const dialogRef = useRef<HTMLElement>(null);
  const objectiveRef = useRef<HTMLTextAreaElement>(null);
  const [objective, setObjective] = useState("");
  const handleKeyDown = useModalFocus({
    open,
    containerRef: dialogRef,
    initialFocus: () => objectiveRef.current,
    onEscape: launching ? () => undefined : onCancel,
    escapeEnabled: !launching,
  });

  // A dialog reopened after a cancel starts empty; it is a new review, not the
  // abandoned one.
  useEffect(() => { if (!open) setObjective(""); }, [open]);

  if (!open) return null;
  const folder = cwd !== null && cwd.trim() !== "" ? cwd : "";
  const pair = adversarialReviewLineup(profiles);
  const blocked = pair === null
    ? "Adversarial review needs both Claude Code and Codex."
    : folder === ""
      ? "Choose a working folder in the launcher before starting review."
      : null;
  const trimmed = objective.trim();
  const disabled = launching || blocked !== null || trimmed === "";

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (disabled || pair === null) return;
    onStart({
      builderProfileId: pair.builder.id,
      reviewerProfileId: pair.reviewer.id,
      cwd: folder,
      objective: trimmed,
    });
  };

  const surface = <>
    <style>{ADVERSARIAL_REVIEW_DIALOG_CSS}</style>
    {/* A backdrop press dismisses an empty dialog, never a typed objective: a
        click beside a textarea is usually a miss, and four sentences of
        instructions are not something to lose to one. Escape and Cancel still
        close it whatever is typed. */}
    <div className="adversarial-review-backdrop" onPointerDown={event => {
      if (!launching && trimmed === "" && event.target === event.currentTarget) onCancel();
    }}>
      <section ref={dialogRef} className="adversarial-review-dialog is-floating" role="dialog"
        aria-modal="true" aria-labelledby={`${baseId}-title`} tabIndex={-1}
        aria-busy={launching || undefined} onKeyDown={handleKeyDown}>
        <form onSubmit={submit}>
          <div className="adversarial-review-copy">
            <h2 id={`${baseId}-title`} className="adversarial-review-title">
              Adversarial review
            </h2>
            {/* The one thing the launcher's mode card never said: this opens no
                terminal, and it leaves the Code surface. */}
            <p className="adversarial-review-lede">
              Opens two agent chat panes on the Threads surface.
            </p>
            <p className="adversarial-review-lineup">
              <AdversarialReviewIcon />
              <span>{pair === null
                ? "Claude Code builds, then Codex reviews"
                : `${pair.builder.label} builds, then ${pair.reviewer.label} reviews`}</span>
            </p>
            <div className="adversarial-review-field">
              <label className="adversarial-review-label" htmlFor={`${baseId}-objective`}>
                <span>Objective</span>
                <span>Required</span>
              </label>
              <textarea ref={objectiveRef} id={`${baseId}-objective`}
                className="adversarial-review-objective" value={objective} maxLength={4000}
                disabled={launching}
                placeholder="What should the builder complete before review?"
                aria-describedby={`${baseId}-help`}
                onChange={event => { setObjective(event.currentTarget.value); }} />
              <p id={`${baseId}-help`} className="adversarial-review-help">
                The reviewer starts after the builder and is asked to challenge the result, not duplicate it.
              </p>
            </div>
            <div className="adversarial-review-field">
              <span className="adversarial-review-label"><span>Working folder</span></span>
              <span className="adversarial-review-folder" title={folder || undefined}>
                {folder || "Choose a working folder in the launcher"}
              </span>
            </div>
            {blocked !== null && (
              <p className="adversarial-review-blocked" role="status">{blocked}</p>
            )}
            {notice !== null && (
              <div className={`adversarial-review-notice is-${notice.kind}`}
                role={notice.kind === "error" ? "alert" : "status"}>
                {notice.message}
              </div>
            )}
          </div>
          <div className="adversarial-review-actions">
            <button type="button" className="adversarial-review-button" disabled={launching}
              onClick={onCancel}>Cancel</button>
            <button type="submit"
              className="adversarial-review-button adversarial-review-submit"
              disabled={disabled}
              title={blocked ?? (trimmed === "" ? "Enter an objective before starting review." : undefined)}>
              {launching ? "Starting review…" : "Start review"}
            </button>
          </div>
        </form>
      </section>
    </div>
  </>;
  // The desktop app always takes the portal branch; the identical surface keeps
  // the ARIA contract inspectable in the project's dependency-free node tests.
  return typeof document === "undefined" ? surface : createPortal(surface, document.body);
}
