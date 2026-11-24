use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use interface_contracts::ipc::{
    InvalidPromptContextDescriptor, PROMPT_CONTEXT_VALIDATION_LIMITS, PromptContextDescriptor,
    PromptContextDescriptorKind, PromptContextInvalidReason, PromptContextValidation,
    RepositoryReview, RepositoryReviewErrorCode, RepositoryReviewState,
};
use interface_workspace::is_search_excluded_path;
use serde_json::Value;
use url::Url;

use crate::path::{
    ContextPathError, ContextPathKind, canonical_launch_root, relative_slash,
    resolve_canonical_in_roots, stable_context_path,
};
use crate::service::{PromptContextService, utf16_len};

struct ValidationOwner {
    root: PathBuf,
    /// The sending session's key; `None` for a launch (a folder target).
    session: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ItemFailure {
    /// A thread reference the threads owner refused, with its own words.
    Thread(PromptContextInvalidReason, &'static str),
    CredentialPath,
    TargetMissing,
    TargetType,
    SymbolicLink,
    OutsideAuthorizedRoot,
    DiffRootMismatch,
    ReviewUnavailable,
    InvalidUrl,
    FieldLimit,
}

/// What one validation has already admitted.
#[derive(Default)]
struct Accepted {
    ids: BTreeSet<String>,
    identities: BTreeSet<String>,
}

const DUPLICATE_ITEM: &str = "The same prompt context item was attached more than once.";

// 10_000_000 is exact in an f64.
#[allow(clippy::cast_precision_loss)]
const MAX_ESTIMATED_TOKENS: f64 = PROMPT_CONTEXT_VALIDATION_LIMITS.max_estimated_tokens as f64;

impl PromptContextService {
    #[must_use]
    pub fn validate_prompt_context(
        &self,
        raw_target: &Value,
        raw_items: &Value,
    ) -> PromptContextValidation {
        let Some(raw_items) = raw_items.as_array() else {
            return invalid_validation(vec![invalid_context(
                None,
                PromptContextInvalidReason::InvalidRequest,
                "Prompt context was malformed and was not sent.",
                None,
            )]);
        };
        if raw_items.len() > PROMPT_CONTEXT_VALIDATION_LIMITS.max_items {
            return invalid_validation(vec![invalid_context(
                None,
                PromptContextInvalidReason::ItemLimit,
                &format!(
                    "Prompt context may contain at most {} items.",
                    PROMPT_CONTEXT_VALIDATION_LIMITS.max_items
                ),
                None,
            )]);
        }

        // One root set per validation: every item sees the same authority,
        // and the registry is cloned once rather than once per item.
        let roots = self.distinct_authorized_roots();
        let owner = match self.validation_owner(raw_target, &roots) {
            Ok(owner) => owner,
            Err(PromptContextInvalidReason::SessionUnavailable) => {
                return invalid_validation(vec![invalid_context(
                    None,
                    PromptContextInvalidReason::SessionUnavailable,
                    "That session no longer has an authorized working folder. Resume or reopen it.",
                    None,
                )]);
            }
            Err(_) => {
                return invalid_validation(vec![invalid_context(
                    None,
                    PromptContextInvalidReason::OutsideAuthorizedRoot,
                    "That workspace is not currently authorized. Reopen the folder before sending.",
                    None,
                )]);
            }
        };

        let mut invalid = Vec::new();
        let mut valid = Vec::new();
        let mut accepted = Accepted::default();
        for (index, raw) in raw_items.iter().enumerate() {
            match self.validate_item_entry(raw, &owner, &roots, &accepted) {
                Ok(descriptor) => {
                    accepted.ids.insert(descriptor.id.clone());
                    accepted
                        .identities
                        .insert(identity(descriptor.kind, &descriptor.value));
                    valid.push(descriptor);
                }
                Err((reason, message)) => {
                    invalid.push(invalid_context(Some(index), reason, message, Some(raw)));
                }
            }
        }

        if invalid.is_empty() {
            PromptContextValidation {
                ok: true,
                items: valid,
                invalid: Vec::new(),
            }
        } else {
            invalid_validation(invalid)
        }
    }

    /// One item's rules in their frozen order; the first failure is its refusal.
    fn validate_item_entry(
        &self,
        raw: &Value,
        owner: &ValidationOwner,
        roots: &[PathBuf],
        accepted: &Accepted,
    ) -> Result<PromptContextDescriptor, (PromptContextInvalidReason, &'static str)> {
        let item = raw.as_object().ok_or((
            PromptContextInvalidReason::InvalidShape,
            "A prompt context item was malformed and was not sent.",
        ))?;
        let text = |name: &str, max: usize| prompt_context_text(item.get(name), max);
        let fields = (
            text("id", PROMPT_CONTEXT_VALIDATION_LIMITS.max_id_chars),
            text("label", PROMPT_CONTEXT_VALIDATION_LIMITS.max_label_chars),
            text("value", PROMPT_CONTEXT_VALIDATION_LIMITS.max_value_chars),
            optional(item.get("detail"), |detail| {
                prompt_context_text(
                    Some(detail),
                    PROMPT_CONTEXT_VALIDATION_LIMITS.max_detail_chars,
                )
            }),
            optional(item.get("estimatedTokens"), |estimated| {
                estimated
                    .as_f64()
                    .filter(|estimated| (0.0..=MAX_ESTIMATED_TOKENS).contains(estimated))
            }),
        );
        let (Some(id), Some(label), Some(value), Ok(detail), Ok(estimated)) = fields else {
            return Err((
                PromptContextInvalidReason::FieldLimit,
                "A prompt context item contains a missing or overlong field and was not sent.",
            ));
        };
        let kind = item
            .get("kind")
            .and_then(Value::as_str)
            .and_then(parse_kind)
            .ok_or((
                PromptContextInvalidReason::UnsupportedKind,
                "That prompt context type is not supported and was not sent.",
            ))?;
        // A diff's Review bytes and a thread's block are always rebuilt here;
        // what the renderer sent for either is ignored.
        if item.contains_key("safeContent")
            && !matches!(
                kind,
                PromptContextDescriptorKind::Diff | PromptContextDescriptorKind::Chat
            )
        {
            return Err((
                PromptContextInvalidReason::InvalidShape,
                "Embedded context is accepted only from a freshly validated diff.",
            ));
        }
        if accepted.ids.contains(&id) {
            return Err((
                PromptContextInvalidReason::Duplicate,
                "The same prompt context identity was attached more than once.",
            ));
        }
        if accepted.identities.contains(&identity(kind, &value)) {
            return Err((PromptContextInvalidReason::Duplicate, DUPLICATE_ITEM));
        }
        let (canonical_value, safe_content) = self
            .validate_item(kind, &value, owner, roots)
            .map_err(|failure| {
                if let ItemFailure::Thread(reason, message) = failure {
                    return (reason, message);
                }
                let reason = invalid_reason(failure);
                (reason, item_failure_message(reason, kind))
            })?;
        if accepted
            .identities
            .contains(&identity(kind, &canonical_value))
        {
            return Err((PromptContextInvalidReason::Duplicate, DUPLICATE_ITEM));
        }
        Ok(PromptContextDescriptor {
            id,
            kind,
            label,
            value: canonical_value,
            detail,
            estimated_tokens: estimated.and_then(floored_u64),
            safe_content: match kind {
                PromptContextDescriptorKind::Diff => Some(safe_content.unwrap_or_default()),
                PromptContextDescriptorKind::Chat => safe_content,
                _ => None,
            },
        })
    }

    fn validation_owner(
        &self,
        raw_target: &Value,
        roots: &[PathBuf],
    ) -> Result<ValidationOwner, PromptContextInvalidReason> {
        if let Some(session_key) = raw_target.as_str() {
            let root = self
                .runtime
                .session_root(session_key)
                .filter(|root| !root.as_os_str().is_empty())
                .ok_or(PromptContextInvalidReason::SessionUnavailable)?;
            let root = resolve_canonical_in_roots(&root, roots)
                .map_err(|_| PromptContextInvalidReason::OutsideAuthorizedRoot)?;
            return Ok(ValidationOwner {
                root,
                session: Some(session_key.to_owned()),
            });
        }
        let root = raw_target
            .as_object()
            .and_then(|target| target.get("root"))
            .and_then(Value::as_str)
            .ok_or(PromptContextInvalidReason::OutsideAuthorizedRoot)?;
        let candidate = canonical_launch_root(Path::new(root))
            .map_err(|_| PromptContextInvalidReason::OutsideAuthorizedRoot)?;
        resolve_canonical_in_roots(&candidate, roots)
            .map(|root| ValidationOwner {
                root,
                session: None,
            })
            .map_err(|_| PromptContextInvalidReason::OutsideAuthorizedRoot)
    }

    fn validate_item(
        &self,
        kind: PromptContextDescriptorKind,
        value: &str,
        owner: &ValidationOwner,
        roots: &[PathBuf],
    ) -> Result<(String, Option<String>), ItemFailure> {
        let mut canonical_value = value.to_owned();
        let mut safe_content = None;
        match kind {
            PromptContextDescriptorKind::File | PromptContextDescriptorKind::Folder => {
                let directory = kind == PromptContextDescriptorKind::Folder;
                if is_search_excluded_path(value, directory) {
                    return Err(ItemFailure::CredentialPath);
                }
                let path_kind = if directory {
                    ContextPathKind::Folder
                } else {
                    ContextPathKind::File
                };
                let canonical = stable_context_path(
                    Path::new(value),
                    path_kind,
                    roots,
                    &|candidate| self.is_exact_grant(candidate),
                    owner.session.is_none().then_some(owner.root.as_path()),
                )
                .map_err(map_path_failure)?;
                let relative = relative_slash(&owner.root, &canonical);
                if is_search_excluded_path(&relative, directory) {
                    return Err(ItemFailure::CredentialPath);
                }
                canonical
                    .to_str()
                    .ok_or(ItemFailure::TargetMissing)?
                    .clone_into(&mut canonical_value);
            }
            PromptContextDescriptorKind::Diff => {
                let canonical =
                    canonical_launch_root(Path::new(value)).map_err(map_path_failure)?;
                if canonical != owner.root {
                    return Err(ItemFailure::DiffRootMismatch);
                }
                let review = self.review.read(&owner.root);
                let usable_complete = review.error.is_none() && review.clean.is_some();
                let usable_bounded_partial = review
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code == RepositoryReviewErrorCode::OutputLimit);
                if !review.repository || (!usable_complete && !usable_bounded_partial) {
                    return Err(ItemFailure::ReviewUnavailable);
                }
                safe_content = Some(bounded_safe_review(&review));
                canonical
                    .to_str()
                    .ok_or(ItemFailure::TargetMissing)?
                    .clone_into(&mut canonical_value);
            }
            PromptContextDescriptorKind::Url => {
                let parsed = Url::parse(value).map_err(|_| ItemFailure::InvalidUrl)?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(ItemFailure::InvalidUrl);
                }
                canonical_value = parsed.to_string();
            }
            PromptContextDescriptorKind::Chat => {
                // T3 section 9: the threads owner decides what another
                // thread's block holds and whether the sender may read it.
                let sender = owner.session.as_deref().ok_or(ItemFailure::Thread(
                    PromptContextInvalidReason::SessionUnavailable,
                    "A thread reference works in a thread's own composer; send the thread first, then refer to the other one.",
                ))?;
                let threads = self.threads.get().ok_or(ItemFailure::Thread(
                    PromptContextInvalidReason::UnsupportedKind,
                    "Thread references are not available in this Interface.",
                ))?;
                let reference = threads
                    .thread_reference(sender, value)
                    .map_err(|refusal| ItemFailure::Thread(refusal.reason, refusal.message))?;
                if utf16_len(&reference.block)
                    > PROMPT_CONTEXT_VALIDATION_LIMITS.max_safe_content_chars
                {
                    return Err(ItemFailure::FieldLimit);
                }
                canonical_value = reference.key;
                safe_content = Some(reference.block);
            }
            PromptContextDescriptorKind::Mcp | PromptContextDescriptorKind::Skill => {}
        }
        if utf16_len(&canonical_value) > PROMPT_CONTEXT_VALIDATION_LIMITS.max_value_chars {
            return Err(ItemFailure::FieldLimit);
        }
        Ok((canonical_value, safe_content))
    }
}

fn prompt_context_text(value: Option<&Value>, max: usize) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && utf16_len(value) <= max).then(|| value.to_owned())
}

/// An absent field is admitted as `None`; a present one only if `admit`
/// accepts it.
fn optional<T>(
    value: Option<&Value>,
    admit: impl FnOnce(&Value) -> Option<T>,
) -> Result<Option<T>, ()> {
    value.map_or(Ok(None), |value| admit(value).map(Some).ok_or(()))
}

/// `value` was admitted to `0.0..=MAX_ESTIMATED_TOKENS`, so the cast neither
/// truncates nor wraps. That range admits negative zero, which has never
/// produced an estimate and still does not.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn floored_u64(value: f64) -> Option<u64> {
    value.is_sign_positive().then(|| value.floor() as u64)
}

fn invalid_context(
    index: Option<usize>,
    reason: PromptContextInvalidReason,
    message: &str,
    candidate: Option<&Value>,
) -> InvalidPromptContextDescriptor {
    let record = candidate.and_then(Value::as_object);
    InvalidPromptContextDescriptor {
        index: index.and_then(|index| u64::try_from(index).ok()),
        id: record.and_then(|record| {
            prompt_context_text(
                record.get("id"),
                PROMPT_CONTEXT_VALIDATION_LIMITS.max_id_chars,
            )
        }),
        kind: record.and_then(|record| prompt_context_text(record.get("kind"), 32)),
        reason,
        message: message.to_owned(),
    }
}

fn invalid_validation(invalid: Vec<InvalidPromptContextDescriptor>) -> PromptContextValidation {
    PromptContextValidation {
        ok: false,
        items: Vec::new(),
        invalid,
    }
}

fn parse_kind(value: &str) -> Option<PromptContextDescriptorKind> {
    match value {
        "file" => Some(PromptContextDescriptorKind::File),
        "folder" => Some(PromptContextDescriptorKind::Folder),
        "diff" => Some(PromptContextDescriptorKind::Diff),
        "chat" => Some(PromptContextDescriptorKind::Chat),
        "url" => Some(PromptContextDescriptorKind::Url),
        "mcp" => Some(PromptContextDescriptorKind::Mcp),
        "skill" => Some(PromptContextDescriptorKind::Skill),
        _ => None,
    }
}

fn identity(kind: PromptContextDescriptorKind, value: &str) -> String {
    format!("{}\0{value}", kind_name(kind))
}

const fn kind_name(kind: PromptContextDescriptorKind) -> &'static str {
    match kind {
        PromptContextDescriptorKind::File => "file",
        PromptContextDescriptorKind::Folder => "folder",
        PromptContextDescriptorKind::Diff => "diff",
        PromptContextDescriptorKind::Chat => "chat",
        PromptContextDescriptorKind::Url => "url",
        PromptContextDescriptorKind::Mcp => "mcp",
        PromptContextDescriptorKind::Skill => "skill",
    }
}

fn map_path_failure(error: ContextPathError) -> ItemFailure {
    match error {
        ContextPathError::TargetType => ItemFailure::TargetType,
        ContextPathError::SymbolicLink => ItemFailure::SymbolicLink,
        ContextPathError::OutsideAuthorizedRoot => ItemFailure::OutsideAuthorizedRoot,
        ContextPathError::InvalidRequest | ContextPathError::TargetMissing => {
            ItemFailure::TargetMissing
        }
    }
}

const fn invalid_reason(failure: ItemFailure) -> PromptContextInvalidReason {
    match failure {
        ItemFailure::CredentialPath => PromptContextInvalidReason::CredentialPath,
        ItemFailure::TargetMissing => PromptContextInvalidReason::TargetMissing,
        ItemFailure::TargetType => PromptContextInvalidReason::TargetType,
        ItemFailure::SymbolicLink => PromptContextInvalidReason::SymbolicLink,
        ItemFailure::OutsideAuthorizedRoot => PromptContextInvalidReason::OutsideAuthorizedRoot,
        ItemFailure::DiffRootMismatch => PromptContextInvalidReason::DiffRootMismatch,
        ItemFailure::ReviewUnavailable => PromptContextInvalidReason::ReviewUnavailable,
        ItemFailure::InvalidUrl => PromptContextInvalidReason::InvalidUrl,
        ItemFailure::FieldLimit => PromptContextInvalidReason::FieldLimit,
        ItemFailure::Thread(reason, _) => reason,
    }
}

fn item_failure_message(
    reason: PromptContextInvalidReason,
    kind: PromptContextDescriptorKind,
) -> &'static str {
    match reason {
        PromptContextInvalidReason::InvalidRequest => {
            "Prompt context was malformed and was not sent."
        }
        PromptContextInvalidReason::ItemLimit => "Too many prompt context items were supplied.",
        PromptContextInvalidReason::InvalidShape => {
            "A prompt context item was malformed and was not sent."
        }
        PromptContextInvalidReason::UnsupportedKind => "That prompt context type is not supported.",
        PromptContextInvalidReason::FieldLimit => "A prompt context field is invalid or too long.",
        PromptContextInvalidReason::Duplicate => {
            "The same prompt context item was attached more than once."
        }
        PromptContextInvalidReason::SessionUnavailable => {
            "That session no longer has an authorized folder."
        }
        PromptContextInvalidReason::OutsideAuthorizedRoot => {
            "That context is outside your currently open folders. Reattach it before sending."
        }
        PromptContextInvalidReason::TargetMissing => {
            "That context target is missing or changed. Reattach it before sending."
        }
        PromptContextInvalidReason::TargetType => match kind {
            PromptContextDescriptorKind::File => {
                "That context target is not a file. Reattach it before sending."
            }
            PromptContextDescriptorKind::Folder => {
                "That context target is not a folder. Reattach it before sending."
            }
            PromptContextDescriptorKind::Diff => {
                "That context target is not a diff. Reattach it before sending."
            }
            PromptContextDescriptorKind::Chat => {
                "That context target is not a chat. Reattach it before sending."
            }
            PromptContextDescriptorKind::Url => {
                "That context target is not a url. Reattach it before sending."
            }
            PromptContextDescriptorKind::Mcp => {
                "That context target is not a mcp. Reattach it before sending."
            }
            PromptContextDescriptorKind::Skill => {
                "That context target is not a skill. Reattach it before sending."
            }
        },
        PromptContextInvalidReason::SymbolicLink => {
            "Symbolic-link context is not sent. Choose the real file or folder instead."
        }
        PromptContextInvalidReason::CredentialPath => {
            "Credential-shaped files and folders cannot be attached."
        }
        PromptContextInvalidReason::DiffRootMismatch => {
            "The queued diff belongs to a different workspace. Reattach it before sending."
        }
        PromptContextInvalidReason::ReviewUnavailable => {
            "Interface could not produce a safe, current Git review. Refresh Changes before sending."
        }
        PromptContextInvalidReason::InvalidUrl => {
            "Only complete http:// or https:// URLs can be attached."
        }
    }
}

fn bounded_safe_review(review: &RepositoryReview) -> String {
    let admitted = review
        .files
        .iter()
        .filter(|file| {
            !file.patch.is_empty()
                && file.omitted.is_none()
                && !file.binary
                && !file.patch_truncated
        })
        .collect::<Vec<_>>();
    let branch = if review.metadata_complete == Some(false) {
        "branch metadata unavailable".to_owned()
    } else if review.branch.is_none() {
        "detached HEAD".to_owned()
    } else if review
        .branch
        .as_ref()
        .is_some_and(|branch| utf16_len(branch) <= 256)
    {
        review.branch.clone().unwrap_or_default()
    } else {
        "branch name withheld (over limit)".to_owned()
    };
    let header_with = |partial: bool, included: usize| {
        [
            "Interface Review snapshot (credential-filtered, read-only)".to_owned(),
            format!("Branch: {branch}"),
            format!(
                "Commit: {}",
                if review.metadata_complete == Some(false) {
                    "commit metadata unavailable".to_owned()
                } else {
                    review.head.as_deref().map_or_else(
                        || "no commits".to_owned(),
                        |head| head.chars().take(12).collect(),
                    )
                }
            ),
            format!("Complete text patches included: {included}"),
            format!(
                "Reported changed paths: {}{}",
                review.files.len(),
                if review.truncated { " (partial list)" } else { "" }
            ),
            if partial {
                "Snapshot status: partial; omitted, non-text, unsafe, truncated, or over-budget data is not included and must not be reconstructed."
            } else {
                "Snapshot status: complete for every reported changed path."
            }
            .to_owned(),
        ]
        .join("\n")
    };
    let header =
        |included: usize| header_with(review.truncated || included < review.files.len(), included);
    let partial_header_chars = utf16_len(&header_with(true, 0));
    let complete_header_chars = utf16_len(&header_with(false, 0));
    let mut blocks = Vec::new();
    let mut accumulated = 0_usize;
    for file in admitted {
        let states = file
            .states
            .iter()
            .map(|state| match state {
                RepositoryReviewState::Staged => "staged",
                RepositoryReviewState::Unstaged => "unstaged",
                RepositoryReviewState::Untracked => "untracked",
                RepositoryReviewState::Conflict => "conflict",
            })
            .collect::<Vec<_>>()
            .join(", ");
        let block = format!("\n\n--- {} ({states}) ---\n{}", file.path, file.patch);
        // UTF-16 length is additive over concatenation, so the accumulated
        // total stands in for re-measuring every admitted block each round.
        // Header length varies with the included-count digit width and the
        // two frozen status strings; both bases are measured once.
        let block_chars = utf16_len(&block);
        let included = blocks.len() + 1;
        let header_base = if review.truncated || included < review.files.len() {
            partial_header_chars
        } else {
            complete_header_chars
        };
        let header_chars = header_base + decimal_digit_count(included) - 1;
        if header_chars + accumulated + block_chars
            <= PROMPT_CONTEXT_VALIDATION_LIMITS.max_safe_content_chars
        {
            accumulated += block_chars;
            blocks.push(block);
        }
    }
    let mut result = format!("{}{}", header(blocks.len()), blocks.concat());
    if blocks.is_empty() {
        let empty = "\n\nNo credential-filtered text patches were available in this snapshot.";
        if utf16_len(&result) + utf16_len(empty)
            <= PROMPT_CONTEXT_VALIDATION_LIMITS.max_safe_content_chars
        {
            result.push_str(empty);
        }
    }
    result
}

const fn decimal_digit_count(value: usize) -> usize {
    match value.checked_ilog10() {
        Some(log) => log as usize + 1,
        None => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::ipc::{
        RepositoryReviewFile, RepositoryReviewFileStatus, RepositoryReviewOmission,
    };

    fn review(files: Vec<RepositoryReviewFile>) -> RepositoryReview {
        RepositoryReview {
            root: "/repo".into(),
            repository: true,
            branch: Some("main".into()),
            head: Some("0123456789abcdef".into()),
            metadata_complete: Some(true),
            clean: Some(false),
            files,
            truncated: false,
            error: None,
        }
    }

    #[test]
    fn safe_review_never_includes_credential_omissions_or_partial_patches() {
        let omitted = RepositoryReviewFile {
            path: ".env".into(),
            previous_path: None,
            states: vec![RepositoryReviewState::Untracked],
            status: RepositoryReviewFileStatus::Untracked,
            additions: None,
            deletions: None,
            binary: false,
            patch: "SECRET=value".into(),
            patch_truncated: false,
            omitted: Some(RepositoryReviewOmission::Credential),
        };
        let partial = RepositoryReviewFile {
            path: "src/lib.rs".into(),
            previous_path: None,
            states: vec![RepositoryReviewState::Unstaged],
            status: RepositoryReviewFileStatus::Modified,
            additions: Some(1),
            deletions: Some(0),
            binary: false,
            patch: "+partial".into(),
            patch_truncated: true,
            omitted: None,
        };
        let result = bounded_safe_review(&review(vec![omitted, partial]));
        assert!(!result.contains("SECRET=value"));
        assert!(!result.contains("+partial"));
        assert!(result.contains("Complete text patches included: 0"));
    }

    #[test]
    fn review_header_admits_the_last_file_that_completes_the_snapshot() {
        let file = |path: &str| RepositoryReviewFile {
            path: path.into(),
            previous_path: None,
            states: vec![RepositoryReviewState::Unstaged],
            status: RepositoryReviewFileStatus::Modified,
            additions: Some(1),
            deletions: Some(0),
            binary: false,
            patch: "+ok\n".into(),
            patch_truncated: false,
            omitted: None,
        };
        let result = bounded_safe_review(&review(vec![file("a.rs"), file("b.rs")]));
        assert!(result.contains("Complete text patches included: 2"));
        assert!(result.contains("--- a.rs"));
        assert!(result.contains("--- b.rs"));
        assert!(result.contains("Snapshot status: complete for every reported changed path."));
        assert_eq!(decimal_digit_count(0), 1);
        assert_eq!(decimal_digit_count(9), 1);
        assert_eq!(decimal_digit_count(10), 2);
    }
}
