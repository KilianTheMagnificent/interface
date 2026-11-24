//! The signed release manifest of the public update channel.
//!
//! One small JSON file names the newest release and the exact bytes of its
//! archive. It carries an Ed25519 signature made with a key that never leaves
//! the publisher's Mac, so the server that hosts the file can withhold an
//! update but cannot forge one. The signature is checked before a single
//! payload field is parsed and long before the archive is downloaded.
//!
//! A client that ships today must still read the manifest published years
//! from now, or its users are stranded. So unknown fields are ignored, the
//! file name and `schema` never change, and anything incompatible would be
//! published under a new name beside this one.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::state::is_plain_version;

/// The feed file every client fetches.
pub const SIGNED_MANIFEST_NAME: &str = "interface-update.json";
pub const MAX_SIGNED_MANIFEST_BYTES: usize = 64 * 1024;
pub const MANIFEST_SCHEMA: u32 = 1;
/// Largest archive a manifest may name; the local feed serves no more.
pub const MAX_RELEASE_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Bound into every signature so a signature over some other document that
/// happens to be made with the same key can never pass as a manifest.
const SIGNING_CONTEXT: &[u8] = b"interface-update-manifest-v1\n";
const MAX_SIGNATURES: usize = 8;
const MAX_LATEST_MAC_YML_BYTES: usize = 16 * 1024;
const MAX_ARTIFACT_NAME_BYTES: usize = 200;
const MAX_LABEL_BYTES: usize = 128;
const PUBLIC_KEY_BYTES: usize = 32;
const SIGNATURE_BYTES: usize = 64;
const SHA512_BYTES: usize = 64;

/// An Ed25519 key a manifest may be signed with.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PublicKey([u8; PUBLIC_KEY_BYTES]);

impl PublicKey {
    #[must_use]
    pub fn from_base64(text: &str) -> Option<Self> {
        let bytes = BASE64.decode(text.trim()).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    #[must_use]
    pub fn to_base64(&self) -> String {
        BASE64.encode(self.0)
    }

    /// A short public name for the key: which of the trusted keys signed.
    #[must_use]
    pub fn id(&self) -> String {
        Sha256::digest(self.0)
            .iter()
            .take(8)
            .fold(String::with_capacity(16), |mut id, byte| {
                use fmt::Write as _;
                let _ = write!(id, "{byte:02x}");
                id
            })
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PublicKey({})", self.id())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseArtifact {
    pub name: String,
    pub size: u64,
    /// Base64 SHA-512, the form `latest-mac.yml` uses.
    pub sha512: String,
}

/// What the publisher signs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub schema: u32,
    pub product: String,
    pub channel: String,
    pub version: String,
    pub published_at: String,
    pub artifact: ReleaseArtifact,
    /// The exact `latest-mac.yml` the packaged release was checked with. The
    /// client hands it, unchanged, to the installer that already ships.
    pub latest_mac_yml: String,
}

impl ReleaseManifest {
    /// The archive's SHA-512 as raw bytes.
    #[must_use]
    pub fn artifact_sha512(&self) -> Option<[u8; SHA512_BYTES]> {
        BASE64.decode(&self.artifact.sha512).ok()?.try_into().ok()
    }
}

#[derive(Deserialize, Serialize)]
struct Envelope {
    schema: u32,
    /// Base64 of the exact signed bytes: nothing is re-serialized to check.
    payload: String,
    signatures: Vec<EnvelopeSignature>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvelopeSignature {
    key_id: String,
    signature: String,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ManifestError {
    #[error("the update manifest is too large")]
    TooLarge,
    #[error("the update manifest is malformed")]
    Malformed,
    #[error("the update manifest is not signed by a trusted key")]
    Untrusted,
    #[error("the update manifest uses a format this version cannot read")]
    UnsupportedSchema,
    #[error("the update manifest is for another product or channel")]
    WrongChannel,
    #[error("the update manifest names an invalid version")]
    InvalidVersion,
    #[error("the update manifest names an invalid archive")]
    InvalidArtifact,
    #[error("the update manifest's installer feed does not match its archive")]
    InconsistentFeed,
    #[error("the signing key is unusable")]
    SigningKey,
}

/// Checks the signature, then the contents, of a fetched manifest.
///
/// # Errors
///
/// Fails closed for oversize or malformed input, a manifest no trusted key
/// signed, another product or channel, and any field the installer could not
/// safely act on.
pub fn verify_signed_manifest(
    bytes: &[u8],
    keys: &[PublicKey],
    product: &str,
    channel: &str,
) -> Result<ReleaseManifest, ManifestError> {
    if bytes.len() > MAX_SIGNED_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| ManifestError::Malformed)?;
    if envelope.schema != MANIFEST_SCHEMA {
        return Err(ManifestError::UnsupportedSchema);
    }
    if envelope.signatures.len() > MAX_SIGNATURES {
        return Err(ManifestError::Malformed);
    }
    let payload = BASE64
        .decode(&envelope.payload)
        .map_err(|_| ManifestError::Malformed)?;
    let message = signed_message(&payload);
    let trusted = envelope.signatures.iter().any(|claimed| {
        BASE64
            .decode(&claimed.signature)
            .ok()
            .filter(|signature| signature.len() == SIGNATURE_BYTES)
            .is_some_and(|signature| {
                keys.iter().any(|key| {
                    UnparsedPublicKey::new(&ED25519, key.0)
                        .verify(&message, &signature)
                        .is_ok()
                })
            })
    });
    if !trusted {
        return Err(ManifestError::Untrusted);
    }
    // Only now is the payload the publisher's word.
    let manifest: ReleaseManifest =
        serde_json::from_slice(&payload).map_err(|_| ManifestError::Malformed)?;
    validate(&manifest, product, channel)?;
    Ok(manifest)
}

/// Signs `manifest` with a PKCS#8 Ed25519 key and returns the feed file.
///
/// # Errors
///
/// Refuses a manifest clients would reject, and an unusable key.
pub fn sign_manifest(manifest: &ReleaseManifest, pkcs8: &[u8]) -> Result<Vec<u8>, ManifestError> {
    validate(manifest, &manifest.product, &manifest.channel)?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| ManifestError::SigningKey)?;
    let key = public_key(&pair)?;
    let payload = serde_json::to_vec(manifest).map_err(|_| ManifestError::Malformed)?;
    let signature = pair.sign(&signed_message(&payload));
    let envelope = Envelope {
        schema: MANIFEST_SCHEMA,
        payload: BASE64.encode(&payload),
        signatures: vec![EnvelopeSignature {
            key_id: key.id(),
            signature: BASE64.encode(signature.as_ref()),
        }],
    };
    let mut bytes = serde_json::to_vec_pretty(&envelope).map_err(|_| ManifestError::Malformed)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_SIGNED_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    Ok(bytes)
}

/// Makes a new signing key: the PKCS#8 private half and its public key.
///
/// # Errors
///
/// Fails only when the system random source does.
pub fn generate_signing_key() -> Result<(Vec<u8>, PublicKey), ManifestError> {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| ManifestError::SigningKey)?;
    let pkcs8 = document.as_ref().to_vec();
    let key = signing_public_key(&pkcs8)?;
    Ok((pkcs8, key))
}

/// The public key of a PKCS#8 Ed25519 private key.
///
/// # Errors
///
/// Fails for bytes that are not such a key.
pub fn signing_public_key(pkcs8: &[u8]) -> Result<PublicKey, ManifestError> {
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| ManifestError::SigningKey)?;
    public_key(&pair)
}

fn public_key(pair: &Ed25519KeyPair) -> Result<PublicKey, ManifestError> {
    pair.public_key()
        .as_ref()
        .try_into()
        .map(PublicKey)
        .map_err(|_| ManifestError::SigningKey)
}

fn signed_message(payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNING_CONTEXT.len() + payload.len());
    message.extend_from_slice(SIGNING_CONTEXT);
    message.extend_from_slice(payload);
    message
}

fn validate(manifest: &ReleaseManifest, product: &str, channel: &str) -> Result<(), ManifestError> {
    if manifest.schema != MANIFEST_SCHEMA {
        return Err(ManifestError::UnsupportedSchema);
    }
    if manifest.product != product
        || manifest.channel != channel
        || !is_label(&manifest.product)
        || !is_label(&manifest.channel)
    {
        return Err(ManifestError::WrongChannel);
    }
    if !is_plain_version(&manifest.version) {
        return Err(ManifestError::InvalidVersion);
    }
    if !is_artifact_name(&manifest.artifact.name)
        || manifest.artifact.size == 0
        || manifest.artifact.size > MAX_RELEASE_ARCHIVE_BYTES
        || manifest.artifact_sha512().is_none()
        || manifest.published_at.len() > MAX_LABEL_BYTES
        || manifest.published_at.chars().any(char::is_control)
    {
        return Err(ManifestError::InvalidArtifact);
    }
    if !feed_names_only_the_artifact(manifest) {
        return Err(ManifestError::InconsistentFeed);
    }
    Ok(())
}

fn is_label(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_LABEL_BYTES
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// One flat ZIP name that needs no escaping in a URL or on disk.
#[must_use]
pub fn is_artifact_name(name: &str) -> bool {
    name.len() <= MAX_ARTIFACT_NAME_BYTES
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
        && !name.contains("..")
        && name
            .strip_suffix(".zip")
            .is_some_and(|stem| !stem.is_empty())
}

/// What a `latest-mac.yml` names: its version, its archive and that
/// archive's digest, plus every file entry as (url, sha512, size).
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct FeedSummary<'a> {
    pub(crate) version: Option<&'a str>,
    pub(crate) path: Option<&'a str>,
    pub(crate) sha512: Option<&'a str>,
    pub(crate) files: Vec<(Option<&'a str>, Option<&'a str>, Option<&'a str>)>,
}

/// Reads the handful of plain scalars electron-builder writes. Not a YAML
/// parser: anything it does not recognise is skipped, and the caller decides
/// whether what is left is enough.
pub(crate) fn feed_summary(text: &str) -> FeedSummary<'_> {
    let mut summary = FeedSummary::default();
    for line in text.lines() {
        let indented = line.starts_with(' ');
        let entry = line.trim_start();
        let (starts_file, entry) = match entry.strip_prefix("- ") {
            Some(rest) => (true, rest),
            None => (false, entry),
        };
        let Some((key, value)) = entry.split_once(':') else {
            continue;
        };
        let value = unquoted(value.trim());
        if !indented {
            match key {
                "version" => summary.version = Some(value),
                "path" => summary.path = Some(value),
                "sha512" => summary.sha512 = Some(value),
                _ => {}
            }
            continue;
        }
        if starts_file {
            summary.files.push((None, None, None));
        }
        if let Some(file) = summary.files.last_mut() {
            match key {
                "url" => file.0 = Some(value),
                "sha512" => file.1 = Some(value),
                "size" => file.2 = Some(value),
                _ => {}
            }
        }
    }
    summary
}

/// The installer reads `latest-mac.yml`, not the manifest, so the two must
/// agree: the same version, one file, the same name, size and digest.
fn feed_names_only_the_artifact(manifest: &ReleaseManifest) -> bool {
    let text = manifest.latest_mac_yml.as_str();
    if text.is_empty() || text.len() > MAX_LATEST_MAC_YML_BYTES || text.contains('\r') {
        return false;
    }
    let artifact = &manifest.artifact;
    let size = artifact.size.to_string();
    feed_summary(text)
        == FeedSummary {
            version: Some(manifest.version.as_str()),
            path: Some(artifact.name.as_str()),
            sha512: Some(artifact.sha512.as_str()),
            files: vec![(
                Some(artifact.name.as_str()),
                Some(artifact.sha512.as_str()),
                Some(size.as_str()),
            )],
        }
}

fn unquoted(value: &str) -> &str {
    for quote in ['\'', '"'] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sha2::Sha512;

    pub(crate) const PRODUCT: &str = "com.example.interface";
    pub(crate) const CHANNEL: &str = "stable";

    pub(crate) fn manifest_for(version: &str, archive: &[u8]) -> ReleaseManifest {
        let name = format!("Interface-{version}-arm64-mac.zip");
        let sha512 = BASE64.encode(Sha512::digest(archive));
        let size = archive.len() as u64;
        ReleaseManifest {
            schema: MANIFEST_SCHEMA,
            product: PRODUCT.to_owned(),
            channel: CHANNEL.to_owned(),
            version: version.to_owned(),
            published_at: "2026-11-21T18:00:00.000Z".to_owned(),
            latest_mac_yml: format!(
                "version: {version}\nfiles:\n  - url: {name}\n    sha512: {sha512}\n    size: {size}\npath: {name}\nsha512: {sha512}\nreleaseDate: '2026-11-21T18:00:00.000Z'\n"
            ),
            artifact: ReleaseArtifact { name, size, sha512 },
        }
    }

    fn signed(manifest: &ReleaseManifest) -> (Vec<u8>, PublicKey) {
        let (pkcs8, key) = generate_signing_key().expect("key");
        (sign_manifest(manifest, &pkcs8).expect("signed"), key)
    }

    #[test]
    fn a_manifest_signed_by_a_trusted_key_is_read_back_exactly() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (bytes, key) = signed(&manifest);
        assert_eq!(
            verify_signed_manifest(&bytes, &[key], PRODUCT, CHANNEL),
            Ok(manifest)
        );
    }

    #[test]
    fn the_backup_key_is_as_good_as_the_first() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (bytes, backup) = signed(&manifest);
        let (_, primary) = generate_signing_key().expect("key");
        assert!(verify_signed_manifest(&bytes, &[primary, backup], PRODUCT, CHANNEL).is_ok());
    }

    #[test]
    fn a_manifest_signed_by_anyone_else_is_refused() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (bytes, _) = signed(&manifest);
        let (_, stranger) = generate_signing_key().expect("key");
        assert_eq!(
            verify_signed_manifest(&bytes, &[stranger], PRODUCT, CHANNEL),
            Err(ManifestError::Untrusted)
        );
        assert_eq!(
            verify_signed_manifest(&bytes, &[], PRODUCT, CHANNEL),
            Err(ManifestError::Untrusted)
        );
    }

    #[test]
    fn changing_one_signed_byte_breaks_the_signature() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (bytes, key) = signed(&manifest);
        let mut envelope: Envelope = serde_json::from_slice(&bytes).expect("envelope");
        let forged = manifest_for("0.4.0", b"another archive");
        envelope.payload = BASE64.encode(serde_json::to_vec(&forged).expect("payload"));
        let tampered = serde_json::to_vec(&envelope).expect("tampered");
        assert_eq!(
            verify_signed_manifest(&tampered, &[key], PRODUCT, CHANNEL),
            Err(ManifestError::Untrusted)
        );
    }

    #[test]
    fn a_signature_is_bound_to_the_manifest_context() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (pkcs8, key) = generate_signing_key().expect("key");
        let payload = serde_json::to_vec(&manifest).expect("payload");
        let pair = Ed25519KeyPair::from_pkcs8(&pkcs8).expect("pair");
        // The same key signing the bare payload, as some other tool might.
        let envelope = Envelope {
            schema: MANIFEST_SCHEMA,
            payload: BASE64.encode(&payload),
            signatures: vec![EnvelopeSignature {
                key_id: key.id(),
                signature: BASE64.encode(pair.sign(&payload).as_ref()),
            }],
        };
        assert_eq!(
            verify_signed_manifest(
                &serde_json::to_vec(&envelope).expect("bytes"),
                &[key],
                PRODUCT,
                CHANNEL
            ),
            Err(ManifestError::Untrusted)
        );
    }

    #[test]
    fn another_products_or_channels_manifest_is_refused() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (bytes, key) = signed(&manifest);
        for (product, channel) in [("com.example.other", CHANNEL), (PRODUCT, "beta")] {
            assert_eq!(
                verify_signed_manifest(&bytes, &[key], product, channel),
                Err(ManifestError::WrongChannel)
            );
        }
    }

    type Spoil = fn(&mut ReleaseManifest);

    #[test]
    fn a_manifest_the_installer_could_not_act_on_is_never_signed_or_accepted() {
        let good = manifest_for("0.4.0", b"archive bytes");
        let cases: [(&str, Spoil, ManifestError); 8] = [
            (
                "a path in the name",
                |m| m.artifact.name = "../Interface-0.4.0.zip".to_owned(),
                ManifestError::InvalidArtifact,
            ),
            (
                "a name that is not a zip",
                |m| m.artifact.name = "Interface-0.4.0.dmg".to_owned(),
                ManifestError::InvalidArtifact,
            ),
            (
                "an empty archive",
                |m| m.artifact.size = 0,
                ManifestError::InvalidArtifact,
            ),
            (
                "a digest that is not SHA-512",
                |m| m.artifact.sha512 = BASE64.encode(b"short"),
                ManifestError::InvalidArtifact,
            ),
            (
                "a version that is not a version",
                |m| m.version = "latest".to_owned(),
                ManifestError::InvalidVersion,
            ),
            (
                "a feed for another version",
                |m| m.latest_mac_yml = m.latest_mac_yml.replace("version: 0.4.0", "version: 0.5.0"),
                ManifestError::InconsistentFeed,
            ),
            (
                "a feed with a second file",
                |m| {
                    m.latest_mac_yml = m.latest_mac_yml.replace(
                        "path:",
                        "  - url: Other.zip\n    sha512: x\n    size: 1\npath:",
                    );
                },
                ManifestError::InconsistentFeed,
            ),
            (
                "a format from the future",
                |m| m.schema = 2,
                ManifestError::UnsupportedSchema,
            ),
        ];
        let (pkcs8, _) = generate_signing_key().expect("key");
        for (name, spoil, expected) in cases {
            let mut manifest = good.clone();
            spoil(&mut manifest);
            assert_eq!(sign_manifest(&manifest, &pkcs8), Err(expected), "{name}");
        }
    }

    #[test]
    fn fields_a_later_publisher_adds_do_not_strand_this_client() {
        let manifest = manifest_for("0.4.0", b"archive bytes");
        let (pkcs8, key) = generate_signing_key().expect("key");
        let mut payload = serde_json::to_value(&manifest).expect("value");
        payload["minimumSystemVersion"] = serde_json::json!("27.0");
        payload["artifact"]["mirrors"] = serde_json::json!(["https://example.test/a.zip"]);
        let payload = serde_json::to_vec(&payload).expect("payload");
        let pair = Ed25519KeyPair::from_pkcs8(&pkcs8).expect("pair");
        let envelope = serde_json::json!({
            "schema": 1,
            "notes": "ignored",
            "payload": BASE64.encode(&payload),
            "signatures": [{
                "keyId": key.id(),
                "algorithm": "ed25519",
                "signature": BASE64.encode(pair.sign(&signed_message(&payload)).as_ref()),
            }],
        });
        assert_eq!(
            verify_signed_manifest(
                &serde_json::to_vec(&envelope).expect("bytes"),
                &[key],
                PRODUCT,
                CHANNEL
            ),
            Ok(manifest)
        );
    }

    #[test]
    fn oversize_and_malformed_input_is_refused_before_any_signature_work() {
        let (_, key) = generate_signing_key().expect("key");
        assert_eq!(
            verify_signed_manifest(
                &vec![b' '; MAX_SIGNED_MANIFEST_BYTES + 1],
                &[key],
                PRODUCT,
                CHANNEL
            ),
            Err(ManifestError::TooLarge)
        );
        for malformed in [
            &b""[..],
            b"{}",
            b"[]",
            b"{\"schema\":1,\"payload\":\"%%\",\"signatures\":[]}",
        ] {
            assert!(matches!(
                verify_signed_manifest(malformed, &[key], PRODUCT, CHANNEL),
                Err(ManifestError::Malformed | ManifestError::Untrusted)
            ));
        }
    }

    #[test]
    fn every_archive_name_a_manifest_may_carry_is_one_the_local_feed_serves() {
        for name in ["Interface-0.4.0-arm64-mac.zip", "Interface_1.0.0+build.zip"] {
            assert!(is_artifact_name(name), "{name}");
            assert_eq!(
                crate::update_feed_file_name(Some(&format!("/{name}"))).as_deref(),
                Some(name)
            );
        }
        for name in [
            ".zip",
            "-leading.zip",
            "a/b.zip",
            "a b.zip",
            "a%2f.zip",
            "a..zip",
            "latest-mac.yml",
        ] {
            assert!(!is_artifact_name(name), "{name}");
        }
    }

    #[test]
    fn a_keys_public_half_round_trips_and_names_itself() {
        let (pkcs8, key) = generate_signing_key().expect("key");
        assert_eq!(signing_public_key(&pkcs8), Ok(key));
        assert_eq!(PublicKey::from_base64(&key.to_base64()), Some(key));
        assert_eq!(key.id().len(), 16);
        assert_eq!(PublicKey::from_base64("not a key"), None);
    }
}
