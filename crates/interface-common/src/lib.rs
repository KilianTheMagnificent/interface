//! Leaf helpers shared verbatim by several crates.
//!
//! Nothing here may depend on another workspace crate, and only logic that is
//! byte-for-byte identical across its callers belongs here. Where copies have
//! drifted the divergent copy stays in its own crate, next to the behavior it
//! decides.

pub mod bounded_read;
#[cfg(unix)]
pub mod fsid;
#[cfg(unix)]
pub mod git_hardening;
pub mod paths;
pub mod process;
pub mod sync;
pub mod time;
#[cfg(unix)]
pub mod unix;
pub mod uuid;
pub mod version;
