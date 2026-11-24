use super::{DesktopService, DesktopServiceError, agent_now_ms};
use interface_common::sync::lock;
use interface_contracts::recovery::{
    PromptDelivery, PromptRecoveryInput, PromptRecoveryReceipt, RecoveryCommand, RecoveryRequest,
    RecoveryResponse,
};
use std::path::PathBuf;
fn failure(error: &interface_settings::SettingsError) -> DesktopServiceError {
    DesktopServiceError::new("prompt-recovery-unavailable", error.to_string(), false)
        .outcome_unknown()
}
impl DesktopService {
    #[must_use]
    pub fn prompt_retention_ready(&self) -> bool {
        lock(&self.prompt_recovery).is_some()
    }
    /// Session `key`'s latest prompt-recovery receipt (Resume at reset reads
    /// it to find a prompt its usage limit refused); `None` without one or
    /// without the store.
    #[must_use]
    pub fn prompt_recovery_receipt(&self, key: &str) -> Option<PromptRecoveryReceipt> {
        lock(&self.prompt_recovery).as_ref()?.read(key)
    }
    pub(super) fn recovery_command(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, DesktopServiceError> {
        let request: RecoveryRequest = serde_json::from_value(payload)
            .map_err(|_| DesktopServiceError::invalid_payload("recoveryControl"))?;
        if request.version != 1 || request.key.is_empty() || request.key.len() > 512 {
            return Err(DesktopServiceError::invalid_payload("recoveryControl"));
        }
        let mut guard = lock(&self.prompt_recovery);
        let store = guard
            .as_mut()
            .ok_or_else(DesktopServiceError::state_unavailable)?;
        let pending = if matches!(request.command, RecoveryCommand::ListPending) {
            store.pending()
        } else {
            Vec::new()
        };
        let prepared = match request.command {
            RecoveryCommand::Read | RecoveryCommand::ListPending => None,
            RecoveryCommand::RetainDrafts { paths } => {
                store.retain_drafts(paths).map_err(|e| failure(&e))?;
                None
            }
            RecoveryCommand::AcknowledgeLaunch { id } => {
                store
                    .acknowledge_launch(&request.key, &id)
                    .map_err(|e| failure(&e))?;
                None
            }
            RecoveryCommand::Prepare { id, mode } => Some(
                store
                    .prepare(&request.key, &id, mode)
                    .map_err(|e| failure(&e))?,
            ),
            RecoveryCommand::Forget { id } => {
                store.forget(&request.key, &id).map_err(|e| failure(&e))?;
                None
            }
        };
        serde_json::to_value(RecoveryResponse {
            version: 1,
            receipt: store.read(&request.key),
            prepared,
            pending,
        })
        .map_err(|_| DesktopServiceError::state_unavailable())
    }
    /// # Errors
    /// A receipt must persist before any prompt bytes can leave.
    pub fn reserve_prompt_recovery(
        &self,
        key: &str,
        input: PromptRecoveryInput,
        serialized: &str,
    ) -> Result<String, DesktopServiceError> {
        lock(&self.prompt_recovery)
            .as_mut()
            .ok_or_else(DesktopServiceError::state_unavailable)?
            .reserve(key, input, serialized, agent_now_ms())
            .map_err(|e| failure(&e))
    }
    /// # Errors
    /// A failed settle leaves the receipt uncertain.
    pub fn settle_prompt_recovery(
        &self,
        key: &str,
        id: &str,
        delivery: PromptDelivery,
    ) -> Result<(), DesktopServiceError> {
        lock(&self.prompt_recovery)
            .as_mut()
            .ok_or_else(DesktopServiceError::state_unavailable)?
            .settle(key, id, delivery)
            .map_err(|e| failure(&e))
    }
    /// # Errors
    /// Refuses a duplicate uncertain launch before provider creation.
    pub fn reserve_launch_recovery(
        &self,
        engine: &str,
        cwd: &str,
        input: PromptRecoveryInput,
        serialized: &str,
    ) -> Result<(String, String), DesktopServiceError> {
        let scope = if cwd.is_empty() {
            "no-project".to_owned()
        } else {
            interface_registry::launch_recovery_scope(std::path::Path::new(cwd)).map_err(
                |message| DesktopServiceError::new("prompt-recovery-unavailable", message, false),
            )?
        };
        let identity = format!("{engine}\0{scope}");
        lock(&self.prompt_recovery)
            .as_mut()
            .ok_or_else(DesktopServiceError::state_unavailable)?
            .reserve_launch(&identity, input, serialized, agent_now_ms())
            .map_err(|e| failure(&e))
    }
    /// # Errors
    /// The pending receipt remains uncertain if binding cannot be persisted.
    pub fn bind_launch_recovery(
        &self,
        pending: &str,
        id: &str,
        key: &str,
    ) -> Result<(), DesktopServiceError> {
        lock(&self.prompt_recovery)
            .as_mut()
            .ok_or_else(DesktopServiceError::state_unavailable)?
            .bind_launch(pending, id, key)
            .map_err(|e| failure(&e))
    }
    #[must_use]
    pub fn prompt_recovery(&self, key: &str) -> Option<PromptRecoveryReceipt> {
        lock(&self.prompt_recovery)
            .as_ref()
            .and_then(|s| s.read(key))
    }
    #[must_use]
    pub fn retained_prompt_paths(&self) -> Vec<PathBuf> {
        lock(&self.prompt_recovery).as_ref().map_or_else(
            Vec::new,
            interface_settings::PromptRecoveryStore::retained_paths,
        )
    }
}
