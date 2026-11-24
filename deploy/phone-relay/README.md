# Personal Interface relay

The recommended host is now the Cloudflare Worker in
[`../phone-relay-worker`](../phone-relay-worker/README.md): no machine to keep
running, free at personal scale, and it serves several Macs. This Fly.io build
remains a working alternative.

Recommended deployment: one always-on Fly.io shared CPU Machine with 512 MB RAM
in Chicago (`ord`), using its supplied `https://APP.fly.dev` address and shared
IPv4. No database, volume, custom domain or second Machine is needed. This relay
keeps routing state in memory, so deploys briefly interrupt the connection and the
Mac reconnects (after 3 s, backing off to at most 30 s while the relay is down). Do not scale it to multiple Machines: routing is intentionally
single-owner and process-local.

Fly's published baseline for a 512 MB shared CPU Machine is about $3.32/month;
regional pricing and outgoing bandwidth affect the bill. Budget roughly $3–5/month
for a small personal deployment, plus usage. This is an estimate, not a billing cap.
See [resource pricing](https://fly.io/docs/about/pricing/). No paid resource has been
created by preparing these files.

## Prepare and inspect

1. In the Mac app, enable **secure phone connection** with no relay address.
   Expand **Relay setup details** and copy its public Mac identity.
2. Stage a new minimal build context from the repository root:

   ```sh
   python3 deploy/phone-relay/prepare-context.py /private/tmp/interface-relay-deploy
   ```

   The script copies only the remote/contracts Rust source and build manifests.
   It does not copy workspace sessions, private configuration, account files,
   Electron source, iPhone assets, the Git directory or the rest of the repository.
3. Copy `fly.example.toml` to `fly.toml` inside that context. Set the app name and
   the exact public Mac identity. The server refuses an invalid identity and asks
   the Mac to prove possession of its private key before it can register.
4. Review the resulting context before an authorized deployment. An authenticated
   Fly account and approval for the paid Machine are required.

## Deploy after approval

Use Fly's CLI from the staged context, create the selected app, allocate a shared
IPv4, then deploy with high availability disabled so only one Machine is created:

```sh
fly apps create YOUR-INTERFACE-RELAY
fly ips allocate-v4 --shared -a YOUR-INTERFACE-RELAY
fly deploy --ha=false
```

Check `https://YOUR-INTERFACE-RELAY.fly.dev/health`, Machine health and the Mac's
**Relay connected** status. Turn off the secure phone connection in Mac Settings,
enable it with this HTTPS address, then scan a fresh QR on the iPhone. Invitations
carry the pinned Mac key and relay address; changing the address requires a new
scan, not a DNS guess or a trust-file copy.

Keep plaintext port 8080 behind Fly's TLS proxy. The Internet endpoints expose
only a health response, a Mac registration socket and encrypted packets. No
session content, credentials or request bodies are logged. The relay can observe
routing identity, timing and packet sizes; it cannot approve devices or decrypt
session content. Do not enable access/request body logging in a proxy.

A stranger who knows the address cannot keep the Mac out: sockets that have not
proved the Mac's key wait in their own room (16, 4 per client address) and a new
one displaces the oldest instead of being refused (`relay_admission.rs`); the
Mac's registration never waits on other requests, and one address may have at
most 16 of the relay's 32 requests in progress. The client address is Fly's
`Fly-Client-IP`, believed only from the proxy's private network. Fly's own
`hard_limit` in `fly.toml` still caps connections at the proxy, in front of
these rules.

## Acceptance

The source's loopback tests cover the real HTTP/WebSocket relay, Mac key proof,
encrypted requests, unavailable-Mac responses and the session authentication
boundary. A container build, deployed TLS route, physical iPhone pairing, Wi-Fi
to cellular transition, Mac sleep/wake and relay restart must also be verified
before calling the hosted path production-ready.
