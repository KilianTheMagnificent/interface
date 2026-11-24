# Proposed bounded collaboration audit storage

Status: superseded by the owner's September 14 approval of **lossless numbered
segments**, preserving all detailed events with growing disk usage. The two-slot
retention policy below was not selected. See [the implemented v2 format](AUDIT_STORAGE_V2.md).

## Problem and scope

`CollaborationAuditSink::record` rewrites its entire `audit.jsonl` on every
event. The file has a 64 MiB limit. Once it reaches the limit, audit writes
fail and the sink becomes poisoned, while canonical collaboration transitions
have already committed. Removing fsync or replacing atomic writes with an
unguarded append would weaken durability.

The proposal changes only the privacy-preserving audit projection. Canonical
collaboration records, provider effects, receipts and replay rules stay under
their existing owners. A failed audit write continues to mark audit degradation;
it never rolls back or repeats a provider action.

## Exact proposed format and retention

- Keep the original `audit.jsonl` intact as the imported legacy segment.
- Introduce `audit-state-v2.json` with `version: 2`, `activeSlot: 0 | 1`,
  cumulative `AuditCounters`, and the outstanding peer-review timestamps needed
  to calculate queue latency after reopening.
- Use `audit-0.jsonl` and `audit-1.jsonl` as alternating active slots. Each event
  keeps the existing v1 JSONL event schema. Rotate before an append would take
  the active segment above 256 KiB; reject an individually oversized event.
- Limit the checkpoint to 8 MiB and keep the existing owner-private writer
  lease, bounded strict decoding, atomic replacement, readback and fsync.
- Lifetime counters survive rotation. Detailed events after migration survive
  only in the active segment and, until the next reuse, the preceding slot.
  Older detailed events are deliberately overwritten as a retention policy.
  The imported legacy file is never overwritten by slot rotation.

The retention tradeoff needs explicit approval: bounded storage sacrifices the
complete detailed audit-event history while retaining aggregate counters.
An alternative is indefinitely retained sealed segments, which avoids deleting
detail but does not bound disk growth. That alternative needs a different
capacity and operational policy.

## Publication and recovery

1. Validate the active file and checkpoint against their exact snapshots.
2. Atomically install an empty inactive slot. The current checkpoint does not
   reference this slot, so interrupted preparation cannot change the current
   audit projection.
3. Revalidate the old active snapshot. Atomically publish a v2 checkpoint that
   contains its cumulative projection and selects the new empty slot.
4. Append the next event by atomically replacing only that active segment.
   Update the in-memory counters only after a successful write.

Before checkpoint publication, recovery uses the old active segment. After
publication, recovery uses the checkpoint plus the selected segment. An absent
selected segment is corruption, not an empty log. An incomplete or failed
publication poisons the writer until it reopens and reconciles. Unreferenced
prepared slots are never replayed. Unknown format versions and external drift
fail closed without repairing or overwriting forensic bytes.

Old application versions do not understand the v2 checkpoint. Downgrading would
show only the retained legacy audit, so rollback needs an explicit export or a
reader that understands v2; silently treating the old reader as complete is
unacceptable.

## Required validation before landing

- Golden v2 checkpoint fixture; reject unknown fields, versions and slot names.
- Compare counters with the existing reference aggregation across repeated
  rollovers and reopenings, including a peer-review wait crossing a boundary.
- Interrupt before and after each atomic publication; recover exactly one
  committed projection without duplicate counters or replayed effects.
- Detect external changes to the active segment/checkpoint and preserve bytes.
- Import empty, small and near-capacity legacy logs without changing them.
- Exercise repeated slot reuse, oversized events, checkpoint capacity, missing
  active slots, malformed JSONL, symlinks and writer-lease loss.
- Measure cumulative bytes written and full recovery time against the current
  implementation; run `cargo xtask check` before any Dev landing.

Approval requested: authorize this v2 format, the bounded detailed-event
retention policy, and its legacy-import/rollback semantics, followed by the
normal tested Dev closeout. Production promotion remains a separate action.
