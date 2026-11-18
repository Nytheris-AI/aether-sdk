use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Utc};
use nytheris_aether_core::{AetherError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::types::{ContentHash, ScopeTag};

/// Validated lowercase non-empty agent identifier (`[a-z0-9._-]`, 1..=128 bytes).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentId(String);

impl AgentId {
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw_s = raw.into();
        let norm = raw_s.trim().to_ascii_lowercase();
        if norm.is_empty()
            || norm.len() > 128
            || !norm.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            })
        {
            return Err(AetherError::InvalidInput(format!(
                "invalid agent_id `{raw_s}`"
            )));
        }
        Ok(Self(norm))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AgentId {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<AgentId> for String {
    fn from(value: AgentId) -> Self {
        value.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Public key identifier (`key_<16-hex>`) used to list and revoke keys without exposing secret material.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeyId(String);

impl KeyId {
    pub fn generate() -> Self {
        let id = Uuid::new_v4();
        Self(format!("key_{}", hex::encode(&id.as_bytes()[..8])))
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let norm = raw.trim().to_ascii_lowercase();
        let Some(rest) = norm.strip_prefix("key_") else {
            return Err(AetherError::InvalidInput(format!("invalid key_id `{raw}`")));
        };
        if !(16..=32).contains(&rest.len()) || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AetherError::InvalidInput(format!(
                "invalid key_id hex `{raw}`"
            )));
        }
        Ok(Self(norm))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for KeyId {
    type Error = AetherError;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<KeyId> for String {
    fn from(value: KeyId) -> Self {
        value.0
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque API key secret (`aek_<64-hex>`). `Debug` and `Display` always emit `[REDACTED]`.
/// Does not implement `Serialize` so secrets cannot accidentally leak into JSON responses.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKeySecret(String);

impl ApiKeySecret {
    /// Generates 256 bits of OS entropy via two `Uuid::new_v4()` calls.
    pub fn generate() -> Self {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut raw = [0u8; 32];
        raw[..16].copy_from_slice(a.as_bytes());
        raw[16..].copy_from_slice(b.as_bytes());
        Self(format!("aek_{}", hex::encode(raw)))
    }

    /// Validates `aek_<hex>` format (`>= 32` hex characters, no whitespace/control chars).
    pub fn parse(raw: &str) -> Result<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        let Some(hex_part) = lower.strip_prefix("aek_") else {
            return Err(AetherError::InvalidInput(
                "api key must start with `aek_`".into(),
            ));
        };
        if hex_part.len() < 32 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AetherError::InvalidInput(
                "api key hex payload invalid or too short".into(),
            ));
        }
        Ok(Self(lower))
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Domain-separated SHA-256 digest persisted in SQLite `agent_api_keys.key_hash`.
    pub(crate) fn digest(&self) -> ContentHash {
        let mut hasher = Sha256::new();
        hasher.update(b"aether:api_key:v1:");
        hasher.update(self.0.as_bytes());
        let bytes: [u8; 32] = hasher.finalize().into();
        ContentHash::parse(&hex::encode(bytes)).expect("32-byte sha256 hex is always valid")
    }
}

impl fmt::Debug for ApiKeySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for ApiKeySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Role hierarchy for authenticated HTTP and MCP operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Reader,
    Contributor,
    Admin,
}

impl AgentRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Contributor => "contributor",
            Self::Admin => "admin",
        }
    }

    pub fn parse_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "reader" => Some(Self::Reader),
            "contributor" => Some(Self::Contributor),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    pub fn satisfies(self, required: AgentRole) -> bool {
        self >= required
    }
}

/// Typed authorization/authentication failure mapped to HTTP 401/403 at `http.rs`
/// and `AetherError::Permanent { connector: "vault-auth", .. }` inside Rust callers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("missing bearer token")]
    MissingToken,
    #[error("invalid API key format or unrecognized secret")]
    InvalidToken,
    #[error("API key `{0}` has been revoked")]
    RevokedToken(KeyId),
    #[error("API key `{0}` has expired")]
    ExpiredToken(KeyId),
    #[error("insufficient role: requires `{required:?}`, caller has `{actual:?}`")]
    InsufficientRole {
        required: AgentRole,
        actual: AgentRole,
    },
    #[error("scope `{requested}` denied for agent `{agent_id}`")]
    ScopeDenied {
        agent_id: AgentId,
        requested: ScopeTag,
    },
}

impl From<AuthError> for AetherError {
    fn from(err: AuthError) -> Self {
        AetherError::Permanent {
            connector: "vault-auth",
            message: err.to_string(),
        }
    }
}

/// Persisted and authenticated key record. Never holds plaintext secret or key hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedApiKey {
    pub key_id: KeyId,
    pub agent_id: AgentId,
    pub role: AgentRole,
    scopes: Vec<ScopeTag>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ScopedApiKey {
    pub fn new(
        key_id: KeyId,
        agent_id: AgentId,
        role: AgentRole,
        scopes: Vec<ScopeTag>,
        created_at: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
    ) -> Result<Self> {
        if scopes.is_empty() {
            return Err(AetherError::InvalidInput(
                "scoped api key must bind at least one ScopeTag".into(),
            ));
        }
        let mut seen = HashSet::new();
        let deduped: Vec<ScopeTag> = scopes
            .into_iter()
            .filter(|s| seen.insert(s.clone()))
            .collect();
        Ok(Self {
            key_id,
            agent_id,
            role,
            scopes: deduped,
            created_at,
            expires_at,
            revoked_at,
        })
    }

    pub fn scopes(&self) -> &[ScopeTag] {
        &self.scopes
    }

    pub fn has_wildcard_scope(&self) -> bool {
        self.scopes.iter().any(|s| s.as_str() == "*")
    }

    pub fn is_active_at(&self, now: DateTime<Utc>) -> std::result::Result<(), AuthError> {
        if self.revoked_at.is_some() {
            return Err(AuthError::RevokedToken(self.key_id.clone()));
        }
        if self.expires_at.is_some_and(|exp| exp <= now) {
            return Err(AuthError::ExpiredToken(self.key_id.clone()));
        }
        Ok(())
    }

    pub fn require_role(&self, required: AgentRole) -> std::result::Result<(), AuthError> {
        if self.role.satisfies(required) {
            Ok(())
        } else {
            Err(AuthError::InsufficientRole {
                required,
                actual: self.role,
            })
        }
    }

    pub fn intersect_scopes(
        &self,
        requested: Option<&ScopeTag>,
    ) -> std::result::Result<Vec<ScopeTag>, AuthError> {
        match requested {
            None => Ok(self.scopes.clone()),
            Some(req) if self.has_wildcard_scope() => Ok(vec![req.clone()]),
            Some(req) if req.as_str() == "global" => Ok(vec![ScopeTag::global()]),
            Some(req) if self.scopes.contains(req) => Ok(vec![req.clone()]),
            Some(req) => Err(AuthError::ScopeDenied {
                agent_id: self.agent_id.clone(),
                requested: req.clone(),
            }),
        }
    }

    pub fn authorize_write_scope(&self, target: &ScopeTag) -> std::result::Result<(), AuthError> {
        if self.has_wildcard_scope() || self.scopes.contains(target) {
            Ok(())
        } else {
            Err(AuthError::ScopeDenied {
                agent_id: self.agent_id.clone(),
                requested: target.clone(),
            })
        }
    }

    pub fn can_read_resource_scopes(&self, resource_scopes: &[ScopeTag]) -> bool {
        if self.has_wildcard_scope() || resource_scopes.is_empty() {
            return true;
        }
        resource_scopes
            .iter()
            .any(|s| s.as_str() == "global" || self.scopes.contains(s))
    }

    /// Returns the first concrete scope bound to this key, falling back to `global` for wildcard keys.
    pub fn default_write_scope(&self) -> ScopeTag {
        self.scopes
            .iter()
            .find(|s| s.as_str() != "*")
            .cloned()
            .unwrap_or_else(ScopeTag::global)
    }
}

/// Input to `VaultEngine::issue_api_key`. Enforces non-empty `scopes` at construction.
#[derive(Debug, Clone)]
pub struct IssueApiKeyRequest {
    pub agent_id: AgentId,
    pub role: AgentRole,
    pub scopes: Vec<ScopeTag>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl IssueApiKeyRequest {
    pub fn new(agent_id: AgentId, role: AgentRole, scopes: Vec<ScopeTag>) -> Result<Self> {
        if scopes.is_empty() {
            return Err(AetherError::InvalidInput(
                "IssueApiKeyRequest requires at least one ScopeTag".into(),
            ));
        }
        let mut seen = HashSet::new();
        let deduped: Vec<ScopeTag> = scopes
            .into_iter()
            .filter(|s| seen.insert(s.clone()))
            .collect();
        Ok(Self {
            agent_id,
            role,
            scopes: deduped,
            expires_at: None,
        })
    }

    pub fn with_expires_at(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }
}

/// Returned strictly once when issuing a key.
#[derive(Debug, Clone)]
pub struct IssuedApiKey {
    pub key: ScopedApiKey,
    pub secret: ApiKeySecret,
}

/// Crate-internal scope policy passed into unified `VaultEngine` query/inspect pipelines.
#[derive(Debug, Clone)]
pub(crate) enum ScopePolicy {
    Unrestricted,
    Allowed(Vec<ScopeTag>),
}

/// Constant-time slice comparison for SHA-256 hex digests.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    #[test]
    fn api_key_secret_redacts_debug_display_and_validates_format() {
        let secret = ApiKeySecret::generate();
        assert!(secret.expose_secret().starts_with("aek_"));
        assert_eq!(secret.expose_secret().len(), 68);
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(format!("{secret}"), "[REDACTED]");

        let parsed = ApiKeySecret::parse(&secret.expose_secret().to_ascii_uppercase()).unwrap();
        assert_eq!(parsed, secret);
        assert_eq!(parsed.digest(), secret.digest());

        assert!(ApiKeySecret::parse("bad_prefix_0123456789abcdef0123456789abcdef").is_err());
        assert!(ApiKeySecret::parse("aek_short").is_err());
        assert!(ApiKeySecret::parse("aek_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
    }

    #[test]
    fn agent_id_key_id_and_scoped_api_key_enforce_invariants() {
        let agent = AgentId::new("  CI-Backend_01 ").unwrap();
        assert_eq!(agent.as_str(), "ci-backend_01");
        assert!(AgentId::new("   ").is_err());
        assert!(AgentId::new("bad/agent").is_err());

        let key_id = KeyId::generate();
        assert_eq!(KeyId::parse(key_id.as_str()).unwrap(), key_id);
        assert!(KeyId::parse("key_short").is_err());
        assert!(KeyId::parse("bad_0123456789abcdef").is_err());

        let eng = ScopeTag::new("engineering").unwrap();
        let ops = ScopeTag::new("ops").unwrap();
        let hr = ScopeTag::new("hr").unwrap();

        assert!(ScopedApiKey::new(
            key_id.clone(),
            agent.clone(),
            AgentRole::Contributor,
            vec![],
            Utc::now(),
            None,
            None,
        )
        .is_err());

        let key = ScopedApiKey::new(
            key_id,
            agent.clone(),
            AgentRole::Contributor,
            vec![eng.clone(), ops.clone(), eng.clone()],
            Utc::now(),
            Some(Utc::now() + Duration::hours(1)),
            None,
        )
        .unwrap();
        assert_eq!(key.scopes(), &[eng.clone(), ops.clone()]);

        assert!(key.require_role(AgentRole::Reader).is_ok());
        assert!(key.require_role(AgentRole::Contributor).is_ok());
        assert!(matches!(
            key.require_role(AgentRole::Admin),
            Err(AuthError::InsufficientRole { .. })
        ));

        assert_eq!(
            key.intersect_scopes(None).unwrap(),
            vec![eng.clone(), ops.clone()]
        );
        assert_eq!(key.intersect_scopes(Some(&eng)).unwrap(), vec![eng.clone()]);
        assert_eq!(
            key.intersect_scopes(Some(&ScopeTag::global())).unwrap(),
            vec![ScopeTag::global()]
        );
        assert!(matches!(
            key.intersect_scopes(Some(&hr)),
            Err(AuthError::ScopeDenied { .. })
        ));

        assert!(key.authorize_write_scope(&eng).is_ok());
        assert!(matches!(
            key.authorize_write_scope(&ScopeTag::global()),
            Err(AuthError::ScopeDenied { .. })
        ));
        assert!(key.can_read_resource_scopes(&[ScopeTag::global()]));
        assert!(key.can_read_resource_scopes(&[hr.clone(), ops]));
        assert!(!key.can_read_resource_scopes(&[hr]));
    }

    #[test]
    fn scoped_api_key_checks_expiration_and_revocation() {
        let now = Utc::now();
        let key_id = KeyId::generate();
        let agent = AgentId::new("reader").unwrap();
        let eng = ScopeTag::new("engineering").unwrap();

        let expired = ScopedApiKey::new(
            key_id.clone(),
            agent.clone(),
            AgentRole::Reader,
            vec![eng.clone()],
            now - Duration::hours(2),
            Some(now - Duration::minutes(1)),
            None,
        )
        .unwrap();
        assert!(matches!(
            expired.is_active_at(now),
            Err(AuthError::ExpiredToken(_))
        ));

        let revoked = ScopedApiKey::new(
            key_id,
            agent,
            AgentRole::Reader,
            vec![eng],
            now - Duration::hours(2),
            None,
            Some(now - Duration::minutes(1)),
        )
        .unwrap();
        assert!(matches!(
            revoked.is_active_at(now),
            Err(AuthError::RevokedToken(_))
        ));
    }
}
