//! The ACP profile roster store and the get/set ACP profile operations.

use std::path::PathBuf;
use std::sync::MutexGuard;

use interface_contracts::ipc::AcpProfilesUpdate;
use interface_settings::{AcpProfileStore, SettingsError};
use serde_json::Value;

use super::{DesktopService, encode, map_settings_error};
use crate::DesktopServiceError;
use crate::payload::{SetAcpProfilesPayload, decode, decode_empty, parse_acp_profiles};

pub(super) enum RecoverableAcpProfileStore {
    Ready(AcpProfileStore),
    Unavailable(PathBuf),
}

impl RecoverableAcpProfileStore {
    fn get(&self) -> Vec<interface_contracts::ipc::AcpProfile> {
        match self {
            Self::Ready(store) => store.get(),
            Self::Unavailable(_) => Vec::new(),
        }
    }

    fn save(
        &mut self,
        profiles: &[interface_contracts::ipc::AcpProfile],
    ) -> Result<Vec<interface_contracts::ipc::AcpProfile>, SettingsError> {
        match self {
            Self::Ready(store) => store.save(profiles),
            Self::Unavailable(path) => {
                // Re-open on every Settings write so repairing/removing an
                // unsafe startup target lets the frozen UI replace it without
                // requiring an extra restart first.
                let mut store = AcpProfileStore::open(path)?;
                let saved = store.save(profiles)?;
                *self = Self::Ready(store);
                Ok(saved)
            }
        }
    }
}

impl DesktopService {
    /// Validated custom ACP roster captured at service startup. The host uses
    /// this defensive clone to register adapters before it announces ready.
    /// Later `setAcpProfiles` calls persist the next-startup roster but never
    /// mutate this snapshot or any live adapter.
    #[must_use]
    pub fn active_acp_profiles(&self) -> Vec<interface_contracts::ipc::AcpProfile> {
        self.active_acp_profiles.clone()
    }
    fn lock_acp_profiles(
        &self,
    ) -> Result<MutexGuard<'_, RecoverableAcpProfileStore>, DesktopServiceError> {
        self.acp_profiles
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())
    }
    pub(super) fn get_acp_profiles(
        &self,
        operation: &str,
        payload: &Value,
    ) -> Result<Value, DesktopServiceError> {
        decode_empty(operation, payload)?;
        encode(operation, self.lock_acp_profiles()?.get())
    }
    pub(super) fn set_acp_profiles(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<SetAcpProfilesPayload>(operation, payload)?;
        let requested_profiles = parse_acp_profiles(request.profiles)?;
        let profiles = self
            .lock_acp_profiles()?
            .save(&requested_profiles)
            .map_err(map_settings_error)?;
        let result = AcpProfilesUpdate {
            restart_required: self.active_acp_profiles != profiles,
            profiles,
        };
        encode(operation, result)
    }
}
