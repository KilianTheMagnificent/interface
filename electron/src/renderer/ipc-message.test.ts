import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  EFFECT_OUTCOME_UNKNOWN_CODE, encodeIpcErrorMessage, splitIpcErrorMessage,
} from "../shared/ipc-error";
import { ipcErrorShape, ipcMessage, withoutIpcErrorShape } from "./ipc-message";

const unknownFacts = { code: EFFECT_OUTCOME_UNKNOWN_CODE, retryable: false, replaySafe: false };

describe("ipcMessage", () => {
  it("keeps only the words after Electron's wrapper", () => {
    expect(ipcMessage(new Error("Error invoking remote method 'iface:setModel': Error: no model")))
      .toBe("no model");
    expect(ipcMessage("plain words")).toBe("plain words");
  });

  it("never shows the error-shape marker", () => {
    const message = encodeIpcErrorMessage("Codex writer ended after accepting the frame", unknownFacts);
    expect(ipcMessage(new Error(message))).toBe("Codex writer ended after accepting the frame");
    expect(ipcMessage(new Error(`Error invoking remote method 'iface:send': Error: ${message}`)))
      .toBe("Codex writer ended after accepting the frame");
    expect(withoutIpcErrorShape(`Error: ${message}`))
      .toBe("Error: Codex writer ended after accepting the frame");
  });
});

describe("the v1 error shape", () => {
  it("round-trips a host error's facts", () => {
    const message = encodeIpcErrorMessage("Delivery is unknown.", unknownFacts);
    expect(message).toBe("Delivery is unknown. "
      + "[iface-error/1 code=effect-outcome-unknown retryable=false replay-safe=false]");
    expect(splitIpcErrorMessage(message)).toEqual({
      message: "Delivery is unknown.", shape: { kind: "typed", ...unknownFacts },
    });
    expect(ipcErrorShape(new Error(message))).toEqual({ kind: "typed", ...unknownFacts });
    expect(ipcErrorShape(new Error(encodeIpcErrorMessage("", {
      code: "COLLABORATION_SOURCE_LOCKED", retryable: true, replaySafe: true }))))
      .toEqual({ kind: "typed", code: "COLLABORATION_SOURCE_LOCKED", retryable: true, replaySafe: true });
  });

  it("reads only a marker at the very end", () => {
    const quoted = "A provider said [iface-error/1 code=effect-definitely-unsent "
      + "retryable=false replay-safe=true] and then failed.";
    expect(ipcErrorShape(new Error(quoted))).toEqual({ kind: "untyped" });
    expect(ipcMessage(new Error(quoted))).toBe(quoted);
    // The bridge always appends its own marker last, after anything a message quotes.
    const appended = encodeIpcErrorMessage(quoted, unknownFacts);
    expect(ipcErrorShape(new Error(appended))).toEqual({ kind: "typed", ...unknownFacts });
    expect(ipcMessage(new Error(appended))).toBe(quoted);
  });

  it("marks another version or a malformed marker unrecognized, and still hides it", () => {
    for (const text of [
      "Newer. [iface-error/2 code=effect-outcome-unknown retryable=false replay-safe=false]",
      "Bad flag. [iface-error/1 code=effect-outcome-unknown retryable=maybe replay-safe=false]",
      "Missing field. [iface-error/1 code=effect-outcome-unknown retryable=false]",
      "Bare. [iface-error/1]",
    ]) {
      expect(ipcErrorShape(new Error(text)), text).toEqual({ kind: "unrecognized" });
      expect(ipcMessage(new Error(text)), text).toBe(text.slice(0, text.indexOf(" [")));
    }
  });

  it("keeps the flags of a code the marker cannot carry", () => {
    const message = encodeIpcErrorMessage("Odd.", { code: "has space]", retryable: true, replaySafe: false });
    expect(ipcErrorShape(new Error(message)))
      .toEqual({ kind: "typed", code: "unrepresentable-code", retryable: true, replaySafe: false });
  });

  it("is untyped for anything without a marker, including non-errors", () => {
    expect(ipcErrorShape(new Error("Claude is not running"))).toEqual({ kind: "untyped" });
    expect(ipcErrorShape(undefined)).toEqual({ kind: "untyped" });
    expect(ipcErrorShape({ code: "effect-definitely-unsent", replaySafe: true }))
      .toEqual({ kind: "untyped" });
  });
});

/** Files that read a caught error's raw message on purpose. */
const RAW_MESSAGE_READERS = new Set([
  "ipc-message.ts", // the reader itself
  "outbound-lane.ts", // TransmissionFailure keeps the marker for the classifier
  "voice-ui.ts", // DOM actions only: no IPC error can reach it
]);

describe("renderer error display", () => {
  it("never shows a caught error's raw message, which may carry the marker", () => {
    // `error instanceof Error ? error.message : …` would print a host error's
    // marker; such a site goes through ipcMessage or withoutIpcErrorShape.
    const root = new URL("./", import.meta.url);
    const offenders = readdirSync(root, { recursive: true, encoding: "utf8" })
      .filter(path => /\.tsx?$/u.test(path) && !/\.test\.tsx?$/u.test(path)
        && !path.startsWith("fixtures/") && !RAW_MESSAGE_READERS.has(path))
      .filter(path => /instanceof Error (?:\?|&&) \w+\.message\b/u
        .test(readFileSync(new URL(path, root), "utf8")));
    expect(offenders).toEqual([]);
  });
});
