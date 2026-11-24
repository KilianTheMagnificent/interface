//! Fixtures the xtask test modules share.
use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, process::Command};

const NAME: &str = "xtask test";
const EMAIL: &str = "xtask@test";

/// Writes `bytes` to `path` as an executable, creating its directory.
pub fn executable(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, bytes).expect("file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("mode");
}

/// Runs git in `dir` with a fixed identity and signing off, whatever the
/// ambient configuration says, and returns its trimmed stdout. A failure
/// fails the test with git's stderr.
pub fn git_in(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            &format!("user.name={NAME}"),
            "-c",
            &format!("user.email={EMAIL}"),
            "-c",
            "commit.gpgsign=false",
        ])
        .args(arguments)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

/// `<dir>/Interface.app`: a copy of `/bin/sleep` as its executable, signed
/// ad hoc with the given codesign `--options` (for example `runtime`). Ad hoc
/// signatures have no team and no secure timestamp.
pub fn ad_hoc_app(dir: &Path, options: Option<&str>) -> std::path::PathBuf {
    let app = dir.join("Interface.app");
    let contents = app.join("Contents");
    fs::create_dir_all(contents.join("MacOS")).expect("app directory");
    fs::write(
        contents.join("Info.plist"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>com.andreromero.interface</string><key>CFBundleExecutable</key><string>Interface</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>\n",
    )
    .expect("Info.plist");
    fs::copy("/bin/sleep", contents.join("MacOS").join("Interface")).expect("executable");
    let mut codesign = Command::new("/usr/bin/codesign");
    codesign.args(["--force", "--sign", "-"]);
    if let Some(options) = options {
        codesign.args(["--options", options]);
    }
    let output = codesign.arg(&app).output().expect("codesign runs");
    assert!(
        output.status.success(),
        "ad hoc signing failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    app
}

/// A new repository at `dir` on `main`, with that identity and signing off
/// written into its configuration: the code under test runs git itself, and a
/// signing key or `commit.gpgsign = true` in the ambient global configuration
/// would fail every commit it writes.
pub fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).expect("repository directory");
    git_in(dir, &["init", "--quiet", "-b", "main"]);
    git_in(dir, &["config", "user.name", NAME]);
    git_in(dir, &["config", "user.email", EMAIL]);
    git_in(dir, &["config", "commit.gpgsign", "false"]);
}
