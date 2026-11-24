use super::{DesktopService, DesktopServiceError, agent_now_ms};
use interface_common::sync::lock;
use interface_contracts::{
    output_preview::{OutputPreviewAction, OutputPreviewRequest, OutputPreviewResponse},
    output_review::{OutputReviewCommand, OutputReviewRequest, OutputReviewResponse},
};
use std::path::PathBuf;
impl DesktopService {
    /// Review records never grant access: every request rechecks the current project authority.
    /// # Errors
    /// Refuses unauthorized roots, stale review revisions and unavailable storage.
    #[allow(clippy::too_many_lines)] // Each frozen review command delegates to its store operation.
    pub fn output_review(
        &self,
        request: &OutputReviewRequest,
        roots: &[PathBuf],
    ) -> Result<OutputReviewResponse, DesktopServiceError> {
        request
            .validate()
            .map_err(|e| DesktopServiceError::refused("Output review refused", &e.to_string()))?;
        interface_workspace::output_preview::read_output_preview(
            &OutputPreviewRequest {
                version: 1,
                action: OutputPreviewAction::List,
                root: request.root.clone(),
                path: String::new(),
            },
            roots,
        )
        .map_err(|e| DesktopServiceError::refused("Output review refused", &e.to_string()))?;
        let mut guard = lock(&self.output_reviews);
        let store = guard.as_mut().ok_or_else(|| {
            DesktopServiceError::refused(
                "Output reviews unavailable",
                "Existing drafts were retained. Restart before saving.",
            )
        })?;
        let map = |e: interface_settings::SettingsError| {
            DesktopServiceError::new("output-review-unavailable", e.to_string(), false)
                .outcome_unknown()
        };
        match &request.command {
            OutputReviewCommand::ListReviews => Ok(OutputReviewResponse::Reviews {
                version: 2,
                reviews: store.list(&request.root),
            }),
            OutputReviewCommand::CaptureReview { path } => {
                let read = interface_workspace::output_preview::read_output_preview(
                    &OutputPreviewRequest {
                        version: 1,
                        action: OutputPreviewAction::Read,
                        root: request.root.clone(),
                        path: path.clone(),
                    },
                    roots,
                )
                .map_err(|e| {
                    DesktopServiceError::refused("Output review refused", &e.to_string())
                })?;
                let OutputPreviewResponse::File { snapshot, .. } = read else {
                    return Err(DesktopServiceError::refused(
                        "Output review refused",
                        "Expected a file snapshot.",
                    ));
                };
                Ok(OutputReviewResponse::Review {
                    version: 2,
                    review: Box::new(store.capture(snapshot, agent_now_ms()).map_err(map)?),
                })
            }
            OutputReviewCommand::SaveReview {
                id,
                revision,
                anchor,
                note,
                thread_key,
                task,
            } => Ok(OutputReviewResponse::Review {
                version: 2,
                review: Box::new(
                    store
                        .save(
                            &request.root,
                            id,
                            *revision,
                            anchor.clone(),
                            note.clone(),
                            thread_key.clone(),
                            task.clone(),
                            agent_now_ms(),
                        )
                        .map_err(map)?,
                ),
            }),
            OutputReviewCommand::ReserveFeedback { id, revision } => {
                Ok(OutputReviewResponse::Review {
                    version: 2,
                    review: Box::new(
                        store
                            .feedback(&request.root, id, *revision, None)
                            .map_err(map)?,
                    ),
                })
            }
            OutputReviewCommand::SettleFeedback {
                id,
                revision,
                delivery,
            } => Ok(OutputReviewResponse::Review {
                version: 2,
                review: Box::new(
                    store
                        .feedback(&request.root, id, *revision, Some(*delivery))
                        .map_err(map)?,
                ),
            }),
            OutputReviewCommand::DeleteReview { id, revision } => {
                store.delete(&request.root, id, *revision).map_err(map)?;
                Ok(OutputReviewResponse::DeletedReview {
                    version: 2,
                    id: id.clone(),
                })
            }
        }
    }
}
