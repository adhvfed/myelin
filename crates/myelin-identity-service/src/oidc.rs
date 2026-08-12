use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock, TryLockError};

use base64::Engine as _;

const OIDC_LOGIN_MATERIAL_PREFIX: &str = "oidc-login.v1.";
const JWKS_REFRESH_INTERVAL_SECS: i64 = 15 * 60;
const JWKS_REFRESH_COOLDOWN_SECS: i64 = 30;
const MAX_OIDC_SCOPE_CLAIM_BYTES: usize = 128;
const MAX_OIDC_SUBJECT_BYTES: usize = 512;
const MAX_OIDC_REPLAY_ID_BYTES: usize = 512;

pub fn oidc_login_material(id_token: &str, expected_nonce: &str) -> Result<String, AuthzError> {
    if !valid_transaction_nonce(expected_nonce) {
        return Err(AuthzError::BadRequest(
            "OIDC login nonce must be 32 random bytes encoded as base64url".into(),
        ));
    }
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "id_token": id_token,
            "expected_nonce": expected_nonce,
        }))
        .map_err(|_| AuthzError::BadRequest("cannot encode OIDC login material".into()))?,
    );
    Ok(format!("{OIDC_LOGIN_MATERIAL_PREFIX}{encoded}"))
}

fn valid_transaction_nonce(nonce: &str) -> bool {
    nonce.len() == 43
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn valid_scope_claim(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_OIDC_SCOPE_CLAIM_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_opaque_claim(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn unwrap_login_material(material: &str) -> Result<(Cow<'_, str>, Option<String>), AuthzError> {
    let Some(encoded) = material.strip_prefix(OIDC_LOGIN_MATERIAL_PREFIX) else {
        return Ok((Cow::Borrowed(material), None));
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AuthzError::BadRequest("malformed bound OIDC login material".into()))?;
    let envelope: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| AuthzError::BadRequest("malformed bound OIDC login material".into()))?;
    let id_token = envelope
        .get("id_token")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AuthzError::BadRequest("malformed bound OIDC login material".into()))?;
    let expected_nonce = envelope
        .get("expected_nonce")
        .and_then(|value| value.as_str())
        .ok_or_else(|| AuthzError::BadRequest("malformed bound OIDC login material".into()))?;
    if !valid_transaction_nonce(expected_nonce) {
        return Err(AuthzError::BadRequest(
            "OIDC login nonce must be 32 random bytes encoded as base64url".into(),
        ));
    }
    Ok((
        Cow::Owned(id_token.to_string()),
        Some(expected_nonce.to_string()),
    ))
}

fn b64url(segment: &str) -> Result<Vec<u8>, AuthzError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment.as_bytes())
        .map_err(|e| AuthzError::BadRequest(format!("malformed base64url JWT segment: {e}")))
}

fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

#[derive(Clone, Debug)]
pub enum JwkKey {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    EcP256 { x: Vec<u8>, y: Vec<u8> },
    Ed25519 { x: Vec<u8> },
}

impl JwkKey {
    pub fn expected_alg(&self) -> &'static str {
        match self {
            JwkKey::Rsa { .. } => "RS256",
            JwkKey::EcP256 { .. } => "ES256",
            JwkKey::Ed25519 { .. } => "EdDSA",
        }
    }

    fn validate_shape(&self) -> Result<(), String> {
        match self {
            JwkKey::Rsa { n, e } => {
                let modulus = rsa::BigUint::from_bytes_be(n);
                if modulus.bits() < 2048 {
                    return Err("RSA JWKS modulus must be at least 2048 bits".into());
                }
                let exponent = rsa::BigUint::from_bytes_be(e);
                if exponent < rsa::BigUint::from(3_u8) || e.last().is_none_or(|byte| byte & 1 == 0)
                {
                    return Err("RSA JWKS exponent must be an odd integer of at least 3".into());
                }
            }
            JwkKey::EcP256 { x, y } if x.len() != 32 || y.len() != 32 => {
                return Err("P-256 JWKS coordinates must each be 32 bytes".into());
            }
            JwkKey::Ed25519 { x } if x.len() != 32 => {
                return Err("Ed25519 JWKS public key must be 32 bytes".into());
            }
            JwkKey::EcP256 { .. } | JwkKey::Ed25519 { .. } => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct JwkSet {
    by_kid: BTreeMap<String, JwkKey>,
}

type JwksRefreshFn = Arc<dyn Fn() -> Result<JwkSet, AuthzError> + Send + Sync>;

struct JwksCache {
    keys: JwkSet,
    last_success: i64,
    last_attempt: i64,
}

#[derive(Clone)]
struct JwksSource {
    cache: Arc<RwLock<JwksCache>>,
    refresh: Option<JwksRefreshFn>,
    refresh_gate: Arc<Mutex<()>>,
}

impl JwksSource {
    fn fixed(keys: JwkSet) -> Self {
        Self {
            cache: Arc::new(RwLock::new(JwksCache {
                keys,
                last_success: 0,
                last_attempt: i64::MIN,
            })),
            refresh: None,
            refresh_gate: Arc::new(Mutex::new(())),
        }
    }

    fn with_refresh(mut self, now: i64, refresh: JwksRefreshFn) -> Self {
        {
            let mut cache = self
                .cache
                .write()
                .unwrap_or_else(|error| error.into_inner());
            cache.last_success = now;
        }
        self.refresh = Some(refresh);
        self
    }

    fn refreshable(&self) -> bool {
        self.refresh.is_some()
    }

    fn key_for(&self, kid: &str, now: i64, force: bool) -> Result<JwkKey, AuthzError> {
        let (cached, stale) = {
            let cache = self.cache.read().unwrap_or_else(|error| error.into_inner());
            (
                cache.keys.get(kid).cloned(),
                now.saturating_sub(cache.last_success) >= JWKS_REFRESH_INTERVAL_SECS,
            )
        };
        if !force && (!stale || self.refresh.is_none()) {
            if let Some(key) = cached.clone() {
                return Ok(key);
            }
        }
        let Some(refresh) = &self.refresh else {
            return cached.ok_or_else(|| refuse("unknown `kid` (not in the JWKS)"));
        };

        let _refresh_guard = match self.refresh_gate.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => match cached.clone() {
                Some(key) => return Ok(key),
                None => self
                    .refresh_gate
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            },
        };

        let (current, due, cooling_down) = {
            let cache = self.cache.read().unwrap_or_else(|error| error.into_inner());
            let current = cache.keys.get(kid).cloned();
            let stale = now.saturating_sub(cache.last_success) >= JWKS_REFRESH_INTERVAL_SECS;
            let due = force || current.is_none() || stale;
            let cooling_down = now.saturating_sub(cache.last_attempt) < JWKS_REFRESH_COOLDOWN_SECS;
            (current, due, cooling_down)
        };
        if !due || cooling_down {
            return current.ok_or_else(|| refuse("unknown `kid` (JWKS refresh rate-limited)"));
        }
        {
            let mut cache = self
                .cache
                .write()
                .unwrap_or_else(|error| error.into_inner());
            cache.last_attempt = now;
        }

        match refresh() {
            Ok(keys) if !keys.is_empty() => {
                let mut cache = self
                    .cache
                    .write()
                    .unwrap_or_else(|error| error.into_inner());
                cache.keys = keys;
                cache.last_success = now;
            }
            Ok(_) | Err(_) => {
                return current.ok_or_else(|| {
                    refuse("unknown `kid` and no usable refreshed JWKS is available")
                })
            }
        }
        self.cache
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .keys
            .get(kid)
            .cloned()
            .ok_or_else(|| refuse("unknown `kid` after JWKS refresh"))
    }
}

impl JwkSet {
    pub fn new() -> JwkSet {
        JwkSet {
            by_kid: BTreeMap::new(),
        }
    }

    pub fn with_key(mut self, kid: impl Into<String>, key: JwkKey) -> JwkSet {
        self.by_kid.insert(kid.into(), key);
        self
    }

    pub fn get(&self, kid: &str) -> Option<&JwkKey> {
        self.by_kid.get(kid)
    }

    pub fn len(&self) -> usize {
        self.by_kid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_kid.is_empty()
    }

    pub fn from_jwks_json(doc: &str) -> Result<JwkSet, AuthzError> {
        let v: serde_json::Value = serde_json::from_str(doc)
            .map_err(|e| AuthzError::BadRequest(format!("malformed JWKS JSON: {e}")))?;
        let keys = v
            .get("keys")
            .and_then(|k| k.as_array())
            .ok_or_else(|| AuthzError::BadRequest("JWKS missing `keys` array".into()))?;
        let mut set = JwkSet::new();
        for k in keys {
            let kid = match k.get("kid").and_then(|x| x.as_str()) {
                Some(kid) if !kid.is_empty() => kid.to_string(),
                _ => continue,
            };
            let kty = k.get("kty").and_then(|x| x.as_str()).unwrap_or("");
            let dec = |field: &str| -> Result<Vec<u8>, AuthzError> {
                let s = k
                    .get(field)
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| AuthzError::BadRequest(format!("JWKS key missing `{field}`")))?;
                b64url(s)
            };
            let parsed = match kty {
                "RSA" => Some(JwkKey::Rsa {
                    n: dec("n")?,
                    e: dec("e")?,
                }),
                "EC" => {
                    let crv = k.get("crv").and_then(|x| x.as_str()).unwrap_or("");
                    if crv != "P-256" {
                        continue;
                    }
                    Some(JwkKey::EcP256 {
                        x: dec("x")?,
                        y: dec("y")?,
                    })
                }
                "OKP" => {
                    let crv = k.get("crv").and_then(|x| x.as_str()).unwrap_or("");
                    if crv != "Ed25519" {
                        continue;
                    }
                    Some(JwkKey::Ed25519 { x: dec("x")? })
                }
                _ => None,
            };
            let Some(key) = parsed else { continue };
            if !jwk_allows_verification(k, key.expected_alg())? {
                continue;
            }
            key.validate_shape().map_err(AuthzError::BadRequest)?;
            if set.by_kid.contains_key(&kid) {
                return Err(AuthzError::BadRequest(
                    "JWKS contains duplicate supported `kid` values".into(),
                ));
            }
            set = set.with_key(kid, key);
        }
        Ok(set)
    }
}

fn jwk_allows_verification(
    value: &serde_json::Value,
    expected_alg: &str,
) -> Result<bool, AuthzError> {
    if let Some(public_use) = value.get("use") {
        let public_use = public_use
            .as_str()
            .ok_or_else(|| AuthzError::BadRequest("JWKS `use` must be a string".into()))?;
        if public_use != "sig" {
            return Ok(false);
        }
    }
    if let Some(alg) = value.get("alg") {
        let alg = alg
            .as_str()
            .ok_or_else(|| AuthzError::BadRequest("JWKS `alg` must be a string".into()))?;
        if alg != expected_alg {
            return Ok(false);
        }
    }
    if let Some(operations) = value.get("key_ops") {
        let operations = operations
            .as_array()
            .ok_or_else(|| AuthzError::BadRequest("JWKS `key_ops` must be an array".into()))?;
        let mut seen = std::collections::BTreeSet::new();
        let mut verifies = false;
        for operation in operations {
            let operation = operation.as_str().ok_or_else(|| {
                AuthzError::BadRequest("JWKS `key_ops` entries must be strings".into())
            })?;
            if !seen.insert(operation) {
                return Err(AuthzError::BadRequest(
                    "JWKS `key_ops` must not contain duplicates".into(),
                ));
            }
            verifies |= operation == "verify";
        }
        if !verifies {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone)]
pub struct ReplayGuard {
    backend: ReplayBackend,
}

type ReplayKey = (String, String, String);
type MemoryReplayState = Arc<Mutex<BTreeMap<ReplayKey, i64>>>;

fn replay_digest(domain: &[u8], value: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update([0]);
    digest.update(value.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize())
}

#[derive(Clone)]
enum ReplayBackend {
    Memory(MemoryReplayState),
    Pg(PgReplayBacking),
}

#[derive(Clone)]
struct PgReplayBacking {
    backing: Arc<myelin_storage::DurableReplayBacking>,
    rt: tokio::runtime::Handle,
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayGuard {
    pub fn new() -> ReplayGuard {
        ReplayGuard {
            backend: ReplayBackend::Memory(Arc::new(Mutex::new(BTreeMap::new()))),
        }
    }

    pub fn with_pg(
        backing: myelin_storage::DurableReplayBacking,
        rt: tokio::runtime::Handle,
    ) -> ReplayGuard {
        ReplayGuard {
            backend: ReplayBackend::Pg(PgReplayBacking {
                backing: Arc::new(backing),
                rt,
            }),
        }
    }

    pub(crate) fn consume_scoped(
        &self,
        tenant: &str,
        namespace: &str,
        id: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<bool, AuthzError> {
        let namespace = replay_digest(b"myelin-auth-replay-namespace-v1", namespace);
        let id = replay_digest(b"myelin-auth-replay-id-v1", id);
        match &self.backend {
            ReplayBackend::Memory(seen) => {
                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                seen.retain(|_, expiry| *expiry >= now);
                let key = (tenant.to_string(), namespace, id);
                match seen.entry(key) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(expires_at);
                        Ok(true)
                    }
                    std::collections::btree_map::Entry::Occupied(_) => Ok(false),
                }
            }
            ReplayBackend::Pg(pg) => pg
                .block(pg.backing.consume(tenant, &namespace, &id, expires_at, now))
                .map_err(|e| refuse(format!("replay store unavailable: {e}"))),
        }
    }
}

impl PgReplayBacking {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub tenant_claim: String,
    pub region_claim: String,
    pub leeway_secs: i64,
    pub require_replay_defence: bool,
}

impl OidcConfig {
    pub fn new(issuer: impl Into<String>, audience: impl Into<String>) -> OidcConfig {
        OidcConfig {
            issuer: issuer.into(),
            audience: audience.into(),
            tenant_claim: "tenant".into(),
            region_claim: "region".into(),
            leeway_secs: 60,
            require_replay_defence: true,
        }
    }

    pub fn with_claims(
        mut self,
        tenant_claim: impl Into<String>,
        region_claim: impl Into<String>,
    ) -> OidcConfig {
        self.tenant_claim = tenant_claim.into();
        self.region_claim = region_claim.into();
        self
    }
}

type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct OidcVerifier {
    config: OidcConfig,
    jwks: JwksSource,
    replay: ReplayGuard,
    now: NowFn,
}

impl OidcVerifier {
    pub fn new(config: OidcConfig, jwks: JwkSet) -> OidcVerifier {
        OidcVerifier {
            config,
            jwks: JwksSource::fixed(jwks),
            replay: ReplayGuard::new(),
            now: Arc::new(system_now),
        }
    }

    pub fn with_replay_guard(mut self, replay: ReplayGuard) -> OidcVerifier {
        self.replay = replay;
        self
    }

    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> OidcVerifier {
        self.now = Arc::new(now);
        self
    }

    pub fn with_jwks_refresh(
        mut self,
        refresh: impl Fn() -> Result<JwkSet, AuthzError> + Send + Sync + 'static,
    ) -> OidcVerifier {
        let now = self.now();
        self.jwks = self.jwks.with_refresh(now, Arc::new(refresh));
        self
    }

    pub fn replay_guard(&self) -> &ReplayGuard {
        &self.replay
    }

    fn now(&self) -> i64 {
        (self.now)()
    }
}

fn num_claim(claims: &serde_json::Value, key: &str) -> Option<i64> {
    let v = claims.get(key)?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

fn aud_is_exact(claims: &serde_json::Value, want: &str) -> bool {
    match claims.get("aud") {
        Some(serde_json::Value::String(s)) => s == want,
        Some(serde_json::Value::Array(arr)) => {
            arr.len() == 1 && arr.first().and_then(serde_json::Value::as_str) == Some(want)
        }
        _ => false,
    }
}

fn verify_signature(key: &JwkKey, msg: &[u8], sig: &[u8]) -> Result<(), AuthzError> {
    key.validate_shape().map_err(refuse)?;
    match key {
        JwkKey::Rsa { n, e } => {
            use rsa::pkcs1v15::{Signature, VerifyingKey};
            use rsa::signature::Verifier;
            use rsa::{BigUint, RsaPublicKey};
            use sha2::Sha256;
            let pubkey = RsaPublicKey::new(BigUint::from_bytes_be(n), BigUint::from_bytes_be(e))
                .map_err(|e| refuse(format!("invalid RSA JWKS key: {e}")))?;
            let vk = VerifyingKey::<Sha256>::new(pubkey);
            let signature = Signature::try_from(sig)
                .map_err(|_| refuse("malformed RS256 signature encoding"))?;
            vk.verify(msg, &signature)
                .map_err(|_| refuse("RS256 signature verification failed"))
        }
        JwkKey::EcP256 { x, y } => {
            use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
            if x.len() != 32 || y.len() != 32 {
                return Err(refuse(
                    "invalid P-256 JWKS coordinates (expected 32 bytes each)",
                ));
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                .verify(msg, sig)
                .map_err(|_| refuse("ES256 signature verification failed"))
        }
        JwkKey::Ed25519 { x } => {
            use ring::signature::{UnparsedPublicKey, ED25519};
            UnparsedPublicKey::new(&ED25519, x.clone())
                .verify(msg, sig)
                .map_err(|_| refuse("EdDSA signature verification failed"))
        }
    }
}

impl CredentialVerifier for OidcVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        if credential.scheme != scheme::OIDC {
            return Err(AuthzError::BadRequest(format!(
                "OidcVerifier received a `{}` credential (expected `oidc`)",
                credential.scheme
            )));
        }

        let material = credential.material.trim();
        let (token, expected_nonce) = unwrap_login_material(material)?;
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthzError::BadRequest(
                "malformed JWT: expected three dot-separated segments (header.payload.signature)"
                    .into(),
            ));
        }
        let (header_b64, payload_b64, sig_b64) = (parts[0], parts[1], parts[2]);

        let header: serde_json::Value = serde_json::from_slice(&b64url(header_b64)?)
            .map_err(|e| AuthzError::BadRequest(format!("malformed JWT header JSON: {e}")))?;
        let alg = header
            .get("alg")
            .and_then(|a| a.as_str())
            .ok_or_else(|| AuthzError::BadRequest("JWT header missing `alg`".into()))?;

        if alg.eq_ignore_ascii_case("none") {
            return Err(refuse(
                "alg:none rejected - an unsigned OIDC token never authenticates (alg-confusion defence)",
            ));
        }
        if alg
            .as_bytes()
            .get(..2)
            .is_some_and(|p| p.eq_ignore_ascii_case(b"HS"))
        {
            return Err(refuse(
                "symmetric alg rejected against an asymmetric JWKS key (the RS256→HS256 \
                 alg-confusion bypass)",
            ));
        }

        let kid = header
            .get("kid")
            .and_then(|k| k.as_str())
            .ok_or_else(|| refuse("JWT header missing `kid` (cannot select a JWKS key)"))?;
        let now = self.now();
        let mut key = self.jwks.key_for(kid, now, false)?;

        let expected = key.expected_alg();
        if alg != expected {
            return Err(refuse(format!(
                "JWT alg does not match the selected JWKS key (expected `{expected}` - \
                 alg-confusion / wrong-alg defence)"
            )));
        }

        let signing_input = format!("{header_b64}.{payload_b64}");
        let sig = b64url(sig_b64)?;
        if let Err(initial_error) = verify_signature(&key, signing_input.as_bytes(), &sig) {
            if !self.jwks.refreshable() {
                return Err(initial_error);
            }
            key = self.jwks.key_for(kid, now, true)?;
            let refreshed_expected = key.expected_alg();
            if alg != refreshed_expected {
                return Err(refuse(format!(
                    "JWT alg does not match the refreshed key (expected `{refreshed_expected}` - \
                     alg-confusion / wrong-alg defence)"
                )));
            }
            verify_signature(&key, signing_input.as_bytes(), &sig)?;
        }

        let claims: serde_json::Value = serde_json::from_slice(&b64url(payload_b64)?)
            .map_err(|e| AuthzError::BadRequest(format!("malformed JWT claims JSON: {e}")))?;

        let iss = claims
            .get("iss")
            .and_then(|i| i.as_str())
            .ok_or_else(|| refuse("token missing `iss`"))?;
        if iss != self.config.issuer {
            return Err(refuse("issuer mismatch"));
        }

        if !aud_is_exact(&claims, &self.config.audience) {
            return Err(refuse(format!(
                "audience mismatch: token `aud` must name exactly this RP `{}`",
                self.config.audience
            )));
        }

        let now = self.now();
        let leeway = self.config.leeway_secs;
        let exp = num_claim(&claims, "exp").ok_or_else(|| refuse("token missing `exp`"))?;
        if exp.saturating_add(leeway) < now {
            return Err(refuse(format!(
                "token expired: exp={exp} (+{leeway}s leeway) < now={now}"
            )));
        }
        if let Some(nbf) = num_claim(&claims, "nbf") {
            if nbf.saturating_sub(leeway) > now {
                return Err(refuse(format!(
                    "token not yet valid: nbf={nbf} (-{leeway}s leeway) > now={now}"
                )));
            }
        }
        if let Some(iat) = num_claim(&claims, "iat") {
            if iat.saturating_sub(leeway) > now {
                return Err(refuse(format!(
                    "token `iat` in the future: iat={iat} (-{leeway}s leeway) > now={now}"
                )));
            }
        }

        let sub = claims
            .get("sub")
            .and_then(|s| s.as_str())
            .filter(|s| valid_opaque_claim(s, MAX_OIDC_SUBJECT_BYTES))
            .ok_or_else(|| refuse("token has no valid bounded `sub` (no subject)"))?;
        let tenant = claims
            .get(&self.config.tenant_claim)
            .and_then(|t| t.as_str())
            .filter(|t| valid_scope_claim(t))
            .ok_or_else(|| {
                refuse(format!(
                    "verified token carries no valid bounded `{}` claim (the tenant is the trust \
                     root and must come from the IdP-verified claims, never a path)",
                    self.config.tenant_claim
                ))
            })?;
        let region = claims
            .get(&self.config.region_claim)
            .and_then(|r| r.as_str())
            .filter(|r| valid_scope_claim(r))
            .ok_or_else(|| {
                refuse(format!(
                    "verified token carries no valid bounded `{}` claim",
                    self.config.region_claim
                ))
            })?;

        if let Some(expected_nonce) = expected_nonce.as_deref() {
            let signed_nonce = claims
                .get("nonce")
                .and_then(|nonce| nonce.as_str())
                .ok_or_else(|| refuse("ID token missing the browser transaction `nonce`"))?;
            if signed_nonce != expected_nonce {
                return Err(refuse(
                    "ID token nonce does not match the browser transaction nonce",
                ));
            }
        }

        let replay_id = claims
            .get("jti")
            .and_then(|j| j.as_str())
            .filter(|id| valid_opaque_claim(id, MAX_OIDC_REPLAY_ID_BYTES))
            .or_else(|| {
                claims
                    .get("nonce")
                    .and_then(|n| n.as_str())
                    .filter(|id| valid_opaque_claim(id, MAX_OIDC_REPLAY_ID_BYTES))
            });
        match replay_id {
            Some(id) => {
                let namespace =
                    serde_json::json!(["oidc", self.config.issuer, self.config.audience, region])
                        .to_string();
                if !self.replay.consume_scoped(
                    tenant,
                    &namespace,
                    id,
                    exp.saturating_add(leeway),
                    now,
                )? {
                    return Err(refuse(
                        "replayed token: its `jti`/`nonce` was already presented (replay defence)",
                    ));
                }
            }
            None => {
                if self.config.require_replay_defence {
                    return Err(refuse(
                        "token carries neither a non-empty `jti` nor `nonce` - no replay-defence material",
                    ));
                }
            }
        }

        Ok(VerifiedAssertion {
            tenant: TenantId(tenant.to_string()),
            region: Region(region.to_string()),
            scheme: scheme::OIDC.to_string(),
            subject_key: sub.to_string(),
            expires_at_unix: Some(exp),
        })
    }
}

pub struct SchemeDispatchVerifier {
    by_scheme: BTreeMap<String, Arc<dyn CredentialVerifier>>,
    fallback: Arc<dyn CredentialVerifier>,
}

impl SchemeDispatchVerifier {
    pub fn new(fallback: Arc<dyn CredentialVerifier>) -> SchemeDispatchVerifier {
        SchemeDispatchVerifier {
            by_scheme: BTreeMap::new(),
            fallback,
        }
    }

    pub fn route(
        mut self,
        scheme: impl Into<String>,
        verifier: Arc<dyn CredentialVerifier>,
    ) -> SchemeDispatchVerifier {
        self.by_scheme.insert(scheme.into(), verifier);
        self
    }
}

impl CredentialVerifier for SchemeDispatchVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        match self.by_scheme.get(&credential.scheme) {
            Some(v) => v.verify(credential),
            None => self.fallback.verify(credential),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticate::StructuralVerifier;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn signing_input(header: &serde_json::Value, claims: &serde_json::Value) -> String {
        format!(
            "{}.{}",
            b64(serde_json::to_string(header).unwrap().as_bytes()),
            b64(serde_json::to_string(claims).unwrap().as_bytes())
        )
    }

    fn jwt(header: &serde_json::Value, claims: &serde_json::Value, sig: &[u8]) -> String {
        format!("{}.{}", signing_input(header, claims), b64(sig))
    }

    const NOW: i64 = 1_700_000_000;

    fn claims(jti: &str) -> serde_json::Value {
        serde_json::json!({
            "iss": "https://idp.example.com",
            "aud": "myelin-rp",
            "sub": "oidc-sub-1",
            "exp": NOW + 300,
            "nbf": NOW - 10,
            "iat": NOW - 10,
            "jti": jti,
            "tenant": "acme",
            "region": "eu-west",
        })
    }

    fn config() -> OidcConfig {
        OidcConfig::new("https://idp.example.com", "myelin-rp")
    }

    fn verifier(jwks: JwkSet) -> OidcVerifier {
        OidcVerifier::new(config(), jwks).with_clock(|| NOW)
    }

    fn cred(token: String) -> Credential {
        Credential {
            scheme: scheme::OIDC.into(),
            material: token,
        }
    }

    struct RsaKey {
        priv_key: rsa::RsaPrivateKey,
    }
    impl RsaKey {
        fn generate() -> RsaKey {
            use rand::rngs::OsRng;
            let priv_key = rsa::RsaPrivateKey::new(&mut OsRng, 2048).expect("rsa keygen");
            RsaKey { priv_key }
        }
        fn jwk(&self) -> JwkKey {
            use rsa::traits::PublicKeyParts;
            let pubk = self.priv_key.to_public_key();
            JwkKey::Rsa {
                n: pubk.n().to_bytes_be(),
                e: pubk.e().to_bytes_be(),
            }
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            use rsa::pkcs1v15::SigningKey;
            use rsa::signature::{SignatureEncoding, Signer};
            use sha2::Sha256;
            let sk = SigningKey::<Sha256>::new(self.priv_key.clone());
            sk.sign(msg).to_vec()
        }
    }

    struct EcKey {
        pair: ring::signature::EcdsaKeyPair,
        rng: ring::rand::SystemRandom,
    }
    impl EcKey {
        fn generate() -> EcKey {
            use ring::rand::SystemRandom;
            use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .expect("ec keygen");
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .expect("ec from pkcs8");
            EcKey { pair, rng }
        }
        fn jwk(&self) -> JwkKey {
            use ring::signature::KeyPair;
            let pt = self.pair.public_key().as_ref();
            assert_eq!(pt.len(), 65, "uncompressed P-256 point");
            JwkKey::EcP256 {
                x: pt[1..33].to_vec(),
                y: pt[33..65].to_vec(),
            }
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            self.pair
                .sign(&self.rng, msg)
                .expect("ec sign")
                .as_ref()
                .to_vec()
        }
    }

    struct EdKey {
        pair: ring::signature::Ed25519KeyPair,
    }
    impl EdKey {
        fn generate() -> EdKey {
            use ring::rand::SystemRandom;
            use ring::signature::Ed25519KeyPair;
            let rng = SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("ed keygen");
            let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("ed from pkcs8");
            EdKey { pair }
        }
        fn jwk(&self) -> JwkKey {
            use ring::signature::KeyPair;
            JwkKey::Ed25519 {
                x: self.pair.public_key().as_ref().to_vec(),
            }
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            self.pair.sign(msg).as_ref().to_vec()
        }
    }

    #[test]
    fn positive_rs256_verifies_and_yields_trust_rooted_assertion() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1", "typ": "JWT"});
        let cl = claims("jti-rs256");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let token = jwt(&header, &cl, &sig);

        let a = verifier(jwks)
            .verify(&cred(token))
            .expect("RS256 must verify");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.region, Region("eu-west".into()));
        assert_eq!(a.scheme, scheme::OIDC);
        assert_eq!(a.subject_key, "oidc-sub-1");
    }

    #[test]
    fn browser_login_requires_the_signed_transaction_nonce_to_match() {
        const NONCE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1", "typ": "JWT"});
        let mut cl = claims("jti-browser-nonce");
        cl["nonce"] = NONCE.into();
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let token = jwt(&header, &cl, &sig);

        let bound = oidc_login_material(&token, NONCE).expect("valid transaction nonce");
        verifier(jwks.clone())
            .verify(&cred(bound))
            .expect("matching signed nonce must verify");

        let wrong = oidc_login_material(&token, "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")
            .expect("well-shaped alternate nonce");
        let error = verifier(jwks).verify(&cred(wrong)).unwrap_err();
        assert!(format!("{error:?}").contains("nonce does not match"));
    }

    #[test]
    fn browser_login_rejects_missing_or_malformed_transaction_nonce() {
        const NONCE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1", "typ": "JWT"});
        let cl = claims("jti-browser-missing-nonce");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let token = jwt(&header, &cl, &sig);
        let bound = oidc_login_material(&token, NONCE).expect("valid transaction nonce");

        let error = verifier(jwks).verify(&cred(bound)).unwrap_err();
        assert!(format!("{error:?}").contains("missing the browser transaction `nonce`"));
        assert!(oidc_login_material(&token, "short").is_err());
    }

    #[test]
    fn positive_es256_verifies_and_yields_trust_rooted_assertion() {
        let key = EcKey::generate();
        let jwks = JwkSet::new().with_key("ec-1", key.jwk());
        let header = serde_json::json!({"alg": "ES256", "kid": "ec-1", "typ": "JWT"});
        let cl = claims("jti-es256");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let token = jwt(&header, &cl, &sig);

        let a = verifier(jwks)
            .verify(&cred(token))
            .expect("ES256 must verify");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.subject_key, "oidc-sub-1");
    }

    #[test]
    fn positive_eddsa_verifies_and_yields_trust_rooted_assertion() {
        let key = EdKey::generate();
        let jwks = JwkSet::new().with_key("ed-1", key.jwk());
        let header = serde_json::json!({"alg": "EdDSA", "kid": "ed-1", "typ": "JWT"});
        let cl = claims("jti-eddsa");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let token = jwt(&header, &cl, &sig);

        let a = verifier(jwks)
            .verify(&cred(token))
            .expect("EdDSA must verify");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.subject_key, "oidc-sub-1");
    }

    #[test]
    fn positive_rs256_via_parsed_jwks_json() {
        let key = RsaKey::generate();
        let (n, e) = match key.jwk() {
            JwkKey::Rsa { n, e } => (n, e),
            _ => unreachable!(),
        };
        let doc = serde_json::json!({
            "keys": [{"kty": "RSA", "kid": "rsa-json", "use": "sig", "alg": "RS256",
                      "n": b64(&n), "e": b64(&e)}]
        })
        .to_string();
        let jwks = JwkSet::from_jwks_json(&doc).expect("parse JWKS");
        assert_eq!(jwks.len(), 1);
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-json"});
        let cl = claims("jti-rs-json");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let a = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap();
        assert_eq!(a.tenant, TenantId("acme".into()));
    }

    #[test]
    fn parsed_jwks_honours_key_intent_and_rejects_ambiguity() {
        let key = RsaKey::generate();
        let (n, e) = match key.jwk() {
            JwkKey::Rsa { n, e } => (b64(&n), b64(&e)),
            _ => unreachable!(),
        };
        let filtered = serde_json::json!({"keys": [
            {"kty":"RSA", "kid":"enc", "use":"enc", "n":n, "e":e},
            {"kty":"RSA", "kid":"ops", "key_ops":["encrypt"], "n":n, "e":e},
            {"kty":"RSA", "kid":"alg", "alg":"PS256", "n":n, "e":e},
            {"kty":"RSA", "kid":"verify", "use":"sig", "key_ops":["verify"],
             "alg":"RS256", "n":n, "e":e}
        ]});
        let parsed = JwkSet::from_jwks_json(&filtered.to_string()).expect("valid intent metadata");
        assert_eq!(parsed.len(), 1);
        assert!(parsed.get("verify").is_some());

        let duplicate = serde_json::json!({"keys": [
            {"kty":"RSA", "kid":"same", "n":n, "e":e},
            {"kty":"RSA", "kid":"same", "n":n, "e":e}
        ]});
        let error = JwkSet::from_jwks_json(&duplicate.to_string()).unwrap_err();
        assert!(matches!(error, AuthzError::BadRequest(message) if message.contains("duplicate")));
    }

    #[test]
    fn parsed_and_injected_jwks_reject_weak_rsa_keys() {
        let weak = JwkKey::Rsa {
            n: vec![0xff; 128],
            e: vec![0x01, 0x00, 0x01],
        };
        let document = serde_json::json!({"keys": [{
            "kty":"RSA", "kid":"weak", "alg":"RS256",
            "n":b64(&[0xff; 128]), "e":"AQAB"
        }]});
        let error = JwkSet::from_jwks_json(&document.to_string()).unwrap_err();
        assert!(matches!(error, AuthzError::BadRequest(message) if message.contains("2048")));

        let header = serde_json::json!({"alg": "RS256", "kid": "weak"});
        let cl = claims("jti-weak-rsa");
        let error = verifier(JwkSet::new().with_key("weak", weak))
            .verify(&cred(jwt(&header, &cl, &[0; 256])))
            .unwrap_err();
        assert!(matches!(error, AuthzError::FailClosed(message) if message.contains("2048")));
    }

    #[test]
    fn malformed_key_operations_fail_loud() {
        let key = RsaKey::generate();
        let (n, e) = match key.jwk() {
            JwkKey::Rsa { n, e } => (b64(&n), b64(&e)),
            _ => unreachable!(),
        };
        for key_ops in [
            serde_json::json!("verify"),
            serde_json::json!(["verify", "verify"]),
            serde_json::json!(["verify", 1]),
        ] {
            let document = serde_json::json!({"keys": [{
                "kty":"RSA", "kid":"rsa", "n":n, "e":e, "key_ops":key_ops
            }]});
            assert!(matches!(
                JwkSet::from_jwks_json(&document.to_string()),
                Err(AuthzError::BadRequest(_))
            ));
        }
    }

    #[test]
    fn negative_alg_none_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "none", "kid": "rsa-1"});
        let cl = claims("jti-none");
        let token = jwt(&header, &cl, b"");
        let err = verifier(jwks).verify(&cred(token)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("alg:none")),
            "alg:none must be refused loudly, got {err:?}"
        );
    }

    #[test]
    fn negative_alg_confusion_rs256_key_as_hs256_is_rejected() {
        let key = RsaKey::generate();
        let pub_jwk = key.jwk();
        let hmac_secret = match &pub_jwk {
            JwkKey::Rsa { n, e } => {
                let mut s = n.clone();
                s.extend_from_slice(e);
                s
            }
            _ => unreachable!(),
        };
        let jwks = JwkSet::new().with_key("rsa-1", pub_jwk);
        let header = serde_json::json!({"alg": "HS256", "kid": "rsa-1"});
        let cl = claims("jti-confusion");
        let si = signing_input(&header, &cl);
        let mac = hmac_sha256(&hmac_secret, si.as_bytes());
        let token = format!("{si}.{}", b64(&mac));
        let err = verifier(jwks).verify(&cred(token)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("alg-confusion")),
            "the RS256-key-as-HS256 confusion must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_wrong_signing_key_is_rejected() {
        let real = RsaKey::generate();
        let attacker = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", real.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = claims("jti-wrongkey");
        let sig = attacker.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("RS256 signature verification failed")),
            "a signature by an unknown key must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_unknown_kid_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-OTHER"});
        let cl = claims("jti-unknownkid");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unknown `kid`")),
            "an unknown kid must be refused, got {err:?}"
        );
    }

    #[test]
    fn unknown_kid_refreshes_once_and_accepts_a_rotated_key() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let old = RsaKey::generate();
        let rotated = RsaKey::generate();
        let refreshed = JwkSet::new().with_key("rsa-2", rotated.jwk());
        let refreshes = Arc::new(AtomicUsize::new(0));
        let refresh_count = refreshes.clone();
        let verifier = OidcVerifier::new(config(), JwkSet::new().with_key("rsa-1", old.jwk()))
            .with_clock(|| NOW)
            .with_jwks_refresh(move || {
                refresh_count.fetch_add(1, Ordering::SeqCst);
                Ok(refreshed.clone())
            });
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-2"});
        let cl = claims("jti-rotated-kid");
        let sig = rotated.sign(signing_input(&header, &cl).as_bytes());

        verifier
            .verify(&cred(jwt(&header, &cl, &sig)))
            .expect("an unknown rotated kid must trigger refresh and verify");
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn signature_failure_refreshes_a_reused_kid() {
        let old = RsaKey::generate();
        let rotated = RsaKey::generate();
        let refreshed = JwkSet::new().with_key("rsa-1", rotated.jwk());
        let verifier = OidcVerifier::new(config(), JwkSet::new().with_key("rsa-1", old.jwk()))
            .with_clock(|| NOW)
            .with_jwks_refresh(move || Ok(refreshed.clone()));
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = claims("jti-reused-kid");
        let sig = rotated.sign(signing_input(&header, &cl).as_bytes());

        verifier
            .verify(&cred(jwt(&header, &cl, &sig)))
            .expect("a signing-key change under the same kid must refresh and verify");
    }

    #[test]
    fn reused_kid_cannot_carry_an_old_algorithm_pin_across_refresh() {
        let old = RsaKey::generate();
        let attacker = RsaKey::generate();
        let refreshed = JwkSet::new().with_key(
            "shared-kid",
            JwkKey::EcP256 {
                x: vec![0; 32],
                y: vec![0; 32],
            },
        );
        let verifier = OidcVerifier::new(config(), JwkSet::new().with_key("shared-kid", old.jwk()))
            .with_clock(|| NOW)
            .with_jwks_refresh(move || Ok(refreshed.clone()));
        let header = serde_json::json!({"alg": "RS256", "kid": "shared-kid"});
        let cl = claims("jti-reused-kid-family-change");
        let sig = attacker.sign(signing_input(&header, &cl).as_bytes());

        let error = verifier
            .verify(&cred(jwt(&header, &cl, &sig)))
            .expect_err("the old RSA alg pin must not survive an RSA-to-EC key refresh");
        assert!(
            matches!(&error, AuthzError::FailClosed(message) if message.contains("refreshed key") && message.contains("expected `ES256`")),
            "unexpected refusal: {error:?}"
        );
    }

    #[test]
    fn refresh_failure_keeps_a_still_valid_cached_key() {
        use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

        let key = RsaKey::generate();
        let clock = Arc::new(AtomicI64::new(NOW));
        let verifier_clock = clock.clone();
        let refreshes = Arc::new(AtomicUsize::new(0));
        let refresh_count = refreshes.clone();
        let verifier = OidcVerifier::new(config(), JwkSet::new().with_key("rsa-1", key.jwk()))
            .with_clock(move || verifier_clock.load(Ordering::SeqCst))
            .with_jwks_refresh(move || {
                refresh_count.fetch_add(1, Ordering::SeqCst);
                Err(refuse("IdP unavailable"))
            });
        clock.store(NOW + JWKS_REFRESH_INTERVAL_SECS, Ordering::SeqCst);
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-cached-during-outage");
        cl["exp"] = (NOW + 2_000).into();
        let sig = key.sign(signing_input(&header, &cl).as_bytes());

        verifier
            .verify(&cred(jwt(&header, &cl, &sig)))
            .expect("a refresh outage must not discard a still-valid cached signing key");
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn negative_expired_token_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-expired");
        cl["exp"] = serde_json::json!(NOW - 1000);
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired token must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_replayed_jti_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let v = verifier(jwks);
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = claims("jti-replay-unique");
        let token = jwt(
            &header,
            &cl,
            &key.sign(signing_input(&header, &cl).as_bytes()),
        );
        v.verify(&cred(token.clone())).expect("first use verifies");
        let err = v.verify(&cred(token)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "a replayed jti must be refused, got {err:?}"
        );
    }

    #[test]
    fn in_memory_replay_entries_expire_and_remain_scoped() {
        let replay = ReplayGuard::new();
        assert!(replay
            .consume_scoped("acme", "issuer-a", "same-id", 200, 100)
            .unwrap());
        assert!(!replay
            .consume_scoped("acme", "issuer-a", "same-id", 300, 150)
            .unwrap());
        assert!(replay
            .consume_scoped("acme", "issuer-b", "same-id", 300, 150)
            .unwrap());
        assert!(replay
            .consume_scoped("globex", "issuer-a", "same-id", 300, 150)
            .unwrap());
        assert!(replay
            .consume_scoped("acme", "issuer-a", "same-id", 400, 201)
            .unwrap());
    }

    #[test]
    fn negative_wrong_audience_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-wrongaud");
        cl["aud"] = serde_json::json!("some-other-rp");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("audience")),
            "a wrong-aud token must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_additional_audience_is_rejected_without_an_explicit_azp_policy() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-extra-aud");
        cl["aud"] = serde_json::json!(["myelin-rp", "attacker-client"]);
        cl["azp"] = serde_json::json!("myelin-rp");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let error = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&error, AuthzError::FailClosed(message) if message.contains("audience")),
            "an unconfigured additional audience must be refused even when `azp` names Myelin"
        );
    }

    #[test]
    fn positive_single_element_audience_array_is_accepted() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-array-aud");
        cl["aud"] = serde_json::json!(["myelin-rp"]);
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .expect("a one-element audience array names exactly this RP");
    }

    #[test]
    fn negative_wrong_issuer_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-wrongiss");
        cl["iss"] = serde_json::json!("https://evil-idp.example.com");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("issuer")),
            "a wrong-iss token must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_tampered_payload_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = claims("jti-tamper");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let mut forged = cl.clone();
        forged["tenant"] = serde_json::json!("globex");
        let header_b64 = b64(serde_json::to_string(&header).unwrap().as_bytes());
        let forged_payload_b64 = b64(serde_json::to_string(&forged).unwrap().as_bytes());
        let token = format!("{header_b64}.{forged_payload_b64}.{}", b64(&sig));
        let err = verifier(jwks).verify(&cred(token)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a tampered payload must be refused (sig/payload mismatch), got {err:?}"
        );
    }

    #[test]
    fn negative_malformed_garbage_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let v = verifier(jwks);
        for bad in [
            "",
            "not-a-jwt",
            "only.two",
            "a.b.c.d",
            "!!!.@@@.###",
            "header.payload",
        ] {
            let err = v.verify(&cred(bad.to_string())).unwrap_err();
            assert!(
                matches!(err, AuthzError::BadRequest(_) | AuthzError::FailClosed(_)),
                "garbage `{bad}` must be refused"
            );
        }
    }

    #[test]
    fn negative_odd_alg_header_bytes_are_refused_not_panicking() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let v = verifier(jwks);
        for bad_alg in ["Héx", "H", "", "hs256", "H€", "🔥256", "Hé"] {
            let header = serde_json::json!({"alg": bad_alg, "kid": "rsa-1"});
            let cl = claims("jti-odd-alg");
            let sig = key.sign(signing_input(&header, &cl).as_bytes());
            let r = v.verify(&cred(jwt(&header, &cl, &sig)));
            assert!(
                r.is_err(),
                "odd alg `{bad_alg:?}` must be refused (and must not panic)"
            );
        }
    }

    #[test]
    fn extreme_numeric_claims_do_not_panic() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let v = verifier(jwks);
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cases = [
            ("exp", i64::MAX),
            ("exp", i64::MIN),
            ("nbf", i64::MAX),
            ("nbf", i64::MIN),
            ("iat", i64::MAX),
            ("iat", i64::MIN),
        ];
        for (i, (field, val)) in cases.iter().enumerate() {
            let mut cl = claims(&format!("jti-extreme-{i}"));
            cl[*field] = serde_json::json!(val);
            let sig = key.sign(signing_input(&header, &cl).as_bytes());
            let _ = v.verify(&cred(jwt(&header, &cl, &sig)));
        }
    }

    #[test]
    fn tenant_comes_only_from_verified_claims() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-notenant");
        cl.as_object_mut().unwrap().remove("tenant");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks.clone())
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("tenant")),
            "a token with no tenant claim must be refused, got {err:?}"
        );
    }

    #[test]
    fn trust_rooted_claims_are_bounded_before_replay_or_storage() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        for (field, value, expected) in [
            ("tenant", "acme/foreign".to_string(), "tenant"),
            ("region", "eu-west\nforged".to_string(), "region"),
            ("sub", "s".repeat(MAX_OIDC_SUBJECT_BYTES + 1), "sub"),
            (
                "jti",
                "j".repeat(MAX_OIDC_REPLAY_ID_BYTES + 1),
                "replay-defence",
            ),
        ] {
            let mut cl = claims("jti-bounded-default");
            cl[field] = value.into();
            let signature = key.sign(signing_input(&header, &cl).as_bytes());
            let error = verifier(jwks.clone())
                .verify(&cred(jwt(&header, &cl, &signature)))
                .unwrap_err();
            assert!(
                matches!(&error, AuthzError::FailClosed(message) if message.contains(expected)),
                "unexpected refusal for {field}: {error:?}"
            );
        }
    }

    #[test]
    fn incomplete_signed_token_does_not_poison_replay_identifier() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let verifier = verifier(jwks);
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});

        let mut incomplete = claims("jti-reused-after-invalid");
        incomplete.as_object_mut().unwrap().remove("tenant");
        let signature = key.sign(signing_input(&header, &incomplete).as_bytes());
        verifier
            .verify(&cred(jwt(&header, &incomplete, &signature)))
            .expect_err("the incomplete signed token must fail");

        let complete = claims("jti-reused-after-invalid");
        let signature = key.sign(signing_input(&header, &complete).as_bytes());
        verifier
            .verify(&cred(jwt(&header, &complete, &signature)))
            .expect("claim validation must happen before replay consumption");
    }

    #[test]
    fn dispatch_routes_oidc_to_real_verifier_and_others_to_fallback() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let oidc = Arc::new(verifier(jwks));
        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::OIDC, oidc);

        let header = serde_json::json!({"alg": "none", "kid": "rsa-1"});
        let cl = claims("jti-dispatch-none");
        let forged = jwt(&header, &cl, b"");
        assert!(
            dispatch.verify(&cred(forged)).is_err(),
            "an OIDC alg:none token must hit the real verifier and be refused"
        );

        let saml = Credential {
            scheme: scheme::SAML.into(),
            material: "acme|eu-west|nameid-1".into(),
        };
        let a = dispatch
            .verify(&saml)
            .expect("SAML routes to the floor fallback");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.scheme, scheme::SAML);
    }

    fn real_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn wired_auth() -> (crate::authenticate::HumanSsoAuthenticator, RsaKey) {
        use crate::authenticate::HumanSsoAuthenticator;
        use crate::principal_store::PrincipalStore;
        use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
        use myelin_storage::{KmsEngine, TenantScope};

        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let admin = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let sc = TenantScope::from_verified_token(&admin, Region("eu-west".into()));
        store
            .put_principal(
                &sc,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        store
            .link_credential(
                &sc,
                scheme::OIDC,
                "oidc-sub-1",
                &PrincipalId("p:alice".into()),
            )
            .unwrap();

        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let cfg = OidcConfig::new("https://idp.example.com", "myelin-rp");
        let auth =
            HumanSsoAuthenticator::production_with_oidc(store, (cfg, jwks), ReplayGuard::new());
        (auth, key)
    }

    fn live_claims(jti: &str) -> serde_json::Value {
        let now = real_now();
        serde_json::json!({
            "iss": "https://idp.example.com",
            "aud": "myelin-rp",
            "sub": "oidc-sub-1",
            "exp": now + 3600,
            "nbf": now - 60,
            "iat": now - 60,
            "jti": jti,
            "tenant": "acme",
            "region": "eu-west",
        })
    }

    #[test]
    fn wired_production_authenticates_valid_oidc_token_to_principal() {
        use myelin_identity::{PrincipalId, PrincipalKind};
        let (auth, key) = wired_auth();
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = live_claims("wired-valid-1");
        let token = jwt(
            &header,
            &cl,
            &key.sign(signing_input(&header, &cl).as_bytes()),
        );

        let p = auth
            .authenticate(&cred(token), Some(&TenantId("globex".into())))
            .expect(
                "a valid OIDC token must authenticate through the wired production authenticator",
            );
        assert_eq!(p.principal_id, PrincipalId("p:alice".into()));
        assert_eq!(
            p.tenant,
            TenantId("acme".into()),
            "tenant is the VERIFIED claim (acme), never the path (globex)"
        );
        assert_eq!(p.region, Region("eu-west".into()));
        assert_eq!(p.kind, PrincipalKind::Human);
    }

    #[test]
    fn wired_production_rejects_forged_oidc_tokens() {
        use myelin_identity::AuthzError;
        let (auth, key) = wired_auth();
        let sign = |header: &serde_json::Value, cl: &serde_json::Value| {
            jwt(header, cl, &key.sign(signing_input(header, cl).as_bytes()))
        };

        let none = jwt(
            &serde_json::json!({"alg": "none", "kid": "rsa-1"}),
            &live_claims("wired-none"),
            b"",
        );
        let mut cl_iss = live_claims("wired-iss");
        cl_iss["iss"] = serde_json::json!("https://evil-idp.example.com");
        let wrong_iss = sign(
            &serde_json::json!({"alg": "RS256", "kid": "rsa-1"}),
            &cl_iss,
        );
        let mut cl_aud = live_claims("wired-aud");
        cl_aud["aud"] = serde_json::json!("some-other-rp");
        let wrong_aud = sign(
            &serde_json::json!({"alg": "RS256", "kid": "rsa-1"}),
            &cl_aud,
        );
        let mut cl_exp = live_claims("wired-exp");
        cl_exp["exp"] = serde_json::json!(real_now() - 10_000);
        let expired = sign(
            &serde_json::json!({"alg": "RS256", "kid": "rsa-1"}),
            &cl_exp,
        );

        for (label, token) in [
            ("alg:none", none),
            ("wrong-iss", wrong_iss),
            ("wrong-aud", wrong_aud),
            ("expired", expired),
        ] {
            let r = auth.authenticate(&cred(token), None);
            assert!(
                matches!(r, Err(AuthzError::FailClosed(_))),
                "forged OIDC token ({label}) must fail closed through the wired authenticator, got {r:?}"
            );
        }
    }

    #[test]
    fn wired_production_without_oidc_refuses_oidc_scheme() {
        use crate::authenticate::HumanSsoAuthenticator;
        use crate::principal_store::PrincipalStore;
        use myelin_identity::AuthzError;
        use myelin_storage::KmsEngine;

        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let auth = HumanSsoAuthenticator::production(store);
        let key = RsaKey::generate();
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = live_claims("no-oidc-configured");
        let token = jwt(
            &header,
            &cl,
            &key.sign(signing_input(&header, &cl).as_bytes()),
        );
        let r = auth.authenticate(&cred(token), None);
        assert!(
            matches!(r, Err(AuthzError::NotYetImplemented(_))),
            "with no OIDC configured, an OIDC token must be refused (refuse-not-mock), got {r:?}"
        );
    }

    fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        const BLOCK: usize = 64;
        let mut k = key.to_vec();
        if k.len() > BLOCK {
            let mut h = Sha256::new();
            h.update(&k);
            k = h.finalize().to_vec();
        }
        k.resize(BLOCK, 0);
        let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
        let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
        let mut inner = Sha256::new();
        inner.update(&ipad);
        inner.update(msg);
        let inner = inner.finalize();
        let mut outer = Sha256::new();
        outer.update(&opad);
        outer.update(inner);
        outer.finalize().to_vec()
    }
}
