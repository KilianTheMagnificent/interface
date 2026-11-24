import { useEffect, useRef, useState } from "react";
import type { FastModeState } from "../../shared/engine";
import { FastBolt } from "./FastBolt";

/**
 * The provider's own vocabulary for why fast mode cannot serve, rendered as a
 * sentence a person can act on. Unknown codes fall back to the generic line
 * rather than leaking identifiers into a tooltip.
 */
const REASON_COPY: Readonly<Record<string, string>> = {
  free: "Fast mode needs a paid plan",
  preference: "Fast mode is turned off in Claude settings",
  extra_usage_disabled: "Enable extra usage in Claude settings to use fast mode",
  network_error: "Fast mode is unreachable right now",
  not_first_party: "Fast mode is only available on first-party Claude",
  disabled_by_env: "Fast mode is disabled by environment configuration",
  model_not_allowed: "This model does not support fast mode",
  sdk_opt_in_required: "Fast mode needs SDK opt-in",
  pending: "Fast mode is warming up",
  unknown: "Fast mode is unavailable right now",
};

export function fastModeReasonCopy(reason: string | undefined): string | null {
  if (reason === undefined) return null;
  return REASON_COPY[reason] ?? REASON_COPY.unknown;
}

/** Cooldown is an enabled preference temporarily throttled by the provider. */
export function fastModeIsEnabled(state: FastModeState): boolean {
  return state !== "off";
}

export const FAST_MODE_CSS = `
/* 24 square: the app's target floor, and the height every other control on the
   composer row it stands in already takes. */
.fastmode-toggle { position:relative; display:inline-flex; align-items:center;
  justify-content:center; width:24px; height:24px; padding:0; overflow:hidden;
  color:var(--fg-muted); background:transparent;
  border:var(--border-w) solid transparent; border-radius:var(--radius-pill);
  font:inherit; line-height:1; cursor:pointer; }
.fastmode-toggle:hover { color:var(--fg); background:var(--row-hover); }
.fastmode-toggle:focus-visible { outline:var(--focus-ring-w) solid var(--focus);
  outline-offset:var(--focus-ring-offset); }
.fastmode-toggle .fast-bolt { flex:none; }
.fastmode-toggle[aria-busy="true"] { color:var(--fg-faint); cursor:default; }
.fastmode-toggle:disabled { color:var(--fg-faint); cursor:default; }
.fastmode-toggle:disabled:hover { background:transparent; }
/* On: the bolt takes the accent and keeps a faint charge. */
.fastmode-toggle.is-on { color:var(--accent-text);
  border-color:var(--border); background:var(--surface-raised); }
.fastmode-toggle.is-on:hover { color:var(--accent-text-hover); }
/* Cooldown: alive but resting — a slow breath, never an error state. */
.fastmode-toggle.is-cooldown { color:var(--fg-muted); }
.fastmode-toggle.is-cooldown .fast-bolt {
  animation:fastmode-breathe 2.4s steps(14) infinite; animation-play-state:var(--ambient-play-state); }
/* The flair: flipping it on sends one light streak through the bolt. */
.fastmode-streak { position:absolute; top:0; bottom:0; left:-60%; width:60%;
  background:linear-gradient(90deg, transparent,
    color-mix(in srgb, var(--accent-text) 35%, transparent), transparent);
  animation:fastmode-dash 340ms ease-out forwards; pointer-events:none; }
@keyframes fastmode-dash {
  from { transform:translateX(0); opacity:1; }
  to { transform:translateX(350%); opacity:0; }
}
@keyframes fastmode-breathe {
  0%, 100% { opacity:.45; }
  50% { opacity:.95; }
}
@media (prefers-reduced-motion: reduce) {
  .fastmode-streak { animation:none; opacity:0; }
  .fastmode-toggle.is-cooldown .fast-bolt { animation:none; opacity:.7; }
}
`;

export interface FastModeToggleProps {
  state: FastModeState;
  /** Provider's disabled/cooldown reason code, when it named one. */
  reason?: string;
  pending?: boolean;
  disabled?: boolean;
  onToggle: (enabled: boolean) => void;
  className?: string;
}

/**
 * A bolt, and nothing else, for engines that advertise fast mode. Off is quiet,
 * on is accent-lit (with a one-shot streak as it ignites), and the SDK's
 * cooldown state breathes instead of pretending to be broken. The state it is
 * in is carried by `aria-pressed` and the title, so the icon needs no label.
 */
export function FastModeToggle({
  state, reason, pending = false, disabled = false, onToggle, className,
}: FastModeToggleProps) {
  const on = state === "on";
  const cooldown = state === "cooldown";
  const enabled = fastModeIsEnabled(state);
  // The streak fires exactly on off->on transitions; keying the element on a
  // counter restarts the CSS animation for every ignition.
  const [streak, setStreak] = useState(0);
  const wasOn = useRef(on);
  useEffect(() => {
    if (on && !wasOn.current) setStreak(count => count + 1);
    wasOn.current = on;
  }, [on]);

  const reasonCopy = fastModeReasonCopy(reason);
  const title = cooldown
    ? reasonCopy ?? "Cooling down after a rate limit — fast mode resumes on its own"
    : on ? "Fast mode is on"
    : reasonCopy ?? "Fast mode: quicker output on models that support it";
  const classes = [
    "fastmode-toggle",
    on ? "is-on" : "",
    cooldown ? "is-cooldown" : "",
    className ?? "",
  ].filter(Boolean).join(" ");

  return <>
    <style>{FAST_MODE_CSS}</style>
    <button type="button" className={classes}
      aria-pressed={enabled} aria-busy={pending || undefined}
      aria-label="Fast mode" disabled={disabled} title={title}
      onClick={() => {
        if (pending || disabled) return;
        onToggle(!enabled);
      }}>
      <FastBolt />
      {streak > 0 && <span key={streak} className="fastmode-streak" aria-hidden="true" />}
    </button>
  </>;
}
