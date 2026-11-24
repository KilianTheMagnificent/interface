use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

use interface_contracts::ipc::{
    CHANNELS, PROMPT_CONTEXT_VALIDATION_LIMITS, PromptContextGrant, PromptContextGrantKind,
    PromptContextGrantReason, PromptContextGrantRejection, PromptContextGrantResult,
    PromptImagePasteResult, PromptImageRejectReason, PromptImageRejection, PromptImageSaved,
    RepositoryReview,
};
use interface_contracts::prompt_images::{
    PROMPT_IMAGE_LIMITS, is_prompt_image_preview_url, prompt_image_extension,
    prompt_image_size_accepted, sniff_prompt_image_format,
};
use interface_review::read_repository_review;
use interface_workspace::is_search_excluded_path;
use serde_json::Value;

use crate::image::{
    PromptImageThumbnailer, RustImageThumbnailer, SystemPromptContextClock,
    SystemPromptImageNameMinter,
};
use crate::path::{
    ContextPathError, ContextPathKind, grant_candidate, inside, relative_slash, stable_context_path,
};

const PASTED_IMAGE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const PASTED_IMAGE_KEEP: usize = 64;
const EXACT_GRANT_LIMIT: usize = 512;

pub const PROMPT_CONTEXT_OPERATIONS: [&str; 4] = [
    CHANNELS.grant_prompt_context_paths,
    CHANNELS.save_pasted_images,
    CHANNELS.read_prompt_context_thumbnail,
    CHANNELS.validate_prompt_context,
];

/// Dynamic authority projected by the session runtime and desktop root picker.
pub trait PromptContextRuntimeView: Send + Sync + 'static {
    fn session_root(&self, session_key: &str) -> Option<PathBuf>;
    fn authorized_roots(&self) -> Vec<PathBuf>;
    /// Durable receipts keep their pasted images alive; these paths do not grant access.
    fn retained_prompt_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn prompt_retention_ready(&self) -> bool {
        true
    }
}

/// A thread reference (T3 section 9), as the threads owner resolved it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadReference {
    /// The referenced thread's head: the chip's canonical value.
    pub key: String,
    /// The block the chip becomes at send (`safeContent`).
    pub block: String,
}

/// Why a thread reference was refused: the reason the renderer reads and
/// the sentence it shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadReferenceRefusal {
    pub reason: interface_contracts::ipc::PromptContextInvalidReason,
    pub message: &'static str,
}

/// The threads owner's seam (T3 section 9): resolves `chat` items and keeps
/// stashed drafts' pasted images. Attached once the threads owner exists;
/// until then a `chat` item is refused.
pub trait PromptContextThreads: Send + Sync + 'static {
    /// The block for `target` as the session `sender` may read it. Runs on
    /// the blocking pool.
    ///
    /// # Errors
    /// Why `sender` may not refer to `target`.
    fn thread_reference(
        &self,
        sender: &str,
        target: &str,
    ) -> Result<ThreadReference, ThreadReferenceRefusal>;
    /// File paths stashed drafts refer to: they keep pasted images. They
    /// grant nothing.
    fn retained_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Read-only repository-review seam used when a `diff` descriptor is sent.
pub trait PromptContextReviewSource: Send + Sync + 'static {
    fn read(&self, root: &Path) -> RepositoryReview;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPromptContextReviewSource;

impl PromptContextReviewSource for SystemPromptContextReviewSource {
    fn read(&self, root: &Path) -> RepositoryReview {
        read_repository_review(root)
    }
}

pub trait PromptContextClock: Send + Sync + 'static {
    fn now(&self) -> SystemTime;
}

pub trait PromptImageNameMinter: Send + Sync + 'static {
    fn mint(&self) -> String;
}

pub struct PromptContextServiceConfig {
    pub runtime: Arc<dyn PromptContextRuntimeView>,
    pub review: Arc<dyn PromptContextReviewSource>,
    pub thumbnailer: Arc<dyn PromptImageThumbnailer>,
    pub pasted_image_directory: Option<PathBuf>,
    pub clock: Arc<dyn PromptContextClock>,
    pub image_name_minter: Arc<dyn PromptImageNameMinter>,
}

impl PromptContextServiceConfig {
    #[must_use]
    pub fn system(
        runtime: Arc<dyn PromptContextRuntimeView>,
        pasted_image_directory: Option<PathBuf>,
    ) -> Self {
        Self {
            runtime,
            review: Arc::new(SystemPromptContextReviewSource),
            thumbnailer: Arc::new(RustImageThumbnailer),
            pasted_image_directory,
            clock: Arc::new(SystemPromptContextClock),
            image_name_minter: Arc::new(SystemPromptImageNameMinter),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PromptContextServiceError {
    #[error("operation is not owned by the prompt-context service: {0}")]
    OperationNotOwned(String),
    #[error("prompt-context response could not be encoded")]
    ResponseEncoding,
}

/// One pasted-image item after a transport has converted its binary wire
/// representation. The renderer contract still exposes `Uint8Array`; this
/// type only prevents the JSON bridge from expanding every byte into a JSON
/// number before the Rust-owned image checks run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PastedImageInput {
    Bytes(Vec<u8>),
    Rejected(PromptImageRejectReason),
}

pub struct PromptContextService {
    pub(crate) runtime: Arc<dyn PromptContextRuntimeView>,
    pub(crate) review: Arc<dyn PromptContextReviewSource>,
    thumbnailer: Arc<dyn PromptImageThumbnailer>,
    pasted_image_directory: Option<PathBuf>,
    clock: Arc<dyn PromptContextClock>,
    image_name_minter: Arc<dyn PromptImageNameMinter>,
    exact_grants: RwLock<BTreeSet<PathBuf>>,
    pub(crate) threads: std::sync::OnceLock<Arc<dyn PromptContextThreads>>,
}

impl PromptContextService {
    #[must_use]
    pub fn new(config: PromptContextServiceConfig) -> Self {
        Self {
            runtime: config.runtime,
            review: config.review,
            thumbnailer: config.thumbnailer,
            pasted_image_directory: config.pasted_image_directory,
            clock: config.clock,
            image_name_minter: config.image_name_minter,
            exact_grants: RwLock::new(BTreeSet::new()),
            threads: std::sync::OnceLock::new(),
        }
    }

    /// Attaches the threads owner (T3 section 9). Attaching twice keeps the
    /// first.
    pub fn attach_threads(&self, threads: Arc<dyn PromptContextThreads>) {
        let _ = self.threads.set(threads);
    }

    /// Dispatches one frozen named-object payload.
    ///
    /// # Errors
    ///
    /// Only returns an error for an operation outside this service or an
    /// impossible response-serialization failure. Malformed renderer input is
    /// represented by the same structured 0.1.169 refusal as the TypeScript
    /// bridge.
    pub fn dispatch(
        &self,
        operation: &str,
        payload: &Value,
    ) -> Result<Value, PromptContextServiceError> {
        let result = if operation == CHANNELS.grant_prompt_context_paths {
            serde_json::to_value(self.grant_prompt_context_paths(field(payload, "paths")))
        } else if operation == CHANNELS.save_pasted_images {
            serde_json::to_value(self.save_pasted_images(field(payload, "images")))
        } else if operation == CHANNELS.read_prompt_context_thumbnail {
            serde_json::to_value(self.read_prompt_context_thumbnail(field(payload, "path")))
        } else if operation == CHANNELS.validate_prompt_context {
            serde_json::to_value(
                self.validate_prompt_context(field(payload, "target"), field(payload, "items")),
            )
        } else {
            return Err(PromptContextServiceError::OperationNotOwned(
                operation.to_owned(),
            ));
        };
        result.map_err(|_| PromptContextServiceError::ResponseEncoding)
    }

    #[must_use]
    pub fn grant_prompt_context_paths(&self, raw_paths: &Value) -> PromptContextGrantResult {
        let Some(raw_paths) = raw_paths.as_array() else {
            return PromptContextGrantResult {
                granted: Vec::new(),
                rejected: vec![grant_rejection(
                    "",
                    PromptContextGrantReason::InvalidRequest,
                    "Dropped paths were malformed and were not attached.",
                )],
            };
        };
        if raw_paths.len() > PROMPT_CONTEXT_VALIDATION_LIMITS.max_items {
            return PromptContextGrantResult {
                granted: Vec::new(),
                rejected: vec![grant_rejection(
                    "",
                    PromptContextGrantReason::ItemLimit,
                    grant_message(PromptContextGrantReason::ItemLimit),
                )],
            };
        }

        let mut granted = Vec::new();
        let mut rejected = Vec::new();
        let mut seen = BTreeSet::new();
        // Canonicalized once per drop, on the first path that needs them.
        let mut canonical_roots: Option<Vec<PathBuf>> = None;
        for raw in raw_paths {
            let Some(path) = raw.as_str() else {
                rejected.push(grant_rejection(
                    "",
                    PromptContextGrantReason::InvalidRequest,
                    grant_message(PromptContextGrantReason::InvalidRequest),
                ));
                continue;
            };
            let requested = path.trim();
            if requested.is_empty() {
                rejected.push(grant_rejection(
                    path,
                    PromptContextGrantReason::InvalidRequest,
                    grant_message(PromptContextGrantReason::InvalidRequest),
                ));
                continue;
            }
            if utf16_len(requested) > PROMPT_CONTEXT_VALIDATION_LIMITS.max_value_chars {
                rejected.push(grant_rejection(
                    &truncate_utf16(requested, PROMPT_CONTEXT_VALIDATION_LIMITS.max_value_chars),
                    PromptContextGrantReason::FieldLimit,
                    grant_message(PromptContextGrantReason::FieldLimit),
                ));
                continue;
            }
            let canonical_roots = canonical_roots.get_or_insert_with(|| {
                self.distinct_authorized_roots()
                    .into_iter()
                    .filter_map(|root| fs::canonicalize(root).ok())
                    .collect()
            });
            match grant_one(Path::new(requested), canonical_roots) {
                Ok((path, kind)) if seen.insert(path.clone()) => {
                    self.remember_exact_grant(path.clone());
                    granted.push(PromptContextGrant {
                        path: path.to_string_lossy().into_owned(),
                        kind: match kind {
                            ContextPathKind::File => PromptContextGrantKind::File,
                            ContextPathKind::Folder => PromptContextGrantKind::Folder,
                        },
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    let reason = grant_reason(error);
                    rejected.push(grant_rejection(requested, reason, grant_message(reason)));
                }
            }
        }
        PromptContextGrantResult { granted, rejected }
    }

    #[must_use]
    pub fn save_pasted_images(&self, raw_images: &Value) -> PromptImagePasteResult {
        let Some(raw_images) = raw_images.as_array() else {
            return self.save_pasted_image_inputs(None);
        };
        let images = raw_images
            .iter()
            .map(|raw| {
                json_bytes(raw).map_or(
                    PastedImageInput::Rejected(PromptImageRejectReason::InvalidRequest),
                    PastedImageInput::Bytes,
                )
            })
            .collect::<Vec<_>>();
        self.save_pasted_image_inputs(Some(&images))
    }

    /// Saves transport-decoded clipboard bytes using the same frozen
    /// per-item refusals as [`Self::save_pasted_images`].
    #[must_use]
    pub fn save_pasted_image_inputs(
        &self,
        raw_images: Option<&[PastedImageInput]>,
    ) -> PromptImagePasteResult {
        let Some(directory) = self
            .pasted_image_directory
            .as_ref()
            .filter(|directory| !directory.as_os_str().is_empty())
        else {
            return image_refusal(PromptImageRejectReason::WriteFailed);
        };
        let Some(raw_images) = raw_images else {
            return image_refusal(PromptImageRejectReason::InvalidRequest);
        };
        if raw_images.len() > PROMPT_CONTEXT_VALIDATION_LIMITS.max_items {
            return image_refusal(PromptImageRejectReason::ItemLimit);
        }
        if !raw_images.is_empty() {
            self.prune_pasted_images(directory);
        }

        let mut saved = Vec::new();
        let mut rejected = Vec::new();
        for raw in raw_images {
            let bytes = match raw {
                PastedImageInput::Bytes(bytes) => bytes,
                PastedImageInput::Rejected(reason) => {
                    rejected.push(image_rejection(*reason));
                    continue;
                }
            };
            if !prompt_image_size_accepted(bytes.len()) {
                rejected.push(image_rejection(PromptImageRejectReason::SizeLimit));
                continue;
            }
            let Some(format) = sniff_prompt_image_format(bytes) else {
                rejected.push(image_rejection(PromptImageRejectReason::NotAnImage));
                continue;
            };
            let Some(preview) = self
                .thumbnailer
                .thumbnail(bytes, PROMPT_IMAGE_LIMITS.thumbnail_max_edge)
            else {
                rejected.push(image_rejection(PromptImageRejectReason::NotAnImage));
                continue;
            };
            let file = directory.join(format!(
                "{}{}",
                self.image_name_minter.mint(),
                prompt_image_extension(format)
            ));
            match write_exclusive(directory, &file, bytes)
                .and_then(|()| fs::canonicalize(&file).map_err(|_| ()))
            {
                Ok(canonical) => {
                    self.remember_exact_grant(canonical.clone());
                    saved.push(PromptImageSaved {
                        path: canonical.to_string_lossy().into_owned(),
                        preview_url: is_prompt_image_preview_url(&preview).then_some(preview),
                    });
                }
                Err(()) => rejected.push(image_rejection(PromptImageRejectReason::WriteFailed)),
            }
        }
        PromptImagePasteResult { saved, rejected }
    }

    #[must_use]
    pub fn read_prompt_context_thumbnail(&self, raw_target: &Value) -> Option<String> {
        let target = raw_target.as_str()?;
        if target.trim().is_empty()
            || utf16_len(target) > PROMPT_CONTEXT_VALIDATION_LIMITS.max_value_chars
        {
            return None;
        }
        let file = stable_context_path(
            Path::new(target),
            ContextPathKind::File,
            &self.distinct_authorized_roots(),
            &|candidate| self.is_exact_grant(candidate),
            None,
        )
        .ok()?;
        let mut handle = open_read_nofollow(&file).ok()?;
        let metadata = handle.metadata().ok()?;
        if !metadata.is_file() || !prompt_image_size_accepted(usize::try_from(metadata.len()).ok()?)
        {
            return None;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
        handle.read_to_end(&mut bytes).ok()?;
        sniff_prompt_image_format(&bytes)?;
        let url = self
            .thumbnailer
            .thumbnail(&bytes, PROMPT_IMAGE_LIMITS.thumbnail_max_edge)?;
        is_prompt_image_preview_url(&url).then_some(url)
    }

    /// The runtime's authorized roots without the per-session duplicates.
    pub(crate) fn distinct_authorized_roots(&self) -> Vec<PathBuf> {
        self.runtime
            .authorized_roots()
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn is_exact_grant(&self, candidate: &Path) -> bool {
        self.exact_grants
            .read()
            .is_ok_and(|grants| grants.contains(candidate))
    }

    /// Currently granted exact files (dropped or pasted attachments), for read-only preview.
    #[must_use]
    pub fn exact_grants(&self) -> Vec<PathBuf> {
        self.exact_grants
            .read()
            .map_or_else(|_| Vec::new(), |grants| grants.iter().cloned().collect())
    }

    /// Configured directory for pasted screenshots, when available.
    #[must_use]
    pub fn pasted_image_directory(&self) -> Option<PathBuf> {
        let directory = self
            .pasted_image_directory
            .as_ref()
            .filter(|dir| !dir.as_os_str().is_empty())?;
        Some(fs::canonicalize(directory).unwrap_or_else(|_| directory.clone()))
    }

    fn remember_exact_grant(&self, target: PathBuf) {
        if let Ok(mut grants) = self.exact_grants.write() {
            grants.insert(target);
            // ponytail: 512-entry cap evicts the lowest path, not the oldest;
            // add an insertion counter if order ever matters
            while grants.len() > EXACT_GRANT_LIMIT {
                grants.pop_first();
            }
        }
    }

    fn prune_pasted_images(&self, directory: &Path) {
        if !self.runtime.prompt_retention_ready() {
            return;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let now = self.clock.now();
        // A grant names the canonicalized file; read_dir names it through the
        // directory as configured, which need not be the same spelling.
        let canonical_directory =
            fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        let retained = self
            .runtime
            .retained_prompt_paths()
            .into_iter()
            .chain(
                self.threads
                    .get()
                    .map(|threads| threads.retained_paths())
                    .unwrap_or_default(),
            )
            .collect::<BTreeSet<_>>();
        let mut kept = Vec::new();
        let mut removed = Vec::new();
        for entry in entries.flatten() {
            // `symlink_metadata` below is the authoritative check, and readdir's
            // own type is often unknown on macOS, which would stat here anyway.
            let target = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&target) else {
                continue;
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                continue;
            }
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if retained.contains(&canonical_directory.join(entry.file_name())) {
                continue;
            }
            if now
                .duration_since(modified)
                .is_ok_and(|age| age > PASTED_IMAGE_TTL)
            {
                if fs::remove_file(&target).is_ok() {
                    removed.push(entry.file_name());
                }
            } else {
                kept.push((target, modified));
            }
        }
        kept.sort_by(|left, right| right.1.cmp(&left.1));
        for (target, _) in kept.into_iter().skip(PASTED_IMAGE_KEEP) {
            if fs::remove_file(&target).is_ok()
                && let Some(name) = target.file_name()
            {
                removed.push(name.to_owned());
            }
        }
        // A grant to a file this store just deleted authorizes nothing, and the
        // map is the only thing that would keep it alive for the process.
        if !removed.is_empty()
            && let Ok(mut grants) = self.exact_grants.write()
        {
            for name in removed {
                grants.remove(&canonical_directory.join(name));
            }
        }
    }
}

fn field<'a>(payload: &'a Value, name: &str) -> &'a Value {
    payload.get(name).unwrap_or(&Value::Null)
}

fn grant_reason(error: ContextPathError) -> PromptContextGrantReason {
    match error {
        ContextPathError::SymbolicLink => PromptContextGrantReason::SymbolicLink,
        ContextPathError::TargetType => PromptContextGrantReason::TargetType,
        ContextPathError::InvalidRequest => PromptContextGrantReason::CredentialPath,
        ContextPathError::TargetMissing | ContextPathError::OutsideAuthorizedRoot => {
            PromptContextGrantReason::TargetMissing
        }
    }
}

fn grant_rejection(
    path: &str,
    reason: PromptContextGrantReason,
    message: &str,
) -> PromptContextGrantRejection {
    PromptContextGrantRejection {
        path: path.to_owned(),
        reason,
        message: message.to_owned(),
    }
}

fn grant_message(reason: PromptContextGrantReason) -> &'static str {
    match reason {
        PromptContextGrantReason::InvalidRequest => {
            "That drop could not be read and was not attached."
        }
        PromptContextGrantReason::ItemLimit => "At most 8 files can be dropped at once.",
        PromptContextGrantReason::TargetMissing => {
            "That file is missing or changed and was not attached."
        }
        PromptContextGrantReason::TargetType => "Only ordinary files and folders can be attached.",
        PromptContextGrantReason::SymbolicLink => {
            "Symbolic-link context is not attached. Choose the real file or folder instead."
        }
        PromptContextGrantReason::CredentialPath => {
            "Credential-shaped files and folders cannot be attached."
        }
        PromptContextGrantReason::FieldLimit => "That file path is too long to attach.",
    }
}

fn image_refusal(reason: PromptImageRejectReason) -> PromptImagePasteResult {
    PromptImagePasteResult {
        saved: Vec::new(),
        rejected: vec![image_rejection(reason)],
    }
}

fn image_rejection(reason: PromptImageRejectReason) -> PromptImageRejection {
    PromptImageRejection {
        reason,
        message: match reason {
            PromptImageRejectReason::InvalidRequest => {
                "That paste could not be read and was not attached."
            }
            PromptImageRejectReason::ItemLimit => "At most 8 images can be attached at once.",
            PromptImageRejectReason::SizeLimit => {
                "That image is too large to attach. Save it and attach the file instead."
            }
            PromptImageRejectReason::NotAnImage => {
                "That clipboard content is not an image Interface can read."
            }
            PromptImageRejectReason::WriteFailed => {
                "Interface could not store that pasted image; nothing was attached."
            }
        }
        .to_owned(),
    }
}

fn json_bytes(value: &Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|byte| u8::try_from(byte.as_u64()?).ok())
        .collect()
}

fn write_exclusive(directory: &Path, file: &Path, bytes: &[u8]) -> Result<(), ()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(directory).map_err(|_| ())?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut handle = options.open(file).map_err(|_| ())?;
    handle.write_all(bytes).map_err(|_| ())
}

fn open_read_nofollow(file: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(file)
}

pub(crate) fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut used = 0;
    value
        .chars()
        .take_while(|character| {
            let next = character.len_utf16();
            if used + next > max_units {
                false
            } else {
                used += next;
                true
            }
        })
        .collect()
}

fn grant_one(
    requested: &Path,
    canonical_roots: &[PathBuf],
) -> Result<(PathBuf, ContextPathKind), ContextPathError> {
    let (canonical, kind) = grant_candidate(requested)?;
    let canonical_text = canonical.to_str().ok_or(ContextPathError::TargetMissing)?;
    if canonical.file_name().is_none() {
        return Err(ContextPathError::TargetMissing);
    }
    let directory = kind == ContextPathKind::Folder;
    // The classifier already applies every leaf rule to the last segment.
    if is_search_excluded_path(canonical_text, directory) {
        return Err(ContextPathError::InvalidRequest);
    }
    for canonical_root in canonical_roots {
        if inside(canonical_root, &canonical)
            && is_search_excluded_path(&relative_slash(canonical_root, &canonical), directory)
        {
            return Err(ContextPathError::InvalidRequest);
        }
    }
    Ok((canonical, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drop_limit_message_names_the_item_limit() {
        assert_eq!(
            grant_message(PromptContextGrantReason::ItemLimit),
            format!(
                "At most {} files can be dropped at once.",
                PROMPT_CONTEXT_VALIDATION_LIMITS.max_items
            )
        );
    }
}
