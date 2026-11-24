import { describe, expect, it } from "vitest";
import type { SessionSummary } from "../shared/engine";
import { sessionConfigFor } from "./app-session-projection";
import {
  givenPermissionMode, modeInForce, modeToRestore, sessionHasPermissionControls,
  sessionHasWorkspaceContext, sessionPermissionMode,
} from "./session-presentation";
import fixture from "../fixtures/session-mode-v68.json";

describe("Chat session presentation and resume", () => {
  it.each(["chat", "voice"] as const)("never restores Code permission presets into %s", kind => {
    for (const saved of [undefined, "default", "auto", "plan", "bypassPermissions"]) {
      expect(sessionPermissionMode({ kind }, undefined, saved)).toBeUndefined();
      expect(sessionPermissionMode({ kind }, saved, "auto")).toBeUndefined();
    }
    expect(sessionHasPermissionControls({ kind })).toBe(false);
  });

  it("distinguishes plain Chat from Voice's host-provided tool context", () => {
    expect(sessionHasWorkspaceContext({ kind: "chat" })).toBe(false);
    expect(sessionHasWorkspaceContext({ kind: "voice" })).toBe(true);
  });

  it.each([undefined, "code", "agent"] as const)("preserves workspace settings for %s", kind => {
    expect(sessionPermissionMode({ kind }, "plan", "auto")).toBe("plan");
    expect(sessionPermissionMode({ kind }, undefined, "auto")).toBe("auto");
    expect(sessionPermissionMode({ kind }, "default", "auto")).toBe("auto");
    expect(sessionPermissionMode({ kind }, undefined, undefined)).toBe("default");
    expect(sessionHasWorkspaceContext({ kind })).toBe(true);
  });

  it("carries the mode a workspace session was given to its forks, and none for Chat or Voice", () => {
    // 6 Oct 2026: Fork from here and Edit from here started an auto-mode
    // thread's fork in the provider default.
    for (const kind of [undefined, "code", "agent"] as const) {
      expect(givenPermissionMode({ kind }, "plan", "auto")).toBe("plan");
      expect(givenPermissionMode({ kind }, undefined, "auto")).toBe("auto");
      expect(givenPermissionMode({ kind }, undefined, undefined)).toBeUndefined();
    }
    for (const kind of ["chat", "voice"] as const) {
      expect(givenPermissionMode({ kind }, "auto", "auto")).toBeUndefined();
    }
    expect(givenPermissionMode(undefined, "auto", "auto")).toBeUndefined();
  });

  it("keeps model selection while removing Chat's permission picker and status", () => {
    const summary = { engine: "claude", sessionId: "chat-settings", kind: "chat",
      model: "haiku" } as SessionSummary;
    const config = sessionConfigFor("claude#chat-settings", summary, [], [], "Claude", {}, {}, {},
      [{ engine: "claude", model: "haiku", effort: "default", permissionMode: "auto" }],
      ["default", "auto", "plan"], [], "auto", false, false);
    expect(config.canSetModel).toBe(true);
    expect(config.canSetPermission).toBe(false);
    expect(config.permissionMode).toBeUndefined();
  });
});

/* 15 Nov 2026: after a reload (and from the start for a thread the host
   started, such as a team helper) the pane said "Manual" or the mode another
   thread last chose for the same model, while the thread ran in its own mode;
   the first send then switched the thread to that saved mode. Protocol 68:
   the host says which mode each session runs in. */
describe("the mode the host runs a session in (protocol 68)", () => {
  const helper = fixture.withMode as SessionSummary;
  const manual = fixture.withDefault as SessionSummary;
  const older = fixture.withoutMode as SessionSummary;
  const savedPlan = [{ engine: "claude", model: "claude-opus", effort: "default", permissionMode: "plan" }];

  it("shows the host's mode after a reload even when another mode is saved for the model", () => {
    // A reload empties the window's own maps: nothing chosen, nothing launched.
    const config = sessionConfigFor("claude#helper", helper, [], [], "Claude", {}, {}, {},
      savedPlan, ["default", "acceptEdits", "plan", "auto"], [], "default", false, false);
    expect(config.permissionMode).toBe("acceptEdits");
    expect(sessionPermissionMode(manual, undefined, "plan")).toBe("default");
    // A mode picked in this window still wins until the host confirms it.
    expect(sessionPermissionMode(helper, "plan", "auto")).toBe("plan");
  });

  it("lets the owner pick Manual for a session the host runs in another mode", () => {
    // The helper runs in Accept edits and this window never launched it: a
    // Manual pick is a change (it was refused as "no change"), and it shows
    // at once, before the host's next registry row confirms it.
    expect(modeInForce(helper, { chosen: undefined, launched: undefined })).toBe("acceptEdits");
    expect(modeInForce(older, { chosen: undefined, launched: undefined })).toBe("default");
    expect(modeInForce(helper, { chosen: "plan", launched: "auto" })).toBe("plan");
    const config = sessionConfigFor("claude#helper", helper, [], [], "Claude", {}, {}, {},
      savedPlan, ["default", "acceptEdits", "plan", "auto"], [], "default", false, false, "default");
    expect(config.permissionMode).toBe("default");
  });

  it("never applies a saved mode before the first send of a session the host has a mode for", () => {
    for (const session of [helper, manual]) {
      expect(modeToRestore(session, { live: false, chosen: undefined, launched: undefined,
        saved: "plan" })).toBeUndefined();
    }
  });

  it("keeps the saved per-model mode as the default for a session the host said nothing about", () => {
    expect(sessionPermissionMode(older, undefined, "plan")).toBe("plan");
    expect(modeToRestore(older, { live: false, chosen: undefined, launched: undefined,
      saved: "plan" })).toBe("plan");
    expect(modeToRestore(older, { live: false, chosen: "auto", launched: undefined,
      saved: "plan" })).toBe("auto");
    expect(modeToRestore(older, { live: true, chosen: undefined, launched: undefined,
      saved: "plan" })).toBeUndefined();
    expect(modeToRestore(older, { live: false, chosen: undefined, launched: undefined,
      saved: "default" })).toBeUndefined();
    expect(modeToRestore({ ...older, kind: "chat" }, { live: false, chosen: undefined,
      launched: undefined, saved: "plan" })).toBeUndefined();
  });
});
