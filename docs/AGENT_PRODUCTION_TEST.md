# Sandboxed Agent production test — September 24, 2026

## Status and scope

**Passed.** A signed local release candidate passed package integrity, Electron
security fuses, the hidden packaged renderer/host smoke, all 16 Agent host
acceptance scenarios, and the focused Agent UI workflows below. No live provider
or owner data was used.

Tested source: `71db4cb9ddeaa63b3ef0ca9fb1d22cee74788312`, application version
`0.3.132`, protocol 29. This deliberately pins the tested candidate; concurrent
Dev/production releases are separate. Branch: `test/agent-production-sandbox-20260924`.
The owner requested testing, not production promotion, feed publication,
installation or a production restart. None of those actions was performed.

| Artifact | SHA-256 |
| --- | --- |
| `Interface-0.3.132-arm64-mac.zip` | `bca200df80478ee7e356501302ef5c00400b6b14104620a9e4194c58b2c724d5` |
| `Interface-0.3.132-arm64.dmg` | `3795a86ceb2b7bac44da4c5953ecd5e035483d4e50e1575ba105a009ae5765e6` |
| ZIP's signed `Resources/bin/interface-host` | `ef3ddf2be273b8113723cb682a1cc5d799e84ae11025aa1ce242ac0051eba57c` |

Artifacts remain in the isolated checkout's `release/`. The package uses the
configured local Apple Development signature; notarization was explicitly off.

## Isolation

The signed application is launched from a disposable extraction, hidden and
muted, with a private profile, HOME, Rust stores, temporary directory, clipboard
and fake-provider PATH. External applications, OS permission prompts and
external renderer networking are denied. Chromium retains its production
sandbox. The Rust host and its provider children run under a macOS Seatbelt
policy that denies writes outside the disposable directory, reads below the
owner's home, and external network connections. The sole process exception is
Apple's immutable `/bin/ps`, which macOS refuses to execute inside Seatbelt
because it is setuid. Its read-only process-identity query runs outside Seatbelt;
the host and providers remain confined. Private filesystem and network
probes check that the policy actually denies operations. An additional probe refused reading the
known nonsecret project `Cargo.toml` below the owner home. A startup probe also verifies real process-identity lookup. A fake provider also
attempts an outside write through the Electron → host → provider process chain.

A later signed-app run reached the renderer but could not open terminals because
Seatbelt refused `/bin/ps`. The narrowly scoped system-utility exception above
addresses this test-harness limitation without changing production PTY logic.

The original whole-app Seatbelt approach was rejected by Chromium's nested
sandbox initialization. It was replaced with host-tree confinement; the
Chromium sandbox was not disabled. Electron main-process isolation is provided
by the private environment and explicit test guards, not an OS sandbox claim.

## Agent acceptance matrix

`python3 docs/testing/agent-production-smoke.py <interface-host>` runs these
scenarios with synthetic Claude/Codex protocol fixtures and disposable state.

| Scenario | Required result | Exact signed ZIP host |
| --- | --- | --- |
| Claude task and scoped tools | Complete; real host verifies JSON artifact | Pass |
| Codex task and scoped tools | Complete; real host verifies JSON artifact | Pass |
| Malformed artifact | Provider completion cannot imply verification | Pass |
| Missing artifact | Verification fails | Pass |
| Owner Stop | Active task becomes stopped | Pass |
| Memory | History, restoration and owner-confirmed fact persist | Pass |
| Calendar DST | Skip spring gap; one fall-fold occurrence | Pass |
| Routine enablement | Verified test required; edits invalidate it | Pass |
| Writable folder leases | Overlapping exclusive scope refused | Pass |
| Consultation | Selected context, tool-free child, review and CAS apply | Pass |
| Approval denial | Outside-scope write asks; denial prevents write | Pass |
| Resume | Scoped host tools work after explicit resume | Pass |
| Required input freshness | Stale input refused before dispatch | Pass |
| Grant revocation | Active provider closes; resume uses reduced roots | Pass |
| Deadline | Automatic stop retains its budget explanation | Pass |
| Host crash | Unknown outcome; no resume until acknowledgment | Pass |

The fixtures exercise all eight Agent host tools, verify their tool catalogue,
and check idempotent repeated requests. The deadline stopped at 10.34 seconds
for a 10-second budget in the exact signed-host run. Evidence:
`/private/tmp/ip-949cc8d2/results.json` and
`/private/tmp/agent-production-exact-zip-host.log`. The earlier optimized-host
preflight also passed all 16 scenarios (10.51-second stop).

## Signed-app UI acceptance

`node docs/testing/agent-production-ui.mjs <extracted-Interface.app>` passed:

- Create an Agent and save its task contract through rendered controls; read
  the persisted contract back through the real preload/host boundary. Scripts
  with unknown effects and consultation both default off.
- Save the first routine paused and verify its Agent home is prepared.
- Preview calendar occurrences without applying them, save America/Chicago
  09:15, and refuse enablement until the current contract passes a test.
- Propose and confirm a memory fact through the UI and verify Rust persistence.
- Render the Task results and Run history controls.

The screenshot was inspected. The expected untested-calendar refusal remained
visible; this was the deliberate negative test. Evidence:
`/private/tmp/agent-ui-tBRhn2/{results.json,agent.png,app.log}` and
`/private/tmp/agent-production-exact-zip-ui.log`.

The standard packaged smoke also passed six real PTYs with fake providers,
large-output preservation across workspace switches, live input after return,
layout/style pins, provider roster and preload/host reads. Its log is
`/private/tmp/agent-production-package3.log`.

## Fixes found by testing

- Private Agent tool sockets and consultation scratch paths now stay inside
  the sandbox's selected roots.
- Shutdown events no longer transiently replace a persisted owner/deadline Stop
  explanation with a generic session-close message. A regression covers both
  idle and close events and reopening the persisted state.
- Packaged smoke now requires a marker for the current isolation implementation
  and refuses incomplete/mismatched private roots before profile access.

## Verification record

The first candidate passed the full source gate and signing, ZIP signature and
fuse checks, then failed the nested-sandbox renderer launch described above.
Later full-gate attempts hit process/timer failures in unrelated terminal and
theme tests under system load. The failed Rust test and all 61 tests in the two
failed JavaScript files passed alone without changing assertions or timeouts.
The final gate with one test worker per suite passed in 318.8 seconds:
`/private/tmp/agent-production-gate11.log`. It includes 1,958 Rust tests
(24 ignored), 2,806 JavaScript tests, three Swift tests, renderer fixtures,
formatting, linting, type checks and builds. After the process-identity harness
fix, the exact-source Rust receipt passed again in gate12; its renderer fixture
timed out. Gate13 passed the JavaScript suite alone on unchanged source in 224.5 seconds.
Gate12 and gate13 receipts cover both suites for the revised source.

## Limits

Providers are controlled protocol fixtures. This is evidence for real release
host behavior and deterministic integration, not live account authentication,
model quality, provider CLI drift, network reliability, paid consultation
benefit, microphone/voice, native permission dialogs, a physical paired phone,
updater publication, installation or production restart. Those gates were not
run. No paid provider calls or owner data are used by these fixtures.

## Dev closeout

Landed **Sandboxed Agent production verification**, `4650f92..745d362`.
Dev renderer → preload → Rust-host readiness passed on `745d362`; fetched
status confirmed it running with both source receipts valid. Closeout integrated
concurrent Dev changes, including protocol 30 and an already-approved PDF
preview dependency. The integrated gate passed 1,965 Rust tests (24 ignored),
2,812 JavaScript tests, three Swift tests, renderer fixtures and builds. Its
initial JavaScript type check correctly detected the not-yet-installed merged
lockfile; `npm ci` and a JavaScript-only rerun resolved that setup failure.

Integrated gate logs: `/private/tmp/agent-production-integrated-gate{,2}.log`.
Landing/readiness: `/private/tmp/agent-production-closeout2.log` and
`/private/tmp/agent-production-status.log`. The signed artifact results above
remain pinned to `71db4cb`; they are not represented as a signed build of the
later integrated Dev source. This task did not promote main, stage an updater
feed, install or restart production. The existing 0.3.134 feed was left alone.
