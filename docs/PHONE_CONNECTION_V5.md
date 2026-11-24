# Interface-owned phone connection (v5)

Status: implemented and verified locally; release and hosted acceptance remain
separate steps. The existing v1–v4 connection remains available. No Internet relay
has been deployed by this work.

## Experience

1. Choose **Pair iPhone** in the Mac's remote-access settings.
2. Scan the QR code inside Interface on iPhone. It identifies this Mac and carries
   a short-lived enrollment invitation, not a reusable device credential.
3. Approve the matching phone request on the Mac. Save the phone credential behind
   the existing Keychain user-presence requirement.
4. Later opens use Face ID or the device passcode. Find the same Mac nearby with
   Bonjour; use the configured relay when no authenticated nearby connection is
   available. Display the actual route and a useful offline state.

The Mac remains the owner of production workspaces, conversation history, input
leases and approval effects. Discovery cannot switch a phone to Dev or another
Mac. Mac sleep and lack of Internet connectivity remain unavailable states.

## Protocol and trust boundary

The new connection has an explicit v5 envelope. Accepted desktop channels,
v1–v4 API semantics, terminal generations, and existing trust storage are retained.
Identity/configuration uses its own versioned, private Rust store.

QR invitations bind a random invitation ID, an expiration, a Mac identity,
the Mac public key and an optional relay origin. Invitations expire after five
minutes and are consumed by one enrollment identity. QR text is bounded and parsed
strictly; unexpected fields, URL credentials, fragments, queries and insecure
remote URLs are rejected. Invitation possession alone never approves a phone.

Bonjour `_interface._tcp` records contain only protocol version and public Mac
identity. Names and IP addresses are discovery hints, never authentication. The
scanned Mac key remains pinned across route changes and reconnects. A key change
requires explicit pairing again, with no silent fallback to another identity.

Encryption uses RFC 9180 HPKE with P-256, HKDF-SHA256 and AES-256-GCM, implemented
by Rust HPKE and Apple's CryptoKit. Contexts bind protocol, direction, identities
and request ID. Fresh response contexts authenticate the pinned Mac. Request
expiry and a bounded replay guard are checked before dispatch; a full guard fails
closed. No custom cipher, TLS bypass or plaintext session route is introduced.

The relay carries bounded encrypted packets between outbound connections. It has
no session API, filesystem access, approval authority or decryption keys. Relay
deployment requires HTTPS/WSS and an explicit allowed-Mac policy. Routing metadata
and connection timing are visible to the relay; content is encrypted end to end.

The native adapter carries the existing narrow session API. It must not expose a
general HTTP proxy or generic native/IPC bridge to WebKit. Local network and relay
paths share the same authentication, request bounds, grant expiry and revocation.
Route changes never replay writes: a lost reply/approval response remains unknown
until the existing receipt and opaque pending-command rules resolve it.

## Approved dependencies

The owner approved this list on 2026-09-11, as required by root AGENTS.md. Additions,
with exact versions and limited features:

| Package | Purpose | Features |
| --- | --- | --- |
| `hpke = 0.13.0` | RFC 9180 encryption interoperable with CryptoKit | defaults off; `alloc,p256` |
| `rand_core = 0.9.5` | OS randomness for HPKE | defaults off; `os_rng` |
| `tokio-tungstenite = 0.29.0` | Outbound relay WebSocket with certificate validation | defaults off; `connect,rustls-tls-webpki-roots` |
| `mdns-sd = 0.21.2` | Advertise the Mac through Bonjour | defaults off |
| `qrcode = 0.14.1` | Generate a QR matrix in Rust for the settings UI | defaults off |

`rand_core` and `tokio-tungstenite` already occur in the lockfile as transitive
dependencies; their new direct use/features are included in that approval. Rustls and
cryptographic/discovery transitive packages are recorded in Cargo.lock. Swift uses
system CryptoKit, Bonjour, AVFoundation, LocalAuthentication and Keychain, with
no third-party Swift packages. The resolved dependencies compile with the pinned
Rust 1.88 toolchain.

## Verification and release

Verified on 2026-09-11:

- All 19 iOS Simulator tests passed. The live fixture decodes Rust's actual QR
  matrix, discovers the Mac through Bonjour, enrolls, unlocks, loads workspaces,
  renders conversation text through the bundled WebKit/native bridge, and rejects
  the grant after locking. Vision revision 1 is used for the Simulator QR pixel
  test because the runtime could not create revision 3's ML inference context.
  Physical camera capture remains untested.
- Rust/CryptoKit HPKE vectors pass in both directions. Rust tests reject replay,
  wrong identity, tampering, absent approval, revoked grants and a relay impostor
  that knows only the public Mac key. Duplicate input does not enqueue twice.
- The real-host remote smoke passes existing reply/approval/unknown-outcome
  behavior and v5 administration, QR generation, stable identity after host
  restart, and persistent disable. All sessions and providers are disposable
  fixtures; this is not a live-provider effects test.
- iPhone 0.3.0 build 5 builds for device Release and passes strict signature
  verification. It has not been installed by this connection update.
- The minimal two-crate relay build context passes locked Cargo compilation.
  Docker/Fly container build, deployed HTTPS/WSS and physical network transitions
  remain pending.

The full `cargo xtask check` passed on the final stable source before Dev closeout
(1,488 Rust tests, 18 intentionally ignored, plus the Electron and renderer suites).
Production promotion, installed Mac/phone versions, and hosted acceptance must be
recorded separately. Recommended hosting and the exact configuration are in
[`deploy/phone-relay/README.md`](../deploy/phone-relay/README.md).

Required coverage and remaining acceptance:

- Shared Rust/TypeScript/Swift fixtures for v5 invitations and packets, including
  rejection of malformed and unsupported versions.
- Cross-language HPKE vectors in both directions, tamper/wrong-identity failures,
  expired invitations, replay refusal and bounded resource behavior.
- Disposable local relay plus Mac host plus Simulator: pairing, explicit approval,
  read/reply/approval, lock/revocation, reconnect, route change and lost-response
  handling. Test actual transport behavior, not just successful compilation.
- Required full `cargo xtask check`, native tests, existing remote smoke and Dev
  closeout. Production staging/installation are separate release evidence.
- A real hosted relay and physical iPhone Wi-Fi/cellular acceptance remain required
  before claiming that remote access works off the Mac's Wi-Fi.

## References

- [RFC 9180](https://www.rfc-editor.org/rfc/rfc9180.html)
- [Rust HPKE 0.13.0 manifest](https://docs.rs/crate/hpke/0.13.0/source/Cargo.toml.orig)
- [Apple CryptoKit HPKE](https://developer.apple.com/documentation/cryptokit/hpke)
- [Bonjour](https://developer.apple.com/bonjour/)
- [Apple local network privacy](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)


## Direct-address pairing wrapper (v6)

On networks where the Mac's numeric LAN address works but its `.local` name does
not resolve, new QR codes include nearby IPv4 routes. The desktop-only
`iface:remoteAccess` request `{ "action": "pairDirect", "version": 6 }` returns
`{ version: 6, invitation: { version: 6, invitation: <unchanged v5 invitation>,
nearbyOrigins: [...] }, qrRows: [...] }`. The QR prefix is `interface-pair:v6:`;
the body is canonical sorted JSON, bounded to 2048 bytes. The Rust, TypeScript
and Swift fixture is `electron/src/fixtures/remote-connection-v6.json`.

At most eight unique canonical `http://<private-or-link-local-IPv4>:<port>` origins
are accepted, with ports 1024–65535. Rust reads source addresses from the OS route
selection without transmitting UDP datagrams. IPv6 and public addresses are not
included. The phone probes the hints and Bonjour together (at most three hint
probes at once, losers cancelled) and tries the configured relay only after every
nearby route has failed, so a phone on the Mac's network never contacts the relay.
Each candidate is authenticated by a read-only HPKE probe of the pinned Mac before
it carries an enrollment, credential, session or command request. Only a failed
exchange resets the route; a decrypted Mac reply keeps it. Failed commands never
replay.
Hints cannot authorize enrollment or replace the Mac identity.
The nearby listener binds every IPv4 interface but answers only loopback,
private and link-local peers (carrier-grade NAT `100.64.0.0/10`, which
Tailscale assigns, stopped being admitted when that route was retired on
2026-10-14);
any other peer's connection is closed when it is accepted, before a byte of
it is read (the per-request 403 stays as a second check). A phone that can
reach the Mac only on a public address uses the relay. The listener holds at
most 32 connections, 16 from any one address, and closes any extra one at once;
a phone keeps at most 4 to one route and probes at most 4 routes while it looks
for the Mac, so one device or two behind one address always fit and no single
address can take every connection. A connection that sends nothing for 10 s
while the Mac waits for its request is closed (the wait pauses while the Mac
answers), and one whose request head has not arrived 15 s after it connected is
closed however steadily it trickles; the phone gives up on a whole request after
15 s, so this never cuts a request it still waits for. Every response carries `Connection: close`, so each
phone packet uses a fresh connection and no idle kept-alive connection can be
closed under the phone's next request. A packet takes one of the eight request
slots it shares with relay packets only after its whole body has arrived, so a
slow or stalled upload holds just its own connection.

The new non-secret `phone-listener.v1.json` sidecar retains the secure listener
port under the existing identity store's exclusive writer lease. Restart binds
that exact port; corrupt state or a port conflict refuses startup instead of
silently invalidating saved routes. The private `phone-connection.v1.json`, trust
store, v5 invitations, HPKE packets and v1–v5 administration stay unchanged.
Native `NearbyRoutesV1` preferences are separate from the existing saved Mac
identity and Keychain origin, contain no invitation or credential, and only load
for the matching Mac ID. If the LAN address changes and Bonjour is unavailable,
scan a fresh QR from the same Mac to refresh the route without changing trust.

New QR codes require iPhone 0.3.1 build 6 or newer. The updated phone still accepts
v5 QR codes and existing remembered connections. Desktop and phone rollout must
be coordinated; older iPhone scanners will reject the explicit v6 prefix.
The live native fixture test disables Bonjour and exercises QR decoding,
enrollment, unlock, workspace/history loading, rendered conversation and lock
through the numeric route. Physical Wi-Fi pairing remains a separate acceptance
check after installing both approved builds.


Verified locally on 2026-09-12 for v6: all 20 native tests passed, including the
live no-Bonjour regression and Rust QR pixel decoding. The Rust contracts/settings/
remote regressions passed, including port reuse, conflict refusal and unchanged
identity storage. The real-host phone smoke passed the v6 IPC request, strict
admission and stable port after restart. iPhone 0.3.1 build 6 was built for device
Release with the installed app's identifier and team; strict signature verification
passed. It has not been installed, and physical pairing with this build is pending.
