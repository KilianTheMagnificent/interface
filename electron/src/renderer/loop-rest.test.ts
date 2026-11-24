import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { LOOP_PAUSED_RECHECK_MS, LOOP_REST_AFTER_MS, LoopRest, type RestingLoop } from "./loop-rest";

function harness() {
  const timers = new Map<number, { callback: () => void; ms: number }>();
  let next = 0;
  const rest = new LoopRest({
    setTimeout: (callback, ms) => { timers.set(++next, { callback, ms }); return next; },
    clearTimeout: id => { timers.delete(id); },
  });
  /** Fire the pending timer after `advance` lets the loops' own clocks move. */
  const fire = (advance: () => void = () => {}): number => {
    const [[id, timer]] = [...timers];
    timers.delete(id);
    advance();
    timer.callback();
    return timer.ms;
  };
  return { timers, rest, fire };
}

function loop(duration: number): RestingLoop & { paused: boolean; playState: AnimationPlayState } {
  const state = {
    paused: false,
    playState: "running" as AnimationPlayState,
    currentTime: 0 as CSSNumberish | null,
    effect: { getTiming: () => ({ duration, iterations: Infinity }) },
    pause() { state.paused = true; state.playState = "paused"; },
  };
  return state;
}

describe("looping marks animate, then rest (owner, 2026-10-27, battery)", () => {
  it("lets a loop run about three seconds, then holds the first frame of its cycle", () => {
    const { timers, rest, fire } = harness();
    const ticker = loop(1_000);
    rest.watch(ticker);
    expect([...timers.values()].map(timer => timer.ms)).toEqual([LOOP_REST_AFTER_MS]);
    fire(() => { ticker.currentTime = 3_420; });
    expect(ticker.paused).toBe(true);
    expect(ticker.currentTime).toBe(3_000);
    expect(timers.size).toBe(0);
  });

  it("counts only the time the loop ran, not time it rested behind another app", () => {
    const { rest, fire } = harness();
    const wave = loop(1_500);
    rest.watch(wave);
    // Interface went behind another app after 1.2 s of motion.
    wave.playState = "paused";
    fire(() => { wave.currentTime = 1_200; });
    expect(wave.paused).toBe(false);
    expect(fire()).toBe(LOOP_PAUSED_RECHECK_MS);
    expect(wave.paused).toBe(false);
    // Back in front: it finishes its three seconds, then rests.
    wave.playState = "running";
    expect(fire()).toBe(LOOP_PAUSED_RECHECK_MS);
    expect(fire(() => { wave.currentTime = 3_000; })).toBe(LOOP_REST_AFTER_MS - 1_200);
    expect(wave.paused).toBe(true);
    expect(wave.currentTime).toBe(3_000);
  });

  it("forgets a loop whose element left or whose rule changed", () => {
    const { timers, rest, fire } = harness();
    const spinner = loop(900);
    rest.watch(spinner);
    spinner.playState = "idle";
    fire(() => { spinner.currentTime = 3_100; });
    expect(spinner.paused).toBe(false);
    expect(timers.size).toBe(0);
  });

  it("is installed by the renderer entry and leaves the Effort popover's hover gate alone", () => {
    const entry = readFileSync(join(__dirname, "main.tsx"), "utf8");
    expect(entry).toContain("installLoopRest(window)");
    const source = readFileSync(join(__dirname, "loop-rest.ts"), "utf8");
    expect(source).toContain('target.closest(".effort-popover") !== null');
  });
});

describe("the sidebar's working marks keep looping (owner, 2026-10-28)", () => {
  it("rests only loops outside the sidebar and leaves the sidebar's to their rules", () => {
    const source = readFileSync(join(__dirname, "loop-rest.ts"), "utf8");
    expect(source).toContain(
      'if (loop !== undefined && (event.target as Element).closest(".sidebar") === null) rest.watch(loop);');
    // Their rules still rest them behind other apps and with the sidebar stowed.
    const theme = readFileSync(join(__dirname, "theme.css"), "utf8");
    expect(theme).toContain(".sidebar .dot-pulse::before { animation-play-state: paused !important; }");
    const sidebar = readFileSync(join(__dirname, "components", "Sidebar.tsx"), "utf8");
    expect(sidebar).toMatch(
      /animation: workspace-ticker-step 1s steps\(6\) infinite;\s*animation-play-state: var\(--ambient-play-state\);/u);
  });
});
