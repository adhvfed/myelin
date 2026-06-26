//! # `webauthn` — REAL WebAuthn / FIDO2 passkey credential verification (MR-010c; the passkey slice
//! of P-526, census SI-001/004). The LAST of the four human/SSO credential types.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §4 (the authentication
//! surfaces — WebAuthn/FIDO2 passkeys; **tenant is taken from the verified credential, never the URL
//! path**, ID-3).
//!
//! ## What this module replaces (the #1 CRITICAL census finding, for the passkey scheme)
//! The production auth graph's floor verifier ([`crate::authenticate::StructuralVerifier`]) parses a
//! PLAINTEXT `<tenant>|<region>|<subject_key>` envelope — so ANYONE forges any principal in any
//! tenant (SI-001/004). [`WebauthnVerifier`] is the REAL cryptographic replacement for the **passkey**
//! scheme: it verifies a WebAuthn **assertion** (the login flow) against the registered COSE public
//! key with VETTED primitives, runs the full challenge / origin / RP-ID / User-Present / signature-
//! counter defence set, and extracts a trust-rooted [`VerifiedAssertion`] from the REGISTERED credential
//! binding — or refuses it LOUDLY. It plugs into the EXISTING [`CredentialVerifier`] seam — the
//! resolution + telemetry body in [`crate::authenticate`] does not change. It is the sibling of
//! [`crate::oidc::OidcVerifier`] (MR-010a) / [`crate::ssh_auth::SshVerifier`] (MR-010d) /
//! [`crate::saml::SamlVerifier`] (MR-010b) — same seam, same rigor (`verify` is TOTAL over attacker
//! bytes — no slice/`unwrap`/overflow panic; every malformed input is a loud [`AuthzError`]).
//!
//! ## STEP-1 dependency decision (vetted pure-Rust CBOR; reuse the existing signature crypto)
//! WebAuthn needs **CBOR** (the `attestationObject`, and the COSE-encoded credential public key) and
//! **COSE** key parsing. No CBOR/COSE crate was in `Cargo.lock`. We add **`ciborium`** — a small,
//! well-maintained, **pure-Rust** CBOR codec (no C deps); it pulls only pure-Rust crates
//! (`ciborium-io` / `ciborium-ll` / `half` / `crunchy`). We do NOT hand-roll the signature crypto: the
//! COSE signature is verified with the SAME vetted primitives the OIDC path already pulls —
//! - **ES256** (COSE alg −7; ECDSA P-256 / SHA-256, **ASN.1-DER** sig — WebAuthn uses DER, not the
//!   fixed `r‖s` of a JWT) — `ring`'s `ECDSA_P256_SHA256_ASN1`.
//! - **RS256** (COSE alg −257; RSA PKCS#1 v1.5 / SHA-256) — `rsa` + `sha2`.
//! - **EdDSA** (COSE alg −8; Ed25519) — `ring`'s `ED25519`.
//!
//! The CBOR is parsed with `ciborium::value::Value` (`from_reader`, which is total — a malformed /
//! truncated / garbage blob returns `Err`, never a panic); the **binary `authenticatorData`** is parsed
//! by a bounds-checked, `checked_add` reader ([`AuthData`]) — the SSH panic-safety lesson.
//!
//! ## The two flows
//! ### 1. Assertion / login (the MAIN path — the [`CredentialVerifier::verify`] body)
//! Given `clientDataJSON` + `authenticatorData` + `signature` + the credential id, for a credential
//! whose COSE public key was registered:
//! - parse `clientDataJSON`: `type == "webauthn.get"`, the `challenge` matches a server-issued,
//!   **single-use** challenge ([`ChallengeGuard`] — replay defence), the `origin` is in the configured
//!   **allowlist** (exact match), and `crossOrigin` (if present) is `false` unless configured otherwise;
//! - parse `authenticatorData`: `rpIdHash == SHA256(configured RP ID)`, the **User-Present** (UP) flag
//!   set (+ **User-Verified** UV if required), and the signature **counter** strictly greater than the
//!   stored counter (clone/replay detection) — a regression is refused, unless the authenticator uses 0;
//! - verify the **signature** over `authenticatorData ‖ SHA256(clientDataJSON)` with the registered COSE
//!   key (alg **pinned from the stored key** — alg-confusion refused);
//! - tenant / region / subject (the credential id) come from the **registered binding only** (ID-3).
//!
//! ### 2. Registration / attestation ([`WebauthnVerifier::register`])
//! Given the `attestationObject` (CBOR: `fmt` + `authData` + `attStmt`) + `clientDataJSON`
//! (`type == "webauthn.create"`, challenge, origin): parse the attestedCredentialData, extract + store
//! the COSE public key + credential id, and verify the attestation statement for:
//! - **`none`** — no attestation; the key is extracted + stored.
//! - **`packed` (self attestation, no `x5c`)** — the `attStmt.sig` over `authData ‖ SHA256(clientDataJSON)`
//!   is verified against the credential's **own** COSE key, `attStmt.alg` pinned to that key (a self-
//!   attestation forgery is refused).
//!
//! ## Attestation formats supported vs DEFERRED (honest scope — not faked)
//! - **Supported (real crypto):** `none`, `packed` **self** attestation.
//! - **DEFERRED, refused LOUDLY (never faked):** `packed` **full** attestation (`x5c` present — the
//!   X.509 attestation-cert chain to a configured root), and `tpm` / `android-key` / `android-safetynet`
//!   / `apple` / `fido-u2f`. There is no pure-Rust X.509 chain verifier in `Cargo.lock`; a full-chain
//!   verifier would add a proc-macro X.509 stack (`x509-cert` + `der_derive` + `tls_codec*` + …) for the
//!   least load-bearing slice, and **faking** chain verification would be a net security regression — so
//!   a `packed`-`x5c` (or other-format) attestation is **refused** as unsupported, never silently
//!   accepted. The assertion path (the security-critical main flow) and self/`none` attestation are
//!   FULLY real. Passkey-sync governance + hardware-attested device-binding depth (tpm/apple/android) are
//!   deferred + named here.
//!
//! ## What is INJECTED, and what is honestly out of scope
//! Both the [`ChallengeGuard`] and the [`CredentialBindingIndex`] (the S1 passkey→principal binding) are
//! **injected** — the crypto path makes NO network call, so unit/integration tests drive the REAL code
//! path deterministically. The challenge issuance/consumption is a **thin in-process layer**: the real
//! wiring (issuing the challenge on the `/webauthn/assert` handshake, a Redis/Valkey-class bound seen-set,
//! and populating the binding index from S1 on registration) lands with the web UI/API; what this module
//! ships — and proves — is the load-bearing verification. The runtime challenge-issuance binding is NOT
//! claimed here.
//!
//! ## Wiring (the dispatch seam — [`crate::oidc::SchemeDispatchVerifier`])
//! [`WebauthnVerifier`] is wired as the `passkey`-scheme verifier via
//! `SchemeDispatchVerifier::route(scheme::PASSKEY, …)` (exercised in the tests). The dispatcher
//! constructs NO `Structural*` type itself (the fallback is injected by the caller), so it adds no
//! mock-crypto construction to the production graph; removing the `StructuralVerifier` prod default
//! entirely is MR-012.

use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ciborium::value::Value as Cbor;
use sha2::{Digest, Sha256};

/// A LOUD refusal of a credential that is well-formed but does NOT verify (forged/invalid signature,
/// unregistered credential, replayed/unknown challenge, wrong origin/RP, UP clear, counter regression,
/// alg confusion, unsupported attestation). It is an `AuthzError::FailClosed` so an unverifiable
/// credential NEVER resolves to a Principal (the assertion is never fabricated/partial).
fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

/// A LOUD structural refusal — the bytes are not even a well-formed WebAuthn credential (bad base64,
/// bad JSON envelope, truncated/garbage CBOR or authenticatorData). `AuthzError::BadRequest`.
fn malformed(msg: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(msg.into())
}

// ================================================================================================
// authenticatorData flag bits (WebAuthn §6.1).
// ================================================================================================

/// User-Present (UP) — bit 0. The user interacted with the authenticator (touched the key / sensor).
const FLAG_UP: u8 = 0x01;
/// User-Verified (UV) — bit 2. The user was verified (PIN / biometric).
const FLAG_UV: u8 = 0x04;
/// Attested-credential-data (AT) — bit 6. `authData` carries attestedCredentialData (registration).
const FLAG_AT: u8 = 0x40;
/// Extension-data (ED) — bit 7. `authData` carries a trailing CBOR extensions map (we skip it safely).
const FLAG_ED: u8 = 0x80;

// ================================================================================================
// COSE public key (RFC 8152) — the registered credential key, family-pinned (the alg-confusion pin).
// ================================================================================================

/// A parsed COSE_Key — the credential public key, with the ONE COSE `alg` it supports pinned by the
/// family (the alg-confusion pin: the verify primitive is chosen from the STORED key, never from a
/// caller-supplied alg).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoseKey {
    /// ES256 (COSE alg −7) — EC2 / P-256, affine coordinates `x`,`y` (32 bytes each).
    Es256 {
        /// The `x` coordinate (32 bytes).
        x: Vec<u8>,
        /// The `y` coordinate (32 bytes).
        y: Vec<u8>,
    },
    /// RS256 (COSE alg −257) — RSA, modulus `n` and exponent `e` (big-endian bytes).
    Rs256 {
        /// RSA modulus `n` (big-endian).
        n: Vec<u8>,
        /// RSA public exponent `e` (big-endian).
        e: Vec<u8>,
    },
    /// EdDSA (COSE alg −8) — OKP / Ed25519, the 32-byte public key.
    Ed25519 {
        /// The Ed25519 public key (32 bytes).
        x: Vec<u8>,
    },
}

impl CoseKey {
    /// The COSE `alg` this key supports (the value the attestation statement / a presented signature is
    /// pinned to — chosen from the KEY, never from attacker input).
    fn cose_alg(&self) -> i128 {
        match self {
            CoseKey::Es256 { .. } => -7,
            CoseKey::Rs256 { .. } => -257,
            CoseKey::Ed25519 { .. } => -8,
        }
    }
}

/// Read a CBOR map value by **integer** label (COSE keys are integer-labelled). Total — a non-map or a
/// missing label is `None`, never a panic.
fn cbor_map_int<'a>(map: &'a [(Cbor, Cbor)], label: i128) -> Option<&'a Cbor> {
    map.iter().find_map(|(k, v)| match k {
        Cbor::Integer(i) if i128::from(*i) == label => Some(v),
        _ => None,
    })
}

/// Read a CBOR map value by **text** label (the attestationObject keys `fmt`/`authData`/`attStmt` are
/// text-labelled). Total.
fn cbor_map_text<'a>(map: &'a [(Cbor, Cbor)], label: &str) -> Option<&'a Cbor> {
    map.iter().find_map(|(k, v)| match k {
        Cbor::Text(t) if t == label => Some(v),
        _ => None,
    })
}

/// Extract a CBOR byte-string, or a loud structural refusal.
fn cbor_bytes(v: &Cbor, what: &str) -> Result<Vec<u8>, AuthzError> {
    match v {
        Cbor::Bytes(b) => Ok(b.clone()),
        _ => Err(malformed(format!("CBOR `{what}` is not a byte string"))),
    }
}

/// Extract a CBOR integer as `i128`, or a loud structural refusal.
fn cbor_int(v: &Cbor, what: &str) -> Result<i128, AuthzError> {
    match v {
        Cbor::Integer(i) => Ok(i128::from(*i)),
        _ => Err(malformed(format!("CBOR `{what}` is not an integer"))),
    }
}

/// Parse a COSE_Key (CBOR map) into a family-pinned [`CoseKey`]. The `kty`(1)/`alg`(3) MUST be
/// consistent (an EC2 key must declare ES256, etc. — the alg-confusion pin is established at parse), and
/// the curve / coordinate lengths are validated. Total over attacker bytes (malformed → loud refusal).
fn parse_cose_key(map: &[(Cbor, Cbor)]) -> Result<CoseKey, AuthzError> {
    // kty (label 1): 2 = EC2, 1 = OKP, 3 = RSA.
    let kty = cbor_int(
        cbor_map_int(map, 1).ok_or_else(|| malformed("COSE key missing `kty` (label 1)"))?,
        "kty",
    )?;
    // alg (label 3): the declared signature algorithm — must match the key family (alg-confusion pin).
    let alg = cbor_int(
        cbor_map_int(map, 3).ok_or_else(|| malformed("COSE key missing `alg` (label 3)"))?,
        "alg",
    )?;
    match kty {
        // EC2 → ES256.
        2 => {
            if alg != -7 {
                return Err(refuse(format!(
                    "COSE EC2 key declares alg {alg} (expected −7 / ES256 — alg-confusion pin)"
                )));
            }
            // crv (label −1) must be P-256 (1).
            let crv = cbor_int(
                cbor_map_int(map, -1).ok_or_else(|| malformed("COSE EC2 key missing `crv`"))?,
                "crv",
            )?;
            if crv != 1 {
                return Err(refuse(format!(
                    "COSE EC2 key curve {crv} unsupported (only P-256 / crv 1 / ES256)"
                )));
            }
            let x = cbor_bytes(
                cbor_map_int(map, -2).ok_or_else(|| malformed("COSE EC2 key missing `x`"))?,
                "x",
            )?;
            let y = cbor_bytes(
                cbor_map_int(map, -3).ok_or_else(|| malformed("COSE EC2 key missing `y`"))?,
                "y",
            )?;
            if x.len() != 32 || y.len() != 32 {
                return Err(malformed(format!(
                    "COSE EC2 P-256 coordinates must be 32 bytes (x={}, y={})",
                    x.len(),
                    y.len()
                )));
            }
            Ok(CoseKey::Es256 { x, y })
        }
        // OKP → Ed25519 (EdDSA).
        1 => {
            if alg != -8 {
                return Err(refuse(format!(
                    "COSE OKP key declares alg {alg} (expected −8 / EdDSA — alg-confusion pin)"
                )));
            }
            let crv = cbor_int(
                cbor_map_int(map, -1).ok_or_else(|| malformed("COSE OKP key missing `crv`"))?,
                "crv",
            )?;
            if crv != 6 {
                return Err(refuse(format!(
                    "COSE OKP key curve {crv} unsupported (only Ed25519 / crv 6)"
                )));
            }
            let x = cbor_bytes(
                cbor_map_int(map, -2).ok_or_else(|| malformed("COSE OKP key missing `x`"))?,
                "x",
            )?;
            if x.len() != 32 {
                return Err(malformed(format!(
                    "COSE Ed25519 public key must be 32 bytes (got {})",
                    x.len()
                )));
            }
            Ok(CoseKey::Ed25519 { x })
        }
        // RSA → RS256.
        3 => {
            if alg != -257 {
                return Err(refuse(format!(
                    "COSE RSA key declares alg {alg} (expected −257 / RS256 — alg-confusion pin)"
                )));
            }
            let n = cbor_bytes(
                cbor_map_int(map, -1).ok_or_else(|| malformed("COSE RSA key missing `n`"))?,
                "n",
            )?;
            let e = cbor_bytes(
                cbor_map_int(map, -2).ok_or_else(|| malformed("COSE RSA key missing `e`"))?,
                "e",
            )?;
            Ok(CoseKey::Rs256 { n, e })
        }
        other => Err(refuse(format!(
            "unsupported COSE key type kty={other} (only EC2/ES256, OKP/Ed25519, RSA/RS256)"
        ))),
    }
}

/// The minimum accepted RSA modulus size, in bits (the same factorability floor the SSH path enforces —
/// a weaker modulus is offline-factorable, after which a forger mints a "valid" signature without the
/// owner's private key).
const MIN_RSA_MODULUS_BITS: u64 = 2048;

/// Verify a COSE signature over `msg` with the registered key, using the vetted crate for the key's
/// family. The verify primitive is chosen from the STORED key (alg-confusion pin); NO signature math is
/// hand-rolled. A WebAuthn ES256 signature is **ASN.1 DER** (not the JWT fixed `r‖s`).
fn verify_cose_signature(key: &CoseKey, msg: &[u8], sig: &[u8]) -> Result<(), AuthzError> {
    match key {
        CoseKey::Es256 { x, y } => {
            use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1};
            if x.len() != 32 || y.len() != 32 {
                return Err(refuse("invalid P-256 coordinates (expected 32 bytes each)"));
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
                .verify(msg, sig)
                .map_err(|_| refuse("ES256 signature verification failed"))
        }
        CoseKey::Rs256 { n, e } => {
            use rsa::pkcs1v15::{Signature, VerifyingKey};
            use rsa::signature::Verifier;
            use rsa::{BigUint, RsaPublicKey};
            let n_int = BigUint::from_bytes_be(n);
            let bits = n_int.bits() as u64;
            if bits < MIN_RSA_MODULUS_BITS {
                return Err(refuse(format!(
                    "RSA key too small: {bits} bits, minimum {MIN_RSA_MODULUS_BITS} (factorable \
                     offline — fail-closed)"
                )));
            }
            let pubkey = RsaPublicKey::new(n_int, BigUint::from_bytes_be(e))
                .map_err(|err| refuse(format!("invalid RSA COSE key: {err}")))?;
            let vk = VerifyingKey::<Sha256>::new(pubkey);
            let signature =
                Signature::try_from(sig).map_err(|_| refuse("malformed RS256 signature encoding"))?;
            vk.verify(msg, &signature)
                .map_err(|_| refuse("RS256 signature verification failed"))
        }
        CoseKey::Ed25519 { x } => {
            use ring::signature::{UnparsedPublicKey, ED25519};
            UnparsedPublicKey::new(&ED25519, x.clone())
                .verify(msg, sig)
                .map_err(|_| refuse("EdDSA signature verification failed"))
        }
    }
}

// ================================================================================================
// authenticatorData — the binary structure (bounds-checked, total over attacker bytes).
// ================================================================================================

/// A parsed `authenticatorData` (WebAuthn §6.1): `rpIdHash[32] ‖ flags[1] ‖ signCount[4] ‖ …`. For a
/// registration (`AT` flag set) the trailing attestedCredentialData carries the credential id + COSE key.
#[derive(Clone, Debug)]
struct AuthData {
    rp_id_hash: [u8; 32],
    flags: u8,
    sign_count: u32,
    /// `(credential_id, cose_key)` — present iff the `AT` flag was set and parsed (registration).
    attested: Option<(Vec<u8>, CoseKey)>,
}

impl AuthData {
    fn up(&self) -> bool {
        self.flags & FLAG_UP != 0
    }
    fn uv(&self) -> bool {
        self.flags & FLAG_UV != 0
    }

    /// Parse `authenticatorData`, bounds-checked + total (truncated / huge-length / garbage → loud
    /// refusal, NEVER a panic — the SSH panic-safety lesson). The attestedCredentialData + the trailing
    /// COSE key (when `AT` is set) and the optional extensions map (when `ED` is set) are parsed via the
    /// total `ciborium` reader.
    fn parse(buf: &[u8]) -> Result<AuthData, AuthzError> {
        // rpIdHash[32] ‖ flags[1] ‖ signCount[4] = 37 fixed bytes.
        let head = buf
            .get(..37)
            .ok_or_else(|| malformed("authenticatorData truncated (need ≥37 bytes)"))?;
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&head[..32]);
        let flags = head[32];
        let sign_count = u32::from_be_bytes([head[33], head[34], head[35], head[36]]);

        let mut attested = None;
        let mut pos = 37usize;
        if flags & FLAG_AT != 0 {
            // attestedCredentialData: aaguid[16] ‖ credIdLen[2] ‖ credId[credIdLen] ‖ COSE key (CBOR).
            let aaguid_end = pos
                .checked_add(16)
                .ok_or_else(|| malformed("authData: aaguid offset overflow"))?;
            buf.get(pos..aaguid_end)
                .ok_or_else(|| malformed("authData: truncated aaguid"))?;
            pos = aaguid_end;
            let len_end = pos
                .checked_add(2)
                .ok_or_else(|| malformed("authData: credIdLen offset overflow"))?;
            let len_bytes = buf
                .get(pos..len_end)
                .ok_or_else(|| malformed("authData: truncated credIdLen"))?;
            let cred_id_len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
            pos = len_end;
            let cred_end = pos
                .checked_add(cred_id_len)
                .ok_or_else(|| malformed("authData: credId length offset overflow"))?;
            let cred_id = buf
                .get(pos..cred_end)
                .ok_or_else(|| malformed("authData: credId runs past the buffer (truncated)"))?
                .to_vec();
            pos = cred_end;
            // The COSE public key is the next CBOR item. `ciborium::from_reader` reads exactly one item
            // off the cursor (total — malformed CBOR is an Err, never a panic). The reader advances; we
            // recover how many bytes it consumed so the optional extensions map can follow.
            let rest = buf
                .get(pos..)
                .ok_or_else(|| malformed("authData: no COSE key after credId"))?;
            let mut cursor = std::io::Cursor::new(rest);
            let key_value: Cbor = ciborium::from_reader(&mut cursor)
                .map_err(|e| malformed(format!("authData: malformed COSE key CBOR: {e}")))?;
            let consumed = cursor.position() as usize;
            pos = pos
                .checked_add(consumed)
                .ok_or_else(|| malformed("authData: COSE key length offset overflow"))?;
            let key_map = match &key_value {
                Cbor::Map(m) => m.as_slice(),
                _ => return Err(malformed("authData: COSE key is not a CBOR map")),
            };
            attested = Some((cred_id, parse_cose_key(key_map)?));
        }
        if flags & FLAG_ED != 0 {
            // Skip the trailing extensions CBOR map (total parse; we do not act on extensions here).
            let rest = buf
                .get(pos..)
                .ok_or_else(|| malformed("authData: ED flag set but no extensions follow"))?;
            let mut cursor = std::io::Cursor::new(rest);
            let _ext: Cbor = ciborium::from_reader(&mut cursor)
                .map_err(|e| malformed(format!("authData: malformed extensions CBOR: {e}")))?;
            pos = pos.saturating_add(cursor.position() as usize);
        }
        // Trailing data (beyond what the structure accounts for) is a structural refusal — a well-formed
        // authData is fully consumed.
        if pos != buf.len() {
            return Err(malformed("authenticatorData has trailing data"));
        }
        Ok(AuthData {
            rp_id_hash,
            flags,
            sign_count,
            attested,
        })
    }
}

// ================================================================================================
// clientDataJSON — the browser-collected client data (parsed via serde_json::Value; total).
// ================================================================================================

/// The fields we read from `clientDataJSON` (WebAuthn §5.8.1). Unknown fields (e.g. an injected
/// `tenant`) are IGNORED — the tenant is NEVER read from the wrapper (ID-3, the tenant-injection
/// defence); it comes only from the registered binding.
struct ClientData {
    type_: String,
    challenge: String,
    origin: String,
    cross_origin: bool,
}

impl ClientData {
    /// Parse `clientDataJSON` from raw bytes (total; malformed JSON / a missing field is a loud
    /// structural refusal).
    fn parse(raw: &[u8]) -> Result<ClientData, AuthzError> {
        let v: serde_json::Value = serde_json::from_slice(raw)
            .map_err(|e| malformed(format!("malformed clientDataJSON: {e}")))?;
        let field = |name: &str| -> Result<String, AuthzError> {
            v.get(name)
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .ok_or_else(|| malformed(format!("clientDataJSON missing `{name}`")))
        };
        Ok(ClientData {
            type_: field("type")?,
            challenge: field("challenge")?,
            origin: field("origin")?,
            // `crossOrigin` is optional; absent ⇒ false (same-origin).
            cross_origin: v
                .get("crossOrigin")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
        })
    }
}

// ================================================================================================
// The challenge store — single-use, time-bounded freshness (replay defence).
// ================================================================================================

type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
struct StoredChallenge {
    expires_at: i64,
    consumed: bool,
}

/// **The challenge store — single-use + time-bounded freshness (the replay defence, CRITICAL).** The
/// server issues a fresh random challenge ([`ChallengeGuard::issue`]); the verifier [`consume`]s it
/// exactly once **by its base64url value** (the value the browser echoes into `clientDataJSON.challenge`).
/// A SECOND consume of the same challenge is rejected (replay); a challenge that was never issued is
/// rejected (unknown); a consume after the expiry bound is rejected (stale). Cloneable (shared inner
/// map) so the issuing side and the verifier consult ONE seen-set. The clock is injected (testable
/// across the expiry boundary). The nonce is generated with `ring`'s CSPRNG.
///
/// [`consume`]: ChallengeGuard::consume
#[derive(Clone)]
pub struct ChallengeGuard {
    inner: Arc<Mutex<BTreeMap<String, StoredChallenge>>>,
    ttl_secs: i64,
    now: NowFn,
}

impl ChallengeGuard {
    /// A fresh store whose challenges live `ttl_secs` seconds, on the system clock.
    pub fn new(ttl_secs: i64) -> ChallengeGuard {
        ChallengeGuard {
            inner: Arc::new(Mutex::new(BTreeMap::new())),
            ttl_secs,
            now: Arc::new(system_now),
        }
    }

    /// Build with an injected clock (Unix seconds) — the deterministic-test seam (advance it past the
    /// TTL to prove expiry).
    pub fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> ChallengeGuard {
        self.now = Arc::new(now);
        self
    }

    fn now(&self) -> i64 {
        (self.now)()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, StoredChallenge>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **Issue a fresh, random, single-use, time-bounded challenge.** Returns the base64url (no-pad)
    /// challenge string the browser embeds in `clientDataJSON.challenge`. The 32-byte challenge is
    /// CSPRNG-generated (`ring`); the entry is stored unconsumed with `expires_at = now + ttl`.
    pub fn issue(&self) -> Result<String, AuthzError> {
        let challenge = URL_SAFE_NO_PAD.encode(random_bytes(32)?);
        let expires_at = self.now().saturating_add(self.ttl_secs);
        self.lock().insert(
            challenge.clone(),
            StoredChallenge {
                expires_at,
                consumed: false,
            },
        );
        Ok(challenge)
    }

    /// Pre-seed an EXPLICIT challenge value (the test/wiring seam — a deterministic challenge). Returns
    /// the `expires_at` it was stored with.
    pub fn issue_explicit(&self, challenge: impl Into<String>) -> i64 {
        let expires_at = self.now().saturating_add(self.ttl_secs);
        self.lock().insert(
            challenge.into(),
            StoredChallenge {
                expires_at,
                consumed: false,
            },
        );
        expires_at
    }

    /// **Consume the challenge `value` ONCE.** The single-use + freshness gate: an UNKNOWN challenge
    /// (never server-issued), an EXPIRED challenge, or an ALREADY-CONSUMED challenge (a replay) is a loud
    /// refusal. On success the entry is marked consumed (a later consume is a replay).
    pub fn consume(&self, value: &str) -> Result<(), AuthzError> {
        let now = self.now();
        let mut map = self.lock();
        let entry = map.get_mut(value).ok_or_else(|| {
            refuse("unknown WebAuthn challenge (not server-issued, or already expired)")
        })?;
        if now > entry.expires_at {
            map.remove(value);
            return Err(refuse(
                "expired WebAuthn challenge (stale — re-issue a fresh challenge)",
            ));
        }
        if entry.consumed {
            return Err(refuse(
                "replayed WebAuthn challenge (this assertion was already presented — replay defence)",
            ));
        }
        entry.consumed = true;
        Ok(())
    }
}

/// CSPRNG bytes via `ring`'s `SystemRandom`. An RNG failure is a loud `Unavailable` (never a panic / a
/// predictable challenge).
fn random_bytes(n: usize) -> Result<Vec<u8>, AuthzError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf)
        .map_err(|_| AuthzError::Unavailable("CSPRNG failure issuing WebAuthn challenge".into()))?;
    Ok(buf)
}

// ================================================================================================
// The credential→principal binding index (S1 passkey registry, injected, with the counter).
// ================================================================================================

/// The trust-rooted facts a REGISTERED passkey binds to + its mutable signature counter (clone
/// detection). `tenant`/`region`/`subject_key` come ONLY from here — never from the credential wrapper
/// (ID-3, the tenant-injection defence). The `subject_key` is the base64url credential id.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredCredential {
    cose_key: CoseKey,
    tenant: TenantId,
    region: Region,
    subject_key: String,
    sign_count: u32,
}

/// **The injected passkey→principal binding index (the S1 SSO-link, the credential registry).** Maps a
/// credential id (raw bytes) to its registered COSE key + binding + signature counter. The verifier
/// consults it on assertion (an UNREGISTERED credential is refused), and the registration flow
/// (`WebauthnVerifier::register`) writes into it. In tests it is built directly; a live deployment
/// populates it from the S1 store when a user registers a passkey.
#[derive(Clone, Default)]
pub struct CredentialBindingIndex {
    inner: Arc<Mutex<BTreeMap<Vec<u8>, StoredCredential>>>,
}

impl CredentialBindingIndex {
    /// An empty registry.
    pub fn new() -> CredentialBindingIndex {
        CredentialBindingIndex {
            inner: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<Vec<u8>, StoredCredential>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The number of registered credentials.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// The stored signature counter for a credential id, if registered (for the drill assertion that the
    /// counter advanced after a successful assertion).
    pub fn sign_count(&self, credential_id: &[u8]) -> Option<u32> {
        self.lock().get(credential_id).map(|c| c.sign_count)
    }

    /// Insert/overwrite a binding (used by `register`). The `initial_count` is the registration-time
    /// `signCount`.
    fn put(
        &self,
        credential_id: Vec<u8>,
        cose_key: CoseKey,
        tenant: TenantId,
        region: Region,
        subject_key: String,
        initial_count: u32,
    ) {
        self.lock().insert(
            credential_id,
            StoredCredential {
                cose_key,
                tenant,
                region,
                subject_key,
                sign_count: initial_count,
            },
        );
    }
}

// ================================================================================================
// Configuration.
// ================================================================================================

/// The relying-party (RP) configuration the verifier validates against — the RP ID (whose SHA-256 must
/// equal the `authenticatorData.rpIdHash`), the **exact-match origin allowlist**, whether User-
/// Verification is required, and whether a cross-origin assertion is accepted.
#[derive(Clone, Debug)]
pub struct WebauthnConfig {
    /// The configured RP ID (e.g. `"example.com"`); `SHA256(rp_id)` must equal the `rpIdHash`.
    pub rp_id: String,
    /// The exact-match origin allowlist (e.g. `"https://example.com"`); the `clientDataJSON.origin` MUST
    /// be one of these (exact string match — a wrong/look-alike origin is refused).
    pub origins: BTreeSet<String>,
    /// Require the User-Verified (UV) flag (PIN/biometric). Default `false` (UP is always required).
    pub require_user_verification: bool,
    /// Accept an assertion whose `clientDataJSON.crossOrigin` is `true`. Default `false` (a cross-origin
    /// assertion — e.g. inside a foreign iframe — is refused).
    pub allow_cross_origin: bool,
}

impl WebauthnConfig {
    /// A config for `rp_id` + the given origin allowlist, with UV optional and cross-origin refused.
    pub fn new(
        rp_id: impl Into<String>,
        origins: impl IntoIterator<Item = impl Into<String>>,
    ) -> WebauthnConfig {
        WebauthnConfig {
            rp_id: rp_id.into(),
            origins: origins.into_iter().map(Into::into).collect(),
            require_user_verification: false,
            allow_cross_origin: false,
        }
    }

    /// Require the User-Verified flag (builder form).
    pub fn requiring_user_verification(mut self) -> WebauthnConfig {
        self.require_user_verification = true;
        self
    }

    /// The configured RP ID hash (`SHA256(rp_id)`).
    fn rp_id_hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(self.rp_id.as_bytes());
        h.finalize().into()
    }

    /// Validate `clientDataJSON` for `expected_type` ("webauthn.get" / "webauthn.create") + consume the
    /// challenge + check the origin allowlist + cross-origin policy. Returns the parsed client data on
    /// success. (Shared by the assertion and registration flows so the type/challenge/origin policy
    /// cannot drift between them.)
    fn validate_client_data(
        &self,
        raw_client_data: &[u8],
        expected_type: &str,
        challenges: &ChallengeGuard,
    ) -> Result<ClientData, AuthzError> {
        let cd = ClientData::parse(raw_client_data)?;
        if cd.type_ != expected_type {
            return Err(refuse(format!(
                "clientDataJSON.type `{}` != expected `{expected_type}`",
                cd.type_
            )));
        }
        // ORIGIN ALLOWLIST — exact match. A wrong / look-alike origin is refused BEFORE consuming the
        // challenge would matter; we consume after the cheap string checks.
        if !self.origins.contains(&cd.origin) {
            return Err(refuse(format!(
                "clientDataJSON.origin `{}` is not in the configured allowlist",
                cd.origin
            )));
        }
        if cd.cross_origin && !self.allow_cross_origin {
            return Err(refuse(
                "clientDataJSON.crossOrigin is true (a cross-origin assertion is refused)",
            ));
        }
        // CHALLENGE — single-use + freshness (replay defence). Consumed exactly once by its value.
        challenges.consume(&cd.challenge)?;
        Ok(cd)
    }
}

// ================================================================================================
// The credential envelope (the client → server WebAuthn wire shape).
// ================================================================================================

/// Encode the assertion `material` the client presents (the login flow): the base64 credential id,
/// `clientDataJSON`, `authenticatorData`, and signature. PUBLIC so the real web client (and the tests)
/// build the SAME shape the verifier parses. The verifier reads tenant/region from the REGISTERED
/// binding — the envelope carries NO tenant (the tenant-injection defence is structural).
pub fn encode_assertion_material(
    credential_id: &[u8],
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
) -> String {
    serde_json::json!({
        "credential_id": B64.encode(credential_id),
        "client_data_json": B64.encode(client_data_json),
        "authenticator_data": B64.encode(authenticator_data),
        "signature": B64.encode(signature),
    })
    .to_string()
}

/// Encode the registration `material` the client presents (the attestation flow): the base64
/// `clientDataJSON` + `attestationObject` (CBOR). PUBLIC so the real web client (and tests) build the
/// SAME shape. The credential id + COSE key are inside the `attestationObject.authData`.
pub fn encode_registration_material(client_data_json: &[u8], attestation_object: &[u8]) -> String {
    serde_json::json!({
        "client_data_json": B64.encode(client_data_json),
        "attestation_object": B64.encode(attestation_object),
    })
    .to_string()
}

/// Read a base64 field from a parsed JSON envelope (a missing/non-string/non-base64 field is a loud
/// structural refusal). Unknown sibling fields are ignored (tenant-injection defence).
fn env_b64(v: &serde_json::Value, name: &str) -> Result<Vec<u8>, AuthzError> {
    let s = v
        .get(name)
        .and_then(|x| x.as_str())
        .ok_or_else(|| malformed(format!("WebAuthn envelope missing `{name}`")))?;
    B64.decode(s.as_bytes())
        .map_err(|e| malformed(format!("WebAuthn envelope `{name}` is not valid base64: {e}")))
}

// ================================================================================================
// The verifier.
// ================================================================================================

/// **The REAL WebAuthn / FIDO2 passkey credential verifier (MR-010c).** [`CredentialVerifier::verify`]
/// runs the **assertion** (login) flow; [`WebauthnVerifier::register`] runs the **registration**
/// (attestation) flow. Both consult the injected [`ChallengeGuard`] (single-use challenge) and
/// [`CredentialBindingIndex`] (the registered COSE key + binding + counter), with the full
/// origin / RP-ID / UP / counter / alg-pin defence set. `verify` is TOTAL over attacker bytes (no
/// panic). Plugs into the existing [`CredentialVerifier`] seam; the [`crate::authenticate`] resolution
/// + telemetry body does not change.
#[derive(Clone)]
pub struct WebauthnVerifier {
    config: WebauthnConfig,
    registry: CredentialBindingIndex,
    challenges: ChallengeGuard,
}

impl WebauthnVerifier {
    /// Build the verifier over an injected RP config + credential registry + challenge store. Wire it as
    /// the `passkey`-scheme verifier via `SchemeDispatchVerifier::route(scheme::PASSKEY, …)`.
    pub fn new(
        config: WebauthnConfig,
        registry: CredentialBindingIndex,
        challenges: ChallengeGuard,
    ) -> WebauthnVerifier {
        WebauthnVerifier {
            config,
            registry,
            challenges,
        }
    }

    /// The shared challenge store (so the issuing side / a caller can issue a fresh challenge).
    pub fn challenges(&self) -> &ChallengeGuard {
        &self.challenges
    }

    /// The injected credential registry (so a caller can inspect the stored counter / bindings).
    pub fn registry(&self) -> &CredentialBindingIndex {
        &self.registry
    }

    /// **Registration / attestation (WebAuthn §7.1) — extract + store the credential public key.** Given
    /// the registration `material` (`clientDataJSON` `type == "webauthn.create"` + the `attestationObject`)
    /// and the server-known `tenant`/`region` the user is registering under (INJECTED — never read from
    /// the credential), verify the attestation statement and, on success, store the COSE key + binding +
    /// initial counter under the extracted credential id. Returns the credential id (the `subject_key`).
    ///
    /// Supported formats: `none`, `packed` **self** (real crypto). `packed` **full** (`x5c`) and
    /// tpm/android/apple/u2f are REFUSED-as-unsupported (deferred; see the module docs).
    pub fn register(
        &self,
        material: &str,
        tenant: &TenantId,
        region: &Region,
    ) -> myelin_identity::Result<Vec<u8>> {
        let env: serde_json::Value = serde_json::from_str(material.trim())
            .map_err(|e| malformed(format!("malformed WebAuthn registration envelope JSON: {e}")))?;
        let raw_client_data = env_b64(&env, "client_data_json")?;
        let attestation_object = env_b64(&env, "attestation_object")?;

        // (1) clientDataJSON — type == webauthn.create, origin allowlisted, challenge consumed.
        let _cd =
            self.config
                .validate_client_data(&raw_client_data, "webauthn.create", &self.challenges)?;
        let client_data_hash = Sha256::digest(&raw_client_data);

        // (2) attestationObject CBOR → { fmt, authData, attStmt }. Total parse (malformed → refusal).
        let att: Cbor = ciborium::from_reader(std::io::Cursor::new(&attestation_object))
            .map_err(|e| malformed(format!("malformed attestationObject CBOR: {e}")))?;
        let att_map = match &att {
            Cbor::Map(m) => m.as_slice(),
            _ => return Err(malformed("attestationObject is not a CBOR map")),
        };
        let fmt = match cbor_map_text(att_map, "fmt") {
            Some(Cbor::Text(t)) => t.clone(),
            _ => return Err(malformed("attestationObject missing `fmt`")),
        };
        let auth_data_bytes = cbor_bytes(
            cbor_map_text(att_map, "authData")
                .ok_or_else(|| malformed("attestationObject missing `authData`"))?,
            "authData",
        )?;

        // (3) authData — must carry attestedCredentialData (AT flag) with the COSE key + credential id,
        //     and its rpIdHash must match the configured RP; UP (and UV if required) must be set.
        let auth_data = AuthData::parse(&auth_data_bytes)?;
        if auth_data.rp_id_hash != self.config.rp_id_hash() {
            return Err(refuse(
                "registration rpIdHash != SHA256(configured RP ID) (wrong relying party)",
            ));
        }
        if !auth_data.up() {
            return Err(refuse("registration User-Present (UP) flag is not set"));
        }
        if self.config.require_user_verification && !auth_data.uv() {
            return Err(refuse(
                "registration User-Verified (UV) flag required but not set",
            ));
        }
        let (cred_id, cose_key) = auth_data.attested.clone().ok_or_else(|| {
            refuse("registration authData carries no attestedCredentialData (AT flag clear)")
        })?;

        // (4) Attestation statement — verify per format (or refuse-as-unsupported, loudly).
        let att_stmt = cbor_map_text(att_map, "attStmt");
        match fmt.as_str() {
            "none" => {
                // No attestation: the attStmt must be an empty map. The key is extracted + stored as-is.
                if let Some(Cbor::Map(m)) = att_stmt {
                    if !m.is_empty() {
                        return Err(refuse("`none` attestation must carry an empty attStmt"));
                    }
                }
            }
            "packed" => {
                let stmt = match att_stmt {
                    Some(Cbor::Map(m)) => m.as_slice(),
                    _ => return Err(malformed("packed attestation missing attStmt map")),
                };
                // FULL attestation (x5c present) is DEFERRED — refuse loudly, never silently accept an
                // un-chained attestation cert (see the module-level scope note).
                if cbor_map_text(stmt, "x5c").is_some() {
                    return Err(refuse(
                        "packed FULL attestation (x5c) is not supported yet — the X.509 attestation-cert \
                         chain-to-root verification is deferred (refused, never faked). Use `none` or \
                         packed self attestation.",
                    ));
                }
                // SELF attestation: alg + sig over (authData ‖ clientDataHash), verified against the
                // credential's OWN COSE key. alg must equal the key's COSE alg (alg-confusion pin).
                let alg = cbor_int(
                    cbor_map_text(stmt, "alg")
                        .ok_or_else(|| malformed("packed attStmt missing `alg`"))?,
                    "alg",
                )?;
                let sig = cbor_bytes(
                    cbor_map_text(stmt, "sig")
                        .ok_or_else(|| malformed("packed attStmt missing `sig`"))?,
                    "sig",
                )?;
                if alg != cose_key.cose_alg() {
                    return Err(refuse(format!(
                        "packed self-attestation alg {alg} != the credential key's COSE alg {} \
                         (alg-confusion pin)",
                        cose_key.cose_alg()
                    )));
                }
                let mut signed = Vec::with_capacity(auth_data_bytes.len() + client_data_hash.len());
                signed.extend_from_slice(&auth_data_bytes);
                signed.extend_from_slice(&client_data_hash);
                verify_cose_signature(&cose_key, &signed, &sig).map_err(|_| {
                    refuse("packed self-attestation signature verification failed")
                })?;
            }
            other => {
                return Err(refuse(format!(
                    "attestation format `{other}` is not supported (only `none` + packed self; \
                     tpm/android-key/android-safetynet/apple/fido-u2f are deferred — refused, not faked)"
                )));
            }
        }

        // (5) Store the binding. subject_key = the base64url credential id (the S1 SSO-link key); the
        //     initial counter is the registration-time signCount (so a later assertion must exceed it,
        //     or both be 0). tenant/region are the SERVER-supplied registration context (never the wire).
        let subject_key = URL_SAFE_NO_PAD.encode(&cred_id);
        self.registry.put(
            cred_id.clone(),
            cose_key,
            tenant.clone(),
            region.clone(),
            subject_key,
            auth_data.sign_count,
        );
        Ok(cred_id)
    }
}

impl CredentialVerifier for WebauthnVerifier {
    /// **Assertion / login (WebAuthn §7.2) — the main path.** Verify a presented assertion against the
    /// registered COSE key, run the full challenge/origin/RP-ID/UP/counter defence set, and resolve the
    /// trust-rooted [`VerifiedAssertion`] from the registered binding — or refuse loudly.
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        // This verifier owns ONLY the passkey scheme; another scheme is a wiring error.
        if credential.scheme != scheme::PASSKEY {
            return Err(malformed(format!(
                "WebauthnVerifier received a `{}` credential (expected `passkey`)",
                credential.scheme
            )));
        }

        // (1) Parse the assertion envelope (base64 credential id + clientDataJSON + authData + signature).
        let env: serde_json::Value = serde_json::from_str(credential.material.trim())
            .map_err(|e| malformed(format!("malformed WebAuthn assertion envelope JSON: {e}")))?;
        let credential_id = env_b64(&env, "credential_id")?;
        let raw_client_data = env_b64(&env, "client_data_json")?;
        let authenticator_data = env_b64(&env, "authenticator_data")?;
        let signature = env_b64(&env, "signature")?;

        // (2) CREDENTIAL BINDING — the presented credential id MUST be REGISTERED. An unregistered
        //     credential is refused (no fabricated principal). The trust root (tenant/region/subject) is
        //     read from THIS binding — never the wrapper (ID-3). Looked up first so an unregistered probe
        //     does not burn a challenge. We clone the snapshot we need and drop the lock before the
        //     crypto, then re-acquire to update the counter (so the lock is never held across verify).
        let stored = self
            .registry
            .lock()
            .get(&credential_id)
            .cloned()
            .ok_or_else(|| {
                refuse(
                    "unregistered passkey credential id (no S1 binding — fail-closed, never a \
                     fabricated principal)",
                )
            })?;

        // (3) clientDataJSON — type == webauthn.get, origin allowlisted, crossOrigin policy, and the
        //     single-use challenge consumed (replay defence).
        let _cd =
            self.config
                .validate_client_data(&raw_client_data, "webauthn.get", &self.challenges)?;

        // (4) authenticatorData — rpIdHash matches the configured RP, UP set (UV if required).
        let auth_data = AuthData::parse(&authenticator_data)?;
        if auth_data.rp_id_hash != self.config.rp_id_hash() {
            return Err(refuse(
                "assertion rpIdHash != SHA256(configured RP ID) (wrong relying party)",
            ));
        }
        if !auth_data.up() {
            return Err(refuse(
                "User-Present (UP) flag is not set (the user was not present)",
            ));
        }
        if self.config.require_user_verification && !auth_data.uv() {
            return Err(refuse("User-Verified (UV) flag required but not set"));
        }

        // (5) SIGNATURE — over `authenticatorData ‖ SHA256(clientDataJSON)` with the REGISTERED COSE key
        //     (the verify primitive pinned from the stored key — alg confusion refused). A forged
        //     signature (a different key), or a signature over different bytes than presented, fails here.
        let client_data_hash = Sha256::digest(&raw_client_data);
        let mut signed = Vec::with_capacity(authenticator_data.len() + client_data_hash.len());
        signed.extend_from_slice(&authenticator_data);
        signed.extend_from_slice(&client_data_hash);
        verify_cose_signature(&stored.cose_key, &signed, &signature)?;

        // (6) SIGNATURE COUNTER — clone/replay detection. After a VALID signature, the presented count
        //     must strictly exceed the stored count, UNLESS the authenticator uses 0 (both 0 ⇒ no counter
        //     support, accepted). A regression (presented ≤ stored, not both 0) signals a cloned
        //     authenticator → refuse. Updated only on an otherwise-valid assertion (so a failed sig
        //     cannot burn the counter).
        let presented = auth_data.sign_count;
        let stored_count = stored.sign_count;
        if presented == 0 && stored_count == 0 {
            // The authenticator does not implement a counter — no clone signal available; accept.
        } else if presented > stored_count {
            // Advance the stored counter.
            if let Some(entry) = self.registry.lock().get_mut(&credential_id) {
                entry.sign_count = presented;
            }
        } else {
            return Err(refuse(format!(
                "signature counter regression: presented {presented} ≤ stored {stored_count} \
                 (cloned authenticator / replay — fail-closed)"
            )));
        }

        // (7) THE TRUST-ROOTED ASSERTION — tenant/region/subject come ONLY from the registered binding.
        Ok(VerifiedAssertion {
            tenant: stored.tenant.clone(),
            region: stored.region.clone(),
            scheme: scheme::PASSKEY.to_string(),
            subject_key: stored.subject_key.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticate::StructuralVerifier;
    use crate::oidc::SchemeDispatchVerifier;
    use ciborium::value::Integer;
    use std::sync::atomic::{AtomicI64, Ordering};

    // ── Test harness constants ───────────────────────────────────────────────────────────────────
    const RP_ID: &str = "example.com";
    const ORIGIN: &str = "https://example.com";
    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

    // ── CBOR encoding helpers (test side — build REAL COSE keys / attestationObjects) ────────────
    fn ci(n: i64) -> Cbor {
        Cbor::Integer(Integer::from(n))
    }
    fn cbytes(b: &[u8]) -> Cbor {
        Cbor::Bytes(b.to_vec())
    }
    fn encode_cbor(v: &Cbor) -> Vec<u8> {
        let mut buf = Vec::new();
        ciborium::into_writer(v, &mut buf).expect("cbor encode");
        buf
    }

    // ── The three authenticator key kinds (REAL keys + REAL signatures) ──────────────────────────
    //
    // Each produces (a) a COSE_Key CBOR blob (the credential public key the authenticator registers)
    // and (b) a real signature over `authData ‖ SHA256(clientDataJSON)`. The verifier only ever sees
    // the PUBLIC half (via the stored COSE key); the private key never leaves the test.
    enum AuthKey {
        Es256(EcKey),
        Rs256(RsaKey),
        Ed25519(EdKey),
    }
    impl AuthKey {
        fn cose_key_cbor(&self) -> Vec<u8> {
            match self {
                AuthKey::Es256(k) => k.cose_key_cbor(),
                AuthKey::Rs256(k) => k.cose_key_cbor(),
                AuthKey::Ed25519(k) => k.cose_key_cbor(),
            }
        }
        fn cose_alg(&self) -> i64 {
            match self {
                AuthKey::Es256(_) => -7,
                AuthKey::Rs256(_) => -257,
                AuthKey::Ed25519(_) => -8,
            }
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            match self {
                AuthKey::Es256(k) => k.sign(msg),
                AuthKey::Rs256(k) => k.sign(msg),
                AuthKey::Ed25519(k) => k.sign(msg),
            }
        }
    }

    struct EcKey {
        pair: ring::signature::EcdsaKeyPair,
        rng: ring::rand::SystemRandom,
    }
    impl EcKey {
        fn generate() -> EcKey {
            use ring::rand::SystemRandom;
            use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .expect("ec keygen");
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .expect("ec from pkcs8");
            EcKey { pair, rng }
        }
        fn cose_key_cbor(&self) -> Vec<u8> {
            use ring::signature::KeyPair;
            let pt = self.pair.public_key().as_ref(); // 0x04 ‖ x ‖ y (65 bytes)
            assert_eq!(pt.len(), 65);
            // COSE EC2 / ES256: { 1:2, 3:-7, -1:1, -2:x, -3:y }
            encode_cbor(&Cbor::Map(vec![
                (ci(1), ci(2)),
                (ci(3), ci(-7)),
                (ci(-1), ci(1)),
                (ci(-2), cbytes(&pt[1..33])),
                (ci(-3), cbytes(&pt[33..65])),
            ]))
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            self.pair.sign(&self.rng, msg).expect("ec sign").as_ref().to_vec()
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
        fn cose_key_cbor(&self) -> Vec<u8> {
            use rsa::traits::PublicKeyParts;
            let pubk = self.priv_key.to_public_key();
            // COSE RSA / RS256: { 1:3, 3:-257, -1:n, -2:e }
            encode_cbor(&Cbor::Map(vec![
                (ci(1), ci(3)),
                (ci(3), ci(-257)),
                (ci(-1), cbytes(&pubk.n().to_bytes_be())),
                (ci(-2), cbytes(&pubk.e().to_bytes_be())),
            ]))
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            use rsa::pkcs1v15::SigningKey;
            use rsa::signature::{SignatureEncoding, Signer};
            let sk = SigningKey::<Sha256>::new(self.priv_key.clone());
            sk.sign(msg).to_vec()
        }
    }

    struct EdKey {
        pair: ring::signature::Ed25519KeyPair,
        public: Vec<u8>,
    }
    impl EdKey {
        fn generate() -> EdKey {
            use ring::rand::SystemRandom;
            use ring::signature::{Ed25519KeyPair, KeyPair};
            let rng = SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("ed keygen");
            let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("ed from pkcs8");
            let public = pair.public_key().as_ref().to_vec();
            EdKey { pair, public }
        }
        fn cose_key_cbor(&self) -> Vec<u8> {
            // COSE OKP / EdDSA: { 1:1, 3:-8, -1:6, -2:x }
            encode_cbor(&Cbor::Map(vec![
                (ci(1), ci(1)),
                (ci(3), ci(-8)),
                (ci(-1), ci(6)),
                (ci(-2), cbytes(&self.public)),
            ]))
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            self.pair.sign(msg).as_ref().to_vec()
        }
    }

    // ── authenticatorData + clientDataJSON + attestationObject builders ──────────────────────────

    fn rp_id_hash(rp: &str) -> [u8; 32] {
        Sha256::digest(rp.as_bytes()).into()
    }

    /// Build an assertion `authenticatorData` (no attestedCredentialData): rpIdHash ‖ flags ‖ count.
    fn assertion_auth_data(rp: &str, flags: u8, count: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&rp_id_hash(rp));
        b.push(flags);
        b.extend_from_slice(&count.to_be_bytes());
        b
    }

    /// Build a registration `authenticatorData` (AT flag set + attestedCredentialData): rpIdHash ‖ flags
    /// ‖ count ‖ aaguid[16] ‖ credIdLen[2] ‖ credId ‖ COSE key.
    fn registration_auth_data(
        rp: &str,
        flags: u8,
        count: u32,
        cred_id: &[u8],
        cose_key_cbor: &[u8],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&rp_id_hash(rp));
        b.push(flags | FLAG_AT);
        b.extend_from_slice(&count.to_be_bytes());
        b.extend_from_slice(&[0u8; 16]); // aaguid
        b.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
        b.extend_from_slice(cred_id);
        b.extend_from_slice(cose_key_cbor);
        b
    }

    fn client_data(type_: &str, challenge: &str, origin: &str) -> Vec<u8> {
        serde_json::json!({
            "type": type_,
            "challenge": challenge,
            "origin": origin,
        })
        .to_string()
        .into_bytes()
    }

    /// Build a `none`-format attestationObject with the given authData (the simplest registration).
    fn attestation_object_none(auth_data: &[u8]) -> Vec<u8> {
        encode_cbor(&Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), cbytes(auth_data)),
        ]))
    }

    /// Build a `packed` SELF-attestation attestationObject (sig over authData ‖ clientDataHash with the
    /// credential's own key, `alg` pinned to the key).
    fn attestation_object_packed_self(
        auth_data: &[u8],
        client_data_hash: &[u8],
        key: &AuthKey,
    ) -> Vec<u8> {
        let mut signed = Vec::new();
        signed.extend_from_slice(auth_data);
        signed.extend_from_slice(client_data_hash);
        let sig = key.sign(&signed);
        encode_cbor(&Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("packed".into())),
            (
                Cbor::Text("attStmt".into()),
                Cbor::Map(vec![
                    (Cbor::Text("alg".into()), ci(key.cose_alg())),
                    (Cbor::Text("sig".into()), cbytes(&sig)),
                ]),
            ),
            (Cbor::Text("authData".into()), cbytes(auth_data)),
        ]))
    }

    // ── Verifier construction + the register→assert round-trip helper ────────────────────────────

    fn config() -> WebauthnConfig {
        WebauthnConfig::new(RP_ID, [ORIGIN])
    }

    fn fresh_verifier() -> WebauthnVerifier {
        WebauthnVerifier::new(config(), CredentialBindingIndex::new(), ChallengeGuard::new(300))
    }

    fn cred(material: String) -> Credential {
        Credential {
            scheme: scheme::PASSKEY.into(),
            material,
        }
    }

    /// Register `key` under credential id `cred_id` via `none` attestation with the given initial
    /// `reg_count`, returning the verifier (so subsequent assertions share the binding + challenge store).
    fn registered_none(key: &AuthKey, cred_id: &[u8], reg_count: u32) -> WebauthnVerifier {
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, reg_count, cred_id, &key.cose_key_cbor());
        let material = encode_registration_material(&cd, &attestation_object_none(&ad));
        v.register(&material, &TenantId(TENANT.into()), &Region(REGION.into()))
            .expect("none registration must succeed");
        v
    }

    /// Build a correctly-signed assertion credential for `key`/`cred_id` over a fresh challenge from
    /// `v`, with the given `flags` + `count`.
    fn signed_assertion(
        v: &WebauthnVerifier,
        key: &AuthKey,
        cred_id: &[u8],
        flags: u8,
        count: u32,
    ) -> Credential {
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        let ad = assertion_auth_data(RP_ID, flags, count);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        cred(encode_assertion_material(cred_id, &cd, &ad, &sig))
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // POSITIVE corpus — a correctly-signed ES256 / RS256 / EdDSA assertion each VERIFY and yield the
    // registered principal (tenant/region/subject from the binding).
    // ════════════════════════════════════════════════════════════════════════════════════════════

    fn positive_for(key: AuthKey) {
        let cred_id = b"cred-positive-001";
        let v = registered_none(&key, cred_id, 0);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        let a = v.verify(&c).expect("a correctly-signed assertion must verify");
        assert_eq!(a.tenant, TenantId(TENANT.into()));
        assert_eq!(a.region, Region(REGION.into()));
        assert_eq!(a.scheme, scheme::PASSKEY);
        assert_eq!(a.subject_key, URL_SAFE_NO_PAD.encode(cred_id));
        // The counter advanced 0 → 1.
        assert_eq!(v.registry().sign_count(cred_id), Some(1));
    }

    #[test]
    fn positive_es256_assertion_verifies() {
        positive_for(AuthKey::Es256(EcKey::generate()));
    }
    #[test]
    fn positive_rs256_assertion_verifies() {
        positive_for(AuthKey::Rs256(RsaKey::generate()));
    }
    #[test]
    fn positive_eddsa_assertion_verifies() {
        positive_for(AuthKey::Ed25519(EdKey::generate()));
    }

    /// A `packed` SELF-attestation registration extracts the key, and a subsequent correct assertion
    /// with that key VERIFIES and yields the right principal (the headline registration→login proof).
    #[test]
    fn positive_packed_self_registration_then_assertion() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-packed-self-1";
        let v = fresh_verifier();
        // Register via packed self attestation.
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let att = attestation_object_packed_self(&ad, &Sha256::digest(&cd), &key);
        let material = encode_registration_material(&cd, &att);
        v.register(&material, &TenantId(TENANT.into()), &Region(REGION.into()))
            .expect("packed self registration must succeed");
        assert_eq!(v.registry().len(), 1);
        // A subsequent correct assertion verifies.
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        let a = v.verify(&c).expect("assertion after packed-self registration must verify");
        assert_eq!(a.subject_key, URL_SAFE_NO_PAD.encode(cred_id));
        assert_eq!(a.tenant, TenantId(TENANT.into()));
    }

    /// Counter-less authenticator (both stored and presented 0) is accepted (no clone signal).
    #[test]
    fn positive_zero_counter_authenticator_is_accepted() {
        let key = AuthKey::Ed25519(EdKey::generate());
        let cred_id = b"cred-zero-counter";
        let v = registered_none(&key, cred_id, 0);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 0);
        v.verify(&c).expect("a 0/0 counter assertion must verify");
        assert_eq!(v.registry().sign_count(cred_id), Some(0));
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // NEGATIVE corpus — each forged/invalid assertion MUST be refused (the whole point).
    // ════════════════════════════════════════════════════════════════════════════════════════════

    /// (a) FORGED SIGNATURE — the victim's REGISTERED credential id + public key, but the challenge is
    /// signed by an ATTACKER's key. The signature must fail against the registered key.
    #[test]
    fn negative_forged_signature_by_a_different_key_is_rejected() {
        let victim = AuthKey::Es256(EcKey::generate());
        let attacker = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-forge";
        let v = registered_none(&victim, cred_id, 0);
        // Present the victim's cred id, but sign with the attacker's key.
        let c = signed_assertion(&v, &attacker, cred_id, FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a signature by a different key must be refused, got {err:?}"
        );
    }

    /// (b) REPLAYED CHALLENGE — the SAME assertion presented twice. First verifies; the second (same
    /// single-use challenge) is refused.
    #[test]
    fn negative_replayed_challenge_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-replay";
        let v = registered_none(&key, cred_id, 0);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        v.verify(&c).expect("first presentation verifies");
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "a replayed challenge must be refused, got {err:?}"
        );
    }

    /// (c) UNKNOWN CHALLENGE — a clientDataJSON challenge that was never server-issued is refused.
    #[test]
    fn negative_unknown_challenge_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-unknown-ch";
        let v = registered_none(&key, cred_id, 0);
        // Hand-build an assertion whose challenge is a random string the guard never issued.
        let bogus = URL_SAFE_NO_PAD.encode(b"a-challenge-never-issued-by-the-server");
        let cd = client_data("webauthn.get", &bogus, ORIGIN);
        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        let c = cred(encode_assertion_material(cred_id, &cd, &ad, &sig));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unknown WebAuthn challenge")),
            "an unknown challenge must be refused, got {err:?}"
        );
    }

    /// (d) WRONG ORIGIN — the clientDataJSON origin is not in the configured allowlist.
    #[test]
    fn negative_wrong_origin_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-origin";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
        // A look-alike origin (not in the allowlist).
        let cd = client_data("webauthn.get", &challenge, "https://evil.example.com");
        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        let c = cred(encode_assertion_material(cred_id, &cd, &ad, &sig));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("origin")),
            "a wrong origin must be refused, got {err:?}"
        );
    }

    /// (e) WRONG rpIdHash — authenticatorData built for a DIFFERENT RP ID. Refused.
    #[test]
    fn negative_wrong_rp_id_hash_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-rpid";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        // authData whose rpIdHash is for a different RP.
        let ad = assertion_auth_data("evil.example.com", FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        let c = cred(encode_assertion_material(cred_id, &cd, &ad, &sig));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("rpIdHash")),
            "a wrong rpIdHash must be refused, got {err:?}"
        );
    }

    /// (f) UP FLAG CLEAR — the User-Present bit is not set (the user was not present). Refused.
    #[test]
    fn negative_user_present_flag_clear_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-up";
        let v = registered_none(&key, cred_id, 0);
        // flags = 0 (no UP).
        let c = signed_assertion(&v, &key, cred_id, 0x00, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("User-Present")),
            "a UP-clear assertion must be refused, got {err:?}"
        );
    }

    /// (f') UV REQUIRED BUT CLEAR — a verifier configured to require User-Verification refuses an
    /// assertion whose UV flag is clear (UP set only).
    #[test]
    fn negative_user_verification_required_but_clear_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-uv";
        // A UV-requiring verifier.
        let v = WebauthnVerifier::new(
            config().requiring_user_verification(),
            CredentialBindingIndex::new(),
            ChallengeGuard::new(300),
        );
        // Register (UP+UV+AT so registration passes UV).
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP | FLAG_UV, 0, cred_id, &key.cose_key_cbor());
        v.register(
            &encode_registration_material(&cd, &attestation_object_none(&ad)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        )
        .unwrap();
        // Assert with UP only (no UV) → refused.
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("User-Verified")),
            "a UV-required-but-clear assertion must be refused, got {err:?}"
        );
    }

    /// (g) COUNTER REGRESSION — stored counter 5, presented counter 3 (a clone). A genuine signature,
    /// but the counter regressed → refused (clone detection). Also tests the equal-counter case.
    #[test]
    fn negative_counter_regression_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-counter";
        // Registered with an initial counter of 5.
        let v = registered_none(&key, cred_id, 5);
        // A correctly-signed assertion presenting counter 3 (< 5) — a cloned authenticator.
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 3);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("counter regression")),
            "a counter regression must be refused, got {err:?}"
        );
        // The stored counter did NOT advance (a regression cannot move it).
        assert_eq!(v.registry().sign_count(cred_id), Some(5));
        // The EQUAL-counter case (presented == stored) is also a clone signal → refused.
        let c_eq = signed_assertion(&v, &key, cred_id, FLAG_UP, 5);
        let err = v.verify(&c_eq).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("counter regression")),
            "an equal counter must be refused, got {err:?}"
        );
    }

    /// (h) SIGNATURE OVER DIFFERENT authData THAN PRESENTED — a VALID signature, but over a DIFFERENT
    /// authenticatorData than the one presented. The verifier checks the sig over the PRESENTED bytes →
    /// mismatch → refused.
    #[test]
    fn negative_signature_over_different_authdata_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-diff-ad";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        // Sign over authData_A (count 99), but PRESENT authData_B (count 1).
        let ad_a = assertion_auth_data(RP_ID, FLAG_UP, 99);
        let ad_b = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad_a);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        let c = cred(encode_assertion_material(cred_id, &cd, &ad_b, &sig));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a signature over different authData must be refused, got {err:?}"
        );
    }

    /// (i) ALG CONFUSION — the stored key is ES256, but the attacker presents an RS256 signature. The
    /// verify primitive is pinned from the STORED key (ES256/ASN.1), so an RSA signature blob simply
    /// fails ES256 verification — the verifier never switches primitive on attacker input.
    #[test]
    fn negative_alg_confusion_es256_key_rsa_signature_is_rejected() {
        let es = AuthKey::Es256(EcKey::generate());
        let rsa = AuthKey::Rs256(RsaKey::generate());
        let cred_id = b"cred-algconf";
        let v = registered_none(&es, cred_id, 0);
        // A genuine RSA signature over the correct message, presented against the ES256 credential.
        let c = signed_assertion(&v, &rsa, cred_id, FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("ES256 signature verification failed")),
            "an alg-confused (RSA-for-ES256) signature must be refused, got {err:?}"
        );
    }

    /// (j) MALFORMED — garbage envelope / clientDataJSON / authenticatorData / a huge credIdLen CBOR must
    /// be REFUSED, never PANIC. `verify` (assertion) + `register` (attestation) are both total.
    #[test]
    fn negative_malformed_inputs_are_refused_not_panicking() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-malformed";
        let v = registered_none(&key, cred_id, 0);

        // (j1) Non-JSON / empty / missing-field envelopes.
        for bad in ["", "not json", "{}", r#"{"credential_id":"!!!"}"#] {
            let r = v.verify(&cred(bad.to_string()));
            assert!(r.is_err(), "malformed envelope `{bad}` must be refused (not panic)");
        }

        // (j2) A valid envelope whose authenticatorData is truncated (< 37 bytes).
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        let c = cred(encode_assertion_material(cred_id, &cd, b"\x00\x01\x02", b"sig"));
        assert!(v.verify(&c).is_err(), "truncated authData must be refused");

        // (j3) A valid envelope whose clientDataJSON is garbage (not JSON).
        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let c = cred(encode_assertion_material(cred_id, b"\xff\xff not json", &ad, b"sig"));
        assert!(v.verify(&c).is_err(), "garbage clientDataJSON must be refused");

        // (j4) A registration whose authData claims a HUGE credIdLen (length-prefix overrun) — the
        //      bounds-checked reader refuses it, never an out-of-bounds panic.
        let mut ad_huge = Vec::new();
        ad_huge.extend_from_slice(&rp_id_hash(RP_ID));
        ad_huge.push(FLAG_UP | FLAG_AT);
        ad_huge.extend_from_slice(&0u32.to_be_bytes());
        ad_huge.extend_from_slice(&[0u8; 16]); // aaguid
        ad_huge.extend_from_slice(&0xFFFFu16.to_be_bytes()); // credIdLen = 65535, but no bytes follow
        let v2 = fresh_verifier();
        let challenge2 = v2.challenges().issue().unwrap();
        let cd2 = client_data("webauthn.create", &challenge2, ORIGIN);
        let r = v2.register(
            &encode_registration_material(&cd2, &attestation_object_none(&ad_huge)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        assert!(r.is_err(), "a huge credIdLen must be refused (not panic)");

        // (j5) attestationObject that is not CBOR at all.
        let v3 = fresh_verifier();
        let challenge3 = v3.challenges().issue().unwrap();
        let cd3 = client_data("webauthn.create", &challenge3, ORIGIN);
        let r = v3.register(
            &encode_registration_material(&cd3, b"\xff\xff\xff not cbor"),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        assert!(r.is_err(), "garbage attestationObject CBOR must be refused");
    }

    /// (k) TENANT INJECTION — the assertion envelope carries an extra `tenant` field claiming a DIFFERENT
    /// tenant than the registered credential. It is IGNORED — the resolved tenant is the registered
    /// binding's (acme), never the wrapper's (globex).
    #[test]
    fn negative_tenant_injection_in_wrapper_is_ignored() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-tenant-inj";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        // Build the envelope BY HAND, injecting a bogus tenant claim.
        let material = serde_json::json!({
            "credential_id": B64.encode(cred_id),
            "client_data_json": B64.encode(&cd),
            "authenticator_data": B64.encode(&ad),
            "signature": B64.encode(&sig),
            "tenant": "globex",
            "region": "us-east",
        })
        .to_string();
        let a = v.verify(&cred(material)).expect("the assertion itself is valid");
        assert_eq!(
            a.tenant,
            TenantId(TENANT.into()),
            "the resolved tenant is the REGISTERED binding's (acme), never the wrapper's (globex)"
        );
        assert_eq!(a.region, Region(REGION.into()));
    }

    /// UNREGISTERED CREDENTIAL — a perfectly valid self-signed assertion, but the credential id is not in
    /// the registry. Refused (no fabricated principal).
    #[test]
    fn negative_unregistered_credential_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let v = registered_none(&key, b"cred-known", 0);
        // A valid assertion for an UNKNOWN credential id.
        let c = signed_assertion(&v, &key, b"cred-UNKNOWN", FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unregistered passkey")),
            "an unregistered credential must be refused, got {err:?}"
        );
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // REGISTRATION negative corpus.
    // ════════════════════════════════════════════════════════════════════════════════════════════

    /// Unsigned/invalid PACKED SELF attestation — the attStmt sig is by a DIFFERENT key than the
    /// credential's own. Refused (the self-attestation check holds).
    #[test]
    fn negative_registration_invalid_packed_self_attestation_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let attacker = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-bad-att";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        // attStmt sig is by the ATTACKER, not the credential's own key — self-attestation must fail.
        let att = attestation_object_packed_self(&ad, &Sha256::digest(&cd), &attacker);
        let r = v.register(&att_material(&cd, &att), &TenantId(TENANT.into()), &Region(REGION.into()));
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("self-attestation")),
            "an invalid packed self-attestation must be refused, got {err:?}"
        );
        assert_eq!(v.registry().len(), 0, "no binding stored on a failed attestation");
    }

    /// PACKED FULL (x5c present) — the X.509 attestation-cert chain path is DEFERRED; it must be REFUSED
    /// as unsupported, never silently accepted (this stands in for "att cert NOT chaining to root").
    #[test]
    fn negative_registration_packed_full_x5c_is_refused_as_unsupported() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-x5c";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        // A packed attStmt WITH an x5c array (a fake cert blob) — full attestation, deferred.
        let att = encode_cbor(&Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("packed".into())),
            (
                Cbor::Text("attStmt".into()),
                Cbor::Map(vec![
                    (Cbor::Text("alg".into()), ci(key.cose_alg())),
                    (Cbor::Text("sig".into()), cbytes(&sig)),
                    (
                        Cbor::Text("x5c".into()),
                        Cbor::Array(vec![cbytes(b"not-a-real-cert")]),
                    ),
                ]),
            ),
            (Cbor::Text("authData".into()), cbytes(&ad)),
        ]));
        let r = v.register(&att_material(&cd, &att), &TenantId(TENANT.into()), &Region(REGION.into()));
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("FULL attestation")),
            "packed full (x5c) must be refused-as-unsupported, got {err:?}"
        );
    }

    /// Unsupported attestation format (e.g. `tpm`) is REFUSED loudly (deferred, never faked).
    #[test]
    fn negative_registration_unsupported_format_is_refused() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-tpm";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let att = encode_cbor(&Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("tpm".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), cbytes(&ad)),
        ]));
        let r = v.register(&att_material(&cd, &att), &TenantId(TENANT.into()), &Region(REGION.into()));
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("not supported")),
            "an unsupported format must be refused, got {err:?}"
        );
    }

    /// Registration with the WRONG clientDataJSON type (`webauthn.get` instead of `webauthn.create`).
    #[test]
    fn negative_registration_wrong_type_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-wrongtype";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN); // wrong type for registration
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let r = v.register(
            &encode_registration_material(&cd, &attestation_object_none(&ad)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("webauthn.create")),
            "a wrong registration type must be refused, got {err:?}"
        );
    }

    /// Registration with a wrong origin / unknown challenge is rejected.
    #[test]
    fn negative_registration_wrong_origin_and_challenge_are_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-reg-origin";
        // Wrong origin.
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, "https://evil.example.com");
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let r = v.register(
            &encode_registration_material(&cd, &attestation_object_none(&ad)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        assert!(
            matches!(r, Err(AuthzError::FailClosed(m)) if m.contains("origin")),
            "a wrong registration origin must be refused"
        );
        // Unknown challenge.
        let v2 = fresh_verifier();
        let cd2 = client_data("webauthn.create", "never-issued-challenge", ORIGIN);
        let ad2 = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let r2 = v2.register(
            &encode_registration_material(&cd2, &attestation_object_none(&ad2)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        assert!(
            matches!(r2, Err(AuthzError::FailClosed(m)) if m.contains("unknown WebAuthn challenge")),
            "an unknown registration challenge must be refused"
        );
    }

    fn att_material(client_data: &[u8], attestation_object: &[u8]) -> String {
        encode_registration_material(client_data, attestation_object)
    }

    // ── The dispatch seam (wiring WebauthnVerifier as the passkey verifier) ──────────────────────

    /// The dispatcher routes the passkey scheme to the REAL [`WebauthnVerifier`] and everything else to
    /// the injected fallback. A forged passkey assertion hits the real verifier (refused), NOT the floor.
    #[test]
    fn dispatch_routes_passkey_to_real_verifier_and_others_to_fallback() {
        let victim = AuthKey::Es256(EcKey::generate());
        let attacker = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-dispatch";
        let webauthn = registered_none(&victim, cred_id, 0);
        // A forged assertion (attacker key) for the routing check.
        let forged = signed_assertion(&webauthn, &attacker, cred_id, FLAG_UP, 1);

        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::PASSKEY, Arc::new(webauthn));

        // A passkey credential goes through the REAL crypto verifier — the forgery is refused by it.
        assert!(
            dispatch.verify(&forged).is_err(),
            "a forged passkey assertion must hit the real verifier and be refused"
        );

        // A SAML credential (not-yet-real) rides the injected floor fallback (unchanged behaviour).
        let saml = Credential {
            scheme: scheme::SAML.into(),
            material: "acme|eu-west|nameid-1".into(),
        };
        let a = dispatch.verify(&saml).expect("SAML routes to the floor fallback");
        assert_eq!(a.tenant, TenantId("acme".into()));
        assert_eq!(a.scheme, scheme::SAML);
    }

    /// Expiry — a challenge consumed after its TTL bound is refused (the clock is advanced past expiry).
    #[test]
    fn negative_expired_challenge_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-expiry";
        let clock = Arc::new(AtomicI64::new(1_000));
        let c2 = clock.clone();
        let challenges = ChallengeGuard::new(300).with_clock(move || c2.load(Ordering::SeqCst));
        let v = WebauthnVerifier::new(config(), CredentialBindingIndex::new(), challenges);
        // Register at t=1000.
        let challenge = v.challenges().issue().unwrap();
        let cd_reg = client_data("webauthn.create", &v.challenges().issue().unwrap(), ORIGIN);
        let ad_reg = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        v.register(
            &encode_registration_material(&cd_reg, &attestation_object_none(&ad_reg)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        )
        .unwrap();
        // Build an assertion over the FIRST issued challenge, then jump the clock past expiry.
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        let c = cred(encode_assertion_material(cred_id, &cd, &ad, &sig));
        clock.store(2_000, Ordering::SeqCst); // past 1000 + 300
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired challenge must be refused, got {err:?}"
        );
    }
}
