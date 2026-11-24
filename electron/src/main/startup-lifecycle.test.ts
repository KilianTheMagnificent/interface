import { describe, expect, it } from "vitest";
import {
  StartupStepSupersededError, createStartupGate, withStartupDeadline,
} from "./startup-lifecycle.js";

describe("startup lifecycle", () => {
  it("returns a host startup result before the deadline", async () => {
    await expect(withStartupDeadline("test service", Promise.resolve("ready"), 50))
      .resolves.toBe("ready");
  });

  it("preserves an immediate host startup failure", async () => {
    const failure = new Error("host refused startup");
    await expect(withStartupDeadline("test service", Promise.reject(failure), 50))
      .rejects.toBe(failure);
  });

  it("rejects a host startup step that never settles", async () => {
    await expect(withStartupDeadline("test service", new Promise(() => {}), 15))
      .rejects.toThrow("The Interface test service did not start within 15 ms.");
  });

  it("refuses a deferred success after its startup owner shuts down", async () => {
    let finish!: (value: string) => void;
    let active = true;
    const requireStep = createStartupGate(
      new Promise<string>((resolve) => { finish = resolve; }),
      () => active,
    );
    const pending = requireStep();

    active = false;
    finish("ready");

    await expect(pending).rejects.toBeInstanceOf(StartupStepSupersededError);
  });

  it("suppresses a deferred failure after its startup owner shuts down", async () => {
    let fail!: (error: unknown) => void;
    let active = true;
    const original = new Error("late configuration failure");
    const requireStep = createStartupGate(
      new Promise<string>((_resolve, reject) => { fail = reject; }),
      () => active,
    );
    const pending = requireStep();

    active = false;
    fail(original);

    await expect(pending).rejects.toBeInstanceOf(StartupStepSupersededError);
  });

  it("preserves a deferred failure while its startup owner is active", async () => {
    const original = new Error("configuration failed");
    const requireStep = createStartupGate(Promise.reject(original), () => true);
    await expect(requireStep()).rejects.toBe(original);
  });
});
