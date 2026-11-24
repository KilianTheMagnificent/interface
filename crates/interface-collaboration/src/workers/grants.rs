//! Explicit owner configuration. Never exposed as a model-callable grant tool.
use super::{Policy, digest};
use interface_storage::{
    JsonLimits, PrivateDirectory, TargetSnapshot, WriterLock, atomic_write_json, read_json,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Grant {
    schema_version: u32,
    source: std::path::PathBuf,
    files: BTreeSet<String>,
    contributor_data_allowed: bool,
}
fn limits() -> JsonLimits {
    JsonLimits::new(64 * 1024, 8, 256).expect("fixed grant limits")
}
fn name(source: &Path) -> String {
    format!("grant-{}.json", digest(&source))
}
fn validate(grant: &Grant) -> Result<(), String> {
    if grant.schema_version != 1
        || !grant.source.is_absolute()
        || grant.files.len() > 48
        || grant.files.iter().any(|f| {
            f.len() > 1024
                || f.is_empty()
                || Path::new(f).is_absolute()
                || Path::new(f)
                    .components()
                    .any(|p| !matches!(p, std::path::Component::Normal(_)))
        })
    {
        return Err("invalidRequest: invalid source grant".into());
    }
    Ok(())
}
/// Writes only an owner-requested grant; does not dispatch or capture any files.
pub fn write(root: &Path, policy: Policy) -> Result<(), String> {
    let _root = PrivateDirectory::open_or_create(root).map_err(|e| e.to_string())?;
    let directory =
        PrivateDirectory::open_or_create(root.join("grants")).map_err(|e| e.to_string())?;
    let writer = WriterLock::acquire(&directory, "grants.lock").map_err(|e| e.to_string())?;
    let grant = Grant {
        schema_version: 1,
        source: policy.source.canonicalize().map_err(|e| e.to_string())?,
        files: policy.files,
        contributor_data_allowed: policy.contributor_data_allowed,
    };
    validate(&grant)?;
    let name = name(&grant.source);
    let expected = read_json::<Grant>(&directory, &name, limits())
        .map_err(|e| e.to_string())?
        .map_or(TargetSnapshot::Missing, |r| {
            TargetSnapshot::File(r.into_parts().1)
        });
    atomic_write_json(&directory, &writer, &name, &grant, &expected, limits())
        .map(|_| ())
        .map_err(|e| e.to_string())
}
/// Missing grants deny sharing. An unreadable or malformed grant is an error.
pub fn read(root: &Path, source: &Path) -> Result<Policy, String> {
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let denied = Policy {
        source: source.clone(),
        files: BTreeSet::new(),
        contributor_data_allowed: false,
    };
    let path = root.join("grants");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(denied);
    }
    let directory = PrivateDirectory::open(path).map_err(|e| e.to_string())?;
    let Some(loaded) =
        read_json::<Grant>(&directory, &name(&source), limits()).map_err(|e| e.to_string())?
    else {
        return Ok(denied);
    };
    let (grant, _) = loaded.into_parts();
    validate(&grant)?;
    if grant.source != source {
        return Err("scopeDenied: grant source mismatch".into());
    }
    Ok(Policy {
        source,
        files: grant.files,
        contributor_data_allowed: grant.contributor_data_allowed,
    })
}

/// Admission rechecks an explicit host grant after an asynchronous packet capture.
pub fn assert_current(root: &Path, policy: &Policy) -> Result<(), String> {
    let current = read(root, &policy.source)?;
    if !current.contributor_data_allowed || !policy.files.is_subset(&current.files) {
        return Err("scopeDenied: source grant changed; definitely unsent".into());
    }
    Ok(())
}
