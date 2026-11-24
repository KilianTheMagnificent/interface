# Lossless collaboration audit storage v2

The owner approved this migration on September 14, 2026: retain the original log
and all future detailed events in sealed segments; disk usage grows. This
replaces the unselected two-slot policy in [the proposal](AUDIT_STORAGE_V2_PROPOSAL.md).
Canonical collaboration records, provider actions and replay rules are unchanged.

## Format and capacity

- `audit.jsonl`: the original v1 log, read with the existing 64 MiB bound and
  preserved byte for byte. Opening a legacy store does not migrate it.
- `audit-segment-00000000000000000000.jsonl` and increasing, zero-padded 20-digit
  identifiers: unchanged v1 JSONL events. An active segment holds at most 256 KiB;
  rotation seals it permanently. No segment is deleted or reused by retention.
- `audit-state-v2.json`: strict JSON, bounded to 8 MiB, depth 32 and one million
  parser nodes. Its fields are `version` (2), `activeSegment` (u64),
  `legacySha256` (nullable 32-byte array), `precedingSha256` (nullable 32-byte
  array), and `projection`. The projection contains the existing `counters`
  shape and `heldAt` mapping collaboration IDs to peer-review start milliseconds.
  The checkpoint excludes active-segment events. `activeByState` is empty in
  storage; current record states supply this value at snapshot time.
- `audit.lock`: existing descriptor-bound, owner-private single-writer lease.

The first successful append preserves the legacy snapshot and publishes segment
zero. Later steady appends atomically replace only the active segment, checking
the checkpoint and active snapshots first. Rotation checkpoints lifetime counters
and outstanding timestamps before appending the next event. Snapshot reads use
the in-memory projection; they do not read or parse the detailed history.

An individual event over 256 KiB, an oversized checkpoint, a full disk, or an
exhausted segment identifier stops writes and poisons the sink. These failures
never trigger pruning. The checkpoint's outstanding timestamp reserve can grow
to its limit; it is not silently truncated. Sealed-file count and total disk
usage have no automatic bound. The original log still contributes to open cost.

## Publication and recovery

1. Hold the writer lease and verify the exact checkpoint snapshot.
2. Prepare an empty next segment with the existing atomic write, readback and
   fsync primitives. A previously prepared empty file can be reused; nonempty
   unreferenced data is refused and preserved.
3. Revalidate the previous active or legacy snapshot. Atomically publish the
   checkpoint with its complete projection and the newly selected empty segment.
4. Atomically replace that active segment with the new event included. Advance
   in-memory counters only on successful completion.

A stop before checkpoint publication leaves the previous selection authoritative;
an unreferenced empty preparation is inert. After publication, recovery combines
the checkpoint and its selected segment exactly once. A selected empty segment
is valid; an absent selected segment is corruption. After an uncertain write,
the sink must reopen and reconcile the selected files rather than retry blindly.
Audit failure does not roll back a committed collaboration transition.

Opening v2 verifies the legacy digest and immediately preceding sealed-segment
digest, then strictly decodes the selected active file. Unknown versions/fields,
duplicate JSON members, unsafe file metadata and detected drift are refused
without overwriting evidence. Historical segments older than the immediately
preceding segment are retained but not rescanned for integrity at each open;
the checkpoint is the cumulative projection authority. This is not a
tamper-proof archive or a backup against external deletion.

## Reading and downgrade

`CollaborationAuditSink::log_file()` returns the currently active detail file,
or the legacy path before migration. It is no longer the entire history. To read
all details, use the original legacy log followed by numbered segments through
the checkpoint's active identifier, in numeric order. Unreferenced files are
forensic evidence, never part of automatic replay.

Old binaries understand only `audit.jsonl`; they cannot report complete post-v2
details or lifetime counters. Downgrade requires a v2-aware reader or complete
export. A combined export can exceed the old reader's 64 MiB limit, so simply
concatenating into its writable store is not a general downgrade mechanism.
No production migration or automatic downgrade/export is performed by packaging.

## Validation

`cargo test -p interface-collaboration audit` exercises the frozen checkpoint
shape, duplicate/unknown fields and versions, repeated lossless rotation with
peer-review latency crossing a checkpoint, publication boundaries, forensic
orphan handling, checkpoint and active-file drift, near-64-MiB legacy import,
event/checkpoint capacity, missing selected files, symlink substitution, and
writer-lease loss. The existing audit suite checks v1 event validation, private
permissions and projection semantics. The canonical gate and signed packaged
smoke remain required before release. Synthetic tests do not establish physical
power-loss behavior or live-provider latency.
