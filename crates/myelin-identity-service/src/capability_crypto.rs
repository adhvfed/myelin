//! # `capability_crypto` — REAL machine/capability-token crypto: PASETO v4.public + macaroon
//! attenuation + DPoP (MR-011; census SI-002/003, the machine-identity slice of P-527).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §4 (capability tokens are
//! **attenuable bearer tokens** — PASETO v4 envelope — whose authority is a **macaroon/biscuit caveat
//! chain** with **monotone attenuation**; **DPoP** sender-constrains long-lived PATs).
//!
//! ## What this module replaces (the census SI-002/003 CRITICAL)
//! The machine-token graph was wired to [`crate::machine_auth::StructuralTokenVerifier`] /
//! [`crate::mint::StructuralTokenSigner`] — MOCK crypto that parses/emits a PLAINTEXT
//! `<tenant>|<region>|…` envelope, so ANYONE can forge any capability token in any tenant. This module
//! makes the CRYPTO real behind the SAME [`crate::machine_auth::TokenVerifier`] /
//! [`crate::mint::TokenSigner`] seams (the authority LOGIC — monotone attenuation, DPoP-binding,
//! denylist — already lives in `machine_auth`; here we make signing/attenuation/proof CRYPTOGRAPHIC).
//!
//! ## The three primitives (vetted crates only — no hand-rolled signature/MAC math)
//! 1. **PASETO v4.public** (Ed25519-signed, hand-assembled on `ring`): the cell signs the token's
//!    ROOT claims (tenant/region/sub/jti/exp/root-authority/cnf). A forged or tampered token fails the
//!    Ed25519 verify against the cell's PUBLIC key (the injected trust anchor — never read from the
//!    token). PASETO v4.public is pre-auth-encoding + Ed25519 verify — a well-specified, simple format,
//!    NOT a crypto minefield; we assemble PAE + call `ring`'s vetted Ed25519, never novel crypto.
//! 2. **Macaroon-style HMAC caveat chain** (`hmac`+`sha2`): the holder can attenuate OFFLINE (add a
//!    narrowing caveat) but CANNOT amplify. Each caveat is chained `tag_i = HMAC(key = tag_{i-1}, msg
//!    = caveat_i)`, with `tag_0 = HMAC(key = K_mac, msg = signature)` seeded from a **cell-held secret**
//!    `K_mac` (NOT in the token). A holder knows the current tail tag (so can EXTEND) but not `K_mac`
//!    (so cannot recompute an earlier tag to REMOVE a caveat) → attenuation is one-way. Amplification
//!    is doubly impossible: (a) the verifier rejects any caveat naming a grant outside the running
//!    set, and (b) the effective authority is the INTERSECTION down the chain, so it is always ⊆ the
//!    Ed25519-signed root authority (widening would require forging the root signature).
//! 3. **DPoP (RFC 9449)**: a long-lived PAT is bound to a client key — the `cnf.jkt` thumbprint in the
//!    signed token. Each request carries a DPoP proof (an Ed25519-signed `htm`/`htu`/`iat`/`jti`,
//!    domain-separated). The verifier checks the proof signature against the proof's embedded key, that
//!    the key's thumbprint == the token's bound `jkt`, that `htm`/`htu` match the (injected) request,
//!    freshness (`iat` within a window), and single-use (`jti` replay guard). A stolen token presented
//!    WITHOUT a valid proof (or with a proof by a DIFFERENT key) is refused.
//!
//! ## What is INJECTED, and what is honestly deferred
//! The cell PUBLIC key + the macaroon secret + the DPoP request binding (`htm`/`htu`) + the clock + the
//! replay guard are **injected** (no network / no wall-clock in the crypto path — the corpus is
//! deterministic). **Deferred (named, not faked):** PASETO footer-based key-id / key rotation (a single
//! anchor here); a full biscuit asymmetric block chain (the macaroon HMAC chain is the sound, simpler
//! construction the prompt names); binding `kind` into the signed body (it is read from the credential
//! scheme, exactly as the structural floor did — a deny-on-mismatch hardening, MR-011b). The runtime
//! per-request DPoP binding wiring (the gateway threading the real `htm`/`htu`) is the same injected
//! seam the SSH challenge wiring is.

use crate::machine_auth::{Authority, CapabilityToken, MachineKind};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use ring::signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, BTreeMap};
use std::sync::{Arc, Mutex};

type HmacSha256 = Hmac<Sha256>;

/// The PASETO v4.public header (the only version/purpose this module mints/verifies).
const V4_PUBLIC_HEADER: &str = "v4.public.";
/// The DPoP proof header (a domain-separation prefix; an Ed25519-signed `htm`/`htu`/`iat`/`jti`).
const DPOP_HEADER: &str = "dpop.v1.";
/// The HMAC personalization for the macaroon root tag (`tag_0 = HMAC(K_mac, sig)` domain).
const MACAROON_DOMAIN: &[u8] = b"myelin.cap.macaroon.v1";

/// base64url (no padding) — the SAME codec the OIDC/SSH paths use (RFC 7515 §2 alphabet).
fn b64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
fn b64url_decode(s: &str) -> Result<Vec<u8>, AuthzError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|e| AuthzError::BadRequest(format!("malformed base64url segment: {e}")))
}

/// A LOUD refusal of a well-formed-but-invalid capability credential (bad signature, amplified caveat,
/// expired, DPoP failure, replay). `FailClosed` so an unverifiable token NEVER resolves to a Principal.
fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

// ================================================================================================
// PASETO v4.public — pre-auth encoding + Ed25519 (hand-assembled on `ring`, no novel crypto).
// ================================================================================================

/// PASETO `LE64`: a 64-bit little-endian length with the MSB cleared (the spec's pre-auth-encoding
/// length prefix — clearing the high bit reserves it, per the PASETO PAE definition).
fn le64(n: u64) -> [u8; 8] {
    let mut out = n.to_le_bytes();
    out[7] &= 0x7f;
    out
}

/// PASETO **PAE** (pre-auth encoding) of a piece list — `LE64(count) ‖ for each p: LE64(len(p)) ‖ p`.
/// This is the EXACT message Ed25519 signs/verifies over, so a tampered field changes the signed
/// bytes and fails verification (the binding the whole token rests on).
fn pae(pieces: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&le64(pieces.len() as u64));
    for p in pieces {
        out.extend_from_slice(&le64(p.len() as u64));
        out.extend_from_slice(p);
    }
    out
}

/// Sign `claims` as a PASETO v4.public token with the cell's Ed25519 key (footer empty). The signature
/// is over `PAE([header, claims, footer, implicit])` — the spec's signing input.
fn paseto_v4_public_sign(key: &Ed25519KeyPair, claims: &[u8]) -> String {
    let m2 = pae(&[V4_PUBLIC_HEADER.as_bytes(), claims, b"", b""]);
    let sig = key.sign(&m2);
    let mut body = claims.to_vec();
    body.extend_from_slice(sig.as_ref()); // m ‖ sig
    format!("{V4_PUBLIC_HEADER}{}", b64url_encode(&body))
}

/// Verify a PASETO v4.public token against the cell's 32-byte Ed25519 PUBLIC key (the injected trust
/// anchor). Returns `(claims_bytes, signature_bytes)` on success; a forged/tampered token is a LOUD
/// refusal. TOTAL over attacker bytes — every decode is checked, no slice can panic.
fn paseto_v4_public_verify(public_key: &[u8], token: &str) -> Result<(Vec<u8>, Vec<u8>), AuthzError> {
    let rest = token.strip_prefix(V4_PUBLIC_HEADER).ok_or_else(|| {
        AuthzError::BadRequest("not a v4.public token (bad header/version)".into())
    })?;
    // A footer (a second `.`) is not minted by this module; reject one rather than ignore it.
    if rest.contains('.') {
        return Err(AuthzError::BadRequest(
            "v4.public token carries an unexpected footer segment".into(),
        ));
    }
    let body = b64url_decode(rest)?;
    if body.len() < 64 {
        return Err(AuthzError::BadRequest(
            "v4.public body shorter than a 64-byte Ed25519 signature".into(),
        ));
    }
    let (claims, sig) = body.split_at(body.len() - 64);
    let m2 = pae(&[V4_PUBLIC_HEADER.as_bytes(), claims, b"", b""]);
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&m2, sig)
        .map_err(|_| refuse("capability-token signature verification failed (forged or tampered)"))?;
    Ok((claims.to_vec(), sig.to_vec()))
}

// ================================================================================================
// The cell's token authority (mint side) + the verifier's trust anchor (verify side).
// ================================================================================================

/// **The cell's secret capability-token authority (the MINT side).** Holds the Ed25519 signing key
/// (whose PUBLIC half is the verifier's trust anchor) and the macaroon root secret `K_mac` (the
/// holder never sees either secret). Constructed from a 32-byte Ed25519 seed + a 32-byte MAC key
/// (deterministic for tests / the drill; a production cell loads these from the KMS-sealed cell root).
pub struct CellTokenAuthority {
    signing_key: Ed25519KeyPair,
    public_key: [u8; 32],
    mac_key: [u8; 32],
}

impl CellTokenAuthority {
    /// Build from an explicit 32-byte Ed25519 seed + 32-byte macaroon secret (the injected cell
    /// material). A malformed seed is a loud error (never a fabricated key).
    pub fn from_seed(ed25519_seed: &[u8; 32], mac_key: &[u8; 32]) -> Result<CellTokenAuthority, AuthzError> {
        let signing_key = Ed25519KeyPair::from_seed_unchecked(ed25519_seed)
            .map_err(|e| AuthzError::BadRequest(format!("invalid Ed25519 cell seed: {e}")))?;
        use ring::signature::KeyPair;
        let mut public_key = [0u8; 32];
        let pk = signing_key.public_key().as_ref();
        if pk.len() != 32 {
            return Err(AuthzError::BadRequest("unexpected Ed25519 public-key length".into()));
        }
        public_key.copy_from_slice(pk);
        Ok(CellTokenAuthority {
            signing_key,
            public_key,
            mac_key: *mac_key,
        })
    }

    /// **Generate a fresh per-cell token authority (the MR-012 production-boot floor).** Samples a
    /// random 32-byte Ed25519 seed + 32-byte macaroon secret from the OS CSPRNG and builds a REAL
    /// signing authority. This is genuine Ed25519/PASETO crypto — the production composition root signs
    /// AND verifies per-run tokens under this keypair, so a FORGED token is rejected (no plaintext
    /// envelope, no mock). The KMS-sealed cell-root LOAD of this material (so the key survives a cell
    /// restart / is shared across the cell) is the named key-provenance follow-on (P-527 / MR-025) — a
    /// key-management concern, NOT mock crypto. The private seed never leaves this process.
    pub fn generate() -> CellTokenAuthority {
        use ring::rand::SecureRandom;
        let rng = ring::rand::SystemRandom::new();
        let mut seed = [0u8; 32];
        let mut mac = [0u8; 32];
        rng.fill(&mut seed).expect("OS CSPRNG fills the Ed25519 seed");
        rng.fill(&mut mac).expect("OS CSPRNG fills the macaroon secret");
        CellTokenAuthority::from_seed(&seed, &mac)
            .expect("a random 32-byte Ed25519 seed is always a valid cell authority")
    }

    /// The verifier's trust anchor (the cell PUBLIC key + the shared macaroon secret). Injected into a
    /// [`PasetoCapabilityVerifier`]; it carries NO Ed25519 private key (a verifier can check but never
    /// mint a token).
    pub fn trust_anchor(&self) -> CellTrustAnchor {
        CellTrustAnchor {
            public_key: self.public_key,
            mac_key: self.mac_key,
        }
    }

    /// **Mint a capability credential (the cell's full-control minting API).** Signs the root claims
    /// and seeds the macaroon chain (zero caveats at mint). Returns the credential `material` string
    /// `<paseto>|<caveats_b64>|<tail_b64>[|<dpop_b64>]` a [`PasetoCapabilityVerifier`] verifies. `dpop`
    /// is the bound client thumbprint (`cnf.jkt`) for a long-lived PAT — `None` for the TTL-constrained
    /// per-run tokens.
    pub fn mint(&self, spec: &CapabilityMintSpec) -> String {
        let mut claims = serde_json::Map::new();
        claims.insert("tenant".into(), spec.tenant.clone().into());
        claims.insert("region".into(), spec.region.clone().into());
        claims.insert("sub".into(), spec.subject_key.clone().into());
        claims.insert("jti".into(), spec.jti.clone().into());
        claims.insert("exp".into(), spec.exp_unix.into());
        let auth: Vec<serde_json::Value> = spec
            .authority
            .iter()
            .map(|g| serde_json::Value::String(g.clone()))
            .collect();
        claims.insert("auth".into(), auth.into());
        if let Some(jkt) = &spec.dpop_jkt {
            let mut cnf = serde_json::Map::new();
            cnf.insert("jkt".into(), jkt.clone().into());
            claims.insert("cnf".into(), cnf.into());
        }
        let claims_bytes = serde_json::to_vec(&serde_json::Value::Object(claims))
            .expect("claims serialize");
        let paseto = paseto_v4_public_sign(&self.signing_key, &claims_bytes);
        // The macaroon root tag binds the (empty) caveat chain to THIS token's unique signature under
        // the cell secret K_mac — the holder cannot recompute it (no K_mac), so caveats are extend-only.
        let sig = paseto_root_signature(&paseto).expect("freshly-minted token has a signature");
        let tail = macaroon_root_tag(&self.mac_key, &sig);
        encode_material(&paseto, &[], &tail, None)
    }
}

/// **The injected verifier trust anchor** — the cell PUBLIC key (for the Ed25519 root verify) + the
/// shared macaroon secret (to recompute `tag_0`; the same cell mints and verifies). Holds NO private
/// signing key.
#[derive(Clone)]
pub struct CellTrustAnchor {
    public_key: [u8; 32],
    mac_key: [u8; 32],
}

/// The claims a cell stamps into a freshly-minted capability token (the ROOT authority — caveats
/// narrow it offline).
#[derive(Clone, Debug)]
pub struct CapabilityMintSpec {
    /// The tenant the token is minted for (the trust root — signed, never path-derived).
    pub tenant: String,
    /// The residency region.
    pub region: String,
    /// The S1 token-record subject key.
    pub subject_key: String,
    /// The token's unique revocation id.
    pub jti: String,
    /// The expiry as a Unix-seconds instant (a numeric instant — no lexical-compare hazard).
    pub exp_unix: i64,
    /// The ROOT authority (the grant set; attenuation only narrows it).
    pub authority: Vec<String>,
    /// The DPoP-bound client thumbprint (`cnf.jkt`) for a long-lived PAT (`None` for per-run tokens).
    pub dpop_jkt: Option<String>,
}

// ================================================================================================
// The credential material wire format: `<paseto>|<caveats_b64>|<tail_b64>[|<dpop_b64>]`.
// ================================================================================================

/// Encode the credential `material`. PASETO v4.public + base64url use no `|`, so `|` is an unambiguous
/// outer delimiter. `caveats` is the (possibly empty) offline-added narrowing chain; `tail` is the
/// macaroon tag; `dpop` is the optional per-request proof.
fn encode_material(paseto: &str, caveats: &[BTreeSet<String>], tail: &[u8], dpop: Option<&str>) -> String {
    let caveats_json: Vec<Vec<String>> = caveats.iter().map(|c| c.iter().cloned().collect()).collect();
    let caveats_b64 = b64url_encode(&serde_json::to_vec(&caveats_json).expect("caveats serialize"));
    let tail_b64 = b64url_encode(tail);
    match dpop {
        Some(d) => format!("{paseto}|{caveats_b64}|{tail_b64}|{}", b64url_encode(d.as_bytes())),
        None => format!("{paseto}|{caveats_b64}|{tail_b64}"),
    }
}

/// Extract the Ed25519 signature (64 bytes) from a PASETO v4.public token (the macaroon root seed).
fn paseto_root_signature(paseto: &str) -> Result<Vec<u8>, AuthzError> {
    let rest = paseto
        .strip_prefix(V4_PUBLIC_HEADER)
        .ok_or_else(|| AuthzError::BadRequest("not a v4.public token".into()))?;
    let body = b64url_decode(rest.split('.').next().unwrap_or(""))?;
    if body.len() < 64 {
        return Err(AuthzError::BadRequest("token body too short for a signature".into()));
    }
    Ok(body[body.len() - 64..].to_vec())
}

/// `tag_0 = HMAC-SHA256(K_mac, MACAROON_DOMAIN ‖ signature)`. Seeded from the CELL SECRET so a holder
/// (who knows the signature but not `K_mac`) cannot reconstruct it → cannot remove a caveat.
fn macaroon_root_tag(mac_key: &[u8; 32], sig: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(mac_key).expect("HMAC accepts a 32-byte key");
    mac.update(MACAROON_DOMAIN);
    mac.update(sig);
    mac.finalize().into_bytes().to_vec()
}

/// Canonical bytes of one caveat (a sorted grant set) for the HMAC chain — deterministic so the
/// minter, an attenuating holder, and the verifier all fold the SAME bytes.
fn caveat_bytes(caveat: &BTreeSet<String>) -> Vec<u8> {
    let sorted: Vec<&String> = caveat.iter().collect();
    serde_json::to_vec(&sorted).expect("caveat serialize")
}

/// `tag_{i} = HMAC-SHA256(key = tag_{i-1}, msg = caveat_i)` — the macaroon fold. Extend-only: a holder
/// with `tag_{i-1}` can compute `tag_i` (add a caveat) but cannot invert to a prior tag.
fn macaroon_fold(prev_tag: &[u8], caveat: &BTreeSet<String>) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(prev_tag).expect("HMAC accepts any key length");
    mac.update(&caveat_bytes(caveat));
    mac.finalize().into_bytes().to_vec()
}

/// **Offline holder-side attenuation (NO secrets needed).** Given a credential `material` and a
/// narrowing `caveat_grants`, append the caveat and advance the macaroon tail. The result is a fresh
/// credential whose effective authority is the INTERSECTION of the parent with the caveat — it can
/// only NARROW (the verifier rejects any caveat that names a grant the parent lacks). A holder cannot
/// widen or remove a caveat (no `K_mac`). Returns a loud error on a malformed input material.
pub fn attenuate<I, S>(material: &str, caveat_grants: I) -> Result<String, AuthzError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let parsed = ParsedMaterial::parse(material)?;
    let caveat: BTreeSet<String> = caveat_grants.into_iter().map(Into::into).collect();
    let new_tail = macaroon_fold(&parsed.tail, &caveat);
    let mut caveats = parsed.caveats.clone();
    caveats.push(caveat);
    Ok(encode_material(
        &parsed.paseto,
        &caveats,
        &new_tail,
        parsed.dpop.as_deref(),
    ))
}

/// The parsed credential material (TOTAL over attacker bytes — every field is decode-checked).
struct ParsedMaterial {
    paseto: String,
    caveats: Vec<BTreeSet<String>>,
    tail: Vec<u8>,
    dpop: Option<String>,
}

impl ParsedMaterial {
    fn parse(material: &str) -> Result<ParsedMaterial, AuthzError> {
        let parts: Vec<&str> = material.split('|').collect();
        if parts.len() < 3 || parts.len() > 4 {
            return Err(AuthzError::BadRequest(
                "malformed capability credential (expected `<paseto>|<caveats>|<tail>[|<dpop>]`)".into(),
            ));
        }
        let paseto = parts[0].to_string();
        if !paseto.starts_with(V4_PUBLIC_HEADER) {
            return Err(AuthzError::BadRequest("credential token is not a v4.public PASETO".into()));
        }
        let caveats_raw: serde_json::Value = serde_json::from_slice(&b64url_decode(parts[1])?)
            .map_err(|e| AuthzError::BadRequest(format!("malformed caveat chain: {e}")))?;
        let caveats = match caveats_raw {
            serde_json::Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for c in arr {
                    let set: BTreeSet<String> = match c {
                        serde_json::Value::Array(grants) => grants
                            .into_iter()
                            .filter_map(|g| g.as_str().map(str::to_string))
                            .collect(),
                        _ => {
                            return Err(AuthzError::BadRequest(
                                "a caveat must be a JSON array of grant strings".into(),
                            ))
                        }
                    };
                    out.push(set);
                }
                out
            }
            _ => return Err(AuthzError::BadRequest("the caveat chain must be a JSON array".into())),
        };
        let tail = b64url_decode(parts[2])?;
        let dpop = match parts.get(3) {
            Some(d) => Some(
                String::from_utf8(b64url_decode(d)?)
                    .map_err(|_| AuthzError::BadRequest("DPoP proof is not valid UTF-8".into()))?,
            ),
            None => None,
        };
        Ok(ParsedMaterial {
            paseto,
            caveats,
            tail,
            dpop,
        })
    }
}

// ================================================================================================
// DPoP (RFC 9449) — the sender-constraint proof.
// ================================================================================================

/// **A DPoP client key (the sender).** Holds an Ed25519 keypair; its PUBLIC thumbprint (`jkt`) is the
/// `cnf.jkt` bound into a long-lived PAT. The client signs a fresh proof per request.
pub struct DpopClientKey {
    key: Ed25519KeyPair,
    public_key: [u8; 32],
}

impl DpopClientKey {
    /// Build from a 32-byte Ed25519 seed (deterministic for tests; a real client holds it in its
    /// keystore).
    pub fn from_seed(seed: &[u8; 32]) -> Result<DpopClientKey, AuthzError> {
        let key = Ed25519KeyPair::from_seed_unchecked(seed)
            .map_err(|e| AuthzError::BadRequest(format!("invalid DPoP client seed: {e}")))?;
        use ring::signature::KeyPair;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(key.public_key().as_ref());
        Ok(DpopClientKey { key, public_key })
    }

    /// The JWK thumbprint (`jkt`) — `base64url(SHA-256(public_key))`. This is the value bound into the
    /// PAT's `cnf.jkt`; a proof's key must hash to the SAME thumbprint.
    pub fn jkt(&self) -> String {
        dpop_jkt(&self.public_key)
    }

    /// Produce a DPoP proof binding this request: `htm` (method), `htu` (URL), `iat` (Unix seconds),
    /// `jti` (a per-proof unique id). The proof carries the client public key and an Ed25519 signature
    /// over the domain-separated `PAE([header, payload])`.
    pub fn prove(&self, htm: &str, htu: &str, iat: i64, jti: &str) -> String {
        let payload = serde_json::json!({
            "jwk": b64url_encode(&self.public_key),
            "htm": htm,
            "htu": htu,
            "iat": iat,
            "jti": jti,
        });
        let payload_bytes = serde_json::to_vec(&payload).expect("dpop payload serialize");
        let signing_input = pae(&[DPOP_HEADER.as_bytes(), &payload_bytes]);
        let sig = self.key.sign(&signing_input);
        format!(
            "{DPOP_HEADER}{}.{}",
            b64url_encode(&payload_bytes),
            b64url_encode(sig.as_ref())
        )
    }
}

/// The JWK thumbprint of a 32-byte Ed25519 public key — `base64url(SHA-256(pub))`.
fn dpop_jkt(public_key: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(public_key);
    b64url_encode(&h.finalize())
}

/// **The injected request binding the DPoP proof must match (RFC 9449 `htm`/`htu`).** In production the
/// gateway supplies the real method + canonical URL of THIS request; in tests it is injected.
#[derive(Clone, Debug)]
pub struct DpopBinding {
    /// The HTTP method the proof must carry (`htm`).
    pub htm: String,
    /// The HTTP target URI the proof must carry (`htu`).
    pub htu: String,
}

/// **The DPoP single-use replay guard** — a proof `jti` is consumed once; a second presentation of the
/// SAME `jti` is rejected (a captured proof cannot be replayed). Shared (cloneable) so one seen-set
/// backs every verifier handle. Mirrors [`crate::oidc::ReplayGuard`] (one replay-defence shape).
#[derive(Clone, Default)]
pub struct DpopReplayGuard {
    seen: Arc<Mutex<BTreeSet<String>>>,
}

impl DpopReplayGuard {
    /// A fresh, empty replay guard.
    pub fn new() -> DpopReplayGuard {
        DpopReplayGuard {
            seen: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Consume `jti`. `true` if FRESH (newly recorded), `false` if it was ALREADY seen (a replay).
    fn consume(&self, jti: &str) -> bool {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(jti.to_string())
    }
}

/// Verify a DPoP proof against the token's bound `jkt`, the request binding, and freshness/replay.
/// TOTAL over attacker bytes; a LOUD refusal on any failure.
#[allow(clippy::too_many_arguments)]
fn verify_dpop_proof(
    proof: &str,
    bound_jkt: &str,
    binding: &DpopBinding,
    now: i64,
    window_secs: i64,
    replay: &DpopReplayGuard,
) -> Result<(), AuthzError> {
    let rest = proof
        .strip_prefix(DPOP_HEADER)
        .ok_or_else(|| AuthzError::BadRequest("DPoP proof has a bad header".into()))?;
    let mut segs = rest.split('.');
    let payload_b64 = segs.next().unwrap_or("");
    let sig_b64 = segs
        .next()
        .ok_or_else(|| AuthzError::BadRequest("DPoP proof missing signature segment".into()))?;
    if segs.next().is_some() {
        return Err(AuthzError::BadRequest("DPoP proof has trailing segments".into()));
    }
    let payload_bytes = b64url_decode(payload_b64)?;
    let sig = b64url_decode(sig_b64)?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| AuthzError::BadRequest(format!("malformed DPoP payload JSON: {e}")))?;

    // (1) The embedded client key, and the Ed25519 signature over the domain-separated PAE.
    let jwk_b64 = payload
        .get("jwk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| refuse("DPoP proof missing `jwk`"))?;
    let pub_key = b64url_decode(jwk_b64)?;
    if pub_key.len() != 32 {
        return Err(refuse("DPoP `jwk` is not a 32-byte Ed25519 key"));
    }
    let signing_input = pae(&[DPOP_HEADER.as_bytes(), &payload_bytes]);
    UnparsedPublicKey::new(&ED25519, pub_key.clone())
        .verify(&signing_input, &sig)
        .map_err(|_| refuse("DPoP proof signature verification failed"))?;

    // (2) THE SENDER-CONSTRAINT — the proof key's thumbprint MUST equal the token's bound `cnf.jkt`.
    //     A proof signed by a DIFFERENT key verifies against ITS embedded jwk but its thumbprint will
    //     not match the bound jkt — refused (a stolen token + the thief's own key cannot bind).
    let proof_jkt = dpop_jkt(&pub_key);
    if proof_jkt != bound_jkt {
        return Err(refuse(
            "DPoP proof key thumbprint does not match the token's bound `cnf.jkt` (sender-constraint \
             violated — the proof was signed by a different key than the token is bound to)",
        ));
    }

    // (3) htm / htu MUST match THIS request (a proof minted for another method/URL is refused).
    let htm = payload.get("htm").and_then(|v| v.as_str()).unwrap_or("");
    let htu = payload.get("htu").and_then(|v| v.as_str()).unwrap_or("");
    if htm != binding.htm {
        return Err(refuse(format!(
            "DPoP `htm` mismatch: proof=`{htm}` request=`{}`",
            binding.htm
        )));
    }
    if htu != binding.htu {
        return Err(refuse(format!(
            "DPoP `htu` mismatch: proof=`{htu}` request=`{}`",
            binding.htu
        )));
    }

    // (4) FRESHNESS — `iat` within ±window. Saturating arithmetic (iat is attacker-controlled; a plain
    //     add could overflow-panic in debug builds — a per-request DoS). Saturation only tightens.
    let iat = payload
        .get("iat")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| refuse("DPoP proof missing integer `iat`"))?;
    if iat.saturating_add(window_secs) < now || iat.saturating_sub(window_secs) > now {
        return Err(refuse(format!(
            "DPoP proof `iat`={iat} is outside the ±{window_secs}s freshness window (now={now})"
        )));
    }

    // (5) SINGLE-USE — consume the proof `jti`; a replay (same jti) is refused.
    let jti = payload
        .get("jti")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| refuse("DPoP proof missing `jti`"))?;
    if !replay.consume(jti) {
        return Err(refuse(format!(
            "DPoP proof `jti` `{jti}` was already presented (replay defence)"
        )));
    }
    Ok(())
}

// ================================================================================================
// The TokenVerifier — the real machine-token verifier (the authenticate path).
// ================================================================================================

/// The "now" source in Unix seconds — injected for deterministic tests / the drill (the production
/// default reads the system clock).
type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// **The REAL machine/capability-token verifier (MR-011) — plugs into the [`crate::machine_auth::
/// TokenVerifier`] seam.** Verifies the PASETO v4.public Ed25519 signature against the injected cell
/// public key, recomputes the macaroon caveat chain (rejecting any amplification), checks the signed
/// `exp` (a numeric instant), and — for a DPoP-bound PAT — verifies the DPoP proof against the bound
/// `cnf.jkt` + the injected request binding + freshness + replay. Returns a trust-rooted
/// [`CapabilityToken`] (tenant/authority from the VERIFIED token only) or refuses LOUDLY.
#[derive(Clone)]
pub struct PasetoCapabilityVerifier {
    anchor: CellTrustAnchor,
    now: NowFn,
    /// The injected per-request DPoP binding (`htm`/`htu`). `None` ⇒ no request bound (a DPoP-bound
    /// token then cannot be honoured — fail-closed — since the request context is absent).
    binding: Option<DpopBinding>,
    dpop_window_secs: i64,
    replay: DpopReplayGuard,
}

impl PasetoCapabilityVerifier {
    /// Build the verifier over the injected cell trust anchor (public key + macaroon secret), with the
    /// system clock, a fresh DPoP replay guard, a 60s DPoP freshness window, and NO request binding
    /// (set one with [`Self::with_request_binding`] to honour DPoP-bound PATs).
    pub fn new(anchor: CellTrustAnchor) -> PasetoCapabilityVerifier {
        PasetoCapabilityVerifier {
            anchor,
            now: Arc::new(system_now),
            binding: None,
            dpop_window_secs: 60,
            replay: DpopReplayGuard::new(),
        }
    }

    /// Inject the per-request DPoP binding (`htm`/`htu`) — the method + canonical URL of THIS request.
    pub fn with_request_binding(mut self, binding: DpopBinding) -> PasetoCapabilityVerifier {
        self.binding = Some(binding);
        self
    }

    /// Inject a deterministic clock (Unix seconds) — the test / drill seam.
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> PasetoCapabilityVerifier {
        self.now = Arc::new(now);
        self
    }

    /// Share an explicit DPoP replay guard (so several verifier handles share one seen-set).
    pub fn with_replay_guard(mut self, replay: DpopReplayGuard) -> PasetoCapabilityVerifier {
        self.replay = replay;
        self
    }

    /// Override the DPoP freshness window (seconds; default 60).
    pub fn with_dpop_window(mut self, secs: i64) -> PasetoCapabilityVerifier {
        self.dpop_window_secs = secs;
        self
    }

    /// The core verification, shared by the [`crate::machine_auth::TokenVerifier`] impl. Separated so
    /// it can be unit-tested directly with a `MachineKind` (the trait reads the kind from the
    /// credential scheme, exactly as the structural floor does).
    pub fn verify_material(
        &self,
        material: &str,
        kind: MachineKind,
    ) -> myelin_identity::Result<CapabilityToken> {
        let parsed = ParsedMaterial::parse(material)?;

        // (1) PASETO v4.public Ed25519 verify against the INJECTED cell public key (never the token).
        let (claims_bytes, sig) = paseto_v4_public_verify(&self.anchor.public_key, &parsed.paseto)?;
        let claims: serde_json::Value = serde_json::from_slice(&claims_bytes)
            .map_err(|e| AuthzError::BadRequest(format!("malformed verified claims JSON: {e}")))?;

        // (2) Only NOW (signature proven) do we trust the body. Tenant/region/sub/jti are read from the
        //     VERIFIED claims only — never caller-supplied (ID-3, the IDOR floor).
        let tenant = str_claim(&claims, "tenant")?;
        let region = str_claim(&claims, "region")?;
        let subject_key = str_claim(&claims, "sub")?;
        let jti = str_claim(&claims, "jti")?;
        let exp = claims
            .get("exp")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| refuse("verified token missing integer `exp`"))?;
        let root_grants: BTreeSet<String> = match claims.get("auth") {
            Some(serde_json::Value::Array(arr)) => {
                arr.iter().filter_map(|g| g.as_str().map(str::to_string)).collect()
            }
            None => BTreeSet::new(),
            _ => return Err(AuthzError::BadRequest("`auth` claim must be an array".into())),
        };
        let bound_jkt = claims
            .get("cnf")
            .and_then(|c| c.get("jkt"))
            .and_then(|j| j.as_str())
            .map(str::to_string);

        // (3) EXPIRY — `exp` is a numeric Unix instant (no lexical-compare hazard by construction). A
        //     token at/after its exp is refused.
        let now = (self.now)();
        if exp <= now {
            return Err(refuse(format!(
                "capability token expired: exp={exp} <= now={now}"
            )));
        }

        // (4) THE MACAROON CHAIN — recompute `tag_0` from the cell secret + the root signature, then
        //     fold each caveat. Each caveat MUST be a subset of the running effective set (a caveat
        //     naming a grant the parent lacks is an AMPLIFICATION attempt → refused), and the final
        //     tag MUST equal the presented tail (a forged/removed/reordered caveat fails the HMAC).
        let mut tag = macaroon_root_tag(&self.anchor.mac_key, &sig);
        let mut effective = root_grants.clone();
        for caveat in &parsed.caveats {
            for g in caveat {
                if !effective.contains(g) {
                    return Err(refuse(format!(
                        "amplified caveat: grant `{g}` is not held by the parent authority — a caveat \
                         may only NARROW (monotone attenuation), never widen — refused"
                    )));
                }
            }
            effective = effective.intersection(caveat).cloned().collect();
            tag = macaroon_fold(&tag, caveat);
        }
        // Constant-time compare of the recomputed tag against the presented tail.
        if !ct_eq(&parsed.tail, &tag) {
            return Err(refuse(
                "macaroon caveat-chain tag mismatch — the caveat chain was forged, reordered, or a \
                 caveat was removed (the chain is bound under the cell secret) — refused",
            ));
        }

        // (5) DPoP — a token bound to a client key (`cnf.jkt` present) MUST come with a valid proof.
        //     A bound token with NO proof, a proof by the WRONG key, a replayed proof, a wrong
        //     htm/htu, or a stale iat is refused. An unbound token (no cnf) carries no proof.
        let dpop_bound = match (&bound_jkt, &parsed.dpop) {
            (Some(jkt), Some(proof)) => {
                let binding = self.binding.as_ref().ok_or_else(|| {
                    refuse(
                        "a DPoP-bound token requires a request binding (htm/htu) to verify the proof \
                         against — none injected — fail-closed",
                    )
                })?;
                verify_dpop_proof(
                    proof,
                    jkt,
                    binding,
                    now,
                    self.dpop_window_secs,
                    &self.replay,
                )?;
                true
            }
            (Some(_), None) => {
                return Err(refuse(
                    "a DPoP-bound token (cnf.jkt present) was presented WITHOUT a DPoP proof — a \
                     bearer-only presentation of a sender-constrained token is refused (RFC 9449)",
                ))
            }
            (None, Some(_)) => {
                // A proof on an unbound token is meaningless — refuse rather than silently ignore it
                // (it signals confusion / a downgrade attempt).
                return Err(refuse(
                    "a DPoP proof was presented for a token that carries no `cnf.jkt` binding — refused",
                ))
            }
            (None, None) => false,
        };

        Ok(CapabilityToken {
            tenant: TenantId(tenant),
            region: Region(region),
            kind,
            subject_key,
            authority: Authority::of(effective),
            jti,
            dpop_bound,
        })
    }
}

impl crate::machine_auth::TokenVerifier for PasetoCapabilityVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<CapabilityToken> {
        // The kind is read from the credential scheme (the SAME posture as the structural floor — the
        // five machine schemes map to a MachineKind; a human/SSO scheme is refused here loudly).
        let kind = MachineKind::from_scheme(&credential.scheme).ok_or_else(|| {
            AuthzError::BadRequest(format!(
                "scheme `{}` is not a capability-token / machine-identity surface \
                 (pat/ci/agent/deploy_key/per_job)",
                credential.scheme
            ))
        })?;
        self.verify_material(&credential.material, kind)
    }
}

/// Read a non-empty string claim from the VERIFIED body (else a loud refusal — never a fabricated
/// tenant/subject).
fn str_claim(claims: &serde_json::Value, key: &str) -> Result<String, AuthzError> {
    claims
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| refuse(format!("verified token missing/empty `{key}` claim")))
}

/// Constant-time equality (the `subtle`-class compare for the macaroon tag — never short-circuits on
/// the first differing byte).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ================================================================================================
// The TokenSigner — the real per-run-token signer (the mint path).
// ================================================================================================

/// **The REAL per-run-token signer (MR-011) — plugs into the [`crate::mint::TokenSigner`] seam.** The
/// mint applies the monotone intersection and hands this signer the ALREADY-attenuated effective
/// authority; the signer stamps a PASETO v4.public token (Ed25519) with `exp = now + ttl` (a per-run
/// token is TTL-constrained, not DPoP-bound → no `cnf`), seeds the macaroon chain, and returns the
/// credential material a [`PasetoCapabilityVerifier`] round-trips. The mint's intersection + scope +
/// TTL logic is unchanged (the signer never widens the authority it is given).
#[derive(Clone)]
pub struct PasetoCapabilitySigner {
    authority: Arc<CellTokenAuthority>,
    now: NowFn,
    ttl_secs: i64,
}

impl PasetoCapabilitySigner {
    /// Build over the cell's token authority, with the system clock and a per-run-token `exp` of
    /// `now + ttl_secs` (the run-life ceiling the mint ALSO registers in the durable S7 store).
    pub fn new(authority: Arc<CellTokenAuthority>, ttl_secs: i64) -> PasetoCapabilitySigner {
        PasetoCapabilitySigner {
            authority,
            now: Arc::new(system_now),
            ttl_secs,
        }
    }

    /// Inject a deterministic clock (Unix seconds) — the test / drill seam.
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> PasetoCapabilitySigner {
        self.now = Arc::new(now);
        self
    }
}

impl crate::mint::TokenSigner for PasetoCapabilitySigner {
    fn sign(&self, tenant: &str, region: &str, subject_key: &str, jti: &str, grants: &[&str]) -> String {
        let exp = (self.now)().saturating_add(self.ttl_secs);
        self.authority.mint(&CapabilityMintSpec {
            tenant: tenant.to_string(),
            region: region.to_string(),
            subject_key: subject_key.to_string(),
            jti: jti.to_string(),
            exp_unix: exp,
            authority: grants.iter().map(|g| g.to_string()).collect(),
            dpop_jkt: None, // a per-run token is TTL-constrained, not DPoP-bound (§4).
        })
    }
}

/// A registry of cell trust anchors keyed by a cell id — a thin forward-looking carrier for the
/// multi-cell / key-rotation layer (a single anchor is the MR-011 floor; rotation is named-deferred).
/// Kept tiny so the wiring point exists without over-building.
#[derive(Clone, Default)]
pub struct CellAnchorSet {
    by_cell: BTreeMap<String, CellTrustAnchor>,
}

impl CellAnchorSet {
    /// An empty anchor set.
    pub fn new() -> CellAnchorSet {
        CellAnchorSet {
            by_cell: BTreeMap::new(),
        }
    }
    /// Register `anchor` under `cell_id` (builder form).
    pub fn with_anchor(mut self, cell_id: impl Into<String>, anchor: CellTrustAnchor) -> CellAnchorSet {
        self.by_cell.insert(cell_id.into(), anchor);
        self
    }
    /// The anchor for `cell_id`, if registered.
    pub fn get(&self, cell_id: &str) -> Option<&CellTrustAnchor> {
        self.by_cell.get(cell_id)
    }
    /// How many anchors are registered.
    pub fn len(&self) -> usize {
        self.by_cell.len()
    }
    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.by_cell.is_empty()
    }
}

#[cfg(test)]
#[path = "capability_crypto_tests.rs"]
mod tests;
