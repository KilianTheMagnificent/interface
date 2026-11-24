import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { EXTRA_CHANNELS } from "./bridge.js";
import {
  RENDERER_REQUEST_CHANNELS, encodeRendererPayload,
} from "./ipc-payload.js";
import { RUST_HOST_PUSH_CHANNELS, RUST_HOST_PUSH_OPERATIONS } from "./rust-host-client.js";
import {
  CHANNELS, type CreateTerminalRequest, type TerminalLaunchProfile,
  type TerminalLaunchProfileId, type TerminalLaunchRequestV1,
} from "../shared/ipc.js";

describe("Electron to Rust payload compatibility", () => {
  it("preserves the additive compact Away status request", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/project-away-status-v28.json", import.meta.url), "utf8"));
    expect(encodeRendererPayload(CHANNELS.projectAway, [fixture.request])).toEqual(fixture.request);
  });
  it("encodes project creation with the same strict payload consumed by Rust", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/project-creation-v26.json", import.meta.url), "utf8"));
    expect(encodeRendererPayload(CHANNELS.createProject, [fixture.name])).toEqual(fixture);
    expect(() => encodeRendererPayload(CHANNELS.createProject, [])).toThrow();
  });
  it("encodes the v40 Projects folder requests unchanged on the createProject channel", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/projects-folder-v40.json", import.meta.url), "utf8"));
    for (const request of fixture.requests) {
      expect(encodeRendererPayload(CHANNELS.createProject, [request])).toEqual(request);
    }
    expect(() => encodeRendererPayload(CHANNELS.createProject, [["web"]])).toThrow();
    expect(() => encodeRendererPayload(CHANNELS.createProject, [null])).toThrow();
  });
  it("encodes moved terminal folders with the strict payloads consumed by Rust", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/moved-terminal-folder-v27.json", import.meta.url), "utf8"));
    expect(encodeRendererPayload(CHANNELS.resolveMovedTerminalFolder, [fixture.resolve.path]))
      .toEqual(fixture.resolve);
    expect(encodeRendererPayload(CHANNELS.setTerminalWorkspaces,
      [fixture.save.workspaces, fixture.save.movedFolders])).toEqual(fixture.save);
    // An ordinary save stays the exact v26 payload.
    expect(encodeRendererPayload(CHANNELS.setTerminalWorkspaces, [[]])).toEqual({ workspaces: [] });
    expect(encodeRendererPayload(CHANNELS.setTerminalWorkspaces, [[], []])).toEqual({ workspaces: [] });
    expect(() => encodeRendererPayload(CHANNELS.setTerminalWorkspaces,
      [[], [{ from: "/a" }]])).toThrow();
    expect(() => encodeRendererPayload(CHANNELS.resolveMovedTerminalFolder, [])).toThrow();
  });
  it("preserves the project Away v22 fixture shared with Rust", () => {
    const fixture = JSON.parse(readFileSync(new URL("../fixtures/project-away-v22.json", import.meta.url), "utf8"));
    for (const request of fixture.commands) expect(encodeRendererPayload(CHANNELS.projectAway, [request])).toEqual(request);
  });
  it("preserves the native workspace-tool v15 fixture shared with Rust", () => {
    const requests = JSON.parse(readFileSync(new URL("../fixtures/workspace-tools-v15.json", import.meta.url), "utf8"));
    for (const request of requests) expect(encodeRendererPayload(CHANNELS.openWorkspaceTool, [request])).toEqual(request);
    expect(() => encodeRendererPayload(CHANNELS.openWorkspaceTool, ["/project"])).toThrow();
  });
  it("covers the version 72 127-request surface exactly once", () => {
    expect(RENDERER_REQUEST_CHANNELS).toHaveLength(127);
    expect(new Set(RENDERER_REQUEST_CHANNELS).size).toBe(127);
    expect(RENDERER_REQUEST_CHANNELS).toContain(CHANNELS.createSession);
    expect(RENDERER_REQUEST_CHANNELS).toContain(CHANNELS.listTerminalLaunchProfiles);
    expect(RENDERER_REQUEST_CHANNELS).toContain(CHANNELS.createTerminal);
    expect(RENDERER_REQUEST_CHANNELS).toContain(CHANNELS.startAdversarialReview);
    expect(RENDERER_REQUEST_CHANNELS).toContain(EXTRA_CHANNELS.sendLive);
    expect(RENDERER_REQUEST_CHANNELS).not.toContain(CHANNELS.event);
    expect(RENDERER_REQUEST_CHANNELS).not.toContain(EXTRA_CHANNELS.worktree);
  });

  it("splits the channel namespace into the request surface and the one push set", () => {
    expect(RUST_HOST_PUSH_CHANNELS.size).toBe(RUST_HOST_PUSH_OPERATIONS);
    const all = [...Object.values(CHANNELS), ...Object.values(EXTRA_CHANNELS)];
    expect(all.filter(channel => RUST_HOST_PUSH_CHANNELS.has(channel)).sort())
      .toEqual([...RUST_HOST_PUSH_CHANNELS].sort());
    expect(RENDERER_REQUEST_CHANNELS.length + RUST_HOST_PUSH_CHANNELS.size).toBe(all.length);
  });

  it("keeps integrated-terminal control requests as strict named objects", () => {
    const request = {
      id: "2f56fa6c-e113-4e88-bb31-6878c8cc3702",
      cwd: "/tmp/project",
      size: { columns: 100, rows: 30 },
    };
    expect(encodeRendererPayload(CHANNELS.createTerminal, [request])).toBe(request);
    expect(encodeRendererPayload(CHANNELS.dismissTerminal, [request.id]))
      .toEqual({ id: request.id });
    expect(encodeRendererPayload(CHANNELS.listTerminals, [])).toEqual({});
    expect(encodeRendererPayload(CHANNELS.pickDirectory, [])).toEqual({});
    expect(encodeRendererPayload(CHANNELS.pickDirectory, ["/work/last"]))
      .toEqual({ preferredPath: "/work/last" });
    expect(encodeRendererPayload(CHANNELS.pickDirectory, [""])).toEqual({});
    expect(() => encodeRendererPayload(CHANNELS.pickDirectory, [1]))
      .toThrow(/preferredPath/);
    expect(() => encodeRendererPayload(CHANNELS.resizeTerminal, ["not-an-object"]))
      .toThrow(/named-object/);
  });

  // The voice family is the one snake_case DTO on this wire. A camelCase slip
  // here is not a type error on either side — the host just reads undefined.
  it("keeps the voice requests on their exact snake_case field names", () => {
    expect(encodeRendererPayload(CHANNELS.voiceSetEnabled, [true, "/work/iface"]))
      .toEqual({ enabled: true, cwd: "/work/iface" });
    expect(encodeRendererPayload(CHANNELS.voiceInterrupt, [])).toEqual({});
    expect(encodeRendererPayload(CHANNELS.voiceUiResult, ["c1", false, "unknown id"]))
      .toEqual({ call_id: "c1", ok: false, output: "unknown id" });
  });

  it("matches the Rust v1 terminal-launch DTOs without reshaping them", () => {
    const profileIds: TerminalLaunchProfileId[] = [
      "shell", "claude", "codex", "grok", "qwen", "gemini",
    ];
    const profiles: TerminalLaunchProfile[] = [
      { id: "shell", label: "Shell", available: true },
      { id: "claude", engine: "claude", label: "Claude Code", available: true },
      { id: "codex", engine: "codex", label: "Codex", available: true },
      { id: "grok", engine: "acp:grok", label: "Grok", available: false,
        unavailableReason: "The Grok CLI is not installed." },
      { id: "qwen", engine: "acp:qwen", label: "Qwen", available: false },
      { id: "gemini", engine: "acp:gemini", label: "Antigravity", available: true },
    ];
    const launch: TerminalLaunchRequestV1 = { version: 1, profileId: "claude" };
    const request: CreateTerminalRequest = {
      id: "2f56fa6c-e113-4e88-bb31-6878c8cc3702",
      cwd: "/tmp/project",
      launch,
    };

    expect(profileIds).toEqual(["shell", "claude", "codex", "grok", "qwen", "gemini"]);
    expect(profiles[3]).toEqual({
      id: "grok",
      engine: "acp:grok",
      label: "Grok",
      available: false,
      unavailableReason: "The Grok CLI is not installed.",
    });
    expect(encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [])).toEqual({});
    expect(encodeRendererPayload(CHANNELS.createTerminal, [request])).toEqual({
      id: request.id,
      cwd: request.cwd,
      launch: { version: 1, profileId: "claude" },
    });
  });

  it("passes the versioned adversarial-review request to Rust without reshaping it", () => {
    const request = {
      builderEngineId: "claude" as const,
      reviewerEngineId: "codex" as const,
      canonicalCwd: "/tmp/project",
      objective: "Implement the requested fix and address the independent review.",
      clientRequestId: "review-request-1",
    };
    expect(encodeRendererPayload(CHANNELS.startAdversarialReview, [request])).toBe(request);
    expect(() => encodeRendererPayload(CHANNELS.startAdversarialReview, ["claude"]))
      .toThrow(/named-object/);
  });

  it("preserves the exact preload API while naming scalar arguments", () => {
    expect(encodeRendererPayload(CHANNELS.send, ["codex#thread-1", "hello"]))
      .toEqual({ sessionKey: "codex#thread-1", text: "hello" });
    expect(encodeRendererPayload(CHANNELS.saveFile, ["/tmp/a", "text", "rev-1"]))
      .toEqual({ path: "/tmp/a", text: "text", expectedRevision: "rev-1" });
    expect(encodeRendererPayload(CHANNELS.respond,
      ["claude#one", "approval-1", "allow", undefined, { question: "yes" }]))
      .toEqual({
        sessionKey: "claude#one",
        approvalId: "approval-1",
        verdict: "allow",
        note: undefined,
        answers: { question: "yes" },
      });
  });

  it("separates create routing from provider launch options", () => {
    expect(encodeRendererPayload(CHANNELS.createSession, [{
      engine: "codex",
      cwd: "/tmp/project",
      prompt: "hello",
      fastMode: true,
    }])).toEqual({
      engine: "codex",
      options: { cwd: "/tmp/project", prompt: "hello", fastMode: true },
    });
    expect(encodeRendererPayload(CHANNELS.createSession, [{
      cwd: "/tmp/project", prompt: "hello",
    }])).toEqual({
      engine: "claude",
      options: { cwd: "/tmp/project", prompt: "hello" },
    });
    expect(encodeRendererPayload(CHANNELS.createSession, [{
      engine: "claude",
      cwd: "/tmp/chat-home",
      prompt: "hello",
      kind: "chat",
    }])).toEqual({
      engine: "claude",
      options: { cwd: "/tmp/chat-home", prompt: "hello", kind: "chat" },
    });
    expect(() => encodeRendererPayload(CHANNELS.createSession, []))
      .toThrow("iface:createSession expected 1 argument(s), received 0.");
    for (const request of [null, [], "claude"]) {
      expect(() => encodeRendererPayload(CHANNELS.createSession, [request]))
        .toThrow("iface:createSession expected one named-object argument.");
    }
  });

  it("carries structured-clone image bytes without JSON number expansion", () => {
    const jpeg = new Uint8Array([255, 216]).buffer;
    expect(encodeRendererPayload(CHANNELS.savePastedImages,
      [[new Uint8Array([137, 80, 78, 71]), jpeg]]))
      .toEqual({
        encoding: "base64-v1",
        images: [
          { kind: "base64", data: "iVBORw==" },
          { kind: "base64", data: "/9g=" },
        ],
      });
    expect(encodeRendererPayload(CHANNELS.savePastedImages, [[[137, 80, 78, 71]]]))
      .toEqual({ encoding: "base64-v1", images: [{ kind: "invalid-request" }] });
  });

  it("rejects arity drift and unknown channels before contacting the host", () => {
    expect(() => encodeRendererPayload(CHANNELS.send, ["claude#one"]))
      .toThrow(/expected 2 or 3 argument/);
    expect(() => encodeRendererPayload("iface:notReal", []))
      .toThrow(/Unknown Interface request channel/);
  });
});
