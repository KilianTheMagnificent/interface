use std::{
    env,
    ffi::OsStr,
    fmt,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use interface_common::version::{PRODUCT_VERSION_VARIABLE, is_product_version};
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};
use tempfile::Builder;

use std::os::unix::fs::PermissionsExt as _;

use crate::fsx::{self, FileState};
use crate::release::{
    ReleasePublication, UpdateSource, extension_is, inspect_update_source, optional_blockmaps,
    preflight_release_directory, publish_release_artifacts, selected_update_artifacts,
    verify_update_source_checksums,
};

const APP_NAME: &str = "Interface.app";
const BUNDLE_IDENTIFIER: &str = "com.andreromero.interface";
const CODESIGN: &str = "/usr/bin/codesign";
pub const PRIVATE_TEMP_PARENT: &str = "/private/tmp";
const PACKAGE_TEMP_PREFIX: &str = "interface-rust-package-";
const ZIP_TEMP_PREFIX: &str = "interface-rust-zip-verify-";
const MAX_PACKAGE_JSON_BYTES: u64 = 1024 * 1024;
const EXPECTED_BINARIES: [&str; 3] = ["interface-host", "iface", "interface-voice"];
const VOICE_HELPER: &str = "interface-voice";
pub(crate) const VOICE_PACKAGE_DIR: &str = "apps/interface-voice";
pub(crate) const XCRUN: &str = "/usr/bin/xcrun";
const VOICE_SDK_MAJOR: u32 = 26;
const MACHO_64_MAGIC: u32 = 0xfeed_facf;
const LC_SEGMENT_64: u32 = 0x19;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const CHROME_INFO_PLIST: &str = "/Applications/Google Chrome.app/Contents/Info.plist";
/// Everything `scripts/make-icon.mjs` reads or writes, relative to `electron/`.
const ICON_FILES: [&str; 5] = [
    "scripts/make-icon.mjs",
    "build/icon.svg",
    "build/icon-dev.svg",
    "build/icon.icns",
    "build/icon-dev.icns",
];
const ICON_STAMP: &str = "target/xtask/icon-stamp";
pub(crate) const SMOKE_TAIL_LINES: usize = 40;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPackage {
    pub version: String,
    pub unpacked_apps: usize,
    pub dmgs: usize,
    pub zips: usize,
    pub smoke_runs: usize,
}

/// `Interface 0.3.140 (1 app, 1 DMG, 1 ZIP, 1 smoke run)`, as package-mac and
/// verify-package report it.
impl fmt::Display for VerifiedPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Interface {} ({} app, {} DMG, {} ZIP, {} smoke run)",
            self.version, self.unpacked_apps, self.dmgs, self.zips, self.smoke_runs
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageResult {
    pub verified: VerifiedPackage,
    pub publication: ReleasePublication,
}

/// How packaging treats the gate for the checkout's source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateTrust {
    /// Run the full gate.
    Run,
    /// Skip it when receipts cover both suites, then rebuild the renderer.
    Receipts,
    /// As `Receipts`, and the Electron suite (`npm run check`, ending in
    /// `npm run build`) just passed in this checkout: its build is reused.
    ReceiptsAndBuild,
}

#[derive(Debug, Deserialize)]
struct PackageJson {
    version: String,
}

pub fn package_mac(
    root: &Path,
    allow_same_version: bool,
    smoke: bool,
    gate: GateTrust,
) -> Result<PackageResult> {
    ensure!(
        env::consts::OS == "macos",
        "macOS packaging requires macOS."
    );
    let root = fs::canonicalize(root).context("could not resolve workspace root")?;
    // The receipt at the end refuses a dirty tree too, but only after the gate,
    // the signed build and the smoke, and after release/ was replaced.
    ensure!(
        crate::gate::tree_is_clean(&root)?,
        "package-mac needs a clean committed tree"
    );
    let candidate = crate::gate::rev_parse(&root, "HEAD")?;
    let electron = root.join("electron");
    let release = root.join("release");
    let temp_parent = fs::canonicalize(PRIVATE_TEMP_PARENT)
        .context("could not resolve private temporary directory")?;
    ensure!(
        !temp_parent.starts_with(&root) && !root.starts_with(&temp_parent),
        "Private package output must not overlap the project."
    );

    // The gate is skipped only when source/toolchain receipts prove both suites
    // passed. Only the application version fields are normalized.
    match (gate != GateTrust::Run)
        .then(|| crate::gate::gate_skip_reason(&root, &candidate))
        .transpose()?
        .flatten()
    {
        Some(reason) => {
            println!("package-mac: gate skipped — {reason}");
            // A receipt proves source was tested, not that this checkout's
            // ignored build output exists or belongs to the same revision —
            // unless that suite just ran here, exactly as the gate arm below trusts.
            if gate == GateTrust::ReceiptsAndBuild {
                println!(
                    "package-mac: renderer build reused from the Electron suite that just ran"
                );
            } else {
                run_command("npm", ["run", "build"], &electron)?;
            }
        }
        None => crate::gate::run_full_gate(&root)?,
    }
    let version = read_package_version(&electron.join("package.json"))?;
    preflight_release_directory(&release, &version, allow_same_version)?;
    let cargo_target = cargo_target_dir(&root)?;
    build_stamped_release(&root, &cargo_target, &version)?;
    build_voice_helper(&root, "release")?;
    build_icons(&root, &electron)?;

    let output = Builder::new()
        .prefix(PACKAGE_TEMP_PREFIX)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&temp_parent)
        .context("could not create private package output")?;
    let builder = electron
        .join("node_modules")
        .join("electron-builder")
        .join("cli.js");
    ensure!(builder.is_file(), "electron-builder is not installed.");
    let output_override = format!("--config.directories.output={}", output.path().display());
    // electron-builder resolves `from` against its project before expanding
    // macros. An absolute macro value would be appended to that project path.
    let target_dir = relative_resource_dir(&electron, &cargo_target)?;
    run_command_env(
        env::var_os("NODE").unwrap_or_else(|| "node".into()),
        [
            builder.as_os_str(),
            OsStr::new(&output_override),
            OsStr::new("--config"),
            OsStr::new("electron-builder.yml"),
            OsStr::new("--publish"),
            OsStr::new("never"),
        ],
        &electron,
        &[("INTERFACE_TARGET_DIR", target_dir.as_os_str())],
    )?;

    let plan = inspect_update_source(output.path())?;
    ensure!(
        plan.manifest.version == version,
        "Packaged update version {} does not match package.json version {version}.",
        plan.manifest.version
    );
    let verified = verify_package_source(&root, &plan, smoke)?;
    let blockmaps = optional_blockmaps(&plan)?;
    let publication = publish_release_artifacts(&plan, &blockmaps, &release, allow_same_version)?;
    crate::artifact_receipt::record(&root, &candidate, verified.smoke_runs)?;
    Ok(PackageResult {
        verified,
        publication,
    })
}

pub fn verify_package_directory(
    root: &Path,
    source: &Path,
    smoke: bool,
) -> Result<VerifiedPackage> {
    ensure!(
        env::consts::OS == "macos",
        "macOS package verification requires macOS."
    );
    let root = fs::canonicalize(root).context("could not resolve workspace root")?;
    let plan = inspect_update_source(source)?;
    verify_package_source(&root, &plan, smoke)
}

/// Builds the shipped Rust binaries with the product version stamped in, so
/// `iface --version`, the host ready frame and the provider handshakes report
/// the release rather than the workspace crate version, then proves the stamp
/// reached them. Only this build sets the variable: the gate's builds, their
/// receipts and a version-only bump never see it.
fn build_stamped_release(root: &Path, cargo_target: &Path, version: &str) -> Result<()> {
    ensure!(
        is_product_version(version),
        "package.json version {version:?} cannot be stamped into the Rust binaries"
    );
    run_command_env(
        env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        [
            "build",
            "--release",
            "--package",
            "interface-host",
            "--package",
            "iface",
        ],
        root,
        &[(PRODUCT_VERSION_VARIABLE, OsStr::new(version))],
    )?;
    let iface = cargo_target.join("release").join("iface");
    let output = Command::new(&iface)
        .current_dir(root)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to launch {}", iface.display()))?;
    ensure!(
        output.status.success(),
        "{} --version failed with {}",
        iface.display(),
        output.status
    );
    ensure_stamped(&output.stdout, version)
}

/// `iface --version` output must be exactly the stamped version and a newline.
fn ensure_stamped(reported: &[u8], version: &str) -> Result<()> {
    ensure!(
        reported == format!("{version}\n").as_bytes(),
        "release iface reports version {:?}, not the package version {version}",
        String::from_utf8_lossy(reported).trim_end()
    );
    Ok(())
}

/// The directory Cargo actually builds into, including custom target paths.
/// `CARGO_TARGET_DIR` comes first because cargo obeys it over any config file.
fn cargo_target_dir(root: &Path) -> Result<PathBuf> {
    #[derive(Deserialize)]
    struct Metadata {
        target_directory: PathBuf,
    }

    if let Some(configured) = env::var_os("CARGO_TARGET_DIR").filter(|value| !value.is_empty()) {
        return Ok(root.join(configured));
    }
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(&cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to launch {}", cargo.to_string_lossy()))?;
    ensure!(
        output.status.success(),
        "cargo metadata failed with {}",
        output.status
    );
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("could not read cargo metadata")?;
    Ok(metadata.target_directory)
}

/// Express the actual build directory relative to electron-builder's project.
/// Resolve symlinks first so `..` cannot step through a different physical tree.
fn relative_resource_dir(project: &Path, target: &Path) -> Result<PathBuf> {
    let project = fs::canonicalize(project).context("could not resolve Electron project")?;
    let target = fs::canonicalize(target).context("could not resolve Cargo target directory")?;
    for (parents, base) in project.ancestors().enumerate() {
        if let Ok(suffix) = target.strip_prefix(base) {
            let mut relative: PathBuf = std::iter::repeat_n("..", parents).collect();
            relative.push(suffix);
            if relative.as_os_str().is_empty() {
                relative.push(".");
            }
            return Ok(relative);
        }
    }
    bail!("Electron project and Cargo target directory do not share a filesystem root.")
}

/// Builds the Swift voice helper and installs it beside the Rust binaries,
/// where `EXPECTED_BINARIES` and electron-builder's `extraResources` look for it.
pub(crate) fn build_voice_helper(root: &Path, profile: &str) -> Result<()> {
    let package = root.join(VOICE_PACKAGE_DIR);
    ensure!(
        package.join("Package.swift").is_file(),
        "Missing {}; {VOICE_HELPER} cannot be built.",
        package.join("Package.swift").display()
    );
    require_voice_sdk()?;
    // `Package.swift` embeds Info.plist from a package-relative linker path, so
    // the build runs with the package itself as the working directory.
    run_command(XCRUN, ["swift", "build", "-c", profile], &package)?;
    let built = package.join(".build").join(profile).join(VOICE_HELPER);
    let target = cargo_target_dir(root)?.join(profile);
    fs::create_dir_all(&target)
        .with_context(|| format!("could not create {}", target.display()))?;
    // Truncating a copy that is still running fails; replacing the name does not.
    let destination = target.join(VOICE_HELPER);
    fs::remove_file(&destination).ok();
    fs::copy(&built, &destination).with_context(|| {
        format!(
            "could not install {} at {}",
            built.display(),
            destination.display()
        )
    })?;
    Ok(())
}

/// The helper's `SpeechAnalyzer` needs the macOS 26 SDK. `xcrun` alone proves
/// nothing (it ships with the command line tools), so ask it for the SDK it
/// would build against and refuse before `swift build` fails obscurely.
fn require_voice_sdk() -> Result<()> {
    let reported = Command::new(XCRUN)
        .args(["--sdk", "macosx", "--show-sdk-version"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    ensure!(
        sdk_major_version(&reported).is_some_and(|major| major >= VOICE_SDK_MAJOR),
        "Xcode with the macOS {VOICE_SDK_MAJOR} SDK is required to build {VOICE_PACKAGE_DIR} \
         (`xcrun --sdk macosx --show-sdk-version` reported {reported:?})."
    );
    Ok(())
}

/// The major component of a `--show-sdk-version` line such as `26.5`.
fn sdk_major_version(reported: &str) -> Option<u32> {
    reported.trim().split('.').next()?.parse().ok()
}

#[allow(clippy::too_many_lines)]
fn verify_package_source(root: &Path, plan: &UpdateSource, smoke: bool) -> Result<VerifiedPackage> {
    verify_update_source_checksums(plan)?;
    let selected = selected_update_artifacts(plan)?;
    let apps = discover_mac_apps(&plan.source_dir)?;
    let dmgs: Vec<_> = selected
        .iter()
        .filter(|artifact| extension_is(&artifact.manifest.name, "dmg"))
        .map(|artifact| artifact.source_path.clone())
        .collect();
    let zips: Vec<_> = selected
        .iter()
        .filter(|artifact| extension_is(&artifact.manifest.name, "zip"))
        .map(|artifact| artifact.source_path.clone())
        .collect();
    let expected = expected_binary_paths(root)?;
    let [app] = apps.as_slice() else {
        bail!("The arm64 package must produce exactly one unpacked app.");
    };
    verify_app(root, app, &plan.manifest.version, &expected)?;
    let signed_binaries = embedded_binary_file_digests(app)?;
    for dmg in &dmgs {
        run_command_os(
            "/usr/bin/hdiutil",
            [OsStr::new("verify"), dmg.as_os_str()],
            root,
        )?;
    }
    for zip in &zips {
        run_command_os("/usr/bin/unzip", [OsStr::new("-tq"), zip.as_os_str()], root)?;
    }

    let temp_parent = fs::canonicalize(PRIVATE_TEMP_PARENT)?;
    let mut smoke_runs = 0_usize;
    for zip in &zips {
        let extraction = Builder::new()
            .prefix(ZIP_TEMP_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&temp_parent)
            .context("could not create ZIP verification directory")?;
        run_command_os(
            "/usr/bin/ditto",
            [
                OsStr::new("-x"),
                OsStr::new("-k"),
                zip.as_os_str(),
                extraction.path().as_os_str(),
            ],
            root,
        )?;
        let archived_app = exact_archived_app(extraction.path())?;
        verify_app(root, &archived_app, &plan.manifest.version, &expected)?;
        verify_embedded_binary_file_digests(&archived_app, &signed_binaries)?;
        if smoke {
            run_smoke(root, &archived_app)?;
            smoke_runs += 1;
        }
    }

    Ok(VerifiedPackage {
        version: plan.manifest.version.clone(),
        unpacked_apps: apps.len(),
        dmgs: dmgs.len(),
        zips: zips.len(),
        smoke_runs,
    })
}

/// The release checks every shipped app passes, the unpacked one and the one
/// extracted from the ZIP alike: bundle metadata, a strict deep signature, a
/// signature installed copies can keep updating from, the fuses, the exact
/// embedded binaries, and no bundled Claude CLI.
fn verify_app(
    root: &Path,
    app: &Path,
    version: &str,
    expected: &[(String, PathBuf, MachoContentDigest)],
) -> Result<()> {
    verify_bundle_metadata(app, version)?;
    run_command_os(
        CODESIGN,
        [
            OsStr::new("--verify"),
            OsStr::new("--deep"),
            OsStr::new("--strict"),
            OsStr::new("--verbose=2"),
            app.as_os_str(),
        ],
        root,
    )?;
    verify_public_signature(app)?;
    verify_fuses(root, app)?;
    verify_embedded_binaries(app, expected)?;
    assert_no_bundled_claude_binary(app)
}

/// What `codesign -d --verbose=2` reports about one piece of signed code.
#[derive(Debug, Default, Eq, PartialEq)]
struct SigningFacts {
    /// `runtime` among the code directory flags: the hardened runtime.
    runtime: bool,
    /// A `Timestamp=` line: Apple's secure timestamp, which keeps the
    /// signature valid after the certificate expires. Without one codesign
    /// prints only `Signed Time=`.
    timestamped: bool,
    team: Option<String>,
}

fn signing_facts(display: &str) -> SigningFacts {
    let mut facts = SigningFacts::default();
    for line in display.lines() {
        if let Some(code_directory) = line.strip_prefix("CodeDirectory ") {
            facts.runtime = code_directory
                .split_whitespace()
                .filter_map(|field| field.strip_prefix("flags="))
                .filter_map(|flags| flags.split_once('(')?.1.strip_suffix(')'))
                .any(|names| names.split(',').any(|name| name == "runtime"));
        } else if let Some(time) = line.strip_prefix("Timestamp=") {
            facts.timestamped = !time.trim().is_empty();
        } else if let Some(team) = line.strip_prefix("TeamIdentifier=") {
            let team = team.trim();
            facts.team = (!team.is_empty() && team != "not set").then(|| team.to_owned());
        }
    }
    facts
}

/// `identifier "<bundle id>" and anchor apple generic and certificate
/// leaf[subject.OU] = "<team>"`, exactly, as `scripts/sign-mac.cjs` signs it.
/// codesign prints the team unquoted when it starts with a letter.
fn is_team_pinned(requirement: &str, identifier: &str, team: &str) -> bool {
    requirement
        .strip_prefix(&format!(
            "identifier \"{identifier}\" and anchor apple generic and certificate leaf[subject.OU] = "
        ))
        .is_some_and(|value| value == team || value == format!("\"{team}\""))
}

/// Both of codesign's output streams for one display command.
fn codesign_display(path: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new(CODESIGN)
        .args(arguments)
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("failed to launch codesign")?;
    ensure!(
        output.status.success(),
        "codesign could not read the signature of {}",
        path.display()
    );
    let mut display = String::from_utf8_lossy(&output.stdout).into_owned();
    display.push('\n');
    display.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(display)
}

/// The Electron helpers and frameworks, then the first-party executables.
fn nested_signed_code(app: &Path) -> Result<Vec<PathBuf>> {
    let mut nested = Vec::new();
    let frameworks = app.join("Contents").join("Frameworks");
    for entry in fs::read_dir(&frameworks)
        .with_context(|| format!("could not list {}", frameworks.display()))?
    {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "app" || extension == "framework")
        {
            nested.push(path);
        }
    }
    nested.sort();
    let bin = app.join("Contents").join("Resources").join("bin");
    nested.extend(EXPECTED_BINARIES.iter().map(|name| bin.join(name)));
    Ok(nested)
}

/// A build every installed copy can keep updating from, without notarization:
/// the app and all code inside it carry the hardened runtime and Apple's
/// secure timestamp under one team, and the app's designated requirement is
/// one the updater can read (`interface_updater::parse_designated_requirement`)
/// and pins that team. An installed copy accepts an update only if it
/// satisfies this requirement, so a build that fails here would either turn
/// updates off for everyone who installs it or strand them at a certificate
/// renewal.
pub(crate) fn verify_public_signature(app: &Path) -> Result<()> {
    let name = app.file_name().map_or_else(
        || app.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let facts = signing_facts(&codesign_display(app, &["-d", "--verbose=2"])?);
    ensure!(
        facts.runtime,
        "{name} is not signed with the hardened runtime, so other programs on the Mac could load their own code into it."
    );
    ensure!(
        facts.timestamped,
        "{name} has no secure timestamp from Apple, so its signature would stop being valid when the signing certificate expires."
    );
    let team = facts
        .team
        .with_context(|| format!("{name} is not signed by an Apple team."))?;
    let requirement =
        interface_updater::parse_designated_requirement(&codesign_display(app, &["-d", "-r-"])?)
            .with_context(|| {
                format!("{name} carries a designated requirement the updater cannot read, so installed copies would turn updates off.")
            })?;
    ensure!(
        is_team_pinned(&requirement, BUNDLE_IDENTIFIER, &team),
        "{name} has a designated requirement that does not pin its signing team {team}, so installed copies would refuse updates once the certificate changes. Sign it with scripts/sign-mac.cjs."
    );
    for code in nested_signed_code(app)? {
        let nested = signing_facts(&codesign_display(&code, &["-d", "--verbose=2"])?);
        ensure!(
            nested.runtime && nested.timestamped && nested.team.as_deref() == Some(team.as_str()),
            "{} inside {name} is not signed with the hardened runtime, a secure timestamp and team {team}.",
            code.strip_prefix(app).unwrap_or(&code).display()
        );
    }
    Ok(())
}

/// `build:icon` re-renders the tracked icons through headless Chrome, 30-60 s,
/// and reproduces the committed bytes (the clean-tree check after packaging
/// would refuse otherwise). A render whose inputs, outputs, Chrome and macOS
/// all match the last one here would write those same bytes, so it is skipped.
fn build_icons(root: &Path, electron: &Path) -> Result<()> {
    let stamp = root.join(ICON_STAMP);
    let current = icon_stamp(electron, Path::new(CHROME_INFO_PLIST)).ok();
    if current.is_some() && fs::read_to_string(&stamp).ok() == current {
        println!(
            "package-mac: icons reused — sources, icons, Chrome and macOS match the last render"
        );
        return Ok(());
    }
    run_command("npm", ["run", "build:icon"], electron)?;
    if let Ok(rendered) = icon_stamp(electron, Path::new(CHROME_INFO_PLIST)) {
        fs::create_dir_all(stamp.parent().context("icon stamp has no parent")?)?;
        fs::write(&stamp, rendered)?;
    }
    Ok(())
}

/// A digest of every icon input and output plus the rasterizer's identity.
/// Any unreadable piece (no Chrome, no icons yet) means no stamp: render.
fn icon_stamp(electron: &Path, chrome_info_plist: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    for name in ICON_FILES {
        hash.update(name.as_bytes());
        hash.update(fs::read(electron.join(name))?);
    }
    hash.update(fs::read(chrome_info_plist)?);
    let macos = Command::new("/usr/bin/sw_vers")
        .arg("-buildVersion")
        .output()?;
    ensure!(macos.status.success(), "sw_vers failed");
    hash.update(macos.stdout);
    Ok(format!("{:x}", hash.finalize()))
}

/// The last lines of a failed smoke, kept in the error so a promote record says
/// why rather than only an exit status.
fn smoke_tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    lines[lines.len().saturating_sub(SMOKE_TAIL_LINES)..].join("\n")
}

/// Probes the archived app. A first attempt that failed on one of the known
/// timing races is retried once against that same verified app: nothing is
/// rebuilt or re-signed, so the retry proves exactly what the first run would.
fn run_smoke(root: &Path, app: &Path) -> Result<()> {
    let node = env::var_os("NODE").unwrap_or_else(|| "node".into());
    let script = root
        .join("electron")
        .join("scripts")
        .join("smoke-packaged.mjs");
    let arguments = [script.as_os_str(), app.as_os_str()];
    let (status, output) = crate::check_run::run_tee(&node, arguments, root)?;
    if status.success() {
        return Ok(());
    }
    ensure!(
        crate::gate::smoke_retryable(&output),
        "packaged smoke failed with {status}:\n{}",
        smoke_tail(&output)
    );
    println!(
        "package-mac: smoke lost a known timing race; retrying it once against the same \
         verified app."
    );
    let (status, output) = crate::check_run::run_tee(&node, arguments, root)?;
    ensure!(
        status.success(),
        "packaged smoke failed with {status} on retry:\n{}",
        smoke_tail(&output)
    );
    println!("package-mac: smoke passed on retry");
    Ok(())
}

pub fn read_package_version(path: &Path) -> Result<String> {
    let bytes = fsx::read_checked_file(path, "electron package.json", MAX_PACKAGE_JSON_BYTES)?;
    let package: PackageJson =
        serde_json::from_slice(&bytes).context("electron package.json is invalid")?;
    crate::release::compare_versions(&package.version, &package.version)?;
    Ok(package.version)
}

fn discover_mac_apps(output: &Path) -> Result<Vec<PathBuf>> {
    let mut apps = Vec::new();
    for entry in fs::read_dir(output).context("could not enumerate builder output")? {
        let entry = entry.context("could not inspect builder output")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !(name == "mac"
            || name.strip_prefix("mac-").is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            }))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Unexpected macOS output path: {name}"
        );
        let app = entry.path().join(APP_NAME);
        if app.exists() {
            let metadata = fs::symlink_metadata(&app)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Packaged app must be a real directory."
            );
            apps.push(app);
        }
    }
    apps.sort();
    ensure!(
        !apps.is_empty(),
        "electron-builder did not produce {APP_NAME}."
    );
    Ok(apps)
}

fn exact_archived_app(extraction: &Path) -> Result<PathBuf> {
    let mut entries = fs::read_dir(extraction)
        .context("could not inspect extracted ZIP")?
        .collect::<io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() == 1 && entries[0].file_name() == OsStr::new(APP_NAME),
        "Packaged ZIP must contain exactly one top-level {APP_NAME}."
    );
    let app = entries.swap_remove(0).path();
    let metadata = fs::symlink_metadata(&app)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Archived app must be a real directory."
    );
    Ok(app)
}

fn expected_binary_paths(root: &Path) -> Result<Vec<(String, PathBuf, MachoContentDigest)>> {
    let release = cargo_target_dir(root)?.join("release");
    let mut result = Vec::new();
    for name in EXPECTED_BINARIES {
        let path = release.join(name);
        let digest = digest_macho_content(&path, &format!("release {name}"))?;
        result.push((name.to_owned(), path, digest));
    }
    Ok(result)
}

fn verify_embedded_binaries(
    app: &Path,
    expected: &[(String, PathBuf, MachoContentDigest)],
) -> Result<()> {
    for (name, source, expected_digest) in expected {
        let packaged = app
            .join("Contents")
            .join("Resources")
            .join("bin")
            .join(name);
        let actual = digest_macho_content(&packaged, &format!("packaged {name}"))?;
        ensure!(
            actual == *expected_digest,
            "Packaged {name} Mach-O content does not match {} before the code-signature blob.",
            source.display()
        );
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileDigest {
    length: u64,
    sha512: [u8; 64],
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MachoContentDigest {
    signed_content_length: u64,
    sha512: [u8; 64],
}

fn digest_macho_content(path: &Path, label: &str) -> Result<MachoContentDigest> {
    let bytes = read_executable(path, label)?;
    ensure!(bytes.len() >= 32, "{label} has a truncated Mach-O header.");
    ensure!(
        read_u32(&bytes, 0)? == MACHO_64_MAGIC,
        "{label} is not a thin 64-bit little-endian Mach-O executable."
    );
    let commands = usize::try_from(read_u32(&bytes, 16)?).context("Mach-O command overflow")?;
    let command_bytes =
        usize::try_from(read_u32(&bytes, 20)?).context("Mach-O command size overflow")?;
    let command_end = 32_usize
        .checked_add(command_bytes)
        .context("Mach-O command size overflow")?;
    ensure!(
        command_end <= bytes.len(),
        "{label} has truncated Mach-O load commands."
    );
    let mut offset = 32_usize;
    let mut signature = None;
    let mut linkedit = None;
    for _ in 0..commands {
        ensure!(
            offset + 8 <= command_end,
            "{label} has a truncated Mach-O load command."
        );
        let command = read_u32(&bytes, offset)?;
        let size = usize::try_from(read_u32(&bytes, offset + 4)?)
            .context("Mach-O load command size overflow")?;
        ensure!(
            size >= 8 && offset + size <= command_end,
            "{label} has an invalid Mach-O load command."
        );
        if command == LC_CODE_SIGNATURE {
            ensure!(
                size == 16 && signature.is_none(),
                "{label} has an invalid code-signature command."
            );
            let data_offset = usize::try_from(read_u32(&bytes, offset + 8)?)
                .context("Mach-O signature offset overflow")?;
            let data_size = usize::try_from(read_u32(&bytes, offset + 12)?)
                .context("Mach-O signature size overflow")?;
            ensure!(
                data_offset >= command_end
                    && data_offset
                        .checked_add(data_size)
                        .is_some_and(|end| end <= bytes.len()),
                "{label} has an invalid code-signature range."
            );
            signature = Some((offset, data_offset));
        } else if command == LC_SEGMENT_64 && size >= 72 {
            let segment_name = &bytes[offset + 8..offset + 24];
            if segment_name.starts_with(b"__LINKEDIT\0") {
                ensure!(
                    linkedit.replace(offset).is_none(),
                    "{label} has duplicate __LINKEDIT segments."
                );
            }
        }
        offset += size;
    }
    ensure!(
        offset == command_end,
        "{label} has inconsistent Mach-O load commands."
    );
    let (signature_command, signed_content_end) =
        signature.with_context(|| format!("{label} has no embedded code-signature command."))?;
    let linkedit = linkedit.with_context(|| format!("{label} has no __LINKEDIT segment."))?;
    let mut content = bytes[..signed_content_end].to_vec();
    // Re-signing changes only the __LINKEDIT capacity fields and the signature
    // blob size. File offsets and every byte before the signature stay pinned.
    // Zero those size claims so the Cargo linker's ad-hoc signature and the
    // final Apple Development signature compare by executable content.
    content[linkedit + 32..linkedit + 40].fill(0);
    content[linkedit + 48..linkedit + 56].fill(0);
    content[signature_command + 12..signature_command + 16].fill(0);
    Ok(MachoContentDigest {
        signed_content_length: u64::try_from(signed_content_end)
            .context("Mach-O signed content length overflow")?,
        sha512: Sha512::digest(content).into(),
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value: [u8; 4] = bytes
        .get(offset..offset + 4)
        .context("Mach-O value is truncated")?
        .try_into()
        .context("Mach-O value has the wrong width")?;
    Ok(u32::from_le_bytes(value))
}

/// Opens an executable while it is still the file checked, with its length.
fn open_executable(path: &Path, label: &str) -> Result<(File, u64)> {
    let metadata = fsx::regular_metadata(path, label)?;
    ensure!(
        metadata.permissions().mode() & 0o111 != 0,
        "{label} is not executable."
    );
    let expected = FileState::from_metadata(&metadata);
    let file = fsx::open_unchanged(path, &expected, label)?;
    Ok((file, expected.length))
}

fn read_executable(path: &Path, label: &str) -> Result<Vec<u8>> {
    let (mut file, length) = open_executable(path, label)?;
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .with_context(|| format!("could not read {label}"))?;
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) == length,
        "{label} changed while reading."
    );
    Ok(bytes)
}

fn digest_executable_file(path: &Path, label: &str) -> Result<FileDigest> {
    let (mut file, length) = open_executable(path, label)?;
    let mut hasher = Sha512::new();
    let copied =
        io::copy(&mut file, &mut hasher).with_context(|| format!("could not read {label}"))?;
    ensure!(copied == length, "{label} changed while reading.");
    Ok(FileDigest {
        length: copied,
        sha512: hasher.finalize().into(),
    })
}

fn embedded_binary_file_digests(app: &Path) -> Result<Vec<(String, FileDigest)>> {
    EXPECTED_BINARIES
        .into_iter()
        .map(|name| {
            let path = app.join("Contents/Resources/bin").join(name);
            Ok((
                name.to_owned(),
                digest_executable_file(&path, &format!("signed packaged {name}"))?,
            ))
        })
        .collect()
}

fn verify_embedded_binary_file_digests(
    app: &Path,
    expected: &[(String, FileDigest)],
) -> Result<()> {
    for (name, digest) in expected {
        let path = app.join("Contents/Resources/bin").join(name);
        ensure!(
            digest_executable_file(&path, &format!("archived packaged {name}"))? == *digest,
            "Archived {name} bytes differ from the signed unpacked app."
        );
    }
    Ok(())
}

fn assert_no_bundled_claude_binary(app: &Path) -> Result<()> {
    let scope = app
        .join("Contents")
        .join("Resources")
        .join("app.asar.unpacked")
        .join("node_modules")
        .join("@anthropic-ai");
    match fs::symlink_metadata(&scope) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Packaged dependency scope is unsafe."
            );
            for entry in fs::read_dir(scope)? {
                let name = entry?.file_name();
                if name.to_string_lossy().starts_with("claude-agent-sdk-") {
                    bail!("Packaged app contains the unusable bundled Claude CLI.");
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("could not inspect packaged dependencies"),
    }
}

fn verify_bundle_metadata(app: &Path, expected_version: &str) -> Result<()> {
    let info = app.join("Contents").join("Info.plist");
    fsx::checked_regular_file(&info, "packaged Info.plist")?;
    let version = read_plist_value(&info, "CFBundleShortVersionString")?;
    ensure!(
        version == expected_version,
        "Packaged app version {version} does not match latest-mac.yml version {expected_version}."
    );
    let identifier = read_plist_value(&info, "CFBundleIdentifier")?;
    ensure!(
        identifier == BUNDLE_IDENTIFIER,
        "Packaged app has the wrong bundle identifier."
    );
    Ok(())
}

fn read_plist_value(info: &Path, key: &str) -> Result<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(info)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("failed to launch plutil")?;
    ensure!(
        output.status.success(),
        "plutil failed with {}",
        output.status
    );
    String::from_utf8(output.stdout)
        .context("plutil returned non-UTF-8 metadata")
        .map(|value| value.trim().to_owned())
}

fn verify_fuses(root: &Path, app: &Path) -> Result<()> {
    run_command_os(
        env::var_os("NODE").unwrap_or_else(|| "node".into()),
        [
            root.join("electron")
                .join("scripts")
                .join("verify-fuses.cjs")
                .into_os_string()
                .as_os_str(),
            app.as_os_str(),
        ],
        root,
    )
}

pub fn run_command<const N: usize>(program: &str, arguments: [&str; N], root: &Path) -> Result<()> {
    run_command_os(program, arguments.map(OsStr::new), root)
}

pub fn run_command_os<P, I, S>(program: P, arguments: I, root: &Path) -> Result<()>
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_command_env(program, arguments, root, &[])
}

/// `run_command_os` with extra environment for the child only.
fn run_command_env<P, I, S>(
    program: P,
    arguments: I,
    root: &Path,
    variables: &[(&str, &OsStr)],
) -> Result<()>
where
    P: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program.as_ref());
    command
        .current_dir(root)
        .args(arguments)
        .stdin(Stdio::null());
    for (name, value) in variables {
        command.env(name, value);
    }
    let status = command
        .status()
        .with_context(|| format!("failed to launch {}", program.as_ref().to_string_lossy()))?;
    if status.success() {
        Ok(())
    } else {
        bail!(
            "{} failed with {status}",
            program.as_ref().to_string_lossy()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::executable;

    #[test]
    fn a_verified_package_reads_as_the_commands_report_it() {
        let verified = VerifiedPackage {
            version: "0.3.140".to_owned(),
            unpacked_apps: 1,
            dmgs: 1,
            zips: 1,
            smoke_runs: 0,
        };
        assert_eq!(
            format!("Verified {verified}."),
            "Verified Interface 0.3.140 (1 app, 1 DMG, 1 ZIP, 0 smoke run)."
        );
    }

    #[test]
    fn release_binaries_must_report_exactly_the_stamped_package_version() {
        assert!(ensure_stamped(b"0.3.140\n", "0.3.140").is_ok());
        // An unstamped (workspace-version) build, a stale one, or anything
        // but the exact line is refused before electron-builder packages it.
        for reported in [
            &b"0.3.10\n"[..],
            b"0.3.139\n",
            b"0.3.140",
            b"0.3.140\n\n",
            b"",
        ] {
            assert!(ensure_stamped(reported, "0.3.140").is_err(), "{reported:?}");
        }
        assert_eq!(PRODUCT_VERSION_VARIABLE, "INTERFACE_PRODUCT_VERSION");
    }

    #[test]
    fn icon_render_is_reused_only_for_identical_inputs_outputs_and_chrome() {
        let temp = tempfile::tempdir().unwrap();
        let electron = temp.path();
        fs::create_dir_all(electron.join("scripts")).unwrap();
        fs::create_dir_all(electron.join("build")).unwrap();
        for name in ICON_FILES {
            fs::write(electron.join(name), name).unwrap();
        }
        let chrome = electron.join("Info.plist");
        fs::write(&chrome, "chrome 1").unwrap();
        let first = icon_stamp(electron, &chrome).unwrap();
        assert_eq!(icon_stamp(electron, &chrome).unwrap(), first);
        for name in ICON_FILES {
            fs::write(electron.join(name), "edited").unwrap();
            assert_ne!(icon_stamp(electron, &chrome).unwrap(), first, "{name}");
            fs::write(electron.join(name), name).unwrap();
        }
        fs::write(&chrome, "chrome 2").unwrap();
        assert_ne!(
            icon_stamp(electron, &chrome).unwrap(),
            first,
            "Chrome update"
        );
        fs::remove_file(&chrome).unwrap();
        assert!(
            icon_stamp(electron, &chrome).is_err(),
            "no rasterizer, no stamp"
        );
        fs::write(&chrome, "chrome 1").unwrap();
        fs::remove_file(electron.join("build/icon.icns")).unwrap();
        assert!(
            icon_stamp(electron, &chrome).is_err(),
            "missing icon, no stamp"
        );

        let output = (1..=50)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = smoke_tail(&output);
        assert!(tail.starts_with("line 11\n") && tail.ends_with("line 50"));
    }

    fn macho(code: &[u8], signature_size: u32) -> Vec<u8> {
        let mut bytes = vec![0_u8; 64];
        bytes[0..4].copy_from_slice(&MACHO_64_MAGIC.to_le_bytes());
        bytes[16..20].copy_from_slice(&2_u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&16_u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&88_u32.to_le_bytes());
        bytes.resize(136, 0);
        bytes[32..36].copy_from_slice(&LC_SEGMENT_64.to_le_bytes());
        bytes[36..40].copy_from_slice(&72_u32.to_le_bytes());
        bytes[40..56].copy_from_slice(b"__LINKEDIT\0\0\0\0\0\0");
        bytes[64..72].copy_from_slice(&u64::from(signature_size).to_le_bytes());
        bytes[80..88].copy_from_slice(&u64::from(signature_size).to_le_bytes());
        bytes[104..108].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
        bytes[108..112].copy_from_slice(&16_u32.to_le_bytes());
        bytes[112..116].copy_from_slice(&136_u32.to_le_bytes());
        bytes[116..120].copy_from_slice(&signature_size.to_le_bytes());
        for (index, byte) in code.iter().enumerate() {
            bytes[120 + index % 16] ^= byte;
        }
        bytes.resize(
            136 + usize::try_from(signature_size).expect("signature size"),
            0xa5,
        );
        bytes
    }

    #[test]
    fn exact_binary_inspection_accepts_only_release_bytes() {
        let root = tempfile::tempdir().expect("root");
        let app = root.path().join(APP_NAME);
        let expected_root = root.path().join("expected");
        let mut expected = Vec::new();
        for name in EXPECTED_BINARIES {
            let source = expected_root.join(name);
            let packaged = app
                .join("Contents")
                .join("Resources")
                .join("bin")
                .join(name);
            executable(&source, &macho(name.as_bytes(), 32));
            executable(&packaged, &macho(name.as_bytes(), 48));
            expected.push((
                name.to_owned(),
                source.clone(),
                digest_macho_content(&source, name).expect("digest"),
            ));
        }
        verify_embedded_binaries(&app, &expected).expect("exact binaries");
        let host = app.join("Contents/Resources/bin/interface-host");
        let streamed = digest_executable_file(&host, "host").expect("streamed digest");
        let bytes = read_executable(&host, "host").expect("bytes");
        assert_eq!(streamed.length, u64::try_from(bytes.len()).expect("len"));
        assert_eq!(
            streamed.sha512.as_slice(),
            Sha512::digest(&bytes).as_slice()
        );
        fs::write(
            app.join("Contents/Resources/bin/interface-host"),
            b"different",
        )
        .expect("tamper");
        assert!(verify_embedded_binaries(&app, &expected).is_err());
    }

    #[test]
    fn every_expected_binary_is_a_packaged_resource() {
        let config = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../electron/electron-builder.yml"),
        )
        .expect("electron-builder.yml");
        for name in EXPECTED_BINARIES {
            assert!(
                config.contains(&format!("    to: bin/{name}\n")),
                "{name} is verified in the package but electron-builder does not bundle it"
            );
        }
        assert_eq!(
            config.matches("    to: bin/").count(),
            EXPECTED_BINARIES.len()
        );
    }

    #[test]
    fn resource_paths_resolve_local_external_and_symlinked_targets() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("release checkout/electron");
        fs::create_dir_all(&project).expect("project");
        for (target, relative) in [
            (root.path().join("release checkout/target"), "../target"),
            (
                root.path().join("shared build/cache"),
                "../../shared build/cache",
            ),
            (project.clone(), "."),
            (project.join("nested/target"), "nested/target"),
        ] {
            fs::create_dir_all(&target).expect("target");
            let actual = relative_resource_dir(&project, &target).expect("relative path");
            assert_eq!(actual, Path::new(relative));
            assert_eq!(
                fs::canonicalize(project.join(actual)).expect("resolved resource"),
                fs::canonicalize(&target).expect("resolved target"),
            );
        }
        let linked = root.path().join("linked-target");
        std::os::unix::fs::symlink(root.path().join("shared build/cache"), &linked)
            .expect("target symlink");
        assert_eq!(
            relative_resource_dir(&project, &linked).expect("linked target"),
            Path::new("../../shared build/cache"),
        );
        assert!(relative_resource_dir(&project, &root.path().join("missing")).is_err());
    }

    #[test]
    fn sdk_major_version_reads_only_the_leading_component() {
        assert_eq!(sdk_major_version("26.5\n"), Some(26));
        assert_eq!(sdk_major_version("15.4"), Some(15));
        assert_eq!(sdk_major_version(""), None);
        assert_eq!(
            sdk_major_version("xcrun: error: unable to lookup item 'SDKVersion'"),
            None
        );
    }

    #[test]
    fn archived_zip_shape_is_exact_and_link_safe() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join(APP_NAME)).expect("app");
        assert_eq!(
            exact_archived_app(root.path()).expect("app"),
            root.path().join(APP_NAME)
        );
        fs::write(root.path().join("extra"), b"extra").expect("extra");
        assert!(exact_archived_app(root.path()).is_err());
    }

    /// `codesign -d --verbose=2` of an app signed the way `sign-mac.cjs`
    /// signs (the name and team are made up; the shape is codesign's).
    const HARDENED_DISPLAY: &str = "Executable=/private/tmp/Interface.app/Contents/MacOS/Interface\nIdentifier=com.andreromero.interface\nFormat=app bundle with Mach-O thin (arm64)\nCodeDirectory v=20500 size=445 flags=0x10000(runtime) hashes=3+7 location=embedded\nSignature size=4797\nAuthority=Apple Development: Example Builder (ABCDE12345)\nAuthority=Apple Worldwide Developer Relations Certification Authority\nAuthority=Apple Root CA\nTimestamp=Nov 23, 2026 at 5:36:57 PM\nInfo.plist entries=35\nTeamIdentifier=1A2B3C4D5E\nRuntime Version=26.0.0\nSealed Resources version=2 rules=13 files=22\nInternal requirements count=1 size=180\n";
    /// The same for every build up to 0.3.181: no flags, no timestamp.
    const UNHARDENED_DISPLAY: &str = "Executable=/Applications/Interface.app/Contents/MacOS/Interface\nIdentifier=com.andreromero.interface\nFormat=app bundle with Mach-O thin (arm64)\nCodeDirectory v=20400 size=445 flags=0x0(none) hashes=3+7 location=embedded\nSignature size=4795\nAuthority=Apple Development: Example Builder (ABCDE12345)\nAuthority=Apple Worldwide Developer Relations Certification Authority\nAuthority=Apple Root CA\nSigned Time=Nov 15, 2026 at 10:32:17 PM\nInfo.plist entries=35\nTeamIdentifier=1A2B3C4D5E\nSealed Resources version=2 rules=13 files=22\nInternal requirements count=1 size=204\n";

    #[test]
    fn signing_facts_read_runtime_timestamp_and_team_from_codesign() {
        assert_eq!(
            signing_facts(HARDENED_DISPLAY),
            SigningFacts {
                runtime: true,
                timestamped: true,
                team: Some("1A2B3C4D5E".to_owned()),
            }
        );
        assert_eq!(
            signing_facts(UNHARDENED_DISPLAY),
            SigningFacts {
                runtime: false,
                timestamped: false,
                team: Some("1A2B3C4D5E".to_owned()),
            }
        );
        assert_eq!(
            signing_facts(
                "CodeDirectory v=20500 size=3 flags=0x10002(adhoc,runtime) hashes=1+0 location=embedded\nTeamIdentifier=not set\n"
            ),
            SigningFacts {
                runtime: true,
                timestamped: false,
                team: None,
            }
        );
        // A flag merely named like runtime, or runtime outside the flags, is not one.
        assert!(!signing_facts("CodeDirectory v=1 flags=0x0(none) runtime\n").runtime);
        assert!(!signing_facts("CodeDirectory v=1 flags=0x2(adhoc,runtimes)\n").runtime);
        assert!(!signing_facts("Timestamp=\n").timestamped);
    }

    #[test]
    fn only_the_team_pinned_requirement_is_a_public_one() {
        let pinned = "identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.OU] = ";
        assert!(is_team_pinned(
            &format!("{pinned}\"1A2B3C4D5E\""),
            BUNDLE_IDENTIFIER,
            "1A2B3C4D5E"
        ));
        assert!(is_team_pinned(
            &format!("{pinned}ABCDE12345"),
            BUNDLE_IDENTIFIER,
            "ABCDE12345"
        ));
        for refused in [
            // The automatic Apple Development requirement pins a name.
            "identifier \"com.andreromero.interface\" and anchor apple generic and certificate leaf[subject.CN] = \"Apple Development: Example Builder (ABCDE12345)\" and certificate 1[field.1.2.840.113635.100.6.2.1] exists".to_owned(),
            format!("{pinned}\"0000000000\""),
            format!("{pinned}\"1A2B3C4D5E\" and certificate leaf[subject.CN] = \"x\""),
            format!("{pinned}\"1A2B3C4D5E"),
            "identifier \"com.example.other\" and anchor apple generic and certificate leaf[subject.OU] = \"1A2B3C4D5E\"".to_owned(),
        ] {
            assert!(
                !is_team_pinned(&refused, BUNDLE_IDENTIFIER, "1A2B3C4D5E"),
                "{refused}"
            );
        }
    }

    #[test]
    fn an_app_without_the_hardened_runtime_timestamp_and_team_is_refused() {
        let root = tempfile::tempdir().expect("root");
        let plain = crate::test_fixtures::ad_hoc_app(&root.path().join("plain"), None);
        let error = verify_public_signature(&plain)
            .expect_err("unhardened")
            .to_string();
        assert!(error.contains("hardened runtime"), "{error}");
        let hardened =
            crate::test_fixtures::ad_hoc_app(&root.path().join("hardened"), Some("runtime"));
        let error = verify_public_signature(&hardened)
            .expect_err("untimestamped")
            .to_string();
        assert!(error.contains("secure timestamp"), "{error}");
    }

    #[test]
    fn package_temp_builder_enforces_owner_only_permissions() {
        let root = tempfile::tempdir().expect("root");
        let output = Builder::new()
            .prefix(PACKAGE_TEMP_PREFIX)
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(root.path())
            .expect("private package output");
        assert_eq!(
            fs::metadata(output.path())
                .expect("meta")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
