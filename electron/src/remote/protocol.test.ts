import { describe, expect, it } from "vitest";
import fixture from "../fixtures/remote-access-v1.json";
import { decodeOutput } from "./protocol";
import type { RemoteAccessStatus } from "../shared/remote";

describe("remote protocol v1", () => {
  it("decodes the Rust-shared binary fixture without moving bytes into JSON", () => {
    const data = Uint8Array.from(fixture.output.hex.match(/../g)!, byte => parseInt(byte, 16));
    const frame = decodeOutput(data.buffer);
    expect({ start: frame.start, end: frame.end, gap: frame.gap }).toEqual({ start: 5, end: 8, gap: false });
    expect(new TextDecoder().decode(frame.bytes)).toBe(fixture.output.text);
    const status: RemoteAccessStatus = { ...fixture.status, version: 1 };
    expect(status.devices[0].name).toBe("iPhone");
  });
  it("rejects truncated, wrong-version, unsafe or inconsistent frames", () => {
    const bytes = Uint8Array.from(fixture.output.hex.match(/../g)!, byte => parseInt(byte, 16));
    expect(() => decodeOutput(bytes.slice(0, 20).buffer)).toThrow();
    const corrupt = bytes.slice(); corrupt[0] = 0;
    expect(() => decodeOutput(corrupt.buffer)).toThrow();
    const length = bytes.slice(); length[12] = 9;
    expect(() => decodeOutput(length.buffer)).toThrow();
    const unsafe = bytes.slice(); new DataView(unsafe.buffer).setBigUint64(12, 2n ** 60n, true);
    expect(() => decodeOutput(unsafe.buffer)).toThrow();
  });
});

import companion from '../fixtures/remote-companion-v2.json';
import type { RemoteWorkspaceSnapshotV2, RemoteHistoryV2 } from '../shared/remote';
import { createConversationProjection } from '../renderer/conversation-projection';

it('projects the Rust-shared companion v2 fixture into the desktop conversation', () => {
  const workspace: RemoteWorkspaceSnapshotV2 = JSON.parse(JSON.stringify(companion.snapshot));
  const history: RemoteHistoryV2 = JSON.parse(JSON.stringify(companion.history));
  expect(workspace.version).toBe(2);
  const terminal = workspace.workspaces[0].terminals[0];
  expect(terminal.launchSession?.id).toBe('confirmed-provider-chat');
  expect(history.terminalId).toBe(terminal.id);
  expect(history.generation).toBe(terminal.generation);
  expect(createConversationProjection()(history.history.events)).toHaveLength(2);
});

import trustFixture from '../fixtures/remote-trust-v3.json';
import type { RemoteTrustRequest, RemoteTrustStatus } from '../shared/remote';
import approvalFixture from '../fixtures/remote-approval-v4.json';
import type { RemoteApprovalObservationV4, RemoteApprovalResponseV4, RemoteApprovalSnapshotV4 } from '../shared/remote';

it('uses the Rust approval v4 fixture and the existing bounded screen vocabulary', () => {
  const observation = approvalFixture.observation as RemoteApprovalObservationV4;
  const response = approvalFixture.response as RemoteApprovalResponseV4;
  const snapshot = approvalFixture.snapshot as RemoteApprovalSnapshotV4;
  expect(observation.version).toBe(4);
  expect(response.screen).toEqual(observation.screen);
  expect(response.cursor).toBe(observation.cursor);
  expect(response.decision).toBe('allowOnce');
  expect(snapshot.approval?.canAllowOnce).toBe(true);
});
it('shares the explicit trusted-device v3 desktop envelope with Rust and iOS', () => {
  const status: RemoteTrustStatus = { ...trustFixture.status, version: 3 };
  const request: RemoteTrustRequest = { action: 'trusted', version: 3, command: 'approve', id: status.pending[0].id };
  expect(request).toEqual(trustFixture.admin[1]);
  expect(status.pending[0].verification).toBe(trustFixture.reply.verification);
  expect(status).not.toHaveProperty('secret');
});
