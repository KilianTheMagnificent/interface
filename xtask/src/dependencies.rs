//! Dependency identity excludes only the application's own version fields.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{fs, path::Path, process::Command};

pub fn without_app_version(bytes: &[u8], lockfile: bool) -> Result<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object_mut()
        .context("package metadata must be an object")?;
    object.remove("version");
    if lockfile {
        if let Some(package) = value
            .pointer_mut("/packages/")
            .and_then(Value::as_object_mut)
        {
            package.remove("version");
        }
    }
    Ok(serde_json::to_vec(&value)?)
}

pub fn fingerprint(lock: &[u8]) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(without_app_version(lock, true)?)
    ))
}

pub fn ensure_installed(root: &Path) -> Result<()> {
    ensure_with(root, |electron| {
        crate::package::run_command("npm", ["ci"], electron)?;
        if !electron
            .join("node_modules/electron/dist/Electron.app/Contents/MacOS/Electron")
            .is_file()
        {
            crate::package::run_command("node", ["node_modules/electron/install.js"], electron)?;
        }
        ensure!(
            Command::new("node")
                .current_dir(electron)
                .args([
                    "-e",
                    "require('electron'); require('typescript'); require.resolve('vite')"
                ])
                .status()?
                .success(),
            "dependency verification failed"
        );
        Ok(())
    })
}

fn complete(modules: &Path) -> bool {
    modules.join(".bin/vite").is_file()
        && modules.join(".bin/tsc").is_file()
        && modules
            .join("electron/dist/Electron.app/Contents/MacOS/Electron")
            .is_file()
}

fn ensure_with(root: &Path, install: impl Fn(&Path) -> Result<()>) -> Result<()> {
    let electron = root.join("electron");
    let modules = electron.join("node_modules");
    let lock = fs::read(electron.join("package-lock.json"))?;
    let expected = fingerprint(&lock)?;
    let record = modules.join(".interface-dependencies-v1.json");
    let installed = crate::pipeline_state::read::<String>(&record)?.as_deref() == Some(&expected);
    if installed && complete(&modules) {
        println!("dependencies: npm ci skipped — verified install matches the dependency graph");
        return Ok(());
    }
    // Remove proof before npm can alter the install. A failed attempt must retry.
    if record.exists() {
        fs::remove_file(&record)?;
    }
    println!("dependencies: installing (missing, incomplete, or changed dependency graph)");
    install(&electron)?;
    ensure!(
        complete(&modules),
        "dependency install is incomplete; no proof recorded"
    );
    ensure!(
        fs::read(electron.join("package-lock.json"))? == lock,
        "lockfile changed during installation; retry"
    );
    crate::pipeline_state::write(&record, &expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_installs_retry_and_version_only_restarts_reuse_the_success() {
        let temp = tempfile::tempdir().unwrap();
        let electron = temp.path().join("electron");
        let modules = electron.join("node_modules");
        fs::create_dir_all(&modules).unwrap();
        let lock = electron.join("package-lock.json");
        fs::write(&lock, r#"{"version":"1","packages":{"":{"version":"1"}}}"#).unwrap();
        let record = modules.join(".interface-dependencies-v1.json");
        crate::pipeline_state::write(&record, &"outdated").unwrap();
        assert!(ensure_with(temp.path(), |_| anyhow::bail!("injected npm failure")).is_err());
        assert!(!record.exists());
        ensure_with(temp.path(), |_| {
            for relative in [
                ".bin/vite",
                ".bin/tsc",
                "electron/dist/Electron.app/Contents/MacOS/Electron",
            ] {
                let file = modules.join(relative);
                fs::create_dir_all(file.parent().unwrap())?;
                fs::write(file, b"fixture")?;
            }
            Ok(())
        })
        .unwrap();
        fs::write(&lock, r#"{"version":"2","packages":{"":{"version":"2"}}}"#).unwrap();
        ensure_with(temp.path(), |_| {
            panic!("version-only change must reuse the install")
        })
        .unwrap();
        fs::remove_file(modules.join(".bin/tsc")).unwrap();
        assert!(
            ensure_with(temp.path(), |_| anyhow::bail!(
                "incomplete install requires repair"
            ))
            .is_err()
        );
        assert!(!record.exists());
    }
    #[test]
    fn root_versions_are_ignored_but_dependency_versions_are_not() {
        let before =
            br#"{"version":"1","packages":{"":{"version":"1"},"node_modules/x":{"version":"1"}}}"#;
        let bump =
            br#"{"version":"2","packages":{"":{"version":"2"},"node_modules/x":{"version":"1"}}}"#;
        let dependency =
            br#"{"version":"1","packages":{"":{"version":"1"},"node_modules/x":{"version":"2"}}}"#;
        assert_eq!(fingerprint(before).unwrap(), fingerprint(bump).unwrap());
        assert_ne!(
            fingerprint(before).unwrap(),
            fingerprint(dependency).unwrap()
        );
        assert!(fingerprint(b"broken").is_err());
    }
}
