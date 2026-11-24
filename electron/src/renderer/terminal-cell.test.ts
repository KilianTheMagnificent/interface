import { describe, expect, it } from "vitest";
import { cellSnapLetterSpacing, terminalAdvance } from "./terminal-cell";

describe("terminal cell width", () => {
  it("matches the owner's Rex pane: MonoLisa 11.5px is 15 device pixels wide on Retina", () => {
    // MonoLisa advances 0.64em. Rex reports 15x32 device pixel cells at 11.5pt.
    const advance = 0.64 * 11.5;
    expect(Math.floor(advance * 2) + cellSnapLetterSpacing(advance, 2)).toBe(15);
    // On a 1x display the nearest pixel is the floor, so nothing is added.
    expect(Math.floor(advance) + cellSnapLetterSpacing(advance, 1)).toBe(7);
  });

  it("adds a pixel only when the nearest device pixel is above the floor", () => {
    expect(cellSnapLetterSpacing(7.2, 2)).toBe(0);   // 14.4 -> 14
    expect(cellSnapLetterSpacing(7.25, 2)).toBe(1);  // 14.5 -> 15
    expect(cellSnapLetterSpacing(8.4, 2)).toBe(1);   // 16.8 -> 17
    expect(cellSnapLetterSpacing(8, 2)).toBe(0);
    expect(cellSnapLetterSpacing(7.36, 1.5)).toBe(0); // 11.04 -> 11
  });

  it("adds nothing when the advance could not be measured", () => {
    for (const advance of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(cellSnapLetterSpacing(advance, 2)).toBe(0);
    }
    expect(cellSnapLetterSpacing(7.36, Number.NaN)).toBe(0);
    // No document here, so no canvas to measure with.
    expect(terminalAdvance("monospace", 12)).toBe(0);
  });
});
