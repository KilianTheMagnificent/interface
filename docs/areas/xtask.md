# xtask — checks, Dev verification and resumable releases

Rules: root `AGENTS.md` and `xtask/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

- `main.rs`: CLI routing for `check`, `closeout`, `dev-restart`, `promote`,
  `status`, `landings`, packaging, staging and worktree cleanup.
- `provider_canary.rs`: `provider-canary` runs `iface doctor providers` (the
  provider CLI drift canary; help and version output only, never a model
  turn) and fails when an installed CLI no longer matches what Interface
  depends on. It is on demand, not part of `check`.
- `gate.rs`: conservative changed-path classification and Git helpers. Every
  git child starts from `git_command`: no stdin, and an HTTPS fetch or push
  stalled below 1 KB/s for a minute fails instead of holding a lock forever.
- `navigation.rs`: read-only `navigate [query] [--json]` over
  `docs/agent-map.json`; `navigate --check` validates paths, scoped rules and Cargo
  member coverage. `navigation_tests.rs` pins routing and rejects stale/unsafe maps.
- `gate_cache.rs`: per-suite source identities plus the tools each suite
  runs, and separate Rust/Electron receipts, shared through the repository's
  common Git directory.
- `pipeline_state.rs`: bounded, private, atomic JSON receipt storage.
- `fsx.rs`: files checked before they are trusted — examined without following
  links, opened with `O_NOFOLLOW`, and compared again on the open descriptor.
- `dependencies.rs`: dependency identity and successful-install evidence.
- `closeout.rs`: integrate dev, validate the resulting tree, land one named
  merge, then verify Dev. Retries validate the newly integrated tree.
- `dev_restart.rs`: serialize Dev refresh, update the fixed checkout, verify
  dependencies, stop the previous supervisor/app/host and launch a new one,
  hidden unless Interface Dev was the app in front (`INTERFACE_DEV_QUIET=0`).
- `promotion_run.rs`: durable pinned release coordinator, isolated preparation,
  retries, atomic main/dev publication, feed readback and timings.
- `promote.rs`: version bump, selection, worktree and iCloud recovery helpers.
- `artifact_receipt.rs`: bind signature/archive/smoke evidence to the exact
  commit, manifest and artifact bytes; archive and reuse only matching bytes.
- `pipeline_status.rs`: independent readback of refs, Dev, gates, package and feed.
- `update_channel.rs`: the publisher's side of the public update channel
  ([Public updates](../PUBLIC_UPDATES.md)). `update-key` makes an Ed25519
  signing key under `~/.config/interface-release`; `publish-update` signs the
  staged release, rehearses it with `interface-updater`'s own mirror, refuses
  an archive whose app fails `package::verify_public_signature`, refuses an
  un-notarized build unless `--allow-unnotarized` (the normal mode with the
  free Apple Development certificate), and with `--github` uploads a draft
  release, makes it the latest and reads the live feed back. Never part of
  `promote`.
- `landings.rs`: named first-parent landings, shipped-set filtering and overlap;
  landing kinds, check lines, pins and supersedes as trailers; the release
  split, `--leave-out` acknowledgment, summary and `Ships:`/`Leaves-out:` trailers.
- `prove.rs`: `prove --pin <path>::<test> [--base <rev>]` runs one pinned
  test on the base (default `origin/dev`, not fetched) in the kept detached
  checkout `~/.interface-pipeline/prove/worktree` (own target directory,
  incremental off; reused and reset each run; `worktree-prune` keeps it),
  then on this checkout. One line per side (`passed`, `failed`, `absent`);
  exit 0 only when the base did not pass and the candidate did, 1 candidate
  failed, 2 base already passes, 3 could not run. A Rust pin lists, then runs
  `cargo test -p <crate> <target> <full name> -- --exact` (exactly one test);
  a TypeScript pin runs `npx vitest run <file> -t <escaped title>` with the
  JSON reporter (the base checkout gets `npm ci` the first time). No gate
  receipt, no ref moved.
- `pins.rs`: shipped pin tests — spans, the closeout guard against deleting or
  editing one without `Supersedes:`, and the promote candidate re-check.
- `check_run.rs`: canonical check execution — both suites, their step and half
  times, the npm deadline and the renderer lane pool.
- `test_runner.rs`: the Rust tests — one `cargo test --no-run`, then the
  test executables four at a time, longest first, with the per-executable
  deadline, flaky retry and time table.
- `package.rs`: signing/archive verification and smoke.
  `verify_public_signature` refuses an app (unpacked or from the ZIP) unless
  it, its Electron helpers and frameworks and its `Resources/bin` executables
  carry the hardened runtime and a secure timestamp under one team, and its
  designated requirement is one `interface_updater` reads and is exactly the
  team-pinned form `electron/scripts/sign-mac.cjs` signs with.
- `release.rs`: manifest validation, private writer locks, journalled feed/file
  publication and crash recovery. Manifests are installed last.

## Checks and dependency reuse

`cargo xtask check` and `check --full` always run both suites. Rust formatting,
Clippy, Swift helper build/tests and Rust tests run alongside `npm run check`,
whose lines stream under an `npm| ` prefix. The Rust tests compile once
(`cargo test --no-run`, the same targets, profile and features as before);
the test executables cargo reports then run four at a time, heaviest first
(`test_runner::WEIGHTS`, measured), each in its package directory with
cargo's `CARGO_*` variables, while Clippy (then any doc tests) takes the build
lock beside them. Each executable's output prints as one block when it ends
(`check: test <package> <kind> <target> passed in 3.2 s`), every failing one
is named with the `cargo test -p …` command that reruns it, a table of
executable times closes the step, and executables still running are named
every minute. A failure whose every test is in `gate::KNOWN_FLAKY` (empty
now) reruns only that executable, once. `npm run check` gets
`INTERFACE_RENDERER_POOL` 5 when it runs alone and 4 beside the Rust half,
unless the caller set one. `npm run check`
still running after 30
minutes (`INTERFACE_GATE_NPM_TIMEOUT` seconds, when set) is stopped with its
whole process tree and fails with its last 40 lines. A Rust test executable
still running after 10 minutes (`INTERFACE_GATE_TEST_TIMEOUT` seconds) is
stopped the same way and fails with its output so far ("did not finish within
600 s"). Both stay in the gate's process group, so ^C still reaches them.
`cargo fmt`, the test build, Clippy, doc tests, the smoke pins, the Swift
helper's build and `swift test` have no deadline. Both sides' failures are
reported. Each step prints its own time (`check: cargo clippy 11.2 s`), and each
half its total (`check: Rust half 114.2 s`), pass or fail, and each receipt
records its own suite's time, so the half that binds the gate is visible.
`--changed` selects the suites using all committed and working-tree
changes against its merge base; unknown paths and shared contracts require
both, and xtask is Rust. A file one suite reads from the other side of the tree
also selects that suite (`gate::CROSS_LANGUAGE`: the fixtures under
`docs/fixtures` and `electron/src/fixtures`, `PROTOCOL_FACTS.md` and the host
protocol sources, the `.mjs` transports in crates, `rust-host-client.ts`, the
navigation map, `xtask/src/package.rs`, which `package-parity.test.mjs`
reads, and the hands-free kit's fake providers, which the host suite's
outside agents test runs); a
test scans Rust and Electron sources for relative reads so the table cannot
drift. Those docs still count as documentation for landing kinds and Dev
refresh. `docs/UPDATES.md`, which the renderer embeds, is in neither suite:
every landing rewrites it. Its format is pinned instead by
`docs/fixtures/update-log-v1.md`, which `update_log::render_markdown` must
reproduce byte for byte and `update-log.test.ts` parses; `vite build` refuses
to run without the log. Rust-only checks also run smoke pins.
Every gate invocation validates the navigation map before suite selection/cache
reuse, even for documentation-only diffs. It checks route metadata, not prose
truth or live model behavior. Focused commands in the map are never auto-executed.

Each successful suite records a receipt keyed by the bytes that suite reads
plus the tools that suite runs: `node`, `npm` and what is installed for
Electron (`installed`: the digest of npm's `node_modules/.package-lock.json`
without the app version, or `none`, so a pass under an install from an older
lockfile is not saved for the new one); `rustc -Vv`,
`cargo`, Xcode's `swift --version` and SDK version and the Rust build
variables for the Rust half and its Swift steps. So another `node` on `PATH`
leaves Rust receipts valid and an Xcode update leaves Electron's (an Xcode
26.6 → 27.0 update overnight is why the 0.3.151 promote re-ran both suites on
dev's own tree). A receipt also keeps its source digest and tool versions:
when a suite misses although a receipt for the same source exists, the gate
prints the tools that changed (`check: Rust inputs unchanged since their
receipt; its tools changed: swift … -> …`). Dirty-but-stable source can earn a receipt
and committing those same bytes preserves it. A failed suite never earns one,
even if the other passes. What a suite reads is `gate::scope_of`, the
classification `--changed` selects by, including `gate::CROSS_LANGUAGE`: the
Rust identity hashes Rust and shared files, the Electron identity Electron and
shared files. Two reads that classification leaves to other rules are added:
the Rust identity also carries every file outside `electron/` an Electron
script names by a relative literal (the cross-language test, a Rust test, reads
those), and the Electron identity every Rust file the smoke-pin sources name
(`gate::SMOKE_PIN_SOURCES`; `smoke-pins.test.mjs` reads them). So a landing that
changes one side keeps the other side's receipt, and a closeout whose merge of
dev brought only the other side's already-gated changes reuses that suite.
The Swift helper's build and tests, which run inside the Rust half, have a
receipt of their own (`voice`), keyed by `apps/interface-voice/**`, the two
xtask files that say how they run (`package.rs`, `check_run.rs`) and the
Rust half's tools: a Rust half whose helper inputs match a passed receipt skips them
(`check: voice helper reused`); `check --full` always repeats them.
Inputs are enumerated with Git; generated/ignored artifacts are excluded (the
scratch directories `work/` and `output/` are ignored, so a transcript written
there never moves an identity or dirties the tree), and credentials or
nonregular inputs disable caching without reading their contents. A source/toolchain change during a run prevents reuse. Receipts made
before per-suite identities (`interface-gate-source-v2`), or before per-suite
tools (`interface-gate-subject-v2`), no longer match.
Only the application's root version fields in package.json and package-lock.json
are normalized; dependency versions remain part of the identity. Legacy
`target/xtask/gate-stamp` files are no longer trusted.

Receipts live under `<git-common-dir>/interface-pipeline-v1/checks`. Worktrees
share this evidence, never Cargo target directories or generated JS output.
`package-mac --trust-gate` still rebuilds JS output before packaging; promote
skips that rebuild only when its own Electron suite (`npm run check`, which ends
in `npm run build`) just passed in the release worktree. A missing receipt runs
the checks again; `check --full` always bypasses reuse.

Packaging's `cargo build --release` alone sets `INTERFACE_PRODUCT_VERSION` to
the package.json version, so `iface --version`, the TUI title, the host ready
frame's `hostVersion` and the Codex/ACP client handshakes report the release;
it then refuses unless the built `iface --version` prints exactly that
version. Only iface and interface-host's binary target (`main.rs`, not the
library iface also links) read the variable, each expanding
`interface_common::product_version!` once into its own `PRODUCT_VERSION`. They
hand it to the host library (`HostServerConfig::host_version`, the bootstrap's
`client_version`) and to the engines
(`NativeAdapterConfiguration::with_client_version`) at runtime, so a new
version recompiles only those two targets, in parallel, in the release build. Every other build reports the
workspace crate version, so the stamp is never a gate input and a version-only
bump still reuses both receipts. Nothing compares `hostVersion`: Electron main
checks only that it is a string.

Packaging re-renders the tracked icons through Chrome only when
`target/xtask/icon-stamp` (icon sources, `.icns` bytes, `make-icon.mjs`,
Chrome's Info.plist, macOS build) differs from the last render in that checkout.
A failed smoke keeps its last 40 output lines in the error, so the promote run
record says why. `sha2` is optimised in the dev profile: debug xtask hashes each
~140 MB artifact several times per promote.

Dependency installation records live inside node_modules. A valid record and
complete Electron/compiler/bundler installation skip npm ci. App-version bumps
preserve the dependency identity; changed dependencies invalidate it. The proof
is removed before installation and written only after success and verification,
so interrupted installs retry. The first run without the new receipt installs
once, including Electron's binary when npm's install-script policy omitted it.

## Land and verify Dev

Closeout requires a clean task branch and a meaningful landing name. It pins the
fetched dev SHA, merges it, validates all changes since the common ancestor,
and creates a merge commit whose first parent is dev. HEAD and source must
still match the validated tree before pushing. It retains the settled-look guard
and adds the shipped-pin guard. The landing carries `Landing-kind:`,
`Landing-check:`, `Landing-pin:` and any `Supersedes:` the branch's commits
declare, so a `-x` cherry-pick of the landing alone keeps them. A test span runs
from the attributes or comments directly above its name to the line formatting
closes it on: a lone `}` (Rust, Swift) or a `})`-style end of the `it(`/`test(`
call (JS/TS) at the name line's indentation, or the line before a sibling test
that starts first. Only the name line is lexed, so regexes, template literals
and embedded scripts in a body cannot move the end; bracket counting remains
the fallback. An insertion after the closing line is a new neighbour, not a
change. The guards run from the xtask built before closeout merged dev; when
that merge changed xtask, a refusal says so, and a rerun decides with the
merged code. The pin guard reads only pins in files `dev..HEAD` changes (renames
count as a deletion), each file once; promote's candidate check reads each
pinned file once.
After a product landing it runs the Dev refresh; docs-only landings skip it.
A failed refresh leaves the landing on dev and can be resumed by name. A push
that fails without a rejection is read back: when dev already carries the
landing, closeout finishes as landed, and a rerun after such a failure finds
the landing that merged its branch, moves the branch onto it and refreshes.

Dev refresh uses the fixed `.interface-dev` checkout. The main-process dev-only
readiness probe waits for a rendered root and calls the existing read-only
`getAppearance` API through renderer → preload → IPC → Rust host. Its log
response carries the fresh launch identity, revision and Electron PID. Old or
pre-launch messages cannot satisfy it; failure messages take precedence. Rust
writes the readiness receipt only while the source revision is still clean.
Status checks that the recorded process and source still match, and reports
unverified/stopped instances explicitly. This is startup/bridge verification,
not a substitute for exercising the changed product behavior.

## Prepare and publish an approved release

`promote --version <v> --revision <sha>` pins an approved revision already on dev
that contains current main. Without --revision it uses HEAD. `--change` selects
named landings, oldest first, applied to pinned main in the managed worktree.
The caller's working tree and branch remain untouched. Conflicts preserve the
candidate record and report overlapping landings.

The shared `~/.interface-pipeline/.promotion.lock` covers preparation through
publication. Manual package-mac and stage-update use the same lock.
package-mac refuses a dirty tree before it gates, builds or replaces `release/`. The default
release checkout remains `~/.interface-release`, outside iCloud and /private/tmp.
Existing detached worktrees must belong to this repository and be clean.
Dependencies and build outputs stay local to that persistent worktree.

Per-version run records under `<git-common-dir>/interface-pipeline-v1/promotions`
pin the selection, source, local release ref, worktree, verified artifact and
completed steps. Local `refs/interface/releases/<version>` retain candidates
across interruptions. Package verification records bind the commit and smoke
success to manifest and artifact digests; an archive under
`~/.interface-pipeline/artifacts/<commit>` survives later packaging runs.
A retry validates the archived bytes and skips rebuilding/signing/smoking them.

Before publication, merge-tree constructs an integration of the candidate and
latest dev without leaving merge conflicts in a checkout. That tree is gated.
One atomic fast-forward push publishes main and dev together. A lost race
retries integration/validation up to three times, keeping the release artifact
unchanged. Ref readback also handles an ambiguous network failure. The feed is
staged only after both refs contain the candidate; matching already-staged bytes
are an idempotent success, while different equal-version bytes or downgrades are
refused. Feed publication retains its existing journal/rollback transaction.

`--no-push` stops with a durable verified candidate. Rerun the same command
without it to publish. `--dry-run` fetches refs and prints the plan without
preparation, records, pushes or staging. Both selective and whole-dev releases
resume after a staging failure. A newer release on main supersedes an older
candidate instead of silently staging an obsolete version.

`status [--fetch] [--json]` reads each state independently and prints timings,
failures and the next action. The `packaged` time runs from before the release
checkout through signing and smoke, so when promote ran the gate it includes
`checked`; status labels it `packaged (includes checked)`. A feed holding the candidate's version with other
bytes is `feed.error`, and the next action is to inspect the feed, not to
resume a promote that would refuse it. Source checks refer to the caller's current source;
package checks refer to the recorded release candidate. No fetch is implicit in
status. `landings` remains the menu of changes the owner can select.

## Verification

Run `cargo test --locked -p xtask` for receipt, retry, lock, atomic Git publication,
artifact corruption and scoped-gate regressions. The main-process tests cover
Dev probe shutdown/failure behavior. Then run the full `cargo xtask check`.
Live Dev startup and packaged/signing/smoke verification are separate gates;
report whether each was exercised. Never stage a package whose smoke failed.
