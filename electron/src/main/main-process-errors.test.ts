import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  MAIN_PROCESS_ERROR_LOG_LIMIT_BYTES, MAIN_PROCESS_ERROR_LOG_NAME,
  describeMainProcessError, recordMainProcessError, runShutdownSteps,
} from "./main-process-errors.js";

function logFile(): string {
  return join(mkdtempSync(join(tmpdir(), "iface-errlog-")), MAIN_PROCESS_ERROR_LOG_NAME);
}

describe("recordMainProcessError", () => {
  it("appends timestamped records in order with the kind, phase and stack", () => {
    const file = logFile();
    const first = new Error("first failure");
    first.stack = "Error: first failure\n    at shutdown (main.ts:1:1)";
    const second = new Error("second failure");
    second.stack = "Error: second failure\n    at running (main.ts:2:2)";

    recordMainProcessError(
      file, "uncaughtException", first, "shutdown", new Date("2026-08-30T17:00:00.000Z"));
    recordMainProcessError(
      file, "unhandledRejection", second, "running", new Date("2026-08-30T17:00:01.000Z"));

    expect(readFileSync(file, "utf8")).toBe([
      "2026-08-30T17:00:00.000Z uncaughtException (shutdown)",
      "Error: first failure",
      "    at shutdown (main.ts:1:1)",
      "",
      "2026-08-30T17:00:01.000Z unhandledRejection (running)",
      "Error: second failure",
      "    at running (main.ts:2:2)",
      "",
      "",
    ].join("\n"));
  });

  it("restarts the file once it grows past the limit", () => {
    const file = logFile();
    writeFileSync(file, "x".repeat(MAIN_PROCESS_ERROR_LOG_LIMIT_BYTES + 1));
    const error = new Error("after the limit");
    error.stack = "Error: after the limit\n    at overflow (main.ts:3:3)";

    recordMainProcessError(
      file, "uncaughtException", error, "shutdown", new Date("2026-08-30T17:00:02.000Z"));

    expect(readFileSync(file, "utf8")).toBe(
      "2026-08-30T17:00:02.000Z uncaughtException (shutdown)\n"
      + "Error: after the limit\n    at overflow (main.ts:3:3)\n\n");
  });

  it("returns the description and stays silent when the path is unwritable", () => {
    const file = join(mkdtempSync(join(tmpdir(), "iface-errlog-")), "missing", "errors.log");
    const error = new Error("nowhere to write");
    error.stack = "Error: nowhere to write\n    at unwritable (main.ts:4:4)";

    expect(recordMainProcessError(file, "uncaughtException", error, "shutdown"))
      .toBe("Error: nowhere to write\n    at unwritable (main.ts:4:4)");
  });

  it("returns the recorded description to the caller", () => {
    const file = logFile();
    const error = new Error("recorded");
    error.stack = "Error: recorded\n    at caller (main.ts:5:5)";
    expect(recordMainProcessError(file, "uncaughtException", error, "running"))
      .toBe("Error: recorded\n    at caller (main.ts:5:5)");
  });
});

describe("describeMainProcessError", () => {
  it("prefers the stack and falls back to name and message", () => {
    const stacked = new Error("with stack");
    stacked.stack = "Error: with stack\n    at frame (main.ts:6:6)";
    expect(describeMainProcessError(stacked)).toBe("Error: with stack\n    at frame (main.ts:6:6)");

    const stackless = new TypeError("no stack");
    stackless.stack = undefined;
    expect(describeMainProcessError(stackless)).toBe("TypeError: no stack");
  });

  it("stringifies a rejection reason that is not an Error", () => {
    expect(describeMainProcessError("plain string reason")).toBe("plain string reason");
    expect(describeMainProcessError(undefined)).toBe("undefined");
    expect(describeMainProcessError({ code: 7 })).toBe("[object Object]");
  });

  it("describes a value String() refuses to convert", () => {
    expect(describeMainProcessError(Symbol("reason"))).toBe("Symbol(reason)");
    // A null-prototype object has no toString, so String() throws.
    expect(describeMainProcessError(Object.create(null) as unknown))
      .toBe("an undescribable object value");
  });
});

describe("runShutdownSteps", () => {
  it("runs every later step, in order, after one throws", () => {
    const ran: string[] = [];
    const failures: unknown[] = [];
    const destroyed = new TypeError("Object has been destroyed");
    runShutdownSteps([
      () => { ran.push("badge"); },
      () => { ran.push("terminal bridge"); throw destroyed; },
      () => { ran.push("ipc bridge"); },
      () => { ran.push("updater"); },
    ], error => { failures.push(error); });
    ran.push("host stop");
    expect(ran).toEqual(["badge", "terminal bridge", "ipc bridge", "updater", "host stop"]);
    expect(failures).toEqual([destroyed]);
  });
});
