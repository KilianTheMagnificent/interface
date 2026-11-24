# Remote access and native shells

Start with `cargo xtask navigate phone` or `cargo xtask navigate voice-helper`.
The checked routes in `docs/agent-map.json` carry exact rules, source and tests.

## Phone request path

```text
apps/interface-ios                 SwiftUI library, Keychain, QR, Bonjour, HPKE
  SessionWebView.swift              native/WebKit boundary
electron/src/remote                 bundled React conversation and native transport
  native-transport.ts               versioned native request/response projection
crates/interface-remote             access, trust, replay, HTTP/direct connection
apps/interface-host/src/remote.rs   host adapter into the actual Mac session graph
crates/interface-terminal          terminal generation and input authority
```

`task-result-polling.ts` owns saved-result reads: one request at a time, paused
while hidden, with a 10-second deadline and exact terminal generation checks.
Disposal aborts web requests and ignores late native bridge responses.
`roster-polling.ts` reads `/api/v2/workspaces` every 2 s while visible and on
return; an open native session re-reads it every 10 s, at the next tick after
output, and at once when its stream drops.

The Mac remains authoritative. `interface-remote/src/connection.rs` and
`connection_crypto.rs` implement the v5 encrypted connection; `http/direct.rs`
adapts it to the HTTP/WebSocket surface. Its nearby listener (`NearbyListener`,
`NEARBY_LIMITS`) drops non-nearby peers at accept, caps connections at 32 in all
and 16 per address (`AddressShare`; a phone uses at most 8), gives a request's
head 15 s (the phone's own whole-request limit), fails a read after 10 s of
silence while it waits for the phone (`NearbyStream`; paused while a packet is
answered) and closes every connection after one response. The `packet` handler takes a request slot, shared with relay packets,
only after the body is read. `trust.rs`, `access.rs`, `approval.rs`
and `replay.rs` own trust, access, displayed approval identity and replay controls.
`crates/interface-settings/src/phone_connection.rs` stores connection settings.
The Mac's relay connection (`relay_loop`) retries 3 s after a drop, then backs off
6, 12, 24 and 30 s while the relay stays unreachable, and starts at 3 s again once
a session reaches it.
DTOs live in `crates/interface-contracts/src/remote*.rs`; TypeScript projections
live in `electron/src/remote` and `electron/src/shared`.
The phone roster is `/api/v2/workspaces` plus `/api/v3/workspaces`, which adds each
terminal's sidebar project (`workspaces.rs` `terminal_projects`, mirroring
`terminalProjectKeyOf` in `electron/src/renderer/viewmodels.ts`). The host
indexes it under the registry lock (`SessionProjects::index`) without copying
or sorting any session row. Both sides read
`electron/src/fixtures/remote-projects-v3.json`; change the rule on both at once.
Roster v4 (`/api/v4/workspaces`, `workspace_snapshot_v4_indexed`) keeps each
terminal's host `attention` (needs you); v1–v3 strip it (`without_attention`).
Phone Threads v1 (`/api/v1/threads…`) are pure projections in
`interface-remote/src/threads.rs` (list order, recent turns, the pending
approval) over what the host backend reads in `apps/interface-host/src/remote.rs`
(`ThreadHost::view`, the registry, provider history, live events). Answers and
follow-ups share one path: `RemoteHub::claim_thread`, `begin_thread_effect` /
`settle_thread_effect` (the receipt ledger terminal input uses), then the
guarded runtime owner's `respond` or `send`, refused while a Code pane holds
the session. Fixture: `electron/src/fixtures/remote-phone-v4.json`; Swift in
`apps/interface-ios/PhoneThreads.swift`. See [IPHONE_REMOTE.md](../IPHONE_REMOTE.md).
`/api/v1/routes` (direct connection only) returns `MacRoutesV1`: the Mac's
nearby origins (`nearby_origin`, the same rule as v6 QR codes; the tailnet
address it also sent was retired with the Tailscale route on 2026-10-14). The
phone saves them after each unlock as `interface.macRoutes.v1` and probes them
with the QR routes; a saved list holding a 100.64/10 origin is not used. Both sides read
`electron/src/fixtures/remote-routes-v1.json`.
`electron/src/fixtures/remote-connection-crypto-v5.json` holds the cross-language
HPKE vectors that `connection_crypto.rs` and `MacAddressTests.swift` pin: its `rust`
half is printed by `cargo run -p interface-remote --example connection_vectors` and
its `swift` half by `apps/interface-ios/generate-connection-vectors.swift`.

`apps/interface-ios/PhoneModel.swift` drives native library state;
`DirectConnection.swift` handles encrypted nearby transport;
`TrustedConnection.swift` handles protected connection identity;
`NativeSessionTransport.swift` and `SessionWebView.swift` adapt requests to WebKit.
The phone's bundled page is built by `electron/vite.phone.config.ts`. Do not edit
`dist-phone`, `dist-remote`, Xcode derived data or generated bundles.

Read [PHONE_CONNECTION_V5.md](../PHONE_CONNECTION_V5.md) for the retained v1-v4
boundary, explicit pairing authority and physical acceptance requirements.
[IPHONE_REMOTE.md](../IPHONE_REMOTE.md) documents the earlier access path.
[The iPhone README](../../apps/interface-ios/README.md) has exact Simulator,
fixture and native test commands. The relay is hosted as a Cloudflare Worker
(`deploy/phone-relay-worker`, recommended: free, several Macs, keepalive packets
because Workers cannot ping) or the Fly.io build in `deploy/phone-relay`. The
Worker compiles `connection_crypto.rs` and `relay_admission.rs` by path. Both
relays admit `/v5/mac` sockets by `relay_admission.rs`: a socket that has not
proved the Mac's key waits in its own room (16, 4 per client address) and
displaces the oldest waiting one instead of being refused, so a stranger never
keeps the Mac out; the Fly relay also exempts registration from its request
slots and gives one address at most 16 of 32 (`HandlerShare`), and the Worker
refuses an address holding 32 sockets that are not a proven Mac. The Worker is
checked by its own
`cargo test` and `local-check.sh`, which runs this crate's opt-in
`relay_worker_carries_the_real_mac_session` against it; neither runs in the
gate. Reading their instructions does not authorize a deployment.

Focused Rust tests: `cargo test --locked -p interface-remote`. Remote UI/transport
tests run under Vitest from `electron`. `npm --prefix electron run smoke:remote`
uses a disposable host. Native Xcode tests and physical phone/network acceptance
are separate from the desktop gate. Never infer hosting or installation from
source files or an earlier dated report.

## Voice request path

`crates/interface-voice` owns protocol/chunking/text projection;
`apps/interface-host/src/voice` coordinates the helper and session effects;
`apps/interface-voice` adapts Apple speech/audio frameworks. The Swift helper
is an intentional platform adapter, not a second canonical policy store.
Read [VOICE_ASSISTANT.md](../VOICE_ASSISTANT.md) and the
[helper README](../../apps/interface-voice/README.md). Rust and Swift unit checks
do not establish microphone permission, real recognition or speaker behavior.
