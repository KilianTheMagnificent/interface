/** Window-level navigation; presentation only, with destinations owned by App. */
export function SurfaceSwitch({ surface, onChange }: {
  surface: "agent" | "code" | "chat";
  onChange: (surface: "agent" | "code" | "chat") => void;
}) {
  return <nav className="surface-switch" aria-label="Workspace">
    {(["agent", "code", "chat"] as const).map(value => {
      // "Threads" keeps the internal "chat" id and its stored surface value.
      const label = value === "agent" ? "Agent" : value === "code" ? "Code" : "Threads";
      return <button key={value} type="button"
        className={surface === value ? "surface-switch-tab is-active" : "surface-switch-tab"}
        aria-pressed={surface === value} aria-label={label} title={label}
        onClick={() => { onChange(value); }}>
        <span>{label}</span>
      </button>;
    })}
  </nav>;
}
