# Pane menu workspace tools

The pane `+` opens the New pane menu: Terminal (⌘D), Files, Browser, iOS
Simulator, Interface's existing host-discovered harnesses, Split Down (⌘⇧D),
New Tab and New Session. The pane ellipsis keeps Split right/down, Pass off,
delegated work and Clear scrollback. No additional harness is registered by
this change. Canvas left the menu on 2026-09-26 at the owner's request; the
agent canvas is still reachable from its own navigation.

Terminal and harness rows split a fresh process next to the source pane. New
Tab creates a shell tab in that exact pane; its asynchronous reply retains the
originating workspace even if selection changes. New Session opens a fresh
workspace launcher seeded with the pane's folder. These actions preserve live
PTYs.

Files opens the embedded [output preview workspace](OUTPUT_PREVIEW.md), with
an explicit Finder fallback.

Browser adds a Browser tab to the pane whose `+` was clicked (owner,
2026-11-15; until then it split that pane): the tab is shown with its address
bar focused, and nothing else in the layout moves. At the pane's tab limit it
says so, as New Tab does. The command palette's Browser does the same in the
focused pane, and with no pane at all opens the first one. A link to a running
server (the task inspector's server links, a terminal's link) still opens a
Browser pane beside that terminal, halving its pane the way the `+` places a
terminal, so the terminal stays in view.
A Browser tab is a tab like the others: it sits in the pane head with a globe
and the page's title, and a pane may hold terminal and Browser tabs together.
Pane actions follow the tab that is showing: New Tab opens another page over a
page and a shell over a terminal; Pass off, Clear scrollback, Send selection
and Fast mode step aside while a page shows; the pane's close ends a terminal
but only closes a page, and closing a pane's last tab closes the pane. The
pane head counts its terminals and pages apart ("Terminal pane, 1 terminal,
1 page"; a pane of pages only reads "Browser pane"). The pane splits, swaps,
maximizes, flips to a card and moves with Tidy whatever it holds. A new Browser tab opens
Google (`https://www.google.com/`) with its address bar focused. The address
bar reads `5173`, `5173/path` and `localhost:5173` as the local server over
http and a bare domain as https; anything else (words, a sentence, a number
that is not a port) is a Google search. It loads nothing but http(s): another
scheme is refused in words. A host that does not resolve offers "Search Google
for …" beside Retry. The host is asked which loopback servers the workspace's
running terminals listen on (`iface:inspectTerminalServers`, source folder
first) when the tab is drawn and again whenever its address bar is focused; the
first one not already shown sits in the bar as a `localhost:PORT` button (hidden
in a pane narrower than 360px). It has back, forward, reload/stop and "Open in
default browser" (the native Browser tool below). It is drawn like the agent
overview (owner, 2026-10-14): a 34px rounded search field with a leading
glyph (search while typing, globe on a page) and the load running along its
lower edge, pill chips for the server and the notice's choices, and the page
as a rounded card inset under the bar.

The Live panes column beside the panes (the agent overview, `AgentOverview`)
has a Browser tab too (Panes · Agent tasks · Browser, owner 2026-10-14; named
Agents · Tasks until review C2): the same Browser filling the column, one for
every workspace, offering the showing workspace's servers. It loads nothing
until Browser is first chosen, then stays loaded (parked) behind Panes. Its page
lives in a layer inside the column (`.agent-overview-browser`), which never
moves; closing the column closes the page, and reopening it in the same
session reloads the address it was on. The chosen tab is a `localStorage`
preference beside the column's open state; the address is not saved.
The task inspector's server links open here too, beside the terminal serving
them, and fall back to the default browser only without a terminal centre.

A Browser tab is a layout tab whose id starts `browser:`; it never names a
host terminal, so the host roster neither reopens nor prunes it. The tab keeps
its last web address (`url`, http(s) only, no credentials) in the renderer's
layout JSON, which the host stores as an opaque string, so a workspace reopens
on the same page after a restart. Older builds read that layout and drop the
tab. A workspace that is not showing keeps its pages alive out of sight.

Each page's `<webview>` lives in the terminal centre's page layer
(`.browser-page-layer`), not in its pane: Electron destroys a guest whose
element leaves the document, and panes move on every split, swap, tidy and
workspace switch. The pane body holds a slot carrying the page's CSS anchor
name; the page is anchored over it (`anchor()`, Chromium's CSS anchor
positioning). When the tab is in the background, the card is flipped, a
neighbour is maximized or the launcher covers the panes, the slot has no box
(`[inert] .browser-pane-slot { display: none }` for a flipped card or the
launcher) and the page is parked: hidden at its last size, so it keeps its
scroll and state, and Chromium throttles it as a hidden page (timers about
once a second, no frames, video paused). Hiding only the slot hid the page's
paint while it kept running at full rate. Pages are made when their tab is
first drawn and closed when the tab leaves every workspace's layout.

A Browser tab behaves like a Chrome tab (owner, 2026-11-15). A right-click
opens a native menu with only what applies, in Chrome's order: on a link, Open
Link in New Tab and Copy Link Address; on an image, Open Image in New Tab, Save
Image As… (a normal download) and Copy Image; in a text field, Chromium's
spelling suggestions, then Cut, Copy, Paste and Select All; on selected text,
Copy and Search Google for “…”; then Back, Forward and Reload; Inspect last.
`target=_blank` and a `window.open` without a size open a new Browser tab
right after the page in the same pane, in front; ⌘-click, middle-click and
Open Link in New Tab open it behind (it loads when first shown). A
`window.open` with a size (a "Sign in with Google/GitHub" popup) opens a small
child window of the app that keeps `window.opener` and closes when the page
closes it; its title always starts with the site it is on. Whatever the page
asks for, it is an ordinary window with its title bar, closable, movable,
opaque and never on top, at least 320 × 240 and never larger than or off the
screen it opens on, also when the page resizes or moves it later. As in Chrome, a
page opens a tab or a window only just after a click or key press in it (5 s,
one per press); a script with nothing pressed opens nothing, quietly, and a
tab holds at most two popup windows open at once. Downloads save straight
into Downloads with Chrome's naming
(`report (1).pdf` when the name is taken) and never overwrite. One the page
starts without a click or key press in it asks first ("“example.com” wants to
download files without a click", Block / Allow, once per site until the app
quits); an installer or script started that way is refused unless the site
was allowed. Every saved file carries macOS's quarantine flag, so Gatekeeper
checks it before it first opens; one that cannot be flagged fails and is
removed. The
pane says "Downloading report.pdf…", then "Saved report.pdf to Downloads" or
that it failed (a dropped connection fails it and cancels it, so no partial
file stays and nothing resumes later), and the Dock's Downloads stack bounces. ⌘F, with the page or
its address bar focused, opens the terminal search field over the page's top
right: it matches as you type and says "3 of 12"; Enter / ⇧Enter and ↓ / ↑
step; Esc closes it and gives the page the keyboard. ⌘+ / ⌘= / ⌘− / ⌘0 zoom the
page by Chrome's steps (Chromium keeps the zoom per site for the app run), ⌘R
reloads it (⌘⇧R without the cache), ⌘[ and ⌘] go back and forward, and ⌘L puts
the keyboard in the address bar. These are the only chords a Browser tab
claims; ⌘W, ⌘T, ⌘Q and the app's own chords are left alone, and the app's
own shortcuts (the command palette's ⌘⇧P, for one) do not reach the app while
a page has the keyboard. A page asking for the camera, microphone,
notifications or to read the clipboard gets a sheet on the app window
("“meet.google.com” wants to use your camera and microphone", Block / Allow,
Block the default so a stray Return grants nothing), once per site and
permission until the app quits; a site's Copy button just works. A page's fullscreen (a
video's) fills the Interface window, not a new Space; Esc or the page leaving
it puts the pane back as it was. A PDF opens in the page with Chromium's
viewer.

Pages run in an Electron `<webview>` in the `persist:browser-pane` session,
never the app's own, and nothing about a page reaches the app or the Rust
host except through main. Main pins every guest before it attaches (no
preload, no Node, sandboxed, context-isolated, Chromium's PDF viewer on; the
app window's own preference keeps a page's fullscreen inside it), refuses any other session or a non-web
`src`, and serves no `file:` URL. Every main-frame navigation and redirect of
a page goes only to http(s); one to another app's scheme (`mailto:`,
`zoommtg:`…) is stopped and asks the owner with the scheme named, then opens
in that app only if allowed; anything else (`file:`, `javascript:`, `data:`,
`about:`, `chrome:`) is refused. A request for a new tab or window to an
http(s) page becomes a Browser tab (main tells the renderer on the main-only
`iface:browserEvent`; the renderer finds the page by its guest's webContents
id), or for a sized `window.open` a popup window that Electron opens in the
same session, pinned exactly like a page (no preload, no Node, sandboxed,
context-isolated, PDF viewer on), a child of the app window, with the same
guards on its own navigations, popups, right-click menu and keys; a popup in
any other session is destroyed. Main builds the popup's window itself
(`createWindow`, from `browserPopupWindowOptions`), so none of the frame,
always-on-top, closable, movable, focus, transparency, opacity or other
window options Electron lets `window.open` features set reaches it, nor any
it adds later; only the asked-for size and place are read, clamped by
`browserPopupBounds`, which also clamps `resizeTo`/`moveTo`
(`content-bounds-updated`). Any other new window is refused. A tab or
window needs a fresh press in the page that asks (`input-event` mouse down,
touch end or tap, or a key other than Escape and the page chords, within
`BROWSER_ACTIVATION_MS`), spent by the one it opens; a tab's popups, its
popups' own included, are capped at `MAX_BROWSER_POPUPS`; and a popup's
title is `browserPopupTitle` (host, then the page's title). Permissions
are decided in main for this session only: camera, microphone, notifications
and clipboard read ask the owner with a native sheet, once per site (origin)
and permission, the answer kept in memory for the app run and never stored;
fullscreen, pointer lock and writing to the clipboard (a site's Copy button)
are allowed without asking, as Chrome allows them; opening another app asks;
everything else (geolocation, screen capture, MIDI, USB, HID, serial,
Bluetooth, idle detection, protected media, …) is refused, and device access
is refused. The permission check answers yes only for fullscreen, pointer
lock, clipboard write and what the owner granted that site; everything else,
including camera, microphone, notifications and clipboard read before the
owner has answered, is no, because Electron's check can only say yes or no,
never "not asked yet". So a site that has not asked reads "denied" from
`navigator.permissions.query` and `Notification.permission`, and sees no
device names or ids from `enumerateDevices()`. Known limit: a site that checks
before it asks may show itself as blocked and never ask; the owner can still
use a site that asks directly. A yes until asked was tried for camera and
microphone (15 Nov 2026, fake devices, real guards): every capture in the
page, a same-origin iframe and a cross-origin iframe still waited for the
sheet and Block still captured nothing, but `enumerateDevices()` then handed
every site the names and ids of the owner's cameras and microphones before it
had asked, so the check stays no. For notifications a yes would let a site
show them without asking. The guest has
no IPC route: host requests are accepted only from the app window's main
frame, and `iface:browserEvent` runs only from main to the app window.
A download is the owner's, and asks nothing, when Chromium reports a press
started it (`hasUserGesture`), the page was pressed within
`BROWSER_ACTIVATION_MS` (one download per press, apart from the window
allowance), the page has no document yet (a tab opened on a download's
address), main itself downloads that address (`downloadURL`, the right-click
menu's Save Image As…) or the app is loading it (a main-frame navigation with
no initiating frame, such as the address bar). Any other is held paused while
the owner is asked through the same per-site grants as the permissions
(`download`), and an installer or script (the Save As extensions) is refused
outright unless the site was allowed. A finished download is saved only once
`quarantineBrowserDownload` has written `com.apple.quarantine`
(`0081;<hex time>;Interface;<event id>`, the flags macOS itself writes for a
web download), read it back, and written "Where from"
(`com.apple.metadata:kMDItemWhereFroms`, the file's address and the page's,
without user names or passwords); if the flag cannot be written the file is
removed and the download fails. Only these downloads are flagged, not the
app's or the Rust host's own files (no `LSFileQuarantineEnabled`), and no
LaunchServices quarantine-event record is written, so Gatekeeper's prompt does
not name the site. In a
hands-free run or the packaged smoke nothing is asked (every prompt is
Block), no popup is shown, no other app opens, and downloads go to a temp
folder, never the owner's Downloads.

iOS Simulator starts an iPhone and shows it in Apple's Simulator. The host
lists available devices with `xcrun simctl list devices available -j` and
picks a booted iPhone, else the one used last, else the newest runtime's first
listed model; it boots that device when needed and opens Simulator with
`-CurrentDeviceUDID`. Missing Xcode (checked with `xcode-select -p`, which
raises no install prompt) or no iPhone simulator is refused before any effect.

Host protocol v15 adds `iface:openWorkspaceTool`, with 108 requests and 10
pushes (118 desktop channels). The request has `version: 1`, `root`, `tool`
(`files`, `browser`, `ios-simulator`) and a Browser-only `url`. It returns null.
The shared fixture `electron/src/fixtures/workspace-tools-v15.json` is consumed
by Rust contract and Electron payload tests. Existing wire and stored fields
are unchanged; the in-app Browser and the Simulator's device choice added none.

The Rust terminal dispatcher collects existing workspace/session authority and
the private roots of live PTYs, including those reopened from saved workspaces.
`interface-terminal::workspace_tools` validates the request and rechecks the
folder against a canonical approved root or descendant. Fixed argv runs each
tool (`/usr/bin/open`, `/usr/bin/xcode-select`, `/usr/bin/xcrun simctl`),
without a shell, each bounded: an app launch 15 seconds, the device listing 30
and a boot 90. An error after spawning a launch or boot is an unknown outcome
and is never retried. The result confirms OS launch acceptance, not website
readiness or simulator boot completion.
