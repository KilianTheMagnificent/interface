# Interface phone relay on Cloudflare Workers

The recommended way to reach the Mac from a phone on another network. It is the
same relay as `crates/interface-remote/src/relay.rs` (see
[`../phone-relay/README.md`](../phone-relay/README.md) for the Fly.io build),
served by a Cloudflare Worker and one Durable Object instead of a machine you
keep running. Neither the Mac app nor the iPhone app changes: the Mac keeps an
outbound WebSocket to `wss://<relay>/v5/mac`, the phone posts sealed packets to
`https://<relay>/v5/packet`, and the relay only ever sees ciphertext, routing
identity, timing and sizes.

Cost: Workers Free covers a personal relay (`*.workers.dev` address, no
domain). The Durable Object hibernates between messages; its alarm wakes it
every 25 s to keep each Mac's connection open.

## How it differs from the Fly relay

- **Several Macs.** `ALLOWED_MACS` lists public Mac identities (comma or space
  separated). Each must prove its private key before it can register. The Fly
  relay serves exactly one.
- **The newest proven connection wins.** After a network change the old
  socket can still look open; the Fly relay refused the new one until its
  heartbeat noticed (up to 45 s). Requests in flight on a replaced socket get
  504, never a replay.
- **Keepalive packets instead of pings.** Workers cannot send WebSocket pings,
  and a Mac ends a relay connection that stays silent for 50 s. Every 25 s the
  relay sends each Mac a well-formed packet it cannot open; the Mac drops it
  before any key work and answers nothing (`protocol::keepalive`).
- **Content-Length is required** on phone packets (the iPhone and Mac send
  it), so a body is bounded before it is read.
- **A stranger cannot keep the Mac out.** Both relays admit `/v5/mac` sockets
  by the Mac's own `relay_admission.rs` (compiled here by path): a socket that
  has not proved a Mac's key waits in its own room (16, 4 per client address,
  from `CF-Connecting-IP`) and never takes a proven Mac's place. A new socket
  is never refused for room; it displaces the oldest waiting one, from its own
  address first, and one that has not named an allowed Mac before one that
  has. The runtime keeps a closed socket until its client answers the close,
  so an address holding 32 sockets that are not a proven Mac gets 429 until it
  lets some go; a Mac answers at once and holds one or two.
- One Durable Object holds every Mac, because a Mac names itself only after
  its socket is open. Sharding by Mac would need the Mac to put its id in the
  URL, which is a Mac protocol change.

`src/protocol.rs` is the relay's logic, tested natively. It compiles the
Mac's own `connection_crypto.rs` (by path), so the challenge is sealed by the
same code the Mac opens it with. `src/lib.rs` adapts it to the Workers runtime.

## Check

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.88.0
cargo install worker-build@0.8.6 --locked
npm ci
cargo test                 # protocol + the shared HPKE vectors, natively
./local-check.sh           # the real Mac relay session against the Worker in workerd
```

`local-check.sh` starts `wrangler dev` with a test-only Mac identity and runs
interface-remote's opt-in `relay_worker_carries_the_real_mac_session`: impostor
refusal, ping and roster through the relay, the Mac surviving 60 s on
keepalives, and a reconnect replacing the old socket. It takes about 80 s.

## Deploy

Deploying needs the owner's Cloudflare account (`npx wrangler login`).

1. In the Mac app, turn on the secure phone connection with no relay address,
   expand **Relay setup details** and copy the public Mac identity.
2. From this folder:

   ```sh
   npx wrangler deploy
   printf '%s' '<public Mac identity>' | npx wrangler secret put ALLOWED_MACS
   curl https://interface-relay.<account-subdomain>.workers.dev/health   # ok
   ```

3. In the Mac app, turn the secure phone connection off, then on again with
   `https://interface-relay.<account-subdomain>.workers.dev` as the relay
   address, wait for **Relay connected**, and scan a fresh QR code on the
   iPhone. Invitations carry the relay address; changing it needs a new scan.

`npx wrangler tail` shows only "mac <id prefix> connected/disconnected". No
request bodies, cookies or contents are logged; keep it that way.

Acceptance still needs the owner's phone: pair, then turn Wi-Fi off and check
the session list, a conversation and a reply over cellular, a Mac sleep/wake,
and a relay redeploy (the Mac reconnects within 3 s).
