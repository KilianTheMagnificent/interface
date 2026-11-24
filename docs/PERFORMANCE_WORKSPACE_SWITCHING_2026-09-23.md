# Workspace switching — September 23, 2026

Switching workspaces discarded every departing pane's WebGL renderer, then
rebuilt it before painting when the pane returned. The emulator and PTY were
retained, but shader setup, renderer replacement and texture work still ran on
every switch. The regression is in `TerminalView`'s slot/parking lifecycle.

`terminal-webgl-pool.ts` now keeps up to 12 recent renderers per document.
Visible panes retain their renderer; only the oldest parked renderer can be
evicted. (5 October 2026: a renderer parked for 60 s, or every renderer after
2 min of a hidden window, is also given back; a switch inside that window
stays warm, and one after it pays the cold cost measured below.) Twelve leaves four slots below Chromium's measured 16-context ceiling.
Eviction releases the underlying GL context explicitly because xterm addon
disposal alone removes its canvas but defers context reclamation to GC.
The xterm emulator, parser, screen, scrollback and PTY generation remain intact.

Measured with real xterm/WebGL renderers in isolated Electron and synthetic
provider bytes, on the same machine and base `9bd3b24`:

| Eight switches between two four-pane workspaces | Before | After |
| --- | ---: | ---: |
| GPU renderer creations | 32 | 0 |
| GPU renderer disposals | 32 | 0 |
| GPU setup/teardown time, total | 281.8 ms | 0 ms |
| Median selection-to-two-animation-frames time | 55.1 ms | 16.5 ms |

Timings are informational, not timing assertions or live-provider benchmarks.
Cold workspaces and returns after cache eviction still initialize renderers.
Two eight-pane workspaces exceed the cache: that test measured about 58 ms per
switch while reusing half their GPU renderers. Tests visit five workspaces
(20 or 40 terminals), assert at most 12 actual live GL contexts, preserve
terminal identity/history/input, and verify context release on unmount.

The real app's hidden hands-free harness also completed 12 sidebar switches
between two four-pane fake-Codex workspaces. All eight DOM emulators and GPU
canvases were retained, host terminal IDs/generations were unchanged, output
survived, and fresh input echoed through the Rust host. The hidden app stayed
out of the foreground and was stopped after verification.

Evidence: `/private/tmp/interface-switch-before.log`,
`/private/tmp/interface-switch-after.log`,
`/private/tmp/interface-switch-hf-result.json`, and
`/private/tmp/interface-switch-verified.png`.

The first full-gate attempts hit renderer deadlines during macOS sleep/wake
cycles; `pmset` recorded repeated Maintenance Sleep/DarkWake transitions.
The focused pane-motion lane passed unchanged. The final
`cargo xtask check --changed` passed with a temporary `caffeinate -is` assertion:
2,777 JavaScript tests, 44 release tests, type checking, all real Electron
renderer lanes and production bundle builds. No test deadlines or checks were
relaxed. Final gate evidence: `/private/tmp/interface-switch-gate-awake.log`.

Live-provider, packaged-app, signing, updater and installed-production gates
were not run for this renderer-only Dev change.
