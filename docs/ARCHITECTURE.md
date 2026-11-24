# Shipping architecture

Interface keeps its Electron/React presentation layer and moves the entire
operational core into Rust.

```text
React renderer
    │ frozen window.iface DTOs + push events
Electron preload/main
    │ versioned, bounded, correlated frames
Rust interface-host
    ├── contracts / settings / workspace / registry
    ├── orchestration / collaboration / review / updater
    ├── Claude / Codex / ACP engines
    └── terminal / PTY / worktree / process supervision
             │
             └── authenticated third-party CLIs and local OS services

iface (Ratatui) ── same contracts and Rust services ──┘
```

## Responsibility boundary

| Layer | Owns | Must not own |
| --- | --- | --- |
| React renderer | panes, layout, interaction, visual state | provider processes, canonical stores, authority decisions |
| Electron main/preload | windows, menus, app lifecycle, Rust-host supervision, DTO translation | duplicate service logic or direct domain-store writes |
| Rust host | validation, persistence, process effects, events, recovery, policy | renderer layout or unversioned UI assumptions |
| Engine adapters | provider-native protocol and lifecycle truth | renderer policy or duplicate registry state |
| `iface` | terminal presentation and commands | a second implementation of host rules |

## Host transport

The first transport is newline-delimited JSON over the supervised child
process's standard streams. Standard output is protocol-only; logs go to
standard error. Every request carries a protocol version, bounded request ID,
operation, and payload. Every response is correlated. The host emits a ready
frame before accepting traffic and eight frozen push-channel families. Frames,
concurrency, and outbound queues are bounded.

Version 2 retains newline-delimited JSON for correlated control requests and
low-volume lifecycle pushes, and adds a separately bounded binary data plane
for integrated-terminal bytes. Terminal output never competes with registry or
control responses on the JSON queue.

Electron treats an unexpected host exit as a visible backend failure. It does
not silently fall back to the legacy TypeScript implementation. Restart and
replay are permitted only for requests whose effect certainty is definitely
unsent; ambiguous writes remain quarantined.

## Feature boundary

The strict language port remains the accepted 0.2.0 baseline. Post-parity
features are added through versioned contracts without weakening its ownership
rules. Integrated terminals are the first expansion: Rust owns PTYs, process
lifecycle, terminal identity, worktree leases, and byte-stream integrity;
React owns terminal rendering and pane layout; Electron only supervises and
transfers the versioned control and binary data planes.

Teammates, memory, skills, Places, routines, plugins, and lineup presets remain
outside this terminal work unless separately authorized.

## Migration rule

One operation family moves only after its TypeScript behavior has golden
fixtures, the Rust implementation passes them, and the Electron bridge passes a
real request/push smoke test. The old implementation is then deleted. Rust and
Node never write the same canonical store concurrently.
