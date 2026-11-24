use std::{
    cmp::Ordering,
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use interface_common::paths::lexically_normalized;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use tempfile::Builder;

use crate::fsx::{
    FileState, checked_regular_file, open_unchanged, read_checked_file, validate_regular_metadata,
};

pub const MANIFEST_NAME: &str = "latest-mac.yml";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const TRANSACTION_PREFIX: &str = ".package-mac-transaction-";
const JOURNAL_NAME: &str = "transaction.json";
const BACKUP_COMPLETE_NAME: &str = "backup-complete";
const RELEASE_LOCK_NAME: &str = ".package-mac.lock";
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestArtifact {
    pub name: String,
    pub sha512: String,
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateManifest {
    pub version: String,
    pub primary_path: String,
    pub primary_sha512: String,
    pub files: Vec<ManifestArtifact>,
}

#[derive(Clone, Debug)]
pub struct SourceArtifact {
    pub manifest: ManifestArtifact,
    pub source_path: PathBuf,
    state: FileState,
}

impl SourceArtifact {
    /// Opens the artifact only while it is still the file inspection saw.
    fn open(&self) -> Result<File> {
        open_unchanged(
            &self.source_path,
            &self.state,
            &format!("artifact {}", self.manifest.name),
        )
    }
}

#[derive(Clone, Debug)]
pub struct UpdateSource {
    pub source_dir: PathBuf,
    pub manifest_bytes: Vec<u8>,
    pub manifest: UpdateManifest,
    pub artifacts: Vec<SourceArtifact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedFeed {
    pub target_dir: PathBuf,
    pub version: String,
    pub artifacts: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePublication {
    pub release_dir: PathBuf,
    pub version: String,
    pub artifacts: Vec<String>,
}

#[derive(Clone, Debug)]
struct PreparedArtifact {
    name: String,
    source_path: PathBuf,
    state: FileState,
    manifest_artifact: Option<SourceArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryJournal {
    version: u8,
    old_names: Vec<String>,
    new_names: Vec<String>,
    old_manifest_digest: Option<String>,
    new_manifest_digest: String,
}

struct ReleaseLock {
    path: PathBuf,
    state: FileState,
    root: PathBuf,
    file: File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionParts {
    core: [String; 3],
    prerelease: Option<Vec<String>>,
}

#[derive(Debug, Default)]
struct PendingFile {
    url: Option<String>,
    sha512: Option<String>,
    size: Option<String>,
}

pub fn compare_versions(first: &str, second: &str) -> Result<Ordering> {
    let left = parse_version(first)?;
    let right = parse_version(second)?;
    for (a, b) in left.core.iter().zip(right.core.iter()) {
        let compared = compare_decimal(a, b);
        if compared != Ordering::Equal {
            return Ok(compared);
        }
    }
    match (&left.prerelease, &right.prerelease) {
        (None, None) => Ok(Ordering::Equal),
        (None, Some(_)) => Ok(Ordering::Greater),
        (Some(_), None) => Ok(Ordering::Less),
        (Some(left), Some(right)) => {
            for index in 0..left.len().max(right.len()) {
                let Some(a) = left.get(index) else {
                    return Ok(Ordering::Less);
                };
                let Some(b) = right.get(index) else {
                    return Ok(Ordering::Greater);
                };
                if a == b {
                    continue;
                }
                let a_numeric = a.bytes().all(|byte| byte.is_ascii_digit());
                let b_numeric = b.bytes().all(|byte| byte.is_ascii_digit());
                if a_numeric && b_numeric {
                    return Ok(compare_decimal(a, b));
                }
                if a_numeric != b_numeric {
                    return Ok(if a_numeric {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    });
                }
                return Ok(a.cmp(b));
            }
            Ok(Ordering::Equal)
        }
    }
}

pub fn assert_version_progression(
    candidate: &str,
    current: Option<&str>,
    allow_same_version: bool,
) -> Result<()> {
    let Some(current) = current else {
        compare_versions(candidate, candidate)?;
        return Ok(());
    };
    match compare_versions(candidate, current)? {
        Ordering::Greater => Ok(()),
        Ordering::Equal if allow_same_version => Ok(()),
        Ordering::Less => bail!(
            "Update version {candidate} is older than existing version {current}; downgrades are never published."
        ),
        Ordering::Equal => bail!(
            "Update version {candidate} must be greater than existing version {current}. Use --allow-same-version only for a deliberate local replacement."
        ),
    }
}

#[allow(clippy::too_many_lines)]
pub fn parse_latest_mac_manifest(bytes: &[u8]) -> Result<UpdateManifest> {
    ensure!(!bytes.is_empty(), "{MANIFEST_NAME} is empty.");
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= MAX_MANIFEST_BYTES,
        "{MANIFEST_NAME} is unexpectedly large."
    );
    let source = std::str::from_utf8(bytes).context("latest-mac.yml is not UTF-8.")?;
    let mut version = None;
    let mut primary_path = None;
    let mut primary_sha512 = None;
    let mut files_seen = false;
    let mut in_files = false;
    let mut item = None;
    let mut item_index = 0_usize;
    let mut files = Vec::new();

    for (line_index, raw_line) in source.lines().enumerate() {
        ensure!(
            !raw_line.contains('\t'),
            "{MANIFEST_NAME} line {} contains a tab.",
            line_index + 1
        );
        ensure!(
            !raw_line.contains('\0'),
            "{MANIFEST_NAME} line {} contains NUL.",
            line_index + 1
        );
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw_line.len() - raw_line.trim_start().len();
        if indent == 0 {
            if in_files {
                finish_file(item.take(), &mut files, item_index)?;
                in_files = false;
            }
            let (key, raw) = yaml_field(trimmed, line_index + 1)?;
            match key {
                "files" => {
                    ensure!(
                        !files_seen,
                        "{MANIFEST_NAME} contains duplicate files fields."
                    );
                    ensure!(
                        raw.trim().is_empty(),
                        "{MANIFEST_NAME} files must be a block list."
                    );
                    files_seen = true;
                    in_files = true;
                }
                "version" => {
                    ensure!(
                        version.is_none(),
                        "{MANIFEST_NAME} contains duplicate version fields."
                    );
                    version = Some(yaml_scalar(raw, "version")?);
                }
                "path" => {
                    ensure!(
                        primary_path.is_none(),
                        "{MANIFEST_NAME} contains duplicate path fields."
                    );
                    primary_path = Some(yaml_scalar(raw, "path")?);
                }
                "sha512" => {
                    ensure!(
                        primary_sha512.is_none(),
                        "{MANIFEST_NAME} contains duplicate sha512 fields."
                    );
                    primary_sha512 = Some(valid_sha512(&yaml_scalar(raw, "sha512")?, "sha512")?);
                }
                _ => {}
            }
            continue;
        }
        if !in_files {
            continue;
        }
        let mut content = raw_line.trim_start();
        if let Some(rest) = content.strip_prefix('-') {
            finish_file(item.take(), &mut files, item_index)?;
            item_index += 1;
            item = Some(PendingFile::default());
            content = rest.trim_start();
            if content.is_empty() {
                continue;
            }
        } else {
            ensure!(
                item.is_some(),
                "{MANIFEST_NAME} files must contain list items."
            );
        }
        let (key, raw) = yaml_field(content, line_index + 1)?;
        if !matches!(key, "url" | "sha512" | "size") {
            continue;
        }
        let pending = item.as_mut().context("manifest file item is missing")?;
        let value = yaml_scalar(raw, &format!("files item {item_index} {key}"))?;
        match key {
            "url" => set_once(&mut pending.url, value, item_index, key)?,
            "sha512" => set_once(&mut pending.sha512, value, item_index, key)?,
            "size" => set_once(&mut pending.size, value, item_index, key)?,
            _ => unreachable!(),
        }
    }
    if in_files {
        finish_file(item, &mut files, item_index)?;
    }

    let manifest = UpdateManifest {
        version: version.context("latest-mac.yml is missing version.")?,
        primary_path: primary_path.context("latest-mac.yml is missing path.")?,
        primary_sha512: primary_sha512.context("latest-mac.yml is missing sha512.")?,
        files,
    };
    ensure!(files_seen, "{MANIFEST_NAME} has no artifact files.");
    validate_manifest(manifest)
}

pub fn inspect_update_source(source: &Path) -> Result<UpdateSource> {
    let source_dir = absolute_path(source)?;
    checked_directory(&source_dir, "source directory")?;
    let manifest_path = source_dir.join(MANIFEST_NAME);
    let manifest_bytes = read_checked_file(&manifest_path, "manifest", MAX_MANIFEST_BYTES)?;
    let manifest = parse_latest_mac_manifest(&manifest_bytes)?;
    let mut artifacts = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        let source_path = source_dir.join(&file.name);
        ensure!(
            source_path.parent() == Some(source_dir.as_path()),
            "Artifact escapes source directory: {}",
            file.name
        );
        let state = checked_regular_file(&source_path, &format!("artifact {}", file.name))?;
        ensure!(
            state.length == file.size,
            "Artifact size does not match {MANIFEST_NAME}: {}",
            file.name
        );
        artifacts.push(SourceArtifact {
            manifest: file.clone(),
            source_path,
            state,
        });
    }
    Ok(UpdateSource {
        source_dir,
        manifest_bytes,
        manifest,
        artifacts,
    })
}

pub fn stage_update_feed(
    source: &Path,
    target: &Path,
    allow_same_version: bool,
) -> Result<StagedFeed> {
    let plan = inspect_update_source(source)?;
    let target_dir = validate_target_path(target, &plan.source_dir)?;
    let target_dir = prepare_feed_directory(&target_dir)?;
    let lock = ReleaseLock::acquire(&target_dir)?;
    let publication = finish_locked(
        publish_release_artifacts_unlocked(
            &plan,
            &[],
            &target_dir,
            allow_same_version,
            false,
            |from, to| fs::rename(from, to),
        ),
        lock,
    )?;

    Ok(StagedFeed {
        target_dir,
        version: plan.manifest.version,
        artifacts: publication.artifacts,
    })
}

pub fn default_feed_target() -> Result<PathBuf> {
    Ok(crate::pipeline_state::home()?
        .join("Library")
        .join("Application Support")
        .join("Interface")
        .join("update-feed"))
}

/// Bring the installed production app forward after a feed is staged so its
/// local-feed watcher checks immediately. Best-effort: a missing or stopped
/// app must not fail the stage.
pub fn nudge_running_production_app() {
    let _ = Command::new("/usr/bin/open")
        .args(["-a", "Interface"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn selected_update_artifacts(plan: &UpdateSource) -> Result<Vec<SourceArtifact>> {
    for artifact in &plan.artifacts {
        ensure!(
            extension_is(&artifact.manifest.name, "zip")
                || extension_is(&artifact.manifest.name, "dmg"),
            "The update manifest references an unsupported artifact: {}",
            artifact.manifest.name
        );
    }
    ensure!(
        plan.artifacts
            .iter()
            .any(|artifact| extension_is(&artifact.manifest.name, "zip")),
        "latest-mac.yml must reference a ZIP for Squirrel.Mac."
    );
    Ok(plan.artifacts.clone())
}

fn hash_stream<R: Read>(
    reader: &mut R,
    mut sink: Option<&mut dyn Write>,
    artifact: &ManifestArtifact,
    verb: &str,
    overflow: &'static str,
) -> Result<()> {
    let mut hasher = Sha512::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut length = 0_u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .context("could not read artifact")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        if let Some(sink) = sink.as_mut() {
            sink.write_all(&buffer[..count])
                .context("could not write staged artifact")?;
        }
        length = length
            .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
            .context(overflow)?;
    }
    ensure!(
        length == artifact.size,
        "Artifact size changed while {verb}: {}",
        artifact.name
    );
    let expected = BASE64
        .decode(&artifact.sha512)
        .context("manifest SHA-512 is invalid")?;
    ensure!(
        hasher.finalize().as_slice() == expected,
        "Artifact checksum does not match {MANIFEST_NAME}: {}",
        artifact.name
    );
    Ok(())
}

pub fn verify_update_source_checksums(plan: &UpdateSource) -> Result<()> {
    for artifact in &plan.artifacts {
        let mut file = artifact.open()?;
        hash_stream(
            &mut file,
            None,
            &artifact.manifest,
            "verifying",
            "artifact length overflow",
        )?;
    }
    Ok(())
}

pub fn optional_blockmaps(plan: &UpdateSource) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for artifact in &plan.artifacts {
        let name = format!("{}.blockmap", artifact.manifest.name);
        validate_flat_name(&name, "blockmap")?;
        let candidate = plan.source_dir.join(name);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                validate_regular_metadata(&metadata, "blockmap")?;
                result.push(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not inspect optional blockmap"),
        }
    }
    Ok(result)
}

pub fn preflight_release_directory(
    release: &Path,
    candidate_version: &str,
    allow_same_version: bool,
) -> Result<usize> {
    let release = prepare_release_directory(release)?;
    let lock = ReleaseLock::acquire(&release)?;
    let result = (|| {
        let recovered = recover_release_transactions_unlocked(&release)?;
        let current = existing_release_plan(&release)?;
        assert_version_progression(
            candidate_version,
            current.as_ref().map(|plan| plan.manifest.version.as_str()),
            allow_same_version,
        )?;
        Ok(recovered)
    })();
    finish_locked(result, lock)
}

pub fn publish_release_artifacts(
    plan: &UpdateSource,
    optional_artifacts: &[PathBuf],
    release: &Path,
    allow_same_version: bool,
) -> Result<ReleasePublication> {
    let release = prepare_release_directory(release)?;
    let lock = ReleaseLock::acquire(&release)?;
    let result = publish_release_artifacts_unlocked(
        plan,
        optional_artifacts,
        &release,
        allow_same_version,
        true,
        |from, to| fs::rename(from, to),
    );
    finish_locked(result, lock)
}

#[allow(clippy::too_many_lines)]
fn publish_release_artifacts_unlocked<F>(
    plan: &UpdateSource,
    optional_artifacts: &[PathBuf],
    release: &Path,
    allow_same_version: bool,
    preserve_unrelated: bool,
    mut rename_file: F,
) -> Result<ReleasePublication>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    recover_release_transactions_unlocked(release)?;
    selected_update_artifacts(plan)?;
    let current = existing_release_plan(release)?;
    assert_version_progression(
        &plan.manifest.version,
        current.as_ref().map(|old| old.manifest.version.as_str()),
        allow_same_version,
    )?;

    let mut prepared = Vec::new();
    let mut new_names = HashSet::new();
    for artifact in &plan.artifacts {
        ensure!(
            new_names.insert(artifact.manifest.name.clone()),
            "Release artifact is duplicated: {}",
            artifact.manifest.name
        );
        prepared.push(PreparedArtifact {
            name: artifact.manifest.name.clone(),
            source_path: artifact.source_path.clone(),
            state: artifact.state.clone(),
            manifest_artifact: Some(artifact.clone()),
        });
    }
    for path in optional_artifacts {
        let name = path
            .file_name()
            .and_then(OsStr::to_str)
            .context("optional release artifact name is not UTF-8")?
            .to_owned();
        validate_flat_name(&name, "optional release artifact")?;
        ensure!(
            name.ends_with(".blockmap"),
            "Unsupported optional release artifact: {name}"
        );
        ensure!(
            new_names.insert(name.clone()),
            "Release artifact is duplicated: {name}"
        );
        prepared.push(PreparedArtifact {
            name,
            source_path: path.clone(),
            state: checked_regular_file(path, "optional release artifact")?,
            manifest_artifact: None,
        });
    }

    let mut old_names = HashSet::new();
    if let Some(old) = &current {
        old_names.insert(MANIFEST_NAME.to_owned());
        for artifact in &old.artifacts {
            old_names.insert(artifact.manifest.name.clone());
            let blockmap = format!("{}.blockmap", artifact.manifest.name);
            let candidate = release.join(&blockmap);
            if fs::symlink_metadata(&candidate).is_ok() {
                checked_regular_file(&candidate, "existing release blockmap")?;
                old_names.insert(blockmap);
            }
        }
    }
    let managed_names: HashSet<_> = old_names.iter().chain(new_names.iter()).cloned().collect();
    if !preserve_unrelated {
        for entry in fs::read_dir(release).context("could not enumerate update feed")? {
            let entry = entry.context("could not inspect update feed entry")?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Update feed entry name is not UTF-8."))?;
            ensure!(
                name == RELEASE_LOCK_NAME || managed_names.contains(&name),
                "Update feed contains an unmanaged entry: {name}"
            );
        }
    }
    let mut known_destinations = HashSet::new();
    for name in &managed_names {
        validate_flat_name(name, "release artifact")?;
        let path = release.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                checked_regular_file(&path, "existing release artifact")?;
                known_destinations.insert(name.clone());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not inspect release destination"),
        }
    }

    let transaction = Builder::new()
        .prefix(TRANSACTION_PREFIX)
        .tempdir_in(release)
        .context("could not create release transaction")?;
    set_private_directory_mode(transaction.path())?;
    let next = transaction.path().join("next");
    let previous = transaction.path().join("previous");
    fs::create_dir(&next).context("could not create release next directory")?;
    fs::create_dir(&previous).context("could not create release previous directory")?;
    set_private_directory_mode(&next)?;
    set_private_directory_mode(&previous)?;

    for artifact in &prepared {
        let destination = next.join(&artifact.name);
        if let Some(manifest_artifact) = &artifact.manifest_artifact {
            copy_verified_artifact(manifest_artifact, &destination)?;
        } else {
            copy_checked_file(
                &artifact.source_path,
                &artifact.state,
                &destination,
                &artifact.name,
            )?;
        }
    }
    write_new_file(&next.join(MANIFEST_NAME), &plan.manifest_bytes, 0o644)?;
    sync_directory(&next)?;

    let mut old_names_sorted: Vec<_> = known_destinations.iter().cloned().collect();
    old_names_sorted.sort();
    let mut new_names_sorted: Vec<_> = new_names.iter().cloned().collect();
    new_names_sorted.push(MANIFEST_NAME.to_owned());
    new_names_sorted.sort();
    let journal = RecoveryJournal {
        version: 1,
        old_names: old_names_sorted.clone(),
        new_names: new_names_sorted,
        old_manifest_digest: current.as_ref().map(|old| hex_digest(&old.manifest_bytes)),
        new_manifest_digest: hex_digest(&plan.manifest_bytes),
    };
    let journal_bytes = serde_json::to_vec(&journal).context("could not encode release journal")?;
    ensure!(
        u64::try_from(journal_bytes.len()).unwrap_or(u64::MAX) <= MAX_JOURNAL_BYTES,
        "Release recovery journal is unexpectedly large."
    );
    let mut journal_line = journal_bytes;
    journal_line.push(b'\n');
    write_new_file(&transaction.path().join(JOURNAL_NAME), &journal_line, 0o600)?;
    sync_directory(transaction.path())?;

    let transaction_path = transaction.keep();
    let next = transaction_path.join("next");
    let previous = transaction_path.join("previous");
    let mut moved_previous = Vec::new();
    let mut published = Vec::new();
    let result = (|| {
        let mut backup_order = old_names_sorted;
        backup_order.sort_by_key(|name| (name != MANIFEST_NAME, name.clone()));
        for name in backup_order {
            rename_file(&release.join(&name), &previous.join(&name))
                .with_context(|| format!("could not back up release artifact {name}"))?;
            moved_previous.push(name);
        }
        sync_directory(&previous)?;
        sync_directory(release)?;
        write_new_file(&transaction_path.join(BACKUP_COMPLETE_NAME), b"ok\n", 0o600)?;
        sync_directory(&transaction_path)?;

        let mut publish_names: Vec<_> = new_names.into_iter().collect();
        publish_names.sort();
        for name in publish_names {
            rename_file(&next.join(&name), &release.join(&name))
                .with_context(|| format!("could not publish release artifact {name}"))?;
            published.push(name);
        }
        sync_directory(&next)?;
        sync_directory(release)?;
        rename_file(&next.join(MANIFEST_NAME), &release.join(MANIFEST_NAME))
            .context("could not publish release manifest")?;
        published.push(MANIFEST_NAME.to_owned());
        sync_directory(release)?;
        Ok(())
    })();

    if let Err(error) = result {
        let rollback = rollback_publication(
            release,
            &next,
            &previous,
            &published,
            &moved_previous,
            &mut rename_file,
        );
        if let Err(rollback_error) = rollback {
            return Err(error).context(format!(
                "Release publication rollback was incomplete: {rollback_error:#}"
            ));
        }
        fs::remove_dir_all(&transaction_path).ok();
        return Err(error);
    }

    fs::remove_dir_all(&transaction_path).context("could not remove release transaction")?;
    sync_directory(release)?;
    let mut artifacts: Vec<_> = prepared.into_iter().map(|artifact| artifact.name).collect();
    artifacts.sort();
    Ok(ReleasePublication {
        release_dir: release.to_owned(),
        version: plan.manifest.version.clone(),
        artifacts,
    })
}

fn rollback_publication<F>(
    release: &Path,
    next: &Path,
    previous: &Path,
    published: &[String],
    moved_previous: &[String],
    rename_file: &mut F,
) -> Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    for name in published.iter().rev() {
        rename_file(&release.join(name), &next.join(name))
            .with_context(|| format!("could not roll back new release artifact {name}"))?;
    }
    for name in moved_previous.iter().rev() {
        rename_file(&previous.join(name), &release.join(name))
            .with_context(|| format!("could not restore prior release artifact {name}"))?;
    }
    sync_directory(release)
}

fn recover_release_transactions_unlocked(release: &Path) -> Result<usize> {
    let mut transactions = Vec::new();
    for entry in fs::read_dir(release).context("could not enumerate release directory")? {
        let entry = entry.context("could not inspect release entry")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(TRANSACTION_PREFIX) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Release transaction path is unsafe."
        );
        transactions.push(entry.path());
    }
    transactions.sort();
    for transaction in &transactions {
        recover_one_transaction(release, transaction)?;
    }
    Ok(transactions.len())
}

fn recover_one_transaction(release: &Path, transaction: &Path) -> Result<()> {
    let journal_path = transaction.join(JOURNAL_NAME);
    if !journal_path.exists() {
        return discard_journal_less_transaction(release, transaction);
    }
    let bytes = read_checked_file(&journal_path, "release recovery journal", MAX_JOURNAL_BYTES)?;
    let journal: RecoveryJournal =
        serde_json::from_slice(&bytes).context("Release recovery journal is not valid JSON.")?;
    validate_recovery_journal(&journal)?;
    let next = transaction.join("next");
    let previous = transaction.join("previous");
    checked_directory(&next, "release recovery next directory")?;
    checked_directory(&previous, "release recovery previous directory")?;
    let manifest_path = release.join(MANIFEST_NAME);
    match fs::symlink_metadata(&manifest_path) {
        Ok(_) => {
            let bytes = read_checked_file(
                &manifest_path,
                "release recovery manifest",
                MAX_MANIFEST_BYTES,
            )?;
            let digest = hex_digest(&bytes);
            if digest == journal.new_manifest_digest {
                for name in journal
                    .new_names
                    .iter()
                    .filter(|name| name.as_str() != MANIFEST_NAME)
                {
                    checked_regular_file(&release.join(name), "committed release artifact")?;
                }
            } else if journal.old_manifest_digest.as_deref() == Some(digest.as_str()) {
                recover_with_old_manifest(release, &next, &previous, &journal)?;
            } else {
                bail!("Release manifest does not match the transaction journal.");
            }
            fs::remove_dir_all(transaction).context("could not finish release recovery")?;
            sync_directory(release)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if transaction.join(BACKUP_COMPLETE_NAME).exists() {
                for name in journal
                    .new_names
                    .iter()
                    .filter(|name| name.as_str() != MANIFEST_NAME)
                {
                    let published = release.join(name);
                    if published.exists() {
                        checked_regular_file(&published, "published recovery artifact")?;
                        ensure!(
                            !next.join(name).exists(),
                            "Release recovery found two copies of a managed artifact."
                        );
                        fs::rename(&published, next.join(name))?;
                    }
                }
            }
            restore_previous_files(release, &previous, &journal.old_names)?;
            fs::remove_dir_all(transaction).context("could not finish release rollback")?;
            sync_directory(release)
        }
        Err(error) => Err(error).context("could not inspect release recovery manifest"),
    }
}

fn recover_with_old_manifest(
    release: &Path,
    next: &Path,
    previous: &Path,
    journal: &RecoveryJournal,
) -> Result<()> {
    let old_names: HashSet<_> = journal.old_names.iter().map(String::as_str).collect();
    for name in journal
        .new_names
        .iter()
        .filter(|name| name.as_str() != MANIFEST_NAME && !old_names.contains(name.as_str()))
    {
        let live = release.join(name);
        if live.exists() {
            checked_regular_file(&live, "partially published release artifact")?;
            ensure!(
                !next.join(name).exists(),
                "Release recovery found two copies of a new artifact."
            );
            fs::rename(&live, next.join(name))?;
        }
    }

    for name in journal
        .old_names
        .iter()
        .filter(|name| name.as_str() != MANIFEST_NAME)
    {
        let live = release.join(name);
        let backup = previous.join(name);
        match (live.exists(), backup.exists()) {
            (true, true) => {
                checked_regular_file(&live, "partially published release artifact")?;
                checked_regular_file(&backup, "prior release recovery artifact")?;
                ensure!(
                    !next.join(name).exists(),
                    "Release recovery found two copies of a managed artifact."
                );
                fs::rename(&live, next.join(name))?;
                fs::rename(&backup, &live)?;
            }
            (false, true) => {
                checked_regular_file(&backup, "prior release recovery artifact")?;
                fs::rename(&backup, &live)?;
            }
            (true, false) => {
                checked_regular_file(&live, "prior release artifact")?;
            }
            (false, false) => {
                bail!("Release recovery could not find a prior managed artifact.");
            }
        }
    }
    sync_directory(next)?;
    sync_directory(previous)?;
    sync_directory(release)
}

fn discard_journal_less_transaction(release: &Path, transaction: &Path) -> Result<()> {
    let mut next = None;
    let mut previous = None;
    for entry in fs::read_dir(transaction)? {
        let entry = entry?;
        let name = entry.file_name();
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Journal-less release transaction has unsafe contents."
        );
        match name.to_str() {
            Some("next") => next = Some(entry.path()),
            Some("previous") => previous = Some(entry.path()),
            _ => bail!("Journal-less release transaction has unsafe contents."),
        }
    }
    if let Some(previous) = &previous {
        ensure!(
            fs::read_dir(previous)?.next().is_none(),
            "Journal-less release transaction may contain prior artifacts."
        );
    }
    if let Some(next) = &next {
        for entry in fs::read_dir(next)? {
            let entry = entry?;
            validate_flat_name(&entry.file_name().to_string_lossy(), "generated artifact")?;
            checked_regular_file(&entry.path(), "journal-less generated artifact")?;
        }
    }
    fs::remove_dir_all(transaction).context("could not discard pre-commit transaction")?;
    sync_directory(release)
}

fn restore_previous_files(release: &Path, previous: &Path, names: &[String]) -> Result<()> {
    let mut ordered: Vec<_> = names
        .iter()
        .filter(|name| name.as_str() != MANIFEST_NAME)
        .cloned()
        .collect();
    ordered.sort();
    if names.iter().any(|name| name == MANIFEST_NAME) {
        ordered.push(MANIFEST_NAME.to_owned());
    }
    for name in ordered {
        let source = previous.join(&name);
        if source.exists() {
            checked_regular_file(&source, "prior release recovery artifact")?;
            ensure!(
                !release.join(&name).exists(),
                "Release recovery destination is unexpectedly occupied."
            );
            fs::rename(source, release.join(name))?;
        } else {
            checked_regular_file(&release.join(name), "prior release artifact")?;
        }
    }
    sync_directory(release)
}

fn validate_recovery_journal(journal: &RecoveryJournal) -> Result<()> {
    ensure!(journal.version == 1, "Release recovery journal is invalid.");
    ensure!(
        journal.new_names.iter().any(|name| name == MANIFEST_NAME),
        "Recovery journal does not own a new manifest."
    );
    for names in [&journal.old_names, &journal.new_names] {
        let mut unique = HashSet::new();
        for name in names {
            validate_flat_name(name, "recovery journal artifact")?;
            ensure!(unique.insert(name), "Recovery journal contains duplicates.");
        }
    }
    ensure!(
        journal.old_manifest_digest.is_some()
            == journal.old_names.iter().any(|name| name == MANIFEST_NAME),
        "Recovery journal old manifest ownership is inconsistent."
    );
    for digest in journal
        .old_manifest_digest
        .iter()
        .chain(std::iter::once(&journal.new_manifest_digest))
    {
        ensure!(
            digest.len() == 128 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Recovery journal digest is invalid."
        );
    }
    Ok(())
}

fn existing_release_plan(release: &Path) -> Result<Option<UpdateSource>> {
    match fs::symlink_metadata(release.join(MANIFEST_NAME)) {
        Ok(_) => inspect_update_source(release).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("could not inspect existing release manifest"),
    }
}

fn prepare_release_directory(release: &Path) -> Result<PathBuf> {
    let release = absolute_path(release)?;
    fs::create_dir_all(&release).context("could not create release directory")?;
    checked_directory(&release, "release directory")?;
    Ok(release)
}

fn prepare_feed_directory(target: &Path) -> Result<PathBuf> {
    let target = absolute_path(target)?;
    let parent = target
        .parent()
        .context("update target has no parent directory")?;
    fs::create_dir_all(parent).context("could not create update target parent")?;
    checked_directory(parent, "update target parent")?;

    match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Target must be a real directory."
            );
            ensure!(
                metadata.permissions().mode() & 0o777 == 0o700,
                "Existing update target must have mode 0700."
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(&target)
                .context("could not create update target")?;
            set_private_directory_mode(&target)?;
            sync_directory(parent)?;
        }
        Err(error) => return Err(error).context("could not inspect update target"),
    }

    checked_directory(&target, "update target")?;
    Ok(target)
}

/// Serializes an entire coordinated operation, not merely its final file copy.
pub fn with_pipeline_lock<T>(
    root: &Path,
    name: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    crate::pipeline_state::private_directory(root)?;
    let lock = ReleaseLock::acquire_named(root, name)?;
    finish_locked(operation(), lock)
}

fn finish_locked<T>(result: Result<T>, lock: ReleaseLock) -> Result<T> {
    let released = lock.release();
    match (result, released) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(release_error)) => Err(error).context(format!(
            "release lock cleanup also failed: {release_error:#}"
        )),
    }
}

impl ReleaseLock {
    fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_named(root, RELEASE_LOCK_NAME)
    }

    fn acquire_named(root: &Path, name: &str) -> Result<Self> {
        let path = root.join(name);
        for _ in 0..2 {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            options.mode(0o600);
            match options.open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{{\"pid\":{}}}", std::process::id())?;
                    file.sync_all()?;
                    let state = FileState::from_metadata(&file.metadata()?);
                    sync_directory(root)?;
                    return Ok(Self {
                        path,
                        state,
                        root: root.to_owned(),
                        file,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let state = checked_regular_file(&path, "release package lock")?;
                    let bytes = read_checked_file(&path, "release package lock", 4096)?;
                    let value: serde_json::Value =
                        serde_json::from_slice(&bytes).context("Release lock is invalid.")?;
                    let pid = value
                        .get("pid")
                        .and_then(serde_json::Value::as_u64)
                        .context("Release lock owner is invalid.")?;
                    if process_is_alive(pid) {
                        bail!("Another package publication is active (pid {pid}).");
                    }
                    let current = checked_regular_file(&path, "stale release package lock")?;
                    ensure!(
                        state.matches_state(&current),
                        "Release package lock changed."
                    );
                    fs::remove_file(&path).context("could not remove stale release lock")?;
                    sync_directory(root)?;
                }
                Err(error) => return Err(error).context("could not acquire release package lock"),
            }
        }
        bail!("Could not acquire the release package lock.")
    }

    fn release(self) -> Result<()> {
        self.file.sync_all()?;
        drop(self.file);
        let current = checked_regular_file(&self.path, "release package lock")?;
        ensure!(
            self.state.matches_state(&current),
            "Release package lock changed while held."
        );
        fs::remove_file(&self.path).context("could not release package lock")?;
        sync_directory(&self.root)
    }
}

fn process_is_alive(pid: u64) -> bool {
    if pid == u64::from(std::process::id()) {
        return true;
    }
    let status = Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .status();
    status.is_ok_and(|status| status.success())
}

fn copy_checked_file(
    source_path: &Path,
    expected: &FileState,
    destination: &Path,
    label: &str,
) -> Result<()> {
    let mut source = open_unchanged(source_path, expected, label)?;
    let mut destination = create_new_file(destination, 0o644)?;
    io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    ensure!(
        destination.metadata()?.len() == expected.length,
        "{label} changed while copying."
    );
    Ok(())
}

/// Lowercase hexadecimal, the encoding every digest in this tool is written in:
/// the digest types format that way themselves.
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha512::digest(bytes))
}

fn parse_version(value: &str) -> Result<VersionParts> {
    let (without_build, build) = value
        .split_once('+')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    if let Some(build) = build {
        validate_identifiers(build, false)?;
    }
    let (core, prerelease) = without_build
        .split_once('-')
        .map_or((without_build, None), |(a, b)| (a, Some(b)));
    let core: Vec<_> = core.split('.').collect();
    ensure!(core.len() == 3, "Invalid update version: {value}");
    for part in &core {
        ensure!(
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (*part == "0" || !part.starts_with('0')),
            "Invalid update version: {value}"
        );
    }
    let prerelease = prerelease
        .map(|raw| {
            validate_identifiers(raw, true)?;
            Ok::<_, anyhow::Error>(raw.split('.').map(str::to_owned).collect())
        })
        .transpose()?;
    Ok(VersionParts {
        core: [core[0].to_owned(), core[1].to_owned(), core[2].to_owned()],
        prerelease,
    })
}

fn validate_identifiers(value: &str, reject_numeric_leading_zero: bool) -> Result<()> {
    ensure!(!value.is_empty(), "Invalid update version: {value}");
    for identifier in value.split('.') {
        ensure!(
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "Invalid update version: {value}"
        );
        if reject_numeric_leading_zero
            && identifier.len() > 1
            && identifier.bytes().all(|byte| byte.is_ascii_digit())
        {
            ensure!(
                !identifier.starts_with('0'),
                "Invalid update version: {value}"
            );
        }
    }
    Ok(())
}

fn compare_decimal(first: &str, second: &str) -> Ordering {
    first
        .len()
        .cmp(&second.len())
        .then_with(|| first.cmp(second))
}

fn yaml_field(text: &str, line: usize) -> Result<(&str, &str)> {
    let (key, raw) = text
        .split_once(':')
        .with_context(|| format!("{MANIFEST_NAME} line {line} is not a supported YAML field."))?;
    ensure!(
        !key.is_empty()
            && key.bytes().enumerate().all(|(index, byte)| if index == 0 {
                byte.is_ascii_alphabetic()
            } else {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
            }),
        "{MANIFEST_NAME} line {line} is not a supported YAML field."
    );
    Ok((key, raw))
}

fn yaml_scalar(raw: &str, context: &str) -> Result<String> {
    let mut value = raw.trim();
    ensure!(!value.is_empty(), "{context} must have a value.");
    let owned;
    if value.starts_with('\'') {
        ensure!(
            value.len() >= 2 && value.ends_with('\''),
            "{context} has an invalid quoted value."
        );
        owned = value[1..value.len() - 1].replace("''", "'");
        value = &owned;
    } else if value.starts_with('"') {
        owned = serde_json::from_str::<String>(value)
            .with_context(|| format!("{context} has an invalid quoted value."))?;
        value = &owned;
    } else {
        if let Some(comment) = value.find(" #") {
            value = value[..comment].trim_end();
        }
        ensure!(
            !value
                .bytes()
                .next()
                .is_some_and(|byte| matches!(byte, b'!' | b'&' | b'*' | b'|' | b'>' | b'{' | b'[')),
            "{context} uses unsupported YAML syntax."
        );
    }
    ensure!(
        !value.is_empty() && !value.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')),
        "{context} must be one line of text."
    );
    Ok(value.to_owned())
}

fn set_once(field: &mut Option<String>, value: String, item_index: usize, key: &str) -> Result<()> {
    ensure!(field.is_none(), "files item {item_index} repeats {key}.");
    *field = Some(value);
    Ok(())
}

fn finish_file(
    item: Option<PendingFile>,
    files: &mut Vec<ManifestArtifact>,
    index: usize,
) -> Result<()> {
    let Some(item) = item else {
        return Ok(());
    };
    let label = format!("files item {index}");
    let name = item
        .url
        .with_context(|| format!("{label} is missing url."))?;
    let sha512 = valid_sha512(
        &item
            .sha512
            .with_context(|| format!("{label} is missing sha512."))?,
        &format!("{label} sha512"),
    )?;
    let size = item
        .size
        .with_context(|| format!("{label} is missing size."))?;
    ensure!(
        size == "0" || (!size.starts_with('0') && size.bytes().all(|byte| byte.is_ascii_digit())),
        "{label} has an invalid size."
    );
    let size = size
        .parse::<u64>()
        .with_context(|| format!("{label} has an invalid size."))?;
    files.push(ManifestArtifact { name, sha512, size });
    Ok(())
}

fn validate_manifest(manifest: UpdateManifest) -> Result<UpdateManifest> {
    parse_version(&manifest.version)?;
    ensure!(
        !manifest.files.is_empty(),
        "Manifest has no artifact files."
    );
    validate_artifact_name(&manifest.primary_path, &manifest.version)?;
    valid_sha512(&manifest.primary_sha512, "sha512")?;
    let mut names = HashSet::new();
    for file in &manifest.files {
        validate_artifact_name(&file.name, &manifest.version)?;
        valid_sha512(&file.sha512, &format!("{} sha512", file.name))?;
        ensure!(
            names.insert(file.name.clone()),
            "Manifest repeats artifact: {}",
            file.name
        );
    }
    let primary = manifest
        .files
        .iter()
        .find(|file| file.name == manifest.primary_path)
        .context("Manifest path is not present in files.")?;
    ensure!(
        primary.sha512 == manifest.primary_sha512,
        "Manifest path sha512 does not match its files entry."
    );
    Ok(manifest)
}

fn validate_artifact_name(name: &str, version: &str) -> Result<()> {
    validate_flat_name(name, "artifact")?;
    ensure!(
        !name.contains(['%', '?', '#']),
        "Artifact path is unsafe: {name}"
    );
    ensure!(
        name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'+' | b'(' | b')' | b'@' | b' ' | b'-')
        }),
        "Artifact name contains unsupported characters: {name}"
    );
    ensure!(
        version_at_boundary(name, version),
        "Artifact name does not contain version {version}: {name}"
    );
    Ok(())
}

pub(crate) fn extension_is(path: &str, expected: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn version_at_boundary(name: &str, version: &str) -> bool {
    name.match_indices(version).any(|(index, _)| {
        let before = name[..index].bytes().next_back();
        let after = name[index + version.len()..].bytes().next();
        before.is_none_or(|byte| matches!(byte, b'-' | b'_' | b' '))
            && after
                .is_none_or(|byte| matches!(byte, b'-' | b'_' | b'.' | b'+' | b' ' | b'(' | b')'))
    })
}

fn validate_flat_name(name: &str, label: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 255
            && Path::new(name).file_name() == Some(OsStr::new(name))
            && !name.contains(['/', '\\'])
            && !matches!(name, "." | "..")
            && !name.chars().any(char::is_control),
        "{label} has an unsafe name: {name}"
    );
    Ok(())
}

fn valid_sha512(value: &str, context: &str) -> Result<String> {
    ensure!(
        value.len() == 88
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')),
        "{context} is not a SHA-512 value."
    );
    let decoded = BASE64
        .decode(value)
        .with_context(|| format!("{context} is not a SHA-512 value."))?;
    ensure!(
        decoded.len() == 64 && BASE64.encode(decoded) == value,
        "{context} is not a canonical SHA-512 value."
    );
    Ok(value.to_owned())
}

fn validate_target_path(target: &Path, source: &Path) -> Result<PathBuf> {
    let target = absolute_path(target)?;
    let root = target
        .components()
        .next()
        .map_or_else(PathBuf::new, |component| match component {
            Component::RootDir => PathBuf::from("/"),
            Component::Prefix(prefix) => PathBuf::from(prefix.as_os_str()),
            _ => PathBuf::new(),
        });
    ensure!(target != root, "Refusing unsafe target directory.");
    if let Some(home) = env::var_os("HOME") {
        ensure!(
            target.as_os_str() != home,
            "Refusing unsafe target directory."
        );
    }
    ensure!(
        target != source && !target.starts_with(source) && !source.starts_with(&target),
        "Source and target directories must not overlap."
    );
    Ok(target)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()
            .context("could not resolve current directory")?
            .join(path)
    };
    Ok(lexically_normalized(&path))
}

fn checked_directory(path: &Path, label: &str) -> Result<FileState> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("Missing {label}."))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{label} must be a real directory, not a symlink."
    );
    Ok(FileState::from_metadata(&metadata))
}

fn copy_verified_artifact(artifact: &SourceArtifact, destination: &Path) -> Result<()> {
    let mut source = artifact.open()?;
    let mut target = create_new_file(destination, 0o644)?;
    // `hash_stream` already reads and writes a COPY_BUFFER_BYTES block at a
    // time, so buffering wrappers here would only add a second copy of it.
    hash_stream(
        &mut source,
        Some(&mut target),
        &artifact.manifest,
        "staging",
        "artifact size overflow",
    )?;
    target
        .sync_all()
        .context("could not sync staged artifact")?;
    Ok(())
}

fn create_new_file(path: &Path, mode: u32) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(mode);
    options.open(path).context("could not create private file")
}

fn write_new_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = create_new_file(path, mode)?;
    file.write_all(bytes).context("could not write file")?;
    file.sync_all().context("could not sync file")
}

fn set_private_directory_mode(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .context("could not set private directory permissions")?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("could not sync directory")
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        fmt::Write as _,
        os::unix::fs::{MetadataExt as _, symlink},
    };

    use super::*;

    fn digest(bytes: &[u8]) -> String {
        BASE64.encode(Sha512::digest(bytes))
    }

    /// Receipts, identities and journals written before the digest types
    /// formatted themselves were two lowercase digits per byte; they still match.
    #[test]
    fn digests_are_two_lowercase_hex_digits_per_byte() {
        use sha2::Sha256;
        let byte_by_byte = |bytes: &[u8]| {
            bytes.iter().fold(String::new(), |mut text, byte| {
                let _ = write!(text, "{byte:02x}");
                text
            })
        };
        assert_eq!(
            hex_digest(b""),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        for input in [&b"latest-mac.yml"[..], b"\0", &[0xff; 300]] {
            assert_eq!(hex_digest(input), byte_by_byte(&Sha512::digest(input)));
            assert_eq!(
                format!("{:x}", Sha256::digest(input)),
                byte_by_byte(&Sha256::digest(input))
            );
        }
    }

    fn manifest(version: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let primary = files
            .iter()
            .find(|(name, _)| extension_is(name, "zip"))
            .expect("zip");
        let mut text = format!("version: {version}\nfiles:\n");
        for (name, bytes) in files {
            write!(
                text,
                "  - url: {name}\n    sha512: {}\n    size: {}\n",
                digest(bytes),
                bytes.len()
            )
            .expect("manifest row");
        }
        write!(
            text,
            "path: {}\nsha512: {}\nreleaseDate: '2026-08-27T00:00:00.000Z'\n",
            primary.0,
            digest(primary.1)
        )
        .expect("manifest footer");
        text.into_bytes()
    }

    fn write_source(root: &Path, version: &str) -> PathBuf {
        let source = root.join(format!("source-{version}"));
        fs::create_dir(&source).expect("source");
        let zip_name = format!("Interface-{version}-arm64-mac.zip");
        let dmg_name = format!("Interface-{version}-arm64.dmg");
        let zip = format!("zip-{version}").into_bytes();
        let dmg = format!("dmg-{version}").into_bytes();
        fs::write(source.join(&zip_name), &zip).expect("zip");
        fs::write(source.join(&dmg_name), &dmg).expect("dmg");
        fs::write(
            source.join(MANIFEST_NAME),
            manifest(version, &[(&zip_name, &zip), (&dmg_name, &dmg)]),
        )
        .expect("manifest");
        source
    }

    #[test]
    fn parses_exact_builder_manifest_and_arbitrary_precision_semver() {
        let files: &[(&str, &[u8])] = &[
            ("Interface-2.0.0-arm64-mac.zip", b"zip"),
            ("Interface-2.0.0-arm64.dmg", b"dmg"),
        ];
        let parsed = parse_latest_mac_manifest(&manifest("2.0.0", files)).expect("manifest");
        assert_eq!(parsed.version, "2.0.0");
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(
            compare_versions(
                "999999999999999999999999.0.0",
                "999999999999999999999998.999999999999999999.9"
            )
            .expect("compare"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0-alpha.2", "1.0.0-alpha.10").expect("compare"),
            Ordering::Less
        );
        assert!(compare_versions("1.0.0-01", "1.0.0").is_err());
    }

    #[test]
    fn rejects_traversal_versionless_and_noncanonical_manifest_values() {
        let hash = digest(b"zip");
        for name in [
            "../Interface-1.0.0.zip",
            "Interface-current.zip",
            "bad%1.0.0.zip",
        ] {
            let bytes = format!(
                "version: 1.0.0\nfiles:\n  - url: {name}\n    sha512: {hash}\n    size: 3\npath: {name}\nsha512: {hash}\n"
            );
            assert!(
                parse_latest_mac_manifest(bytes.as_bytes()).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn stages_only_manifest_files_and_requires_version_progression() {
        let root = tempfile::tempdir().expect("root");
        let first = write_source(root.path(), "1.0.0");
        fs::write(first.join("unreferenced.txt"), b"secret").expect("extra");
        let target = root.path().join("feed");
        let staged = stage_update_feed(&first, &target, false).expect("first stage");
        assert_eq!(staged.version, "1.0.0");
        assert!(!target.join("unreferenced.txt").exists());
        assert!(stage_update_feed(&first, &target, false).is_err());
        stage_update_feed(&first, &target, true).expect("same version replacement");

        let second = write_source(root.path(), "1.1.0");
        fs::write(target.join("unexpected.txt"), b"unexpected").expect("unexpected target");
        assert!(stage_update_feed(&second, &target, false).is_err());
        fs::remove_file(target.join("unexpected.txt")).expect("remove unexpected target");
        stage_update_feed(&second, &target, false).expect("upgrade");
        let active = inspect_update_source(&target).expect("active feed");
        assert_eq!(active.manifest.version, "1.1.0");
    }

    #[test]
    fn checksum_failure_and_linked_input_leave_existing_feed_untouched() {
        let root = tempfile::tempdir().expect("root");
        let first = write_source(root.path(), "1.0.0");
        let target = root.path().join("feed");
        stage_update_feed(&first, &target, false).expect("first stage");
        let prior = fs::read(target.join(MANIFEST_NAME)).expect("prior");

        let second = write_source(root.path(), "1.1.0");
        let zip = second.join("Interface-1.1.0-arm64-mac.zip");
        fs::write(&zip, b"tampered").expect("tamper");
        assert!(stage_update_feed(&second, &target, false).is_err());
        assert_eq!(
            fs::read(target.join(MANIFEST_NAME)).expect("current"),
            prior
        );

        let linked = root.path().join("linked-source");
        symlink(&second, &linked).expect("symlink");
        assert!(inspect_update_source(&linked).is_err());

        let third = write_source(root.path(), "1.2.0");
        let artifact = third.join("Interface-1.2.0-arm64-mac.zip");
        fs::hard_link(&artifact, root.path().join("extra-hard-link")).expect("hard link");
        assert!(inspect_update_source(&third).is_err());
    }

    #[test]
    fn staging_preserves_the_live_feed_directory_identity() {
        let root = tempfile::tempdir().expect("root");
        let first = write_source(root.path(), "1.0.0");
        let target = root.path().join("feed");
        stage_update_feed(&first, &target, false).expect("first stage");
        let pinned = File::open(&target).expect("pin live feed directory");
        let before = pinned.metadata().expect("pinned metadata");

        let second = write_source(root.path(), "1.1.0");
        stage_update_feed(&second, &target, false).expect("upgrade");

        let opened_after = pinned.metadata().expect("open directory after staging");
        let named_after = fs::symlink_metadata(&target).expect("named directory after staging");
        assert_eq!(
            (before.dev(), before.ino()),
            (opened_after.dev(), opened_after.ino())
        );
        assert_eq!(
            (before.dev(), before.ino()),
            (named_after.dev(), named_after.ino())
        );
        assert_eq!(
            inspect_update_source(&target)
                .expect("active feed")
                .manifest
                .version,
            "1.1.0"
        );
    }

    #[test]
    fn release_publication_preserves_unrelated_files_and_replaces_managed_set() {
        let root = tempfile::tempdir().expect("root");
        let release = root.path().join("release");
        let old_source = write_source(root.path(), "1.0.0");
        stage_update_feed(&old_source, &release, false).expect("old release");
        fs::write(release.join("keep.txt"), b"unrelated").expect("unrelated");
        fs::write(
            release.join("Interface-1.0.0-arm64-mac.zip.blockmap"),
            b"old map",
        )
        .expect("old blockmap");

        let new_source = write_source(root.path(), "1.1.0");
        for name in [
            "Interface-1.1.0-arm64-mac.zip.blockmap",
            "Interface-1.1.0-arm64.dmg.blockmap",
        ] {
            fs::write(new_source.join(name), b"new map").expect("new blockmap");
        }
        let plan = inspect_update_source(&new_source).expect("new plan");
        let blockmaps = optional_blockmaps(&plan).expect("blockmaps");
        let result =
            publish_release_artifacts(&plan, &blockmaps, &release, false).expect("publication");
        assert_eq!(result.version, "1.1.0");
        assert_eq!(
            fs::read(release.join("keep.txt")).expect("keep"),
            b"unrelated"
        );
        assert!(!release.join("Interface-1.0.0-arm64-mac.zip").exists());
        assert!(
            !release
                .join("Interface-1.0.0-arm64-mac.zip.blockmap")
                .exists()
        );
        assert!(release.join("Interface-1.1.0-arm64-mac.zip").exists());
        assert!(release.join("Interface-1.1.0-arm64.dmg.blockmap").exists());
        assert_eq!(
            inspect_update_source(&release)
                .expect("published feed")
                .manifest
                .version,
            "1.1.0"
        );
    }

    #[test]
    fn release_publish_failure_rolls_back_manifest_and_artifacts() {
        let root = tempfile::tempdir().expect("root");
        let release = root.path().join("release");
        let old_source = write_source(root.path(), "1.0.0");
        stage_update_feed(&old_source, &release, false).expect("old release");
        fs::write(release.join("keep.txt"), b"unrelated").expect("unrelated");
        let prior_manifest = fs::read(release.join(MANIFEST_NAME)).expect("prior manifest");

        let new_source = write_source(root.path(), "1.1.0");
        let plan = inspect_update_source(&new_source).expect("new plan");
        let calls = Cell::new(0_u8);
        let result =
            publish_release_artifacts_unlocked(&plan, &[], &release, false, true, |from, to| {
                calls.set(calls.get() + 1);
                if calls.get() == 5 {
                    return Err(io::Error::other("injected mid-publish failure"));
                }
                fs::rename(from, to)
            });
        assert!(result.is_err());
        assert_eq!(
            fs::read(release.join(MANIFEST_NAME)).expect("restored manifest"),
            prior_manifest
        );
        assert_eq!(
            inspect_update_source(&release)
                .expect("restored feed")
                .manifest
                .version,
            "1.0.0"
        );
        assert_eq!(
            fs::read(release.join("keep.txt")).expect("keep"),
            b"unrelated"
        );
        assert!(!release.join("Interface-1.1.0-arm64-mac.zip").exists());
        assert!(fs::read_dir(&release).expect("release").all(|entry| {
            !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(TRANSACTION_PREFIX)
        }));
    }

    #[test]
    fn recovery_restores_a_partially_moved_artifact_under_the_old_manifest() {
        let root = tempfile::tempdir().expect("root");
        let release = root.path().join("release");
        let old_source = write_source(root.path(), "1.0.0");
        stage_update_feed(&old_source, &release, false).expect("old release");
        let old_plan = inspect_update_source(&release).expect("old plan");
        let new_source = write_source(root.path(), "1.1.0");
        let new_plan = inspect_update_source(&new_source).expect("new plan");

        let transaction = release.join(format!("{TRANSACTION_PREFIX}partial-backup"));
        let next = transaction.join("next");
        let previous = transaction.join("previous");
        fs::create_dir_all(&next).expect("next");
        fs::create_dir(&previous).expect("previous");
        for artifact in &new_plan.artifacts {
            fs::copy(&artifact.source_path, next.join(&artifact.manifest.name))
                .expect("next artifact");
        }
        fs::write(next.join(MANIFEST_NAME), &new_plan.manifest_bytes).expect("next manifest");

        let moved_name = &old_plan.artifacts[0].manifest.name;
        fs::rename(release.join(moved_name), previous.join(moved_name))
            .expect("partially persisted backup");
        let journal = RecoveryJournal {
            version: 1,
            old_names: std::iter::once(MANIFEST_NAME.to_owned())
                .chain(
                    old_plan
                        .artifacts
                        .iter()
                        .map(|artifact| artifact.manifest.name.clone()),
                )
                .collect(),
            new_names: std::iter::once(MANIFEST_NAME.to_owned())
                .chain(
                    new_plan
                        .artifacts
                        .iter()
                        .map(|artifact| artifact.manifest.name.clone()),
                )
                .collect(),
            old_manifest_digest: Some(hex_digest(&old_plan.manifest_bytes)),
            new_manifest_digest: hex_digest(&new_plan.manifest_bytes),
        };
        let mut bytes = serde_json::to_vec(&journal).expect("journal");
        bytes.push(b'\n');
        fs::write(transaction.join(JOURNAL_NAME), bytes).expect("journal file");

        assert_eq!(
            recover_release_transactions_unlocked(&release).expect("recovery"),
            1
        );
        assert_eq!(
            inspect_update_source(&release)
                .expect("restored release")
                .manifest
                .version,
            "1.0.0"
        );
        assert!(!transaction.exists());
    }

    #[test]
    fn interrupted_release_transaction_recovers_prior_manifest_last() {
        let root = tempfile::tempdir().expect("root");
        let release = root.path().join("release");
        let old_source = write_source(root.path(), "1.0.0");
        stage_update_feed(&old_source, &release, false).expect("old release");
        let old_plan = inspect_update_source(&release).expect("old plan");
        let new_source = write_source(root.path(), "1.1.0");
        let new_plan = inspect_update_source(&new_source).expect("new plan");

        let transaction = release.join(format!("{TRANSACTION_PREFIX}fixture"));
        let next = transaction.join("next");
        let previous = transaction.join("previous");
        fs::create_dir_all(&next).expect("next");
        fs::create_dir(&previous).expect("previous");
        for name in std::iter::once(MANIFEST_NAME.to_owned()).chain(
            old_plan
                .artifacts
                .iter()
                .map(|item| item.manifest.name.clone()),
        ) {
            fs::rename(release.join(&name), previous.join(&name)).expect("backup old");
        }
        let published_name = &new_plan.artifacts[0].manifest.name;
        fs::copy(
            &new_plan.artifacts[0].source_path,
            release.join(published_name),
        )
        .expect("partial publish");
        for artifact in new_plan.artifacts.iter().skip(1) {
            fs::copy(&artifact.source_path, next.join(&artifact.manifest.name))
                .expect("next artifact");
        }
        fs::write(next.join(MANIFEST_NAME), &new_plan.manifest_bytes).expect("next manifest");
        let journal = RecoveryJournal {
            version: 1,
            old_names: std::iter::once(MANIFEST_NAME.to_owned())
                .chain(
                    old_plan
                        .artifacts
                        .iter()
                        .map(|item| item.manifest.name.clone()),
                )
                .collect(),
            new_names: std::iter::once(MANIFEST_NAME.to_owned())
                .chain(
                    new_plan
                        .artifacts
                        .iter()
                        .map(|item| item.manifest.name.clone()),
                )
                .collect(),
            old_manifest_digest: Some(hex_digest(&old_plan.manifest_bytes)),
            new_manifest_digest: hex_digest(&new_plan.manifest_bytes),
        };
        let mut bytes = serde_json::to_vec(&journal).expect("journal");
        bytes.push(b'\n');
        fs::write(transaction.join(JOURNAL_NAME), bytes).expect("journal");
        fs::write(transaction.join(BACKUP_COMPLETE_NAME), b"ok\n").expect("marker");

        assert_eq!(
            recover_release_transactions_unlocked(&release).expect("recover"),
            1
        );
        assert_eq!(
            inspect_update_source(&release)
                .expect("restored")
                .manifest
                .version,
            "1.0.0"
        );
        assert!(!transaction.exists());
        assert!(!release.join(published_name).exists());
    }

    #[test]
    fn selected_update_artifacts_accepts_zip_only_manifest() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("source-zip-only");
        fs::create_dir(&source).expect("source");
        let zip_name = "Interface-1.0.0-arm64-mac.zip";
        let zip = b"zip-1.0.0";
        fs::write(source.join(zip_name), zip).expect("zip");
        fs::write(
            source.join(MANIFEST_NAME),
            manifest("1.0.0", &[(zip_name, zip)]),
        )
        .expect("manifest");
        let plan = inspect_update_source(&source).expect("plan");
        let selected = selected_update_artifacts(&plan).expect("zip-only selected");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].manifest.name, zip_name);
    }
}
