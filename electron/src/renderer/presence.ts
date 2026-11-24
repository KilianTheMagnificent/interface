import { useCallback, useLayoutEffect, useRef, useState, type AnimationEvent, type ReactNode } from "react";

/** Where a conditionally rendered surface is in its life. `still` covers the
 * first render too: a panel that is already open when its owner mounts (a
 * remembered sidebar, a restored workspace) appears in place rather than
 * sliding in on every launch. `leaving` keeps it mounted, inert, until its
 * exit animation ends. */
export type PresencePhase = "still" | "entering" | "leaving";

/** The phase after `open` changes; an `instant` change (the surface belongs to
 * a workspace the user just switched away from or to) neither enters nor
 * leaves. Pure so the transitions are testable. */
export function nextPresencePhase(phase: PresencePhase, wasOpen: boolean, open: boolean,
    instant = false): PresencePhase {
  if (open === wasOpen) return phase;
  if (instant) return "still";
  return open ? "entering" : "leaving";
}

/** Class for the surface's root in `phase`; the stylesheet owns the motion. */
export function presenceClass(phase: PresencePhase): string {
  return phase === "entering" ? " is-entering" : phase === "leaving" ? " is-leaving" : "";
}

/** Longer than any enter or exit animation using it, so a missed animationend
 * (the surface was hidden mid-animation, a stylesheet removed the motion)
 * still settles the phase instead of leaving an inert panel in place. */
const PRESENCE_SETTLE_MS = 480;

export interface Presence {
  /** Render the surface: open, or still animating out. */
  present: boolean;
  phase: PresencePhase;
  /** Settles the phase early; for owners whose animation ends on a descendant. */
  settle: () => void;
  /** Attach to the surface's root; ignores animations bubbling from inside. */
  onAnimationEnd: (event: AnimationEvent<Element>) => void;
}

/** Keeps a toggled surface mounted through its exit animation. */
export function usePresence(open: boolean, instant = false): Presence {
  const [wasOpen, setWasOpen] = useState(open);
  const [storedPhase, setPhase] = useState<PresencePhase>("still");
  // Derived during render, so the frame that closes the surface already
  // carries its leaving class and nothing is ever unmounted for a frame.
  const phase = nextPresencePhase(storedPhase, wasOpen, open, instant);
  if (open !== wasOpen) {
    setWasOpen(open);
    setPhase(phase);
  }
  const settle = useCallback((): void => { setPhase("still"); }, []);
  useLayoutEffect(() => {
    if (phase === "still") return undefined;
    if (phase === "leaving" && window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      settle();
      return undefined;
    }
    const timer = window.setTimeout(settle, PRESENCE_SETTLE_MS);
    return () => { window.clearTimeout(timer); };
  }, [phase, settle]);
  const onAnimationEnd = useCallback((event: AnimationEvent<Element>): void => {
    if (event.target === event.currentTarget) settle();
  }, [settle]);
  return { present: open || phase === "leaving", phase, settle, onAnimationEnd };
}

/** Draws `render` while open and, while leaving, the last thing it drew open.
 * For a surface whose data is gone the moment it closes (the subagent detail
 * clears its selection): it animates out still showing what it showed. The
 * phase comes from the owner's usePresence, which may need it elsewhere. */
export function RetainedPresence({ open, phase, render }: {
  open: boolean;
  phase: PresencePhase;
  render: (phase: PresencePhase) => ReactNode;
}): ReactNode {
  const last = useRef(render);
  useLayoutEffect(() => { if (open) last.current = render; });
  if (open) return render(phase);
  return phase === "leaving" ? last.current(phase) : null;
}
