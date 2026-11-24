import type { CSSProperties } from "react";
import type { UpdateState } from "../../shared/updater";

/**
 * The window rail's update control (owner, 2026-10-01, design D4 on the
 * update-button canvas): a small blue-violet tile and its words in the same
 * colours. Available shows an up arrow and "Update"; downloading turns the
 * tile into a progress ring; ready shows a restart arrow, a popping dot on the
 * tile and colour drifting slowly through "Restart to update".
 *
 * Shown only while App's label helper names a phase (available, downloading,
 * ready); the label stays the accessible name and the title says the version.
 */
export function UpdateButton({ state, label, title, disabled, onClick }: {
  readonly state: Pick<UpdateState, "phase" | "progress">;
  /** updateButtonLabel(state): the accessible name. */
  readonly label: string;
  readonly title: string;
  readonly disabled?: boolean;
  readonly onClick: () => void;
}) {
  const downloading = state.phase === "downloading";
  const ready = state.phase === "ready";
  const progress = downloading && state.progress !== null
    ? Math.max(0, Math.min(100, Math.round(state.progress))) : null;
  const words = state.phase === "available" ? "Update" : label;
  return (
    <button type="button"
      className={`workspace-toolbar-btn workspace-toolbar-update is-${state.phase}`}
      aria-label={label}
      title={title}
      disabled={disabled}
      onClick={onClick}>
      {downloading ? (
        <span className={progress === null ? "workspace-update-ring is-waiting" : "workspace-update-ring"}
          style={progress === null ? undefined
            : { "--update-progress": `${progress.toString()}%` } as CSSProperties}
          aria-hidden="true">
          <span>
            <svg width="8" height="8" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2.6"
              strokeLinecap="round" strokeLinejoin="round"><path d="M8 3v9M4.5 8.5 8 12l3.5-3.5" /></svg>
          </span>
        </span>
      ) : (
        <span className="workspace-update-tile" aria-hidden="true">
          {ready ? (
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"
              strokeLinecap="round" strokeLinejoin="round">
              <path d="M12.5 8a4.5 4.5 0 1 1-1.3-3.2" /><path d="M12.6 3v2.6H10" />
            </svg>
          ) : (
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"
              strokeLinecap="round" strokeLinejoin="round"><path d="M8 12.5V4M4.5 7.5 8 4l3.5 3.5" /></svg>
          )}
        </span>
      )}
      <span className="workspace-update-words">{words}</span>
    </button>
  );
}
