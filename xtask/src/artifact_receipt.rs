//! A successful signature/archive/smoke verification belongs to exact bytes.
use crate::{gate, pipeline_state, release};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: u8,
    pub commit: String,
    pub version: String,
    pub directory: PathBuf,
    pub manifest_sha256: String,
    pub smoke_runs: usize,
    pub verified_at: u64,
}

/// The digest a receipt binds its manifest by, as lowercase hex.
fn manifest_digest(source: &release::UpdateSource) -> String {
    format!("{:x}", Sha256::digest(&source.manifest_bytes))
}

pub fn record(root: &Path, expected: &str, smoke_runs: usize) -> Result<Receipt> {
    ensure!(
        gate::rev_parse(root, "HEAD")? == expected && gate::tree_is_clean(root)?,
        "source changed during packaging; verification cannot be attributed to the candidate"
    );
    let directory = root.join("release");
    let source = release::inspect_update_source(&directory)?;
    release::verify_update_source_checksums(&source)?;
    let receipt = Receipt {
        schema: 1,
        commit: expected.to_owned(),
        manifest_sha256: manifest_digest(&source),
        version: source.manifest.version,
        directory,
        smoke_runs,
        verified_at: pipeline_state::now(),
    };
    let dir = root.join("target/xtask");
    fs::create_dir_all(&dir)?;
    pipeline_state::write(&dir.join("package-receipt.json"), &receipt)?;
    Ok(receipt)
}

pub fn validate(receipt: &Receipt, commit: &str, version: &str) -> Result<()> {
    ensure!(
        receipt.schema == 1
            && receipt.commit == commit
            && receipt.version == version
            && receipt.smoke_runs > 0,
        "package receipt does not cover this release and its smoke gate"
    );
    let source = release::inspect_update_source(&receipt.directory)?;
    ensure!(
        source.manifest.version == version && manifest_digest(&source) == receipt.manifest_sha256,
        "verified artifact manifest changed; refusing to reuse it"
    );
    release::verify_update_source_checksums(&source)
}

/// Copy only after validation, then prove the durable copy has the same bytes.
pub fn archive(receipt: &Receipt, destination: &Path) -> Result<Receipt> {
    validate(receipt, &receipt.commit, &receipt.version)?;
    let mut archived = receipt.clone();
    destination.clone_into(&mut archived.directory);
    if !destination.join(release::MANIFEST_NAME).exists() {
        release::stage_update_feed(&receipt.directory, destination, false)?;
    }
    validate(&archived, &receipt.commit, &receipt.version)?;
    Ok(archived)
}

pub fn feed_matches(receipt: &Receipt, target: &Path) -> Result<bool> {
    if !target.join(release::MANIFEST_NAME).exists() {
        return Ok(false);
    }
    let source = release::inspect_update_source(target)?;
    if source.manifest.version != receipt.version {
        return Ok(false);
    }
    ensure!(
        manifest_digest(&source) == receipt.manifest_sha256,
        "feed already uses this version with different bytes; refusing replacement"
    );
    release::verify_update_source_checksums(&source)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use sha2::Sha512;

    #[test]
    fn archive_and_feed_resume_require_identical_verified_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("source");
        fs::create_dir(&directory).unwrap();
        let bytes = b"previously verified fixture";
        let digest = STANDARD.encode(Sha512::digest(bytes));
        let name = "Interface-1.0.0-arm64-mac.zip";
        fs::write(directory.join(name), bytes).unwrap();
        fs::write(directory.join("Interface-1.0.0-arm64.dmg"), bytes).unwrap();
        let manifest = format!(
            "version: 1.0.0\nfiles:\n  - url: {name}\n    sha512: {digest}\n    size: {}\n  - url: Interface-1.0.0-arm64.dmg\n    sha512: {digest}\n    size: {}\npath: {name}\nsha512: {digest}\n",
            bytes.len(),
            bytes.len()
        );
        fs::write(directory.join(release::MANIFEST_NAME), &manifest).unwrap();
        let receipt = Receipt {
            schema: 1,
            commit: "a".repeat(40),
            version: "1.0.0".into(),
            directory,
            manifest_sha256: format!("{:x}", Sha256::digest(manifest.as_bytes())),
            smoke_runs: 1,
            verified_at: 1,
        };
        let copy = archive(&receipt, &temp.path().join("archive")).unwrap();
        assert!(feed_matches(&receipt, &copy.directory).unwrap());
        archive(&receipt, &copy.directory).unwrap(); // idempotent crash recovery
        fs::write(copy.directory.join(name), b"changed bytes").unwrap();
        assert!(validate(&copy, &copy.commit, &copy.version).is_err());
        assert!(feed_matches(&receipt, &copy.directory).is_err());
    }
    #[test]
    fn unrelated_or_unsmoked_receipts_are_never_reused() {
        let receipt = Receipt {
            schema: 1,
            commit: "a".repeat(40),
            version: "1.0.0".into(),
            directory: PathBuf::from("/does-not-exist"),
            manifest_sha256: String::new(),
            smoke_runs: 0,
            verified_at: 0,
        };
        assert!(
            validate(&receipt, &receipt.commit, &receipt.version)
                .unwrap_err()
                .to_string()
                .contains("smoke")
        );
        assert!(validate(&receipt, &"b".repeat(40), &receipt.version).is_err());
    }
}
