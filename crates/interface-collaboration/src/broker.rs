//! Provider-free collaboration broker authority registry.
//!
//! Socket listeners and MCP calls intentionally live outside this crate. This
//! module ports the security-critical part of the runtime: unforgeable
//! out-of-band capabilities, exact source binding, duplicate ownership
//! refusal, depth fences, synchronous revocation and stable source lineage.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::state::{
    CollaborationEngineId, PROVIDER_SESSION_ID_BYTES_MAX, SOURCE_SESSION_KEY_BYTES_MAX,
    domain_hash, engine_from_session_key, provider_id_from_session_key,
};

const MAX_CAPABILITY_BYTES: usize = 512;
pub(crate) const MAX_PROVISIONAL_ID_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationModelAuthority {
    pub engine: CollaborationEngineId,
    pub source_session_keys: Vec<String>,
    pub source_lineage_ids: BTreeMap<String, String>,
    pub depth: u8,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BrokerSessionBinding {
    token: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BrokerSourceRegistration {
    token: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BrokerDepthFence {
    token: u64,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum BrokerError {
    #[error("collaboration broker is stopping")]
    Stopping,
    #[error("collaboration broker capability generation failed closed")]
    InvalidCapability,
    #[error("collaboration broker capability was already used")]
    DuplicateCapability,
    #[error("invalid collaboration broker session request")]
    InvalidSessionRequest,
    #[error("collaboration source identity is invalid")]
    InvalidSource,
    #[error("collaboration source identity does not match its provider session")]
    SourceMismatch,
    #[error("collaboration source already has an active broker binding")]
    DuplicateSourceOwner,
    #[error("collaboration broker session binding was already confirmed")]
    BindingAlreadyConfirmed,
    #[error("collaboration broker binding is no longer active")]
    BindingInactive,
}

#[derive(Debug)]
struct BoundAuthority {
    engine: CollaborationEngineId,
    capability_digest: String,
    provisional_session_id: Option<String>,
    resumed: bool,
    source_keys: BTreeMap<String, u64>,
}

/// In-memory authority core for the collaboration broker. Raw capabilities
/// are accepted at creation/lookup only and are never retained.
pub struct BrokerRuntime {
    accepting: bool,
    next_token: u64,
    codex_bound: u64,
    authorities: BTreeMap<String, u64>,
    bounds: BTreeMap<u64, BoundAuthority>,
    source_owners: BTreeMap<String, (u64, u64)>,
    depth_fences: BTreeSet<u64>,
    used_capabilities: BTreeSet<String>,
}

impl BrokerRuntime {
    /// Creates the process-scoped Codex authority. The caller gives the raw
    /// capability directly to its transport; this runtime keeps only a digest.
    pub fn new(codex_capability: &str) -> Result<Self, BrokerError> {
        let digest = capability_digest_checked(codex_capability)?;
        let codex_bound = 1;
        let mut authorities = BTreeMap::new();
        authorities.insert(digest.clone(), codex_bound);
        let mut bounds = BTreeMap::new();
        bounds.insert(
            codex_bound,
            BoundAuthority {
                engine: CollaborationEngineId::Codex,
                capability_digest: digest.clone(),
                provisional_session_id: None,
                resumed: false,
                source_keys: BTreeMap::new(),
            },
        );
        Ok(Self {
            accepting: true,
            next_token: 2,
            codex_bound,
            authorities,
            bounds,
            source_owners: BTreeMap::new(),
            depth_fences: BTreeSet::new(),
            used_capabilities: BTreeSet::from([digest]),
        })
    }

    #[must_use]
    pub fn generate_capability() -> String {
        // A UUID plus domain-separated entropy is bounded, printable and well
        // above the legacy 16-byte minimum. UUID v4 randomness is OS-backed.
        format!("iface-{}", Uuid::new_v4())
    }

    pub fn create_session_binding(
        &mut self,
        engine: CollaborationEngineId,
        provisional_session_id: &str,
        resumed: bool,
        raw_capability: &str,
    ) -> Result<BrokerSessionBinding, BrokerError> {
        self.assert_accepting()?;
        if engine == CollaborationEngineId::Codex
            || provisional_session_id.is_empty()
            || provisional_session_id.len() > MAX_PROVISIONAL_ID_BYTES
            || unsafe_controls(provisional_session_id)
        {
            return Err(BrokerError::InvalidSessionRequest);
        }
        let digest = capability_digest_checked(raw_capability)?;
        if !self.used_capabilities.insert(digest.clone()) {
            return Err(BrokerError::DuplicateCapability);
        }
        let token = self.take_token()?;
        self.authorities.insert(digest.clone(), token);
        self.bounds.insert(
            token,
            BoundAuthority {
                engine,
                capability_digest: digest,
                provisional_session_id: Some(provisional_session_id.to_owned()),
                resumed,
                source_keys: BTreeMap::new(),
            },
        );
        Ok(BrokerSessionBinding { token })
    }

    pub fn confirm_source_session_key(
        &mut self,
        binding: BrokerSessionBinding,
        source_key: &str,
    ) -> Result<(), BrokerError> {
        self.assert_accepting()?;
        let (engine, provisional, resumed, existing) = {
            let bound = self
                .bounds
                .get(&binding.token)
                .ok_or(BrokerError::BindingInactive)?;
            (
                bound.engine,
                bound.provisional_session_id.clone(),
                bound.resumed,
                bound.source_keys.keys().next().cloned(),
            )
        };
        if engine == CollaborationEngineId::Codex {
            return Err(BrokerError::BindingInactive);
        }
        validate_source_for_engine(
            source_key,
            engine,
            if resumed || engine == CollaborationEngineId::Claude {
                provisional.as_deref()
            } else {
                None
            },
        )?;
        if let Some(current) = existing {
            if current == source_key {
                return Ok(());
            }
            return Err(BrokerError::BindingAlreadyConfirmed);
        }
        if self.source_owners.contains_key(source_key) {
            return Err(BrokerError::DuplicateSourceOwner);
        }
        let owner_token = self.take_token()?;
        self.source_owners
            .insert(source_key.to_owned(), (binding.token, owner_token));
        self.bounds
            .get_mut(&binding.token)
            .ok_or(BrokerError::BindingInactive)?
            .source_keys
            .insert(source_key.to_owned(), owner_token);
        Ok(())
    }

    /// Revokes authority synchronously. Any later/slow transport shutdown is
    /// outside this core and cannot keep the capability live.
    pub fn dispose_session_binding(
        &mut self,
        binding: BrokerSessionBinding,
    ) -> Result<(), BrokerError> {
        let Some(bound) = self.bounds.get(&binding.token) else {
            return Ok(());
        };
        if bound.engine == CollaborationEngineId::Codex {
            return Err(BrokerError::BindingInactive);
        }
        self.revoke(binding.token);
        Ok(())
    }

    pub fn register_codex_source(
        &mut self,
        source_key: &str,
    ) -> Result<BrokerSourceRegistration, BrokerError> {
        self.assert_accepting()?;
        if !self.bounds.contains_key(&self.codex_bound) {
            return Err(BrokerError::Stopping);
        }
        validate_source_for_engine(source_key, CollaborationEngineId::Codex, None)?;
        if self.source_owners.contains_key(source_key) {
            return Err(BrokerError::DuplicateSourceOwner);
        }
        let token = self.take_token()?;
        self.source_owners
            .insert(source_key.to_owned(), (self.codex_bound, token));
        self.bounds
            .get_mut(&self.codex_bound)
            .ok_or(BrokerError::Stopping)?
            .source_keys
            .insert(source_key.to_owned(), token);
        Ok(BrokerSourceRegistration { token })
    }

    pub fn dispose_codex_source(
        &mut self,
        registration: BrokerSourceRegistration,
    ) -> Result<(), BrokerError> {
        let source = self
            .source_owners
            .iter()
            .find_map(|(source, (bound, token))| {
                (*bound == self.codex_bound && *token == registration.token).then(|| source.clone())
            });
        if let Some(source) = source {
            self.source_owners.remove(&source);
            if let Some(codex) = self.bounds.get_mut(&self.codex_bound) {
                codex.source_keys.remove(&source);
            }
        }
        Ok(())
    }

    pub fn enter_codex_depth_fence(&mut self) -> Result<BrokerDepthFence, BrokerError> {
        self.assert_accepting()?;
        let token = self.take_token()?;
        self.depth_fences.insert(token);
        Ok(BrokerDepthFence { token })
    }

    pub fn leave_codex_depth_fence(&mut self, fence: BrokerDepthFence) {
        self.depth_fences.remove(&fence.token);
    }

    #[must_use]
    pub fn resolve_model_authority(
        &self,
        origin_connection_id: &str,
    ) -> Option<CollaborationModelAuthority> {
        let digest = capability_digest_checked(origin_connection_id).ok()?;
        let bound_id = *self.authorities.get(&digest)?;
        let bound = self.bounds.get(&bound_id)?;
        let source_session_keys =
            if bound.engine == CollaborationEngineId::Codex && !self.depth_fences.is_empty() {
                Vec::new()
            } else {
                bound.source_keys.keys().cloned().collect()
            };
        let source_lineage_ids = source_session_keys
            .iter()
            .map(|key| (key.clone(), collaboration_source_lineage_id(key)))
            .collect();
        Some(CollaborationModelAuthority {
            engine: bound.engine,
            source_session_keys,
            source_lineage_ids,
            depth: 0,
        })
    }

    /// Immediately revokes every capability and source owner.
    pub fn stop(&mut self) {
        if !self.accepting {
            return;
        }
        self.accepting = false;
        self.depth_fences.clear();
        let tokens: Vec<_> = self.bounds.keys().copied().collect();
        for token in tokens {
            self.revoke(token);
        }
    }

    pub fn drain(&self) -> Result<(), BrokerError> {
        // This core has no live transports. The method intentionally exists so
        // transport adapters can preserve the legacy stop/drain ordering.
        Ok(())
    }

    /// Drops the binding outright: every reader treats a missing entry as
    /// revoked, and `used_capabilities` keeps the digest from being reissued.
    fn revoke(&mut self, token: u64) {
        let Some(bound) = self.bounds.remove(&token) else {
            return;
        };
        self.authorities.remove(&bound.capability_digest);
        for source in bound.source_keys.keys() {
            if self
                .source_owners
                .get(source)
                .is_some_and(|(owner, _)| *owner == token)
            {
                self.source_owners.remove(source);
            }
        }
    }

    fn assert_accepting(&self) -> Result<(), BrokerError> {
        if !self.accepting {
            return Err(BrokerError::Stopping);
        }
        Ok(())
    }

    fn take_token(&mut self) -> Result<u64, BrokerError> {
        let token = self.next_token;
        self.next_token = self
            .next_token
            .checked_add(1)
            .ok_or(BrokerError::Stopping)?;
        Ok(token)
    }
}

fn unsafe_controls(value: &str) -> bool {
    value
        .chars()
        .any(|character| u32::from(character) < 0x20 || u32::from(character) == 0x7f)
}

fn safe_capability(value: &str) -> bool {
    value.len() >= 16 && value.len() <= MAX_CAPABILITY_BYTES && !unsafe_controls(value)
}

fn capability_digest_checked(value: &str) -> Result<String, BrokerError> {
    if !safe_capability(value) {
        return Err(BrokerError::InvalidCapability);
    }
    Ok(domain_hash("iface-collaboration-capability", value))
}

#[must_use]
pub fn collaboration_source_lineage_id(source_session_key: &str) -> String {
    domain_hash("iface-collaboration-source-lineage-v1", source_session_key)
}

fn validate_source_for_engine(
    source_key: &str,
    engine: CollaborationEngineId,
    exact_provider_id: Option<&str>,
) -> Result<(), BrokerError> {
    if source_key.is_empty()
        || source_key.len() > SOURCE_SESSION_KEY_BYTES_MAX
        || unsafe_controls(source_key)
    {
        return Err(BrokerError::InvalidSource);
    }
    if engine_from_session_key(source_key).map_err(|_| BrokerError::InvalidSource)? != engine {
        return Err(BrokerError::SourceMismatch);
    }
    let provider_id =
        provider_id_from_session_key(source_key).map_err(|_| BrokerError::InvalidSource)?;
    if provider_id.len() > PROVIDER_SESSION_ID_BYTES_MAX
        || exact_provider_id.is_some_and(|expected| expected != provider_id)
        || source_key.strip_prefix(engine.session_prefix()) != Some(provider_id)
    {
        return Err(BrokerError::SourceMismatch);
    }
    Ok(())
}
