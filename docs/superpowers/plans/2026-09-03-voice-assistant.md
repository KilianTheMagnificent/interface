# Voice Assistant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A built-in voice assistant: toolbar toggle, on-device speech in and out through a Swift helper, a dedicated Claude session as the brain, and MCP tools that let it see and operate Interface.

**Architecture:** Rust host spawns `interface-voice` (Swift, Apple frameworks only) over NDJSON stdio, creates/reuses a `SessionKind::Voice` Claude session through the same transaction path the renderer uses, taps assistant text deltas, chunks them into sentences, and speaks them. App-control tools are served over a per-session Unix-socket MCP server owned by the host; UI tools round-trip to the renderer through one push + one request. Renderer shows a toggle, an indicator, and the normal session transcript.

**Tech Stack:** Rust (tokio, serde_json; no new crates), Swift 6 / SwiftPM (Speech, AVFoundation), Electron + React (no new npm packages).

**Spec:** `docs/superpowers/specs/2026-09-03-voice-assistant-design.md`

Plan style note: builders are senior agents working from the fact map below; code blocks are given where the plan fixes a contract (names, wire shapes, prompts, schemas). Everything else says exactly where to look and what the result must be.

## Global Constraints

- No new Rust crate dependencies, no new npm dependencies, no third-party Swift packages. Ask before adding any.
- First-party behaviour in Rust; Electron main/renderer only render state, translate frozen DTOs, and execute UI-bridge calls.
- Adding channels is the atomic multi-file edit (see Fact Map §3). Shared channel count 113→116, pushes 8→10, requests 109→112, `ALL_DESKTOP_CHANNEL_COUNT` 117→121. A mismatch crashes Electron at startup, so both sides must land the same literals.
- `cargo xtask check` must be green before any commit. Commit by pathspec on `feat/voice-assistant` in worktree `/private/tmp/interface-voice-20260903`. Never push to `dev` (owner holds deployment).
- Wire field casing: Rust↔Electron DTOs follow the existing snake_case DTO convention (e.g. `project_root`, `session_key`); helper NDJSON is camelCase (`endSilenceMs`).
- Never read `.env` or credential files. Never touch `~/Desktop/Projects/.interface-dev` or `/Applications/Interface.app`.

## Fact Map (verified 2026-09-03 on the worktree)

1. Session creation: `iface:createSession` → `apps/interface-host/src/dispatcher.rs:1303` → `create_session_transaction` (`dispatcher.rs:1072`) → `RuntimeBridgeCommand::StartSession` → `SessionRuntime::start_session` (`crates/interface-runtime/src/runtime.rs:856`) → `ClaudeAdapter::create_session` (`crates/engine-claude/src/adapter.rs:585`). Wire `LaunchOptions` is `crates/interface-contracts/src/engine.rs:587` (`deny_unknown_fields`; has `cwd, model, effort, name, allowed_tools, disallowed_tools, kind: Option<SessionKind>`; no system-prompt or MCP field). `append_system_prompt` is set only in `apply_session_kind` (`adapter.rs:1807-1822`) keyed on `SessionKind` (`engine.rs:539`, list `SESSION_KINDS` at `:545`). `runtime.send` `:1230`, `cancel_session` (= interrupt) `:1234`, `take_session_events` `:1190` is single-consumer. Assistant deltas: `EngineEventKind::MessageDelta { text, agent_key }` (`engine.rs:917`) from `crates/engine-claude/src/mapper.rs:268-286`. Observation seam without owning the stream: `CollaborationSessionHooks::observe_event` (`apps/interface-host/src/collaboration.rs:194`) called at `dispatcher.rs:1443`, installed by `with_collaboration_hooks` (`dispatcher.rs:529`, one slot). Owning-stream template: `orchestration.rs:1500` `consume_agent_events` + `push_desktop_event` `:1382`.
2. MCP: `--mcp-config` built at `crates/engine-claude/src/launch.rs:268-284` from `mcp_servers: BTreeMap<String, McpServer>`; `McpServer::as_wire_value` `:105-115` → `{"type":"stdio","command","args","env"}`. Populated only in `prepare_collaboration_binding` (`adapter.rs:927`, early-returns for Chat `:933`); descriptor built at `crates/interface-collaboration/src/broker_runtime.rs:219-228` with command `/usr/bin/nc` args `["-U", socket_path]`. Socket root `<data_root>/collaboration/sockets/<id>.sock`, max path 103 bytes (`mcp.rs:56`). JSON-RPC server shape to copy: `serve_connection` (`crates/interface-collaboration/src/mcp.rs:584`), `handle_frame` `:689`, `run_request` `:886` (initialize / tools/list / tools/call), `tool_definition` `:2180-2202`. `ClaudeAdapter` has one factory slot (`adapter.rs:96`, `with_collaboration_factory` `:148`).
3. Channel edit set: `crates/interface-contracts/src/ipc.rs` (table `:1008-1154`, pushes last `:1148-1153`, `EXTRA_CHANNELS` `:1161`, goldens `:1240-1242`); `apps/interface-host/src/protocol.rs` (`PUSH_OPERATIONS: [&str; 8]` `:194`, `PushFrame::new` `:146`, tests `:239-242,:273`); `apps/interface-host/src/router.rs` (`COMPOSITE_OWNED_OPERATION_COUNT` `:23`, `validate_operation_owners` `:73`, arms `:102-139`, tests `:614-615,:664-673`); owner tables like `terminal.rs:60`; `electron/src/shared/ipc.ts` (`IfaceApi ~:1256`, push subscribers `~:1295`, `CHANNELS :1309`); `electron/src/main/rust-host-client.ts` (`:12-25`); `electron/src/main/ipc-payload.ts` (`:10-19`, encoders `:184-240`, throws `:248-262`); `electron/src/main/preload.cts` (`CH :66-180`, methods `:595-670`). Tests with literals: `apps/interface-host/tests/child_protocol.rs:71-72`, `electron/src/main/ipc-payload.test.ts:12-21`, `rust-host-client.test.ts:23-24,102-104,325`, `rust-ipc-bridge.test.ts:25,39`, `electron/src/shared/ipc.test.ts:55-96`, `electron/scripts/smoke-integrated-terminal.mjs:102-103`. Templates: request `dismissTerminal` (`ipc.rs:1135`, `contracts/terminal.rs:316,322`, `terminal.rs:60-69,2857`, `ipc.ts:1256,1404`, `ipc-payload.ts:228`, `preload.cts:161,607`); push `terminalLifecycle` (`ipc.rs:1153`, `protocol.rs:202`, `terminal.rs:140-158`, `rust-host-client.ts:24`, `ipc-payload.ts:16`, `ipc.ts:449,1295,1422`, `preload.cts:179,669`). Push delivery: `rust-ipc-bridge.ts:59-63` sends on the operation string; preload `subscribe()` `:421-425`.
4. Host owner graph is `apps/interface-host/src/main.rs` (runtime `:186`, orchestration slot `:187-209`, push_sender `:158`, terminal dispatcher `:303`, router `:385`, shutdown `:402-420`). Long-lived NDJSON child template: `crates/engine-claude/src/transport.rs:169-230`. Binary lookup: host `current_exe()` at `bootstrap.rs:692`; Electron `rust-host-path.ts:12-23` (packaged `Resources/bin/`, dev `target/debug/`).
5. Terminal: `TerminalService::write(&self, TerminalGenerationRequest{id,generation}, &[u8])` at `crates/interface-terminal/src/service.rs:1796`; `create` `:1302`. Output pump `forward_output` (`apps/interface-host/src/terminal.rs:3045-3082`), pumps map `output_pumps` keyed `(TerminalId, TerminalGeneration)` (`terminal.rs:688`), pruned by `prune_output_pumps` `:2225`. `iface:listTerminals` → `TerminalSnapshot` (`terminal.rs:2281`); `iface:createTerminal` takes `CreateTerminalRequest` (`contracts/terminal.rs:274`). `TerminalLaunchProfileId` (`contracts/terminal.rs:108`): Shell/Claude/Codex/Grok/Qwen/Gemini(label "Antigravity"), labels `crates/interface-terminal/src/launch.rs:690-710`. Projects: renderer groups by `SessionSummary.project_root` (`contracts/engine.rs:505`, computed by host `crates/interface-runtime/src/project_root.rs`); host sources `runtime.list_sessions()`, `DesktopService::workspace_roots()` (`crates/interface-desktop-services/src/service.rs:601`), `terminal_workspace_roots()` `:611`.
6. Renderer: toolbar `App.tsx:6620-6699` (template Sessions toggle `:6628-6635`), statusbar `:7211-7278` (template `:7239-7251`), CSS `theme.css:2937,3000,3104-3160`; `selectSession(key)` `App.tsx:4437`; `tileConversation` `:6152`. `main.ts`: permission handler slot after `applyCsp(location)` at `:393`. `electron-builder.yml` `mac.extendInfo` `:56-67`; `dev-app.mjs` `setPlistString` `:42-48`, `applyIdentity` `:83-92`, `DEV_APP_LAUNCHER_REVISION` `:17` (bump it).
7. Build: `xtask/src/package.rs` `EXPECTED_BINARIES` `:30`, build step `:73`, `expected_binary_paths` `:367-375` (hardcodes `target/release/<name>`), `run_check` `:138-184`, thin-arm64 digest `:409-415`; `electron-builder.yml` `extraResources` `:17-23`; `electron/scripts/package-parity.test.mjs:25-38`; `dev.mjs:151` builds debug host, `:289-293` watches it.

---

## Contract (fixed by this plan; both sides implement verbatim)

### Channels
Requests: `iface:voiceSetEnabled`, `iface:voiceInterrupt`, `iface:voiceUiResult`. Pushes: `iface:voice`, `iface:voiceUiCall`.

### DTOs (`crates/interface-contracts/src/voice.rs`, mirrored in `electron/src/shared/ipc.ts`)
```rust
pub const VOICE_STATES: [&str; 6] = ["off","starting","listening","thinking","speaking","error"];
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)] #[serde(rename_all = "snake_case")]
pub enum VoiceState { Off, Starting, Listening, Thinking, Speaking, Error }
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(deny_unknown_fields)]
pub struct VoiceAsset { pub status: String /* installed|installing|failed */, pub progress: f64 }
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(deny_unknown_fields)]
pub struct VoiceEvent {
    pub state: VoiceState,
    pub session_key: Option<String>,
    pub cwd: Option<String>,
    pub partial: Option<String>,
    pub level: Option<f64>,
    pub message: Option<String>,
    pub microphone: Option<String>, // granted|denied|undetermined
    pub asset: Option<VoiceAsset>,
}
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(deny_unknown_fields)]
pub struct VoiceSetEnabledRequest { pub enabled: bool, pub cwd: String }
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(deny_unknown_fields)]
pub struct VoiceSetEnabledResponse { pub state: VoiceState, pub session_key: Option<String>, pub message: Option<String> }
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(deny_unknown_fields)]
pub struct VoiceUiCall { pub call_id: String, pub tool: String, pub input: serde_json::Value }
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)] #[serde(deny_unknown_fields)]
pub struct VoiceUiResultRequest { pub call_id: String, pub ok: bool, pub output: String }
```
`iface:voiceInterrupt` has no payload and returns `{}`. `iface:voiceUiResult` returns `{}`. Golden JSON example for `VoiceEvent`:
`{"state":"listening","session_key":"s1","cwd":"/p","partial":null,"level":0.2,"message":null,"microphone":"granted","asset":null}`.

### SessionKind
Add `SessionKind::Voice` (wire `"voice"`) to `crates/interface-contracts/src/engine.rs:539` and `SESSION_KINDS`. `apply_session_kind` appends `VOICE_PROMPT_APPEND` for it and `prepare_collaboration_binding` treats Voice like Chat (no collaboration tools).

### Voice session launch
`LaunchOptions { cwd, kind: Some(Voice), model: Some("sonnet"), effort: Some("low"), name: Some("Voice assistant"), allowed_tools: [read-only voice tools below, fully qualified as `mcp__iface_voice__<name>`], ..Default }`. Reuse rule: on enable, if a session with `kind == Voice` (or title "Voice assistant" when kind is not in the summary) and the same `project_root` exists, resume it; else create.

### `VOICE_PROMPT_APPEND` (`crates/engine-claude/src/adapter.rs`, next to `AGENT_PROMPT_APPEND`)
```text
You are Interface's voice assistant. The user is speaking to you and hears your replies read aloud by a speech synthesizer. Reply in plain spoken sentences: no markdown, no bullet lists, no headings, no code blocks, and do not spell out URLs or paths character by character. Keep replies to one to three sentences unless the user asks for detail. Transcripts come from speech recognition and may contain errors; infer the intent, and ask one short question only when it is genuinely ambiguous. You can see and operate the Interface app through the iface_voice tools: call app_state before acting on panes or sessions, confirm before starting, closing, or sending commands to a pane, and after acting say briefly what you did. Keep narration around tool use short.
```

### Helper NDJSON protocol
As in the spec (`protocol: 1`). Rust types in `crates/interface-voice/src/protocol.rs`:
```rust
#[derive(Serialize, Debug, Clone)] #[serde(tag = "type", rename_all = "camelCase")]
pub enum HostToHelper {
    #[serde(rename_all = "camelCase")] Start { end_silence_ms: u32, locale: String },
    Stop, Speak { id: String, text: String }, Cancel, SetVoice { voice: String }, Shutdown,
}
#[derive(Deserialize, Debug, Clone, PartialEq)] #[serde(tag = "type", rename_all = "camelCase")]
pub enum HelperToHost {
    #[serde(rename_all = "camelCase")] Ready { protocol: u32, voices: Vec<VoiceInfo>, default_voice: String },
    Permission { microphone: String },
    Asset { status: String, #[serde(default)] progress: f64 },
    State { listening: bool, speaking: bool },
    Partial { text: String }, Final { text: String },
    SpeechStarted { id: String }, SpeechFinished { id: String }, SpeechCancelled { id: String },
    Interrupted, Error { message: String, #[serde(default)] fatal: bool }, Level { rms: f64 },
    #[serde(other)] Unknown,
}
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)] pub struct VoiceInfo { pub id: String, pub name: String, pub quality: String, pub language: String }
```
Helper location: `INTERFACE_VOICE_HELPER` env override, else `<current_exe dir>/interface-voice`.

### MCP server `iface_voice` (host-owned, per voice session, `/usr/bin/nc -U <socket>` stdio bridge like collaboration)
Tools and JSON Schemas (`additionalProperties: false`; `read_only` annotation as noted). Results are `text` content unless stated.

| name | input | read-only | result |
|---|---|---|---|
| `app_state` | `{}` | yes | JSON `{active_project, projects:[{root,name,session_keys[],pane_ids[]}], sessions:[{session_key,title,engine,cwd,project_root,status,kind}], panes:[{pane_id,generation,profile,label,cwd,title,waiting}]}` |
| `list_panes` | `{}` | yes | JSON panes array as above |
| `start_pane` | `{profile: "shell"\|"claude"\|"codex"\|"grok"\|"qwen"\|"antigravity", cwd: string, title?: string}` | no | JSON pane summary |
| `send_to_pane` | `{pane_id: string, text: string, enter?: boolean (default true)}` | no | `{"ok":true}` |
| `read_pane` | `{pane_id: string, lines?: integer (default 60, max 400)}` | yes | ANSI-stripped tail text |
| `close_pane` | `{pane_id: string}` | no | `{"ok":true}` |
| `list_sessions` | `{}` | yes | JSON sessions array |
| `read_session` | `{session_key: string, last_n?: integer (default 10, max 50)}` | yes | text: last N turns as `user:`/`assistant:` lines |
| `send_to_session` | `{session_key: string, text: string}` | no | `{"ok":true}` |
| `notify` | `{title: string, body: string}` | no | `{"ok":true}` |
| `ui_snapshot` | `{}` | yes | text tree (format below) |
| `ui_act` | `{id: string, action: "click"\|"type"\|"key"\|"focus", text?: string}` | no | `{"ok":true}` or error text |
| `ui_screenshot` | `{}` | yes | `image` content (PNG base64) + text `captured <w>x<h>` |
| `open_project` | `{root: string}` | no | `{"ok":true}` |
| `focus_pane` | `{pane_id: string}` | no | `{"ok":true}` |

`ui_*`, `open_project`, `focus_pane` are forwarded to the renderer via push `iface:voiceUiCall {call_id, tool, input}`; the host awaits `iface:voiceUiResult {call_id, ok, output}` with a 10 s timeout (error text on timeout). `ui_screenshot` is answered by Electron main (`webContents.capturePage`), the others by the renderer. `ui_snapshot` output format, one element per line, two-space indent per DOM depth, capped at 400 lines:
```
[v12] button "Sessions" pressed
[v13] tab "Hegemon" selected
[v20] textbox "Message Claude" value=""
[v31] terminal "Claude Code — Hegemon" waiting
    <last 30 lines of that terminal's xterm buffer, indented>
```
Ids come from a `data-voice-id` attribute assigned from a module counter at snapshot time; `ui_act` on an id not present in the latest snapshot returns `unknown id; call ui_snapshot again`. `click` = `element.focus(); element.click()`; `type` = focus + React-compatible value set (native value setter + `input` event) on input/textarea only; `key` = dispatch `keydown`/`keyup` with the given key on the focused element; `focus` = `element.focus()`. Hidden/inert/aria-hidden subtrees are skipped.

---

### Task 1: Swift helper `apps/interface-voice`
Already in progress by a separate builder (protocol above, `--selftest`). Deliverable: `xcrun swift build -c release` succeeds; `README.md` documents protocol and selftest.

### Task 2: `crates/interface-voice` pure library
**Files:** Create `crates/interface-voice/Cargo.toml` (serde, serde_json only; workspace member in root `Cargo.toml`), `src/lib.rs`, `src/protocol.rs` (types above), `src/chunker.rs`, `src/speakable.rs`, `src/tail.rs`.
**Interfaces produced:**
- `SentenceChunker::new(min_chars: usize) -> Self`; `push(&mut self, delta: &str) -> Vec<String>` returns completed sentences (split at `.`, `!`, `?` followed by whitespace/end, or at `\n`; never emits a chunk shorter than `min_chars` unless followed by a newline); `flush(&mut self) -> Option<String>`.
- `speakable(text: &str) -> String`: strips `**`, `*`, `_` emphasis, `#` headings, backticks, list markers (`- `, `* `, `1. `), converts `[text](url)` to `text`, replaces fenced code blocks with `code omitted`, collapses whitespace.
- `TailBuffer::new(capacity: usize)`, `push(&mut self, bytes: &[u8])`, `tail_lines(&self, n: usize) -> String` (ANSI CSI/OSC sequences stripped, `\r` handled as line overwrite, `\x07` dropped).
**Tests (in-file `#[cfg(test)]`):** chunker splits `"Hello there. How are"` + `" you? Fine"` + flush into `["Hello there.", "How are you?", "Fine"]`; min_chars holds `"Yes. I did it."` as one chunk when min_chars=20; speakable turns `"**Done.** See `foo.rs`.\n\n```rust\nlet x=1;\n```"` into `"Done. See foo.rs. code omitted"`; tail buffer drops oldest bytes at capacity and strips `\x1b[31mred\x1b[0m` to `red`; protocol round-trips `{"type":"speechStarted","id":"u1"}` and serialises `Start` as `{"type":"start","endSilenceMs":800,"locale":"en-US"}`; unknown helper message decodes to `Unknown`.
Steps: write tests → `cargo test -p interface-voice` fails → implement → passes → commit `feat(voice): add interface-voice protocol, chunker, and tail buffer`.

### Task 3: Contracts — `SessionKind::Voice`, voice DTOs, channels
**Files:** Modify `crates/interface-contracts/src/engine.rs:539-560` (add `Voice`, extend `SESSION_KINDS`, fix any exhaustive match/tests), create `crates/interface-contracts/src/voice.rs` (DTOs above, `pub mod voice` in `lib.rs`), modify `crates/interface-contracts/src/ipc.rs` (three requests in the request block, two pushes at the end of the push block, `ALL_CHANNELS` 116, `ALL_DESKTOP_CHANNEL_COUNT` 121, goldens at `:1240-1242`).
**Tests:** golden round-trip for `VoiceEvent` (JSON above) in `voice.rs`; existing `ipc.rs` tests updated to the new counts and names; `SessionKind` wire test includes `"voice"`.
Commit `feat(contracts): add voice session kind, DTOs, and channels`.

### Task 4: engine-claude — voice prompt and MCP attachment
**Files:** Modify `crates/engine-claude/src/adapter.rs` (`VOICE_PROMPT_APPEND` const near `:63`; `apply_session_kind` `:1807-1822` sets `append_system_prompt` for Voice; `prepare_collaboration_binding` `:927-940` returns early for Voice too); add a second, voice-only MCP attachment: `ClaudeAdapter::with_voice_mcp(provider: Arc<dyn VoiceMcpProvider>)` where `pub trait VoiceMcpProvider: Send + Sync { fn descriptor_for(&self, session_key: &str, cwd: &Path) -> Option<McpServer>; }` and `launch_configuration` inserts it under key `"iface_voice"` only when `kind == Voice`. Keep the renderer unable to inject sockets: the provider is installed by the host in `main.rs`.
**Tests:** `build_spec` test asserting a Voice launch carries `--mcp-config` with `iface_voice` and `--append-system-prompt` containing "voice assistant"; a Chat launch carries neither. `cargo test -p engine-claude`.
Commit `feat(engine-claude): voice session prompt and MCP attachment`.

### Task 5: Host — tail buffer, voice service, MCP server, UI bridge, router wiring
**Files:** Modify `apps/interface-host/src/terminal.rs` (`forward_output` `:3045-3082` gets a `&TailSink`; `TerminalHostDispatcher` gains `output_tails: Mutex<BTreeMap<(TerminalId, TerminalGeneration), TailBuffer>>` (64 KiB each), pruned alongside `prune_output_pumps` `:2225`; `pub(crate) fn read_tail(&self, id, generation, lines) -> Option<String>`; `pub(crate)` entry points for create/write/close/list used by tools), modify `apps/interface-host/src/dispatcher.rs` (make `create_session_transaction`, the `iface:send` transaction, the resume transaction, and interrupt callable from the voice module as `pub(crate)`; replace the single hooks slot with a small composite so `observe_event` fans out to collaboration and voice), create `apps/interface-host/src/voice/{mod.rs,helper.rs,session.rs,tools.rs,mcp.rs,ui_bridge.rs}`, create `apps/interface-host/src/voice_owner.rs` or fold the owner table into `voice/mod.rs` (`OWNED_OPERATIONS: [&str; 3]`), modify `apps/interface-host/src/protocol.rs` (`PushFrame` variants `Voice(VoiceEvent)`, `VoiceUiCall(VoiceUiCall)`; `PUSH_OPERATIONS: [&str; 10]`; tests), `router.rs` (`COMPOSITE_OWNED_OPERATION_COUNT` 112, owner validation, arms), `main.rs` (construct `VoiceService` after the runtime, install `with_voice_mcp` provider on the Claude adapter, register the owner, shutdown in the reverse block), `apps/interface-host/tests/child_protocol.rs` (counts; new case).
**Interfaces produced:** `VoiceService::new(deps) -> Arc<Self>`, `async fn set_enabled(&self, VoiceSetEnabledRequest) -> VoiceSetEnabledResponse`, `async fn interrupt(&self)`, `fn ui_result(&self, VoiceUiResultRequest)`, `async fn shutdown(&self)`.
**Behaviour:**
- `helper.rs`: spawn helper (path rule above) with piped stdio using the `transport.rs:169-230` pattern; writer/reader/stderr/supervise tasks; `send(HostToHelper)`; events delivered on an mpsc; one automatic restart on unexpected exit, then state `error`.
- `session.rs`: state machine Off→Starting→Listening→Thinking→Speaking with pushes of `VoiceEvent` on every change (partials and levels pushed while listening, throttled to 10 Hz). On `Final{text}`: send user turn through the dispatcher transaction so the transcript shows it; state Thinking. On `MessageDelta` for the voice session key: chunker → `speakable` → `Speak{id}` with ids `u<n>`; first `SpeechStarted` → Speaking. On turn end (the engine's end-of-turn event, whatever `mapper.rs` emits for `result`): flush chunker. On last `SpeechFinished` with an empty queue → Listening. On `Interrupted` or a `Final` while Thinking/Speaking: `Cancel`, dispatcher interrupt, clear queue, then treat the text as the next turn. Disable: `Stop` + `Shutdown`, wait ≤2 s, kill, push Off. Mic `denied` → Error "Microphone access denied. Allow Interface in System Settings → Privacy & Security → Microphone."; `Asset installing` → Starting with progress.
- `mcp.rs`: Unix socket at `<data_root>/voice/sockets/<session-id>.sock` (path ≤103 bytes; error otherwise), JSON-RPC shaped exactly like `collaboration/src/mcp.rs` `serve_connection`/`run_request` (initialize, tools/list, tools/call; protocol version string copied from there), tool catalogue from the table; `VoiceMcpProvider` implementation returns `McpServer{command:"/usr/bin/nc", args:["-U", path]}`.
- `tools.rs`: implementations against `SessionRuntime`, `DesktopService`, `TerminalHostDispatcher`; `start_pane` maps profile names to `TerminalLaunchProfileId` (`antigravity` → `Gemini`) and builds `CreateTerminalRequest` the way `iface:createTerminal` does (default size 120x40, fresh id); `send_to_pane` appends `\r` when `enter`; `read_pane` uses `read_tail`; `notify` uses the existing OS-notification push if one exists, else pushes `iface:event`-independent `VoiceEvent{message}` (state unchanged).
- `ui_bridge.rs`: `call(tool, input) -> Result<String, String>`: allocate `call_id`, push `VoiceUiCall`, await oneshot with 10 s timeout; `ui_result` resolves it. `ui_screenshot` result text is the base64 PNG; `mcp.rs` wraps it as an `image` content block.
**Tests:** unit tests for the state machine transitions with a fake helper channel (Final → Thinking, deltas → Speak ids in order, Interrupted clears queue); `child_protocol.rs` case: with `INTERFACE_VOICE_HELPER` pointing at a fake script that prints `ready`, then `state`, then `final` "hello" after 200 ms, drive `iface:voiceSetEnabled{enabled:true,cwd}` and assert a `iface:voice` push with `state:"starting"` then `"listening"` arrives, then `iface:voiceSetEnabled{enabled:false}` → `"off"`. Run `cargo test -p interface-host`.
Commit `feat(host): voice service, MCP tools, and UI bridge`.

### Task 6: Electron main/preload/shared — channels and screenshot
**Files:** Modify `electron/src/shared/ipc.ts` (DTO types mirroring Task 3, `IfaceApi.voiceSetEnabled/voiceInterrupt/voiceUiResult`, `onVoice`, `onVoiceUiCall`, `CHANNELS` entries), `electron/src/main/rust-host-client.ts` (`RUST_HOST_REQUEST_OPERATIONS` 112, `RUST_HOST_PUSH_OPERATIONS` 10, push set), `electron/src/main/ipc-payload.ts` (push set, `addNamed(CHANNELS.voiceSetEnabled, "enabled", "cwd")`, `addEmpty(CHANNELS.voiceInterrupt)`, `addNamed(CHANNELS.voiceUiResult, "call_id","ok","output")`, throws), `electron/src/main/preload.cts` (`CH` entries, methods, subscriptions), `electron/src/main/rust-ipc-bridge.ts` (intercept push `iface:voiceUiCall` with `tool === "ui_screenshot"`: `capturePage()` → PNG base64 → `voiceUiResult`; forward all other calls to the renderer), `electron/src/main/main.ts` (after `applyCsp(location)` `:393`: `session.setPermissionRequestHandler` allowing only `media` with `mediaTypes` containing `audio` for the app origin, denying everything else — must not contain the literals `main-wiring.test.ts` orders on), tests `ipc-payload.test.ts`, `rust-host-client.test.ts`, `rust-ipc-bridge.test.ts`, `shared/ipc.test.ts`, `electron/scripts/smoke-integrated-terminal.mjs:102-103`.
**Tests:** each existing count/name test updated; new `rust-ipc-bridge.test.ts` case that a `voiceUiCall` for `ui_screenshot` is answered from main and one for `ui_snapshot` is forwarded. `npm --prefix electron run check`.
Commit `feat(electron): voice channels, permission handler, screenshot bridge`.

### Task 7: Renderer — toggle, indicator, session selection, UI-bridge executor
**Files:** Modify `electron/src/renderer/App.tsx` (toolbar toggle after the Sessions toggle `:6628-6635`, codicon `mic`/`unmute`, `aria-pressed`, label "Voice assistant"; statusbar item after `:7239-7251` showing state text and a 5-bar level meter while listening, partial transcript text while listening, error message on error; `voiceSetEnabled({enabled, cwd: active project root or home})` on click; on response/push with `session_key` call `selectSession`; subscribe `onVoice` in the existing subscription effect), create `electron/src/renderer/voice-ui.ts` (`snapshot(root: Document): string`, `act(id, action, text?): {ok, message?}` implementing the format/rules in the Contract; `open_project` → the same state change the sidebar project click performs; `focus_pane` → the same call the pane click performs; wired from `onVoiceUiCall` in `App.tsx` and answered via `voiceUiResult`), `theme.css` (`.voice-indicator`, `.is-listening/.is-thinking/.is-speaking/.is-error`, meter bars), tests `electron/src/renderer/voice-ui.test.ts` (snapshot of a small DOM fixture yields the exact lines; `act` on a stale id returns the exact error; `type` sets a React-controlled input value), `workspace-ui.test.ts:471,498` source-text updates.
Commit `feat(renderer): voice toggle, indicator, and UI bridge`.

### Task 8: Build and packaging
**Files:** Modify `xtask/src/package.rs` (after the cargo build at `:73`: run `xcrun swift build -c release --package-path apps/interface-voice` and copy `.build/release/interface-voice` to `target/release/interface-voice` (thin arm64); `EXPECTED_BINARIES` becomes `["interface-host","iface","interface-voice"]`; `run_check` `:138-184` gains the Swift build (build only, no selftest) before `cargo test`, failing with a clear message if `xcrun` is missing; unit test `:727`), `electron/electron-builder.yml` (third `extraResources` entry `bin/interface-voice`; `NSMicrophoneUsageDescription: "Interface uses the microphone for the voice assistant."` in `mac.extendInfo`), `electron/scripts/package-parity.test.mjs:25-38` (three entries), `electron/scripts/dev.mjs:151` (also `xcrun swift build --package-path apps/interface-voice` and copy `.build/debug/interface-voice` to `target/debug/`), `electron/scripts/dev-app.mjs` (`setPlistString(... "NSMicrophoneUsageDescription", ...)` in `applyIdentity`; bump `DEV_APP_LAUNCHER_REVISION` to 4).
**Tests:** `cargo test -p xtask`; `npm --prefix electron run check`; `cargo xtask check` end to end.
Commit `build: bundle interface-voice helper and microphone usage description`.

### Task 9: Integration
- Merge all task branches/commits on `feat/voice-assistant`; `cargo xtask check` green; handshake literals equal on both sides.
- `apps/interface-voice/.build/release/interface-voice --selftest` passes.
- Manual smoke (owner or a session with a display): toggle on, speak, hear reply, barge-in, `start_pane` + `read_pane` via the assistant, `ui_snapshot` + `ui_act`, toggle off. Record what was and was not run in the session note.
- Do not push to `dev`.
