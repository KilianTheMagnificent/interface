import { describe, it, expect } from "vitest";
import type { IfaceApi } from "./ipc";
import { sessionKey } from "./engine";
import { CHANNELS, ENGINE_OPS, HISTORY_LIMIT,
  isProviderListingFailure, parseKey } from "./ipc";

describe("isProviderListingFailure", () => {
  it("does not treat an engine without a store listing as a refresh failure", () => {
    expect(isProviderListingFailure({
      engine: "acp:grok", status: "unavailable", complete: false,
      error: "Grok ACP does not expose a stored-session listing.",
    })).toBe(false);
  });

  it("flags a stale or incomplete listing that should still be explained", () => {
    expect(isProviderListingFailure({
      engine: "claude", status: "fresh", complete: true,
    })).toBe(false);
    expect(isProviderListingFailure({
      engine: "codex", status: "stale", complete: false,
    })).toBe(true);
    expect(isProviderListingFailure({
      engine: "claude", status: "fresh", complete: false,
    })).toBe(true);
  });
});

describe("parseKey", () => {
  it("splits on the first # only", () => {
    expect(parseKey("claude#ab#cd")).toEqual({ engine: "claude", sessionId: "ab#cd" });
  });
  it("round-trips a key built by sessionKey", () => {
    expect(parseKey(sessionKey("codex", "01J#x"))).toEqual({
      engine: "codex", sessionId: "01J#x" });
  });
  it("keeps acp: engine ids intact", () => {
    expect(parseKey("acp:gemini#s1")).toEqual({ engine: "acp:gemini", sessionId: "s1" });
  });
  it("throws on a key with no separator", () => {
    expect(() => parseKey("claude")).toThrow();
  });
});

describe("CHANNELS", () => {
  it("values are unique", () => {
    const values = Object.values(CHANNELS);
    expect(new Set(values).size).toBe(values.length);
  });
  // The preload builds each string from its key (it cannot import this table).
  it("names every channel after its key under iface:", () => {
    for (const [key, value] of Object.entries(CHANNELS)) {
      expect(value).toBe(`iface:${key}`);
    }
  });
  it("has a channel per IfaceApi method plus the two subscription channels", () => {
    const expected = ["backgroundControl", "customizationControl", "recoveryControl", "getAppearance", "setAppearance", "getAcpProfiles", "setAcpProfiles",
      "agentControl", "getTerminalWorkspaces", "setTerminalWorkspaces", "resolveMovedTerminalFolder",
      "getUpdateState", "checkForUpdates",
      "restartToUpdate", "updateSessions",
      "getOrchestrationSnapshot", "readOrchestrationTimeline",
      "createOrchestrationAgent", "startAdversarialReview", "sendOrchestrationMessage",
      "replyOrchestrationMessage",
      "readOrchestrationTranscript", "readOrchestrationInbox", "stopOrchestrationAgent",
      "steerOrchestrationAgent", "claimOrchestrationRole", "relinquishOrchestrationRole",
      "escrowOrchestrationWorktree",
      "setOrchestratorPolicy", "clearOrchestratorPolicy", "bindOrchestrationParent",
      "registerOrchestrationSession",
      "submitCoordinatedWorker", "integrateCoordinatedWorker", "abandonCoordinatedTask",
      "ackCoordinatedObligation", "getCoordinationStatus", "getOrchestratorModels",
      "listCollaborations", "submitCollaboration", "getCollaborationStatus",
      "approveCollaboration", "denyCollaboration", "acceptCollaborationPeer",
      "discardCollaborationPeer", "cancelCollaboration", "setCollaborationDraftState",
      "listSessions", "listSessionSnapshot", "listArchivedSessions",
      "listProviderCommands", "createSession", "resumeSession", "send", "interrupt", "closeSession",
      "forgetSession",
      "respond", "setModel", "setEffort", "setFastMode", "setPermissionMode",
      "archive", "renameSession", "deleteSession", "forkSession", "adoptSession",
      "setEngineArchived", "readUsage", "engineOps", "readHistory",
      "getDefaultDirectory", "pickDirectory", "createProject", "grantPromptContextPaths",
      "savePastedImages", "readPromptContextThumbnail",
      "readFile", "readFileSnapshot", "previewOutput", "saveFile", "setWorkspaceOpen", "listDir",
      "harnessControl", "threadControl", "threadJournal", "providerAccounts", "draftStash", "threadSecret", "inspectTerminalServers",
      "listProjectFiles", "searchProject", "searchSessions", "openTerminal", "openWorkspaceTool",
      "listTerminalLaunchProfiles", "listTerminals", "createTerminal", "renameTerminal",
      "resizeTerminal", "setTerminalWorkspaceActivity", "advanceTerminalControls",
      "restartTerminal", "closeTerminal", "dismissTerminal", "readTerminalDelegation",
      "listManagedWorktrees", "openManagedWorktree", "cleanupManagedWorktree",
      "readRepositoryReview", "discoverReviewChecks", "listCustomization", "runReviewChecks",
      "createReviewCheckpoint", "listReviewCheckpoints", "restoreReviewCheckpoint",
      "validatePromptContext",
      "probeEngines", "diagnostics",
      "remoteAccess", "voiceControl", "projectAway", "voiceSetEnabled", "voiceInterrupt", "voiceUiResult",
      "event", "registry", "updateState", "collaborationEvent", "orchestrationEvent",
      "terminalLifecycle", "voice", "voiceUiCall", "appearanceChanged", "threads"];
    const channels: Record<string, string> = CHANNELS;
    for (const name of expected) expect(typeof channels[name]).toBe("string");
    expect(Object.keys(CHANNELS).sort()).toEqual([...expected].sort());
  });

  // The lifecycle channels a second map in the main process used to spell out
  // as well (see the note above CHANNELS). Pinning the strings here is what
  // made that duplication safe to delete, and it is what keeps the deletion
  // safe: a renderer and a handler that disagree on one of these invokes a
  // channel nobody registered, and the button silently does nothing — the
  // failure mode with no error message.
  it("spells the session-lifecycle channels exactly as the handlers expect", () => {
    expect(CHANNELS.renameSession).toBe("iface:renameSession");
    expect(CHANNELS.forgetSession).toBe("iface:forgetSession");
    expect(CHANNELS.deleteSession).toBe("iface:deleteSession");
    expect(CHANNELS.forkSession).toBe("iface:forkSession");
    expect(CHANNELS.adoptSession).toBe("iface:adoptSession");
    expect(CHANNELS.setEngineArchived).toBe("iface:setEngineArchived");
    expect(CHANNELS.setPermissionMode).toBe("iface:setPermissionMode");
    expect(CHANNELS.setFastMode).toBe("iface:setFastMode");
    expect(CHANNELS.grantPromptContextPaths).toBe("iface:grantPromptContextPaths");
    expect(CHANNELS.readUsage).toBe("iface:readUsage");
    expect(CHANNELS.engineOps).toBe("iface:engineOps");
    expect(CHANNELS.listProviderCommands).toBe("iface:listProviderCommands");
  });

  it("spells the configurable ACP roster channels exactly", () => {
    expect(CHANNELS.getAcpProfiles).toBe("iface:getAcpProfiles");
    expect(CHANNELS.setAcpProfiles).toBe("iface:setAcpProfiles");
  });

  it("spells the Rust-owned terminal-launch discovery channel exactly", () => {
    expect(CHANNELS.listTerminalLaunchProfiles).toBe("iface:listTerminalLaunchProfiles");
  });

  it("spells every narrow orchestration intent and invalidation event exactly", () => {
    expect(CHANNELS.getOrchestrationSnapshot).toBe("iface:getOrchestrationSnapshot");
    expect(CHANNELS.readOrchestrationTimeline).toBe("iface:readOrchestrationTimeline");
    expect(CHANNELS.createOrchestrationAgent).toBe("iface:createOrchestrationAgent");
    expect(CHANNELS.startAdversarialReview).toBe("iface:startAdversarialReview");
    expect(CHANNELS.sendOrchestrationMessage).toBe("iface:sendOrchestrationMessage");
    expect(CHANNELS.replyOrchestrationMessage).toBe("iface:replyOrchestrationMessage");
    expect(CHANNELS.readOrchestrationTranscript).toBe("iface:readOrchestrationTranscript");
    expect(CHANNELS.readOrchestrationInbox).toBe("iface:readOrchestrationInbox");
    expect(CHANNELS.stopOrchestrationAgent).toBe("iface:stopOrchestrationAgent");
    expect(CHANNELS.steerOrchestrationAgent).toBe("iface:steerOrchestrationAgent");
    expect(CHANNELS.claimOrchestrationRole).toBe("iface:claimOrchestrationRole");
    expect(CHANNELS.relinquishOrchestrationRole).toBe("iface:relinquishOrchestrationRole");
    expect(CHANNELS.escrowOrchestrationWorktree).toBe("iface:escrowOrchestrationWorktree");
    expect(CHANNELS.orchestrationEvent).toBe("iface:orchestrationEvent");
  });

  it("spells the four orchestrator-authority channels exactly", () => {
    expect(CHANNELS.setOrchestratorPolicy).toBe("iface:setOrchestratorPolicy");
    expect(CHANNELS.clearOrchestratorPolicy).toBe("iface:clearOrchestratorPolicy");
    expect(CHANNELS.bindOrchestrationParent).toBe("iface:bindOrchestrationParent");
    expect(CHANNELS.registerOrchestrationSession).toBe("iface:registerOrchestrationSession");
  });

  it("spells the six coordinated-worker and model channels exactly", () => {
    expect(CHANNELS.submitCoordinatedWorker).toBe("iface:submitCoordinatedWorker");
    expect(CHANNELS.integrateCoordinatedWorker).toBe("iface:integrateCoordinatedWorker");
    expect(CHANNELS.abandonCoordinatedTask).toBe("iface:abandonCoordinatedTask");
    expect(CHANNELS.ackCoordinatedObligation).toBe("iface:ackCoordinatedObligation");
    expect(CHANNELS.getCoordinationStatus).toBe("iface:getCoordinationStatus");
    expect(CHANNELS.getOrchestratorModels).toBe("iface:getOrchestratorModels");
  });

  it("answers integrateCoordinatedWorker with the verifying claim, not the verify", () => {
    // Ruling R-L: the verify is asynchronous. The canvas gets the claim at once
    // and the accepted paths, receipts or refusal arrive as a worker notice.
    const claim: Awaited<ReturnType<IfaceApi["integrateCoordinatedWorker"]>> = {
      agentId: "agent_w1", taskId: "task_1", dryRun: true, state: "verifying",
    };
    expect(claim).toEqual({ agentId: "agent_w1", taskId: "task_1", dryRun: true,
      state: "verifying" });
    // @ts-expect-error the verify outcome never comes back on this channel.
    expect(claim.paths).toBeUndefined();
  });
});

// The wire names of the Rust EngineOp values (interface_contracts::ipc), not
// prettier UI words: engineOps answers with exactly these strings.
describe("ENGINE_OPS", () => {
  it("names the engine lifecycle operations", () => {
    expect([...ENGINE_OPS]).toEqual(["renameSession", "deleteSession",
      "forkSession", "setArchived", "adoptSession", "readUsage"]);
  });
  it("leaves out setPermissionMode, which is not on the adapter at all", () => {
    // It lives on the live session handle, so it cannot be feature-detected from
    // an adapter and must not be advertised as if it could.
    expect([...ENGINE_OPS]).not.toContain("setPermissionMode");
  });
});

describe("HISTORY_LIMIT", () => {
  it("is a positive whole number of messages", () => {
    expect(Number.isInteger(HISTORY_LIMIT)).toBe(true);
    expect(HISTORY_LIMIT).toBeGreaterThan(0);
  });
});
