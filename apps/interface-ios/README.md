# Interface for iPhone

Native SwiftUI workspaces, search, navigation and Face ID unlock, with Interface’s
shared React conversation rendered for phone reading inside each session. The Mac’s Rust
host owns sessions, approval, persistent trust, leases and input effects. No third-party Swift
packages are required. Minimum deployment target: iOS 17.

## QR pairing and encrypted transport

Choose **Scan Mac QR code** on iPhone. On the Mac, enable the secure phone
connection and choose **Pair iPhone**. Confirm whether the code identifies
production or Dev, then approve the matching verification number on that Mac.
Face ID or the device passcode unlocks the saved connection on later opens.

Nearby connections try the private IPv4 routes in the QR code and Bonjour
together, using the same pinned-key encryption as the relay; the relay is tried
only after every nearby route fails. New QR codes require phone
0.3.1 build 6 or newer. If the Mac changes address and discovery is unavailable, scan a
fresh code from the same Mac; saved approval and identity remain intact.
After each unlock the Mac sends its current nearby addresses over the encrypted
connection (`/api/v1/routes`, `MacRoutesV1`) and the phone saves them, so a
changed Mac address never needs a new QR code. Cellular and other Wi-Fi
networks require a configured, deployed relay; see
[deployment instructions](../../deploy/phone-relay/README.md). The Tailscale
route (the Mac's tailnet address, and the typed `.ts.net` address) was retired
on 2026-10-14: see `docs/archive/tailscale-route-20261014/README.md`. The app displays the
route actually in use and keeps the conversation interface bundled locally.

The Xcode build phase builds `electron/vite.phone.config.ts` and copies only its
fixed public assets into the signed app. Install the repository's existing npm
dependencies before building. No Swift packages are needed.

For a disposable live direct-address/HPKE test, first run from the repository root:

```sh
npm --prefix electron run build:remote
cargo run -p interface-remote --example phone_fixture -- /private/tmp/interface-phone-v5-fixture.json
```

Then run the Xcode tests in Simulator. The live test decodes the Rust QR matrix,
disables Bonjour to reproduce discovery-blocked Wi-Fi, validates enrollment, renders fixture history through the actual WebKit/native
bridge and checks locked-grant rejection. The example uses public test keys,
memory-only trust and synthetic provider data; it does not open real sessions.
Stop it with Ctrl-C afterward. Without this fixture, that one native test reports
an explicit skip. HPKE interoperability, native bridge and bundled WebKit tests
still run. Hardware camera, real Face ID, cellular switching and a deployed relay
remain separate physical-device checks.

Automatic unlock runs once on launch or after backgrounding a connected session.
Face ID's own inactive/active transitions never retry an unsuccessful unlock.
After cancellation or refusal, use the visible button to try again. If the Mac
rejects a remembered connection (for example after switching from Dev to production),
the button becomes **Request Mac approval** and starts a fresh enrollment after
Face ID. The phone identity stays in its protected Keychain; approval is still
required from the selected Mac.

**Turn off phone access** remembers the disabled state across Mac restarts and
closes all active grants. Trusted devices remain listed for explicit revocation.
A conflicting port or unsafe store keeps remote access off. The Mac must stay
awake with Interface running.

## Build and test

The iPhone icon and connection-screen logo adapt Interface's emerald **Fold** mark.
The editable master is `Artwork/icon.svg`; regenerate the checked-in asset catalog
and matching phone web icon with `node apps/interface-ios/scripts/make-icon.mjs`
from the repository root. This uses the same installed Chrome rasterizer as the
desktop icon. iOS receives an opaque 1024px square and applies its own corner mask.

Open `InterfacePhone.xcodeproj`, choose **InterfacePhone** and an installed iPhone
Simulator, and Run. Let Xcode sign the Simulator build normally (ad-hoc, no paid
team required). Disabling signing prevents realistic Keychain testing.

```sh
xcodebuild -project apps/interface-ios/InterfacePhone.xcodeproj \
  -scheme InterfacePhone -configuration Debug \
  -destination 'platform=iOS Simulator,name=Interface iPhone,OS=26.5' \
  -derivedDataPath /private/tmp/interface-ios-build test
```

Without `OS=`, xcodebuild looks for the named Simulator only in the newest
installed runtime and fails if it lives in an older one. `xcrun simctl list
devices available` shows each Simulator under its runtime.

To install on a physical iPhone signed by a free Personal Team, turn on
Developer Mode on the phone (Settings → Privacy & Security), keep it unlocked,
and pass the team and a bundle identifier that team owns on the command line;
a Personal Team cannot sign `com.interface.phone`. The owner's phone uses
`com.andreromero.interface.phone`. Personal Team builds expire after seven days
and must be reinstalled.

```sh
xcodebuild -project apps/interface-ios/InterfacePhone.xcodeproj \
  -scheme InterfacePhone -configuration Release -destination 'id=<device UDID>' \
  -derivedDataPath /private/tmp/interface-ios-device -allowProvisioningUpdates \
  DEVELOPMENT_TEAM=<team id> PRODUCT_BUNDLE_IDENTIFIER=com.andreromero.interface.phone build
xcrun devicectl device install app --device <device UDID> \
  /private/tmp/interface-ios-device/Build/Products/Release-iphoneos/InterfacePhone.app
```

Use Simulator → Features → Face ID → Enrolled, then Matching Face or Non-matching
Face for the system challenge. Simulator does not prove Secure Enclave or physical
Keychain enforcement. The app explicitly authenticates before every protected
Keychain read, and physical devices additionally enforce the item’s `userPresence`
ACL and `WhenPasscodeSetThisDeviceOnly` accessibility. No simulator credential
bypass is compiled into the app.

QR connections pin the Mac key and use encrypted nearby transport or the
invitation's validated HTTPS relay. Redirects are
refused; web navigation stays on the chosen origin. Native credentials never go
through JavaScript, URL parameters, preferences, logs, or iCloud synchronization.
A fifteen-minute session cookie exists only in memory and an ephemeral web view.
Backgrounding removes session content, cancels reads and attempts immediate grant
revocation; offline grants still expire. Unlock happens again before new content.

A fixed native reply bridge stores only opaque pending-input markers, scoped to
the selected Mac, terminal and generation. Input waits for that write before
sending. View replacement or app restart cannot silently clear uncertainty.
The bridge provides no general native, file or IPC access.

The native list groups sessions by project exactly as the Mac sidebar does: roster
v3 (`/api/v3/workspaces`) carries each terminal's sidebar project, computed in Rust
by `terminal_projects` and pinned to the sidebar by
`electron/src/fixtures/remote-projects-v3.json`. Chats outside any repository go
under "Other". Against a Mac without v3 the phone groups by each session's folder. Claude and Codex use shared
conversation history when the host has confirmed their provider identity. The phone
has no terminal view. Other profiles show a conversation-unavailable explanation.
Sending a message acquires a short control lease on that deliberate tap; Mac input
can still take control back. A transport-only emulator checks the live prompt and
supplies bounded screen observations to Rust; it is hidden and inert on iPhone.

Roster v4 (`/api/v4/workspaces`) carries each session's needs-you from the Mac's
Rust host, and Threads v1 (`/api/v1/threads…`, `PhoneThreads.swift`) lists the
Mac's Threads (needs you first), reads a thread's recent turns, answers its
pending approval and sends a follow-up to a thread at rest. Both effects take
the thread's short control lease on the deliberate tap and send one command id;
an uncertain outcome leaves an opaque marker that blocks repeating it (see
`docs/IPHONE_REMOTE.md`). The phone falls back to v3 or v2 against an older Mac
and hides Threads there.

Recognized standard Claude and Codex permission dialogs appear as request cards
with **Allow once** and **Decline**. HTTP v4 pins the displayed request to the terminal,
generation and output cursor. Rust chooses the input, rejects changed/consumed
screens, and retains command receipts. Claude permits Allow once only when its
single-use Yes choice is selected; Codex requires its explicit `(y)` shortcut.
Custom keymaps, clipped requests, other permission types and CLI menus stay on the
Mac. Session creation and all processes remain on the Mac.

Required gates: `cargo xtask check`, `npm --prefix electron run smoke:remote`, native
Xcode tests and rendered Simulator acceptance. Physical signing/install, real
Face ID hardware, cellular switching, sleep/wake and software-keyboard interaction
are separate acceptance gates. TestFlight/App Store distribution needs the owner’s
Apple development team and distribution setup.

## Phone presentation

Version 0.2.0 unifies the native library and shared conversation in a warm charcoal
and emerald palette. Projects sort by name ("Other" last) and filter from chips; inside one,
live sessions come first, then the most recently changed. A session that needs you
(roster v4, decided on the Mac) shows an amber "Needs you" label and tile dot, comes
first in its project and lifts its project to the top; the header says how many need
you. A Code | Threads switch shows the Mac's Threads (needs you, working, then the
rest); a thread opens a native page of its recent turns, with an Allow once / Deny
card for a pending approval and a follow-up field once the thread is at rest. Search matches project,
workspace, title, folder and provider. The reply field grows with its draft, and
approval requests replace it until resolved without discarding the draft.

For native visual inspection, a Debug Simulator launch with `--design-preview`
opens a render-only library of invented sessions and threads (add
`--design-threads`, `--design-thread` or `--design-follow-up` to open Threads, a
thread waiting on approval, or one at rest). It never instantiates a Mac
client or grants access, and is excluded from physical and Release builds.
Inspect the actual shared conversation with `npm --prefix electron run smoke:remote`,
which uses a disposable Rust host, transcript and PTY.
