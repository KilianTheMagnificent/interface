//! Bounded, read-only repository review and package-script checks.

mod checks;
pub mod diagnostics;
mod process;
mod repository;
mod safety;
pub mod thread_git;

pub use checks::{
    REVIEW_ACTION_LIMITS, ReviewActionLimits, ReviewCheckError, ReviewCheckInvocation,
    ReviewCheckRunOptions, ReviewCheckRunner, ReviewCheckRunnerResult, SystemReviewCheckRunner,
    discover_review_checks, run_review_checks,
};
pub use repository::{
    REPOSITORY_REVIEW_LIMITS, REVIEW_GIT_TIMED_OUT, REVIEW_GIT_TIMEOUT, REVIEW_GIT_UNAVAILABLE,
    RepositoryReviewHooks, RepositoryReviewLimits, RepositoryReviewOptions, ReviewGitResult,
    ReviewGitRunner, SystemReviewGitRunner, read_repository_review, read_repository_review_with,
};

pub mod workspace_setup;
