import "./DropIndicator.css";

/** The one attachment-drop cue: a soft glow behind a pill saying what the drop does. */
export function DropIndicator({ label }: { readonly label: string }) {
  return (
    <div className="drop-indicator" aria-hidden="true">
      <span className="drop-indicator-pill">
        <span className="codicon codicon-file" />{label}
      </span>
    </div>
  );
}
