//! Durable annotations bind to immutable, host-read output versions.
use crate::{
    ValidationError,
    output_preview::{OutputPreviewAction, OutputPreviewRequest, OutputPreviewSnapshot},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OutputAnchor {
    File,
    Text {
        quote: String,
        #[serde(rename = "startLine", default, skip_serializing_if = "Option::is_none")]
        start_line: Option<u32>,
        #[serde(rename = "endLine", default, skip_serializing_if = "Option::is_none")]
        end_line: Option<u32>,
    },
    Page {
        page: u32,
    },
    Region {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
}
impl OutputAnchor {
    #[must_use]
    pub fn valid(&self) -> bool {
        match self {
            Self::File => true,
            Self::Text {
                quote,
                start_line,
                end_line,
            } => {
                !quote.is_empty()
                    && quote.len() <= 8000
                    && match (start_line, end_line) {
                        (None, None) => true,
                        (Some(a), Some(b)) => *a > 0 && a <= b && *b <= 10_000,
                        _ => false,
                    }
            }
            Self::Page { page } => *page > 0 && *page <= 100_000,
            Self::Region {
                x,
                y,
                width,
                height,
            } => {
                [*x, *y, *width, *height]
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                    && x + width <= 1.0001
                    && y + height <= 1.0001
            }
        }
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputReview {
    pub id: String,
    pub revision: u64,
    pub snapshot: OutputPreviewSnapshot,
    pub anchor: OutputAnchor,
    pub note: String,
    pub thread_key: Option<String>,
    pub task: String,
    pub updated_at: u64,
    pub delivery: Option<OutputFeedbackDelivery>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputFeedbackDelivery {
    Sending,
    Sent,
    Unknown,
    Unsent,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputReviewRequest {
    pub version: u8,
    pub root: String,
    #[serde(flatten)]
    pub command: OutputReviewCommand,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum OutputReviewCommand {
    ListReviews,
    CaptureReview {
        path: String,
    },
    SaveReview {
        id: String,
        revision: u64,
        anchor: OutputAnchor,
        note: String,
        #[serde(rename = "threadKey")]
        thread_key: Option<String>,
        task: String,
    },
    DeleteReview {
        id: String,
        revision: u64,
    },
    ReserveFeedback {
        id: String,
        revision: u64,
    },
    SettleFeedback {
        id: String,
        revision: u64,
        delivery: OutputFeedbackDelivery,
    },
}
impl OutputReviewRequest {
    /// # Errors
    /// Refuses unknown versions and invalid project roots.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.version != 2 {
            return Err(ValidationError::new("version", "expected 2"));
        }
        OutputPreviewRequest {
            version: 1,
            action: OutputPreviewAction::List,
            root: self.root.clone(),
            path: String::new(),
        }
        .validate()?;
        match &self.command {
            OutputReviewCommand::CaptureReview { path } => OutputPreviewRequest {
                version: 1,
                action: OutputPreviewAction::Read,
                root: self.root.clone(),
                path: path.clone(),
            }
            .validate(),
            OutputReviewCommand::SaveReview {
                id,
                revision,
                anchor,
                note,
                thread_key,
                task,
            } if valid_id(id)
                && *revision > 0
                && anchor.valid()
                && note.len() <= 32_000
                && task.len() <= 512
                && thread_key.as_ref().is_none_or(|k| {
                    !k.is_empty() && k.len() <= 512 && !k.chars().any(char::is_control)
                }) =>
            {
                Ok(())
            }
            OutputReviewCommand::DeleteReview { id, revision }
            | OutputReviewCommand::ReserveFeedback { id, revision }
            | OutputReviewCommand::SettleFeedback { id, revision, .. }
                if valid_id(id) && *revision > 0 =>
            {
                Ok(())
            }
            OutputReviewCommand::ListReviews => Ok(()),
            _ => Err(ValidationError::new(
                "review",
                "invalid or oversized output review",
            )),
        }
    }
}
#[must_use]
pub fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OutputReviewResponse {
    Reviews {
        version: u8,
        reviews: Vec<OutputReview>,
    },
    Review {
        version: u8,
        review: Box<OutputReview>,
    },
    DeletedReview {
        version: u8,
        id: String,
    },
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum OutputPreviewControlRequest {
    Inspect(OutputPreviewRequest),
    Review(OutputReviewRequest),
}
impl OutputPreviewControlRequest {
    /// # Errors
    /// Refuses invalid inspection or review requests.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Inspect(r) => r.validate(),
            Self::Review(r) => r.validate(),
        }
    }
}

impl<'de> Deserialize<'de> for OutputReviewRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            version: u8,
            root: String,
            #[serde(flatten)]
            command: OutputReviewCommand,
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        let wire: Wire = serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
        let encoded = serde_json::to_value(&wire).map_err(serde::de::Error::custom)?;
        if value
            .as_object()
            .is_none_or(|object| object.keys().any(|key| encoded.get(key).is_none()))
        {
            return Err(serde::de::Error::custom("unknown request field"));
        }
        Ok(Self {
            version: wire.version,
            root: wire.root,
            command: wire.command,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_reviews_v49_fixture_round_trips_and_rejects_unknown_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/output-reviews-v49.json"
        ))
        .unwrap();
        for value in fixture["requests"].as_array().unwrap() {
            let request: OutputPreviewControlRequest =
                serde_json::from_value(value.clone()).unwrap();
            request.validate().unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
            let mut bad = value.clone();
            bad["grant"] = serde_json::json!(true);
            assert!(serde_json::from_value::<OutputPreviewControlRequest>(bad).is_err());
        }
        for value in fixture["responses"].as_array().unwrap() {
            let response: OutputReviewResponse = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), *value);
        }
    }
}
