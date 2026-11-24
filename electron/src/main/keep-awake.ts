/**
 * Keeps the Mac out of idle sleep while agents work (review D3, 2026-10-14).
 *
 * Driven by the renderer's work-in-flight report, the same count the quit
 * prompt reads (shared/quit-guard.ts): one `prevent-app-suspension` blocker
 * while anything is working, none otherwise. The display may still sleep,
 * and so may a closed lid; an agent only waiting for an answer holds nothing,
 * since nothing would progress overnight anyway.
 */

import { workingNow, type WorkInFlightV1 } from "../shared/quit-guard.js";

/** Electron's `powerSaveBlocker`, as far as this uses it. */
export interface PowerSaveBlockerPort {
  start(type: "prevent-app-suspension"): number;
  stop(id: number): void;
}

export interface KeepAwake {
  /** Hold or release for the latest report; null (no report, host gone) releases. */
  update(work: WorkInFlightV1 | null): void;
  /** Release for good: Interface is quitting. */
  release(): void;
  held(): boolean;
}

export function createKeepAwake(blocker: PowerSaveBlockerPort): KeepAwake {
  let id: number | null = null;
  let released = false;
  const stop = (): void => {
    if (id === null) return;
    const held = id;
    id = null;
    blocker.stop(held);
  };
  return {
    update(work) {
      if (released) return;
      if (!workingNow(work)) { stop(); return; }
      if (id === null) id = blocker.start("prevent-app-suspension");
    },
    release() {
      released = true;
      stop();
    },
    held: () => id !== null,
  };
}

/** For sessions with no person at the Mac (hands-free, packaged smoke). */
export const NO_KEEP_AWAKE: KeepAwake = { update() {}, release() {}, held: () => false };
