import { Buffer } from "node:buffer";

import { EXTRA_CHANNELS } from "./bridge.js";
import {
  RUST_HOST_PUSH_CHANNELS, RUST_HOST_PUSH_OPERATIONS, RUST_HOST_REQUEST_OPERATIONS,
} from "./rust-host-client.js";
import { CHANNELS, PROMPT_CONTEXT_VALIDATION_LIMITS } from "../shared/ipc.js";
import { PROMPT_IMAGE_LIMITS } from "../shared/prompt-images.js";

type PayloadEncoder = (args: readonly unknown[]) => unknown;

function arity(operation: string, args: readonly unknown[], expected: number): void {
  if (args.length !== expected) {
    throw new TypeError(
      `${operation} expected ${String(expected)} argument(s), received ${String(args.length)}.`,
    );
  }
}

function empty(operation: string): PayloadEncoder {
  return args => {
    arity(operation, args, 0);
    return {};
  };
}

function named(operation: string, names: readonly string[]): PayloadEncoder {
  return args => {
    arity(operation, args, names.length);
    return Object.fromEntries(names.map((name, index) => [name, args[index]]));
  };
}

function input(operation: string): PayloadEncoder {
  return args => {
    arity(operation, args, 1);
    const value = args[0];
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new TypeError(`${operation} expected one named-object argument.`);
    }
    return value;
  };
}

function createSessionPayload(args: readonly unknown[]): unknown {
  const { engine = "claude", recovery, ...options } =
    input(CHANNELS.createSession)(args) as Record<string, unknown>;
  return { engine, options, ...(recovery === undefined ? {} : { recovery }) };
}

function pastedImagePayload(args: readonly unknown[]): unknown {
  arity(CHANNELS.savePastedImages, args, 1);
  if (!Array.isArray(args[0])) {
    return { encoding: "base64-v1", images: null };
  }
  if (args[0].length > PROMPT_CONTEXT_VALIDATION_LIMITS.maxItems) {
    // The Rust service checks count before inspecting items. Send only enough
    // placeholders to preserve its item-limit result without base64-encoding
    // bytes that the frozen handler would never examine.
    return {
      encoding: "base64-v1",
      images: Array.from(
        { length: PROMPT_CONTEXT_VALIDATION_LIMITS.maxItems + 1 },
        () => ({ kind: "invalid-request" }),
      ),
    };
  }
  const images = args[0].map((image) => {
    let bytes: Uint8Array | undefined;
    if (image instanceof Uint8Array) {
      bytes = image;
    } else if (image instanceof ArrayBuffer) {
      bytes = new Uint8Array(image);
    } else {
      return { kind: "invalid-request" };
    }
    if (bytes.byteLength === 0 || bytes.byteLength > PROMPT_IMAGE_LIMITS.maxSourceBytes) {
      return { kind: "size-limit" };
    }
    return { kind: "base64", data: Buffer.from(bytes).toString("base64") };
  });
  return { encoding: "base64-v1", images };
}

const encoders = new Map<string, PayloadEncoder>();

function addEmpty(...operations: string[]): void {
  for (const operation of operations) encoders.set(operation, empty(operation));
}

function addInput(...operations: string[]): void {
  for (const operation of operations) encoders.set(operation, input(operation));
}

function addNamed(operation: string, ...names: string[]): void {
  encoders.set(operation, named(operation, names));
}

function optionalString(operation: string, name: string): PayloadEncoder {
  return args => {
    if (args.length === 0) return {};
    arity(operation, args, 1);
    const value = args[0];
    if (value === undefined || value === null || value === "") return {};
    if (typeof value !== "string") {
      throw new TypeError(`${operation} expected a string ${name}.`);
    }
    return { [name]: value };
  };
}

/** Protocol 41: an explicit refresh finds the provider CLIs again first.
 * No argument (or `refresh: false`) keeps the exact pre-v41 empty payload. */
function optionalRefresh(operation: string, flags: readonly string[] = ["refresh"]): PayloadEncoder {
  return args => {
    if (args.length === 0 || args[0] === undefined) return {};
    arity(operation, args, 1);
    const options = args[0];
    if (typeof options !== "object" || options === null || Array.isArray(options)
        || Object.keys(options).some(key => !flags.includes(key))) {
      throw new TypeError(`${operation} expected {${flags.join(", ")}}.`);
    }
    const payload: Record<string, true> = {};
    for (const flag of flags) {
      const value = (options as Record<string, unknown>)[flag];
      if (value !== undefined && typeof value !== "boolean") {
        throw new TypeError(`${operation} expected a boolean ${flag}.`);
      }
      if (value === true) payload[flag] = true;
    }
    return payload;
  };
}

/** `listTerminalLaunchProfiles`: the protocol 41/43 flags, plus protocol 48's
 * `paneTools: {profileId, enabled}`. */
function launchProfilesPayload(args: readonly unknown[]): Record<string, unknown> {
  const operation = CHANNELS.listTerminalLaunchProfiles;
  const options = args[0];
  if (args.length !== 1 || typeof options !== "object" || options === null
      || Array.isArray(options) || !("paneTools" in options)) {
    return optionalRefresh(operation, ["refresh", "details"])(args) as Record<string, unknown>;
  }
  const { paneTools, ...flags } = options as Record<string, unknown>;
  const payload = optionalRefresh(operation, ["refresh", "details"])([flags]) as Record<string, unknown>;
  if (paneTools === undefined) return payload;
  if (typeof paneTools !== "object" || paneTools === null || Array.isArray(paneTools)
      || Object.keys(paneTools).some(key => key !== "profileId" && key !== "enabled")) {
    throw new TypeError(`${operation} expected paneTools {profileId, enabled}.`);
  }
  const { profileId, enabled } = paneTools as { profileId?: unknown; enabled?: unknown };
  if (typeof profileId !== "string" || profileId === "" || typeof enabled !== "boolean") {
    throw new TypeError(`${operation} expected paneTools {profileId, enabled}.`);
  }
  payload.paneTools = { profileId, enabled };
  return payload;
}

/** A roster save, optionally carrying owner-confirmed folder moves. No moves
 * keeps the exact pre-v27 payload. */
function terminalWorkspacesPayload(args: readonly unknown[]): Record<string, unknown> {
  const operation = CHANNELS.setTerminalWorkspaces;
  if (args.length !== 1 && args.length !== 2) arity(operation, args, 1);
  const [workspaces, movedFolders] = args;
  if (movedFolders === undefined || (Array.isArray(movedFolders) && movedFolders.length === 0)) {
    return { workspaces };
  }
  if (!Array.isArray(movedFolders) || !movedFolders.every(moved =>
    typeof moved === "object" && moved !== null
      && typeof (moved as { from?: unknown }).from === "string"
      && typeof (moved as { to?: unknown }).to === "string"
      && Object.keys(moved).length === 2)) {
    throw new TypeError(`${operation} expected {from, to} folder moves.`);
  }
  return { workspaces, movedFolders };
}

addEmpty(
  CHANNELS.getAppearance,
  CHANNELS.getAcpProfiles,
  CHANNELS.getTerminalWorkspaces,
  CHANNELS.getUpdateState,
  CHANNELS.checkForUpdates,
  CHANNELS.restartToUpdate,
  CHANNELS.getOrchestrationSnapshot,
  CHANNELS.getOrchestratorModels,
  CHANNELS.listCollaborations,
  CHANNELS.listSessions,
  CHANNELS.listSessionSnapshot,
  CHANNELS.listArchivedSessions,
  CHANNELS.readUsage,
  CHANNELS.engineOps,
  CHANNELS.getDefaultDirectory,
  CHANNELS.listManagedWorktrees,
  CHANNELS.voiceInterrupt,
  CHANNELS.listTerminals,
);
encoders.set(CHANNELS.probeEngines, optionalRefresh(CHANNELS.probeEngines));
// Protocol 42 adds `details` (each found CLI's version) for Settings' Terminal tools;
// protocol 48 adds `paneTools`, Settings' switch for Interface's tools in a CLI's config.
encoders.set(CHANNELS.listTerminalLaunchProfiles, launchProfilesPayload);
encoders.set(CHANNELS.pickDirectory, optionalString(CHANNELS.pickDirectory, "preferredPath"));
// v1 creates by name; from protocol 40 a named object is a Projects folder request.
encoders.set(CHANNELS.createProject, args => typeof args[0] === "string"
  ? named(CHANNELS.createProject, ["name"])(args)
  : input(CHANNELS.createProject)(args));

addInput(
  CHANNELS.updateSessions,
  CHANNELS.remoteAccess,
  CHANNELS.agentControl,
  CHANNELS.harnessControl,
  CHANNELS.threadControl,
  CHANNELS.threadJournal,
  CHANNELS.providerAccounts,
  CHANNELS.draftStash,
  CHANNELS.threadSecret,
  CHANNELS.voiceControl,
  CHANNELS.projectAway,
  CHANNELS.inspectTerminalServers,
  CHANNELS.readOrchestrationTimeline,
  CHANNELS.createOrchestrationAgent,
  CHANNELS.startAdversarialReview,
  CHANNELS.sendOrchestrationMessage,
  CHANNELS.replyOrchestrationMessage,
  CHANNELS.readOrchestrationTranscript,
  CHANNELS.readOrchestrationInbox,
  CHANNELS.stopOrchestrationAgent,
  CHANNELS.steerOrchestrationAgent,
  CHANNELS.claimOrchestrationRole,
  CHANNELS.relinquishOrchestrationRole,
  CHANNELS.escrowOrchestrationWorktree,
  CHANNELS.setOrchestratorPolicy,
  CHANNELS.clearOrchestratorPolicy,
  CHANNELS.bindOrchestrationParent,
  CHANNELS.registerOrchestrationSession,
  CHANNELS.submitCoordinatedWorker,
  CHANNELS.integrateCoordinatedWorker,
  CHANNELS.abandonCoordinatedTask,
  CHANNELS.ackCoordinatedObligation,
  CHANNELS.getCoordinationStatus,
  CHANNELS.submitCollaboration,
  CHANNELS.approveCollaboration,
  CHANNELS.denyCollaboration,
  CHANNELS.acceptCollaborationPeer,
  CHANNELS.discardCollaborationPeer,
  CHANNELS.setCollaborationDraftState,
  CHANNELS.listProviderCommands,
  CHANNELS.previewOutput,
  CHANNELS.diagnostics,
  CHANNELS.openWorkspaceTool,
  CHANNELS.readTerminalDelegation,
  CHANNELS.advanceTerminalControls,
  CHANNELS.createTerminal,
  CHANNELS.renameTerminal,
  CHANNELS.resizeTerminal,
  CHANNELS.setTerminalWorkspaceActivity,
  CHANNELS.restartTerminal,
  CHANNELS.closeTerminal,
);

addNamed(CHANNELS.setAppearance, "preference");
addNamed(CHANNELS.setAcpProfiles, "profiles");
encoders.set(CHANNELS.setTerminalWorkspaces, terminalWorkspacesPayload);
addNamed(CHANNELS.resolveMovedTerminalFolder, "path");
addNamed(CHANNELS.getCollaborationStatus, "collaborationId");
addNamed(CHANNELS.cancelCollaboration, "collaborationId");
encoders.set(CHANNELS.createSession, createSessionPayload);
addNamed(CHANNELS.resumeSession, "sessionKey");
encoders.set(CHANNELS.send, args => {
  if (args.length !== 2 && args.length !== 3) throw new TypeError("iface:send expected 2 or 3 arguments.");
  return { sessionKey: args[0], text: args[1], ...(args[2] === undefined ? {} : { recovery: args[2] }) };
});
addInput(CHANNELS.recoveryControl);
addNamed(CHANNELS.interrupt, "sessionKey");
addNamed(CHANNELS.closeSession, "sessionKey");
addNamed(CHANNELS.forgetSession, "sessionKey");
addNamed(CHANNELS.respond, "sessionKey", "approvalId", "verdict", "note", "answers");
addNamed(CHANNELS.setModel, "sessionKey", "model");
addNamed(CHANNELS.setEffort, "sessionKey", "effort");
addNamed(CHANNELS.setFastMode, "sessionKey", "enabled");
addNamed(CHANNELS.setPermissionMode, "sessionKey", "mode");
addNamed(CHANNELS.archive, "sessionKey", "archived");
addNamed(CHANNELS.renameSession, "sessionKey", "title");
addNamed(CHANNELS.deleteSession, "sessionKey");
addNamed(CHANNELS.forkSession, "sessionKey");
addNamed(CHANNELS.adoptSession, "sessionKey", "adopt");
addNamed(CHANNELS.setEngineArchived, "sessionKey", "archived");
addNamed(CHANNELS.readHistory, "sessionKey", "options");
addNamed(CHANNELS.grantPromptContextPaths, "paths");
encoders.set(CHANNELS.savePastedImages, pastedImagePayload);
addNamed(CHANNELS.readPromptContextThumbnail, "path");
addNamed(CHANNELS.readFile, "path");
addNamed(CHANNELS.readFileSnapshot, "path");
addNamed(CHANNELS.saveFile, "path", "text", "expectedRevision");
addNamed(CHANNELS.setWorkspaceOpen, "root", "open");
addNamed(CHANNELS.listDir, "path");
addNamed(CHANNELS.listProjectFiles, "root");
addNamed(CHANNELS.searchProject, "root", "query");
addNamed(CHANNELS.searchSessions, "query");
addNamed(CHANNELS.openTerminal, "root");
addNamed(CHANNELS.dismissTerminal, "id");
addNamed(CHANNELS.openManagedWorktree, "sessionKey");
addNamed(CHANNELS.cleanupManagedWorktree, "sessionKey");
addNamed(CHANNELS.readRepositoryReview, "root");
addNamed(CHANNELS.discoverReviewChecks, "root");
addNamed(CHANNELS.listCustomization, "root");
addInput(CHANNELS.backgroundControl, CHANNELS.customizationControl);
addNamed(CHANNELS.runReviewChecks, "root", "selected");
addNamed(CHANNELS.createReviewCheckpoint, "root", "kind");
addNamed(CHANNELS.listReviewCheckpoints, "root");
addNamed(CHANNELS.restoreReviewCheckpoint, "root", "id", "paths");
addNamed(CHANNELS.validatePromptContext, "target", "items");
// The voice DTOs are snake_case on the wire; see the note in shared/ipc.ts.
addNamed(CHANNELS.voiceSetEnabled, "enabled", "cwd");
addNamed(CHANNELS.voiceUiResult, "call_id", "ok", "output");
addNamed(EXTRA_CHANNELS.adoptAll, "adopt");
addNamed(EXTRA_CHANNELS.sendLive, "sessionKey", "text");

export const RENDERER_REQUEST_CHANNELS = Object.freeze([
  ...Object.values(CHANNELS),
  ...Object.values(EXTRA_CHANNELS),
].filter(channel => !RUST_HOST_PUSH_CHANNELS.has(channel)));
const rendererRequestChannelSet = new Set<string>(RENDERER_REQUEST_CHANNELS);

if (RENDERER_REQUEST_CHANNELS.length !== RUST_HOST_REQUEST_OPERATIONS
    || rendererRequestChannelSet.size !== RENDERER_REQUEST_CHANNELS.length) {
  throw new Error("The Electron request surface is not exactly "
    + `${RUST_HOST_REQUEST_OPERATIONS} unique channels.`);
}
if (RUST_HOST_PUSH_CHANNELS.size !== RUST_HOST_PUSH_OPERATIONS) {
  throw new Error(`The Electron push surface is not exactly ${RUST_HOST_PUSH_OPERATIONS} channels.`);
}
for (const operation of RENDERER_REQUEST_CHANNELS) {
  if (!encoders.has(operation)) {
    throw new Error(`Missing Rust payload encoder for ${operation}.`);
  }
}
for (const operation of encoders.keys()) {
  if (!rendererRequestChannelSet.has(operation)) {
    throw new Error(`Payload encoder is not part of the frozen surface: ${operation}.`);
  }
}

export function encodeRendererPayload(operation: string, args: readonly unknown[]): unknown {
  const encoder = encoders.get(operation);
  if (encoder === undefined) throw new TypeError(`Unknown Interface request channel: ${operation}.`);
  return encoder(args);
}
