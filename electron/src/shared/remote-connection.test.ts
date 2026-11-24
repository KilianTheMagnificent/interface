import { expect, it } from 'vitest';
import fixture from '../fixtures/remote-connection-v5.json';
import type { RemotePairingInvitationV5 } from './remote';

it('pins the v5 QR contract consumed by Rust and the native iPhone scanner', () => {
  const invitation: RemotePairingInvitationV5 = {
    ...fixture.invitation, version: 5, kind: 'interfacePairing', profile: 'production',
  };
  expect(invitation).toEqual(fixture.invitation);
  const encoded = JSON.stringify(invitation, Object.keys(invitation).sort());
  expect(fixture.qr).toBe(`interface-pair:v5:${encoded}`);
  expect(invitation).not.toHaveProperty('secret');
  expect(invitation).not.toHaveProperty('cookie');
});


it('pins the explicit v6 QR wrapper while retaining the v5 identity and transport', async () => {
  const { default: direct } = await import('../fixtures/remote-connection-v6.json');
  const request: import('./remote').RemotePairingRequestV6 = { action: 'pairDirect', version: 6 };
  const wrapper: import('./remote').RemotePairingInvitationV6 = {
    version: 6, invitation: { ...fixture.invitation, version: 5, kind: 'interfacePairing', profile: 'production' },
    nearbyOrigins: direct.invitation.nearbyOrigins,
  };
  expect(request).toEqual(direct.admin);
  expect(wrapper).toEqual(direct.invitation);
  const canonical = JSON.stringify({ invitation: JSON.parse(fixture.qr.slice('interface-pair:v5:'.length)), nearbyOrigins: wrapper.nearbyOrigins, version: 6 });
  expect(direct.qr).toBe(`interface-pair:v6:${canonical}`);
  expect(direct.qr.length).toBeLessThanOrEqual(2048);
});
