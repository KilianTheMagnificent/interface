use serde::{Deserialize, Serialize};

use crate::validation::validate_timestamp;

pub const MAX_UPDATE_VERSION_LENGTH: usize = 128;
pub const MAX_UPDATE_ERROR_LENGTH: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    Disabled,
    Idle,
    Checking,
    Available,
    Downloading,
    Ready,
    Installing,
    UpToDate,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSource {
    Local,
    Remote,
    None,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateState {
    pub phase: UpdatePhase,
    pub source: UpdateSource,
    pub current_version: String,
    pub available_version: Option<String>,
    pub progress: Option<f64>,
    pub last_checked_at: Option<String>,
    pub error: Option<String>,
    pub can_check: bool,
    pub can_restart: bool,
}

impl UpdateState {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        if self.current_version.is_empty() || self.current_version.len() > MAX_UPDATE_VERSION_LENGTH
        {
            return false;
        }
        if self
            .available_version
            .as_ref()
            .is_some_and(|version| version.is_empty() || version.len() > MAX_UPDATE_VERSION_LENGTH)
            || self
                .error
                .as_ref()
                .is_some_and(|error| error.is_empty() || error.len() > MAX_UPDATE_ERROR_LENGTH)
            || self
                .progress
                .is_some_and(|progress| !progress.is_finite() || !(0.0..=100.0).contains(&progress))
            || self
                .last_checked_at
                .as_ref()
                .is_some_and(|at| at.len() > 64 || validate_timestamp(at, "lastCheckedAt").is_err())
        {
            return false;
        }
        self.can_restart == matches!(self.phase, UpdatePhase::Ready)
    }
}

#[must_use]
pub fn initial_update_state(current_version: &str) -> UpdateState {
    let current_version =
        if current_version.is_empty() || current_version.len() > MAX_UPDATE_VERSION_LENGTH {
            "unknown".to_owned()
        } else {
            current_version.to_owned()
        };
    UpdateState {
        phase: UpdatePhase::Disabled,
        source: UpdateSource::None,
        current_version,
        available_version: None,
        progress: None,
        last_checked_at: None,
        error: None,
        can_check: false,
        can_restart: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn golden_update_snapshot_uses_the_ipc_names() {
        assert_eq!(
            serde_json::to_value(initial_update_state("0.2.0")).unwrap(),
            json!({
                "phase":"disabled", "source":"none", "currentVersion":"0.2.0",
                "availableVersion":null, "progress":null, "lastCheckedAt":null,
                "error":null, "canCheck":false, "canRestart":false
            })
        );
    }

    #[test]
    fn restart_is_true_only_for_ready() {
        let mut state = initial_update_state("0.2.0");
        state.phase = UpdatePhase::Ready;
        assert!(!state.is_valid());
        state.can_restart = true;
        assert!(state.is_valid());
        state.progress = Some(100.1);
        assert!(!state.is_valid());
    }
}
