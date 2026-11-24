use super::{DesktopService, encode};
use crate::{DesktopServiceError, payload::decode};
use interface_contracts::customization::CustomizationCommand;
use interface_workspace::{
    CustomizationRoots, check_mcp, provider_inventory, revalidate_customization_source,
};
use serde_json::Value;
use std::path::PathBuf;

impl DesktopService {
    /// Startup-only provider homes, resolved by the same host that launches providers.
    /// # Errors
    /// Refuses poisoned state.
    pub fn configure_customization_roots(
        &self,
        roots: CustomizationRoots,
    ) -> Result<(), DesktopServiceError> {
        *self
            .customization_roots
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())? = roots;
        Ok(())
    }

    pub(super) fn customization_control(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let command = decode::<CustomizationCommand>(operation, payload)?;
        let (requested_root, provider) = match &command {
            CustomizationCommand::Read { root, provider }
            | CustomizationCommand::Source { root, provider, .. }
            | CustomizationCommand::CheckMcp { root, provider, .. } => (root, *provider),
        };
        let root = self.exact_root(
            requested_root,
            session_roots,
            (
                "Could not read provider customization.",
                "Choose an open project before inspecting its customization.",
            ),
            "read provider customization",
        )?;
        let roots = self
            .customization_roots
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?
            .clone();
        let mut result = provider_inventory(&root, &roots, provider);
        match command {
            CustomizationCommand::Read { .. } => {}
            CustomizationCommand::Source { path, .. } => {
                result.source_path = Some(
                    revalidate_customization_source(&result, &path)
                        .map_err(|message| {
                            DesktopServiceError::refused("Could not reveal this source.", message)
                        })?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            CustomizationCommand::CheckMcp { path, name, .. } => {
                result.health = Some(check_mcp(&result, &path, &name));
            }
        }
        encode(operation, result)
    }
}
