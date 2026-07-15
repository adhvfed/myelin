//! # `oidc` — REAL OIDC ID-token (JWT) credential verification (MR-010a; census SI-001/004, the
//! OIDC slice of P-526).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §4 (the authentication
//! surfaces — OIDC; **tenant is taken from the verified credential, never the URL path**, ID-3).
//!
//! ## What this module replaces (the #1 CRITICAL census finding)
//! The production auth graph's floor verifier ([`crate::authenticate::StructuralVerifier`]) parses a
//! PLAINTEXT `<tenant>|<region>|<subject_key>` envelope — so ANYONE can forge any principal in any
//! tenant (SI-001/004). [`OidcVerifier`] is the REAL cryptographic replacement for the **OIDC**
//! scheme: it verifies an OIDC ID token (a JWT) against the IdP's JWKS public key with VETTED
//! primitives and extracts a trust-rooted [`VerifiedAssertion`] from the VERIFIED claims, or refuses
//! it LOUDLY. It plugs into the EXISTING [`CredentialVerifier`] seam — the resolution + telemetry
//! body in [`crate::authenticate`] does not change.
//!
//! ## The algorithms (vetted crates only — no hand-rolled signature math)
//! - **RS256** (RSA PKCS#1 v1.5 / SHA-256) — verified with the `rsa` + `sha2` RustCrypto crates.
//! - **ES256** (ECDSA P-256 / SHA-256, fixed `r‖s`) — verified with `ring`.
//! - **EdDSA** (Ed25519) — verified with `ring`.
//!
//! ## The alg-confusion defence (CRITICAL — the classic JWT bypass)
//! The expected algorithm is PINNED FROM THE JWKS KEY (selected by the token's `kid`), **never from
//! the token header**. We REJECT: `alg: none`; a symmetric alg (`HS*`) presented against an
//! asymmetric key (the RS256→HS256 attack, where the attacker signs with the RSA *public* key as an
//! HMAC secret); an unknown/missing `kid`; and any `alg` the selected key does not support.
//!
//! ## Claim validation (all from the VERIFIED payload, after the signature checks out)
//! `iss` (must equal the configured issuer), `aud` (must contain this RP), `exp` (expired → reject,
//! small clock-skew leeway), `nbf`/`iat` sanity, and `nonce`/`jti` replay defence (a replayed token
//! is rejected). The `tenant`/`region`/`subject_key` are read ONLY from the verified claims — never
//! caller-supplied (ID-3, the IDOR floor).
//!
//! ## What is INJECTED, and what is honestly out of scope
//! The JWKS is **injected** ([`JwkSet`]) — the crypto path makes NO network call, so unit/integration
//! tests provide a static JWKS and there is no network in the test. A runtime `jwks_uri` fetch +
//! OIDC discovery + key rotation is a thin layer to be added later (it would refresh the injected
//! [`JwkSet`]); it is OUT OF SCOPE here and is NOT claimed.
//!
//! ## Wiring (the dispatch seam — [`SchemeDispatchVerifier`])
//! [`OidcVerifier`] is wired as the OIDC-scheme verifier via [`SchemeDispatchVerifier`], which routes
//! a credential to a per-scheme verifier and falls back to an INJECTED default for the not-yet-real
//! schemes (SAML/SCIM/passkey/SSH → MR-010b/c/d). The dispatcher constructs NO `Structural*` type
//! itself (the fallback is injected by the caller), so it adds no new mock-crypto construction to the
//! production graph; removing the `StructuralVerifier` prod default entirely is MR-012.

use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use base64::Engine as _;

/// Decode one base64url (no-padding) JWS/JWT segment (RFC 7515 §2). A malformed segment is a loud
/// structural error (never coerced).
fn b64url(segment: &str) -> Result<Vec<u8>, AuthzError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment.as_bytes())
        .map_err(|e| AuthzError::BadRequest(format!("malformed base64url JWT segment: {e}")))
}

/// A LOUD refusal of a credential that is well-formed but does NOT verify (bad signature, alg
/// confusion, `alg:none`, expired, wrong `iss`/`aud`, replay, missing trust-rooted claim). It is an
/// `AuthzError` so an unverifiable token NEVER resolves to a Principal (fail-closed — the assertion
/// is never fabricated/partial).
fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

// ================================================================================================
// JWKS — the injected public-key set (RFC 7517). No network in the crypto path.
// ================================================================================================

/// The public-key material for one JWKS key — already base64url-decoded and family-pinned. The
/// family fixes the ONE JWS `alg` the key supports (the alg-confusion pin).
#[derive(Clone, Debug)]
pub enum JwkKey {
    /// RSA (RS256) — modulus `n` and public exponent `e`, big-endian bytes.
    Rsa {
        /// RSA modulus `n` (big-endian).
        n: Vec<u8>,
        /// RSA public exponent `e` (big-endian).
        e: Vec<u8>,
    },
    /// EC P-256 (ES256) — affine coordinates `x`, `y` (32 bytes each).
    EcP256 {
        /// The `x` coordinate (32 bytes).
        x: Vec<u8>,
        /// The `y` coordinate (32 bytes).
        y: Vec<u8>,
    },
    /// Ed25519 (EdDSA) — the 32-byte public key.
    Ed25519 {
        /// The Ed25519 public key (32 bytes).
        x: Vec<u8>,
    },
}

impl JwkKey {
    /// The ONE JWS `alg` this key type supports — the alg-confusion pin (expected from the KEY, never
    /// taken from the token header).
    pub fn expected_alg(&self) -> &'static str {
        match self {
            JwkKey::Rsa { .. } => "RS256",
            JwkKey::EcP256 { .. } => "ES256",
            JwkKey::Ed25519 { .. } => "EdDSA",
        }
    }
}

/// **The injected JWKS — the IdP's published public keys, keyed by `kid`.** Provided to the verifier
/// at construction (a static key set in tests); the crypto path never fetches it over the network.
#[derive(Clone, Debug, Default)]
pub struct JwkSet {
    by_kid: BTreeMap<String, JwkKey>,
}

impl JwkSet {
    /// An empty key set.
    pub fn new() -> JwkSet {
        JwkSet {
            by_kid: BTreeMap::new(),
        }
    }

    /// Add a key under its `kid` (builder form).
    pub fn with_key(mut self, kid: impl Into<String>, key: JwkKey) -> JwkSet {
        self.by_kid.insert(kid.into(), key);
        self
    }

    /// The key published under `kid`, if any (the verifier selects by the token's `kid` — never by a
    /// header-supplied algorithm).
    pub fn get(&self, kid: &str) -> Option<&JwkKey> {
        self.by_kid.get(kid)
    }

    /// The number of keys held.
    pub fn len(&self) -> usize {
        self.by_kid.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.by_kid.is_empty()
    }

    /// Parse a standard RFC 7517 JWKS JSON document (the shape an IdP's `jwks_uri` returns) into an
    /// injected key set. Supported families: RSA (RS256), EC P-256 (ES256), OKP/Ed25519 (EdDSA). A
    /// key with no `kid`, or an unsupported `kty`/`crv`, is SKIPPED (a JWKS may legitimately carry
    /// encryption keys / other curves we do not verify). A supported family with malformed
    /// parameters is a loud `BadRequest`.
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
                _ => continue, // a key with no kid cannot be selected by `kid` — skip.
            };
            let kty = k.get("kty").and_then(|x| x.as_str()).unwrap_or("");
            let dec = |field: &str| -> Result<Vec<u8>, AuthzError> {
                let s = k
                    .get(field)
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| AuthzError::BadRequest(format!("JWKS key missing `{field}`")))?;
                b64url(s)
            };
            match kty {
                "RSA" => {
                    set = set.with_key(
                        kid,
                        JwkKey::Rsa {
                            n: dec("n")?,
                            e: dec("e")?,
                        },
                    );
                }
                "EC" => {
                    let crv = k.get("crv").and_then(|x| x.as_str()).unwrap_or("");
                    if crv != "P-256" {
                        continue; // only ES256 / P-256 is supported here.
                    }
                    set = set.with_key(
                        kid,
                        JwkKey::EcP256 {
                            x: dec("x")?,
                            y: dec("y")?,
                        },
                    );
                }
                "OKP" => {
                    let crv = k.get("crv").and_then(|x| x.as_str()).unwrap_or("");
                    if crv != "Ed25519" {
                        continue; // only Ed25519 EdDSA is supported here.
                    }
                    set = set.with_key(kid, JwkKey::Ed25519 { x: dec("x")? });
                }
                _ => continue, // unsupported key type — skip (do not fabricate a key).
            }
        }
        Ok(set)
    }
}

// ================================================================================================
// Replay defence — the seen `jti`/`nonce` set.
// ================================================================================================

/// **The replay defence — a set of already-consumed `jti`/`nonce` values.** A presented OIDC ID
/// token's replay identifier (`jti`, falling back to `nonce`) is consumed once; a second
/// presentation of the SAME identifier is rejected (a replayed token does not authenticate). Shared
/// (cloneable) so every verifier handle consults one set. (A real deployment bounds this set by the
/// token TTL / a Redis-class store; here it is the in-process defence the corpus proves.)
#[derive(Clone, Default)]
pub struct ReplayGuard {
    seen: Arc<Mutex<BTreeSet<String>>>,
}

impl ReplayGuard {
    /// A fresh, empty replay guard.
    pub fn new() -> ReplayGuard {
        ReplayGuard {
            seen: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Consume `id`. Returns `true` if it was FRESH (newly recorded), `false` if it was ALREADY seen
    /// (a replay — the caller rejects).
    pub fn consume(&self, id: &str) -> bool {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string())
    }
}

// ================================================================================================
// Configuration + clock.
// ================================================================================================

/// The relying-party (RP) configuration the verifier validates a token against — the issuer it
/// trusts, the audience it IS, the claim names the tenant/region are read from, the clock-skew
/// leeway, and whether replay-defence material (`jti`/`nonce`) is mandatory.
#[derive(Clone, Debug)]
pub struct OidcConfig {
    /// The exact `iss` the token must carry (the configured IdP issuer).
    pub issuer: String,
    /// The audience this RP is — the token's `aud` MUST contain it.
    pub audience: String,
    /// The verified-claim name the TENANT is read from (the trust root; default `tenant`).
    pub tenant_claim: String,
    /// The verified-claim name the REGION is read from (default `region`).
    pub region_claim: String,
    /// Clock-skew leeway, in seconds, applied to `exp`/`nbf`/`iat` (default 60).
    pub leeway_secs: i64,
    /// Require replay-defence material (`jti` or `nonce`): a token carrying neither is refused
    /// (default `true` — a token with no replay identifier cannot be replay-protected).
    pub require_replay_defence: bool,
}

impl OidcConfig {
    /// A config for `issuer` / `audience` with the conventional defaults (`tenant`/`region` claims,
    /// 60s leeway, replay defence required).
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

    /// Override the claim names the tenant/region are read from (builder form).
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

/// The "now" source, in Unix seconds — injected so a test can pin the clock deterministically across
/// `exp`/`nbf` boundaries (the production default reads the system clock).
type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ================================================================================================
// The verifier.
// ================================================================================================

/// **The REAL OIDC ID-token (JWT) credential verifier (MR-010a).** Verifies an OIDC ID token against
/// the injected [`JwkSet`] with vetted primitives and the alg-confusion / `alg:none` defences, then
/// validates the claims and extracts a trust-rooted [`VerifiedAssertion`] — or refuses it loudly.
#[derive(Clone)]
pub struct OidcVerifier {
    config: OidcConfig,
    jwks: JwkSet,
    replay: ReplayGuard,
    now: NowFn,
}

impl OidcVerifier {
    /// Build the verifier over an injected JWKS + RP config, with a fresh replay guard and the system
    /// clock. (Wire it as the OIDC-scheme verifier via [`SchemeDispatchVerifier::route`].)
    pub fn new(config: OidcConfig, jwks: JwkSet) -> OidcVerifier {
        OidcVerifier {
            config,
            jwks,
            replay: ReplayGuard::new(),
            now: Arc::new(system_now),
        }
    }

    /// Build over an EXPLICIT shared [`ReplayGuard`] (so several verifier handles share one seen-set).
    pub fn with_replay_guard(mut self, replay: ReplayGuard) -> OidcVerifier {
        self.replay = replay;
        self
    }

    /// Build with an injected clock (Unix seconds) — the deterministic-test / drill seam.
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> OidcVerifier {
        self.now = Arc::new(now);
        self
    }

    /// The shared replay guard (so a caller can pre-seed / inspect the seen-set).
    pub fn replay_guard(&self) -> &ReplayGuard {
        &self.replay
    }

    fn now(&self) -> i64 {
        (self.now)()
    }
}

/// Read a numeric claim as `i64` (JWT numeric dates are integers, but tolerate a float encoding).
fn num_claim(claims: &serde_json::Value, key: &str) -> Option<i64> {
    let v = claims.get(key)?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Whether the token's `aud` (a string OR an array of strings) contains `want`.
fn aud_contains(claims: &serde_json::Value, want: &str) -> bool {
    match claims.get("aud") {
        Some(serde_json::Value::String(s)) => s == want,
        Some(serde_json::Value::Array(arr)) => arr.iter().any(|x| x.as_str() == Some(want)),
        _ => false,
    }
}

/// Verify the JWS signature over `msg` with the selected JWKS key, using the vetted crate for the
/// key's family. Returns `Ok(())` only on a cryptographically valid signature; any failure is a loud
/// refusal. NO signature math is hand-rolled — each arm calls the vetted crate's `verify`.
fn verify_signature(key: &JwkKey, msg: &[u8], sig: &[u8]) -> Result<(), AuthzError> {
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
            // The SEC1 uncompressed point `0x04 ‖ x ‖ y` ring's ECDSA verifier expects.
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
        // This verifier owns ONLY the OIDC scheme; another scheme is a wiring error (the dispatcher
        // routes by scheme). Refuse loudly rather than mis-verify.
        if credential.scheme != scheme::OIDC {
            return Err(AuthzError::BadRequest(format!(
                "OidcVerifier received a `{}` credential (expected `oidc`)",
                credential.scheme
            )));
        }

        // (1) Structural shape: a JWS compact serialization is exactly three dot-separated segments.
        let token = credential.material.trim();
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthzError::BadRequest(
                "malformed JWT: expected three dot-separated segments (header.payload.signature)"
                    .into(),
            ));
        }
        let (header_b64, payload_b64, sig_b64) = (parts[0], parts[1], parts[2]);

        // (2) Header — read `alg`/`kid`. The header is UNTRUSTED until the signature checks out; it
        //     is used ONLY to find the key and to detect alg confusion. The key is selected by `kid`.
        let header: serde_json::Value = serde_json::from_slice(&b64url(header_b64)?)
            .map_err(|e| AuthzError::BadRequest(format!("malformed JWT header JSON: {e}")))?;
        let alg = header
            .get("alg")
            .and_then(|a| a.as_str())
            .ok_or_else(|| AuthzError::BadRequest("JWT header missing `alg`".into()))?;

        // (2a) ALG-CONFUSION DEFENCE — `alg:none` is rejected outright (the unsigned-token bypass).
        if alg.eq_ignore_ascii_case("none") {
            return Err(refuse(
                "alg:none rejected — an unsigned OIDC token never authenticates (alg-confusion defence)",
            ));
        }
        // (2b) ALG-CONFUSION DEFENCE — a symmetric (HMAC) alg is rejected: the RS256→HS256 attack
        //      signs with the RSA *public* key as an HMAC secret. We only ever verify against an
        //      ASYMMETRIC JWKS key, so any `HS*` alg is a confusion attempt. We compare BYTES (not a
        //      `&str[..2]` slice — `alg` is attacker-controlled, and a slice that lands mid-UTF-8-char,
        //      e.g. `"Héx"`, would PANIC: a per-request DoS in the auth hot path). This check is
        //      defence-in-depth — the key-pinned `alg != expected` test below is the load-bearing one.
        if alg
            .as_bytes()
            .get(..2)
            .is_some_and(|p| p.eq_ignore_ascii_case(b"HS"))
        {
            return Err(refuse(format!(
                "symmetric alg `{alg}` rejected against an asymmetric JWKS key (the RS256→HS256 \
                 alg-confusion bypass)"
            )));
        }

        // (3) KID selection — the key is chosen by the token's `kid` from the INJECTED JWKS. A
        //     missing/unknown kid is refused (no fallback to a header-chosen alg/key).
        let kid = header
            .get("kid")
            .and_then(|k| k.as_str())
            .ok_or_else(|| refuse("JWT header missing `kid` (cannot select a JWKS key)"))?;
        let key = self
            .jwks
            .get(kid)
            .ok_or_else(|| refuse(format!("unknown `kid` `{kid}` (not in the injected JWKS)")))?;

        // (3a) ALG-CONFUSION DEFENCE — the expected alg is PINNED FROM THE KEY, not the header. The
        //      header `alg` must equal what the selected key supports (e.g. an RSA key only verifies
        //      RS256; a token claiming ES256 against an RSA key is refused).
        let expected = key.expected_alg();
        if alg != expected {
            return Err(refuse(format!(
                "alg `{alg}` does not match the key `{kid}` (expected `{expected}` — \
                 alg-confusion / wrong-alg defence)"
            )));
        }

        // (4) SIGNATURE — verify over the EXACT signing input `header_b64.payload_b64` (RFC 7515) with
        //     the vetted crate for the key family. A tampered payload (sig over different bytes), a
        //     wrong signing key, or a malformed signature all fail here.
        let signing_input = format!("{header_b64}.{payload_b64}");
        let sig = b64url(sig_b64)?;
        verify_signature(key, signing_input.as_bytes(), &sig)?;

        // (5) CLAIMS — only NOW (signature proven) do we trust the payload. Every fact below comes
        //     from the verified claims; nothing is caller-supplied.
        let claims: serde_json::Value = serde_json::from_slice(&b64url(payload_b64)?)
            .map_err(|e| AuthzError::BadRequest(format!("malformed JWT claims JSON: {e}")))?;

        // (5a) iss — must equal the configured issuer.
        let iss = claims
            .get("iss")
            .and_then(|i| i.as_str())
            .ok_or_else(|| refuse("token missing `iss`"))?;
        if iss != self.config.issuer {
            return Err(refuse(format!(
                "issuer mismatch: token `iss`=`{iss}` != configured `{}`",
                self.config.issuer
            )));
        }

        // (5b) aud — must contain this RP.
        if !aud_contains(&claims, &self.config.audience) {
            return Err(refuse(format!(
                "audience mismatch: token `aud` does not contain this RP `{}`",
                self.config.audience
            )));
        }

        // (5c) exp — required; expired (beyond leeway) → reject. The numeric claims are
        //      attacker-controlled (`exp`/`nbf`/`iat` can be i64::MAX/MIN), so all leeway arithmetic
        //      is SATURATING — a plain `exp + leeway` would overflow-panic in debug builds (a
        //      per-request DoS in the auth hot path). Saturation only ever makes the bound stricter
        //      (clamps to the i64 extreme), never accepts a token it otherwise would.
        let now = self.now();
        let leeway = self.config.leeway_secs;
        let exp = num_claim(&claims, "exp").ok_or_else(|| refuse("token missing `exp`"))?;
        if exp.saturating_add(leeway) < now {
            return Err(refuse(format!(
                "token expired: exp={exp} (+{leeway}s leeway) < now={now}"
            )));
        }
        // (5d) nbf — if present, must not be in the future (beyond leeway).
        if let Some(nbf) = num_claim(&claims, "nbf") {
            if nbf.saturating_sub(leeway) > now {
                return Err(refuse(format!(
                    "token not yet valid: nbf={nbf} (-{leeway}s leeway) > now={now}"
                )));
            }
        }
        // (5e) iat — if present, sanity: not issued (far) in the future.
        if let Some(iat) = num_claim(&claims, "iat") {
            if iat.saturating_sub(leeway) > now {
                return Err(refuse(format!(
                    "token `iat` in the future: iat={iat} (-{leeway}s leeway) > now={now}"
                )));
            }
        }

        // (5f) REPLAY DEFENCE — consume the `jti` (else `nonce`); a replayed identifier is rejected.
        let replay_id = claims
            .get("jti")
            .and_then(|j| j.as_str())
            .or_else(|| claims.get("nonce").and_then(|n| n.as_str()));
        match replay_id {
            Some(id) => {
                if !self.replay.consume(id) {
                    return Err(refuse(format!(
                        "replayed token: `jti`/`nonce` `{id}` was already presented (replay defence)"
                    )));
                }
            }
            None => {
                if self.config.require_replay_defence {
                    return Err(refuse(
                        "token carries neither `jti` nor `nonce` — no replay-defence material",
                    ));
                }
            }
        }

        // (6) THE TRUST-ROOTED ASSERTION — tenant/region/subject from the VERIFIED claims only.
        let sub = claims
            .get("sub")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| refuse("token missing `sub` (no subject)"))?;
        let tenant = claims
            .get(&self.config.tenant_claim)
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                refuse(format!(
                    "verified token carries no `{}` claim (the tenant is the trust root and must \
                     come from the IdP-verified claims, never a path)",
                    self.config.tenant_claim
                ))
            })?;
        let region = claims
            .get(&self.config.region_claim)
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
            .ok_or_else(|| {
                refuse(format!(
                    "verified token carries no `{}` claim",
                    self.config.region_claim
                ))
            })?;

        Ok(VerifiedAssertion {
            tenant: TenantId(tenant.to_string()),
            region: Region(region.to_string()),
            scheme: scheme::OIDC.to_string(),
            subject_key: sub.to_string(),
        })
    }
}

// ================================================================================================
// The per-scheme dispatch seam (wiring OidcVerifier as the OIDC verifier; fallback INJECTED).
// ================================================================================================

/// **Routes a credential to a per-scheme [`CredentialVerifier`], with an INJECTED fallback.** This is
/// how [`OidcVerifier`] is wired as the OIDC-scheme verifier while the not-yet-real schemes
/// (SAML/SCIM/passkey/SSH — MR-010b/c/d) ride the injected fallback. The dispatcher constructs NO
/// verifier itself (both the per-scheme verifiers and the fallback are injected), so it adds no
/// `Structural*` mock-crypto construction to the production graph; the full `StructuralVerifier`
/// prod-default removal is MR-012.
pub struct SchemeDispatchVerifier {
    by_scheme: BTreeMap<String, Arc<dyn CredentialVerifier>>,
    fallback: Arc<dyn CredentialVerifier>,
}

impl SchemeDispatchVerifier {
    /// A dispatcher whose unmatched schemes route to `fallback` (the caller injects it — e.g. the
    /// floor verifier for the not-yet-real schemes).
    pub fn new(fallback: Arc<dyn CredentialVerifier>) -> SchemeDispatchVerifier {
        SchemeDispatchVerifier {
            by_scheme: BTreeMap::new(),
            fallback,
        }
    }

    /// Route `scheme` to `verifier` (builder form) — e.g. `.route(scheme::OIDC, oidc)`.
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

    // ── Test token-minting helpers (REAL keys + REAL signatures) ─────────────────────────────────
    //
    // These mint genuinely-signed tokens so the positive corpus is a real crypto round-trip and the
    // negative corpus is real forgery. The keys are generated here; the verifier only ever sees the
    // PUBLIC half (via the injected JWKS).

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

    /// A fixed "now" (well within validity for the standard claims below).
    const NOW: i64 = 1_700_000_000;

    /// Standard valid claims for tenant `acme` / region `eu-west` / subject `oidc-sub-1`, with a
    /// unique `jti` so the replay guard does not collide across tests.
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

    // ── RSA (RS256) key + signer ─────────────────────────────────────────────────────────────────
    struct RsaKey {
        priv_key: rsa::RsaPrivateKey,
    }
    impl RsaKey {
        fn generate() -> RsaKey {
            use rand::rngs::OsRng;
            // 2048-bit — the OIDC floor. Generated once per test that needs it.
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

    // ── EC P-256 (ES256) key + signer (ring) ─────────────────────────────────────────────────────
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
            // The public key is the SEC1 uncompressed point `0x04 ‖ x ‖ y` (65 bytes).
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

    // ── Ed25519 (EdDSA) key + signer (ring) ──────────────────────────────────────────────────────
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

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // POSITIVE corpus — a correctly-signed RS256, ES256, and EdDSA token each VERIFY and yield the
    // right tenant/region/subject from the verified claims.
    // ════════════════════════════════════════════════════════════════════════════════════════════

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

    /// JWKS round-trips through the standard RFC 7517 JSON shape (the `jwks_uri` body), and a token
    /// verifies against the parsed key set.
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

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // NEGATIVE corpus — each forged/invalid token MUST be refused (the whole point of the prompt).
    // ════════════════════════════════════════════════════════════════════════════════════════════

    /// (a) `alg:none` — an unsigned token never authenticates (the unsigned-token bypass).
    #[test]
    fn negative_alg_none_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "none", "kid": "rsa-1"});
        let cl = claims("jti-none");
        // No signature (or any bytes) — alg:none must be refused regardless.
        let token = jwt(&header, &cl, b"");
        let err = verifier(jwks).verify(&cred(token)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("alg:none")),
            "alg:none must be refused loudly, got {err:?}"
        );
    }

    /// (b) ALG CONFUSION — an RS256 key, but the token claims HS256 and is "signed" with the RSA
    /// public key as an HMAC secret (the classic RS256→HS256 bypass). MUST be rejected.
    #[test]
    fn negative_alg_confusion_rs256_key_as_hs256_is_rejected() {
        let key = RsaKey::generate();
        let pub_jwk = key.jwk();
        // The attacker's HMAC secret = the RSA public key bytes (n‖e) — the public material.
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
        // A real HMAC-SHA256 over the signing input with the public key as the secret.
        let mac = hmac_sha256(&hmac_secret, si.as_bytes());
        let token = format!("{si}.{}", b64(&mac));
        let err = verifier(jwks).verify(&cred(token)).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("alg-confusion")),
            "the RS256-key-as-HS256 confusion must be refused, got {err:?}"
        );
    }

    /// (c) WRONG KEY — a structurally valid RS256 token signed by an UNKNOWN key (not in the JWKS for
    /// that kid). The signature must fail to verify against the published key.
    #[test]
    fn negative_wrong_signing_key_is_rejected() {
        let real = RsaKey::generate();
        let attacker = RsaKey::generate();
        // The JWKS publishes the REAL key under kid `rsa-1`; the attacker signs with THEIR key.
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

    /// (c') UNKNOWN KID — the token names a `kid` not in the JWKS. No fallback; refuse.
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

    /// (d) EXPIRED — `exp` in the past beyond leeway. Reject.
    #[test]
    fn negative_expired_token_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let mut cl = claims("jti-expired");
        cl["exp"] = serde_json::json!(NOW - 1000); // well beyond the 60s leeway
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        let err = verifier(jwks)
            .verify(&cred(jwt(&header, &cl, &sig)))
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired token must be refused, got {err:?}"
        );
    }

    /// (e) REPLAY — the SAME token presented twice. First verifies; the second (same `jti`) refused.
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

    /// (f) WRONG AUD — the token's audience is some other RP. Reject.
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

    /// (g) WRONG ISS — the token's issuer is not the configured IdP. Reject.
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

    /// (h) TAMPERED PAYLOAD — a valid signature over the ORIGINAL claims, then the payload segment is
    /// swapped for different bytes (tenant `acme`→`globex`). The sig no longer matches → reject. This
    /// is the load-bearing IDOR/forgery case: you cannot edit the tenant after signing.
    #[test]
    fn negative_tampered_payload_is_rejected() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        let cl = claims("jti-tamper");
        let sig = key.sign(signing_input(&header, &cl).as_bytes());
        // Forge: keep the header + signature, but substitute a payload claiming tenant `globex`.
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

    /// (i) MALFORMED / GARBAGE — not a JWT, bad base64, wrong segment count. Loud structural refusal.
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
            "header.payload", // 2 segments
        ] {
            let err = v.verify(&cred(bad.to_string())).unwrap_err();
            assert!(
                matches!(err, AuthzError::BadRequest(_) | AuthzError::FailClosed(_)),
                "garbage `{bad}` must be refused"
            );
        }
    }

    /// (j) PANIC-SAFETY — a NON-ASCII / odd-byte `alg` header must be REFUSED, never PANIC. `alg` is
    /// attacker-controlled; a `&str[..2]` byte-slice that lands mid-UTF-8-char (e.g. `"Héx"`, where
    /// byte 2 is inside the 2-byte `é`) would panic "byte index 2 is not a char boundary" — a
    /// per-request DoS in the auth hot path. The fix compares bytes via `get(..2)`. The verify call
    /// returning at all (no panic) + an `Err` is the proof.
    #[test]
    fn negative_odd_alg_header_bytes_are_refused_not_panicking() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let v = verifier(jwks);
        // Various attacker `alg` shapes that previously risked a byte-slice panic: a multibyte char
        // straddling index 2, a 1-byte alg, an empty alg, a lowercase `hs`, and emoji.
        for bad_alg in ["Héx", "H", "", "hs256", "H€", "🔥256", "Hé"] {
            let header = serde_json::json!({"alg": bad_alg, "kid": "rsa-1"});
            let cl = claims("jti-odd-alg");
            // Sign over the (odd-header) signing input so the path is realistic; verification must
            // refuse on alg grounds (or sig), and crucially must not panic on the header parse.
            let sig = key.sign(signing_input(&header, &cl).as_bytes());
            let r = v.verify(&cred(jwt(&header, &cl, &sig)));
            assert!(
                r.is_err(),
                "odd alg `{bad_alg:?}` must be refused (and must not panic)"
            );
        }
    }

    /// (k) PANIC-SAFETY — attacker-controlled EXTREME numeric claims (`exp`/`nbf`/`iat` at i64::MAX /
    /// i64::MIN) must not overflow-panic the leeway arithmetic (a debug-build per-request DoS). They
    /// are handled with saturating arithmetic; the verify call returns a verdict, never panics. The
    /// tokens are properly SIGNED so the path actually reaches the (5c/5d/5e) numeric checks.
    #[test]
    fn extreme_numeric_claims_do_not_panic() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let v = verifier(jwks);
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        // Each case mutates one numeric claim to an i64 extreme; all must return (no panic).
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
            // We only assert it RETURNS (no panic). Some extremes verify (e.g. exp=MAX is "not
            // expired"), some refuse (nbf=MAX is "not yet valid") — both are acceptable; a panic is
            // not. `verify` is total over attacker bytes.
            let _ = v.verify(&cred(jwt(&header, &cl, &sig)));
        }
    }

    /// THE IDOR/TRUST-ROOT PROPERTY — the tenant comes from the VERIFIED claims. A token verified for
    /// `acme` yields `acme`; the only way to assert a different tenant is to re-sign (which the
    /// attacker cannot, lacking the IdP private key — proven by (c)/(h)). And a verified token that
    /// carries NO tenant claim is refused (we never fabricate a tenant).
    #[test]
    fn tenant_comes_only_from_verified_claims() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let header = serde_json::json!({"alg": "RS256", "kid": "rsa-1"});
        // No `tenant` claim → refuse (never a fabricated/empty tenant).
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

    // ── The dispatch seam ────────────────────────────────────────────────────────────────────────

    /// The dispatcher routes the OIDC scheme to the REAL [`OidcVerifier`] and everything else to the
    /// injected fallback. (Construction of the floor `StructuralVerifier` here is `#[cfg(test)]`, so
    /// the production-graph scanner admits it.)
    #[test]
    fn dispatch_routes_oidc_to_real_verifier_and_others_to_fallback() {
        let key = RsaKey::generate();
        let jwks = JwkSet::new().with_key("rsa-1", key.jwk());
        let oidc = Arc::new(verifier(jwks));
        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::OIDC, oidc);

        // An OIDC credential goes through the real crypto verifier — a forged (alg:none) one is
        // refused by it, NOT silently accepted by the floor.
        let header = serde_json::json!({"alg": "none", "kid": "rsa-1"});
        let cl = claims("jti-dispatch-none");
        let forged = jwt(&header, &cl, b"");
        assert!(
            dispatch.verify(&cred(forged)).is_err(),
            "an OIDC alg:none token must hit the real verifier and be refused"
        );

        // A SAML credential (not-yet-real) rides the injected floor fallback — the floor parses its
        // structural envelope (proving routing reached the fallback, unchanged from before).
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

    // ── A tiny, self-contained HMAC-SHA256 for the alg-confusion forgery ONLY (test code). ───────
    // This is NOT used by the verifier (which never accepts a symmetric alg); it exists only to MINT
    // the attacker's forged HS256 token so we can prove the verifier refuses it.
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
