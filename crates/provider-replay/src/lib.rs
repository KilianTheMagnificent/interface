//! Test-only provider replay (never packaged; `xtask/src/package.rs` builds
//! only `interface-host` and `iface`).
//!
//! A real provider session is recorded once through `provider-record` and
//! kept as a transcript under `fixtures/<engine>/<scenario>.json`. The
//! `provider-replay` binary then stands at the provider's executable path
//! and serves that transcript to the real adapter, frame by frame, by what
//! arrives rather than by time; an unexpected frame fails loudly, naming the
//! expected and the received frame. Design and contract:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 4.

pub mod matcher;
pub mod record;
pub mod replay;
pub mod scrub;
pub mod transcript;

pub use record::{Meta, RECORD_OUT_ENV, RECORD_TARGET_ENV, finish, journal_directory};
pub use replay::{Replay, Start, start};
pub use scrub::{Identity, Scrubber};
pub use transcript::{Entry, Probe, Transcript};

/// The transcript the replayer serves. Read from the environment, never
/// from argv: the adapters pass their argv through unchanged, and ACP's
/// profile validation refuses credential-shaped arguments.
pub const TRANSCRIPT_ENV: &str = "INTERFACE_REPLAY_TRANSCRIPT";

/// The directory holding the committed transcripts.
#[must_use]
pub fn fixtures_directory() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[cfg(test)]
mod matcher_tests;
