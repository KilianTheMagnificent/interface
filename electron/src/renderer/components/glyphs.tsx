import { WorkingIndicator } from "./WorkingIndicator";
import type { RunState } from "../tool-activity";

// VS Code's own icon font, already on disk via monaco — see codicons.css. Icons
// inherit `color`, so they theme for free; every one is decorative and the
// control that owns it carries the accessible name.
export function Icon({ name, sm, className }: { name: string; sm?: boolean; className?: string }) {
  const size = sm === true ? " codicon-sm" : "";
  const extra = className === undefined ? "" : ` ${className}`;
  return <span className={`codicon codicon-${name}${size}${extra}`} aria-hidden="true" />;
}

/** Each surface's colour classes for a settled state. */
const GLYPH_TIERS = {
  transcript: { ok: "conv-glyph-ok", failed: "conv-glyph-fail", ended: "conv-glyph-idle" },
  panel: { ok: "subp-ok", failed: "subp-fail", ended: "subp-idle" },
} as const;

// The glyph half of a status. Running keeps the app's pulsing dot — the same
// shape the sidebar and the subagent rail use, so "running" looks like one thing
// everywhere — and every settled state gets a codicon instead of a colour.
// "ended" is deliberately neutral: debug-stop in the dim tier says the stream
// stopped, which is all we know, and it never pulses.
export function StateGlyph({ state, tier = "transcript" }: {
  state: RunState;
  tier?: keyof typeof GLYPH_TIERS;
}) {
  if (state === "running") return <WorkingIndicator />;
  const classes = GLYPH_TIERS[tier];
  if (state === "ok") return <Icon name="check" sm className={classes.ok} />;
  if (state === "failed") return <Icon name="error" sm className={classes.failed} />;
  return <Icon name="debug-stop" sm className={classes.ended} />;
}
