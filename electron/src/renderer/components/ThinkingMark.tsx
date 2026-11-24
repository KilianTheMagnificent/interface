import type { ReactElement } from "react";
import "./ThinkingMark.css";

/**
 * THE THREADS THINKING MARK (owner, 2026-10-23, "Pair"): two beads that trade
 * places, one passing behind the other, as two threads do in a braid. It marks
 * a Threads turn that is working: the live turn's status line, the jump
 * capsule and an inbox row's slot. Elsewhere the settled Wave stays.
 *
 * Presentation only; callers decide whether work is active. It moves like every
 * loop in the app (about 12 frames a second, resting while Interface is not in
 * front) and holds as the resting pair under reduced motion or when `still`,
 * which is also the mark beside the New thread greeting.
 */
export function ThinkingMark({ size = 16, still = false, className }: {
  size?: number;
  still?: boolean;
  className?: string;
}): ReactElement {
  const classes = ["thinking-mark", still ? "is-still" : "", className ?? ""].filter(Boolean).join(" ");
  return (
    <svg className={classes} width={size} height={size} viewBox="0 0 24 24"
      aria-hidden="true" focusable="false">
      <circle className="thinking-mark-a" cx="7" cy="12" r="3.25" />
      <circle className="thinking-mark-b" cx="17" cy="12" r="3.25" />
    </svg>
  );
}
