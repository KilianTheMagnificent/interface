//! The publisher's side of the public update channel.
//!
//! `update-key` makes the Ed25519 key that signs release manifests. The
//! private half stays in `~/.config/interface-release` on this Mac; the
//! public half goes into `crates/interface-updater/release-channel.json` and
//! so into every build. `publish-update` signs the release that is staged in
//! the local feed, rehearses it against the same code a published app runs,
//! and only then writes or uploads it.
//!
//! Nothing here runs as part of `promote`: putting a release in front of
//! every installed copy is its own step, asked for by name.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use interface_common::time::{IsoFraction, unix_seconds_iso};
use interface_updater::{
    CHANNEL_STATE_NAME, FetchError, MANIFEST_SCHEMA, MirrorOutcome, MirrorPlan, PublicKey,
    ReleaseArtifact, ReleaseChannel, ReleaseManifest, RemoteFetcher, SIGNED_MANIFEST_NAME,
    UreqFetcher, generate_signing_key, refresh_mirror, sign_manifest, signing_public_key,
    verify_signed_manifest,
};

use crate::release;

const CHANNEL_FILE: &str = "crates/interface-updater/release-channel.json";
const KEY_DIRECTORY: &str = ".config/interface-release";
/// A version no release is older than: the rehearsal's "installed" app.
const OLDEST_VERSION: &str = "0.0.0";

pub struct PublishRequest {
    pub source: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub key: String,
    pub github: Option<String>,
    pub notes: Option<String>,
    pub allow_unnotarized: bool,
}

/// Makes a signing key, keeps its private half on this Mac and, when asked,
/// lists its public half in the channel file.
pub fn new_key(root: &Path, name: &str, add_to_channel: bool) -> Result<()> {
    let path = key_path(name)?;
    ensure!(
        !path.exists(),
        "A key named {name} already exists at {}. A key that builds already trust must never be replaced; make another with --name.",
        path.display()
    );
    let (pkcs8, public) = generate_signing_key().context("could not make a signing key")?;
    let directory = path.parent().context("key path has no parent")?;
    if let Some(parent) = directory.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    crate::pipeline_state::private_directory(directory)
        .with_context(|| format!("could not create {}", directory.display()))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    writeln!(file, "{}", BASE64.encode(&pkcs8))?;
    file.sync_all()?;

    println!("Made update signing key \"{name}\".");
    println!(
        "  Private key (keep, back up, never share): {}",
        path.display()
    );
    println!("  Public key: {}", public.to_base64());
    println!("  Key id:     {}", public.id());
    if add_to_channel {
        add_key_to_channel(&root.join(CHANNEL_FILE), &public)?;
        println!("  Added to {CHANNEL_FILE}. Builds made from now on trust it.");
    } else {
        println!(
            "  Not yet trusted by any build: rerun with --add-to-channel, or add the public key to {CHANNEL_FILE}."
        );
    }
    println!(
        "Back the private key file up somewhere that is not this Mac. Without it, and without a second key that builds already trust, no installed copy can ever be updated again."
    );
    Ok(())
}

/// Signs the staged release, proves a published app would take it, and
/// writes the feed folder; with `github`, uploads it and checks it live.
pub fn publish(root: &Path, request: &PublishRequest) -> Result<()> {
    let channel = channel(root)?.context(
        "The public update channel is off: crates/interface-updater/release-channel.json lists no feed or no key. See docs/PUBLIC_UPDATES.md.",
    )?;
    let source = request
        .source
        .clone()
        .map_or_else(release::default_feed_target, Ok)?;
    let plan = release::inspect_update_source(&source)?;
    release::verify_update_source_checksums(&plan)?;
    ensure!(
        plan.artifacts.len() == 1,
        "The public feed carries one ZIP; {} lists {} files.",
        release::MANIFEST_NAME,
        plan.artifacts.len()
    );
    let artifact = &plan.artifacts[0];
    let version = plan.manifest.version.clone();
    assert_publishable(&artifact.source_path, request.allow_unnotarized)?;

    let manifest = ReleaseManifest {
        schema: MANIFEST_SCHEMA,
        product: channel.product().to_owned(),
        channel: channel.channel().to_owned(),
        version: version.clone(),
        published_at: unix_seconds_iso(
            i64::try_from(crate::pipeline_state::now()).unwrap_or(0),
            IsoFraction::Zeros,
        ),
        artifact: ReleaseArtifact {
            name: artifact.manifest.name.clone(),
            size: artifact.manifest.size,
            sha512: artifact.manifest.sha512.clone(),
        },
        latest_mac_yml: String::from_utf8(plan.manifest_bytes.clone())
            .context("latest-mac.yml is not UTF-8")?,
    };
    let pkcs8 = read_key(&request.key)?;
    let signed = sign_manifest(&manifest, &pkcs8)
        .map_err(|error| anyhow::anyhow!("could not sign the release: {error}"))?;
    verify_signed_manifest(&signed, channel.keys(), channel.product(), channel.channel())
        .map_err(|_| {
            anyhow::anyhow!(
                "Key \"{}\" is not one {CHANNEL_FILE} trusts, so every installed copy would refuse this release. Sign with a trusted key.",
                request.key
            )
        })?;

    let out = request
        .out
        .clone()
        .unwrap_or_else(|| root.join("release").join("public-feed").join(&version));
    fs::create_dir_all(&out)?;
    let out = out.canonicalize()?;
    let archive = out.join(&manifest.artifact.name);
    if archive != artifact.source_path {
        let _ = fs::remove_file(&archive);
        fs::copy(&artifact.source_path, &archive)
            .with_context(|| format!("could not copy the archive to {}", out.display()))?;
    }
    fs::write(out.join(SIGNED_MANIFEST_NAME), &signed)?;

    // The folder is now exactly what a host would serve. Take it the way a
    // published app would: signature, download, size, SHA-512, staging.
    rehearse(
        &channel,
        &DirectoryFeed {
            directory: out.clone(),
        },
        &manifest,
    )
    .context("a published app would not accept this feed folder")?;
    println!(
        "Signed Interface {version} with key \"{}\" and rehearsed it.",
        request.key
    );
    println!("  Feed folder: {}", out.display());
    println!("  {SIGNED_MANIFEST_NAME}");
    println!("  {}", manifest.artifact.name);

    let Some(repository) = request.github.as_deref() else {
        println!(
            "Not uploaded. Upload both files to the folder the feed address points at, or rerun with --github <owner>/<repo>."
        );
        return Ok(());
    };
    upload_to_github(
        root,
        &channel,
        repository,
        request.notes.as_deref(),
        &out,
        &manifest,
    )
}

/// Uploads a rehearsed feed folder as a GitHub release and reads it back
/// through the live feed.
fn upload_to_github(
    root: &Path,
    channel: &ReleaseChannel,
    repository: &str,
    notes: Option<&str>,
    out: &Path,
    manifest: &ReleaseManifest,
) -> Result<()> {
    let version = manifest.version.as_str();
    ensure!(
        is_repository(repository),
        "--github takes <owner>/<repo>, for example interface-app/releases."
    );
    let expected_feed = format!("https://github.com/{repository}/releases/latest/download/");
    ensure!(
        channel
            .feeds()
            .iter()
            .any(|feed| feed.as_str().starts_with(&expected_feed)),
        "No feed in {CHANNEL_FILE} reads from {expected_feed}, so installed copies would never see a release uploaded there."
    );
    if let Some(live) = live_manifest(channel) {
        ensure!(
            release::compare_versions(version, &live.version)? == std::cmp::Ordering::Greater,
            "Interface {} is already published; {version} is not newer.",
            live.version
        );
    }
    let notes = notes.map_or_else(|| format!("Interface {version}"), str::to_owned);
    for arguments in github_commands(repository, version, &notes, out, &manifest.artifact.name) {
        let status = Command::new("gh")
            .args(&arguments)
            .stdin(Stdio::null())
            .status()
            .context("could not run gh")?;
        ensure!(status.success(), "gh {} failed", arguments.join(" "));
    }
    check_live(root, true)?;
    println!("Published Interface {version}. Installed copies pick it up within four hours.");
    Ok(())
}

/// Reads the live feed the way a published app does and says what it offers.
pub fn check_live(root: &Path, download: bool) -> Result<()> {
    let channel = channel(root)?.context("The public update channel is off.")?;
    let fetcher = UreqFetcher::new();
    for feed in channel.feeds() {
        let bytes = match fetcher.fetch(feed, interface_updater::MAX_SIGNED_MANIFEST_BYTES as u64) {
            Ok(bytes) => bytes,
            Err(FetchError::Missing) => {
                println!("{feed}\n  Nothing is published there yet.");
                continue;
            }
            Err(error) => bail!("{feed} could not be read: {error:?}"),
        };
        let manifest =
            verify_signed_manifest(&bytes, channel.keys(), channel.product(), channel.channel())
                .map_err(|error| anyhow::anyhow!("{feed}: {error}"))?;
        println!(
            "{feed}\n  Interface {} ({}, {} bytes), signed by a trusted key.",
            manifest.version, manifest.artifact.name, manifest.artifact.size
        );
    }
    if download {
        let manifest = live_manifest(&channel).context("no feed offers a release")?;
        rehearse(&channel, &fetcher, &manifest)
            .context("the live feed's archive is not the signed one")?;
        println!("  The archive downloads and matches its signed size and SHA-512.");
    }
    Ok(())
}

fn live_manifest(channel: &ReleaseChannel) -> Option<ReleaseManifest> {
    let fetcher = UreqFetcher::new();
    channel.feeds().iter().find_map(|feed| {
        let bytes = fetcher
            .fetch(feed, interface_updater::MAX_SIGNED_MANIFEST_BYTES as u64)
            .ok()?;
        verify_signed_manifest(&bytes, channel.keys(), channel.product(), channel.channel()).ok()
    })
}

/// Runs the installed app's own update check against `fetcher`, as a copy
/// that has never updated, into a throwaway data folder.
fn rehearse(
    channel: &ReleaseChannel,
    fetcher: &dyn RemoteFetcher,
    expected: &ReleaseManifest,
) -> Result<()> {
    let data = tempfile::tempdir()?;
    let feed_dir = data.path().join("update-feed");
    fs::create_dir(&feed_dir)?;
    let plan = MirrorPlan {
        channel: channel.clone(),
        feed_dir: feed_dir.clone(),
        state_path: data.path().join(CHANNEL_STATE_NAME),
        current_version: OLDEST_VERSION.to_owned(),
    };
    let outcome = refresh_mirror(&plan, fetcher, &mut |_| {}, &AtomicBool::new(false))
        .map_err(|error| anyhow::anyhow!("the update check failed: {error:?}"))?;
    ensure!(
        outcome == MirrorOutcome::Staged(expected.version.clone()),
        "the update check did not take Interface {}: {outcome:?}",
        expected.version
    );
    let staged = release::inspect_update_source(&feed_dir)?;
    release::verify_update_source_checksums(&staged)?;
    ensure!(
        staged.manifest_bytes == expected.latest_mac_yml.as_bytes(),
        "the staged installer feed differs from the signed one"
    );
    Ok(())
}

/// A feed folder on disk, served to the rehearsal by file name.
struct DirectoryFeed {
    directory: PathBuf,
}

impl DirectoryFeed {
    fn open(&self, url: &url::Url) -> Result<fs::File, FetchError> {
        let name = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .ok_or(FetchError::Unreachable)?;
        fs::File::open(self.directory.join(name)).map_err(|_| FetchError::Unreachable)
    }
}

impl RemoteFetcher for DirectoryFeed {
    fn fetch(&self, url: &url::Url, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        let mut bytes = Vec::new();
        self.open(url)?
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| FetchError::Unreachable)?;
        if bytes.len() as u64 > max_bytes {
            return Err(FetchError::TooLarge);
        }
        Ok(bytes)
    }

    fn download(
        &self,
        url: &url::Url,
        expected_bytes: u64,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), FetchError> {
        let mut file = self.open(url)?;
        let mut buffer = vec![0_u8; 1024 * 1024].into_boxed_slice();
        let mut sent = 0_u64;
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| FetchError::Unreachable)?;
            if count == 0 {
                break;
            }
            sent = sent.saturating_add(count as u64);
            if sent > expected_bytes {
                return Err(FetchError::TooLarge);
            }
            sink(&buffer[..count]).map_err(|_| FetchError::Stopped)?;
        }
        if sent == expected_bytes {
            Ok(())
        } else {
            Err(FetchError::Unreachable)
        }
    }
}

/// The two `gh` calls of a release: created as a draft with both files, then
/// published as the latest, so the feed never names an archive that is not
/// there yet.
fn github_commands(
    repository: &str,
    version: &str,
    notes: &str,
    out: &Path,
    archive: &str,
) -> Vec<Vec<String>> {
    let tag = format!("v{version}");
    vec![
        vec![
            "release".to_owned(),
            "create".to_owned(),
            tag.clone(),
            out.join(SIGNED_MANIFEST_NAME)
                .to_string_lossy()
                .into_owned(),
            out.join(archive).to_string_lossy().into_owned(),
            "--repo".to_owned(),
            repository.to_owned(),
            "--title".to_owned(),
            format!("Interface {version}"),
            "--notes".to_owned(),
            notes.to_owned(),
            "--draft".to_owned(),
        ],
        vec![
            "release".to_owned(),
            "edit".to_owned(),
            tag,
            "--repo".to_owned(),
            repository.to_owned(),
            "--draft=false".to_owned(),
            "--latest".to_owned(),
        ],
    ]
}

fn is_repository(text: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    text.split_once('/')
        .is_some_and(|(owner, name)| part(owner) && part(name))
}

/// Every published release must be one installed copies can keep updating
/// from (`package::verify_public_signature`): hardened runtime, Apple's secure
/// timestamp, and a designated requirement the updater reads and that pins
/// the signing team. `--allow-unnotarized`, the normal mode while the app is
/// signed with a free Apple Development certificate, skips only the
/// notarization check.
fn assert_publishable(archive: &Path, allow_unnotarized: bool) -> Result<()> {
    let (_extraction, app) = extracted_app(archive)?;
    crate::package::verify_public_signature(&app).context(
        "Installed copies could not keep updating from this release, so it is not published. Package it again with `cargo xtask package-mac`.",
    )?;
    if !allow_unnotarized {
        assert_notarized(&app)?;
    }
    Ok(())
}

/// The app inside a release archive, extracted into a directory that lives as
/// long as the returned guard.
fn extracted_app(archive: &Path) -> Result<(tempfile::TempDir, PathBuf)> {
    let extraction = tempfile::tempdir()?;
    let unzipped = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(extraction.path())
        .stdin(Stdio::null())
        .status()?;
    ensure!(unzipped.success(), "could not open {}", archive.display());
    let app = fs::read_dir(extraction.path())?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .context("the archive holds no app")?;
    Ok((extraction, app))
}

/// Strangers can open the first download only if Apple notarized it. The
/// update path does not need this, but a feed whose release cannot be opened
/// fresh is not one to publish.
fn assert_notarized(app: &Path) -> Result<()> {
    let assessment = Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute", "-vv"])
        .arg(app)
        .stdin(Stdio::null())
        .output()?;
    let said = String::from_utf8_lossy(&assessment.stderr);
    ensure!(
        assessment.status.success() && said.contains("Notarized Developer ID"),
        "This release is not signed with a Developer ID and notarized by Apple, so macOS will not open a fresh download of it. Publish a notarized build, or pass --allow-unnotarized for people who know to open it by hand.\n{}",
        said.trim()
    );
    Ok(())
}

fn channel(root: &Path) -> Result<Option<ReleaseChannel>> {
    let text = fs::read_to_string(root.join(CHANNEL_FILE))
        .with_context(|| format!("could not read {CHANNEL_FILE}"))?;
    ReleaseChannel::parse(&text).map_err(|error| anyhow::anyhow!("{CHANNEL_FILE}: {error}"))
}

fn add_key_to_channel(path: &Path, key: &PublicKey) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let mut file: serde_json::Value = serde_json::from_str(&text)?;
    let keys = file
        .get_mut("keys")
        .and_then(serde_json::Value::as_array_mut)
        .context("the channel file has no keys list")?;
    let text = key.to_base64();
    ensure!(
        !keys.iter().any(|listed| listed.as_str() == Some(&text)),
        "That key is already listed."
    );
    keys.push(serde_json::Value::String(text));
    let mut bytes = serde_json::to_vec_pretty(&file)?;
    bytes.push(b'\n');
    ReleaseChannel::parse(std::str::from_utf8(&bytes)?)
        .map_err(|error| anyhow::anyhow!("the channel file would be invalid: {error}"))?;
    fs::write(path, bytes)?;
    Ok(())
}

fn key_path(name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty()
            && name.len() <= 32
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "A key name is lowercase letters, digits and hyphens."
    );
    Ok(crate::pipeline_state::home()?
        .join(KEY_DIRECTORY)
        .join(format!("update-key-{name}.pk8")))
}

fn read_key(name: &str) -> Result<Vec<u8>> {
    let path = key_path(name)?;
    let metadata = fs::symlink_metadata(&path).with_context(|| {
        format!(
            "There is no signing key \"{name}\" at {}. Make one with `cargo xtask update-key`.",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "{} must be a plain file only you can read (chmod 600).",
            path.display()
        );
    }
    let pkcs8 = BASE64
        .decode(fs::read_to_string(&path)?.trim())
        .context("the signing key file is damaged")?;
    signing_public_key(&pkcs8).map_err(|_| anyhow::anyhow!("the signing key file is damaged"))?;
    Ok(pkcs8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCHIVE: &[u8] = b"a zip of Interface.app";

    fn feed_folder(version: &str) -> (tempfile::TempDir, ReleaseManifest) {
        use sha2::{Digest as _, Sha512};
        let folder = tempfile::tempdir().expect("folder");
        let name = format!("Interface-{version}-arm64-mac.zip");
        let sha512 = BASE64.encode(Sha512::digest(ARCHIVE));
        let size = ARCHIVE.len();
        let yml = format!(
            "version: {version}\nfiles:\n  - url: {name}\n    sha512: {sha512}\n    size: {size}\npath: {name}\nsha512: {sha512}\nreleaseDate: '2026-11-21T18:00:00.000Z'\n"
        );
        fs::write(folder.path().join(&name), ARCHIVE).expect("archive");
        let manifest = ReleaseManifest {
            schema: MANIFEST_SCHEMA,
            product: "com.example.interface".to_owned(),
            channel: "stable".to_owned(),
            version: version.to_owned(),
            published_at: "2026-11-21T18:00:00.000Z".to_owned(),
            artifact: ReleaseArtifact {
                name,
                size: size as u64,
                sha512,
            },
            latest_mac_yml: yml,
        };
        (folder, manifest)
    }

    fn channel_trusting(key: &PublicKey) -> ReleaseChannel {
        ReleaseChannel::loopback(
            "com.example.interface",
            "stable",
            &["http://127.0.0.1:9/releases/interface-update.json"],
            &[*key],
        )
        .expect("channel")
    }

    #[test]
    fn a_signed_feed_folder_is_taken_by_the_installed_apps_own_check() {
        let (folder, manifest) = feed_folder("0.4.0");
        let (pkcs8, key) = generate_signing_key().expect("key");
        fs::write(
            folder.path().join(SIGNED_MANIFEST_NAME),
            sign_manifest(&manifest, &pkcs8).expect("signed"),
        )
        .expect("manifest");
        let feed = DirectoryFeed {
            directory: folder.path().to_path_buf(),
        };
        rehearse(&channel_trusting(&key), &feed, &manifest).expect("rehearsal");
    }

    #[test]
    fn the_rehearsal_fails_a_folder_an_installed_app_would_refuse() {
        let (folder, manifest) = feed_folder("0.4.0");
        let (pkcs8, key) = generate_signing_key().expect("key");
        let signed = sign_manifest(&manifest, &pkcs8).expect("signed");
        fs::write(folder.path().join(SIGNED_MANIFEST_NAME), &signed).expect("manifest");
        let feed = DirectoryFeed {
            directory: folder.path().to_path_buf(),
        };

        let (_, stranger) = generate_signing_key().expect("key");
        assert!(rehearse(&channel_trusting(&stranger), &feed, &manifest).is_err());

        fs::write(
            folder.path().join(&manifest.artifact.name),
            b"a different zip here!!",
        )
        .expect("swap");
        assert!(rehearse(&channel_trusting(&key), &feed, &manifest).is_err());
    }

    #[test]
    fn a_release_is_uploaded_as_a_draft_and_only_then_made_the_latest() {
        let commands = github_commands(
            "interface-app/releases",
            "0.4.0",
            "What changed",
            Path::new("/feed"),
            "Interface-0.4.0-arm64-mac.zip",
        );
        assert_eq!(
            commands,
            [
                vec![
                    "release",
                    "create",
                    "v0.4.0",
                    "/feed/interface-update.json",
                    "/feed/Interface-0.4.0-arm64-mac.zip",
                    "--repo",
                    "interface-app/releases",
                    "--title",
                    "Interface 0.4.0",
                    "--notes",
                    "What changed",
                    "--draft",
                ],
                vec![
                    "release",
                    "edit",
                    "v0.4.0",
                    "--repo",
                    "interface-app/releases",
                    "--draft=false",
                    "--latest",
                ],
            ]
        );
    }

    #[test]
    fn a_repository_is_an_owner_and_a_name_and_nothing_else() {
        assert!(is_repository("interface-app/releases"));
        for refused in ["releases", "a/b/c", "/b", "a/", "a b/c", "a/b --draft"] {
            assert!(!is_repository(refused), "{refused}");
        }
    }

    #[test]
    fn a_new_key_joins_the_channel_file_once() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("release-channel.json");
        fs::write(
            &path,
            r#"{"schema":1,"product":"com.example.interface","channel":"stable","feeds":["https://updates.example.test/interface-update.json"],"keys":[]}"#,
        )
        .expect("channel file");
        let (_, key) = generate_signing_key().expect("key");
        add_key_to_channel(&path, &key).expect("added");
        let channel = ReleaseChannel::parse(&fs::read_to_string(&path).expect("read"))
            .expect("valid")
            .expect("on");
        assert_eq!(channel.keys(), [key]);
        assert!(add_key_to_channel(&path, &key).is_err());
    }

    #[test]
    fn key_names_cannot_leave_the_key_folder() {
        assert!(key_path("primary").is_ok());
        for refused in ["", "../x", "a/b", "Primary", "a b"] {
            assert!(key_path(refused).is_err(), "{refused}");
        }
    }

    fn zipped(app: &Path) -> PathBuf {
        let archive = app.with_extension("zip");
        let status = Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(app)
            .arg(&archive)
            .status()
            .expect("ditto runs");
        assert!(status.success());
        archive
    }

    #[test]
    fn an_unnotarized_release_must_still_be_one_installed_copies_update_from() {
        let root = tempfile::tempdir().expect("root");
        let plain = zipped(&crate::test_fixtures::ad_hoc_app(
            &root.path().join("plain"),
            None,
        ));
        let hardened = zipped(&crate::test_fixtures::ad_hoc_app(
            &root.path().join("hardened"),
            Some("runtime"),
        ));
        for allow_unnotarized in [true, false] {
            let error = format!(
                "{:#}",
                assert_publishable(&plain, allow_unnotarized).expect_err("unhardened")
            );
            assert!(
                error.contains("could not keep updating") && error.contains("hardened runtime"),
                "{error}"
            );
            let error = format!(
                "{:#}",
                assert_publishable(&hardened, allow_unnotarized).expect_err("untimestamped")
            );
            assert!(error.contains("secure timestamp"), "{error}");
        }
    }
}
