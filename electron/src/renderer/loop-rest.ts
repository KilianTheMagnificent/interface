/**
 * Looping marks animate, then rest (owner, 2026-10-27, battery).
 *
 * `steps()` does not lower Chromium's frame rate: while any loop runs, the
 * compositor still draws the whole window at the display's refresh rate. One
 * working agent's sidebar ticker held a test window at about 54 frames a
 * second against 21 without it, 12% CPU against 5%. So every `infinite` CSS
 * animation runs for about three seconds of its own time once it starts, then
 * holds the first keyframe of its current cycle (a spinner upright, a pulse
 * lit, the ticker's wave frozen mid-step) until its element or animation is
 * replaced: a new working state mounts a new mark, and that one animates again.
 * Time spent paused, as while Interface is not in front (window-activity.ts),
 * does not count. The Effort popover's loops keep their own hover gate.
 *
 * The sidebar's marks are how you see at a glance that an agent is still
 * working, so they never rest and loop as their rules say (owner, 2026-10-28:
 * "the working indicators next to the name of the workspace no longer move").
 * Their rules still hold them while Interface is behind another app or the
 * sidebar is stowed. Stepping them from a twelve-a-second timer instead was
 * measured and saved nothing: it halved the frames, but each became a
 * main-thread frame, where Chromium's own loop draws on the compositor alone.
 *
 * A theme file's moving backdrop never rests either (owner, 2026-11-15:
 * "themes should be able to be animated"): it runs only in a theme the owner
 * picked, and theme-look.ts moves it itself, thirty frames a second, and not
 * at all while Interface is behind another app, under Reduce motion, or when
 * Settings turns moving backdrops off.
 */
export const LOOP_REST_AFTER_MS = 3_000;
/** How often a loop paused before its three seconds are up looks again. */
export const LOOP_PAUSED_RECHECK_MS = 1_000;

/** The part of a CSS animation the rest reads and holds. */
export interface RestingLoop {
  readonly playState: AnimationPlayState;
  currentTime: CSSNumberish | null;
  readonly effect: { getTiming(): EffectTiming } | null;
  pause(): void;
}

/** The loop an `animationstart` names, if it is one that should rest. */
export function startedLoop(event: AnimationEvent): RestingLoop | undefined {
  const target = event.target;
  if (!(target instanceof Element) || target.closest(".effort-popover") !== null) return undefined;
  return target.getAnimations({ subtree: true }).find(animation => {
    if (!(animation instanceof CSSAnimation) || animation.animationName !== event.animationName) return false;
    const effect = animation.effect;
    return effect instanceof KeyframeEffect && effect.target === target
      && (effect.pseudoElement ?? "") === event.pseudoElement
      && effect.getTiming().iterations === Infinity;
  });
}

interface RestClock {
  setTimeout(callback: () => void, ms: number): number;
  clearTimeout(id: number): void;
}

export class LoopRest {
  private readonly timers = new Set<number>();

  constructor(private readonly clock: RestClock) {}

  /** Start counting a loop's own running time toward its rest. */
  watch(loop: RestingLoop): void {
    this.later(loop, Math.max(0, LOOP_REST_AFTER_MS - ran(loop)));
  }

  stop(): void {
    for (const timer of this.timers) this.clock.clearTimeout(timer);
    this.timers.clear();
  }

  private later(loop: RestingLoop, ms: number): void {
    const timer = this.clock.setTimeout(() => {
      this.timers.delete(timer);
      this.check(loop);
    }, ms);
    this.timers.add(timer);
  }

  private check(loop: RestingLoop): void {
    // Cancelled: its element left, or its rule no longer names it.
    if (loop.playState === "idle" || loop.playState === "finished") return;
    const time = ran(loop);
    if (time < LOOP_REST_AFTER_MS) {
      this.later(loop, loop.playState === "running" ? LOOP_REST_AFTER_MS - time : LOOP_PAUSED_RECHECK_MS);
      return;
    }
    const duration = Number(loop.effect?.getTiming().duration);
    loop.pause();
    if (duration > 0) loop.currentTime = Math.floor(time / duration) * duration;
  }
}

function ran(loop: RestingLoop): number {
  return Number(loop.currentTime ?? 0);
}

export function installLoopRest(win: Window = window): () => void {
  const rest = new LoopRest({
    setTimeout: (callback, ms) => win.setTimeout(callback, ms),
    clearTimeout: id => { win.clearTimeout(id); },
  });
  const started = (event: AnimationEvent): void => {
    // A theme's moving backdrop keeps looping (theme-look.ts steps it instead).
    if (event.target instanceof Element && event.target.closest(".theme-backdrop") !== null) return;
    const loop = startedLoop(event);
    if (loop !== undefined && (event.target as Element).closest(".sidebar") === null) rest.watch(loop);
  };
  win.document.addEventListener("animationstart", started, true);
  return () => {
    win.document.removeEventListener("animationstart", started, true);
    rest.stop();
  };
}
