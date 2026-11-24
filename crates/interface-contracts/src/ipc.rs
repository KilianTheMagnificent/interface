//! Desktop IPC data transfer objects and stable channel identifiers.

use serde::{Deserialize, Serialize};

use crate::{
    ValidationError,
    engine::{EngineEvent, EngineId, EngineInfo, SessionKey, SessionSummary},
};

pub const HISTORY_LIMIT: u64 = 500;
pub const HISTORY_MAX_LIMIT: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryReadOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
}

#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn history_read_limit(requested: Option<f64>) -> u64 {
    let Some(requested) = requested.filter(|value| value.is_finite()) else {
        return HISTORY_LIMIT;
    };
    (requested.floor() as i128).clamp(i128::from(HISTORY_LIMIT), i128::from(HISTORY_MAX_LIMIT))
        as u64
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<HistoryAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub events: Vec<EngineEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<u64>,
    pub older_messages: u64,
}

impl HistoryPage {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            availability: None,
            reason: None,
            events: Vec::new(),
            messages: None,
            older_messages: 0,
        }
    }
}

#[must_use]
pub fn unavailable_history(reason: impl Into<String>) -> HistoryPage {
    HistoryPage {
        availability: Some(HistoryAvailability::Unavailable),
        reason: Some(reason.into()),
        ..HistoryPage::empty()
    }
}

#[must_use]
pub fn as_history_page(events: Vec<EngineEvent>) -> HistoryPage {
    HistoryPage {
        events,
        ..HistoryPage::empty()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineOp {
    RenameSession,
    DeleteSession,
    ForkSession,
    SetArchived,
    AdoptSession,
    ReadUsage,
}

pub const ENGINE_OPS: [EngineOp; 6] = [
    EngineOp::RenameSession,
    EngineOp::DeleteSession,
    EngineOp::ForkSession,
    EngineOp::SetArchived,
    EngineOp::AdoptSession,
    EngineOp::ReadUsage,
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineOpSupport {
    pub engine: EngineId,
    pub ops: Vec<EngineOp>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineProbe {
    pub engine: EngineId,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<EngineInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpProfile {
    pub slug: String,
    pub display_name: String,
    pub executable_path: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpProfilesUpdate {
    pub profiles: Vec<AcpProfile>,
    pub restart_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileSnapshot {
    pub text: String,
    pub revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSearchMatch {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub preview: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSearchResponse {
    pub matches: Vec<ProjectSearchMatch>,
    pub files_scanned: u64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectFileEntry {
    pub path: String,
    pub relative_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectFileListResponse {
    pub files: Vec<ProjectFileEntry>,
    pub files_scanned: u64,
    pub entries_scanned: u64,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderListingFreshness {
    Fresh,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderListingStatus {
    pub engine: EngineId,
    pub status: ProviderListingFreshness,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionListingSnapshot {
    pub sessions: Vec<SessionSummary>,
    pub providers: Vec<ProviderListingStatus>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionSearchMatchKind {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSearchMatch {
    pub kind: SessionSearchMatchKind,
    pub preview: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSearchHit {
    pub key: SessionKey,
    pub title: String,
    pub cwd: String,
    pub matches: Vec<SessionSearchMatch>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSearchProviderNotice {
    pub engine: EngineId,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSearchResponse {
    pub hits: Vec<SessionSearchHit>,
    pub providers: Vec<SessionSearchProviderNotice>,
    pub sessions_scanned: u64,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedWorktreeState {
    Active,
    Retained,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedWorktreeInfo {
    pub key: SessionKey,
    pub path: String,
    pub session_cwd: String,
    pub branch: String,
    pub repo_root: String,
    pub source_cwd: String,
    pub dirty_source: bool,
    pub state: ManagedWorktreeState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedWorktreeKeptReason {
    Dirty,
    Failed,
    Open,
    Active,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedWorktreeCleanup {
    pub worktree: ManagedWorktreeInfo,
    pub removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<ManagedWorktreeKeptReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TerminalApp {
    Ghostty,
    Terminal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenTerminalResult {
    pub root: String,
    pub app: TerminalApp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryReviewState {
    Staged,
    Unstaged,
    Untracked,
    Conflict,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryReviewFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
    TypeChanged,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryReviewOmission {
    Credential,
    Binary,
    Symlink,
    NotRegular,
    UnsafePath,
    OutputLimit,
    GitError,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryReviewFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    pub states: Vec<RepositoryReviewState>,
    pub status: RepositoryReviewFileStatus,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
    pub patch: String,
    pub patch_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<RepositoryReviewOmission>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryReviewErrorCode {
    GitUnavailable,
    NotRepository,
    GitError,
    OutputLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryReviewError {
    pub code: RepositoryReviewErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryReview {
    pub root: String,
    pub repository: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_complete: Option<bool>,
    pub clean: Option<bool>,
    pub files: Vec<RepositoryReviewFile>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RepositoryReviewError>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewCheckName {
    Check,
    Test,
    Typecheck,
    Lint,
    Build,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewCheckDiscoveryError {
    InvalidPackage,
    PackageTooLarge,
    UnsafePackage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewCheckDiscovery {
    pub root: String,
    pub package_present: bool,
    pub checks: Vec<ReviewCheckName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReviewCheckDiscoveryError>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewCheckRunStatus {
    Passed,
    Failed,
    TimedOut,
    OutputLimit,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewCheckRunResult {
    pub name: ReviewCheckName,
    pub status: ReviewCheckRunStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub output_truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewCheckpointKind {
    Review,
    Recovery,
    Turn,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewCheckpointOmissionReason {
    Credential,
    Binary,
    Symlink,
    NotRegular,
    UnsafePath,
    OutputLimit,
    GitError,
    Deleted,
    Changed,
    FileLimit,
    ByteLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewCheckpointSummary {
    pub id: String,
    pub kind: ReviewCheckpointKind,
    pub created_at: String,
    pub repository_root: String,
    pub repository_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_checkpoint_id: Option<String>,
    pub file_count: u64,
    pub omitted_count: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCheckpointFile {
    pub path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCheckpointOmission {
    pub path: String,
    pub reason: ReviewCheckpointOmissionReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewCheckpointDetail {
    #[serde(flatten)]
    pub summary: ReviewCheckpointSummary,
    pub files: Vec<ReviewCheckpointFile>,
    pub omitted: Vec<ReviewCheckpointOmission>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptContextDescriptorKind {
    File,
    Folder,
    Diff,
    Chat,
    Url,
    Mcp,
    Skill,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptContextDescriptor {
    pub id: String,
    pub kind: PromptContextDescriptorKind,
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_content: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptContextGrantKind {
    File,
    Folder,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptContextGrant {
    pub path: String,
    pub kind: PromptContextGrantKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptContextGrantReason {
    InvalidRequest,
    ItemLimit,
    TargetMissing,
    TargetType,
    SymbolicLink,
    CredentialPath,
    FieldLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptContextGrantRejection {
    pub path: String,
    pub reason: PromptContextGrantReason,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptContextGrantResult {
    pub granted: Vec<PromptContextGrant>,
    pub rejected: Vec<PromptContextGrantRejection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptImageSaved {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptImageRejectReason {
    InvalidRequest,
    ItemLimit,
    SizeLimit,
    NotAnImage,
    WriteFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptImageRejection {
    pub reason: PromptImageRejectReason,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptImagePasteResult {
    pub saved: Vec<PromptImageSaved>,
    pub rejected: Vec<PromptImageRejection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PromptContextValidationLimits {
    pub max_items: usize,
    pub max_id_chars: usize,
    pub max_label_chars: usize,
    pub max_value_chars: usize,
    pub max_detail_chars: usize,
    pub max_estimated_tokens: u64,
    pub max_safe_content_chars: usize,
}

pub const PROMPT_CONTEXT_VALIDATION_LIMITS: PromptContextValidationLimits =
    PromptContextValidationLimits {
        max_items: 8,
        max_id_chars: 128,
        max_label_chars: 256,
        max_value_chars: 512,
        max_detail_chars: 512,
        max_estimated_tokens: 10_000_000,
        max_safe_content_chars: 8_000,
    };

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptContextInvalidReason {
    InvalidRequest,
    ItemLimit,
    InvalidShape,
    UnsupportedKind,
    FieldLimit,
    Duplicate,
    SessionUnavailable,
    OutsideAuthorizedRoot,
    TargetMissing,
    TargetType,
    SymbolicLink,
    CredentialPath,
    DiffRootMismatch,
    ReviewUnavailable,
    InvalidUrl,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvalidPromptContextDescriptor {
    pub index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub reason: PromptContextInvalidReason,
    pub message: String,
}

/// A batch is sent only when `ok`; a refused batch carries no items. Unknown
/// keys are ignored on decode, as the former untagged enum ignored them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptContextValidation {
    pub ok: bool,
    pub items: Vec<PromptContextDescriptor>,
    pub invalid: Vec<InvalidPromptContextDescriptor>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomizationScope {
    Project,
    User,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomizationKind {
    Rules,
    Skill,
    Hook,
    Mcp,
    Command,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomizationEntry {
    pub kind: CustomizationKind,
    pub name: String,
    pub detail: String,
    pub scope: CustomizationScope,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomizationInventory {
    pub root: String,
    pub entries: Vec<CustomizationEntry>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CollaborationEngine {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "acp:grok")]
    AcpGrok,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationMode {
    Consult,
    Handoff,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationState {
    Requested,
    AwaitingApproval,
    Approved,
    Preflight,
    PreparingTarget,
    RunningTarget,
    ResultReady,
    AwaitingPeerReview,
    DeliveryPending,
    ParkingSource,
    OwnershipTransfer,
    ActivatingTarget,
    Completed,
    Denied,
    Cancelled,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationSubmitRequest {
    pub source_session_key: SessionKey,
    pub mode: CollaborationMode,
    pub target_engine: CollaborationEngine,
    pub objective: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationErrorDto {
    pub code: String,
    pub safe_next_action: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationReceiptDto {
    pub version: u8,
    pub collaboration_id: String,
    pub revision: u64,
    pub state: CollaborationState,
    pub mode: CollaborationMode,
    pub source_engine: CollaborationEngine,
    pub target_engine: CollaborationEngine,
    pub approval_required: bool,
    pub cancellation_safe: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CollaborationErrorDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalDto {
    pub collaboration_id: String,
    pub approval_id: String,
    pub expected_revision: u64,
    pub expires_at: String,
    pub mode: CollaborationMode,
    pub source_engine: CollaborationEngine,
    pub source_session_key: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_title: Option<String>,
    pub canonical_cwd: String,
    pub canonical_project_root: String,
    pub target_engine: CollaborationEngine,
    pub objective: String,
    pub packet_hash: String,
    pub packet_preview: String,
    pub packet_bytes: u64,
    pub context_bytes: u64,
    pub omissions: u64,
    pub managed_worktree: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollaborationPeerState {
    AwaitingReview,
    DeliveryPending,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPeerDto {
    pub state: CollaborationPeerState,
    pub collaboration_id: String,
    pub expected_revision: u64,
    pub result_id: String,
    pub result_digest: String,
    pub result_bytes: u64,
    pub source_session_key: SessionKey,
    pub source_engine: CollaborationEngine,
    pub target_session_key: SessionKey,
    pub target_engine: CollaborationEngine,
    pub objective: String,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationStatusDto {
    pub receipt: CollaborationReceiptDto,
    pub source_session_key: SessionKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_key: Option<SessionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<CollaborationApprovalDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<CollaborationPeerDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalAction {
    pub collaboration_id: String,
    pub approval_id: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPeerAction {
    pub collaboration_id: String,
    pub result_id: String,
    pub result_digest: String,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationDraftStateUpdate {
    pub source_session_key: SessionKey,
    pub revision: u64,
    pub empty: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationEventDto {
    pub version: u8,
    pub kind: String,
    pub status: CollaborationStatusDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationTimelineRequest {
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationTranscriptRequest {
    /// The app sends `agentId`; `agent_id` stays accepted for older `iface` clients.
    #[serde(alias = "agent_id")]
    pub agent_id: String,
    pub lines: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationInboxRequest {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentStopRequest {
    /// The app sends `agentId`; `agent_id` stays accepted for older `iface` clients.
    #[serde(alias = "agent_id")]
    pub agent_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationAgentSteerRequest {
    pub agent_id: String,
    pub body: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationRoleClaimRequest {
    pub agent_id: String,
    pub role: String,
    pub responsibility: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationRoleRelinquishRequest {
    pub claim_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationWorktreeEscrowRequest {
    pub lease_id: String,
    pub source_agent_id: String,
    pub target_agent_id: String,
    pub source_owner_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestrationEventDto {
    pub version: u8,
    pub kind: String,
    pub host_id: String,
    pub runtime_epoch: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IpcChannel {
    pub name: &'static str,
    pub value: &'static str,
}

macro_rules! channels {
    ($(($field:ident, $wire:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct IpcChannels { $(pub $field: &'static str),+ }

        pub const CHANNELS: IpcChannels = IpcChannels { $($field: $wire),+ };

        pub const ALL_CHANNELS: &[IpcChannel] = &[$(IpcChannel { name: stringify!($field), value: $wire }),+];

        #[must_use]
        pub fn is_shared_channel(value: &str) -> bool {
            matches!(value, $($wire)|+)
        }
    };
}

channels!(
    (get_appearance, "iface:getAppearance"),
    (set_appearance, "iface:setAppearance"),
    (get_acp_profiles, "iface:getAcpProfiles"),
    (set_acp_profiles, "iface:setAcpProfiles"),
    (agent_control, "iface:agentControl"),
    (harness_control, "iface:harnessControl"),
    (recovery_control, "iface:recoveryControl"),
    (thread_control, "iface:threadControl"),
    (thread_journal, "iface:threadJournal"),
    (provider_accounts, "iface:providerAccounts"),
    (draft_stash, "iface:draftStash"),
    (thread_secret, "iface:threadSecret"),
    (inspect_terminal_servers, "iface:inspectTerminalServers"),
    (get_terminal_workspaces, "iface:getTerminalWorkspaces"),
    (set_terminal_workspaces, "iface:setTerminalWorkspaces"),
    (
        resolve_moved_terminal_folder,
        "iface:resolveMovedTerminalFolder"
    ),
    (update_sessions, "iface:updateSessions"),
    (get_update_state, "iface:getUpdateState"),
    (check_for_updates, "iface:checkForUpdates"),
    (restart_to_update, "iface:restartToUpdate"),
    (get_orchestration_snapshot, "iface:getOrchestrationSnapshot"),
    (
        read_orchestration_timeline,
        "iface:readOrchestrationTimeline"
    ),
    (start_adversarial_review, "iface:startAdversarialReview"),
    (create_orchestration_agent, "iface:createOrchestrationAgent"),
    (send_orchestration_message, "iface:sendOrchestrationMessage"),
    (
        reply_orchestration_message,
        "iface:replyOrchestrationMessage"
    ),
    (
        read_orchestration_transcript,
        "iface:readOrchestrationTranscript"
    ),
    (read_orchestration_inbox, "iface:readOrchestrationInbox"),
    (stop_orchestration_agent, "iface:stopOrchestrationAgent"),
    (steer_orchestration_agent, "iface:steerOrchestrationAgent"),
    (claim_orchestration_role, "iface:claimOrchestrationRole"),
    (
        relinquish_orchestration_role,
        "iface:relinquishOrchestrationRole"
    ),
    (
        escrow_orchestration_worktree,
        "iface:escrowOrchestrationWorktree"
    ),
    (set_orchestrator_policy, "iface:setOrchestratorPolicy"),
    (clear_orchestrator_policy, "iface:clearOrchestratorPolicy"),
    (bind_orchestration_parent, "iface:bindOrchestrationParent"),
    (
        register_orchestration_session,
        "iface:registerOrchestrationSession"
    ),
    (submit_coordinated_worker, "iface:submitCoordinatedWorker"),
    (
        integrate_coordinated_worker,
        "iface:integrateCoordinatedWorker"
    ),
    (abandon_coordinated_task, "iface:abandonCoordinatedTask"),
    (ack_coordinated_obligation, "iface:ackCoordinatedObligation"),
    (get_coordination_status, "iface:getCoordinationStatus"),
    (get_orchestrator_models, "iface:getOrchestratorModels"),
    (list_collaborations, "iface:listCollaborations"),
    (submit_collaboration, "iface:submitCollaboration"),
    (get_collaboration_status, "iface:getCollaborationStatus"),
    (approve_collaboration, "iface:approveCollaboration"),
    (deny_collaboration, "iface:denyCollaboration"),
    (accept_collaboration_peer, "iface:acceptCollaborationPeer"),
    (discard_collaboration_peer, "iface:discardCollaborationPeer"),
    (cancel_collaboration, "iface:cancelCollaboration"),
    (
        set_collaboration_draft_state,
        "iface:setCollaborationDraftState"
    ),
    (list_sessions, "iface:listSessions"),
    (list_session_snapshot, "iface:listSessionSnapshot"),
    (list_archived_sessions, "iface:listArchivedSessions"),
    (list_provider_commands, "iface:listProviderCommands"),
    (create_session, "iface:createSession"),
    (resume_session, "iface:resumeSession"),
    (send, "iface:send"),
    (interrupt, "iface:interrupt"),
    (close_session, "iface:closeSession"),
    (forget_session, "iface:forgetSession"),
    (respond, "iface:respond"),
    (set_model, "iface:setModel"),
    (set_effort, "iface:setEffort"),
    (set_fast_mode, "iface:setFastMode"),
    (set_permission_mode, "iface:setPermissionMode"),
    (archive, "iface:archive"),
    (rename_session, "iface:renameSession"),
    (delete_session, "iface:deleteSession"),
    (fork_session, "iface:forkSession"),
    (adopt_session, "iface:adoptSession"),
    (set_engine_archived, "iface:setEngineArchived"),
    (read_usage, "iface:readUsage"),
    (engine_ops, "iface:engineOps"),
    (read_history, "iface:readHistory"),
    (get_default_directory, "iface:getDefaultDirectory"),
    (pick_directory, "iface:pickDirectory"),
    (create_project, "iface:createProject"),
    (grant_prompt_context_paths, "iface:grantPromptContextPaths"),
    (save_pasted_images, "iface:savePastedImages"),
    (
        read_prompt_context_thumbnail,
        "iface:readPromptContextThumbnail"
    ),
    (read_file, "iface:readFile"),
    (read_file_snapshot, "iface:readFileSnapshot"),
    (preview_output, "iface:previewOutput"),
    (save_file, "iface:saveFile"),
    (set_workspace_open, "iface:setWorkspaceOpen"),
    (list_dir, "iface:listDir"),
    (list_project_files, "iface:listProjectFiles"),
    (search_project, "iface:searchProject"),
    (search_sessions, "iface:searchSessions"),
    (open_terminal, "iface:openTerminal"),
    (open_workspace_tool, "iface:openWorkspaceTool"),
    (
        list_terminal_launch_profiles,
        "iface:listTerminalLaunchProfiles"
    ),
    (list_terminals, "iface:listTerminals"),
    (read_terminal_delegation, "iface:readTerminalDelegation"),
    (advance_terminal_controls, "iface:advanceTerminalControls"),
    (create_terminal, "iface:createTerminal"),
    (rename_terminal, "iface:renameTerminal"),
    (resize_terminal, "iface:resizeTerminal"),
    (
        set_terminal_workspace_activity,
        "iface:setTerminalWorkspaceActivity"
    ),
    (restart_terminal, "iface:restartTerminal"),
    (close_terminal, "iface:closeTerminal"),
    (dismiss_terminal, "iface:dismissTerminal"),
    (list_managed_worktrees, "iface:listManagedWorktrees"),
    (open_managed_worktree, "iface:openManagedWorktree"),
    (cleanup_managed_worktree, "iface:cleanupManagedWorktree"),
    (read_repository_review, "iface:readRepositoryReview"),
    (discover_review_checks, "iface:discoverReviewChecks"),
    (list_customization, "iface:listCustomization"),
    (background_control, "iface:backgroundControl"),
    (customization_control, "iface:customizationControl"),
    (run_review_checks, "iface:runReviewChecks"),
    (create_review_checkpoint, "iface:createReviewCheckpoint"),
    (list_review_checkpoints, "iface:listReviewCheckpoints"),
    (restore_review_checkpoint, "iface:restoreReviewCheckpoint"),
    (validate_prompt_context, "iface:validatePromptContext"),
    (probe_engines, "iface:probeEngines"),
    (diagnostics, "iface:diagnostics"),
    (remote_access, "iface:remoteAccess"),
    (voice_set_enabled, "iface:voiceSetEnabled"),
    (voice_control, "iface:voiceControl"),
    (project_away, "iface:projectAway"),
    (voice_interrupt, "iface:voiceInterrupt"),
    (voice_ui_result, "iface:voiceUiResult"),
    (event, "iface:event"),
    (registry, "iface:registry"),
    (update_state, "iface:updateState"),
    (collaboration_event, "iface:collaborationEvent"),
    (orchestration_event, "iface:orchestrationEvent"),
    (terminal_lifecycle, "iface:terminalLifecycle"),
    (voice, "iface:voice"),
    (voice_ui_call, "iface:voiceUiCall"),
    (appearance_changed, "iface:appearanceChanged"),
    (threads, "iface:threads"),
);

/// Channels owned by the desktop bridge rather than the shared engine map.
///
/// Keep this second namespace explicit: the legacy Electron preload freezes
/// these four values separately, and the host must still validate the combined
/// versioned desktop surface without pretending they are engine operations.
pub const EXTRA_CHANNELS: &[IpcChannel] = &[
    IpcChannel {
        name: "adoptAll",
        value: "iface:adoptAll",
    },
    IpcChannel {
        name: "sendLive",
        value: "iface:sendLive",
    },
    IpcChannel {
        name: "importProgress",
        value: "iface:importProgress",
    },
    IpcChannel {
        name: "worktree",
        value: "iface:worktree",
    },
];

pub const ALL_DESKTOP_CHANNEL_COUNT: usize = ALL_CHANNELS.len() + EXTRA_CHANNELS.len();

#[must_use]
pub fn is_desktop_channel(value: &str) -> bool {
    is_shared_channel(value) || EXTRA_CHANNELS.iter().any(|channel| channel.value == value)
}

/// Splits a serialized session key at its first provider separator.
///
/// # Errors
///
/// Returns an error when the engine prefix or provider session id is invalid.
pub fn parse_key(key: &str) -> Result<(EngineId, String), ValidationError> {
    let (engine, session_id) = key
        .split_once('#')
        .ok_or_else(|| ValidationError::new("sessionKey", "missing # separator"))?;
    Ok((engine.parse()?, session_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::engine::{EngineEventKind, session_key};
    use serde_json::json;

    #[test]
    fn all_channel_identifiers_are_unique_and_prefixed() {
        assert_eq!(
            ALL_CHANNELS
                .iter()
                .map(|channel| channel.value)
                .collect::<HashSet<_>>()
                .len(),
            ALL_CHANNELS.len()
        );
        assert!(
            ALL_CHANNELS
                .iter()
                .all(|channel| channel.value.starts_with("iface:"))
        );
        assert_eq!(
            CHANNELS.integrate_coordinated_worker,
            "iface:integrateCoordinatedWorker"
        );
        assert!(
            !ALL_CHANNELS
                .iter()
                .any(|channel| channel.value.contains("Teammate"))
        );
    }

    #[test]
    fn shared_and_bridge_owned_channels_form_the_frozen_desktop_surface() {
        let values = ALL_CHANNELS
            .iter()
            .chain(EXTRA_CHANNELS)
            .map(|channel| channel.value)
            .collect::<HashSet<_>>();
        assert_eq!(ALL_CHANNELS.len(), 135);
        assert_eq!(EXTRA_CHANNELS.len(), 4);
        assert_eq!(ALL_DESKTOP_CHANNEL_COUNT, 139);
        assert_eq!(values.len(), ALL_DESKTOP_CHANNEL_COUNT);
        assert!(values.iter().all(|value| value.starts_with("iface:")));
        assert!(is_desktop_channel("iface:adoptAll"));
        assert!(is_desktop_channel("iface:worktree"));
        assert!(is_desktop_channel("iface:createTerminal"));
        assert!(is_desktop_channel("iface:listTerminalLaunchProfiles"));
        assert!(is_desktop_channel("iface:terminalLifecycle"));
        assert!(is_desktop_channel("iface:voiceSetEnabled"));
        assert!(is_desktop_channel("iface:voiceInterrupt"));
        assert!(is_desktop_channel("iface:voiceUiResult"));
        assert!(is_desktop_channel("iface:voice"));
        assert!(is_desktop_channel("iface:voiceUiCall"));
        assert!(is_desktop_channel("iface:threadControl"));
        assert!(is_desktop_channel("iface:threadJournal"));
        assert!(is_desktop_channel("iface:providerAccounts"));
        assert!(is_desktop_channel("iface:draftStash"));
        assert!(is_desktop_channel("iface:threadSecret"));
        assert!(is_desktop_channel("iface:threads"));
        assert!(!is_desktop_channel("iface:notReal"));
        assert!(is_shared_channel("iface:createTerminal"));
        assert!(!is_shared_channel("iface:adoptAll"));
        assert!(!is_shared_channel("iface:notReal"));
    }

    #[test]
    fn parse_key_splits_only_the_first_separator() {
        let engine: EngineId = "acp:grok".parse().unwrap();
        let key = session_key(&engine, "thread#branch");
        assert_eq!(parse_key(&key).unwrap(), (engine, "thread#branch".into()));
        assert!(parse_key("missing").is_err());
    }

    #[test]
    fn history_limit_defaults_clamps_and_floors() {
        assert_eq!(history_read_limit(None), HISTORY_LIMIT);
        assert_eq!(history_read_limit(Some(12.0)), HISTORY_LIMIT);
        assert_eq!(history_read_limit(Some(750.9)), 750);
        assert_eq!(history_read_limit(Some(50_000.0)), HISTORY_MAX_LIMIT);
    }

    #[test]
    fn prompt_context_validation_golden_json() {
        let valid = PromptContextValidation {
            ok: true,
            items: vec![PromptContextDescriptor {
                id: "one".into(),
                kind: PromptContextDescriptorKind::Url,
                label: "Site".into(),
                value: "https://example.com/".into(),
                detail: None,
                estimated_tokens: Some(12),
                safe_content: None,
            }],
            invalid: Vec::new(),
        };
        let invalid = PromptContextValidation {
            ok: false,
            items: Vec::new(),
            invalid: vec![
                InvalidPromptContextDescriptor {
                    index: Some(1),
                    id: Some("two".into()),
                    kind: Some("url".into()),
                    reason: PromptContextInvalidReason::InvalidUrl,
                    message: "Only complete http:// or https:// URLs can be attached.".into(),
                },
                InvalidPromptContextDescriptor {
                    index: None,
                    id: None,
                    kind: None,
                    reason: PromptContextInvalidReason::ItemLimit,
                    message: "Prompt context may contain at most 8 items.".into(),
                },
            ],
        };
        let valid_json = r#"{"ok":true,"items":[{"id":"one","kind":"url","label":"Site","value":"https://example.com/","estimatedTokens":12}],"invalid":[]}"#;
        let invalid_json = concat!(
            r#"{"ok":false,"items":[],"invalid":["#,
            r#"{"index":1,"id":"two","kind":"url","reason":"invalid-url","message":"Only complete http:// or https:// URLs can be attached."},"#,
            r#"{"index":null,"reason":"item-limit","message":"Prompt context may contain at most 8 items."}]}"#
        );
        assert_eq!(serde_json::to_string(&valid).unwrap(), valid_json);
        assert_eq!(serde_json::to_string(&invalid).unwrap(), invalid_json);
        for json in [valid_json, invalid_json] {
            let decoded: PromptContextValidation = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&decoded).unwrap(), json);
        }
        // Decoding ignores unknown keys, keeps `ok` as sent and refuses a
        // missing field.
        let decoded: PromptContextValidation =
            serde_json::from_str(r#"{"ok":false,"items":[],"invalid":[],"extra":1}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            r#"{"ok":false,"items":[],"invalid":[]}"#
        );
        assert!(
            serde_json::from_str::<PromptContextValidation>(r#"{"ok":true,"items":[]}"#).is_err()
        );
    }

    #[test]
    fn history_golden_json_retains_older_message_count() {
        let page = HistoryPage {
            availability: None,
            reason: None,
            events: vec![EngineEvent {
                at: None,
                event: EngineEventKind::SessionClosed,
            }],
            messages: Some(500),
            older_messages: 853,
        };
        assert_eq!(
            serde_json::to_value(page).unwrap(),
            json!({
                "events":[{"kind":"session.closed"}],"messages":500,"olderMessages":853
            })
        );
    }

    #[test]
    fn orchestration_agent_requests_accept_both_agent_id_spellings() {
        // The app sends `agentId`; `iface` builds before the fix sent `agent_id`.
        for value in [json!({"agentId": "a"}), json!({"agent_id": "a"})] {
            let stop: OrchestrationAgentStopRequest = serde_json::from_value(value).unwrap();
            assert_eq!(stop.agent_id, "a");
        }
        for value in [
            json!({"agentId": "a", "lines": 5}),
            json!({"agent_id": "a", "lines": 5}),
        ] {
            let transcript: OrchestrationTranscriptRequest = serde_json::from_value(value).unwrap();
            assert_eq!((transcript.agent_id.as_str(), transcript.lines), ("a", 5));
        }
        assert_eq!(
            serde_json::to_value(OrchestrationAgentStopRequest {
                agent_id: "a".into()
            })
            .unwrap(),
            json!({"agentId": "a"})
        );
        assert!(
            serde_json::from_value::<OrchestrationAgentStopRequest>(
                json!({"agentId": "a", "x": 1})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<OrchestrationTranscriptRequest>(
                json!({"agentId": "a", "lines": 5, "x": 1})
            )
            .is_err()
        );
    }
}
