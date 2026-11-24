# xtask

Read root `AGENTS.md` and `docs/areas/xtask.md`. This crate owns repository tooling
and release coordination. Navigation: `cargo xtask navigate pipeline`.

- Keep commands in Rust; do not add dependencies for repository navigation.
- `navigation.rs` reads `docs/agent-map.json`; it displays checks, never runs them.
  New areas need routes, real entry/test paths and every applicable scoped rule.
- Navigation validation runs even for documentation-only `check --changed`.
  It is independent of cached Rust/Electron receipts.
- Preserve exact-source gate receipts, exclusive release locks and the separate
  authorities for Dev landing, promotion, staging, installation and restart.
- `closeout` runs the xtask built from the branch before it merges dev, so a
  guard fix that reached dev only applies from the next run. A refusal after a
  merge that changed xtask says so; rerun closeout with the same arguments.
- `worktree-prune` never removes the Dev checkout (`dev_restart::DEV_CHECKOUT`),
  the promote driver (`~/Developer/.interface-promote`), a release worktree,
  or a checkout of `dev` or `main`, and never deletes those branches. Move a
  managed checkout only together with its constant.

- `update-key` and `publish-update` (`update_channel.rs`) are the publisher's
  side of the public update channel. Publishing reaches every installed copy
  and is its own owner-requested step; read `docs/PUBLIC_UPDATES.md` first and
  never run it as part of promotion or a test.

Verify with `cargo test --locked -p xtask`, `cargo xtask navigate --check`, then
the full `cargo xtask check`. Do not call release commands as navigation tests.
