export type RemoteAccessRequest =
  | { action: "status" | "disable" | "pair" }
  | { action: "enable"; origin: string }
  | { action: "revoke"; id: string };

export interface RemoteAccessStatus {
  version: 1;
  enabled: boolean;
  localUrl: string | null;
  publicUrl: string | null;
  devices: { id: string; name: string }[];
  pairingCode?: string;
}

/** HTTP v2 projections; v1 pairing, control and byte-stream contracts remain valid. */
export interface RemoteWorkspaceSnapshotV2 {
  version: 2;
  workspaces: { id: string; name: string; root: string; terminals: import('./ipc.js').TerminalSummary[] }[];
}
/**
 * Phone roster v4 (review D6): the v3 shape (projects keyed by terminal id,
 * null for "Other"), whose terminals carry the host's `attention` (desktop
 * protocol v44). Rosters v1–v3 never send it. Shared fixture:
 * `electron/src/fixtures/remote-phone-v4.json`.
 */
export interface RemoteWorkspaceSnapshotV4 {
  version: 4;
  workspaces: RemoteWorkspaceSnapshotV2['workspaces'];
  projects: Record<string, string | null>;
}

/** Phone Threads v1: the Mac's Threads, needs you first. */
export type RemoteThreadStatus = 'needsYou' | 'working' | 'idle' | 'failed';
export interface RemoteThreadV1 {
  key: string;
  engine: string;
  sessionId: string;
  title: string;
  project?: string;
  status: RemoteThreadStatus;
  section: 'pinned' | 'active' | 'snoozed' | 'settled';
  unread: boolean;
  lastActivity: string;
}
export interface RemoteThreadsV1 { version: 1; threads: RemoteThreadV1[] }
export interface RemoteThreadTurnV1 { prompt: string; reply: string; tools: number; clipped: boolean }
export interface RemoteThreadApprovalV1 { id: string; tool: string; summary: string; answerable: boolean }
export interface RemoteThreadDetailV1 {
  version: 1;
  thread: RemoteThreadV1;
  turns: RemoteThreadTurnV1[];
  olderTurns: number;
  approval?: RemoteThreadApprovalV1;
  unavailable?: string;
}
/** POST …/threads/{engine}/{sessionId}/approval, with the control lease and a new command id. */
export interface RemoteThreadApprovalResponseV1 {
  version: 1;
  approvalId: string;
  decision: 'allowOnce' | 'deny';
}
/** POST …/threads/{engine}/{sessionId}/send: a follow-up for a thread at rest, same lease and command rules. */
export interface RemoteThreadSendV1 { version: 1; text: string }

export interface RemoteHistoryV2 {
  version: 2;
  terminalId: string;
  generation: number;
  history: import('./ipc.js').HistoryPage;
}

/** Explicit v3 envelope on the retained desktop-only remoteAccess operation. */
export type RemoteTrustRequest = { action: "trusted"; version: 3 } & (
  | { command: "status" }
  | { command: "approve" | "reject" | "forget"; id: string }
);
export interface RemoteTrustStatus {
  version: 3;
  pending: { id: string; name: string; verification: string }[];
  devices: { id: string; name: string }[];
}
export type RemoteAdministrationRequest = RemoteAccessRequest | RemoteTrustRequest | RemoteConnectionRequestV5 | RemotePairingRequestV6;
export type RemoteAdministrationStatus = RemoteAccessStatus | RemoteTrustStatus | RemoteConnectionStatusV5 | RemotePairingStatusV6;

export type RemotePairingRequestV6 = { action: 'pairDirect'; version: 6 };
export interface RemotePairingInvitationV6 {
  version: 6;
  invitation: RemotePairingInvitationV5;
  nearbyOrigins: string[];
}
export interface RemotePairingStatusV6 {
  version: 6;
  invitation: RemotePairingInvitationV6;
  qrRows: string[];
}

/** HTTP-only v4 projection of a current CLI approval. No desktop IPC change. */
export interface RemoteApprovalObservationV4 {
  version: 4;
  cursor: number;
  screen: import('./ipc.js').TerminalControlScreenV1;
}
export interface RemoteApprovalResponseV4 extends RemoteApprovalObservationV4 {
  approvalId: string;
  decision: 'allowOnce' | 'deny';
}
export interface RemoteApprovalV4 {
  id: string;
  title: string;
  detail: string;
  canAllowOnce: boolean;
}
export interface RemoteApprovalSnapshotV4 { version: 4; approval: RemoteApprovalV4 | null }

/** Shared Threads/Agent attention inbox; GET /api/v2/inbox. */
export interface RemoteInboxV2 { version: 2; items: RemoteInboxItemV2[]; unavailable?: string }
export interface RemoteInboxItemV2 {
  id: string; source: "thread" | "agentThread" | "agentTask";
  title: string; status: RemoteThreadStatus; detail: string; canSend: boolean;
  agentName?: string; thread?: RemoteThreadV1;
}

/** V5 QR metadata is generated and admitted by Rust. It grants no session access. */
export interface RemotePairingInvitationV5 {
  version: 5;
  kind: 'interfacePairing';
  macId: string;
  macName: string;
  profile: 'production' | 'development';
  publicKey: string;
  invitationId: string;
  issuedAt: number;
  expiresAt: number;
  relayOrigin: string | null;
}
export type RemoteConnectionRequestV5 = { action: 'connection'; version: 5 } & (
  | { command: 'status' | 'pair' | 'disable' }
  | { command: 'enable'; relayOrigin: string | null }
);
export interface RemoteConnectionStatusV5 {
  version: 5;
  enabled: boolean;
  macId: string | null;
  relayOrigin: string | null;
  relayOnline: boolean;
  nearbyAvailable: boolean;
  invitation: RemotePairingInvitationV5 | null;
  qrRows: string[] | null;
}
