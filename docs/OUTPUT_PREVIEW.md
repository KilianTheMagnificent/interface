# Output preview workspace

Open **Files** from a terminal pane's `+` menu, or choose **Output preview** in
the Code toolbar. The panel opens beside the existing terminal on wide windows
and overlays it on narrow windows. Its project comes from that workspace/pane;
opening, closing or switching previews does not recreate the terminal or PTY.
The agent overview and output preview occupy the same side of the workspace,
so opening either closes the other.

In Threads (review B5), a project thread's changes card and Changes panel
offer **Preview** on a PDF, image or Markdown file it changed, and Quick Open
(⌘P over the thread) opens the highlighted one with **⌘↵**. The preview opens
beside the thread in the column the Changes panel uses; its root is the
thread's folder, an approved session root, and paths outside it are refused
before any read.

## First release

- Browse project folders or enter a project-relative output path. The browser
  includes supported images and documents, with credential/generated-path
  exclusions. Up to eight files across projects can remain open in this app
  session; unsaved changes are guarded when closing a file.
- Render Markdown, UTF-8 text, patch/diff files, PNG/JPEG/GIF/WebP and PDFs.
  HTML, SVG and source files display as inert source text; an `.html`,
  `.htm` or `.svg` file also offers **Render** (T3 section 8), which draws it
  in the sandboxed visual-reply frame (scripts only, opaque origin, no
  network, storage or navigation; Rust still reads it as text). Office containers,
  malformed binary formats and files over the limit show an explicit fallback:
  open the project in Finder and copy the exact file path.
- Select rendered text (or raw source), drag a region on an image, or choose
  **Comment on page** in a PDF. Write a revision request and **Copy feedback**
  to paste into the chosen agent when ready. The draft contains the absolute
  path, SHA-256 of the displayed bytes, and the quote, line range when provable,
  page number or normalized image coordinates. Browsing PDF pages does not
  silently move a draft to another page.
- Open tabs hold immutable snapshots. **Check for changes** reads a fresh
  snapshot and reports a different version without replacing the view or
  draft. **Load latest** explicitly captures a new version while retaining the old annotation in saved versions. Reopening the
  panel or returning to a workspace preserves the currently reviewed snapshot.

## Durable review and recovery (protocol 49)

Opening a supported output now saves an immutable version in the owner-private
`output-reviews.v2.json` store. Feedback, text/page/region annotations, the
explicitly selected thread and an optional related-task name save automatically;
the panel reports unsaved/save failures and offers **Save draft**. Open the same
path after a restart to recover its saved review. The store retains up to 32
versions; **Delete saved review** makes room explicitly. No files in the
project are changed by review storage.

`output-reviews.v2.json` holds only review metadata (8 MiB cap), so saving a
note, anchor or thread rewrites a few kilobytes. Each snapshot's text or bytes
is written once, at capture, to its own owner-private file in
`output-review-snapshots/`, named by the SHA-256 of those bytes and shared by
reviews of identical bytes. A snapshot is at most 5 MB (the preview's own read
limit; a larger one is refused with "larger than the 5 MB a saved review can
keep"), so 32 reviews take at most 160 MiB. A snapshot file is written before
the metadata naming it and removed after the metadata that dropped it; a file
no review names (left by a crash between the two) is removed on the next open.
A missing or changed snapshot file makes the store unavailable rather than
showing a different version. The first open without `output-reviews.v2.json`
carries `output-reviews.v1.json` (snapshot bytes inline as JSON number arrays)
over once and leaves it untouched; a v1 file that cannot be read is left for
the build that wrote it and the store starts empty. An older build that only
knows v1 still reads the v1 file as it was at the upgrade.

**Check for changes** still leaves the reviewed bytes untouched. **Compare
versions** renders the reviewed snapshot beside the current file, including
text, images and PDFs. **Load latest** captures a separate version and retains
earlier notes under **Saved version**. Unsupported files keep the original
honest fallback without pretending they have a saved preview.

**Send feedback** requires choosing a thread. The saved review reserves its
attempt durably before the existing outbound prompt lane sends it. Confirmed
sends are marked sent, definite non-delivery may be retried, and uncertainty or
a restart during sending remains non-replayable. The target thread is the place
to inspect uncertain outcomes. Copy feedback remains available.

The existing read/list requests remain version 1; review actions use request
version 2 on the same `previewOutput` channel. `output-reviews-v49.json` pins
the cross-language shapes. Rust owns snapshots, revision checks, persistence
and feedback-attempt state; React renders those records.

Prompt recovery uses the separately leased `prompt-recovery.v1.json`. The
outbound lane includes original text and validated context descriptors in the
send envelope; Rust stores a receipt before provider I/O and settles from typed
delivery certainty. Only definitely-unsent receipts offer **Retry preserved
prompt**. Failed, interrupted and ambiguous attempts offer **Continue safely**,
a new instruction to inspect completed work and avoid repeating ambiguous
external actions. Attachment paths are reauthorized and validated at each send.
Pasted images referenced by retained receipts survive normal image pruning;
retention itself grants no read authority. One latest receipt per session is
kept (256 sessions, 16 MiB cap). At either cap a new receipt replaces the
oldest settled one (accepted or definitely unsent), which only removes that old
session's Retry or Continue offer. A receipt whose outcome is unknown, a pending
launch and the thread a saved launch points at are never evicted; the send is
refused, as before, only when every other receipt is one of those.
Older legacy turns without receipts refuse text-only retry and require a new
owner instruction. No receipt auto-sends.
In a conversation pane a receipt with something to recover is one line inside
the composer's top edge (`KeptPromptLine`, 2026-11-15): "A message you wrote
earlier was not sent" (or "may not have finished", for an unknown outcome or a
failed or interrupted turn), its first words and attachment count, then
**Retry** or **Continue safely**, **Put it back** (the text into the draft;
file and folder attachments go through the drop path and are authorized again)
and **Discard** (the forget action, confirmed first). A message delivered and
answered draws no line; its receipt is still stored as before. Saved thread
launches keep the full panel.

Initial thread and one-off agent-task launches carry the same original context
beside provider launch options. Rust reserves a pending receipt before creating
the provider session, then atomically binds it to the actual thread. An identical
launch with an unresolved outcome is refused, including after restart; the
owner must inspect existing threads rather than recreate uncertain work.
The launch composer lists saved unbound launches after restart. Their original
prompt and attachments remain inspectable; only definitely-unsent launches can
restore a draft, while an explicit confirmed Forget releases an obsolete record.
Completed threads expose the same Forget action in collapsed saved-prompt details.

Queued and current composer file references also pin pasted images in Rust
(2,048 paths and 1 MiB total path text). The renderer awaits that durable pin
projection before the paste operation that can prune images. Retention failure
refuses the paste, and unavailable recovery storage disables pruning. Clearing
drafts or queue rows releases those hints; delivery receipts retain their own
references independently. Pins grant no file-reading authority.

Output discovery from transcripts, HTML execution and dev-server embedding
remain outside this workflow.

## Authority and rendering boundary

Host protocol **v30** adds the single read-only `iface:previewOutput` request
(`version: 1`, `action: list | read`, `root`, relative `path`), bringing the
handshake to **117 requests / 10 pushes / 127 channels**. The shared fixture is
`electron/src/fixtures/output-preview-v1.json`. Existing channels and persisted
schemas remain intact.

`TerminalHostDispatcher` collects current host-approved workspace/session and
live-PTY roots on the bounded blocking worker. `interface-workspace` validates
the request, rejects traversal, protected absolute/relative paths and symlinks,
then rechecks containment and inode/mtime/ctime identity around a descriptor-bound
read. A displayed version is SHA-256 of those exact bytes. The text editor keeps
its existing binary rejection policy; the preview read shares its race checks.
Directory enumeration stops at 5,001 entries and refuses an oversized listing.
Preview files are at most **5 MiB**; text display is at most **512 KiB / 10,000 lines**. Binary
responses use bounded byte arrays; unsupported formats are not presented as
successfully decoded or given a fabricated revision.

Markdown uses the existing React text-node parser. Raster formats require a
matching file signature and never admit SVG data URLs. PDF.js (owner-approved
dependency) is lazy-loaded, uses a bundled isolated worker, and renders a single
page to a bounded canvas. No scripting manager, form controls, annotation/link
layer or attachment execution is installed; PDF inputs are bytes, not URLs.
The renderer CSP allows only self-hosted workers, with no `unsafe-eval` or remote
worker permission. Document content never gains an Interface bridge.

## Verification

- Rust snapshot/authority tests cover formats, byte hashes, text-editor policy,
  unsupported/oversized files, no-grant requests, traversal, protected roots,
  directory filtering and a binary-read ancestor-swap race.
- Shared Rust/TypeScript fixtures and protocol ownership/count assertions pin
  the channel addition. Pure feedback tests cover version/path/quote binding,
  ambiguous text locations and normalized image regions.
- `INTERFACE_OUTPUT_PREVIEW_ONLY=1 npm --prefix electron run test:renderer`
  exercises real xterm and PDF.js in the isolated renderer at 1440, 640 and
  390 pixels, light/dark and Refined/Bare CLI. It covers selection, clipboard
  drafts, stale snapshots, explicit reload, inert HTML, image regions, PDF
  navigation, project isolation and preserved terminal input/DOM identity.
- The handsfree app is the end-to-end renderer → preload → host check. Use
  generated outputs in its private project and inspect rendered captures.
  Keep native Finder launches, live providers and production acceptance separate.
