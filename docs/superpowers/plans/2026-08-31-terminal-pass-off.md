# Terminal Pass Off Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From a terminal pane's `⋯` menu, hand the pane's work to a new pane running a different agent CLI: the new agent starts in the source agent's live working directory with a host-templated prompt pointing at a host-written transcript file.

**Architecture:** The renderer captures the source pane's rendered xterm transcript and sends it as a new optional `handoff` field on the existing `createTerminal` request (no new IPC channel). The Rust terminal service resolves the source child's live cwd (`proc_pidinfo`), authorizes it under a narrow inside-a-root rule, writes the transcript to `temp_dir()/Interface/handoffs/`, and appends one positional prompt argument to the host-built argv. Spec: `docs/superpowers/specs/2026-08-31-terminal-pass-off-design.md` (read it first).

**Tech Stack:** Rust (serde contracts, libc proc_pidinfo), TypeScript/React renderer, vitest, cargo test.

**Spec:** `docs/superpowers/specs/2026-08-31-terminal-pass-off-design.md`

## Global Constraints

- Work ONLY in `~/Desktop/Projects/Interface-Rust/.worktrees/top-bar` (branch `port/top-bar`). Never touch the repo root checkout or other worktrees.
- `cargo` is at `~/.cargo/bin/cargo`. The JS gate runs from `electron/` (`npm run check`); the canonical gate is `cargo xtask check`.
- No new dependencies. libc is already a workspace dependency (0.2.189 locked).
- All contract structs use `#[serde(rename_all = "camelCase", deny_unknown_fields)]`; TS mirrors live in `electron/src/shared/ipc.ts`. The 109-operation channel surface must not change size.
- Handoff is transient: never in `CreateFingerprint`, `TerminalSummary`, or `TerminalWorkspaceTerminal`.
- Source-text test pins you must not break (`electron/`):
  - `TerminalPrimitives.test.ts:673-698` — the `split` callback in `IntegratedTerminalCenter.tsx` must stay byte-identical, ending `}, [activeCwd, createTerminal, report]);`, and the center must NOT contain the substring `terminal.launch`.
  - `terminal-redraw-integration.test.ts` slices center source between `const kickRedraw = useCallback` and `const insertPaths = useCallback` — do not insert new code between those two callbacks.
  - `TerminalLauncher.test.ts:338-339` — the center must NOT contain `"claude --"` or `"codex --"`.
  - `status-waiting-allowlist.test.ts` — exactly one `--status-waiting` use in `terminal.css` (do not add amber).
  - `TerminalPrimitives.test.ts:139-147` — header action order pins; `:253-256` — `TerminalPaneNewMenu` with 3 profiles renders exactly 3 `role="menuitem"`.
- The repo-root checkout has another agent's uncommitted changes to `launch.rs`/`service.rs` (codex resume fix). Do not copy or anticipate them; merge conflicts are handled later by the owner's flow.
- Commit after every task with a conventional message; end commit messages with:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01TLJjxmCSmaRa6M83aLh5ci`

---

### Task 1: Contract — `TerminalHandoffV1` on both sides of the wire

**Files:**
- Modify: `crates/interface-contracts/src/terminal.rs` (near `CreateTerminalRequest`, ~L252)
- Modify: `electron/src/shared/ipc.ts` (near `CreateTerminalRequest`, ~L393)
- Test: `crates/interface-contracts/src/terminal.rs` (inline `#[cfg(test)]` alongside existing ones — check the file tail for the existing test module and match its style)

**Interfaces:**
- Produces (Rust): `pub struct TerminalHandoffV1 { pub version: u8, pub source_terminal_id: TerminalId, pub transcript: String, pub from_title: Option<String> }`, `pub const TERMINAL_HANDOFF_VERSION: u8 = 1`, `pub const TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES: usize = 512 * 1024`, `pub const TERMINAL_HANDOFF_TITLE_MAX_BYTES: usize = 200`, and `CreateTerminalRequest.handoff: Option<TerminalHandoffV1>`.
- Produces (TS): `interface TerminalHandoffV1 { version: number; sourceTerminalId: TerminalId; transcript: string; fromTitle?: string }` and `CreateTerminalRequest.handoff?: TerminalHandoffV1`.

- [ ] **Step 1: Write the failing Rust test** (in the existing test module of `terminal.rs`):

```rust
#[test]
fn create_terminal_request_round_trips_handoff() {
    let json = serde_json::json!({
        "id": "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e11",
        "cwd": "/tmp/project",
        "handoff": {
            "version": 1,
            "sourceTerminalId": "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12",
            "transcript": "hello",
            "fromTitle": "bridgemind"
        }
    });
    let request: CreateTerminalRequest = serde_json::from_value(json).expect("decode");
    let handoff = request.handoff.expect("handoff");
    assert_eq!(handoff.version, TERMINAL_HANDOFF_VERSION);
    assert_eq!(handoff.transcript, "hello");
    assert_eq!(handoff.from_title.as_deref(), Some("bridgemind"));
    // Unknown fields inside the handoff are refused like everywhere else.
    let bad = serde_json::json!({"version": 1, "sourceTerminalId":
        "0a4763cb-9c3b-4a7e-9a83-1f1f0aa50e12", "transcript": "x", "extra": true});
    assert!(serde_json::from_value::<TerminalHandoffV1>(bad).is_err());
}
```

- [ ] **Step 2: Run it** — `~/.cargo/bin/cargo test -p interface-contracts create_terminal_request_round_trips_handoff` → FAIL (type not defined).
- [ ] **Step 3: Implement.** In `terminal.rs`, directly above `CreateTerminalRequest`:

```rust
pub const TERMINAL_HANDOFF_VERSION: u8 = 1;
pub const TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES: usize = 512 * 1024;
pub const TERMINAL_HANDOFF_TITLE_MAX_BYTES: usize = 200;

/// Transient pass-off input consumed at spawn. It never persists: excluded
/// from the create fingerprint, terminal summaries, and saved workspaces.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalHandoffV1 {
    pub version: u8,
    pub source_terminal_id: TerminalId,
    pub transcript: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_title: Option<String>,
}
```

and add to `CreateTerminalRequest` (after `launch_session`):

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<TerminalHandoffV1>,
}
```

In `electron/src/shared/ipc.ts`, above `CreateTerminalRequest`:

```ts
/** Transient pass-off input consumed at spawn; never persisted or echoed back. */
export interface TerminalHandoffV1 {
  version: number;
  sourceTerminalId: TerminalId;
  transcript: string;
  fromTitle?: string;
}
```

and add `handoff?: TerminalHandoffV1;` to `CreateTerminalRequest`.

- [ ] **Step 4: Run** the Rust test (PASS) and `cd electron && npx tsc -p tsconfig.json --noEmit` (or `npm run check` typecheck portion) to confirm TS compiles.
- [ ] **Step 5: Commit** — `feat(contracts): TerminalHandoffV1 on createTerminal`.

Note: no `ipc-payload.ts` change is needed — `createTerminal` uses the generic `input()` single-object passthrough encoder (`electron/src/main/ipc-payload.ts:43-52,223`), and `preload.cts` forwards the object as-is.

---

### Task 2: `authorize_pass_off` — inside-a-root cwd rule

**Files:**
- Modify: `crates/interface-terminal/src/safety.rs` (add beside `authorize`, ~L143-155)
- Test: same file's test module (check tail; create one matching crate style if absent — other tests for this type live in `crates/interface-terminal/tests/`; put unit tests where `snapshot_directory` tests already are, or add `#[cfg(test)] mod tests` at the file end)

**Interfaces:**
- Produces: `pub(crate) fn authorize_pass_off(&self, target: &Path) -> Result<DirectorySnapshot, ApprovedTerminalRootError>` on `ApprovedTerminalRoots`.

- [ ] **Step 1: Failing test:**

```rust
#[test]
fn pass_off_authorizes_directories_inside_a_root_but_not_outside() {
    let root = tempfile::tempdir().expect("root");
    let nested = root.path().join("wt");
    std::fs::create_dir(&nested).expect("nested");
    let outside = tempfile::tempdir().expect("outside");
    let approved =
        ApprovedTerminalRoots::new([root.path().to_path_buf()]).expect("approved");
    // The exact rule still refuses the nested directory…
    assert!(approved.authorize(&nested).is_err());
    // …the pass-off rule accepts the root itself and directories inside it…
    assert!(approved.authorize_pass_off(root.path()).is_ok());
    assert!(approved.authorize_pass_off(&nested).is_ok());
    // …and still refuses anything outside.
    assert_eq!(
        approved.authorize_pass_off(outside.path()),
        Err(ApprovedTerminalRootError::Unauthorized)
    );
}
```

(If `tempfile` is not already a dev-dependency of this crate, check `crates/interface-terminal/Cargo.toml`; the integration tests under `tests/` already create directories — reuse whatever helper pattern they use instead of adding a dependency.)

- [ ] **Step 2: Run** — `~/.cargo/bin/cargo test -p interface-terminal pass_off_authorizes` → FAIL.
- [ ] **Step 3: Implement** in `impl ApprovedTerminalRoots`:

```rust
    /// Pass-off panes may start in the source process's live cwd. That path is
    /// host-derived (never renderer-named), so the rule is deliberately wider
    /// than `authorize`: the same safety snapshot, accepted when the directory
    /// is an approved root or inside one.
    pub(crate) fn authorize_pass_off(
        &self,
        target: &Path,
    ) -> Result<DirectorySnapshot, ApprovedTerminalRootError> {
        let candidate = snapshot_directory(target)?;
        let Some(approved) = self.roots.iter().find(|approved| {
            **approved == candidate || candidate.path.starts_with(&approved.path)
        }) else {
            return Err(ApprovedTerminalRootError::Unauthorized);
        };
        assert_directory(approved)?;
        Ok(candidate)
    }
```

- [ ] **Step 4: Run** the test → PASS.
- [ ] **Step 5: Commit** — `feat(terminal): pass-off cwd rule accepts directories inside an approved root`.

---

### Task 3: Live process cwd — `proc_cwd.rs` + pid exposure

**Files:**
- Create: `crates/interface-terminal/src/proc_cwd.rs`
- Modify: `crates/interface-terminal/src/lib.rs` (add `mod proc_cwd;` beside the other module declarations)
- Modify: `crates/interface-terminal/src/embedded.rs` — `trait TerminalProcessControl` (L48) gains a defaulted method; the concrete control (`SystemTerminalControl`, impl at L484) returns the pid captured at spawn (`require_process_id(child.process_id())`, ~L676; the struct that retains it is near L118-130 — thread a `process_id: u32` field into `SystemTerminalControl` if it does not already hold one)
- Modify: `crates/interface-terminal/Cargo.toml` — add `libc.workspace = true` if not present
- Test: inline `#[cfg(test)]` in `proc_cwd.rs`

**Interfaces:**
- Produces: `pub(crate) fn live_process_cwd(pid: u32) -> Option<PathBuf>` and `TerminalProcessControl::process_id(&self) -> Option<u32>` (default `None`).

- [ ] **Step 1: Failing test** (in `proc_cwd.rs`):

```rust
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::live_process_cwd;

    #[test]
    fn reads_the_live_cwd_of_a_child_process() {
        let dir = std::env::temp_dir();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .current_dir(&dir)
            .spawn()
            .expect("spawn sleep");
        let cwd = live_process_cwd(child.id()).expect("live cwd");
        let canonical = std::fs::canonicalize(&dir).expect("canonical tmp");
        assert_eq!(std::fs::canonicalize(cwd).expect("canonical live"), canonical);
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn returns_none_for_a_dead_pid() {
        assert_eq!(live_process_cwd(u32::MAX - 7), None);
    }
}
```

- [ ] **Step 2: Run** — `~/.cargo/bin/cargo test -p interface-terminal proc_cwd` → FAIL (module missing).
- [ ] **Step 3: Implement** `proc_cwd.rs`:

```rust
//! Live cwd of a spawned terminal child, for pass-off panes that must start
//! where the source agent actually works (e.g. a CLI-created worktree).

use std::path::PathBuf;

/// Best-effort: `None` on any failure, non-macOS, or an empty path.
#[cfg(target_os = "macos")]
pub(crate) fn live_process_cwd(pid: u32) -> Option<PathBuf> {
    use std::ffi::CStr;
    let Ok(pid) = libc::c_int::try_from(pid) else {
        return None;
    };
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).ok()?;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    // libc models `vip_path` as `[[c_char; 32]; 32]` (1024 bytes, MAXPATHLEN)
    // for old-rustc compatibility; it is one NUL-terminated C string.
    let bytes = info.pvi_cdir.vip_path.as_ptr().cast::<libc::c_char>();
    let path = unsafe { CStr::from_ptr(bytes) }.to_str().ok()?;
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn live_process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}
```

Trait method in `embedded.rs` (inside `trait TerminalProcessControl`, after `terminate_tree`):

```rust
    /// The direct child's OS pid, when the backend retains one. Pass-off uses
    /// it to read the live cwd; test doubles may leave the default `None`.
    fn process_id(&self) -> Option<u32> {
        None
    }
```

and in `impl TerminalProcessControl for SystemTerminalControl`:

```rust
    fn process_id(&self) -> Option<u32> {
        Some(self.process_id)
    }
```

(threading the spawn-time pid into the struct as a plain `process_id: u32` field set where the control is constructed — follow the value already computed as `process_id` near `embedded.rs:676`).

- [ ] **Step 4: Run** — both proc_cwd tests PASS; `~/.cargo/bin/cargo test -p interface-terminal` still green.
- [ ] **Step 5: Commit** — `feat(terminal): read a spawned child's live cwd on macOS`.

---

### Task 4: Errors + prompt binding in `launch.rs`

**Files:**
- Modify: `crates/interface-terminal/src/error.rs` (add three variants; then fix the exhaustive match arms the compiler reports in `apps/interface-host/src/terminal.rs` ~L715 error table and `apps/interface-host/src/native.rs` ~L669)
- Modify: `crates/interface-terminal/src/launch.rs` (new `bind_handoff_prompt` beside `bind_session`, ~L233)
- Test: `launch.rs` test module (the existing tests around L559-652 show the argv-assert style — copy it)

**Interfaces:**
- Produces: `TerminalError::{HandoffSourceUnavailable, HandoffUnsupportedProfile(TerminalLaunchProfileId), InvalidHandoff}` and `pub(crate) fn bind_handoff_prompt(launch: &mut ResolvedTerminalLaunch, profile_id: TerminalLaunchProfileId, prompt: &str) -> Result<(), TerminalError>`.

- [ ] **Step 1: Failing tests** (match the existing `bind_session` test style in `launch.rs`):

```rust
#[test]
fn handoff_prompt_lands_as_one_trailing_positional_argument() {
    for profile in [
        TerminalLaunchProfileId::Claude,
        TerminalLaunchProfileId::Codex,
        TerminalLaunchProfileId::Grok,
        TerminalLaunchProfileId::Gemini,
    ] {
        let mut launch = ResolvedTerminalLaunch::Program {
            executable: PathBuf::from("/bin/echo"),
            arguments: vec![OsString::from("--session-id=abc")],
            environment: Vec::new(),
        };
        bind_handoff_prompt(&mut launch, profile, "Read /tmp/h.md").expect("bind");
        let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
            unreachable!()
        };
        assert_eq!(
            arguments,
            vec![
                OsString::from("--session-id=abc"),
                OsString::from("Read /tmp/h.md"),
            ]
        );
    }
}

#[test]
fn handoff_prompt_refuses_shell_qwen_and_oversized_prompts() {
    let mut shell = ResolvedTerminalLaunch::LoginShell;
    assert_eq!(
        bind_handoff_prompt(&mut shell, TerminalLaunchProfileId::Shell, "x"),
        Err(TerminalError::HandoffUnsupportedProfile(TerminalLaunchProfileId::Shell))
    );
    let mut program = ResolvedTerminalLaunch::Program {
        executable: PathBuf::from("/bin/echo"),
        arguments: Vec::new(),
        environment: Vec::new(),
    };
    assert_eq!(
        bind_handoff_prompt(&mut program, TerminalLaunchProfileId::Qwen, "x"),
        Err(TerminalError::HandoffUnsupportedProfile(TerminalLaunchProfileId::Qwen))
    );
    let oversized = "p".repeat(4_097);
    assert_eq!(
        bind_handoff_prompt(&mut program, TerminalLaunchProfileId::Claude, &oversized),
        Err(TerminalError::InvalidHandoff)
    );
}
```

- [ ] **Step 2: Run** — `~/.cargo/bin/cargo test -p interface-terminal handoff_prompt` → FAIL.
- [ ] **Step 3: Implement.** `error.rs` additions (match the existing voice):

```rust
    #[error("terminal pass-off requires a running source terminal")]
    HandoffSourceUnavailable,
    #[error("terminal launch profile {0} cannot take a pass-off")]
    HandoffUnsupportedProfile(TerminalLaunchProfileId),
    #[error("terminal pass-off payload is malformed or oversized")]
    InvalidHandoff,
```

`launch.rs` (below `bind_session`):

```rust
/// Append the host-templated pass-off prompt as one positional argument.
///
/// The prompt is host-built (the renderer supplies only the transcript file's
/// CONTENT, never argv), but it is bounded here anyway because it bypasses
/// `TerminalLaunchProgram::new`, which validated the catalog argv at boot.
pub(crate) fn bind_handoff_prompt(
    launch: &mut ResolvedTerminalLaunch,
    profile_id: TerminalLaunchProfileId,
    prompt: &str,
) -> Result<(), TerminalError> {
    if prompt.is_empty() || prompt.len() > MAX_ARGUMENT_BYTES || prompt.contains('\0') {
        return Err(TerminalError::InvalidHandoff);
    }
    let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
        return Err(TerminalError::HandoffUnsupportedProfile(profile_id));
    };
    match profile_id {
        // Verified against each CLI's --help: an interactive session accepts
        // an initial prompt as a positional argument.
        TerminalLaunchProfileId::Claude
        | TerminalLaunchProfileId::Codex
        | TerminalLaunchProfileId::Grok
        | TerminalLaunchProfileId::Gemini => {
            arguments.push(OsString::from(prompt));
            Ok(())
        }
        TerminalLaunchProfileId::Qwen | TerminalLaunchProfileId::Shell => {
            Err(TerminalError::HandoffUnsupportedProfile(profile_id))
        }
    }
}
```

Then run `~/.cargo/bin/cargo build --workspace` and add the two new host-side match arms it demands:
- `apps/interface-host/src/terminal.rs` (~L715 table): `TerminalError::HandoffSourceUnavailable => ("terminal-handoff-source-unavailable", false, true),`, `TerminalError::HandoffUnsupportedProfile(_) => ("terminal-handoff-unsupported-profile", false, true),`, `TerminalError::InvalidHandoff => ("terminal-handoff-invalid", false, true),` — mirror the neighbors' tuple shape exactly as found.
- `apps/interface-host/src/native.rs` (~L669): follow the surrounding `refused(...)` pattern with user-facing copy: source unavailable → `"The pane being passed off is no longer running."`; unsupported profile → `"This launcher cannot take a pass-off."`; invalid → `"The pass-off payload was refused."`.

- [ ] **Step 4: Run** — tests PASS; workspace builds.
- [ ] **Step 5: Commit** — `feat(terminal): bind a pass-off prompt into a launch argv`.

---

### Task 5: Service `create` handoff flow — cwd override, file write, prune

**Files:**
- Modify: `crates/interface-terminal/src/service.rs` — `create` (~L458), a new private `handoff` module section near the top of the impl, and `TerminalService` shared state
- Modify: `crates/interface-terminal/src/service.rs` `Shared` struct — add `handoff_directory: PathBuf`; set it in the constructor(s) (`with_dependencies_and_launches` and any sibling) to `std::env::temp_dir().join("Interface").join("handoffs")` by default, with a `pub fn with_handoff_directory(mut self, directory: PathBuf) -> Self`-style override ONLY if tests need it — prefer passing the directory through the existing constructor chain; find every call site with `grep -rn "with_dependencies" crates apps` and update them
- Test: `crates/interface-terminal/tests/embedded_terminal.rs` (integration style already used there; its fixtures spawn real PTYs with a fake backend — follow that harness)

**Interfaces:**
- Consumes: Task 1 `TerminalHandoffV1` + bounds consts; Task 2 `authorize_pass_off`; Task 3 `live_process_cwd` + `process_id()`; Task 4 `bind_handoff_prompt` + error variants.
- Produces: `create` behavior — a request with `handoff` spawns in the source's live cwd (fallback: request cwd via exact rule) with the prompt argv; handoff file at `<handoff_directory>/<new-terminal-id>.md`.

- [ ] **Step 1: Failing integration test** (in `embedded_terminal.rs`, using its existing fixture helpers — read the file's create tests first and reuse the harness; assert through the fake backend's recorded `TerminalSpawnSpec`):

```rust
#[test]
fn create_with_handoff_appends_prompt_and_writes_transcript_file() {
    // Arrange: fixture with one approved root; create a running source
    // terminal (claude profile) exactly as the existing launch tests do.
    // Act: create a second terminal with launch profile codex and
    //   handoff: TerminalHandoffV1 { version: 1,
    //     source_terminal_id: <source id>, transcript: "did A\ndid B".into(),
    //     from_title: Some("bridgemind".into()) }
    // Assert on the backend's recorded spawn spec for the new terminal:
    //   - spec.launch is Program and its LAST argument contains
    //     "taking over" and the handoff file path;
    //   - the handoff file exists and its contents contain "did A" and
    //     "bridgemind";
    //   - spec.cwd == the source terminal's cwd (live-cwd equals it here,
    //     because the fake backend spawns no real process: the service must
    //     fall back to the source entry's recorded cwd).
}
```

Also add these failing negative tests (same harness):

```rust
// handoff with a source id that does not exist -> TerminalError::HandoffSourceUnavailable
// handoff with launch = None (login shell)     -> TerminalError::HandoffUnsupportedProfile(Shell)
// handoff transcript > TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES -> TerminalError::InvalidHandoff
// handoff.version != 1 -> TerminalError::InvalidHandoff
```

- [ ] **Step 2: Run** — `~/.cargo/bin/cargo test -p interface-terminal --test embedded_terminal handoff` → FAIL.
- [ ] **Step 3: Implement** in `service.rs`. Validation helper:

```rust
fn validated_handoff(
    handoff: &interface_contracts::terminal::TerminalHandoffV1,
) -> Result<(), TerminalError> {
    use interface_contracts::terminal::{
        TERMINAL_HANDOFF_TITLE_MAX_BYTES, TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES,
        TERMINAL_HANDOFF_VERSION,
    };
    let title_ok = handoff.from_title.as_deref().is_none_or(|title| {
        !title.is_empty()
            && title.len() <= TERMINAL_HANDOFF_TITLE_MAX_BYTES
            && !title.chars().any(char::is_control)
    });
    if handoff.version != TERMINAL_HANDOFF_VERSION
        || handoff.transcript.len() > TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES
        || handoff.transcript.contains('\0')
        || !title_ok
    {
        return Err(TerminalError::InvalidHandoff);
    }
    Ok(())
}
```

In `create`, replace the single `let cwd = approved.authorize(...)` with:

```rust
        if let Some(handoff) = request.handoff.as_ref() {
            validated_handoff(handoff)?;
            if request.launch.is_none() {
                return Err(TerminalError::HandoffUnsupportedProfile(
                    TerminalLaunchProfileId::Shell,
                ));
            }
        }
        let cwd = match request.handoff.as_ref() {
            Some(handoff) => {
                self.pass_off_cwd(handoff.source_terminal_id, &request.cwd, approved)?
            }
            None => approved
                .authorize(Path::new(&request.cwd))
                .map_err(root_error)?,
        };
```

`pass_off_cwd` (private method; the lock scope mirrors how `create` already reads state):

```rust
    /// The pass-off pane starts where the source agent actually works: the
    /// live cwd of its child process when readable and inside an approved
    /// root, else the source pane's recorded cwd, else the requested cwd.
    fn pass_off_cwd(
        &self,
        source_id: TerminalId,
        requested: &str,
        approved: &ApprovedTerminalRoots,
    ) -> Result<DirectorySnapshot, TerminalError> {
        let (recorded_cwd, pid) = {
            let state = lock(&self.shared.state);
            let Some(entry) = state.entries.get(&source_id) else {
                return Err(TerminalError::HandoffSourceUnavailable);
            };
            if entry.summary.state != TerminalState::Running {
                return Err(TerminalError::HandoffSourceUnavailable);
            }
            (
                PathBuf::from(entry.summary.cwd.clone()),
                entry.resources.control.process_id(),
            )
        };
        if let Some(live) = pid.and_then(crate::proc_cwd::live_process_cwd) {
            if let Ok(snapshot) = approved.authorize_pass_off(&live) {
                return Ok(snapshot);
            }
        }
        if let Ok(snapshot) = approved.authorize_pass_off(&recorded_cwd) {
            return Ok(snapshot);
        }
        approved
            .authorize(Path::new(requested))
            .map_err(root_error)
    }
```

(Adjust the `entry.resources.control` path to the real field names — `prepare`/entry storage shows them; the resize handler at ~L631 reads `entry.resources.control` already.)

After `let launch = self.shared.launches.resolve(...)`, add:

```rust
        let launch = match request.handoff.as_ref() {
            None => launch,
            Some(handoff) => {
                let profile_id = request
                    .launch
                    .map(|launch| launch.profile_id)
                    .ok_or(TerminalError::HandoffUnsupportedProfile(
                        TerminalLaunchProfileId::Shell,
                    ))?;
                let mut launch = launch;
                let file = self.write_handoff_file(request.id, handoff, &cwd.path)?;
                let prompt = handoff_prompt(handoff.from_title.as_deref(), &file);
                bind_handoff_prompt(&mut launch, profile_id, &prompt)?;
                launch
            }
        };
```

File write + prompt + prune (private helpers in `service.rs`; prune mirrors `prune_pasted_images` in `crates/interface-prompt-context/src/service.rs:405` — age > 24h or beyond the newest 64 by mtime, files only, never following symlinks):

```rust
const HANDOFF_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const HANDOFF_KEEP: usize = 64;

fn handoff_prompt(from_title: Option<&str>, file: &Path) -> String {
    let from = from_title.map_or(String::new(), |title| format!(" ({title})"));
    format!(
        "You are taking over for another agent{from}. Read {} - its terminal \
         transcript, newest last - then continue its work in this directory. \
         Do not redo work the transcript shows as finished.",
        file.display()
    )
}
```

```rust
    fn write_handoff_file(
        &self,
        id: TerminalId,
        handoff: &interface_contracts::terminal::TerminalHandoffV1,
        cwd: &Path,
    ) -> Result<PathBuf, TerminalError> {
        let directory = &self.shared.handoff_directory;
        std::fs::create_dir_all(directory)
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        self.prune_handoffs(directory);
        let file = directory.join(format!("{id}.md"));
        let from = handoff.from_title.as_deref().unwrap_or("another agent");
        let body = format!(
            "# Pass-off transcript\n\nFrom: {from}\nWorking directory: {}\n\n\
             Rendered terminal transcript, newest last; TUI chrome may appear.\n\n\
             ```text\n{}\n```\n",
            cwd.display(),
            handoff.transcript,
        );
        std::fs::write(&file, body)
            .map_err(|error| TerminalError::Backend(error.to_string()))?;
        Ok(file)
    }
```

Do NOT touch `CreateFingerprint` — an idempotent retry of the same create returns the existing entry, which is correct.

- [ ] **Step 4: Run** — `~/.cargo/bin/cargo test -p interface-terminal` → all green, including the four negatives.
- [ ] **Step 5: Commit** — `feat(terminal): pass-off creates spawn in the source's live cwd with a transcript prompt`.

---

### Task 6: Host dispatcher round-trip test

**Files:**
- Test: `apps/interface-host/src/terminal.rs` test module (the `create_payload` fixture at ~L1154 and the dispatcher tests around it show the harness)

**Interfaces:**
- Consumes: everything from Tasks 1-5 through the real `CHANNELS.create_terminal` dispatch path.

- [ ] **Step 1: Write the test** — clone the existing cold-create dispatcher test (~L1181): first dispatch a normal create in `fixture.default_root` with a `launch` profile the fixture catalog provides (read how the fixture builds its catalog; if it is shell-only, extend the fixture the way other launch dispatcher tests do — search the file for `TerminalLaunchCatalog::new`), then dispatch a second create whose payload adds:

```rust
        "handoff": {
            "version": 1,
            "sourceTerminalId": first_id,
            "transcript": "step 1 done",
        }
```

Assert the response is an error with code `terminal-handoff-unsupported-profile` when the target launch is absent, and a success whose `cwd` equals the source's cwd when a bindable profile is used. Also assert a payload with an unknown key inside `handoff` is refused (serde `deny_unknown_fields` surfaces as the dispatcher's decode error).
- [ ] **Step 2: Run** — `~/.cargo/bin/cargo test -p interface-host terminal` → new test fails only if wiring is wrong; fix until PASS (no production code should be needed — this task exists to prove the wire).
- [ ] **Step 3: Commit** — `test(host): pass-off createTerminal round-trip`.

---

### Task 7: Renderer pure pieces — `pass-off.ts` and `readLines`

**Files:**
- Create: `electron/src/renderer/pass-off.ts`
- Create: `electron/src/renderer/pass-off.test.ts`
- Modify: `electron/src/renderer/components/TerminalView.tsx` — `TerminalViewHandle` (L24-43) + handle implementation (~L270-320)

**Interfaces:**
- Produces:
  - `passOffTranscript(lines: readonly string[]): string` — joins with `\n`, strips trailing blank lines, keeps the newest `PASS_OFF_TRANSCRIPT_MAX_BYTES` (512 * 1024) by dropping whole lines from the front, prefixing `"[transcript truncated]\n"` when it drops any.
  - `passOffRefusal(source: { status: string } | undefined, profile: { available: boolean; label: string; unavailableReason?: string } | undefined): string | null` — `null` when allowed; otherwise a user-facing sentence.
  - `TerminalViewHandle.readLines(): string[]` — full normal buffer, plus the visible alternate screen when active.

- [ ] **Step 1: Failing tests** (`pass-off.test.ts`):

```ts
import { describe, expect, it } from "vitest";
import { PASS_OFF_TRANSCRIPT_MAX_BYTES, passOffRefusal, passOffTranscript } from "./pass-off";

describe("passOffTranscript", () => {
  it("joins lines and strips trailing blanks", () => {
    expect(passOffTranscript(["a", "b", "", ""])).toBe("a\nb");
  });
  it("keeps the newest lines when over budget and says so", () => {
    const line = "x".repeat(1024);
    const lines = Array.from({ length: 600 }, () => line);
    const result = passOffTranscript(lines);
    expect(result.length).toBeLessThanOrEqual(PASS_OFF_TRANSCRIPT_MAX_BYTES);
    expect(result.startsWith("[transcript truncated]\n")).toBe(true);
    expect(result.endsWith(line)).toBe(true);
  });
  it("returns an empty string for an empty buffer", () => {
    expect(passOffTranscript([])).toBe("");
  });
});

describe("passOffRefusal", () => {
  const profile = { available: true, label: "Codex" };
  it("allows a running source and an available profile", () => {
    expect(passOffRefusal({ status: "running" }, profile)).toBeNull();
  });
  it("refuses a missing or non-running source", () => {
    expect(passOffRefusal(undefined, profile)).toMatch(/running/);
    expect(passOffRefusal({ status: "exited" }, profile)).toMatch(/running/);
  });
  it("refuses an unavailable profile with its reason", () => {
    expect(passOffRefusal({ status: "running" },
      { available: false, label: "Codex", unavailableReason: "Codex is not installed" }))
      .toBe("Codex is not installed");
    expect(passOffRefusal({ status: "running" }, undefined)).toMatch(/launcher/i);
  });
});
```

- [ ] **Step 2: Run** — `cd electron && npx vitest run src/renderer/pass-off.test.ts` → FAIL.
- [ ] **Step 3: Implement** `pass-off.ts`:

```ts
/* Pure pieces of the terminal Pass Off flow: transcript shaping and
   eligibility. The capture (xterm) and the wire (createTerminal) stay in
   their owners; this module is what the tests hold still. */

export const PASS_OFF_TRANSCRIPT_MAX_BYTES = 512 * 1024;
const TRUNCATION_NOTE = "[transcript truncated]\n";

/** Newest-last transcript, whole lines only, bounded for the wire. */
export function passOffTranscript(lines: readonly string[]): string {
  let end = lines.length;
  while (end > 0 && lines[end - 1].trim() === "") end--;
  let start = end;
  let bytes = 0;
  while (start > 0) {
    const next = lines[start - 1].length + (start === end ? 0 : 1);
    if (bytes + next > PASS_OFF_TRANSCRIPT_MAX_BYTES - TRUNCATION_NOTE.length) break;
    bytes += next;
    start--;
  }
  const body = lines.slice(start, end).join("\n");
  return start > 0 ? `${TRUNCATION_NOTE}${body}` : body;
}

/** Null when the pass-off may proceed; otherwise the sentence to report. */
export function passOffRefusal(
  source: { status: string } | undefined,
  profile: { available: boolean; label: string; unavailableReason?: string } | undefined,
): string | null {
  if (source === undefined || source.status !== "running") {
    return "Pass off needs a running source terminal.";
  }
  if (profile === undefined) return "That launcher is not available.";
  if (!profile.available) {
    return profile.unavailableReason ?? `${profile.label} is unavailable.`;
  }
  return null;
}
```

`TerminalView.tsx` — add to the `TerminalViewHandle` interface:

```ts
  /** Every rendered line: full normal buffer, plus the visible alternate
      screen when a fullscreen TUI is active. Newest last. */
  readLines(): string[];
```

and to the handle implementation (beside `currentLine`):

```ts
      readLines() {
        const terminal = terminalRef.current;
        if (terminal === null) return [];
        const lines: string[] = [];
        const normal = terminal.buffer.normal;
        for (let y = 0; y < normal.length; y++) {
          lines.push(normal.getLine(y)?.translateToString(true) ?? "");
        }
        const active = terminal.buffer.active;
        if (active !== normal) {
          for (let y = 0; y < active.length; y++) {
            lines.push(active.getLine(y)?.translateToString(true) ?? "");
          }
        }
        return lines;
      },
```

- [ ] **Step 4: Run** — pass-off tests PASS; `npx vitest run src/renderer/prompt-attach.test.ts` still green (it pins TerminalView source strings).
- [ ] **Step 5: Commit** — `feat(renderer): pass-off transcript shaping and xterm readLines`.

---

### Task 8: Renderer wiring — menu page, callback thread, center `passOff`

**Files:**
- Modify: `electron/src/renderer/components/TerminalPaneNewMenu.tsx` — extract the profile-button list into an exported `TerminalLaunchProfileList` presentational component (props: `profiles`, `loading`, `onChoose(profileId)`; renders the existing `terminal-menu-empty` and `terminal-menu-launch` buttons exactly as today) and use it internally so `TerminalPrimitives.test.ts:253-256` still counts 3 menuitems.
- Modify: `electron/src/renderer/components/TerminalPane.tsx` — `TerminalPaneMoreMenu` grows a two-page panel and new props; `TerminalPane` passes them; new `onRequestPassOff` prop.
- Modify: `electron/src/renderer/components/TerminalWorkspace.tsx` — thread `onRequestPassOff` through `TerminalSplitWorkspaceProps`, `TerminalPaneLeaf`, `TerminalWorkspaceCallbacks` (after `onRequestSplit`), and the `<TerminalSplitWorkspace>` call site.
- Modify: `electron/src/renderer/components/IntegratedTerminalCenter.tsx` — `createTerminal` input + call gain `handoff` passthrough; new `passOff` callback; `callbacks` memo gains `onRequestPassOff: passOff`.
- Test: `electron/src/renderer/components/TerminalPrimitives.test.ts`.

**Interfaces:**
- Consumes: Task 7 `passOffTranscript`, `passOffRefusal`, `readLines`; Task 1 `TerminalHandoffV1` TS type.
- Produces: `onRequestPassOff: (paneId: TerminalPaneId, orientation: TerminalSplitOrientation, profileId: TerminalLaunchProfileId) => void` end to end; `TerminalPaneMoreMenuProps` gains `profiles?: readonly TerminalLaunchProfile[]`, `profilesLoading?: boolean`, `onLoadProfiles?: () => void`, `onPassOff?: (profileId: TerminalLaunchProfileId) => void`, `passOffEnabled: boolean`, `initialPage?: "actions" | "pass-off"` (test-only escape hatch, default `"actions"`).

- [ ] **Step 1: Failing tests** in `TerminalPrimitives.test.ts`:

```ts
  it("offers Pass off to… behind the ⋯ menu and lists launchers on its page", () => {
    const profiles = [
      { id: "claude" as const, engine: "claude" as const, label: "Claude Code", available: true },
      { id: "codex" as const, label: "Codex", available: false,
        unavailableReason: "Codex is not installed" },
    ];
    const actions = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, passOffEnabled: true, onPassOff: () => undefined,
    }));
    expect(actions).toContain("Pass off to…");
    const page = renderToStaticMarkup(createElement(TerminalPaneMoreMenu, {
      open: true, onOpenChange: () => undefined, onSplit: () => undefined,
      profiles, passOffEnabled: true, onPassOff: () => undefined,
      initialPage: "pass-off",
    }));
    expect(page).toContain("Claude Code");
    expect(page).toContain('title="Codex is not installed"');
    expect(page).not.toContain("Split right");
  });
```

Plus source-text assertions in the same file's center section:

```ts
    expect(terminalCenterSource).toContain("const passOff = useCallback");
    expect(terminalCenterSource).toContain("passOffRefusal(");
    expect(terminalCenterSource).toContain("passOffTranscript(");
    expect(terminalCenterSource).toContain("onRequestPassOff: passOff");
```

- [ ] **Step 2: Run** — `npx vitest run src/renderer/components/TerminalPrimitives.test.ts` → FAIL.
- [ ] **Step 3: Implement.**

`TerminalPaneMoreMenu` (structure; keep the existing split buttons byte-for-byte):

```tsx
export function TerminalPaneMoreMenu({ open, onOpenChange, onSplit, profiles,
    profilesLoading, onLoadProfiles, onPassOff, passOffEnabled = false,
    initialPage = "actions" }: TerminalPaneMoreMenuProps) {
  const [page, setPage] = useState<"actions" | "pass-off">(initialPage);
  const { triggerRef, menuRef, box, close, onMenuKey } = useTerminalMenuPopover({
    open, onOpenChange, minWidth: 200, placeOn: page,
  });
  useEffect(() => { if (!open) setPage(initialPage); }, [open, initialPage]);
  ...
```

- Page `"actions"`: the two existing split menuitems, then (when `onPassOff !== undefined && profiles !== undefined`) a third menuitem `Pass off to…` (`aria-label="Pass off to another agent"`, `aria-haspopup="menu"`, icon `codicon-arrow-swap` if `grep -o 'codicon-arrow-swap' codicons.css` finds it, else `codicon-arrow-right`), `disabled={!passOffEnabled}`, `title` = "Hand this pane's work to a new agent" (or the refusal text when disabled — pass it in via `passOffTitle?: string`), onClick `{ onLoadProfiles?.(); setPage("pass-off"); }` — do NOT close the menu.
- Page `"pass-off"`: `aria-label="Pass off to"` on the panel, a back menuitem (`‹ Back`, onClick `setPage("actions")`), then `<TerminalLaunchProfileList profiles={profiles} loading={profilesLoading === true} onChoose={id => { onPassOff?.(id); close(true); }} />` with Shell filtered out: `profiles.filter(profile => profile.id !== "shell")`.

`TerminalPane`: extend props with `onRequestPassOff?: (paneId, orientation, profileId) => void`; render:

```tsx
          <TerminalPaneMoreMenu open={moreMenuOpen} onOpenChange={setMoreMenuOpen}
            onSplit={orientation => { onRequestSplit(pane.id, orientation); }}
            profiles={launchProfiles} profilesLoading={launchProfilesLoading}
            onLoadProfiles={onLoadLaunchProfiles}
            passOffEnabled={activeTerminal?.status === "running"}
            onPassOff={onRequestPassOff === undefined ? undefined : profileId => {
              onRequestPassOff(pane.id, splitOrientation(), profileId);
            }} />
```

`TerminalWorkspace.tsx`: add `readonly onRequestPassOff?: (paneId: TerminalPaneId, orientation: TerminalSplitOrientation, profileId: TerminalLaunchProfileId) => void;` to `TerminalSplitWorkspaceProps` and `TerminalWorkspaceCallbacks` (non-optional there, matching `onRequestSplit`'s style), pass through `TerminalPaneLeaf` and the `<TerminalSplitWorkspace …>` call (`onRequestPassOff={callbacks.onRequestPassOff}`).

`IntegratedTerminalCenter.tsx`:
- `createTerminal` input type gains `handoff?: TerminalHandoffV1;` and the request spread gains `...(input.handoff === undefined ? {} : { handoff: input.handoff }),` (inside the existing `window.iface.createTerminal({...})` object).
- New callback placed **immediately after the `split` callback** (which itself must stay untouched):

```ts
  const passOff = useCallback((paneId: TerminalPaneId,
      orientation: TerminalSplitOrientation,
      profileId: TerminalLaunchProfileId): void => {
    const pane = terminalPaneById(layoutRef.current, paneId);
    const source = pane === undefined ? undefined : terminalById(pane.activeTerminalId);
    const profile = launchProfilesRef.current.find(candidate => candidate.id === profileId);
    const refusal = passOffRefusal(
      source === undefined ? undefined : { status: source.state }, profile);
    if (refusal !== null || source === undefined || profile === undefined) {
      report(refusal ?? "Pass off needs a running source terminal.");
      return;
    }
    const handle = handles.current.get(terminalOutputStreamKey(source.id, source.generation));
    const transcript = passOffTranscript(handle?.readLines() ?? []);
    const fromTitle = source.title.trim();
    void createTerminal({
      cwd: source.cwd,
      title: profile.label,
      launch: { version: 1, profileId: profile.id },
      handoff: {
        version: 1,
        sourceTerminalId: source.id,
        transcript,
        ...(fromTitle === "" ? {} : { fromTitle }),
      },
      split: { paneId, orientation },
    });
  }, [createTerminal, report]);
```

(Confirm the field names against the real `TerminalSummary` — status lives at `terminal.state`; `terminalById` returns the summary. Import `passOffRefusal`, `passOffTranscript` from `../pass-off` and `TerminalHandoffV1` from `../../shared/ipc`.)
- `callbacks` memo: add `onRequestPassOff: passOff,` and `passOff` to that memo's dependency array (the memo at ~L1396 lists its deps explicitly — extend that array, NOT `split`'s).

- [ ] **Step 4: Run** — `npx vitest run src/renderer/components/TerminalPrimitives.test.ts src/renderer/terminal-redraw-integration.test.ts src/renderer/components/TerminalLauncher.test.ts src/renderer/pass-off.test.ts` → all PASS. If `terminal-redraw-integration` fails, read its assertion — the `passOff` insertion after `split` may have shifted a source slice; the fix is placement (after `insertPaths`' dependent block instead), never editing that test's boundaries.
- [ ] **Step 5: Run the full JS gate** — `cd electron && npm run check` → exit 0.
- [ ] **Step 6: Commit** — `feat(terminal): Pass off to… hands a pane's work to a new agent`.

---

### Task 9: Gate, docs, version

**Files:**
- Modify: `docs/CODEMAP.md:633` — extend the invariant sentence: renderer input can name a launch profile id **and supply bounded pass-off transcript text; the executable, argv shape, prompt template, and handoff file path remain host-owned**.
- Modify: `electron/package.json` + `electron/package-lock.json` — version `0.3.31` → `0.3.32` (two fields in the lockfile: top-level and `packages[""]`).

- [ ] **Step 1:** Apply both edits.
- [ ] **Step 2:** `~/.cargo/bin/cargo xtask check` from the worktree root → exit 0 (runs fmt, clippy `-D warnings`, `cargo test --workspace`, `npm run check`). Fix anything red (fmt/clippy nits are expected on new Rust).
- [ ] **Step 3: Commit** — `feat(terminal): Pass Off — version 0.3.32, codemap invariant updated` (include any fmt/clippy fixups).
