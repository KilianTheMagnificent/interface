import { useEffect, useState } from "react";
import type { RemoteAdministrationRequest, RemoteAdministrationStatus, RemoteAccessStatus, RemoteTrustStatus, RemoteConnectionStatusV5 } from "../../shared/remote";
import { equalValue } from "../stable-values";
import { withoutIpcErrorShape } from "../ipc-message";
import { startVisiblePolling } from "../visible-polling";
import { productLabel } from "../product";
import { SettingsGroup, SettingsRow, useSettingsDisclosure } from "./SettingsControls";

type RemoteManage = (request: RemoteAdministrationRequest) => Promise<RemoteAdministrationStatus>;
type RemoteInvitation = Pick<RemoteConnectionStatusV5, 'invitation' | 'qrRows'>;

/** How often the open Settings window re-reads remote access; paused while the window is hidden. */
export const REMOTE_ACCESS_POLL_MS = 3000;

/**
 * The three status reads behind the panel, asked together. A refusal throws
 * the first failure in the order the reads were once made one after another,
 * so the same error reaches the owner. Null when the answers are not the
 * versions asked for, which leaves the panel as it was.
 */
export async function readRemoteAccess(manage: RemoteManage): Promise<{
  status: RemoteAccessStatus; trust: RemoteTrustStatus; connection: RemoteConnectionStatusV5 | null;
} | null> {
  const reads = await Promise.allSettled([
    manage({ action: "status" }),
    manage({ action: "trusted", version: 3, command: "status" }),
    manage({ action: 'connection', version: 5, command: 'status' }),
  ]);
  const answers: RemoteAdministrationStatus[] = [];
  for (const read of reads) {
    if (read.status === "rejected") throw read.reason;
    answers.push(read.value);
  }
  const [status, trust, connection] = answers;
  if (status.version !== 1 || trust.version !== 3) return null;
  return { status, trust, connection: connection.version === 5 ? connection : null };
}

/** What one answer to an owner's action changes on the panel. Unknown versions change nothing. */
export function remoteAccessChange(next: RemoteAdministrationStatus): {
  status?: RemoteAccessStatus; code?: string; trust?: RemoteTrustStatus;
  connection?: RemoteConnectionStatusV5; invitation?: RemoteInvitation | null;
} {
  switch (next.version) {
    case 1: return { status: next, code: next.pairingCode ?? "" };
    case 3: return { trust: next };
    case 5: return { connection: next, invitation: next.invitation ? next : null };
    case 6: return { invitation: { invitation: next.invitation.invitation, qrRows: next.qrRows } };
    default: return {};
  }
}

/** Milliseconds until a shown pairing QR code expires; zero once it has. */
export function remoteInvitationRemainingMs(invitation: NonNullable<RemoteInvitation['invitation']>, nowMs: number): number {
  return Math.max(0, invitation.expiresAt * 1000 - nowMs);
}

/** The value already on screen while an answer repeats it, so a poll that changes nothing draws nothing. */
const keep = <T,>(current: T, next: T): T => equalValue(current, next) ? current : next;

export function RemoteAccessPanel({ id, manage, hidden = false }: { id: string; manage: (request: RemoteAdministrationRequest) => Promise<RemoteAdministrationStatus>; hidden?: boolean }) {
  const [status, setStatus] = useState<RemoteAccessStatus | null>(null);
  const [trust, setTrust] = useState<RemoteTrustStatus | null>(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const [connection, setConnection] = useState<RemoteConnectionStatusV5 | null>(null);
  const [invitation, setInvitation] = useState<Pick<RemoteConnectionStatusV5, 'invitation' | 'qrRows'> | null>(null);
  const [relay, setRelay] = useState('');
  const relayDetails = useSettingsDisclosure();
  useEffect(() => {
    if (!invitation?.invitation) return;
    const timer = setTimeout(() => setInvitation(null), remoteInvitationRemainingMs(invitation.invitation, Date.now()));
    return () => clearTimeout(timer);
  }, [invitation]);
  useEffect(() => { if (!code) return; const timer = setTimeout(() => setCode(""), 5 * 60 * 1000); return () => clearTimeout(timer); }, [code]);
  // Polled only while its Settings section is shown: the panel stays mounted
  // (its pairing code and invitation survive a section switch), the poll does
  // not. Showing the section again reads at once.
  useEffect(() => {
    if (hidden) return;
    let stopped = false;
    const polling = startVisiblePolling(async () => {
      try {
        const next = await readRemoteAccess(manage);
        if (stopped || next === null) return;
        setStatus(current => keep(current, next.status)); setTrust(current => keep(current, next.trust));
        const secure = next.connection;
        if (secure !== null) { setConnection(current => keep(current, secure)); if (!secure.enabled) setInvitation(null); }
      } catch (error) { if (!stopped) setError(error instanceof Error ? withoutIpcErrorShape(error.message) : "Remote access is unavailable."); }
    }, REMOTE_ACCESS_POLL_MS);
    return () => { stopped = true; polling(); };
  }, [manage, hidden]);
  const apply = async (request: RemoteAdministrationRequest) => {
    setPending(true); setError(""); setCode("");
    try {
      const change = remoteAccessChange(await manage(request));
      if (change.status !== undefined) setStatus(change.status);
      if (change.code !== undefined) setCode(change.code);
      if (change.trust !== undefined) setTrust(change.trust);
      if (change.connection !== undefined) setConnection(change.connection);
      if (change.invitation !== undefined) setInvitation(change.invitation);
    }
    catch (error) { setError(error instanceof Error ? withoutIpcErrorShape(error.message) : "Remote access could not be changed."); }
    finally { setPending(false); }
  };
  const pendingDevices = trust?.pending ?? [];
  const safariDevices = status?.devices.filter(device => !trust?.devices.some(d => d.id === device.id)) ?? [];
  return <section id={id} className="settings-section remote-settings" data-voice-guard="settings" hidden={hidden} aria-labelledby={`${id}-title`}>
    <div className="settings-section-heading">
      <div>
        <h2 className="settings-section-title" id={`${id}-title`}>iPhone remote access</h2>
        <p className="settings-section-copy">
          Your sessions on your iPhone. Scan this Mac once, approve your phone, then unlock with Face ID.
        </p>
      </div>
    </div>

    <SettingsGroup label="Phone">
      {!connection?.enabled ? <>
        <SettingsRow labelId={`${id}-phone`} label="Secure phone connection"
          description="Encrypted from phone to Mac, on the same Wi-Fi or through your relay.">
          <button type="button" className="settings-button is-primary" disabled={pending || !connection}
            onClick={() => { void apply({ action: 'connection', version: 5, command: 'enable', relayOrigin: relay.trim() || null }); }}>Enable secure phone connection</button>
        </SettingsRow>
        <SettingsRow labelId={`${id}-relay`} label="Relay address"
          description="Optional. Lets your phone reach this Mac away from its Wi-Fi.">
          <input className="settings-input" aria-label="Relay HTTPS address" value={relay}
            onChange={e => setRelay(e.target.value)} placeholder="https://your-interface-relay.fly.dev"
            autoComplete="off" spellCheck={false} />
        </SettingsRow>
      </> : <>
        <SettingsRow labelId={`${id}-phone`} label="Secure phone connection"
          description={`${connection.nearbyAvailable ? 'Nearby discovery is on.' : 'Use a fresh pairing code for nearby access.'} ${connection.relayOrigin ? connection.relayOnline ? 'Relay connected.' : 'Relay is offline. You can still connect on the same Wi-Fi.' : 'Add a relay for access away from this Wi-Fi.'}`}>
          <button type="button" className="settings-button" disabled={pending}
            onClick={() => { void apply({ action: 'connection', version: 5, command: 'disable' }); }}>Turn off phone access</button>
          <button type="button" className="settings-button is-primary" disabled={pending}
            onClick={() => { void apply({ action: 'pairDirect', version: 6 }); }}>Pair iPhone</button>
        </SettingsRow>
        {invitation?.qrRows && <div className="remote-qr">
          <svg role="img" aria-label="Scan this QR code in Interface on iPhone" viewBox={`-4 -4 ${invitation.qrRows.length + 8} ${invitation.qrRows.length + 8}`} style={{ width: 240, maxWidth: '100%', background: '#fff', border: '1px solid var(--border)', borderRadius: 12, padding: 12 }} shapeRendering="crispEdges">
            <path fill="#000" d={invitation.qrRows.flatMap((row, y) => [...row].flatMap((cell, x) => cell === '1' ? [`M${x} ${y}h1v1h-1z`] : [])).join('')} />
          </svg>
          <p className="settings-foot">In Interface 0.3.1 or later on iPhone, choose Scan Mac QR code. This code expires in five minutes. Approve the matching verification number below.</p>
        </div>}
      </>}
      {pendingDevices.map(device => <SettingsRow key={device.id} labelId={`${id}-pending-${device.id}`}
        label={device.name}
        description={<>Approve only if this number matches your iPhone: <code className="remote-verification">{device.verification}</code></>}>
        <button type="button" className="settings-button" disabled={pending}
          onClick={() => { void apply({ action: 'trusted', version: 3, command: 'reject', id: device.id }); }}>Decline</button>
        <button type="button" className="settings-button is-primary" disabled={pending}
          onClick={() => { void apply({ action: 'trusted', version: 3, command: 'approve', id: device.id }); }}>Approve iPhone {device.verification}</button>
      </SettingsRow>)}
    </SettingsGroup>

    {!!trust?.devices.length && <SettingsGroup label="Trusted iPhones">
      {trust.devices.map(device => <SettingsRow key={device.id} labelId={`${id}-trusted-${device.id}`}
        label={device.name} description="Can open your sessions after Face ID.">
        <button type="button" className="settings-button" disabled={pending}
          onClick={() => { void apply({ action: "trusted", version: 3, command: "forget", id: device.id }); }}>Forget iPhone</button>
      </SettingsRow>)}
    </SettingsGroup>}

    <SettingsGroup label="On this Mac">
      <SettingsRow labelId={`${id}-safari`} label="Safari preview on this Mac"
        description={status?.enabled ? <>Open <code>{status.localUrl}</code> in Safari on this Mac. {productLabel()} must stay open.</>
          : `Open your sessions in Safari on this Mac. ${productLabel()} must stay open.`}>
        {!status?.enabled
          ? <button type="button" className="settings-button" disabled={pending || !status}
            onClick={() => { void apply({ action: "enable", origin: "" }); }}>Start the Safari preview</button>
          : <>
            <button type="button" className="settings-button" disabled={pending}
              onClick={() => { void apply({ action: "disable" }); }}>Turn off access</button>
            <button type="button" className="settings-button" disabled={pending}
              onClick={() => { void apply({ action: "pair" }); }}>Pair Safari with a code</button>
          </>}
      </SettingsRow>
      {status?.enabled && code && <div className="remote-code">
        <code className="remote-pairing-code" data-voice-redact="">{code}</code>
        <p className="settings-foot">Enter this code in Safari within five minutes. It works once.</p>
      </div>}
      {status?.enabled && safariDevices.map(device => <SettingsRow key={device.id} labelId={`${id}-safari-${device.id}`}
        label={device.name} description="Paired with the Safari preview.">
        <button type="button" className="settings-button" disabled={pending}
          onClick={() => { void apply({ action: "revoke", id: device.id }); }}>Revoke access</button>
      </SettingsRow>)}
      {connection?.macId && <SettingsRow labelId={`${id}-relay-setup`} label="Relay setup"
        description={<>What your relay needs to allow this Mac. {relayDetails.link("Show", "Hide")}</>}
        detail={relayDetails.panel(<>
          <span className="settings-row-more-line">This Mac&rsquo;s public identity: <code>{connection.macId}</code></span>
          <span className="settings-row-more-line">Relay: {connection.relayOrigin === null
            ? "No relay configured" : <code>{connection.relayOrigin}</code>}</span>
        </>)} />}
    </SettingsGroup>
    {error && <p className="settings-error" role="alert">{error}</p>}
  </section>;
}
