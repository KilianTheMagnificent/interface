//! Independent private identity/configuration store for the v5 phone transport.
use crate::private_path::{SettingsError, StoreLocation, prepare_store};
use interface_contracts::remote_connection::{ConnectionConfigurationV1, ConnectionListenerV1};
use interface_storage::{JsonLimits, WriterLock, atomic_write_json, read_bounded, snapshot_target};
use std::path::Path;
const LIMIT: usize = 4096;

pub struct PhoneConnectionStore {
    location: StoreLocation,
    lease: WriterLock,
}
impl PhoneConnectionStore {
    /// # Errors
    /// Malformed or unsafe listener state must not silently change the saved port.
    pub fn load_listener(&self) -> Result<Option<ConnectionListenerV1>, SettingsError> {
        let Some(file) = read_bounded(&self.location.directory, "phone-listener.v1.json", LIMIT)?
        else {
            return Ok(None);
        };
        let value: ConnectionListenerV1 = serde_json::from_slice(file.bytes())
            .map_err(|_| SettingsError::UnsafeStorage("invalid phone listener"))?;
        if !value.valid() {
            return Err(SettingsError::UnsafeStorage("invalid phone listener"));
        }
        Ok(Some(value))
    }
    /// # Errors
    /// Uses the identity store's existing exclusive writer lease.
    pub fn save_listener(&self, value: &ConnectionListenerV1) -> Result<(), SettingsError> {
        if !value.valid() {
            return Err(SettingsError::UnsafeStorage("invalid phone listener"));
        }
        let name = "phone-listener.v1.json";
        let expected = snapshot_target(&self.location.directory, name, LIMIT)?;
        atomic_write_json(
            &self.location.directory,
            &self.lease,
            name,
            value,
            &expected,
            JsonLimits::new(LIMIT, 8, 128)?,
        )?;
        Ok(())
    }
    /// # Errors
    /// Refuses unsafe storage or another writer.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let location = prepare_store(file)?;
        let lease = WriterLock::acquire(&location.directory, ".phone-connection.lock")?;
        Ok(Self { location, lease })
    }
    /// # Errors
    /// Corruption never rotates identity or silently creates trust.
    pub fn load(&self) -> Result<Option<ConnectionConfigurationV1>, SettingsError> {
        let Some(file) = read_bounded(&self.location.directory, &self.location.name, LIMIT)? else {
            return Ok(None);
        };
        let value: ConnectionConfigurationV1 = serde_json::from_slice(file.bytes())
            .map_err(|_| SettingsError::UnsafeStorage("invalid phone configuration"))?;
        if !value.valid() {
            return Err(SettingsError::UnsafeStorage("invalid phone configuration"));
        }
        Ok(Some(value))
    }
    /// # Errors
    /// Caller must stop serving on failed or uncertain persistence.
    pub fn save(&self, value: &ConnectionConfigurationV1) -> Result<(), SettingsError> {
        if !value.valid() {
            return Err(SettingsError::UnsafeStorage("invalid phone configuration"));
        }
        let expected = snapshot_target(&self.location.directory, &self.location.name, LIMIT)?;
        atomic_write_json(
            &self.location.directory,
            &self.lease,
            &self.location.name,
            value,
            &expected,
            JsonLimits::new(LIMIT, 8, 128)?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn identity_is_private_single_writer_and_survives_reopen_without_rotation() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("phone-connection.v1.json");
        let store = PhoneConnectionStore::open(&path).unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(PhoneConnectionStore::open(&path).is_err());
        let fixture = ConnectionConfigurationV1 {
            version: 1,
            enabled: true,
            private_key: "01".repeat(32),
            relay_origin: Some("https://relay.example.com".into()),
        };
        store.save(&fixture).unwrap();
        let original_identity = fs::read(&path).unwrap();
        assert!(store.load_listener().unwrap().is_none());
        store
            .save_listener(&ConnectionListenerV1 {
                version: 1,
                port: 58747,
            })
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original_identity);
        drop(store);
        let store = PhoneConnectionStore::open(&path).unwrap();
        assert_eq!(store.load_listener().unwrap().unwrap().port, 58747);
        assert!(
            store
                .save_listener(&ConnectionListenerV1 {
                    version: 2,
                    port: 58747
                })
                .is_err()
        );
        assert!(
            store
                .save_listener(&ConnectionListenerV1 {
                    version: 1,
                    port: 80
                })
                .is_err()
        );
        assert_eq!(
            store.load().unwrap().unwrap().private_key,
            fixture.private_key
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::write(path, b"malformed").unwrap();
        assert!(store.load().is_err());
        fs::write(
            temporary.path().join("phone-listener.v1.json"),
            b"malformed",
        )
        .unwrap();
        assert!(store.load_listener().is_err());
    }
}
