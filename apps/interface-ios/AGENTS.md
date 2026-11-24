# Native iPhone shell

Read root `AGENTS.md`, `docs/areas/remote-and-native.md` and this directory's
`README.md`. Navigation: `cargo xtask navigate phone-ui` from the repository root.

- SwiftUI/Apple API integration lives here; the Mac Rust host owns sessions,
  trust, approval decisions, input leases and effect delivery.
- Native secrets stay in Keychain and never enter JavaScript or logs.
- Preserve pinned Mac identity, explicit pairing approval and lock/revoke behavior.
- Shared conversation UI lives in `electron/src/remote`; read `electron/AGENTS.md`
  before editing it. Keep bundled asset and native transport contracts aligned.
- Before a visual change, check `docs/SETTLED_LOOKS.md` for the entries whose
  markers it touches.

Run the Xcode tests described in `README.md` with an installed Simulator, the
relevant remote smoke, and `cargo xtask check`. Report native/physical-device
gates separately; desktop checks alone do not prove iPhone behavior.
