import { describe, expect, it } from "vitest";
import {
  PASS_OFF_TRANSCRIPT_MAX_BYTES, passOffRefusal, passOffTitle, passOffTranscript,
} from "./pass-off";

describe("passOffTranscript", () => {
  it("joins lines and strips trailing blanks", () => {
    expect(passOffTranscript(["a", "b", "", ""])).toBe("a\nb");
  });
  it("keeps the newest lines when over budget and says so", () => {
    const line = "x".repeat(1024);
    const lines = Array.from({ length: 600 }, () => line);
    const result = passOffTranscript(lines);
    expect(result.length).toBeLessThanOrEqual(PASS_OFF_TRANSCRIPT_MAX_BYTES);
    expect(result.startsWith("[transcript truncated]\n")).toBe(true);
    expect(result.endsWith(line)).toBe(true);
  });
  it("returns an empty string for an empty buffer", () => {
    expect(passOffTranscript([])).toBe("");
  });
  it("budgets UTF-8 bytes, not UTF-16 units, for multi-byte lines", () => {
    // The host caps handoff.transcript on String::len(), which counts bytes. A
    // box-drawing character is one UTF-16 unit but three UTF-8 bytes, so
    // budgeting on .length would ship ~3x the cap and be refused on arrival.
    const line = "─".repeat(1024);
    const lines = Array.from({ length: 600 }, () => line);
    const result = passOffTranscript(lines);
    expect(new TextEncoder().encode(result).length)
      .toBeLessThanOrEqual(PASS_OFF_TRANSCRIPT_MAX_BYTES);
    expect(result.startsWith("[transcript truncated]\n")).toBe(true);
    expect(result.endsWith(line)).toBe(true);
  });
  it("reserves room for the truncation note at the byte boundary", () => {
    // 512 lines of 1023 chars join to exactly one byte under the cap: inside the
    // raw limit, but over it once the note is reserved, so it must truncate.
    const atCap = Array.from({ length: 512 }, () => "x".repeat(1023));
    expect(atCap.join("\n").length).toBe(PASS_OFF_TRANSCRIPT_MAX_BYTES - 1);
    expect(passOffTranscript(atCap).startsWith("[transcript truncated]\n")).toBe(true);
    // One line fewer clears the reserved budget and is kept whole.
    const underCap = atCap.slice(1);
    expect(passOffTranscript(underCap)).toBe(underCap.join("\n"));
  });
});

describe("passOffRefusal", () => {
  const profile = { available: true, label: "Codex" };
  it("allows a running source and an available profile", () => {
    expect(passOffRefusal({ status: "running" }, profile)).toBeNull();
  });
  it("refuses a missing or non-running source", () => {
    expect(passOffRefusal(undefined, profile)).toMatch(/running/);
    expect(passOffRefusal({ status: "exited" }, profile)).toMatch(/running/);
  });
  it("refuses an unavailable profile with its reason", () => {
    expect(passOffRefusal({ status: "running" },
      { available: false, label: "Codex", unavailableReason: "Codex is not installed" }))
      .toBe("Codex is not installed");
    expect(passOffRefusal({ status: "running" }, undefined)).toMatch(/launcher/i);
  });
});

describe("passOffTitle", () => {
  it("keeps a plain title, trimmed", () => {
    expect(passOffTitle("  API server  ")).toBe("API server");
  });
  it("drops an empty or control-bearing title rather than losing the request", () => {
    // The host refuses the whole handoff on a bad fromTitle, so a title it
    // would reject is omitted instead of sent.
    expect(passOffTitle("   ")).toBeUndefined();
    expect(passOffTitle("API\u0007server")).toBeUndefined();
    // The host checks Rust's char::is_control, which is the whole Cc
    // category: U+0085 is not ASCII and JS trim() does not remove it.
    expect(passOffTitle("API\u0085server")).toBeUndefined();
  });
  it("drops a title over the host's 200-byte cap, counting UTF-8 bytes", () => {
    expect(passOffTitle("a".repeat(200))).toBe("a".repeat(200));
    expect(passOffTitle("a".repeat(201))).toBeUndefined();
    // 67 box-drawing characters are 67 UTF-16 units but 201 UTF-8 bytes.
    expect(passOffTitle("\u2500".repeat(67))).toBeUndefined();
    expect(passOffTitle("\u2500".repeat(66))).toBe("\u2500".repeat(66));
  });
});
