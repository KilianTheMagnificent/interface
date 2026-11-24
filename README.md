# Interface

A Mac workspace for running several AI coding agents side by side. Interface starts the
agent command-line tools you already have (Claude Code, Codex, Grok and others) in terminal
panes and in threads, shows what each is doing and what it changed, and lets them hand work
to each other. It also ships `iface`, the same workspace as a terminal program.

Interface has no account or server of its own: every agent runs on your Mac, signed in as
you with its own provider.

## Status

A personal project, version 0.3. Its author uses it every day.

- Apple Silicon Macs on macOS 12 or newer. It has only ever run on macOS 27; Intel Macs are
  not supported.
- You need at least one agent tool installed and signed in: [Claude Code](https://claude.com/claude-code)
  or [Codex](https://github.com/openai/codex). Thread names and a few other small jobs use
  Claude.
- Not notarized by Apple. The one-line install below opens without a warning; a copy
  downloaded by hand needs **Open Anyway** once.
- The iPhone companion is source only (`apps/interface-ios`, built with Xcode); there is no
  App Store build.
- No support is promised. Bug reports are welcome as issues; outside code changes are not
  accepted ([CONTRIBUTING](CONTRIBUTING.md)).

## Install

Open Terminal (in Applications > Utilities), paste this line and press Return:

```sh
curl -fsSL https://raw.githubusercontent.com/KilianTheMagnificent/interface/main/install.sh | bash
```

[The script](install.sh) downloads the latest release, checks its checksum and that it is
signed by the maintainer, puts `Interface.app` into Applications and opens it. Interface
then keeps itself up to date: Settings > Updates offers **Restart to update** when a newer
release is out ([how updates work](docs/PUBLIC_UPDATES.md)).

### Installing by hand

Interface is not notarized by Apple (that needs a paid developer account), so macOS refuses
to open a copy downloaded with a browser until you allow it once:

1. Download `Interface-<version>-arm64-mac.zip` from the
   [latest release](https://github.com/KilianTheMagnificent/interface-releases/releases/latest).
2. Unzip it and drag `Interface.app` into Applications.
3. Open Interface. macOS refuses the first time. Go to System Settings > Privacy & Security,
   scroll down and click **Open Anyway**.

## What an agent can do on your Mac

Agents run commands and edit files in the folders you open. Each one asks before commands
and edits according to the permission mode you give its thread.

- A folder you have never used is not trusted: until you answer **Trust this folder?**, a
  Claude thread there runs without the folder's own settings and hooks.
- Agents ask before a web search and before acting on another pane, unless you switch that
  on.
- **Bypass** mode turns the asks off for a thread. Use it only in folders whose contents
  you trust.
- Use only provider accounts you pay for. Each provider's terms still apply.

Opening a project someone else gave you is the main risk with any coding agent: its files
can carry instructions aimed at the agent. Interface narrows what such a project can do
without your say; it cannot make an untrusted project safe to run agents in.

## What leaves the Mac

Interface has no analytics, tracking or crash reports. Settings > Privacy lists everything
it sends on its own:

- **Your prompts** go to the provider of the agent you send them to, under your own account.
- **Thread names.** The first message of each new thread goes to Claude Haiku, through your
  Claude account, to name the thread.
- **Pull requests.** Interface runs `gh` with your GitHub account to follow a thread's pull
  request.
- **Update checks.** About every four hours Interface asks GitHub whether a newer version
  exists. Nothing about you is sent.
- **Phone access** is off until you turn it on. While it is on, the Mac announces itself on
  your network so the iPhone app can find it; what it carries is encrypted.
- **Other tools' settings.** When Muse, Antigravity or Kimi Code is installed, Interface
  adds a small plugin to it so its panes can report what they are doing.

## Building from source

Needs Rust 1.88 (pinned in `rust-toolchain.toml`), Node.js with npm, and Xcode's command
line tools, on an Apple Silicon Mac.

```sh
export PATH="$HOME/.cargo/bin:$PATH"
npm --prefix electron install
npm --prefix electron run dev    # builds the Rust host, then runs the app from this checkout
cargo xtask check                # the full test gate
```

`npm run dev` runs beside an installed Interface with its own profile and data folder, and
with updates off; it also puts an `Interface Dev.app` launcher into Applications. Renderer
edits hot-swap; main-process edits restart Electron; Rust host edits need
`cargo build -p interface-host` in another terminal. Building from source has only been
done on the maintainer's Mac.

Packaging, release and publishing commands (`cargo xtask package-mac`, `promote`,
`publish-update`) sign with the maintainer's Apple team and publish to his update feed
(`crates/interface-updater/release-channel.json`), so they only work for him. A build of
your own needs its own bundle identifier, signing certificate and feed; that path is
untested.

### How the code is laid out

- `electron/`: the window. Electron main and preload, and the React renderer. It draws
  state and translates messages; it owns no data.
- `apps/interface-host`: the Rust process the window talks to, over a versioned protocol
  ([protocol facts](docs/PROTOCOL_FACTS.md)).
- `crates/`: everything the app decides: sessions, threads, terminals, approvals, storage,
  the updater, one adapter per provider.
- `apps/iface`: the terminal program over the same crates.
- `apps/interface-ios`, `deploy/`: the iPhone app and the optional relay it can use.
- `xtask/`: the test gate and the release tooling.

[docs/CODEMAP.md](docs/CODEMAP.md) is the map, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
the boundary between the window and the Rust host. Interface began as a TypeScript app and
was rewritten around a Rust core, keeping the same window; [docs/PARITY_STATUS.md](docs/PARITY_STATUS.md)
and [docs/MIGRATION.md](docs/MIGRATION.md) record that port.

## How this was built

Andre Romero-Carey directs development with AI coding agents (Claude, Codex and others) and
checks the results by running them, rather than writing Rust or TypeScript by hand. The
`AGENTS.md` files and most of `docs/` are the instructions and records those agents work
from, published as they are.

- Every change has to pass the gate (`cargo xtask check`: the Rust and Electron test
  suites, lints, and contract tests between the two) and names the tests that prove it.
- User-facing changes are then seen working in a hidden copy of the app
  ([hands-free testing](docs/AGENT_HANDSFREE_TESTING.md)).
- Security was reviewed in three separate passes before the source was published
  (September and November 2026). The updater installs only a release signed with the
  maintainer's key and built by the same Apple team as the running app.
- It has run on one Mac daily (Apple Silicon, macOS 27). Behaviour on older macOS versions
  comes from reading the code and from tests, not from those machines. Phone access, the
  relay and the iPhone app have had one user.

## Licence

MIT, see [LICENSE](LICENSE). Code and designs adapted from other projects are listed in
[docs/THIRD_PARTY_NOTICES.md](docs/THIRD_PARTY_NOTICES.md).
