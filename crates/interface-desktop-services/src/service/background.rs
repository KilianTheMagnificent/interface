use super::{DesktopService, encode, map_settings_error};
use crate::{DesktopServiceError, payload::decode};
use interface_contracts::background::{BackgroundCommand, BackgroundState};
use serde_json::Value;

impl DesktopService {
    pub(super) fn background_control(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let command = decode::<BackgroundCommand>(operation, payload)?;
        let mut guard = self
            .background
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?;
        if let BackgroundCommand::Set { enabled } = command {
            guard
                .as_mut()
                .ok_or_else(DesktopServiceError::state_unavailable)?
                .set_enabled(enabled)
                .map_err(map_settings_error)?;
        }
        encode(
            operation,
            BackgroundState {
                enabled: guard
                    .as_ref()
                    .is_some_and(interface_settings::BackgroundStore::enabled),
                available: guard.is_some(),
            },
        )
    }
}
