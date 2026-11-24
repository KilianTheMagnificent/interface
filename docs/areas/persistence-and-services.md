# Persistence and host services

Terminal naming: `interface-terminal/src/service/titles.rs` owns pane-name
precedence; `codex_names.rs` reads bounded local Codex session metadata using an
already-bound ID, caches the first real request, and observes saved chat names.
`grok_names.rs` does the same for a Grok pane's minted id: `generated_title`
when Grok has named the chat, otherwise the first real request.
`antigravity_names.rs` does the same for an Antigravity pane's captured
`agy-<uuid>` conversation: `annotations/<id>.pbtxt` when Antigravity has written
a title, otherwise the first explicit user request in `transcript.jsonl`. It
never supplies session authority or changes provider files.
It never supplies session authority or changes provider files. Known approval
title shapes retain Fast metadata separately; unknown shapes keep the last
useful task name. `interface-settings/src/terminal_titles.rs` stores explicit
Interface names in owner-private `terminal-titles.v1.json`; the existing
workspace roster and wire schemas stay unchanged. Host startup drops names of
panes absent from a readable roster (they can never be reopened). Startup
repairs legacy Codex status titles before exposing the roster and
conservatively preserves useful legacy names whose provenance is unknown.
Workspace saves persist an
initial useful member title in the existing name field. Renderer save replies
merge only names still matching the submitted value, preserving newer edits.

Rules for editing here: `crates/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

Storage primitives, the stores built on them, and service crates that own
behavior the shell only renders. Remote/native paths are mapped separately in
[remote-and-native.md](remote-and-native.md).

**crates/interface-storage** — the foundation.

- `secure.rs` — the safety core: `PrivateDirectory`, `FileIdentity`/`FileSnapshot`,
  `O_NOFOLLOW` opens, bounded reads, metadata re-validation helpers.
- `atomic.rs` — temp create, one full flush of the temp (`F_FULLFSYNC` on
  Apple, before the rename so a power loss leaves the old file or the new one,
  never a torn one), lease and snapshot re-check, rename, read-back
  verification, then a plain `fsync(2)` of the directory (`hand_off`): a power
  loss can roll the newest write back to the previous file until the drive
  next flushes. Owns `.interface-atomic-*.tmp` and unlinks its own temp
  (identity-checked) when a write fails before the rename.
- `json.rs` — `JsonLimits`, strict decoding with depth/node budgets and
  duplicate-key rejection, `read_json`/`atomic_write_json`.
- `cas.rs` — the `Revisioned` trait, `compare_and_set_json`, revision guards.
- `lock.rs` — `WriterLock`: persistent, descriptor-bound flock lease.
- `quarantine.rs` — the `.quarantine` child directory: moving corrupt or
  one store's stale-temp artifacts aside for forensics. A
  `QuarantinedArtifact` names where the bytes went, which the registry, the
  threads store and the permission-mode store keep (`quarantined_artifact()`)
  so the host can tell the user once (`iface:diagnostics` notices).
- `log_file.rs` — `RotatingLogFile`, an owner-only append log that starts
  over at a byte cap keeping one predecessor (`host.log`, `host.log.1`), and
  `read_log_tail`. Diagnostic, never authoritative: no fsync, failures dropped.
- `error.rs` — `StorageError`; the `WriteFailed` vs `WriteUncertain` split.

**crates/interface-registry** — `session.rs` (provider-reconstructible session
cache, legacy JSON shape, merge/archive/title/origin/forget rules, corrupt-cache
quarantine), `worktree.rs` (git worktree create/remove/verify via a bounded git
runner plus the durable metadata store), `checkpoint.rs` (review/recovery/turn
checkpoints: staged blob+manifest+journal published by rename, plus a private
credential/binary path policy), `error.rs`.
Private session/title position indexes and a forgotten-membership set accompany
the ordered vectors; indexes are not persisted and must preserve merge/revision
and JSON semantics across reopen. `content_revision()` (not persisted) advances
with `revision()` on every change except an `apply_event` that moved only
`last_activity`; the runtime cache writer and the host registry-push coalescers
use it to tell a timestamp-only change from a real one. `upsert_all` advances
neither when every merged row equals the stored one (an unchanged provider
listing on each window focus). `list_unsorted()` plus
`sort_listed()` equal `list()`, so a caller can clone under the lock and sort
after releasing it. `save()` is `prepare_save()` (copy the rows; needs the
registry) plus `RegistrySave::write()` (serialize, strict re-read, atomic
write under the cache file's own writer lock), so callers can write after
releasing the registry lock; an image no newer than the last one written is
dropped, and bytes equal to the file this registry itself last installed
(still the named inode) are not rewritten. `cache_is_current()` tells the
runtime writer when there is nothing to save; a freshly opened registry is
never current, so its first save still rewrites the file. `restored_from_cache()`
is true only when the rows at open came from a readable cache file; a
registry that started empty (cache missing, unavailable or quarantined)
refills as providers list, so the thread host marks no thread missing
against it. A listing's title replaces the stored one when that is a
placeholder (empty, the session id, "Codex session") or when the provider
first names a Code (or kind-less) session itself (`titleFromSummary` going
true: Claude's summary or custom title, a Codex `name`), which replaces its
launch-prompt title; Chat keeps its prompt title, and a title override (an
owner's or generated rename) always wins.

**crates/interface-settings** — `agents.rs` (durable Agent profiles, routines, run claims and recovery; [agent workspace](agents.md)), `private_path.rs` (`prepare_store`: hardening
parent and file to 0700/0600 without following links), `leased.rs` (the shared
commit protocol: `LeasedDocument` for the stores that hold their writer lease
for life — agents, agent work, first-message intents, agent permissions,
harness, threads — and `write_document` for the per-save lease the appearance, ACP,
roster and reopen-authority saves take; `open_or_quarantine` moves a corrupt
or out-of-bounds document aside and starts empty), `threads.rs` (the
`threads.v5.json` store — store version 2 keeps each turn's end, the
repository-relative files its own edits named and whether others shared its
checkout, and each record whether Interface opened its pull request
(`pullRequestTracked`: tracked whatever the global setting); store version 3
(protocol 46) adds the `handoff` lineage of a thread Continue with started
and the lineage a prepared workspace hands to its thread; store version 4
(protocol 57) adds the `child` lineage of a thread team tools started and
the `teamTools` setting; store version 5 (protocol 59) adds the chain of
provider sessions a switched thread runs as (`chain`, `segmentOf`;
`threads/chain.rs`: one head listed, segments kept for a switch back, the
switch moving the thread's owner-facing state in one commit); it carries a
`threads.v4.json` (else `threads.v3.json`, `threads.v2.json`, then
`threads.v1.json`) over once, read-only — and every
Threads rule as pure functions over its
document (`threads/view.rs` owns ordering and wire projection, with one
worktree-existence check per distinct path per view): sections, woke and unread, base-62 order keys, seeding, run stamps,
auto-settle, restore and release isolation, fork points by prompt digest,
missing records (dropped after a day) and making room at the byte cap, when a generated title is due
(`wants_title_at_first_prompt`: any thread as its first prompt goes out,
`wants_generated_title`: the retry after the first completed turn; never
over a `user` or earlier `generated` title; the one-time
`turn_generated_titles_on_once` behind the `threads.generated-titles-on`
marker; protocol 71's Pane tools without asking in the
`threads.pane-tools-without-asking` file beside the store, `on` or `off`,
anything else or unreadable read as off, never in the document, which
`set_settings` keeps it out of, read lease-free at host start by
`pane_tools_without_asking_at`); [threads
design](../superpowers/specs/2026-10-06-threads-design.md)), `thread_journal.rs`
(protocol 56: each thread's append-only journal under `thread-journal/<first
16 hex of sha256(key)>/segment-<20-digit first seq>.jsonl`, a header line then
one entry a line; `fold.rs` joins message deltas, `bounds.rs` cuts an entry to
64 KiB, `store.rs` is the one writer (its own `journal.lock`, 1 MiB segments,
32 a thread, 4096 threads and 1 GiB, the idlest threads' oldest segments
first; a torn last line is cut at open, a segment that does not parse moves to
`thread-journal/quarantine/`, a turn left open gets the restart marker),
`read.rs` the lock-free readers (pages, `replay` to engine events, `search`,
`last_answer`, `last_ended_turn`, `watch`), `import.rs` a provider's history into entries;
[T3-style systems, section 1](../superpowers/specs/2026-11-14-t3-systems-design.md)),
`draft_stash.rs` (protocol 63, a thread's stashed drafts:
`draft-stash.v1.json`, at most 20 a thread, 64 KiB of text and 16 attachment
references each, 8 MiB a file; references only, a diff's Review bytes are
dropped; pop stashes a non-empty composer in the same commit; follows a
switched thread's head, forgotten with the thread; leased, atomic,
quarantined; its file paths keep pasted images), `thread_reference.rs`
(protocol 63, thread references: team tools' read rule through heads, the
last prompt and answer of a chain's journal, at most 4 KiB, and the block;
[T3-style systems, section 9](../superpowers/specs/2026-11-14-t3-systems-design.md)),
`thread_secrets.rs` (protocol 67, the private secret card: one 0600 file per
secret under `thread-secrets/<digest of the thread key>/` (0700), a
`.thread` file naming the thread, written through a fresh file renamed into
place; names `[A-Za-z0-9_.-]{1,64}` not starting with a dot; expiry 24 h from
the file's modification time; nothing here formats a value), `thread_sides.rs`
(protocol 67, `thread-sides.v1.json`: each thread's open side question, one a
thread, 64 in all; leased, atomic, quarantined;
[T3-style systems, section 12](../superpowers/specs/2026-11-14-t3-systems-design.md)),
`folder_trust.rs` and `trusted_folders.rs` (protocol 72, folder trust:
`trust_root` maps a working folder to the folder an answer covers, the
repository's top folder, a linked worktree to its main repository only when
that repository names it back, never a deeper folder through a home kept
under git; `claude_code_trusts`/`ClaudeTrustCache` read Claude Code's own
`hasTrustDialogAccepted` for exactly that folder, parsed once per change of
the file; `claude_config_file` finds an account's `.claude.json`;
`trusted-folders.v1.json` keeps the owner's answers, trusted and declined, at
most 4096 each, leased, atomic, quarantined, and an unreadable file trusts
nothing). interface-desktop-services `service/folder_trust.rs` opens that
store beside `projects-folder.v1.json` and owns the one decision,
`DesktopService::folder_trust_of`: the store, Claude Code's record for the
session's account, or a folder Interface owns (inside the agents root, or
registered with `add_owned_folder`: the Chat folder); New project records the
empty folder it made as trusted when it is its own trust root.
`provider_accounts.rs` (protocol 60, several accounts per provider:
`provider-accounts.v1.json`, the added Claude/Codex/Grok accounts, at most 8
per provider, and each provider's default; `session-accounts.v1.json`, session
key → added account, at most 10,000, pruned against the registry at startup;
both leased, atomic, quarantined on corruption; the account directories
under `<data root>/accounts/<provider>/<id>/`, 0700; [T3-style systems,
section 6](../superpowers/specs/2026-11-14-t3-systems-design.md)),
`thread_team.rs` (protocol 57, team tools: the permission ladder and each
engine's modes on it, start limits, read and control authorization, the
status a thread reports, the finished-thread notice's wording and when it is
sent, waits or is dropped, `read_thread`'s compact items, and
`thread-team.v2.json`: the started-thread ledger and call receipts, leased,
bounded, quarantined on corruption; [T3-style systems, section
2](../superpowers/specs/2026-11-14-t3-systems-design.md); version 2 (protocol
65, outside agents, [section 11](../superpowers/specs/2026-11-14-t3-systems-design.md))
adds the owner's `outsideAgents` level and a started thread's `startedBy
{kind: outside, client}`, reads `thread-team.v1.json` once and leaves it
unchanged, and the outside rules live here too: what each level may call,
4/16 per session and 64 a day, the project a start may name, the client name
made safe, and the socket path),
`thread_visuals.rs` (protocol 61, visual replies: the page store under
`thread-visuals/<h>/<id>.html`, one writer, owner-private, at most 64 pages
and 16 MiB a thread with the oldest going first, and the pure rules for a
page's title, size and "is it HTML" and its time-ordered id; the journal's
`visual` entry and `merge_visuals` live in `thread_journal/`; [T3-style
systems, section 8](../superpowers/specs/2026-11-14-t3-systems-design.md)),
`thread_schedules.rs` (protocol 62, scheduled prompts and pull-request
watch: due times without bursts, the missed-run policy (fire once under a
day late, else skip and record), the delivery gate, each firing's
occurrence id and its attempt recorded before the send, watch fingerprints
and the prompts' wording, and `thread-schedules.v2.json` (it reads a
`thread-schedules.v1.json` once and carries it over), leased, at most
256 schedules and 64 watches, quarantined on corruption; [T3-style systems,
section 7](../superpowers/specs/2026-11-14-t3-systems-design.md); protocol
64 adds Resume at reset there: the `resume` kind, one per thread, due a
minute after the reset (`resume_at`), the prompt it may carry
(`resume_prompt_choice`: a queued follow-up, else a definitely-unsent
prompt-recovery receipt with no attachments, never an unknown one, else
"Continue from where you stopped."), the reset from full usage windows
(`usage_windows_reset`) and `set_resume`; [section
10](../superpowers/specs/2026-11-14-t3-systems-design.md)),
`appearance.rs` (the theme file and
its five versioned sidecars, one `Sidecar` descriptor each; the theme may be
`custom:<name>` and `system-themes.v1.json` holds Follow macOS's pair),
`custom_themes.rs` (reads, never writes, the user's
`~/.config/interface/themes/*.css`: bounded 1 MB reads cached by size and
mtime, links followed, light/dark and swatch colours scanned out of the CSS,
and the folder fingerprint the host's watcher polls), `theme_import.rs`
(pure conversion of iTerm2, Ghostty, Alacritty, Base16 and VS Code colour
schemes into theme-file CSS for `iface theme convert`; no filesystem access),
`output_reviews.rs` (the `output-reviews.v2.json` metadata store plus one
owner-private, SHA-256-named file per snapshot in `output-review-snapshots/`,
written before and removed after the metadata naming it, unnamed files pruned
at open; it carries an `output-reviews.v1.json` over once, read-only),
`acp_profiles.rs` (roster validation: slug/display/executable/argv bounds, shell
and credential-shaped argv refusals), `terminal_workspaces.rs` (bounded text,
unique ids, one-owner-per-terminal), `default_directory.rs` (home/Desktop/
Projects resolution and the iCloud File Provider probe).
A `LeasedDocument` latches uncertain (refusing every later commit until
restart) only when disk may differ from memory: drift, a lost lease, a failure
after the rename, or unsafe target state met while writing. A refusal where
nothing reached the target (over its bounds, not strictly representable, or an
atomic write that failed before its rename) leaves the store writable.
`tests/golden/` pins the bytes every settings store writes.

**crates/interface-review** — `repository.rs` (the Changes view: porcelain status
parsing, numstat/diff under byte and file budgets, untracked-file synthetic
patches, credential/binary omission), `checks.rs` (`package.json` script
discovery for a fixed allowlist, sequential shell-free `npm run --silent`),
`process.rs` (bounded child runner: timeout including inherited output pipes,
process-group teardown, output cap, lossy UTF-8),
`safety.rs` (the crate's private TOCTOU-hardened read path),
`diagnostics.rs` (the Help → Copy Diagnostics text and its filter: the
thread-ship credential redaction, `…_token`/`…_secret`/`…_key`/`…_password`
field values (kept out of the shared redaction, which the commit panel uses to
refuse patches), credential-named variables, paths the shared
`interface_common::paths` tables call credential material, long mixed-case
secrets, and the home folder as `~`),
`thread_git.rs` (Threads: thread worktrees under
`<repo parent>/.interface-worktrees` on `iface/` branches, outside the managed
ledger; hidden per-turn checkpoint commits under `refs/interface/` staged
through a private temporary index, so the real index and `git status` never
change; bounded credential-filtered diffs and numstat, including the working
tree through a throwaway tree; restore behind a recovery checkpoint;
`move_ref` files a pending launch baseline under its thread in one
`update-ref --stdin` transaction; a worktree holding ignored files is kept
(`KeptIgnored`) like a dirty one; a recreate removes only that path's stale
registration, and a failed or timed-out `worktree add` is abandoned),
`thread_ship.rs` (`thread_git::ship`, protocol 37: a checkout's branch,
remote, upstream counts, default branch and changes; what Commit stages (a
worktree's every change, a Local thread's own files, never a credential
path); commit of exactly those paths with the repository's hooks, a new
branch, push re-read through `ls-remote` when it fails; the `gh` calls for
pull requests, checks, failed logs, review threads and merge; every git and
`gh` child loses the app's and providers' secrets from its environment, and
what they print passes `redact_credentials`).
Every Review and Threads git child except the explicit commit, push and
new-worktree calls (which run the repository's hooks anyway), and every
registry `SystemGit` call, runs none of the programs the repository's own
settings name: filter drivers, ssh, askpass, credential, signing and Git LFS
extension programs are listed first (`git config --show-scope`, which runs
nothing) and overridden for the call through `GIT_CONFIG_COUNT`, lazy fetches
and `ext::` are off, and the Ship card's `ls-remote` names git's own
upload-pack (`interface_common::git_hardening`). A filter only a populated
submodule's own settings define is still reachable; see that module.

**crates/interface-workspace** — `search.rs` (the shared credential/generated/
binary classifiers plus the bounded, link-refusing project walk),
`editor.rs` (`WorkspaceAuthority`: authorized-root set, verified read, directory
listing, SHA-256 compare-and-save with atomic replace), `inventory.rs`
(read-only rules/skills/hooks/MCP/commands inventory with per-kind caps),
`fsid.rs` (containment helpers shared by the other three, and the dev/ino
identity they take from `interface_common::fsid`; the editor pins directories
with `interface_common::fsid::DirectorySnapshot`).
Search excludes `target` and `.worktrees`. Each directory read uses the remaining
entry budget plus one lookahead: if truncated, its filesystem-selected subset is
sorted for presentation and incompleteness is reported. It is not a claim to
return the globally first alphabetic entries. Untruncated ordering is unchanged.
Quick Open and search share one walk (`ProjectWalk`) but stop differently:
Quick Open checks its file budget before every entry and stops before reading
another directory once either budget is spent; search checks its file and byte
budgets only at regular files and still reads a queued directory with an empty
entry budget. `walk_budgets_stop_each_entry_point_where_they_always_have` pins
both.

**crates/interface-prompt-context** — `path.rs` (lexical absolutization,
containment tests, the three canonical-with-identity-recheck resolvers),
`image.rs` (bounded decode/downscale to a PNG data URL), `service.rs` (dispatch
for the four frozen operations, exact-grant memory, pasted-image write/prune),
`validation.rs` (per-item validation and the credential-filtered bounded review
snapshot embedded for diff items).

**crates/interface-terminal** — `service.rs` (`TerminalService` registry:
lifecycle ops, generation fencing, the lock-order rules, lifecycle event
publication, and `spawn_generation`/`install_generation`, the one fail-closed
spawn and install path create and restart share), `service/teardown.rs` (the
PTY reader and waiter of each generation, and teardown that keeps a workspace
lease until the process tree is proven gone), `service/output.rs` (the bounded
single-consumer `OutputBuffer` and its reader), `service/provider_session.rs`
(the provider-session lookup trait, capture owner and lifecycle sink),
`service/titles.rs` (pane-name precedence and OSC auto-title policy),
`service/handoff.rs` (pass-off directory, transcript file and prompt),
`service/shell_handoff.rs` with `shell_handoff.rs` (protocol 50: which clean
agent exits are owed a shell — never one `close` asked for, never twice — and
the create that starts it in the exited agent's folder under its root),
`service/callers.rs` (which pane a pane-tool caller belongs to),
`service/servers.rs` (server links in a pane's process tree),
`service/ticker.rs` (the
one 1 Hz thread that runs every pane's workspace-sleep policy and its armed
shell-display and Grok-title refreshes on up to four workers),
`service/provider_recovery.rs`
(provider capture, receipt/recovery, per-terminal-generation serialization and
`bind_provider_session`, the one rule for filing a provider binding),
`service/polling.rs` (hook-reset recovery backoff), `embedded.rs` (portable-pty
backend: spawn, split handles, SIGHUP-then-kill process-group teardown;
`embedded/workspace_sleep_process.rs` holds its STOP/CONT),
`launch.rs` (host-owned launch profiles and chat-session argv binding;
`resolve_sign_in` runs a profile CLI's own login for a `signIn` create; the
service holds the catalog behind a lock and `replace_launch_catalog` swaps
the whole of it on a provider refresh, each launch reading one snapshot),
`screen.rs` (a rendered CLI screen's bounds, empty-prompt and terminal-report
rules, shared by autonomous pane input and workspace sleep),
`attention.rs` (whether a pane's program waits for the user: protocol v44
`TerminalSummary.attention`, fed the reader's visible bytes and every input
write inside `WorkspacePower`, published by `workspace_sleep::publish_attention`
as a lifecycle upsert; its byte scanner mirrors the renderer's and both replay
`electron/src/fixtures/terminal-attention-v44.json`),
`process_probe.rs` (`PsProbe`, the one way the terminal crate and the host
run `/bin/ps`: argv, C locale, closed stdin; each caller keeps its columns,
bound and failure prose),
`osc_title.rs` (streaming OSC 0/2 scanner + title sanitizer), `safety.rs`
(approved-root allowlist and dev/ino directory snapshots), `external.rs` (open a
root in Ghostty or Terminal.app via `/usr/bin/open` argv), `error.rs` (every
terminal error type, `TerminalBackendError` included), `provider_files.rs`
(`safe_file`, the symlink-refusing open the Codex and Grok name lookups share).

**crates/interface-desktop-services** — `service.rs` (`DesktopService`: store
ownership, workspace authority, the covered-operation routing table, shared
error mapping to stable codes and prose) and its submodules under `service/`:
`terminal_state.rs` (the saved terminal roster and host-private reopen
authority under one lock, the host restart API and the three roster
operations), `workspace_leases.rs` (cleanup leases and removal reservations),
`appearance.rs`, `acp_profiles.rs`, and `workspace_ops.rs` (directory grants,
editor reads and saves, Quick Open and search, customization, Review checks
and checkpoints); `checkpoint.rs` (Review checkpoint
create/list/restore: TOCTOU-hardened capture, recovery checkpoints, atomic
temp+rename restore), `payload.rs` (strict camelCase `deny_unknown_fields`
payload structs and the raw ACP profile shape checks, which apply the
`interface-settings` field rules in wire order), `error.rs`.

**crates/interface-updater** — `service.rs` (`UpdaterService`:
configure/enablement, native event ingestion, verification orchestration,
subscriber fan-out), `state.rs` (`UpdateMachine`: phase transitions, sticky
Ready, generation-fenced verification tickets, sanitized error text),
`macos.rs` (codesign/ditto with bounded output and timeouts, private 0700
workspace, single-top-level-bundle check), `feed.rs` (URL admission policy:
loopback http or remote https, no credentials or query), `local_feed.rs`
(loopback GET/HEAD-only private feed over an inode-pinned directory),
`signature.rs` (designated-requirement parsing and codesign argument
construction; codesign's own ` /* exists */` annotation after a bracketed
field is the one comment accepted, read as the equivalent `exists` in both
the check and the `-R` argument, and any other comment or `or` is refused).
Public channel
([Public updates](../PUBLIC_UPDATES.md)): `channel.rs` (`ReleaseChannel`: the
feed addresses and Ed25519 keys compiled in from `release-channel.json`; off
with no feed or no key), `signed_manifest.rs` (the `interface-update.json`
envelope: signature checked before the payload is parsed, unknown fields
ignored so an old build reads a newer manifest), `remote.rs` (`RemoteFetcher`
and its rustls `UreqFetcher`; `refresh` fetches and verifies the manifest,
downloads the archive, checks size and SHA-512, then renames archive and
`latest-mac.yml` into `update-feed`; `prepare` frees an installed release's
archive; `update-channel.v1.json` keeps the newest version seen signed).
`service.rs` runs it (`check_channel`, `spawn_channel_schedule`) and, while
a channel is followed, drops the installer's own "checking / nothing new /
progress" events, which describe the loopback folder and not the feed.

**Entry points** — `PrivateDirectory::open{,_or_create}`, `WriterLock::acquire`,
`read_json`/`atomic_write_json`, `compare_and_set_json`, `quarantine_file`;
`SessionRegistry::open(_named)`, `ManagedWorktreeStore::open(_named)`,
`CheckpointStore::open`; the three settings stores' open/load/save;
`read_repository_review(_with)`, `discover_review_checks`, `run_review_checks`;
`PromptContextService::new` then `dispatch`; `WorkspaceAuthority` and
`list_project_files`/`search_project`/`read_customization_inventory`;
`TerminalService::new` (the only constructor, taking a
`TerminalServiceParts`) and `create`/`rename`/`resize`/`write`/`close`/
`restart`/`dismiss`/`take_output`; `DesktopService::open` and
`dispatch_blocking_with_session_roots`, `grant_workspace_root`,
`acquire_workspace_lease`/`release_workspace_lease`; `UpdaterService::configure`,
`native_event`, `begin_check`, `request_install`, `subscribe`.

**Invariants**

- Reads never mutate. The only deliberate exceptions in this scope are
  `SessionRegistry`'s legacy 0644→0600 repair and settings' `prepare_store`,
  which chmods and flushes a parent or file only when its mode is not
  already 0700/0600.
- Every mutation holds a `WriterLock` lease and passes an exact `TargetSnapshot`.
  Any failure **after** `fs::rename` must surface as `WriteUncertain` — never
  `WriteFailed`, never `Ok`. Do not simplify away the `renamed` flag or the
  repeated `assert_held_for`/`assert_directory` calls.
- One live writer per data root. `SessionRegistry::open` propagates a held lease
  as a hard startup error even though every other cache failure degrades to an
  empty in-memory projection.
- Persisted formats are frozen: `registry.json` is the legacy
  `{sessions, titles?, forgotten?}` object; `worktrees.json` is
  `{version:1, worktrees:[...]}` pretty-printed with **no** trailing newline;
  checkpoint `manifest.json`/`journal.json` are compact JSON plus one newline.
- `SessionRegistry`'s first observable revision is 1, not 0.
- Checkpoints never persist credential-shaped, binary or unsafe-relative paths.
  An empty Turn checkpoint is reported to the caller but is not made durable and
  does not bump the revision.
- Checkpoint durability order: each staged blob, `manifest.json`,
  `journal.json` and the `blobs` directory get a plain `fsync(2)` (rustix);
  one `F_FULLFSYNC` on the staging directory (std `sync_all`) persists them all
  before the rename publishes the manifest; the store sync after the rename
  makes the publication durable. On Apple a plain fsync leaves data in the
  drive cache, which may persist it after the rename, so the barrier must stay
  a full sync and stay before the rename.
- `interface-workspace::is_search_excluded_path` is the single shared classifier
  for listing, search, review, inventory and prompt-context authorization — but
  **not** for checkpoints, which keep a private copy.
- Desktop-service lock order is `workspace_leases -> terminal_state`. The saved
  terminal roster and the reopen-authority sidecar live in one
  `Mutex<TerminalState>`, so a restart decision never sees one without the
  other; helpers that change the sidecar take `&mut TerminalState`.
- Terminal lock order is `lifecycle_gate -> event_gate -> state`, and every
  lifecycle event is published while `event_gate` is held so revisions and the
  physical effect can never reorder.
- `TerminalServiceConfig::validate` enforces `output_read_chunk_bytes <=
  output_capacity_bytes`; violating it makes `OutputBuffer::push` block forever.
- Every terminal mutation is generation-fenced; dismissed ids go into `retired`
  and can never be recreated.
- `finish_process` drops the workspace lease **before** publishing `Exited`, so
  no snapshot consumer can schedule worktree cleanup while a shell owns the cwd.
- `title_is_user` latches on rename and blocks OSC auto-titles forever after.
- Codex panes launch with the process-local override
  `-c tui.terminal_title=["spinner","thread"]`: Codex announces activity and the first-prompt title,
  then its chat name (including `/rename`) over OSC. `task_title.rs` removes
  conversational lead-ins from automatic request-style labels and clips long
  labels at whole words, locally and without provider calls. Explicit Interface
  renames bypass this formatting. No global config write or
  hook approval is needed for titles. Bare provider UUID titles are ignored so
  a new pane keeps its numbered label until it has a useful title.
- Grok keeps the chat id Interface mints, but no hook advances a Grok binding
  in the host-private reopen authority, so its saved `resume` flag is
  `false` for life. Launch resolution decides `--resume=<id>` versus
  `--session-id=<id>` by whether `~/.grok/sessions/*/<id>/` exists
  (`TerminalLaunchCatalog::with_grok_sessions_root`, `GROK_HOME` honoured);
  `--session-id` on an id Grok already holds exits with "already in use".
  The same id names the pane. Grok's own terminal title stays the app name
  plus activity, which Interface ignores. While the pane is running, a poll
  reads `summary.json` `generated_title` (including `/rename`) and, before
  that exists, the first real `chat_history.jsonl` request. Explicit Interface
  renames still win. A later generated title replaces the request label.
- Login-shell foreground program names, including Claude's native
  `claude/versions/<major.minor.patch>` executable paths, update Codex/Claude
  display profiles without minting session IDs. Manually launched providers
  receive the same Clean composer. Inspection is bounded and skipped for idle
  shells; generation and captured-provider fences preserve lifecycle ownership.
  The private zsh overlay routes bare `codex` through the host's
  `--terminal-manual-codex-wrapper` (the Codex twin of the bare-`claude`
  wrapper): it claims the shell pane's capture generation and execs Codex
  with the same inline hook, so a Codex started from a shell pane is promoted
  to a Codex pane and reopens as `codex resume <id>`. An armed shell is polled
  as Claude; the host falls back to the generation's Codex receipt, and the
  receipt's UUID version (v7 is Codex) picks the promoted profile. Without a
  host-resolved Codex the overlay only adds the title setting. User
  aliases/functions still win and no global config is written.
- Renderer input can only name a launch profile id and, for a pass-off, supply bounded transcript text; executable, argv shape, the pass-off prompt template, and environment come from the host-built `TerminalLaunchCatalog` and the terminal service.
- A handoff carries an optional `kind`: absent or `transcript` is the renderer's
  rendered terminal output ("you are taking over"); `brief` is a task an agent
  wrote through the host's `pass_off` tool, and its file names the source pane
  to answer with `send_to_pane` ("another agent passed you a task"). Either
  way the summary echoes the source as `passedOffFrom`, kept across restarts,
  so a renderer that did not create the pane can seat it beside its source.
- Every launch, login shells included, gets the catalog's pane environment
  (`with_pane_environment`, today `INTERFACE_PANE_MCP_SOCKET`), appended after
  the profile's own entries. Grok panes launch `--no-leader` so the MCP servers
  Grok spawns are children of the pane rather than of its leader daemon.
- `TerminalService::terminal_for_process_ancestor(pid)` names the running pane
  whose child process a pid descends from: one `/bin/ps -axo pid=,ppid=`
  listing (`proc_ancestry.rs`), walked upward at most 32 hops; only `Running`
  entries with a known pid own anything, and no match is a refusal.
- The launch catalog retains each host-selected launcher path, including
  installer-managed symlinks. Each new or restarted pane canonicalizes and
  validates its current executable target so installed CLI updates take effect
  without restarting Interface. Existing panes keep their running process.
- A Claude pane's activity dot follows Claude's own turns: the same inline
  `--settings` hook command is installed on `UserPromptSubmit`, `PreToolUse`
  and `Stop`, and the helper answers those events with a `terminalSequence`
  OSC 2 title `interface-claude-activity:working` / `…:idle`
  (`CLAUDE_ACTIVITY_TITLE_PREFIX`). A `Stop` whose `background_tasks` still
  lists a running `subagent` or `workflow` answers `working`: Claude ends the
  foreground turn while that work continues, and the task's completion starts
  the turn whose `Stop` reports idle. Background shells do not count, because
  servers and watchers may never exit. The same command on
  `PermissionRequest` answers `…:waiting` (and no decision, so Claude still
  shows its dialog): it fires the moment a permission dialog, AskUserQuestion
  or plan approval appears, so the pane's pip and row read "needs you" until
  input reaches the pane, the next tool call or the turn ends. Auto-approved
  tools never fire it. Claude's `permission_prompt` Notification arrives about
  six seconds later and is not used. These events get that frame instead of
  a receipt. The title scanner drops that frame so the tab is never
  renamed; the renderer's `terminal-activity.ts` reads it and holds "working"
  until the idle frame, because Claude Code never animates its title the way
  Codex and Grok do, so without it a Claude pane could never show as working.
- A Codex pane's thread id comes from Codex's own `SessionStart` hook, which
  fires on the first prompt: the host injects it per launch as
  `-c hooks.SessionStart=[…]` (fixed command, capture token in
  `INTERFACE_CODEX_CAPTURE_TOKEN`), the helper appends a receipt to the same
  capture generation Claude uses (`.interface-codex-receipts.json`), and the
  reopen is `codex resume <v7 id>` while that rollout still exists under
  `~/.codex/sessions` (`TerminalLaunchCatalog::with_codex_sessions_root`); a
  deleted or archived thread reopens fresh. Codex asks the user to trust that
  hook once ("Hooks need review"); a pane with no receipt yet (hook untrusted,
  or no prompt sent) saves id-less and reopens fresh. A Codex `startup`
  receipt replaces a stale resume binding (Codex has no provisional startup),
  so a pane that had to launch fresh follows its new thread instead of
  reopening fresh forever. Only a legacy v4 handle
  still gets `codex resume --last` while no other live Codex terminal shares
  that authorized cwd and bare `codex resume` with a peer, because two panes
  on one thread collide on its writer lock. The peer scan runs under
  `lifecycle_gate`.
- Integrated terminal launches never ask provider CLIs to create a worktree.
  Fresh panes stay in the checkout the user selected; pass-offs stay in the
  source agent's live working directory. Composer, Agent Canvas, and
  orchestration worktrees remain separate, explicit per-launch choices; there
  is no global worktree default.
- Fresh terminals can reuse an exact saved workspace root backed by a successful
  host-owned terminal launch, including after app restart clears picker grants.
  The saved roster must still match the private startup proof; remembered paths
  and child directories grant nothing. Managed roots retain their ledger-key
  verification, and the proof is rechecked before publishing the new terminal.
  This does not add workspace-editor authority or change saved-terminal schemas.
- A fresh launch whose cwd is a symlink to an exact approved root (a project
  moved with a link left at its old path) starts in that canonical root
  (`ApprovedTerminalRoots::resolve_linked_root`). Links into a subfolder or
  outside every root still refuse. Saved reopens keep their recorded path, so
  a saved pane at a now-linked path still fails as `terminal-cwd-changed`
  until the owner confirms the move on the failed tab: that roster save carries
  `movedFolders`, and the host re-proves the link and moves the pane's restart
  proof with it (see `docs/PROTOCOL_FACTS.md`, v27).
- Desktop dispatch distinguishes `canonical_authorized_path` (anything under an
  authorized root) from `exact_authorized_root` (the root itself). Review,
  customization and checkpoint ops require the exact root.
- Updater `requirement == None` means "this platform needs no artifact
  verification" and downloads go Ready unverified. `configure()` clears
  `requirement` before awaiting codesign, so it must be called exactly once
  before any native event can arrive (`electron/src/main/main.ts` does this
  before `updater.start()`).
- Verification is generation-fenced by `VerificationTicket`, and
  `complete_verification` only promotes Downloading → Ready.
- Checkpoint restore writes the recovery checkpoint before any rename, and every
  capture re-asserts root and parent dev/ino before and after reading.
- This whole scope is Unix-only in practice: `interface-storage/src/secure.rs`
  carries a non-Unix `compile_error!`.

**Gotchas**

- Deliberately retained read-only helpers. Genuinely zero-caller:
  `load_managed_worktrees_read_only` and `read_checkpoint_blob_read_only` (the
  latter's one caller became `CheckpointBlobReader::open_read_only`).
  Test-only: `read_registry_file`. Still live:
  `list_checkpoints_read_only`/`inspect_checkpoint_read_only`, both called from
  `interface-desktop-services/src/checkpoint.rs`. Do not remove any of them
  because grep is empty.
- `COVERED_OPERATIONS` (`interface-desktop-services/src/service.rs`; its array
  type carries the count) is consumed by `apps/interface-host/src/router.rs` for
  ownership and count assertions. Adding a dispatch branch means editing both.
- Frozen and retained here too: `read_file_snapshot`, `save_file`,
  `set_workspace_open`, `search_project`, `list_review_checkpoints`.
- `checkpoint.rs`'s `deserialize_required_nullable` looks like a no-op wrapper
  around `Option::<String>::deserialize`. Combined with `deserialize_with` and no
  `#[serde(default)]` it makes `repositoryHead` present-but-nullable; deleting it
  is a schema change.
- The `WriterLock` file is intentionally never unlinked on release — unlinking
  would split ownership between an old open inode and a new same-name inode.
- `SessionRegistry`, `ManagedWorktreeStore` and `CheckpointStore` hold their
  writer lease for the object's lifetime.
- The credential/binary path *tables* are shared via `interface-common`
  (2026-08-29), but the classifier *rules* in `interface-registry/checkpoint.rs`
  and `interface-workspace` remain separate copies that already disagree on
  names like `.environment`; changing a rule means mirroring it.
- Project reads that can meet a file iCloud has evicted go through
  `interface_common::bounded_read::within` (2026-10-14): Threads' copy of the
  real index (`thread-git-downloading`), Review's untracked file reads
  (`VERIFIED_READ_LIMIT`, 5 s; after one stall the rest are omitted unread),
  Quick Open's listing (`PROJECT_LISTING_DEADLINE`, 10 s; a dataless file is
  listed from its `lstat` and never opened), the retained `searchProject`
  channel (`PROJECT_SEARCH_DEADLINE`, 10 s; 2026-10-15) and the editor/output
  preview reads and folder listings (`EDITOR_READ_DEADLINE`, 10 s). Past its
  deadline a read fails saying the file is still downloading from iCloud; the
  blocked reader thread is abandoned. git itself stays bounded by its own
  timeouts, whose Threads and Review messages say the deadline and that
  iCloud may still be downloading (Review's is `git-error`, no longer "Git is
  not available"). Tests stand a FIFO (which blocks forever on open) in for an
  evicted file.
- `RepositoryReviewHooks`, `ProjectWalkHooks`, `EditorHooks` and `CheckpointHooks`
  are adversarial-race test seams. The repeated `assert_directory` /
  `assert_verified_file_path` calls around each hook are the point.
- `WorkspaceAuthority::save_text` re-reads and re-SHA-256s the target several
  times per save. That is deliberate TOCTOU narrowing under a 5 MiB ceiling.
- `interface-review` maps a truncated-output run to exit code 0 and a spawn
  failure or timeout to `REVIEW_GIT_UNAVAILABLE` (-1); -2 is an internal-only
  sentinel that never reaches the DTO.
- Review patches come from one batched `git diff` per layer (numstat is batched
  the same way). A file's batched output is used only when it is attributed by
  its exact `diff --git a/<p> b/<p>` header, fully received, valid UTF-8 and not
  a gitlink; it is then capped exactly as its own bounded call would be.
  Conflicts, renames, untracked files, paths git would C-quote and every file a
  failed, truncated, stderr-writing or unattributable batch left behind keep the
  per-file call. The real-git differential test in `tests/suite` pins the two
  paths byte for byte.
- `SessionRegistry::open` prints diagnostics to stderr and degrades to an empty
  in-memory cache for almost every failure. A test asserting "the registry
  loaded" must check the data, not that `open()` returned `Ok`.
- `TerminalState::Starting` is never produced by `TerminalService` (create
  publishes Running immediately) but the renderer reads it. The `== Starting`
  guards are defensive, not dead branches.
- `TerminalExit::signal` is always `None`, and `code` is `None` whenever the child
  died from a signal: portable-pty 0.9 exposes a signal *name* while the frozen
  DTO wants `Option<i32>`. A SIGHUP'd shell reports an empty exit object.
- Process-group signalling and `kill -0` liveness probes are in-process
  `kill(2)` calls through rustix's safe wrappers (`unsafe_code = "forbid"`
  rules out `libc::killpg`); only the `ps` identity snapshot still spawns.
- `kill(-pgid, 0)` reports EPERM, not ESRCH, for a group whose only members are
  exited-but-unreaped zombies (macOS killpg skips exiting processes), so
  `process_group_exists` treats EPERM as absent while capture liveness
  (`captured_process_group_is_live`) still counts it as present.
- desktop-services carries two different path-stability policies:
  `checkpoint::stable_directory` requires canonical == requested (rejects any
  symlinked component), while `service::canonical_grant_candidate` accepts a
  symlinked ancestor. They look mergeable and are not.
- `OscTitleScanner` is not an emulator: OSC 0/2 only, and `sanitize_title` strips
  spinner/braille decoration so a ~10 Hz animated title collapses to one string.
  Changing that causes a rename storm on the event channel.
- `apps/interface-host` owns the real terminal backend/lease/lifecycle wiring;
  the crate's `Noop*` types and `TerminalService::new` are an unused convenience
  constructor.

### OMP terminal profile

Host protocol v7 adds the `omp` launch-profile discriminator. The host discovers
`omp` through the cached login PATH or supported `.local/bin`, `.bun/bin`, npm,
and Homebrew locations. OMP launches directly with no model, permission, or
worktree flags; it inherits the selected project cwd. Its existing launch and
workspace DTOs remain v1. OMP owns conversation ids, so Interface does not mint
or infer a resume binding. Pass-off text uses OMP's positional initial prompt.
The shared `electron/src/fixtures/terminal-omp-v7.json` pins Rust/TS wire bytes.

## Terminal Fast mode (host protocol v8)

Codex panes configure a Shift-F12 native Fast action and a title projection of
spinner, app name, Fast state, model, run state, and thread. `fast_mode.rs`
parses the fixed prefix; `service.rs` emits optional `TerminalFastModeV1` in
summary upserts and retains only the thread as the visible pane title. This
is display metadata, never session identity, resume authority, or proof that
a paid turn was served at a tier. Missing/unsupported metadata hides the
control; restarts discard it. The command uses the CLI's own capability and
idle/modal guards and preserves drafts. No global keymap or title config is
written; manual shell wrappers and direct launchers receive the same flags.

Protocol v8 keeps 104 request and 10 push channels, terminal data v2, and all
persisted schemas. The Rust/TypeScript fixture is `terminal-fast-mode-v8.json`.
The renderer waits for a state change; it never retries or lights the bolt
optimistically. This integration currently covers Codex's advertised Fast
models; other CLIs expose no compatible live state/action and omit the bolt.

## Terminal startup downloads (host protocol v31)

Claude, Codex and a git-aware shell prompt read the project's git HEAD, config
and branch ref and its CLAUDE.md/AGENTS.md chain before their first paint. In
a folder a File Provider (iCloud Drive with Optimize Mac Storage) has evicted,
each read blocks until that file downloads, one after another; on 25 Sep 2026
five such files kept new Claude panes blank for 50 s. `startup_files.rs` asks
for all of them at once, one thread per path, off every service lock: project
files in the cwd, instruction files in each ancestor up to (not including)
`$HOME`, and each ancestor's `.git` (a directory, or a linked worktree's
`gitdir:`/`commondir` pointer), whose HEAD then names the branch ref to fetch.
A metadata probe never downloads; only `SF_DATALESS` files are read. HEAD refs
that leave the git directory are ignored.

`service/startup_download.rs` starts (or joins, per cwd) that fetch on create
and restart. The ticker projects what is still in flight onto the summary as
`TerminalStartupDownloadV1` (three names relative to the cwd or `~/`, and a
count) once the fetch is a second old, clears it when nothing is pending, and
never persists it; process exit and restart clear it too. Every five minutes
the ticker also re-fetches running panes' cwds so re-evicted files are local
before the next pane starts. `TerminalServiceParts::startup_files` is the test
seam; the fixture is `terminal-startup-download-v31.json`.

### OpenCode terminal profile

Host protocol v9 adds `opencode`. The host discovers OpenCode through the login
PATH, its official installer directory (`~/.opencode/bin`), and common npm, Bun
and Homebrew locations. It launches directly in the selected folder with `--auto`
on fresh launches and reopens, auto-approving permissions except explicit denies.
OpenCode still selects the model. Saved workspace DTOs stay v1. A saved
provider ID reopens with `--session=<id>`; no bounded recent-session listing may
silently discard that ID. Legacy panes without an ID ask the private TUI bridge
to open OpenCode's native session picker, even when panes share a folder. A
missing bridge reports an unavailable resume instead of starting fresh. Pass-off
text uses `--prompt` because OpenCode's positional argument is a project path.
The shared `electron/src/fixtures/terminal-opencode-v9.json` pins Rust/TS wire bytes.

### Muse Code terminal profile

Host protocol v35 adds `muse`, Meta's Muse Code. The host finds the `muse`
launcher through the login PATH, the installer's `~/.local/bin`, and Homebrew
locations, and launches it in the selected folder with no injected model,
permission or resume flags: Muse Code keeps its own permission profile.
Foreground detection accepts the launcher and the native `muse-bin-<version>`
it execs. Saved workspace DTOs stay v1; Muse Code resumes only through its own
`muse resume <id-or-name>`, so reopened panes start fresh. Its TUI documents no
initial-prompt argument, so pass-off refuses it and the pane menu omits it.
The shared `electron/src/fixtures/terminal-muse-v35.json` pins Rust/TS wire bytes.

### Kimi Code terminal profile

Host protocol v54 adds `kimi`, Moonshot's Kimi Code. The host finds `kimi`
through the login PATH, the installer's `~/.kimi-code/bin`, and Homebrew
locations, and launches it in the selected folder with no injected model,
permission or resume flags: Kimi Code starts in its own configured permission
mode (Always Ask out of the box). Foreground detection accepts `kimi` and the
`kimi-code` process title. The shared
`electron/src/fixtures/terminal-kimi-v54.json` pins Rust/TS wire bytes.

Protocol v55 makes Kimi panes resumable and gives them activity. Kimi Code has
no plugin-install command, so the host registers a namespaced
`interface-checkpoints` plugin once the way Kimi's `/plugins` panel does: a
managed copy under `<KIMI_CODE_HOME>/plugins/managed/` (default
`~/.kimi-code`) and a record appended to `plugins/installed.json`, written only
if that registry is unchanged since it was read (Kimi takes no lock). A record
already under that id, or one the owner later disables or removes, is never
touched again (`kimi-checkpoints/installed-v1` in the runtime root). Kimi runs
plugin hooks through `sh -c` in the pane's own environment, so each hook routes
by the pane's private `INTERFACE_NATIVE_*` variables, like Antigravity's, and
does nothing outside Interface; stdout stays empty and the exit is always 0, so
no hook can block, answer or add to a turn. `native_checkpoint::KIMI_HOOK_EVENTS`
lists the 13 observed events. Kimi's payload has `hook_event_name` and
`session_id` (`session_<v4 UUID>`) but no cwd, so the capture token and process
lineage alone bind it. The binding is `kimi-<uuid>` and reopens as
`kimi --session session_<uuid>`; Kimi creates its session lazily on the first
prompt, so a pane that never sent one reopens fresh. `SessionStart` with
`startup` (`/new`) or `resume` (a session switch) moves the binding. Every Kimi
agent reports under the root session id, so `Activity` counts open subagent
tasks (`SubagentStart`/`SubagentStop`) and only a `Stop` with none open ends the
turn; work followed by idle is reported `complete` (an `Interrupt` is plain
idle). `PermissionRequest` and the `AskUserQuestion` tool wait.

Kimi Code 2.3 rejects positional arguments (`-p` is headless), so a Kimi
pass-off is typed instead (`launch::HandoffDelivery::Typed`): the host watches
the new pane's output until its editor enables bracketed paste and output rests
600 ms, then pastes the one-line brief and sends Enter 150 ms later, once. A
pane that ends, restarts or is not ready within 60 s is never typed into. The
shared `terminal-kimi-resume-v55.json` pins the binding and resume argument for
Rust and TypeScript.

OpenCode 1.18.29 titles (`OpenCode` / `OC | <name>`) contain no activity state.
The host installs a private `OPENCODE_TUI_CONFIG` overlay pointing at the
dependency-free `opencode_activity_transport.mjs`. It copies only selected
session IDs, parent IDs, native status and pending permission/question counts
from the TUI API; global/project settings and model/permission policies remain
OpenCode-owned. An existing explicit `OPENCODE_TUI_CONFIG` is preserved and the
bridge is skipped with a diagnostic.

`opencode_activity.rs` consumes bounded private v1 OSC snapshots in the PTY
reader, before any renderer/replay output. Rust decides waiting before working
before idle, scopes child approvals to their visible parent, and emits a
versioned activity frame. Same-session transitions alone report completion;
heartbeats restore activity after renderer reconnects. The shared
`terminal-opencode-activity-v1.txt` fixture pins this private stream contract;
NDJSON v9, terminal data v2 and saved workspace schemas are unchanged. Each
generation gets a fresh bridge. Existing OpenCode processes need a new launch
to load the private plugin; a host refresh cannot inject it into a running TUI.

The same transport atomically copies the selected session into a private v1
observation under its capture-generation directory. Rust validates the capture
token, reporting PID and recorded cwd against the host's process owner before
filing the session ID. Selecting an older chat or switching sessions updates the
binding; shared log timestamps never select a pane's conversation. The host
also reconciles this observation after a crash before reopening saved panes.
The cross-language fixture is `terminal-opencode-session-v1.json`. JavaScript
writes only this provider observation; Rust alone writes the canonical roster.

Upstream contracts: `anomalyco/opencode` tag `v1.18.29`,
`packages/plugin/src/tui.ts`, `packages/tui/src/app.tsx`,
`packages/tui/src/routes/session/index.tsx`, and
`packages/opencode/src/config/tui.ts`.


Appearance material preferences use the opt-in `surfaceVersion: 1` extension on
`getAppearance`/`setAppearance`. Legacy requests and theme response bytes remain
unchanged. The versioned set variant supplies `preference: {version: 1, terminal,
sidebar}` (integer percentages 0–100) and changes only those values. V1 responses
include `translucency` with that same shape. Legacy Electron clients negotiate v1; Rust rejects
unknown versions and invalid values before writing. Both languages share
`docs/fixtures/surface-translucency-v1.json`. No new desktop channel is added.
The Rust-owned `surface-translucency.v1.json` sits beside the existing
`appearance.json`; material writes never rewrite the legacy theme file.

Appearance surface protocol v2 extends v1 with `panePresentation: {version: 1,
paneStyle: "terminal" | "clean"}`. Its set variant uses that same object under
`preference` and requires `surfaceVersion: 2`. Current Electron negotiates v2;
v1 and unversioned responses keep their accepted shape. Rust stores the choice
in `pane-presentation.v1.json`, independently of theme/material/workspace files.
`docs/fixtures/pane-presentation-v1.json` pins both languages. Neither appearance
choice creates a provider conversation, mutates a workspace, or restarts a PTY.


## Clean terminal settings (host protocol v13)

Retired with the Clean GUI. `iface:advanceTerminalControls` stays wire
protocol (`docs/CODEMAP.md`, "Deliberately retained, uncalled channels"): the
host still serves it, validates the request version, the bounded screen and
the current running generation, and answers a fixed unavailable
`TerminalControlsStateV1` (not ready, not pending, model and effort null, an
error saying the controls are no longer available). It never writes to the
PTY. The settings automation it used to run (native menu recognition, `/model`
and `/effort` transactions) is on the archive branch named in
`docs/archive/clean-gui-20260914/README.md`. Protocol v13 added the one request
(106 requests, 10 pushes); terminal data v2 and persisted schemas are
unchanged.

### Project pins (appearance surface v4)

`ProjectPresentation` validates at most 128 unique absolute project paths (4096
UTF-8 bytes each, no control characters). Rust stores them atomically in
`project-presentation.v1.json`, separate from workspace pins and every legacy
appearance file. Host protocol v19 opts Electron into surface v4; v1-v3 response
shapes remain unchanged. `docs/fixtures/project-presentation-v1.json` is the
cross-language contract. A pin is navigation metadata, never a filesystem grant.

### Terminal text size in half steps (appearance surface v6)

`WorkspacePresentationV2` holds the terminal text size as a count of half CSS
pixels (10–26px, default 11.5) beside the same workspace pins as version 1.
Rust stores it in `workspace-presentation.v2.json`; until that file exists,
`AppearanceStore::workspace_presentation` carries forward what an earlier build
saved in `workspace-presentation.v1.json`, which is read and never written
again. Host protocol v70 opts Electron into surface v6. Surfaces v3–v5 keep
version 1 on the wire: they read `WorkspacePresentationV2::legacy` (the next
whole size, never under 12) and their saves are kept as version 2.
`docs/fixtures/workspace-presentation-v2.json` is the cross-language contract.

Workspace terminal sleep: `interface-terminal/src/service/workspace_sleep.rs`
owns idle policy; `embedded/workspace_sleep_process.rs` applies exact-identity
STOP/CONT. Suspends and resumes serialize on each pane's transition gate, not
on the power lock the PTY reader takes per output chunk; a probe in flight
reads as sleeping, so input waits for it and then wakes the pane.
See [workspace terminal sleep](../WORKSPACE_TERMINAL_SLEEP.md).

Output previews use `interface-workspace/src/output_preview.rs`, sharing the editor's descriptor and identity checks without weakening its text-only policy. See [output preview workspace](../OUTPUT_PREVIEW.md).

Antigravity and Muse panes use `interface-terminal/src/native_checkpoint.rs`
for native hook contracts, activity and tagged exact resume IDs.
AGY's native OSC 7501 status takes precedence over capture-hook activity titles
once seen, for Rust attention/workspace sleep and the renderer's completion
projection. Stop hooks run before the final TUI state and can be vetoed; an
interleaved hook must neither hide `done`/`blocked` nor put a still-working
pane to sleep. `electron/src/fixtures/terminal-agy-activity-v1.json` pins that
ordering on both sides, including split frames, errors and duplicate completion.
Other providers retain their authoritative hook streams.
A captured provider's later conversation (a new Antigravity conversation, Muse
clear/resume/fork, an OpenCode session switch) reaches the saved roster through
`TerminalWorkspaceStore::reconcile_terminal_provider_reopen`, which takes every
captured profile, not only Claude and Codex. A capture that cannot be filed is
never retired, and `register_capture_owner` then refuses every later launch of
that pane, so a captured profile this seam skipped would never reopen.
