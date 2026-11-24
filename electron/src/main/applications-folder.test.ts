import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  APPLICATIONS_MOVE_QUESTION, offerApplicationsMove, shouldOfferApplicationsMove,
  type ApplicationsFolderSituation,
} from "./applications-folder";

const downloaded: ApplicationsFolderSituation = {
  isPackaged: true, platform: "darwin", inApplicationsFolder: false, unattended: false,
};

describe("the offer to move into Applications", () => {
  it("is made to a packaged app opened from anywhere else on a Mac", () => {
    expect(shouldOfferApplicationsMove(downloaded)).toBe(true);
  });

  it("is never made in Applications, in development, off the Mac or in an unattended run", () => {
    for (const change of [
      { inApplicationsFolder: true }, { isPackaged: false },
      { platform: "linux" as const }, { unattended: true },
    ]) {
      expect(shouldOfferApplicationsMove({ ...downloaded, ...change })).toBe(false);
    }
  });

  it("moves on yes and tells the launch to stop, and stays put on Not Now", () => {
    const calls: string[] = [];
    const effects = (answer: number, moved: boolean) => ({
      ask: () => { calls.push("ask"); return answer; },
      move: () => { calls.push("move"); return moved; },
    });
    expect(offerApplicationsMove(downloaded, effects(0, true))).toBe(true);
    expect(calls).toEqual(["ask", "move"]);
    calls.length = 0;
    expect(offerApplicationsMove(downloaded, effects(APPLICATIONS_MOVE_QUESTION.cancelId, true)))
      .toBe(false);
    expect(calls).toEqual(["ask"]);
  });

  it("keeps running where it is when macOS refuses the move", () => {
    expect(offerApplicationsMove(downloaded, { ask: () => 0, move: () => false })).toBe(false);
    expect(offerApplicationsMove(downloaded, {
      ask: () => 0, move: () => { throw new Error("not permitted"); },
    })).toBe(false);
  });

  it("asks nothing where no offer is due", () => {
    const ask = (): number => { throw new Error("asked"); };
    expect(offerApplicationsMove({ ...downloaded, inApplicationsFolder: true },
      { ask, move: () => true })).toBe(false);
  });

  it("is the first thing a launch does, before the host or a window exists", () => {
    const main = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    const start = main.indexOf("async function start(): Promise<void> {");
    const offer = main.indexOf("offerApplicationsMove(", start);
    expect(start).toBeGreaterThan(-1);
    expect(offer).toBeGreaterThan(start);
    expect(offer).toBeLessThan(main.indexOf("resolveRustHostPath(", start));
  });
});
