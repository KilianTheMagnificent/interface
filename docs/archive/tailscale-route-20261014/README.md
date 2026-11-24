# Tailscale phone route archive — October 14, 2026

The owner dropped Tailscale on 2026-10-05 (it is no longer installed on the Mac;
the hosted Cloudflare relay replaced it) and on 2026-10-14 chose "Archive all
three" dormant features in the opportunity review
(https://claude.ai/artifact/V6oyGmCTdQ7mMLUVUkvUTd, item F3). Both Tailscale
routes to the phone are retired. LAN pairing (QR, nearby routes, Bonjour) and
the relay stay. This directory is historical material; re-adding a Tailscale
route requires a new owner instruction.

## What was retired

1. **The tailnet route** (phone build 11, landing `2f2e1bdd`, shipped in
   0.3.141). After each unlock the Mac's `GET /api/v1/routes` added its
   100.64.0.0/10 address (found by a UDP `connect` toward Tailscale's
   100.100.100.100) to its nearby origins; the phone saved them and named the
   route "Connected through Tailscale". The nearby listener admitted
   carrier-grade NAT peers for it.
2. **The legacy Tailscale Serve address** (pre-v5). Settings → iPhone remote
   access → "Existing Safari and Tailscale access" took a private
   `https://<mac>.<tailnet>.ts.net` origin, admitted it on the loopback
   listener, reported it as `publicUrl` and told the owner to run
   `tailscale serve`. The iPhone app's "Existing Tailscale connection"
   disclosure took the same typed address (`MacAddress.parse`).

## Source preserved

The local branch **`archive/tailscale-route-20261014`** pins the complete
pre-retirement Dev tree at **`78dc388d4c2a7f301a10fac7b5d68afb7292b915`**. Key
paths there: `crates/interface-contracts/src/remote_connection.rs`
(`mac_route_origin`), `crates/interface-remote/src/http/direct.rs`
(`mac_routes`, `nearby_peer`), `crates/interface-remote/src/http.rs`
(`public_origin`), `electron/src/renderer/components/RemoteAccessPanel.tsx`,
and `apps/interface-ios/{PairingInvitation,DirectConnection,PhoneModel,InterfaceApp}.swift`.
The old setup text is in that tree's `docs/IPHONE_REMOTE.md` ("Legacy local
preview and Tailscale setup") and `apps/interface-ios/README.md` ("Legacy
address setup").

```sh
git show archive/tailscale-route-20261014:docs/IPHONE_REMOTE.md
```

## Retirement boundaries

- **Shapes unchanged.** `/api/v1/routes` and `MacRoutesV1` (version 1) stay:
  they still carry the Mac's changed LAN addresses after unlock. A 100.64/10
  origin is simply no longer valid in them (`nearby_origin` on the Mac,
  `PairingInvitation.nearbyURL` on the phone), and the shared fixture
  `electron/src/fixtures/remote-routes-v1.json` lists such origins as invalid.
  The v6 QR wrapper never carried a tailnet route and is unchanged.
- **Desktop v1 administration kept.** `iface:remoteAccess` `{action:"enable",
  origin}` keeps its shape; an empty origin starts the loopback listener as
  before (the secure phone connection needs it), and a non-empty one is refused
  with `invalid-origin` before anything starts. `publicUrl` stays in the status
  and is always null. Settings now shows "Safari preview on this Mac" for that
  listener.
- **Your data stays, unread.** `remote-trust.v1.json` (in
  `~/Library/Application Support/Interface/` and `…/Iface Dev/`) keeps
  `listener.origin`, a required field of a frozen schema; a `.ts.net` value
  saved there is preserved but no longer served, and restore brings the
  listener back on loopback only so the phone connection behind it still
  restores. On the phone, `interface.macOrigin.v1` (the typed address) is no
  longer read; `interface.macRoutes.v1` is still read, and a saved list
  containing a tailnet origin is ignored until the next unlock replaces it.
  Keychain items and `interface.trusted.<origin>` flags for an old `.ts.net`
  origin remain until the phone forgets that connection.
- **Kept on purpose.** The agent command risk table still classifies a
  `tailscale` shell command as `risky:system`
  (`interface-desktop-services/src/agent_policy/risk.rs`): that is agent
  policy, not the phone route. The hub keeps its generic public-origin
  parameter (`RemoteHub::activate`) with its tests; nothing passes one now.
