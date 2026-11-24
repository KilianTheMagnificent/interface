import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type {
  RemoteAccessStatus, RemoteAdministrationRequest, RemoteAdministrationStatus, RemoteConnectionStatusV5,
  RemotePairingInvitationV5, RemotePairingStatusV6, RemoteTrustStatus,
} from "../../shared/remote";
import {
  RemoteAccessPanel, readRemoteAccess, remoteAccessChange, remoteInvitationRemainingMs,
} from "./RemoteAccessPanel";

const access = (over: Partial<RemoteAccessStatus> = {}): RemoteAccessStatus => ({
  version: 1, enabled: true, localUrl: "http://127.0.0.1:4100", publicUrl: null, devices: [], ...over,
});
const trust = (over: Partial<RemoteTrustStatus> = {}): RemoteTrustStatus => ({
  version: 3, pending: [{ id: "p1", name: "iPhone", verification: "4821" }], devices: [], ...over,
});
const invitation = (expiresAt: number): RemotePairingInvitationV5 => ({
  version: 5, kind: "interfacePairing", macId: "mac", macName: "Mac", profile: "development",
  publicKey: "key", invitationId: "inv", issuedAt: expiresAt - 300, expiresAt, relayOrigin: null,
});
const connection = (over: Partial<RemoteConnectionStatusV5> = {}): RemoteConnectionStatusV5 => ({
  version: 5, enabled: true, macId: "mac", relayOrigin: null, relayOnline: false, nearbyAvailable: true,
  invitation: null, qrRows: null, ...over,
});
const pairing = (): RemotePairingStatusV6 => ({
  version: 6, invitation: { version: 6, invitation: invitation(1_000), nearbyOrigins: [] }, qrRows: ["101", "010"],
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
const kindOf = (request: RemoteAdministrationRequest): string =>
  request.action === "trusted" ? "trusted" : request.action === "connection" ? "connection" : "status";

describe("remote access status reads", () => {
  it("asks for all three statuses before any answers", async () => {
    const answers = { status: deferred<RemoteAdministrationStatus>(), trusted: deferred<RemoteAdministrationStatus>(),
      connection: deferred<RemoteAdministrationStatus>() };
    const manage = vi.fn((request: RemoteAdministrationRequest) => answers[kindOf(request) as keyof typeof answers].promise);
    const read = readRemoteAccess(manage);
    expect(manage.mock.calls.map(([request]) => request)).toEqual([
      { action: "status" },
      { action: "trusted", version: 3, command: "status" },
      { action: "connection", version: 5, command: "status" },
    ]);
    answers.connection.resolve(connection());
    answers.trusted.resolve(trust());
    answers.status.resolve(access());
    await expect(read).resolves.toEqual({ status: access(), trust: trust(), connection: connection() });
  });

  it("reports the earliest read's failure even when a later read fails first", async () => {
    const status = deferred<RemoteAdministrationStatus>();
    const manage = vi.fn((request: RemoteAdministrationRequest) => {
      const kind = kindOf(request);
      if (kind === "status") return status.promise;
      return Promise.reject(new Error(`${kind} failed`));
    });
    const read = readRemoteAccess(manage);
    await Promise.resolve();
    status.reject(new Error("Remote access is off in this host."));
    await expect(read).rejects.toThrow("Remote access is off in this host.");
    const trustOnly = vi.fn(async (request: RemoteAdministrationRequest): Promise<RemoteAdministrationStatus> => {
      if (kindOf(request) === "trusted") throw new Error("Trusted devices could not be loaded.");
      return kindOf(request) === "status" ? access() : connection();
    });
    await expect(readRemoteAccess(trustOnly)).rejects.toThrow("Trusted devices could not be loaded.");
  });

  it("leaves the panel alone on unexpected versions and keeps a missing connection status apart", async () => {
    const answer = (status: RemoteAdministrationStatus, devices: RemoteAdministrationStatus,
        secure: RemoteAdministrationStatus) => vi.fn(async (request: RemoteAdministrationRequest) =>
      kindOf(request) === "status" ? status : kindOf(request) === "trusted" ? devices : secure);
    await expect(readRemoteAccess(answer(trust(), trust(), connection()))).resolves.toBeNull();
    await expect(readRemoteAccess(answer(access(), access(), connection()))).resolves.toBeNull();
    await expect(readRemoteAccess(answer(access(), trust(), pairing())))
      .resolves.toEqual({ status: access(), trust: trust(), connection: null });
  });
});

describe("remote access action answers", () => {
  it("applies each administration version to its own part of the panel", () => {
    expect(remoteAccessChange(access({ pairingCode: "ABCD-EFGH" })))
      .toEqual({ status: access({ pairingCode: "ABCD-EFGH" }), code: "ABCD-EFGH" });
    // A status without a code clears the one shown.
    expect(remoteAccessChange(access())).toEqual({ status: access(), code: "" });
    expect(remoteAccessChange(trust())).toEqual({ trust: trust() });
    const shown = connection({ invitation: invitation(1_000), qrRows: ["1"] });
    expect(remoteAccessChange(shown)).toEqual({ connection: shown, invitation: shown });
    expect(remoteAccessChange(connection())).toEqual({ connection: connection(), invitation: null });
    expect(remoteAccessChange(pairing())).toEqual({ invitation: { invitation: invitation(1_000), qrRows: ["101", "010"] } });
  });

  it("ignores an answer version it does not know", () => {
    expect(remoteAccessChange({ version: 7 } as unknown as RemoteAdministrationStatus)).toEqual({});
  });

  it("counts a pairing code down to its expiry and never below zero", () => {
    expect(remoteInvitationRemainingMs(invitation(1_000), 999_000)).toBe(1_000);
    expect(remoteInvitationRemainingMs(invitation(1_000), 1_000_000)).toBe(0);
    expect(remoteInvitationRemainingMs(invitation(1_000), 1_200_000)).toBe(0);
  });
});

describe("remote access panel", () => {
  it("offers phone access disabled until the first status read answers", () => {
    const html = renderToStaticMarkup(createElement(RemoteAccessPanel, { id: "remote", manage: vi.fn() }));
    expect(html).toContain("iPhone remote access");
    expect(html).toContain('data-voice-guard="settings"');
    expect(html).toMatch(/<button type="button" class="settings-button is-primary" disabled="">Enable secure phone connection<\/button>/u);
    // The same rows as every Settings page (owner, 2026-11-15).
    expect(html).toContain('<h3 class="settings-group-label">Phone</h3>');
    expect(html).toContain(">Safari preview on this Mac</span>");
    expect(html).not.toContain("<style>");
    expect(html).not.toContain('role="alert"');
  });
});

describe("remote access polling", () => {
  it("polls only while its Settings section is shown", async () => {
    const { readFileSync } = await import("node:fs");
    const source = readFileSync(new URL("./RemoteAccessPanel.tsx", import.meta.url), "utf8");
    const effect = source.slice(source.indexOf("if (hidden) return;"), source.indexOf("}, [manage, hidden]);"));
    expect(effect.length).toBeGreaterThan(0);
    expect(effect).toContain("startVisiblePolling(");
  });
});

describe("Tailscale route retired (2026-10-14)", () => {
  it("no longer asks for a Tailscale address or explains Tailscale Serve", async () => {
    const { readFileSync } = await import("node:fs");
    const source = readFileSync(new URL("./RemoteAccessPanel.tsx", import.meta.url), "utf8");
    expect(source).not.toMatch(/tailscale|ts\.net/iu);
    expect(source).toContain('apply({ action: "enable", origin: "" })');
    expect(source).toContain('<code className="remote-pairing-code" data-voice-redact="">{code}</code>');
  });
});
