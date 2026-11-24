# BridgeMind One and Claude Desktop stack research

Captured 2026-08-27 from official product documentation and directly inspected
shipping artifacts. Product behavior and observable package facts are treated
as evidence; undocumented private implementation details are not inferred.

## Executive finding

BridgeMind and Claude converge on a multi-pane desktop control surface over
agent engines, but their current shells differ:

- BridgeMind One 0.1.11 is a universal native macOS Swift application using
  SwiftUI/AppKit and local native frameworks.
- Claude Desktop 1.37937.3 is an Electron application whose package metadata
  identifies Vite, React, native Node/Swift helpers, PTY support, MCP runtimes,
  and the Claude Agent SDK.

Interface therefore keeps Electron/React and ports the existing operational
core to a supervised Rust host. This follows Claude's observable desktop-shell
shape without adding BridgeMind features during the parity migration. It does
not claim to reproduce either company's private architecture.

## BridgeMind product model

BridgeMind documents three top-level work contexts: persistent Agent identity,
project-scoped Code work, and standalone Chat conversations. An Agent contains
Name, Brief, Memory, Skills, approved Places, and Chats; Claude Code or Codex is
an engine for that teammate rather than its identity. See the official
[Agent Mode documentation](https://docs.bridgemind.ai/docs/agent-mode).

Code Mode is organized around a project and exposes Terminal, Thread, Files,
Browser, and Simulator panes. Terminals are real interactive shells; Claude and
Codex receive structured thread adapters, while other detected CLIs can run in
terminal panes. Solo, Pair, Workbench, and Swarm presets show a reviewed lineup
before launch. See [Code Mode](https://docs.bridgemind.ai/docs/code-mode).

Skills are reusable procedures. Connections are app-managed separately from
per-agent capability grants, and sensitive writes or spending can require an
approval card. See [Skills and Plugins](https://docs.bridgemind.ai/docs/skills-and-plugins).
Routines run only while the app is open and create a fresh agent chat for each
run rather than contaminating an unrelated thread. See
[Routines](https://docs.bridgemind.ai/docs/routines).

The macOS documentation says engines are detected from the normal login shell,
are not bundled, and only Claude Code and Codex currently have chat adapters.
See [BridgeMind One for macOS](https://docs.bridgemind.ai/docs/macos).

## BridgeMind artifact evidence

The official universal DMG was downloaded from
`https://downloads.bridgemind.ai/bridgemind-one/latest/macos/BridgeMindOne-universal.dmg`.
At inspection it contained BridgeMind One 0.1.11 (`ai.bridgemind.one`), a valid
universal x86_64/arm64 code signature, and SHA-256
`3e428c46202c20c814bd2a1ee4149c22edb25b75b04014a6f042a93746981408`.

The application is a Mach-O executable, not an Electron bundle. Dynamic linkage
and resource metadata directly show SwiftUI, AppKit, Combine, WebKit, SQLite,
Swift concurrency, SwiftTerm, Charts, PDFKit, AVFAudio, Sparkle 2, and PostHog.
Its only declared application entitlement was audio input; no App Sandbox
entitlement was present. These are observable facts about version 0.1.11, not a
claim about future releases or internal source organization.

## Claude Desktop product model

Anthropic documents the Code tab as a graphical surface over the same
underlying Claude Code engine. Sessions have independent histories and changes;
local Git sessions receive worktree isolation; panes include chat, diff,
browser, terminal, and file editor; environments can be local, cloud, or SSH.
Settings, `CLAUDE.md`, MCP servers, hooks, and skills are shared with the CLI.
See [Claude Code Desktop](https://code.claude.com/docs/en/desktop) and
[worktree isolation](https://code.claude.com/docs/en/worktrees).

The Agent SDK documentation makes the engine boundary explicit: an agent loop
selects tools, while permission modes, allow/deny rules, hooks, and approval
callbacks decide whether effects run. Read-only tools may execute concurrently;
state-changing tools are serialized by default. See
[the agent loop](https://code.claude.com/docs/en/agent-sdk/agent-loop) and
[permission evaluation](https://code.claude.com/docs/en/agent-sdk/permissions).
MCP servers are external capabilities transported over stdio, HTTP, or SSE and
remain permission-scoped. See [Agent SDK MCP](https://code.claude.com/docs/en/agent-sdk/mcp).

## Claude artifact evidence

The locally installed `/Applications/Claude.app` was version 1.37937.3 with
bundle identifier `com.anthropic.claudefordesktop`. Its bundle directly contains
`Electron Framework.framework` and `Resources/app.asar`.

The packaged application metadata identifies an Electron Forge/Vite build with
React and React DOM, the Claude Agent SDK, MCP packages, `node-pty`, Playwright,
worker processes for PTY/MCP/file indexing, and native Node bindings including a
Swift helper. This supports an Electron presentation/process-orchestration shell
with native helpers and the Claude Code engine beneath it. It does not establish
the design of Anthropic's unshipped source or cloud services.

## Decision for the parity port

1. Preserve the existing Electron/React renderer and its mature panes.
2. Reduce Electron main/preload to lifecycle, supervision, and a frozen typed
   bridge.
3. Put every canonical store, policy decision, provider/PTY effect, scheduler,
   worktree operation, and recovery path in Rust.
4. Preserve the source release's existing Claude/Codex structured adapters and
   terminal/ACP behavior exactly; do not add engines, modes, or panes.
5. Retain Interface's effect-certainty, idempotency, review, and
   corruption-recovery rules instead of weakening them for visual parity.
6. Defer BridgeMind-style teammates, memory, skills, Places, routines, plugin
   capabilities, Agent/Chat modes, and lineup presets until after the Rust port
   passes end-to-end parity acceptance.
