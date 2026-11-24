//! Crash-consistent, fail-closed storage foundations for Interface.
//!
//! The primitives in this crate deliberately separate reading from recovery:
//! a read never creates a directory, repairs permissions, rewrites JSON, or
//! moves corrupt bytes. Mutating operations require an advisory one-writer
//! lease and use file fsync, atomic rename, and directory fsync.

mod atomic;
mod cas;
mod error;
mod json;
mod lock;
mod log_file;
mod quarantine;
mod secure;

pub use atomic::{
    atomic_temp_digest_prefix, atomic_write, is_atomic_temp_name, is_atomic_temp_name_for,
};
pub use cas::{Revisioned, compare_and_set_json, ensure_current_revision};
pub use error::{Result, StorageError};
pub use json::{
    JsonLimits, LoadedJson, PreparedJson, atomic_write_json, decode_json_strict,
    decode_json_value_strict, prepare_json, read_json,
};
pub use lock::WriterLock;
pub use log_file::{ROTATED_LOG_SUFFIX, RotatingLogFile, read_log_tail};
pub use quarantine::{
    QuarantineReason, QuarantinedArtifact, quarantine_file, quarantine_stale_atomic_writes_for,
};
pub use secure::{
    BoundedFile, FileIdentity, FileSnapshot, PrivateDirectory, TargetSnapshot, read_bounded,
    sha256_hex, snapshot_target,
};

/// Exact mode used for every authoritative directory created by this crate.
pub const OWNER_DIRECTORY_MODE: u32 = 0o700;

/// Exact mode used for every authoritative, lock, temporary, and quarantine file.
pub const OWNER_FILE_MODE: u32 = 0o600;

/// Hard ceiling accepted by [`JsonLimits`], even when a caller asks for more.
pub const HARD_MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
