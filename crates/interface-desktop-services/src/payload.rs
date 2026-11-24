use std::collections::BTreeSet;

use interface_contracts::{
    appearance::AppearanceChange, ipc::AcpProfile, terminal::TerminalWorkspaceRecord,
};
use interface_settings::{
    ACP_PROFILE_LIMITS, SettingsError, validate_acp_argument_bounds, validate_acp_display_name,
    validate_acp_executable_path, validate_acp_slug,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::DesktopServiceError;

const ACP_PROFILE_KEYS: [&str; 4] = ["slug", "displayName", "executablePath", "args"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GetAppearancePayload {
    pub system_dark: bool,
    #[serde(default)]
    pub surface_version: Option<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetAppearancePayload {
    pub preference: AppearanceChange,
    pub system_dark: bool,
    #[serde(default)]
    pub surface_version: Option<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetAcpProfilesPayload {
    pub profiles: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetTerminalWorkspacesPayload {
    pub workspaces: Vec<TerminalWorkspaceRecord>,
    /// Folders the owner confirmed have moved, carried in the same save that
    /// rewrites the roster so a saved pane's host proof moves with it.
    #[serde(default)]
    pub moved_folders: Vec<MovedFolderPayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MovedFolderPayload {
    pub from: String,
    pub to: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PathPayload {
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RootPayload {
    pub root: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PickDirectoryPayload {
    pub selected_path: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateProjectPayload {
    pub name: String,
}

/// `iface:createProject` carries the v1 create (`{name}`) or, from protocol
/// 40, a Projects folder request (`{projectsFolder, …}`).
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum CreateProjectRequest {
    Create(CreateProjectPayload),
    Folder(interface_contracts::projects::ProjectsFolderRequest),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectoryPickerStartPathPayload {
    pub preferred_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetWorkspaceOpenPayload {
    pub root: String,
    pub open: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateReviewCheckpointPayload {
    pub root: String,
    #[serde(default)]
    pub kind: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreReviewCheckpointPayload {
    pub root: String,
    pub id: String,
    #[serde(default)]
    pub paths: OptionalPaths,
}

#[derive(Default)]
pub(crate) enum OptionalPaths {
    #[default]
    Missing,
    Present(Value),
}

impl<'de> Deserialize<'de> for OptionalPaths {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveFilePayload {
    pub path: String,
    pub text: String,
    pub expected_revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchProjectPayload {
    pub root: String,
    pub query: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunReviewChecksPayload {
    pub root: String,
    pub selected: Vec<String>,
}

pub(crate) fn decode_empty(operation: &str, payload: &Value) -> Result<(), DesktopServiceError> {
    if payload.is_null() || payload.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(DesktopServiceError::invalid_payload(operation))
    }
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    operation: &str,
    payload: Value,
) -> Result<T, DesktopServiceError> {
    serde_json::from_value(payload).map_err(|_| DesktopServiceError::invalid_payload(operation))
}

fn invalid_acp(detail: impl Into<String>) -> DesktopServiceError {
    DesktopServiceError::new(
        "invalid-acp-profiles",
        format!("Invalid ACP profile configuration: {}", detail.into()),
        false,
    )
}

/// A settings field rule's refusal, with the same wire prose.
fn invalid_field(error: SettingsError) -> DesktopServiceError {
    match error {
        SettingsError::InvalidAcp(detail) => invalid_acp(detail),
        other => invalid_acp(other.to_string()),
    }
}

/// The wire matches credential words ASCII case-insensitively, as Electron
/// did. The store's regex on save also folds Unicode case, so it can only
/// refuse more; using it here would change which error a roster with several
/// problems reports.
fn credential_shaped(argument: &str) -> bool {
    let lower = argument.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("credential")
        || lower.contains("apikey")
        || lower.contains("api-key")
        || lower.contains("api_key")
}

fn parse_acp_profile(row: Value) -> Result<AcpProfile, DesktopServiceError> {
    let Value::Object(mut fields) = row else {
        return Err(invalid_acp("profile must be an object"));
    };
    if fields.len() != ACP_PROFILE_KEYS.len()
        || fields
            .keys()
            .any(|key| !ACP_PROFILE_KEYS.contains(&key.as_str()))
    {
        return Err(invalid_acp(
            "profile may contain only slug, displayName, executablePath, args",
        ));
    }

    let slug = fields
        .remove("slug")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| invalid_acp("slug must be 1-64 lowercase letters, digits, or hyphens"))?;
    validate_acp_slug(&slug).map_err(invalid_field)?;

    let display_name = fields
        .remove("displayName")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| invalid_acp("displayName must be a string"))?;
    let display_name = validate_acp_display_name(&display_name).map_err(invalid_field)?;

    let executable_path = fields
        .remove("executablePath")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| {
            invalid_acp("executablePath must name an absolute, non-shell executable path")
        })?;
    // Electron measured the path in UTF-16 units, which the character limit
    // the store applies cannot exceed.
    if executable_path.encode_utf16().count() > ACP_PROFILE_LIMITS.executable_path_characters {
        return Err(invalid_acp(
            "executablePath must name an absolute, non-shell executable path",
        ));
    }
    validate_acp_executable_path(&executable_path).map_err(invalid_field)?;

    let args = fields
        .remove("args")
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| invalid_acp("args must contain at most 64 exact argv entries"))?;
    if args.len() > ACP_PROFILE_LIMITS.args_per_profile {
        return Err(invalid_acp(
            "args must contain at most 64 exact argv entries",
        ));
    }
    let mut parsed_args = Vec::with_capacity(args.len());
    for argument in args {
        let Some(argument) = argument.as_str() else {
            return Err(invalid_acp(
                "each argv entry must be a string of at most 1024 characters without NUL",
            ));
        };
        validate_acp_argument_bounds(argument).map_err(invalid_field)?;
        if credential_shaped(argument) {
            return Err(invalid_acp(
                "args cannot carry credential-shaped flags or values",
            ));
        }
        parsed_args.push(argument.to_owned());
    }

    Ok(AcpProfile {
        slug,
        display_name,
        executable_path,
        args: parsed_args,
    })
}

/// Mirrors the original raw Electron validation before the typed settings
/// store sees a profile. This retains its field-specific rejection prose.
pub(crate) fn parse_acp_profiles(value: Value) -> Result<Vec<AcpProfile>, DesktopServiceError> {
    let Value::Array(rows) = value else {
        return Err(invalid_acp("profiles must be an array"));
    };
    if rows.len() > ACP_PROFILE_LIMITS.profiles {
        return Err(invalid_acp("profiles must contain at most 64 entries"));
    }
    let mut profiles = Vec::with_capacity(rows.len());
    let mut slugs = BTreeSet::new();
    for row in rows {
        let profile = parse_acp_profile(row)?;
        if !slugs.insert(profile.slug.clone()) {
            return Err(invalid_acp(format!("duplicate slug \"{}\"", profile.slug)));
        }
        profiles.push(profile);
    }
    Ok(profiles)
}
