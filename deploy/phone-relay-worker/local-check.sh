#!/bin/sh
# Runs the real Mac relay session (interface-remote's opt-in
# relay_worker_carries_the_real_mac_session) against this Worker in
# Cloudflare's local runtime. Needs `npm ci` here and Rust's
# wasm32-unknown-unknown target plus `cargo install worker-build@0.8.6`.
set -eu
cd "$(dirname "$0")"
port="${INTERFACE_RELAY_WORKER_PORT:-8787}"
# The test Mac: private key 7, never a real Mac.
mac=b33e4d1c20df27891225e731ac50234b2a202ec4442427bfbae62ca6a6f393b4
log="$(mktemp -t interface-relay-worker)"
npx wrangler dev --ip 127.0.0.1 --port "$port" --var "ALLOWED_MACS:$mac" >"$log" 2>&1 &
wrangler=$!
trap 'kill "$wrangler" 2>/dev/null; wait "$wrangler" 2>/dev/null || true' EXIT
for _ in $(seq 1 240); do
  curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1 && break
  if ! kill -0 "$wrangler" 2>/dev/null; then cat "$log"; exit 1; fi
  sleep 1
done
curl -fsS "http://127.0.0.1:$port/health" >/dev/null || { cat "$log"; exit 1; }
INTERFACE_RELAY_WORKER="127.0.0.1:$port" cargo test --manifest-path ../../Cargo.toml \
  -p interface-remote --lib relay_worker_carries_the_real_mac_session -- --ignored
