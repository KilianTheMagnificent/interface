//! Lifetime writer lease and atomic, bounded storage for trusted companion devices.
use crate::private_path::{SettingsError, StoreLocation, prepare_store};
use interface_contracts::remote_trust::RemoteTrustData;
use interface_storage::{JsonLimits, WriterLock, atomic_write_json, read_bounded, snapshot_target};
use std::path::Path;
const LIMIT: usize = 16 * 1024;

pub struct RemoteTrustStore {
    location: StoreLocation,
    lease: WriterLock,
}
impl RemoteTrustStore {
    /// # Errors
    /// Refuses unsafe storage or a second writer.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let location = prepare_store(file)?;
        let lease = WriterLock::acquire(&location.directory, ".remote-trust.lock")?;
        Ok(Self { location, lease })
    }
    /// # Errors
    /// Malformed, unsupported or unreadable trust never defaults to trusted state.
    pub fn load(&self) -> Result<RemoteTrustData, SettingsError> {
        let Some(file) = read_bounded(&self.location.directory, &self.location.name, LIMIT)? else {
            return Ok(RemoteTrustData::default());
        };
        let value: RemoteTrustData = serde_json::from_slice(file.bytes())
            .map_err(|_| SettingsError::UnsafeStorage("invalid remote trust data"))?;
        if !value.valid() {
            return Err(SettingsError::UnsafeStorage("invalid remote trust data"));
        }
        Ok(value)
    }
    /// # Errors
    /// Refuses drift, invalid data, or unproven durability; caller must stop access on failure.
    pub fn save(&self, value: &RemoteTrustData) -> Result<(), SettingsError> {
        if !value.valid() {
            return Err(SettingsError::UnsafeStorage("invalid remote trust data"));
        }
        let expected = snapshot_target(&self.location.directory, &self.location.name, LIMIT)?;
        atomic_write_json(
            &self.location.directory,
            &self.lease,
            &self.location.name,
            value,
            &expected,
            JsonLimits::new(LIMIT, 8, 1024)?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::remote_trust::{RememberedListener, TrustedDeviceRecord};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    #[test]
    fn trust_reopens_with_exact_listener_and_has_one_private_writer() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("remote-trust.v1.json");
        let store = RemoteTrustStore::open(&file).unwrap();
        assert!(store.load().unwrap().devices.is_empty());
        assert!(RemoteTrustStore::open(&file).is_err());
        let data = RemoteTrustData {
            version: 1,
            listener: Some(RememberedListener {
                enabled: true,
                port: 4321,
                origin: "https://fixture.test.ts.net".into(),
            }),
            devices: vec![TrustedDeviceRecord {
                id: "12345678-1234-4234-8234-123456789abc".into(),
                name: "Fixture iPhone".into(),
                digest: [7; 32],
            }],
        };
        store.save(&data).unwrap();
        drop(store);
        let reopened = RemoteTrustStore::open(&file).unwrap();
        let loaded = reopened.load().unwrap();
        assert_eq!(loaded.listener.unwrap().port, 4321);
        assert_eq!(loaded.devices[0].digest, [7; 32]);
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(temp.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    #[test]
    fn malformed_unsupported_and_linked_trust_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("remote-trust.v1.json");
        fs::write(&file, b"invalid").unwrap();
        let store = RemoteTrustStore::open(&file).unwrap();
        assert!(store.load().is_err());
        fs::write(&file, br#"{"version":2,"listener":null,"devices":[]}"#).unwrap();
        assert!(store.load().is_err());
        drop(store);
        let link = temp.path().join("linked.json");
        symlink(&file, &link).unwrap();
        assert!(RemoteTrustStore::open(&link).is_err());
        assert_eq!(
            fs::read(&file).unwrap(),
            br#"{"version":2,"listener":null,"devices":[]}"#
        );
    }
}
