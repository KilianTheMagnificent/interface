# Typing, scrolling and terminal pane focus — 2026-09-11

The owner identified typing, scrolling and switching terminal panes as the worst
slowdown after the broad performance pass landed as `60c57b6`. This follow-up
traced native input, normal/alternate scrolling, xterm write scheduling, pane
focus/tab selection and geometry observation on that exact Dev base.

## Confirmed and fixed

`TerminalPane` reports both pointer and focus events. `focusTerminalPane` used to
return a new workspace even when the pane was already focused. Those duplicate
events invalidated the center's projections and triggered additional React work.
It now preserves identity for unchanged focus. Selecting an already-active tab
also preserves its pane array; a real focus change still moves the maximized pane
when required. Unknown pane/tab identities remain inert.

The center's focused-terminal fit effect depended on the entire terminal
registry. A title update in another pane therefore invoked xterm's FitAddon,
including computed-style and geometry reads, on the focused terminal. The effect
now follows focused stream identity and surface/modal transitions. TerminalView's
ResizeObserver, initial fit, theme/font handling and multi-pane geometry recovery
remain responsible for actual size changes.

## Real renderer evidence

The new isolated Electron lane mounts the real `IntegratedTerminalCenter` and four
xterm emulators. Each receives 5,200 history lines (the existing 5,000-row
scrollback retention stays unchanged). It then measures duplicate focus and
background title changes, scrolls the native buffer, and switches focus/sends
keys while all four terminals receive synthetic output every 8 ms.

| Operation | Before | After |
| --- | ---: | ---: |
| 12 pointer events on the already-focused pane | 36 React commits | 0 |
| 8 background terminal title updates | 8 focused-terminal fits | 0 |
| Pane switches under output load | 16 routed correctly | 16 routed correctly |
| Keystrokes through xterm under output load | 16 routed correctly | 16 routed correctly |
| Remaining output acknowledgements | 0 | 0 |
| Terminal remounts during interaction | 0 | 0 |

Normal scrollback visibly moved without emitting provider input. Existing
alternate-screen wheel, resize, layout/maximize and pane-swap fixtures remain in
the full renderer gate. The new focus identity test failed before the change and
passes after it, including maximized-pane transitions.

Informational local timings were 0.40 ms maximum key-event-to-input-callback
dispatch both before and after; maximum observed pane-focus completion was
14.5 ms before and 12.6 ms after. Focus is observed with a polling helper, so these
numbers are coarse. The evidence supports eliminating redundant work; it does
not establish a statistically significant input-latency improvement. Keys and
provider output are synthetic, with real xterm/React handlers; this is not a
hardware-keyboard-to-live-provider or production-workload measurement.

Reproduce from `electron/`:

```sh
INTERFACE_TERMINAL_INTERACTION_ONLY=1 npm run test:renderer
```

The lane is also part of the complete renderer gate. It asserts work counts,
routing, scroll behavior, credit draining and mount identity; measured timing
values are reported rather than used as fragile performance thresholds.

Baseline log: `/private/tmp/interface-interaction-before-20260911.log`.
After log: `/private/tmp/interface-interaction-after-20260911.log`.

## Validation and authority

Scoped validation passed: 46 terminal-workspace tests, Electron typechecks, and
the new real-renderer interaction lane. `cargo xtask check --changed` passed on
2026-09-11: 138 test files / 2,664 Electron tests, release-script checks,
typechecks, all nine renderer lanes and the build. This is an Electron-only
change; Rust was not rerun because no Rust source or shared contract changed.
The current source receipt covers Electron only; the preceding full gate
tested the unchanged Rust code at `60c57b6`. Closeout separately verifies the running
Dev renderer → preload → Rust host.

Gate log: `/private/tmp/interface-interaction-gate-20260911.log`.
Gate renderer artifacts:
`/var/folders/_b/hwd9v4ks1gb8dcf1jl66s8xc0000gn/T/interface-renderer-fixtures-MdwDM9`.

No appearance, dependencies, wire/persisted schemas, accessibility settings,
terminal byte routing, history retention or production installation changes are
included. Production promotion, packaging/updater staging and installation or
restart require the owner's separate release instruction.
