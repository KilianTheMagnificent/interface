# crates/

Every canonical rule in the product lives here. `apps/interface-host` and
`apps/iface` are adapters over these crates; the Electron renderer only draws
what they return. If a behavior can be decided without a window or a socket, it
belongs in a crate.

Module maps, entry points and per-crate invariants and gotchas:
`docs/areas/contracts.md`, `docs/areas/runtime-core.md`,
`docs/areas/engine-adapters.md`, `docs/areas/persistence-and-services.md`,
`docs/areas/collaboration.md`. The table below is the index; the area doc for
the crate you are editing is the detail.

| Crate | Owns | Watch out for |
| --- | --- | --- |
| `interface-contracts` | Frozen DTOs, the channel table, pure `validate()` gates that must agree byte-for-byte with the TS oracle. Leaf crate: no I/O, no async, no provider knowledge. | camelCase + `deny_unknown_fields` everywhere; golden-JSON tests pin exact bytes. `validate()` returns an error, never panics. Timestamps compare as strings only because `validate_timestamp` pins a fixed-width form. The retained-event-floor expression uses saturating subtraction in the two validators and the projection — keep all three aligned. |
| `interface-engine` | `EngineAdapter`/`SessionHandle` traits, the three-state delivery vocabulary, NDJSON + JSON-RPC framing with `PendingRouter`, argv-only `ProcessSpec`, the session lifecycle gate, and the supervised transport (`supervised.rs`) and session core (`session_core.rs`) engine-codex and engine-acp share. `jsonrpc.rs` owns the RPC-outcome translations `delivery_from_rpc`, `prepare_error` and `frame_error` — the engine crates call them, they do not re-derive certainty. | `DefinitelyUnsent` is the only replay-safe outcome. The `Prepared → WriteStarted → FrameAccepted` ladder is monotonic. Legacy count-only streams drop the oldest event. Policy v2 budgeted streams protect control events, replace disposable output with an omission notice, and return explicit errors if control capacity is exhausted. |
| `interface-runtime` | Adapter registry, the session open/resume transaction, the live-attachment map, the event pump, management ops, and the typed `RuntimeBridgeCommand` schema every transport goes through. | Only the in-memory `live` map authorizes effects — durable history may still say open. Evict via `remove_if_current`, never `HashMap::remove`. Provider-first ordering: a local failure after a confirmed provider effect is `OutcomeUnknown`, not a local error. `EngineHistoryStore` is a process-local projection, not a store. |
| `interface-orchestration` | Event-sourced multi-agent state: the pure reducer, byte-budgeted projections, the one-writer CAS store. | **The reducer is event-sourced.** Changing an observable state transition breaks replay of stores already on disk. It is pure — no clocks, no randomness, no I/O. Snapshots are append-only, one revision and one event at a time. `is_legacy_temp_name` must stay `strip_suffix(".tmp")`: clippy pedantic forbids `ends_with` extension comparisons and the check is deliberately case-sensitive for 0.1.169 parity. |
| `interface-collaboration` | The durable collaboration domain: contract + validation (`state/`), the single-writer journal, the coordinator's one-attempt effect protocol, the broker authority core, the owner-private MCP server. The pane-delegation service was retired on 2026-10-14 (`docs/archive/pane-delegation-20261014/README.md`); `delegates.rs` keeps only the Away CLI's worker runner: `DelegateRequest`, `DelegateJob` and the `DelegateRunner` trait (`run`, `apply`). | Call `recover()` before any admission. One attempt per effect; a `started` attempt is never replayed. Ambiguous is never retryable. Do not swap `recover_legacy_layout` for the storage quarantine helper — `records/` accepts only `<64-hex>.json` and the sibling `quarantine/` directory is deliberate. |
| `engine-codex` / `engine-claude` / `engine-acp` | Provider protocol and lifecycle truth. Codex `app-server`; the user's `claude` CLI plus its projects store; ACP v1 for built-in Grok, built-in Qwen (owner-managed MLX backend) and Custom profiles. | None of them read credentials — the executable is the auth boundary and stderr is drained, never mirrored. Launch is argv-only, no shell. `bypassPermissions` is spawn-time only; resume never recovers a volatile policy from the stream, only the owner's standing choice (`ResumeOptions.permission_mode`, an agent's access). engine-acp and engine-codex share one transport (`interface_engine::supervised`) and one session core (`interface_engine::session_core`); each keeps only its event enum, its `Classify` nouns and its inbound handling. engine-claude has its **own** NDJSON decoder with different resync behaviour. |
| `provider-replay` | Test-only: records a real provider CLI session (`provider-record`, scrubbed) and replays it to the real adapter (`provider-replay`); fixtures under `fixtures/<engine>/`. Never packaged, never a production dependency. | Fixtures are real recordings: never hand-write or hand-edit one. The replayer matches by signature and shape and binds values the host makes fresh; an unexpected frame exits 3 with both frames on stderr. |
| `interface-storage` | The persistence foundation: owner-private directories, descriptor-bound writer leases, atomic writes, bounded JSON, quarantine. | Any failure **after** `fs::rename` must surface as `WriteUncertain` — never `WriteFailed`, never `Ok`. Do not simplify away the `renamed` flag or the repeated `assert_held_for`/`assert_directory` calls. The lock file is intentionally never unlinked on release. |
| `interface-registry` | Session cache, managed git worktrees, review/recovery/turn checkpoints. | Persisted formats are frozen down to whitespace (`worktrees.json` is pretty-printed with **no** trailing newline). First observable revision is 1, not 0. `open()` degrades to an empty in-memory cache for almost every failure — assert on the data, not on `Ok`. The credential/binary *tables* are shared via `interface-common`, but the classifier *rules* here and in `interface-workspace` are separate copies that have drifted; mirror rule changes. |
| `interface-settings` | Appearance, the ACP startup roster, saved terminal workspaces, default directory resolution. | The roster is read at host startup and immutable for the process's life — a profile written today registers next launch. Argv validation rejects shells and credential-shaped arguments. |
| `interface-review` | Bounded git/npm subprocesses producing a credential-filtered diff and check results. | Truncated output maps to exit 0; spawn failure maps to `REVIEW_GIT_UNAVAILABLE` (-1) and a timeout to `REVIEW_GIT_TIMED_OUT` (-3, reported as `git-error` naming the deadline and iCloud); -2 is an internal sentinel that never reaches the DTO. |
| `interface-common` | Leaf helper crate: `time::days_from_civil`/`civil_from_days` and the frozen ISO renderer, `paths::lexically_normalized` (the single copy behind every path guard) and `lexically_absolute`, `process::poll_backoff`, capped child-output reads that end with the child rather than the pipe (`process::CapturedPipe::child_exited`), the shared credential/binary path tables, `sync::lock` (poison-tolerant), `unix::effective_uid` and `same_stable_metadata`, `fsid::DirectorySnapshot` (pin a canonical directory by identity; review, Review checkpoints and the workspace editor share it) and `fsid::FileIdentity` (the dev/inode pair workspace and the collaboration MCP socket use; interface-storage keeps its own hashable copy, and the terminal root allowlist and prompt-context roots keep theirs because they differ), `product_version!` (expand it only in iface and interface-host's `main.rs`: a target that reads the release stamp recompiles on every release; libraries and engines get the version at runtime), `bounded_read::within` with `is_dataless` (the one deadline-bounded project read: a file iCloud has evicted fails as still downloading instead of hanging; the reader thread is abandoned, never joined), and `git_hardening` (the one copy of what Interface's own git calls do about programs a repository's settings name: list them with `PROBE_ARGS`, run with `Overrides` plus `STATIC_ARGS`/`STATIC_ENV`; Review's, Threads' and the registry's runners all use it). | Keep it dependency-light and policy-free; behavior that differs per caller (process kill/timeout semantics, classifier rules) stays in the caller. |
| `interface-workspace` | The shared credential/generated/binary path classifier, Quick Open, project search, editor compare-and-save, customization inventory. | `is_search_excluded_path` is the single classifier for listing, search, review, inventory and prompt context — but **not** checkpoints. `save_text` re-reads and re-hashes several times per save: deliberate TOCTOU narrowing. |
| `interface-prompt-context` | Path authorization, the four frozen prompt-context operations, the pasted-image store. | Grants are exact, and every resolver re-checks identity after canonicalizing. |
| `interface-terminal` | The integrated PTY registry, host-owned launch profiles, OSC title scanning, the approved-root allowlist. | Lock order `lifecycle_gate → event_gate → state`, with events published under `event_gate`. Every mutation is generation-fenced; dismissed ids can never be recreated. `finish_process` drops the workspace lease **before** publishing `Exited`. `TerminalState::Starting` is never produced but the renderer reads it — the guards are defensive, not dead. |
| `interface-desktop-services` | The desktop operation dispatcher, workspace grant authority, worktree cleanup leases, Review checkpoints. | `COVERED_OPERATIONS` (asserted by the host router) is consumed by the host router's count assertions — adding a branch means editing both. Typed `DesktopEffectCertainty`, not error prose, controls host replay safety. |
| `interface-updater` | Feed URL admission, enablement, the `UpdateState` machine, macOS codesign/ditto verification, the loopback private feed, and the public update channel: the compiled-in feed and keys (`release-channel.json`), the signed manifest, and the mirror that downloads a checked release into the loopback feed. | `configure()` must be called exactly once, before any native event. `requirement == None` means "this platform needs no verification". Verification is generation-fenced; `complete_verification` only promotes Downloading → Ready. The installer reads only the loopback feed; nothing unsigned may reach that folder from the network. Shipped builds must always be able to read a newer manifest: add fields, never rename the file or raise `schema` (`docs/PUBLIC_UPDATES.md`). |

## Shared rules

- **Fail-closed provider effects.** Distinguish definitely-unsent from unknown.
  Definitely-unsent is the only replay-safe outcome; an unknown is never
  converted, never retried, never silently downgraded. Never widen a
  `Succeeded`/`replay_is_safe` arm, never replay an ambiguous effect. The rule is
  deliberately duplicated in `interface-engine`, `interface-runtime`, each engine
  crate, `interface-collaboration`,
  `apps/interface-host/src/{dispatcher,collaboration,orchestration}.rs`,
  `apps/iface/src/errors.rs` and `App.tsx`. Widening any one of them breaks the
  whole chain.
- **Frozen surfaces.** Wire and persisted shapes, channel names, and error-code
  strings are contracts. A change needs an explicit protocol version and
  cross-language contract tests.
- **Uncalled is not dead.** Retained parity surface with zero in-repo callers is
  common here: the read-only registry/worktree/checkpoint helpers, unused
  `RuntimeBridgeCommand` variants and defaulted adapter methods, most of
  `interface-collaboration`'s `pub use state::*` surface. Grep is not evidence.
  Check `docs/PARITY_STATUS.md` and this file before removing anything.
- **No `unsafe`.** `unsafe_code = "forbid"` workspace-wide — that is why
  interface-terminal signals through rustix's safe `kill` wrappers and other
  crates shell out to `/bin/kill`. clippy `all` + `pedantic` are warnings and
  the gate runs `-D warnings`.
- Edition 2024, MSRV 1.88 pinned in `rust-toolchain.toml`. std `LazyLock` is
  available and is what the regex sites use; do not add `lazy_static` or
  `once_cell`.
- Workspace deps are declared once in the root `Cargo.toml`.
- Rust and Node must never write the same canonical store concurrently.
- Behavior questions go to the read-only oracle at
  `~/Developer/Projects/Interface`. Never edit it.

## Verify

```
cargo test -p <crate>                              # e.g. -p interface-runtime
cargo test -p engine-codex -p engine-acp -p engine-claude
cargo clippy -p <crate> --all-targets -- -D warnings
cargo xtask check                                  # canonical gate
```

Changing `interface-contracts` or `interface-desktop-services` touches the host
channel tables and the Electron side — run `cargo xtask check`, not a scoped
test, before reporting done.
