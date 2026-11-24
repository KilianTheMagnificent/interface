use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use interface_contracts::ipc::AcpProfile;
use interface_storage::{JsonLimits, read_json};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::leased::write_document;
use crate::private_path::{SettingsError, map_document_write, prepare_store};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpProfileLimits {
    pub json_bytes: usize,
    pub profiles: usize,
    pub slug_characters: usize,
    pub display_name_characters: usize,
    pub executable_path_characters: usize,
    pub args_per_profile: usize,
    pub arg_characters: usize,
}

pub const ACP_PROFILE_LIMITS: AcpProfileLimits = AcpProfileLimits {
    json_bytes: 256 * 1024,
    profiles: 64,
    slug_characters: 64,
    display_name_characters: 80,
    executable_path_characters: 4_096,
    args_per_profile: 64,
    arg_characters: 1_024,
};

static SLUG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$").expect("static slug regex")
});
static CREDENTIAL_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:api[-_]?key|token|secret|password|credential)")
        .expect("static credential regex")
});
const SHELL_EXECUTABLES: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "csh", "tcsh", "fish", "nu", "env",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAcpProfiles {
    version: u8,
    profiles: Vec<AcpProfile>,
}

fn invalid(message: impl Into<String>) -> SettingsError {
    SettingsError::InvalidAcp(message.into())
}

/// Validates a profile slug: 1-64 lowercase letters, digits or inner hyphens,
/// and not a built-in provider's name.
///
/// # Errors
///
/// Refuses a malformed or reserved slug.
pub fn validate_acp_slug(slug: &str) -> Result<(), SettingsError> {
    // SLUG already bounds the slug to ACP_PROFILE_LIMITS.slug_characters.
    if !SLUG.is_match(slug) {
        return Err(invalid(
            "slug must be 1-64 lowercase letters, digits, or hyphens",
        ));
    }
    if matches!(slug, "grok" | "qwen") {
        return Err(invalid(format!(
            "slug {slug:?} is reserved for Interface's built-in ACP provider"
        )));
    }
    Ok(())
}

/// Trims a display name and validates it: 1-80 characters, no controls.
///
/// # Errors
///
/// Refuses an empty, over-long or control-bearing name.
pub fn validate_acp_display_name(display_name: &str) -> Result<String, SettingsError> {
    let display_name = display_name.trim();
    if display_name.is_empty()
        || display_name.chars().count() > ACP_PROFILE_LIMITS.display_name_characters
        || display_name
            .chars()
            .any(|character| character <= '\u{1f}' || character == '\u{7f}')
    {
        return Err(invalid("displayName must be 1-80 visible characters"));
    }
    Ok(display_name.to_owned())
}

/// Validates an absolute, non-shell executable path of at most 4,096
/// characters with no NUL, line break or trailing slash.
///
/// # Errors
///
/// Refuses any other executable path.
pub fn validate_acp_executable_path(executable_path: &str) -> Result<(), SettingsError> {
    let executable = Path::new(executable_path);
    let executable_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !executable.is_absolute()
        || executable_path.chars().count() > ACP_PROFILE_LIMITS.executable_path_characters
        || executable_path.contains(['\0', '\r', '\n'])
        || executable_path.ends_with('/')
        || SHELL_EXECUTABLES.contains(&executable_name.as_str())
    {
        return Err(invalid(
            "executablePath must name an absolute, non-shell executable path",
        ));
    }
    Ok(())
}

/// Validates one argv entry's bounds: at most 1,024 characters, no NUL.
/// Credential shape is checked separately by [`validate_acp_profile`].
///
/// # Errors
///
/// Refuses an over-long entry or one containing NUL.
pub fn validate_acp_argument_bounds(argument: &str) -> Result<(), SettingsError> {
    if argument.chars().count() > ACP_PROFILE_LIMITS.arg_characters || argument.contains('\0') {
        return Err(invalid(
            "each argv entry must be a string of at most 1024 characters without NUL",
        ));
    }
    Ok(())
}

/// Validates and normalizes one profile without filesystem access.
///
/// # Errors
///
/// Refuses reserved/invalid slugs, hidden controls, shell executables,
/// over-budget argv, NUL, and credential-shaped argv fields.
pub fn validate_acp_profile(profile: &AcpProfile) -> Result<AcpProfile, SettingsError> {
    validate_acp_slug(&profile.slug)?;
    let display_name = validate_acp_display_name(&profile.display_name)?;
    validate_acp_executable_path(&profile.executable_path)?;

    if profile.args.len() > ACP_PROFILE_LIMITS.args_per_profile {
        return Err(invalid("args must contain at most 64 exact argv entries"));
    }
    let mut args = Vec::with_capacity(profile.args.len());
    for argument in &profile.args {
        validate_acp_argument_bounds(argument)?;
        if CREDENTIAL_SHAPE.is_match(argument) {
            return Err(invalid(
                "args cannot carry credential-shaped flags or values",
            ));
        }
        args.push(argument.clone());
    }

    Ok(AcpProfile {
        slug: profile.slug.clone(),
        display_name,
        executable_path: profile.executable_path.clone(),
        args,
    })
}

/// Validates, deduplicates, and defensively clones a complete provider roster.
///
/// # Errors
///
/// Returns the first bounded-field or duplicate-slug error.
pub fn validate_acp_profiles(profiles: &[AcpProfile]) -> Result<Vec<AcpProfile>, SettingsError> {
    if profiles.len() > ACP_PROFILE_LIMITS.profiles {
        return Err(invalid("profiles must contain at most 64 entries"));
    }
    let mut slugs = BTreeSet::new();
    let mut validated = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let profile = validate_acp_profile(profile)?;
        if !slugs.insert(profile.slug.clone()) {
            return Err(invalid(format!("duplicate slug {:?}", profile.slug)));
        }
        validated.push(profile);
    }
    Ok(validated)
}

/// Owner-private version-1 ACP startup roster. Profiles become active only on
/// restart; this store deliberately has no process hot-swap behavior.
pub struct AcpProfileStore {
    location: crate::private_path::StoreLocation,
    profiles: Vec<AcpProfile>,
}

impl AcpProfileStore {
    /// # Errors
    ///
    /// Refuses relative, linked, hard-linked, non-regular, or unstable storage.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        Ok(Self {
            location: prepare_store(file)?,
            profiles: Vec::new(),
        })
    }

    #[must_use]
    pub fn get(&self) -> Vec<AcpProfile> {
        self.profiles.clone()
    }

    /// # Errors
    ///
    /// Returns an error for invalid/duplicate/bounded JSON or unsafe storage;
    /// the last in-memory roster remains unchanged on failure.
    pub fn load(&mut self) -> Result<Vec<AcpProfile>, SettingsError> {
        let limits = JsonLimits::new(ACP_PROFILE_LIMITS.json_bytes, 16, 10_000)?;
        let Some(loaded) =
            read_json::<StoredAcpProfiles>(&self.location.directory, &self.location.name, limits)?
        else {
            self.profiles.clear();
            return Ok(Vec::new());
        };
        if loaded.value().version != 1 {
            return Err(invalid("document.version must be 1"));
        }
        let validated = validate_acp_profiles(&loaded.value().profiles)?;
        self.profiles.clone_from(&validated);
        Ok(validated)
    }

    /// # Errors
    ///
    /// Validates the complete snapshot before touching disk, then refuses
    /// drift, concurrent writers, unsafe targets, and uncertain durability.
    pub fn save(&mut self, profiles: &[AcpProfile]) -> Result<Vec<AcpProfile>, SettingsError> {
        let profiles = validate_acp_profiles(profiles)?;
        let document = StoredAcpProfiles {
            version: 1,
            profiles: profiles.clone(),
        };
        write_document(
            &self.location,
            ".acp-profiles.lock",
            &document,
            JsonLimits::new(ACP_PROFILE_LIMITS.json_bytes, 16, 10_000)?,
            |error| map_document_write(error, SettingsError::InvalidAcp),
        )?;
        self.profiles.clone_from(&profiles);
        Ok(profiles)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use interface_contracts::ipc::AcpProfile;
    use tempfile::TempDir;

    use super::{ACP_PROFILE_LIMITS, AcpProfileStore, validate_acp_profile, validate_acp_profiles};

    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn profile(slug: &str) -> AcpProfile {
        AcpProfile {
            slug: slug.to_owned(),
            display_name: "Local Agent".to_owned(),
            executable_path: "/opt/local/bin/agent".to_owned(),
            args: vec!["acp".to_owned(), "stdio".to_owned()],
        }
    }

    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("acp-profiles.json");
        let mut store = AcpProfileStore::open(&file).unwrap();
        let mut second = profile("second-agent");
        second.args = vec!["--mode=\"quoted\"".to_owned(), String::new()];
        store.save(&[profile("local-agent"), second]).unwrap();
        crate::golden::assert_golden("acp-profiles.saved.json", temp.path(), &file);
        assert!(temp.path().join(".acp-profiles.lock").exists());
        let mut reopened = AcpProfileStore::open(&file).unwrap();
        assert_eq!(reopened.load().unwrap(), store.get());
    }

    #[test]
    fn validation_normalizes_only_label_and_preserves_exact_argv() {
        let mut input = profile("local-agent");
        input.display_name = "  Local ACP  ".to_owned();
        input.args = vec![
            "--flag=two words".to_owned(),
            String::new(),
            "line one\nline two".to_owned(),
        ];
        let validated = validate_acp_profile(&input).expect("validate");
        assert_eq!(validated.display_name, "Local ACP");
        assert_eq!(validated.args, input.args);
    }

    #[test]
    fn validation_refuses_reserved_invalid_shell_secret_and_duplicate_fields() {
        for slug in ["grok", "qwen", "Uppercase", "-leading", "trailing-", ""] {
            assert!(
                validate_acp_profile(&profile(slug)).is_err(),
                "accepted {slug}"
            );
        }
        let mut shell = profile("shell");
        shell.executable_path = "/bin/sh".to_owned();
        assert!(validate_acp_profile(&shell).is_err());
        let mut secret = profile("secret-argv");
        secret.args = vec!["--api-key".to_owned(), "placeholder".to_owned()];
        assert!(validate_acp_profile(&secret).is_err());
        assert!(validate_acp_profiles(&[profile("same"), profile("same")]).is_err());
        assert!(
            validate_acp_profiles(
                &(0..=ACP_PROFILE_LIMITS.profiles)
                    .map(|index| profile(&format!("p-{index}")))
                    .collect::<Vec<_>>()
            )
            .is_err()
        );
    }

    #[test]
    fn comparison_is_exact_including_order_and_argv() {
        let active = vec![profile("one"), profile("two")];
        assert_eq!(active, active.clone());
        let mut reversed = active.clone();
        reversed.reverse();
        assert_ne!(active, reversed);
    }

    #[test]
    fn store_saves_loads_and_keeps_last_memory_on_bad_disk() {
        let temp = TempDir::new().expect("temp");
        let file = temp.path().join("acp-profiles.json");
        let mut store = AcpProfileStore::open(&file).expect("open");
        assert!(store.load().expect("missing").is_empty());
        assert_eq!(
            store.save(&[profile("local")]).expect("save"),
            [profile("local")]
        );
        assert_eq!(store.get(), [profile("local")]);
        assert_eq!(
            AcpProfileStore::open(&file)
                .expect("reopen")
                .load()
                .expect("load"),
            [profile("local")]
        );

        fs::write(&file, "{broken").expect("break fixture");
        #[cfg(unix)]
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("file mode");
        assert!(store.load().is_err());
        assert_eq!(store.get(), [profile("local")]);
    }

    #[cfg(unix)]
    #[test]
    fn store_repairs_modes_and_refuses_linked_targets() {
        let temp = TempDir::new().expect("temp");
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).expect("dir mode");
        let file = temp.path().join("acp-profiles.json");
        let mut store = AcpProfileStore::open(&file).expect("open");
        store.save(&[profile("local")]).expect("save");
        assert_eq!(
            fs::metadata(temp.path())
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let other = TempDir::new().expect("other");
        let linked = other.path().join("acp-profiles.json");
        symlink(&file, &linked).expect("link");
        assert!(AcpProfileStore::open(&linked).is_err());
    }
}
