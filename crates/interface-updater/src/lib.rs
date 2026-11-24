//! Fail-closed updater policy shared by the desktop host and migration tooling.

mod channel;
mod feed;
mod local_feed;
mod macos;
mod remote;
mod service;
mod signature;
mod signed_manifest;
mod state;

pub use channel::{ChannelError, ReleaseChannel};
pub use remote::{
    CHANNEL_STATE_NAME, FetchError, MirrorError, MirrorEvent, MirrorOutcome, MirrorPlan,
    RemoteFetcher, UreqFetcher, prepare as prepare_mirror, refresh as refresh_mirror,
};
pub use signed_manifest::{
    MANIFEST_SCHEMA, MAX_RELEASE_ARCHIVE_BYTES, MAX_SIGNED_MANIFEST_BYTES, ManifestError,
    PublicKey, ReleaseArtifact, ReleaseManifest, SIGNED_MANIFEST_NAME, generate_signing_key,
    is_artifact_name, sign_manifest, signing_public_key, verify_signed_manifest,
};

pub use feed::{FeedError, NormalizedFeed, content_type, update_feed_file_name};
pub use local_feed::{LocalFeedError, LocalUpdateFeed};
pub use macos::{
    RetainedVerifiedArchive, VerificationError, read_mac_app_requirement,
    retain_verified_archive_file, verify_and_retain_mac_update_artifact,
    verify_mac_update_artifact,
};
pub use service::{
    CHANNEL_CHECK_INTERVAL, CHANNEL_FIRST_CHECK, ChannelCheck, DEFAULT_UPDATE_INTERVAL_MS,
    DEFAULT_UPDATE_STARTUP_DELAY_MS, NativeCheckOutcome, NativeUpdaterConfiguration,
    NativeUpdaterConfigurationInput, NativeUpdaterEvent, SystemUpdaterClock, SystemUpdaterVerifier,
    UpdaterClock, UpdaterService, UpdaterVerifier,
};
pub use signature::{
    CODESIGN_PATH, DITTO_PATH, MAX_REQUIREMENT_LENGTH, is_enforceable_requirement,
    parse_designated_requirement, requirement_arguments,
};
pub use state::{
    REFUSED_UPDATE_MESSAGE, UpdateMachine, VerificationTicket, is_newer_version, safe_update_error,
};
