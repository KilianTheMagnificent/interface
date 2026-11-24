# Electron presentation layer

This directory preserves the Interface 0.1.169 Electron preload, React
renderer, visual assets, and renderer tests. It is the only shipping frontend
for the Rust port.

No legacy Electron main-process service is copied here. `src/main/bridge.ts` is
types and channel augmentation only. The eventual Electron main process will
supervise `interface-host`, translate the existing positional `window.iface`
methods into strict named Rust-host payloads, and forward the seven existing
push channels.

The narrow files under `src/engines` are temporary test oracles used by existing
renderer tests. They are not imported by the production renderer except for the
static Grok/Qwen model fallback catalog. Mapper/history oracles are deleted once
their fixtures are served by Rust parity tests.

This phase must not add panes, commands, providers, or product domains.
