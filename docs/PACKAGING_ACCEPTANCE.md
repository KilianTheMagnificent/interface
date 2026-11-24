# Packaging and updater acceptance

The macOS release path is owned by Rust `xtask`. Electron Builder remains the
effect boundary that assembles and signs the retained Electron shell; the
release policy, manifest inspection, artifact selection, transaction recovery,
feed replacement, and acceptance sequencing are Rust.

## Commands

- `cargo xtask check` runs formatting, strict workspace Clippy, all Rust tests,
  and the Electron check.
- `cargo xtask package-mac` runs that gate, builds release `interface-host` and
  `iface`, builds the icon, invokes Electron Builder into a private directory
  under `/private/tmp`, verifies the output, then transactionally replaces only
  managed files under the repository `release/` directory.
- `cargo xtask package-mac --smoke` additionally launches the exact
  `Interface.app` extracted from the verified ZIP with an isolated Electron
  profile and private Rust-host data/home roots, then probes renderer mount,
  preload exposure, session IPC, default-directory IPC, updater-state IPC, and
  model-scoped provider-command UI through CDP.
- `cargo xtask verify-package --source <builder-output> [--smoke]` repeats
  artifact inspection without publishing anything.
- `cargo xtask stage-update --source <release> [--target <feed>]` validates and
  atomically replaces one dedicated Electron updater feed. Omitting `--target`
  selects the production local feed
  (`~/Library/Application Support/Interface/update-feed`). No updater feed is
  staged during default agent closeout. Root `AGENTS.md` requires explicit
  owner approval and an explicit `--target` before an agent stages either the
  production feed or the Iface Dev feed at
  `~/Library/Application Support/Iface Dev/update-feed`. Equal-version
  replacement requires the explicit `--allow-same-version` flag and downgrades
  are always refused.

`npm run package:mac`, `npm run package:smoke`, `npm run stage:update`, and the
two local-release scripts are compatibility entrypoints that call these Rust
commands. None of them publishes to a remote service.

## Artifact proofs

Before release files become visible, the package path requires all of the
following:

1. `latest-mac.yml` passes the bounded parser, canonical SHA-512 validation,
   arbitrary-precision SemVer policy, flat-name policy, exact size checks, and
   source checksum checks.
2. The manifest contains the Squirrel.Mac ZIP (and any optional DMG) and no
   unsupported artifact type.
3. The unpacked app passes deep/strict `codesign`, its Electron fuse wire is
   re-read, and it contains executable `Resources/bin/interface-host`,
   `Resources/bin/iface` and `Resources/bin/interface-voice`. Each embedded
   Mach-O's executable content must match the release Cargo output (the
   voice helper's Swift build output) after normalizing only the signing-managed
   `__LINKEDIT` size fields and `LC_CODE_SIGNATURE` data size. This permits the
   signer to replace the code-signature blob without weakening the proof over
   the executable content.
4. Any DMG passes `hdiutil verify`; the ZIP passes `unzip -tq` and extracts to
   exactly one real top-level `Interface.app`.
5. The app extracted from the ZIP repeats signature, fuse, bundled-binary, and
   executable-content inspection. Its signed Rust binaries must also match the
   unpacked signed app byte for byte. A requested smoke runs only against this
   archived app.
6. Both apps pass `verify_public_signature` (`xtask/src/package.rs`): the app,
   each Electron helper and framework, and each `Resources/bin` executable
   show the `runtime` flag, a `Timestamp=` line and one team in
   `codesign -d --verbose=2`, and `codesign -d -r-` prints a designated
   requirement that `interface_updater::parse_designated_requirement` accepts
   and that is exactly `identifier "com.andreromero.interface" and anchor
   apple generic and certificate leaf[subject.OU] = "<team>"` for the team
   that signed it. An installed copy updates only to a build that satisfies
   its own designated requirement, so a build failing this would turn updates
   off or strand copies at a certificate renewal.
7. When the Rust host compiles in a public update channel, the packaged smoke
   waits for the updater to finish reading the app's signature and fails if
   it stays `disabled`.
8. Optional adjacent blockmaps are accepted only when derived from validated
   ZIP/DMG names and are regular, single-link files.

The signed app and archived app are both checked because a valid unpacked
builder output does not prove that the shipped ZIP retained the same files.

Signing (`electron/scripts/sign-mac.cjs`) uses the free Apple Development
certificate with the hardened runtime and Apple's secure timestamp, and no
notarization. Entitlements: `electron/build/entitlements.mac.plist` for the
app, `entitlements.mac.inherit.plist` for nested Electron code, and a short
list per `Resources/bin` executable in `sign-mac.cjs`. None of them may hold
`allow-dyld-environment-variables`, `get-task-allow` or
`disable-library-validation` (`sign-mac.test.cjs` checks).

## Transaction and rollback rules

Repository release publication uses an exclusive owner lock and a durable
transaction journal. Existing managed files move to `previous/`, new artifacts
move into the release directory, and `latest-mac.yml` moves last. Rollback moves
the new manifest out first and restores the old manifest last. The next run
recovers either a fully committed set or the prior set after process death;
unknown manifest/journal combinations fail closed. Unrelated release files are
not removed.

Updater-feed staging checksum-verifies into a private sibling directory before
renaming the existing feed aside. If the publish rename fails, the previous feed
is restored. Unit acceptance injects that failure and verifies prior manifest
bytes, prior artifacts, and unrelated siblings remain intact.

## Evidence as of 2026-08-27

- The final canonical `cargo xtask check` passed immediately before packaging:
  strict workspace Clippy, all deterministic Rust tests/doc tests, 2,157
  Electron tests, 14 release-policy tests, both TypeScript checks, and the
  production renderer build.
- Rust packaging/updater tests: 10 passed, including checksum refusal,
  hardlink/symlink refusal, embedded Mach-O content proofs, feed rollback,
  mid-release rollback, interrupted-transaction recovery, and unrelated-file
  preservation.
- Electron release-policy tests: 14 passed, including signing policy, fuse
  inspection, Rust-owned command wiring, exact resource declarations, and the
  packaged CDP probe contract.
- Strict `cargo clippy -p xtask --all-targets -- -D warnings`: passed.
- A fresh disposable Apple Development-signed Electron Builder output under
  `/private/tmp` passed the Rust verifier after the final port changes: one
  arm64 app, one DMG, one ZIP,
  canonical manifest size/SHA-512 checks, deep/strict code-signature checks,
  Electron fuse checks, DMG verification, ZIP integrity and extraction checks,
  and both Rust executable-content proofs.
- The exact fresh ZIP-extracted app passed the CDP acceptance probe with an isolated
  Electron profile and release-safe private Rust-host data/home roots. The
  renderer mounted, the preload API was exposed, Rust host session and
  default-directory IPC returned, updater-state IPC returned, the expected
  model-scoped provider UI rendered, and no renderer error was reported.
- The private builder output was moved to Trash after verification. No
  repository release, updater feed, or installed application was changed.

Not yet run in this migration checkout: protected-folder behavior, staging to
the real local updater feed, updater restart, running-byte replacement,
installation under `/Applications`, or installed rollback. Those updater and
installed-app gates must remain reported as unverified until those exact paths
run. The disposable acceptance did not publish a repository release, stage any
update feed, install the app, or mutate the running application.

## Protocol v3 terminal-launcher acceptance — 2026-08-28

Version 0.3.0 completed a fresh package acceptance after the launcher increment:

- the real-child protocol smoke read back v3 with 106 renderer requests and 8
  push families;
- the signed ZIP-extracted app invoked
  `iface:listTerminalLaunchProfiles` through its real preload/Rust-host
  boundary and confirmed that the public catalog contains only the six frozen
  profile DTOs, without executable paths, argv, or environment;
- the packaged renderer opened the terminal launcher and confirmed the Claude,
  Codex, Grok, Qwen, Gemini, and Shell choices, five agent-logo marks, exact
  count buttons 1 through 10, and the private approved working directory;
- the compiled-host lifecycle smoke separately created a real ten-terminal
  Shell-profile batch, confirmed unique output and exact launch intent for all
  ten processes, and dismissed every retained entry; and
- the fresh Apple Development-signed app, DMG, ZIP, blockmaps, Electron fuses,
  embedded Rust binaries, archive checks, and packaged renderer/Rust-host smoke
  all passed.

This acceptance did not publish or stage an updater feed, install under
`/Applications`, replace a running app, exercise updater restart/rollback, or
claim installed-runtime readback.

The later source-only launcher adjustment caps both the current count selector
and visible pane grid at six. The ten-terminal statements above remain the
historical 0.3.0 package record; the six-terminal cap has not been repackaged or
staged.
