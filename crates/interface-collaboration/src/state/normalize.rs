//! Normalization, bounds and hashing primitives: JavaScript-compatible trim,
//! canonical JSON, domain hashes and the intent payload and dedupe keys.

use std::fmt;

use chrono::{DateTime, Utc};
use interface_contracts::parse_canonical_timestamp;
use interface_storage::sha256_hex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::{
    CLIENT_REQUEST_ID_BYTES_MAX, CollaborationEngineId, CollaborationIntentInput,
    CollaborationIntentV1, CollaborationOrigin, ContractError, ContractResult,
    IDENTIFIER_BYTES_MAX, JS_MAX_SAFE_INTEGER, OBJECTIVE_BYTES_MAX, OBJECTIVE_CHARACTERS_MAX,
    ORIGIN_CAPABILITY_BYTES_MAX, PATH_BYTES_MAX, PROVIDER_SESSION_ID_BYTES_MAX,
    SOURCE_SESSION_KEY_BYTES_MAX,
};

fn has_unsafe_controls(value: &str, allow_newlines: bool) -> bool {
    value.chars().any(|character| {
        let code = u32::from(character);
        code == 0x1b
            || code == 0x7f
            || (code < 0x20 && !(allow_newlines && matches!(code, 0x09 | 0x0a)))
            || (0x80..=0x9f).contains(&code)
    })
}

fn javascript_trim(value: &str) -> &str {
    // ECMAScript TrimString includes the zero-width no-break space/BOM in
    // addition to Unicode whitespace. Rust's `str::trim` intentionally does
    // not, so spell out the bridge-compatible boundary.
    value.trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}')
}

pub(super) fn assert_normalized_opaque(
    value: &str,
    max_bytes: usize,
    min_bytes: usize,
    path: impl fmt::Display,
) -> ContractResult<()> {
    let bytes = value.len();
    invalid_if!(
        !value.nfc().eq(value.chars())
            || javascript_trim(value) != value
            || has_unsafe_controls(value, false)
            || bytes < min_bytes
            || bytes > max_bytes,
        path,
        "must be normalized, trimmed, single-line printable text within its byte bound"
    );
    Ok(())
}

fn normalized_opaque(
    value: &str,
    max_bytes: usize,
    min_bytes: usize,
    path: &str,
) -> ContractResult<String> {
    assert_normalized_opaque(value, max_bytes, min_bytes, path)?;
    Ok(value.to_owned())
}

fn normalized_objective(value: &str) -> ContractResult<String> {
    let normalized = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .nfc()
        .collect::<String>();
    let normalized = javascript_trim(&normalized).to_owned();
    let characters = normalized.chars().count();
    invalid_if!(
        normalized.is_empty()
            || has_unsafe_controls(&normalized, true)
            || characters > OBJECTIVE_CHARACTERS_MAX
            || normalized.len() > OBJECTIVE_BYTES_MAX,
        "intent.objective",
        "must contain safe normalized text within its character and byte bounds"
    );
    Ok(normalized)
}

pub(super) fn bounded_identifier(value: &str, path: impl fmt::Display) -> ContractResult<()> {
    assert_normalized_opaque(value, IDENTIFIER_BYTES_MAX, 1, path)
}

pub(super) fn safe_integer(value: u64, path: &str) -> ContractResult<()> {
    invalid_if!(
        value > JS_MAX_SAFE_INTEGER,
        path,
        "must be a JavaScript-safe non-negative integer"
    );
    Ok(())
}

pub(super) fn bounded_safe_line(value: &str, max_bytes: usize, path: &str) -> ContractResult<()> {
    assert_normalized_opaque(value, max_bytes, 1, path)
}

pub(super) fn bounded_body(value: &str, max_bytes: usize, path: &str) -> ContractResult<String> {
    invalid_if!(
        value.len() > max_bytes || has_unsafe_controls(value, true),
        path,
        "must be bounded UTF-8 text without unsafe controls"
    );
    Ok(value.to_owned())
}

pub(super) fn bounded_path(value: &str, path: &str) -> ContractResult<()> {
    bounded_safe_line(value, PATH_BYTES_MAX, path)?;
    invalid_if!(
        !value.starts_with('/')
            || value.contains("/../")
            || value.ends_with("/..")
            || value.contains("/./")
            || value.ends_with("/."),
        path,
        "must be an absolute canonical path without traversal segments"
    );
    Ok(())
}

pub(super) fn hash_value(value: &str, path: &str) -> ContractResult<()> {
    invalid_if!(
        value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        path,
        "must be a lowercase SHA-256 digest"
    );
    Ok(())
}

pub(crate) fn timestamp_millis(
    value: &str,
    path: impl fmt::Display,
) -> ContractResult<DateTime<Utc>> {
    parse_canonical_timestamp(value)
        .ok_or_else(|| ContractError::invalid(&path, "must be a canonical UTC timestamp"))
}

#[must_use]
pub fn sha256(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

#[must_use]
pub fn domain_hash(domain: &str, payload: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(payload.as_bytes());
    format!("{:x}", digest.finalize())
}

fn write_canonical_json(value: &Value, output: &mut String) {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            output.push_str(&serde_json::to_string(value).expect("JSON value serializes"));
        }
        Value::Array(values) => {
            output.push('[');
            for (index, item) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(item, output);
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).expect("JSON key serializes"));
                output.push(':');
                write_canonical_json(&values[key], output);
            }
            output.push('}');
        }
    }
}

pub(super) fn canonical_json(value: &Value) -> String {
    let mut output = String::new();
    write_canonical_json(value, &mut output);
    output
}

pub fn engine_from_session_key(session_key: &str) -> ContractResult<CollaborationEngineId> {
    let (engine, provider_id) =
        CollaborationEngineId::split_session_key(session_key).ok_or_else(|| {
            ContractError::invalid(
                "intent.sourceSessionKey",
                "does not use a supported engine prefix",
            )
        })?;
    assert_normalized_opaque(
        provider_id,
        PROVIDER_SESSION_ID_BYTES_MAX,
        1,
        "intent.sourceSessionKey provider ID",
    )?;
    Ok(engine)
}

pub fn provider_id_from_session_key(session_key: &str) -> ContractResult<&str> {
    let (_, provider_id) =
        CollaborationEngineId::split_session_key(session_key).ok_or_else(|| {
            ContractError::invalid(
                "record.source.sessionKey",
                "does not use a supported engine prefix",
            )
        })?;
    assert_normalized_opaque(
        provider_id,
        PROVIDER_SESSION_ID_BYTES_MAX,
        1,
        "session key provider ID",
    )?;
    Ok(provider_id)
}

pub fn normalize_collaboration_intent(
    input: &CollaborationIntentInput,
    origin_lineage_id: Option<&str>,
) -> ContractResult<CollaborationIntentV1> {
    invalid_if!(
        input.version.is_some_and(|version| version != 1),
        "intent.version",
        "must be the supported contract version"
    );
    let source_session_key = normalized_opaque(
        &input.source_session_key,
        SOURCE_SESSION_KEY_BYTES_MAX,
        1,
        "intent.sourceSessionKey",
    )?;
    let source_engine = engine_from_session_key(&source_session_key)?;
    invalid_if!(
        source_engine == input.target_engine,
        "intent.targetEngine",
        "must differ from source"
    );
    let objective = normalized_objective(&input.objective)?;
    let client_request_id = normalized_opaque(
        &input.client_request_id,
        CLIENT_REQUEST_ID_BYTES_MAX,
        1,
        "intent.clientRequestId",
    )?;
    match (input.origin, origin_lineage_id) {
        (CollaborationOrigin::Model, None) => {
            return Err(ContractError::invalid(
                "intent.origin",
                "model origin requires a stable source lineage",
            ));
        }
        (CollaborationOrigin::Human, Some(_)) => {
            return Err(ContractError::invalid(
                "intent.origin",
                "human origin cannot carry a model source lineage",
            ));
        }
        _ => {}
    }
    let objective_characters = u64::try_from(objective.chars().count()).unwrap_or(u64::MAX);
    let objective_bytes = u64::try_from(objective.len()).unwrap_or(u64::MAX);
    let origin_lineage_fingerprint = origin_lineage_id
        .map(fingerprint_origin_lineage)
        .transpose()?;
    let intent = CollaborationIntentV1 {
        version: 1,
        mode: input.mode,
        source_session_key,
        target_engine: input.target_engine,
        objective_digest: sha256(&objective),
        objective_bytes,
        objective_characters,
        objective: Some(objective),
        client_request_id,
        origin: input.origin,
        origin_lineage_fingerprint,
    };
    intent.validate()?;
    Ok(intent)
}

pub fn fingerprint_origin_lineage(lineage_id: &str) -> ContractResult<String> {
    assert_normalized_opaque(
        lineage_id,
        ORIGIN_CAPABILITY_BYTES_MAX,
        16,
        "origin source lineage",
    )?;
    Ok(domain_hash(
        "iface-collaboration-origin-lineage-v1",
        lineage_id,
    ))
}

impl CollaborationIntentV1 {
    pub fn validate(&self) -> ContractResult<()> {
        invalid_if!(
            self.version != 1,
            "intent.version",
            "must be the supported contract version"
        );
        assert_normalized_opaque(
            &self.source_session_key,
            SOURCE_SESSION_KEY_BYTES_MAX,
            1,
            "intent.sourceSessionKey",
        )?;
        let source_engine = engine_from_session_key(&self.source_session_key)?;
        invalid_if!(
            source_engine == self.target_engine,
            "intent.targetEngine",
            "must differ from source"
        );
        hash_value(&self.objective_digest, "intent.objectiveDigest")?;
        invalid_if!(
            self.objective_bytes == 0
                || self.objective_bytes > OBJECTIVE_BYTES_MAX as u64
                || self.objective_characters == 0
                || self.objective_characters > OBJECTIVE_CHARACTERS_MAX as u64,
            "intent.objectiveBytes",
            "is outside its bound"
        );
        if let Some(objective) = self.objective.as_deref() {
            let normalized = normalized_objective(objective)?;
            invalid_if!(
                normalized != objective
                    || sha256(objective) != self.objective_digest
                    || u64::try_from(objective.len()).unwrap_or(u64::MAX) != self.objective_bytes
                    || u64::try_from(objective.chars().count()).unwrap_or(u64::MAX)
                        != self.objective_characters,
                "intent.objective",
                "does not match its digest and byte count"
            );
        }
        assert_normalized_opaque(
            &self.client_request_id,
            CLIENT_REQUEST_ID_BYTES_MAX,
            1,
            "intent.clientRequestId",
        )?;
        match (self.origin, self.origin_lineage_fingerprint.as_deref()) {
            (CollaborationOrigin::Model, None) => {
                return Err(ContractError::invalid(
                    "intent.originLineageFingerprint",
                    "is required for model origin",
                ));
            }
            (CollaborationOrigin::Human, Some(_)) => {
                return Err(ContractError::invalid(
                    "intent.originLineageFingerprint",
                    "is forbidden for human origin",
                ));
            }
            (_, Some(fingerprint)) => {
                hash_value(fingerprint, "intent.originLineageFingerprint")?;
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn payload_hash_for_intent(intent: &CollaborationIntentV1) -> ContractResult<String> {
    intent.validate()?;
    Ok(payload_hash_for_validated_intent(intent))
}

pub(super) fn payload_hash_for_validated_intent(intent: &CollaborationIntentV1) -> String {
    let value = serde_json::json!({
        "version": intent.version,
        "mode": intent.mode,
        "sourceSessionKey": intent.source_session_key,
        "targetEngine": intent.target_engine,
        "objectiveDigest": intent.objective_digest,
        "objectiveBytes": intent.objective_bytes,
        "objectiveCharacters": intent.objective_characters,
        "clientRequestId": intent.client_request_id,
        "origin": intent.origin,
        "originLineageFingerprint": intent.origin_lineage_fingerprint,
    });
    domain_hash("iface-collaboration-payload-v1", &canonical_json(&value))
}

pub fn dedupe_key_for_intent(intent: &CollaborationIntentV1) -> ContractResult<String> {
    intent.validate()?;
    Ok(dedupe_key_for_validated_intent(intent))
}

pub(super) fn dedupe_key_for_validated_intent(intent: &CollaborationIntentV1) -> String {
    let origin_identity = match intent.origin {
        CollaborationOrigin::Human => "human".to_owned(),
        CollaborationOrigin::Model => format!(
            "model:{}",
            intent
                .origin_lineage_fingerprint
                .as_deref()
                .unwrap_or_default()
        ),
    };
    let value = serde_json::json!({
        "originIdentity": origin_identity,
        "sourceSessionKey": intent.source_session_key,
        "clientRequestId": intent.client_request_id,
    });
    domain_hash("iface-collaboration-dedupe-v1", &canonical_json(&value))
}

pub fn peer_result_id(collaboration_id: &str, result_digest: &str) -> ContractResult<String> {
    bounded_identifier(collaboration_id, "collaborationId")?;
    hash_value(result_digest, "resultDigest")?;
    let value = serde_json::json!({
        "collaborationId": collaboration_id,
        "resultDigest": result_digest,
    });
    Ok(domain_hash(
        "iface-collaboration-result-v1",
        &canonical_json(&value),
    ))
}
