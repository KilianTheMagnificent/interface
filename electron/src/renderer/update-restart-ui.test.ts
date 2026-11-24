import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const source = readFileSync(fileURLToPath(new URL("./App.tsx", import.meta.url)), "utf8");
const updater = readFileSync(fileURLToPath(new URL("./use-updater.ts", import.meta.url)), "utf8");

describe("update restart UI", () => {
  it("requires explicit confirmation before mutating Rust updater state", () => {
    const request = updater.indexOf("const requestRestartToUpdate");
    const opens = updater.indexOf("setUpdateRestartConfirmOpen(true)", request);
    const confirm = updater.indexOf("const confirmRestartToUpdate", opens);
    const pending = updater.indexOf("setUpdateRestartPending(true)", confirm);
    const invoke = updater.indexOf("api.restartToUpdate()", pending);
    expect(request).toBeGreaterThan(-1);
    expect(opens).toBeGreaterThan(request);
    expect(confirm).toBeGreaterThan(opens);
    expect(pending).toBeGreaterThan(confirm);
    expect(invoke).toBeGreaterThan(pending);
  });

  it("blocks new edits while macOS prepares the native restart", () => {
    expect(source).toContain("open={updateRestartConfirmOpen || updateRestartPending}");
    expect(source).toContain("pending={updateRestartPending}");
    expect(source).toContain('title={updateRestartPending ? "Preparing the update"');
    expect(source).toContain("|| updateRestartPending;");
    expect(updater).toContain('updateState?.phase === "error"');
    expect(updater).toContain("setUpdateRestartPending(false)");
  });

  it("routes every restart control through the confirmation request", () => {
    expect(source).toContain("onRestartToUpdate={requestRestartToUpdate}");
    // The rail's update control (design D4) restarts through the same request.
    expect(source).toContain('onClick={updateState.phase === "ready" ? requestRestartToUpdate : openSettings}');
    expect(source).not.toContain("onRestartToUpdate={confirmRestartToUpdate}");
  });
});
