# Terminal data protocol v2

The 44-byte ITRM envelope now requires version 2. Data (kind 1) and EOF
(kind 2), payload limits, UUIDs and generation-scoped data/input sequences
are unchanged. Kind 3 is an empty Electron-to-host output acknowledgement:
its sequence echoes the highest contiguous consumed output sequence, not an
input sequence. The secure document-token handshake remains version 1.
Old binary peers are rejected; there is no silent protocol downgrade.

Each generation may have at most 256 KiB (including headers) and 64 frames
outstanding from Rust admission through completed xterm writes. A blocked
generation parks only its own existing output thread. Pipe writes do not
return credit. Acknowledgements bypass PTY input lanes and use a bounded,
coalescing priority queue in main, so input saturation cannot discard them.
Both sides bound the number of outstanding generation windows to 128.

Main tracks only frames delivered to its current authenticated document.
It converts individual, possibly out-of-order completions into contiguous
watermarks. Duplicate, stale, wrong-generation and unsent completions grant
no credit. Detaching a document releases its discarded writes exactly once;
an old callback cannot acknowledge on a new port. With no document, main
discards and acknowledges output immediately. Preload's bounded startup tail
and the renderer's bounded replay tail acknowledge storage/discard immediately;
live xterm writes acknowledge on completion or disposal.

The shared terminal-data-v2.json fixture pins Rust/Electron bytes and limits.
The JSON control plane, channel counts, terminal persisted schemas, token
authorization, provider effects and production install are unchanged.
