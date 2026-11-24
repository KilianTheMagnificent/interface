# iPhone remote access and iOS companion

Interface has a native iPhone companion and a Safari/home-screen companion for
existing native terminal sessions. Both show saved workspace groups and reuse the
desktop conversation view for confirmed Claude and Codex sessions. Open **Settings → iPhone remote access** on the Mac to enable it.
The Mac remains the owner of the session and must stay awake with Interface open.
This preview does not include structured Chat sessions, delegated worker views,
new-session creation, push notifications or App Store distribution. Threads reach
the phone through the read-mostly Threads v1 routes below. The native Xcode
project and Simulator instructions are in [apps/interface-ios](../apps/interface-ios/README.md).
The native app opens a refined conversation reader, with a message composer and
single-use cards for recognized Claude/Codex permission prompts. It has no terminal
toggle. Other prompts and menus remain on the Mac. The Safari companion retains
its existing terminal controls.

## Interface-owned connection v5

The native iPhone app can pair with a QR from **Settings → iPhone remote access →
Enable secure phone connection → Pair iPhone**. Scan it in the app and confirm the
production/Dev identity, then approve the matching verification number on the Mac.
The QR expires after five minutes and admits one enrollment identity. Later opens
use the existing Face ID/passcode-protected device credential.

The phone discovers `_interface._tcp` on the local network, then proves the pinned
Mac identity through an encrypted probe. A configured HTTPS relay provides access
from cellular or other Wi-Fi networks. Discovery names and relay routing cannot
select a different Mac or switch between production and Dev. The Mac must remain
awake with Interface running. See [connection design](PHONE_CONNECTION_V5.md) and
the [personal relay deployment](../deploy/phone-relay/README.md).

V5 wraps the retained v1–v4 session operations in authenticated RFC 9180 HPKE
(P-256/HKDF-SHA256/AES-256-GCM). The additional LAN listener exposes only encrypted
packets. It closes connections from outside the local networks before reading
them, holds at most 32 connections and 16 from one address, closes one that stays
silent for 10 s or has not sent its request head within 15 s, and
answers every request with `Connection: close`; see the
[connection design](PHONE_CONNECTION_V5.md). The relay has an explicit public-Mac allowlist, verifies key possession,
and forwards ciphertext over the Mac's outbound WSS connection. It has no session
or approval API. Grants, leases, idempotency and provider effects remain Rust-owned.
Packets expire within 90 seconds; authenticated request IDs are retained until
expiry in a bounded replay guard. Capacity failure never evicts a live ID. A route
change never resends a write whose outcome is unknown.

The iPhone bundles the shared conversation assets in the signed app. Its WebKit
scheme serves only those fixed files and permits no network fetches. A main-frame,
selected-terminal bridge carries bounded encrypted requests; it cannot call
enrollment/unlock, desktop IPC, arbitrary URLs or the filesystem. Credential and
session-cookie values stay native. Durable opaque command markers continue to
guard uncertain replies and approvals, including across app/view replacement.

Desktop-only `remoteAccess` accepts an explicit `action: connection, version: 5`
envelope for status, enable, disable and pairing. This adds no desktop channel and
does not change v1/v3 request bytes. The new private `phone-connection.v1.json`
holds the Mac identity and opted-in relay settings under a separate writer lease;
the existing trust/workspace stores are unchanged. Turn off phone access revokes
current grants and stops both transports. Ordinary app shutdown retains opt-in.

The hosted relay is a separate deployment. Source, Simulator and loopback relay
tests do not establish that a public relay or physical cellular connection is live.

## Phone approval protocol v4

The authenticated POST routes `/api/v4/terminals/{id}/{generation}/approval` and
`/approval/respond` accept bounded rendered-screen observations from the phone's
transport emulator. Rust recognizes standard complete permission dialogs and
projects their exact details. The response requires the same terminal/generation,
screen digest and latest output cursor, plus the existing device grant, control
lease and idempotent command receipt. Any desktop input, new output or prior
response invalidates that observation. Ambiguous delivery stays blocked across
app/view replacement using the existing native opaque input marker.
Because only the latest cursor is answered, the iPhone app's polled output sends
an observation once a read comes back empty (`NativeOutputSocket.onsettled`),
not after every page while output flows; the WebSocket path sends one 180 ms
after its last frame.

This is a conservative bridge for existing CLI sessions, not a provider request-ID
API. A CLI can change its screen before its next output is observed; unknown,
clipped or custom layouts stay on Mac. Only one-use approval is offered: Codex
requires the printed `(y)` shortcut and Claude requires the selected single-use
Yes choice. No persistent permission choice is exposed. The parser follows
[Codex's approval UI](https://github.com/openai/codex/blob/main/codex-rs/tui/src/bottom_pane/approval_overlay.rs)
and [Claude's dialog controls](https://code.claude.com/docs/en/interactive-mode).
Provider-native request IDs are a future integration, not a claim of this bridge.

## Safari preview on this Mac

**Settings → iPhone remote access → Safari preview on this Mac** starts the
loopback listener and shows `http://127.0.0.1:PORT`. Open it in Safari on the
Mac and choose **Pair Safari with a code**. The loopback address cannot be
opened from an iPhone: phones pair by QR code and connect nearby or through the
relay. The Tailscale route (a typed `.ts.net` address forwarded with Tailscale
Serve, and the Mac's tailnet address sent after unlock) was retired on
2026-10-14; see [the archive](archive/tailscale-route-20261014/README.md).

### Use production panes

Interface Dev and Interface each own separate workspaces, remote listeners and
trusted-device records. A phone pairs with the instance whose QR code it
scanned; publishing a Mac update does not change that.

After installing a production version, open **Interface → Settings → iPhone
remote access**, enable the secure phone connection and pair again. On the
phone, unlock and choose **Request Mac approval again** if its previous Dev
connection is rejected. Approve the matching request in production Interface.
Existing panes stay in their original Mac profile; no workspace or trust files
need to be copied. Staging an update and installing/restarting it are separate
release steps.

Select a session, tap **Take control**, and send text using the composer. Sending
text adds Enter; dedicated Esc, Tab, arrow, Enter and Ctrl-C keys are available.
The terminal retains its desktop dimensions; pan horizontally on a narrow phone.
Desktop typing takes control back. A phone lease expires after 30 seconds, and
leaving the phone app releases it. Reconnecting never resends a command.

Enabled access and its loopback port survive Mac app restarts. Turning access off
revokes active sessions and persists the disabled state, while retaining trusted
iPhones for explicit revocation. **Forget iPhone** removes its durable trust and
current session. Browser grants remain temporary: twelve hours or until the host
restarts. Safari pairing codes last five minutes, work once and allow five attempts.
Native session grants last fifteen minutes and require a fresh local unlock when
expired or after backgrounding. No public internet exposure is needed.

## Implementation contract

Desktop protocol v18 retains `iface:remoteAccess` (109 requests, 10 pushes,
119 channels). Administration never appears on HTTP. Its new explicit v3 `trusted` envelope
accepts status, approve, reject and forget commands; legacy v1 requests retain their
exact shape. Existing canonical stores and terminal data protocol v2 are unchanged. Axum is pinned at the explicitly
approved 0.8.9 with default features off and `http1,json,query,tokio,ws` enabled.
Rust owns authentication, leases, receipts, session selection and all effects;
React and xterm only render state and submit explicit commands.

The legacy remote service listens only on IPv4 loopback. It admits only the exact
local authority; mutations and WebSocket upgrades require a matching Origin. A
`.ts.net` address saved before the Tailscale route was retired stays in
`remote-trust.v1.json` (`listener.origin`, a frozen field) but is no longer
served, and `Enable` with a non-empty origin is refused as `invalid-origin`. Pairing exchanges a one-time code for an HttpOnly,
SameSite=Strict cookie (Secure on the HTTPS origin). Only hashes of browser grants and pairing codes are retained in host memory.
Native device credentials are 256 random bits, protected by iOS Keychain; only
their hashes and names are stored on the Mac. No plaintext credential appears in
an ordinary file, URL, JavaScript message or log. API access requires a current grant; browser responses are no-store
with a restricted CSP and no CORS access.

Routes under `/api/v1` are pair, terminal listing and generation-scoped
`control` (POST/DELETE), `input` (POST binary) and `stream?cursor=N` (WebSocket).
Input requires a lease UUID and a unique command UUID in
`X-Interface-Control` and `X-Interface-Command`. HTTP 202 confirms admission to
the existing bounded PTY input lane, not process execution. Duplicate IDs return
the original receipt; changed bytes or target conflict. Unknown outcomes never
dispatch again. A grant holds at most 4,096 command receipts without eviction.
The phone records a pending command marker before sending. A lost response locks
further input until the owner checks the terminal, including after a page reload or native app restart. Only the opaque command ID
is stored, never the input text; legacy session markers are still honored.

Output is observed beside the authoritative desktop pump; remote network
backpressure never consumes desktop credits or resizes the PTY. Retention is
256 KiB / 512 frames per generation, at most 128 generations. Streams use a
21-byte header: `IRP1`, little-endian u64 start and end byte offsets, one gap
byte, then the unchanged PTY bytes. Cursors advance after xterm's write callback.
An unavailable cursor resets the view with a visible history-gap notice. This
is bounded byte replay, not a complete terminal-state snapshot after a long gap.

Limits: eight paired devices, eight connected streams, 32 active HTTP handlers,
16 KiB input bodies, ten-second request handling and two-second WebSocket send
timeouts. Revocation is checked during each output tick (200 ms). Disabling the
service closes streams and listener. The gateway deliberately has no generic
IPC dispatch, filesystem, provider-login, launcher or remote-resize endpoint.

## Companion HTTP v2

`GET /api/v2/workspaces` joins the saved Rust workspace roster to current
terminal identities. Names and order follow the Mac; missing entries are omitted,
duplicates are collapsed and unassigned sessions are grouped by their root.
`GET /api/v2/terminals/{id}/{generation}/history?limit=N` reads a confirmed Claude
or Codex provider binding through the existing Rust runtime. The workspace
`sessionKey` never authorizes history. Neither route accepts arbitrary provider
keys or file paths. Responses carry version 2; existing desktop and PTY contracts
remain unchanged. Rust and TypeScript share `remote-companion-v2.json`.

History reads require a valid grant and current generation before reading, then
recheck the grant, generation and provider binding before returning. At most two
history reads run concurrently. Pages clamp to 500–5,000 messages, and the
serialized response envelope must fit the 2 MiB body limit the tunnel and phone
enforce, or the phone gets the "exceeds the phone limit" page. The phone polls serially while visible and retains stable
conversation rows. A visual empty-prompt guard directs native menus and existing
CLI drafts to Terminal view; it does not replace provider-side approval rules.
The terminal remains mounted while switching views so its byte cursor is retained.
Local file attachments are not exposed through a new file gateway.

## Needs you and Threads on the phone (review D6)

**Roster v4.** `GET /api/v4/workspaces` is the v3 roster whose terminals keep
the host's `attention` (desktop protocol v44): present while a session's
program waits for the user. Rust decides it from the PTY bytes and input it
already sees (`crates/interface-terminal/src/attention.rs`), so the phone and
the Mac agree and nothing on the Mac has to be open. `/api/v1/terminals` and
rosters v2/v3 strip it and keep their bytes. The phone asks for v4, then v3,
then v2, so an older Mac keeps working.

**Threads v1.** `GET /api/v1/threads` lists the Mac's Threads in the host's
order with needs you first, then working, then the rest (pinned, active,
snoozed, settled; at most 60). From terminals, teammates' and archived
sessions are not listed. `GET /api/v1/threads/{engine}/{sessionId}` returns
the thread, its 12 most recent turns read-only (each user prompt with the main
agent's words after it, clipped to fit, and its tool-call count) from the
provider transcript, and the approval it is parked on, if any. A question
(AskUserQuestion) is marked not answerable: it stays on the Mac.

**Answering and following up.** `POST …/{engine}/{sessionId}/control` takes
this phone's 30-second lease on the thread (another phone is refused;
`DELETE` releases it). `POST …/approval` with `{version: 1, approvalId,
decision: allowOnce | deny}` and `POST …/send` with `{version: 1, text}` (at
most 12 KiB) each need that lease in `X-Interface-Control` and a new command
id in `X-Interface-Command`. The hub records the command id as outcome-unknown
before the effect starts and the effect runs to its end even if the request
is dropped; a repeat of the id answers the recorded outcome and never runs
again, and the same id for a different request conflicts. Before delivering,
the host re-reads the thread: it must still be listed, no Code pane may hold
the session (`SessionClaims`, review C1), an answer must name the request the
Mac shows now, and a follow-up goes only to a thread at rest (idle or
failed). It then goes through the same guarded runtime owner the desktop
uses (`respond`, `send`), keeping delivery certainty: definitely-unsent is the
only replay-safe refusal. The phone records an opaque marker (approval or turn
count, and the command id; never the text) in its protected input-marker
file before sending, and keeps it on any unknown outcome: an uncertain answer
blocks answering that request again until the Mac shows it gone or replaced;
an uncertain follow-up blocks the next until the Mac shows a newer turn or the
owner says they checked.

Rust, TypeScript and Swift read `electron/src/fixtures/remote-phone-v4.json`;
the live Simulator test runs roster v4, the list, a detail, an answer and a
follow-up through the encrypted connection against `phone_fixture`. Push
notifications are not part of this (they need APNs and a paid Apple Developer
account). The phone screens that show it are the "Native iPhone navigation"
settled look; see `docs/TODO.md` for their approval.

## Trusted native access v3

`POST /api/v3/enrollment` registers an id, name and credential digest for a
five-minute approval window (at most eight requests). No access is granted until
local desktop IPC approves that exact request and its Mac-generated comparison
number. The caller cannot choose the displayed number. `/enrollment/status` proves the
request secret without creating or extending a request. `/unlock` exchanges
approved trust for a session-only HttpOnly cookie; `/lock` expires that session
and releases its input leases. Every route requires the configured exact Origin.
Credentials never travel in URLs. Remote HTTP cannot approve or forget devices.

`remote-trust.v1.json` is an independent, bounded owner-private file with a lifetime
writer lease and atomic writes. It stores the opted-in listener and up to eight
trusted device digests. Malformed storage, failed writes, or uncertain durability
fail closed. Failed writes stop current access. Startup binds only the remembered
loopback port; it never silently chooses another port on conflict. Browser grants,
approval requests, cookies and control leases remain volatile. Input receipts
survive native cookie renewal within the host process; uncertainty markers on the
phone survive view and app replacement. Rust, TypeScript and Swift consume shared
v3/v2 fixture bytes.

The native workspace list and navigation wrap the same conversation and terminal
views. A native session URL names a terminal and generation, never a secret; stale
generations cannot silently select a replacement process. The ephemeral WebKit
view has one narrowly scoped input-marker reply bridge. The native side checks
the main-frame origin and selected terminal before storing opaque command IDs;
no general filesystem, Keychain or native API is exposed.

## Validation and remaining acceptance

`cargo xtask check` is the required full deterministic gate. Additional end-to-end
coverage is available with `cd electron && npm run smoke:remote`; it starts a
disposable Rust host, real shell PTY and sandboxed Chromium at phone dimensions.
It also verifies native approval, locked grants, trusted host restart, persistent
disablement, same-port restoration, revocation and native view generation fencing,
as well as saved workspace grouping, a provider transcript read through the
real Rust adapter, shared conversation rendering at portrait/landscape sizes,
view switching, pairing, Host/Origin refusal, cookie protection, duplicate-command
handling, stale generations, oversized input, desktop takeover, phone composer
input into the original process, output reconnect cursor, ambiguous response
handling across reload, revocation and listener shutdown. It saves screenshots
under the printed temporary evidence directory without credentials.

Chromium phone-size fixtures do not establish iPhone Safari behavior. Physical
iPhone acceptance must still cover Safari and Home Screen pairing, the software
keyboard and rotation, background/resume, Wi-Fi-to-cellular switching, Mac
sleep/wake and a real agent approval prompt.
Packaged signing, updater staging and production installation are separate gates.
