//! # `ssh_auth` — REAL SSH public-key challenge-response credential verification (MR-010d; the SSH
//! slice of P-526, census SI-001/004).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §4 (the authentication
//! surfaces — SSH; **tenant is taken from the verified credential, never the URL path**, ID-3). The
//! SSH key is the credential the **Git smart-transport** authenticates as (then `check` per ref).
//!
//! ## What this module replaces (the #1 CRITICAL census finding, for the SSH scheme)
//! The production auth graph's floor verifier ([`crate::authenticate::StructuralVerifier`]) parses a
//! PLAINTEXT `<tenant>|<region>|<subject_key>` envelope — so ANYONE forges any principal in any
//! tenant (SI-001/004). [`SshVerifier`] is the REAL cryptographic replacement for the **SSH** scheme:
//! it runs an SSH public-key **challenge-response** with VETTED primitives and extracts a trust-rooted
//! [`VerifiedAssertion`] from the REGISTERED key binding, or refuses it LOUDLY. It plugs into the
//! EXISTING [`CredentialVerifier`] seam — the resolution + telemetry body in [`crate::authenticate`]
//! does not change. It is the sibling of [`crate::oidc::OidcVerifier`] (MR-010a) — same seam, same
//! rigor (`verify` is TOTAL over attacker bytes — no slice/`unwrap`/overflow panic; every malformed
//! input is a loud [`AuthzError`]).
//!
//! ## The protocol (SSH public-key auth IS a challenge-response)
//! 1. The server issues a fresh, random, single-use, time-bounded **challenge** (a nonce) — the
//!    [`ChallengeGuard`].
//! 2. The client signs the [`signed_payload`] over that nonce with its SSH **private** key.
//! 3. The verifier: parses the presented SSH **public** key + signature wire blobs; looks the key's
//!    fingerprint up in the REGISTERED [`KeyBindingIndex`] (an unregistered key is refused); **consumes**
//!    the challenge (single-use + freshness — a replay or an expired challenge is refused); verifies
//!    the signature against the registered public key over the SERVER-issued nonce; and returns the
//!    [`VerifiedAssertion`] whose `tenant`/`region`/`subject_key` come ONLY from the registered
//!    binding — never from the credential wrapper (ID-3, the tenant-injection defence).
//!
//! ## The algorithms (vetted crates only — no hand-rolled signature math)
//! - **Ed25519** (`ssh-ed25519`) — verified with `ring`'s Ed25519 (the same primitive the OIDC EdDSA
//!   path uses).
//! - **RSA** with **rsa-sha2-256 / rsa-sha2-512** (the modern SSH RSA sig algs, RFC 8332) — PKCS#1
//!   v1.5 over SHA-256/512, verified with `rsa` + `sha2` (constructed straight from the wire `n`/`e`).
//!
//! ## Algorithm pinning (CRITICAL — the SHA-1 downgrade defence)
//! - **Weak `ssh-rsa` (SHA-1) is REJECTED.** Only `rsa-sha2-256` / `rsa-sha2-512` are accepted for an
//!   RSA key. An attacker cannot downgrade an RSA key to the SHA-1 signature algorithm.
//! - **The signature algorithm must match the key type** (an Ed25519 key takes only an `ssh-ed25519`
//!   signature; an RSA key takes only an `rsa-sha2-*` signature). A cross-type signature is refused.
//! - An **unknown** signature/key algorithm is refused (never coerced).
//!
//! ## The SSH wire format (hand-parsed, bounds-checked, total — no `ssh-key` crate in the lock)
//! The SSH wire format (RFC 4251/4253/8332/8709) is a sequence of length-prefixed **strings** (a
//! 4-byte big-endian length + that many bytes). [`SshReader`] reads it with `get(..)`-bounds-checked
//! slicing and `checked_add` length math, so a truncated / garbage / wrong-field-count / huge-length
//! blob is a loud refusal, NEVER a panic (the OIDC panic-safety lesson — `verify` is total over
//! attacker-controlled bytes).
//!
//! ## What is INJECTED, and what is honestly out of scope
//! Both the [`ChallengeGuard`] and the [`KeyBindingIndex`] (the S1 SSH-key→principal binding index) are
//! **injected** — the crypto path makes NO network call and holds NO global state, so unit/integration
//! tests drive the REAL code path deterministically. The challenge issuance/consumption is a **thin
//! in-process layer**: the real wiring (issuing the challenge on the Git transport handshake, a
//! Redis/Valkey-class bound seen-set) lands with the Git smart-transport prompt; what this module
//! ships — and proves — is the load-bearing verification: real signatures, real forgeries refused,
//! single-use + time-bounded replay defence, and tenant-from-the-registered-binding. The runtime
//! challenge-issuance binding is NOT claimed here.
//!
//! ## Wiring (the dispatch seam — [`crate::oidc::SchemeDispatchVerifier`])
//! [`SshVerifier`] is wired as the `ssh`-scheme verifier via `SchemeDispatchVerifier::route(scheme::SSH,
//! …)` (exercised in the tests). The dispatcher constructs NO `Structural*` type itself (the fallback
//! is injected by the caller), so it adds no mock-crypto construction to the production graph; removing
//! the `StructuralVerifier` prod default entirely is MR-012.

use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use crate::principal_store::{PrincipalError, PrincipalStore};
use myelin_identity::{AuthzError, Credential, PrincipalId};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::{STANDARD as B64, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine as _;

/// A LOUD refusal of a credential that is well-formed but does NOT verify (forged/invalid signature,
/// unregistered key, replayed/expired/unknown challenge, alg downgrade, alg/key mismatch). It is an
/// `AuthzError::FailClosed` so an unverifiable credential NEVER resolves to a Principal (the assertion
/// is never fabricated/partial).
fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

/// A LOUD structural refusal — the bytes are not even a well-formed SSH credential (bad base64, bad
/// JSON envelope, truncated/garbage wire blob, wrong field count). `AuthzError::BadRequest`.
fn malformed(msg: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(msg.into())
}

/// The domain-separation context the challenge nonce is signed under (so an SSH-auth signature can
/// never be replayed as a signature for some OTHER protocol that signs raw bytes, and vice-versa).
const SSH_AUTH_CONTEXT: &[u8] = b"myelin-ssh-auth-challenge-v1\n";

/// The exact bytes the client signs (and the verifier checks the signature over): the
/// domain-separation context followed by the SERVER-issued challenge nonce. The nonce is always taken
/// from the [`ChallengeGuard`] (server-issued), never from the credential.
pub fn signed_payload(nonce: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(SSH_AUTH_CONTEXT.len() + nonce.len());
    m.extend_from_slice(SSH_AUTH_CONTEXT);
    m.extend_from_slice(nonce);
    m
}

// ================================================================================================
// The SSH wire reader — bounds-checked, total over attacker bytes (no panic).
// ================================================================================================

/// A cursor over an SSH wire blob that reads RFC 4251 length-prefixed **strings** with FULLY
/// bounds-checked slicing (`get(..)`) and `checked_add` length math — so a truncated, garbage,
/// wrong-field-count, or huge-length blob is a loud refusal, NEVER a panic (the OIDC panic-safety
/// lesson: `verify` is total over attacker-controlled bytes).
struct SshReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> SshReader<'a> {
    fn new(buf: &'a [u8]) -> SshReader<'a> {
        SshReader { buf, pos: 0 }
    }

    /// Read a 4-byte big-endian length. Out-of-bounds (truncated) is a loud refusal, not a panic.
    fn read_u32(&mut self) -> Result<u32, AuthzError> {
        let end = self
            .pos
            .checked_add(4)
            .ok_or_else(|| malformed("SSH wire: length offset overflow"))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| malformed("SSH wire: truncated (no 4-byte length prefix)"))?;
        self.pos = end;
        Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    /// Read one length-prefixed string. A length that runs past the buffer (the classic truncation /
    /// huge-length attack) is a loud refusal — `get(..)` returns `None`, never an out-of-bounds panic.
    fn read_string(&mut self) -> Result<&'a [u8], AuthzError> {
        let len = self.read_u32()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| malformed("SSH wire: string length offset overflow"))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| malformed("SSH wire: string length runs past the buffer (truncated)"))?;
        self.pos = end;
        Ok(slice)
    }

    /// Bytes not yet consumed. A well-formed blob is fully consumed; trailing data is rejected.
    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
}

// ================================================================================================
// SSH public key + signature wire types.
// ================================================================================================

/// A parsed SSH public key (the family pins the ONE signature-algorithm class it accepts).
#[derive(Clone, Debug, PartialEq, Eq)]
enum SshPublicKey {
    /// Ed25519 — the raw 32-byte public key.
    Ed25519(Vec<u8>),
    /// RSA — the public exponent `e` and modulus `n` (SSH mpint big-endian bytes).
    Rsa { e: Vec<u8>, n: Vec<u8> },
}

/// A presented SSH signature, with its (validated, pinned) algorithm.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SshSignature {
    alg: SigAlg,
    bytes: Vec<u8>,
}

/// The accepted SSH signature algorithms (the SHA-1 `ssh-rsa` is NOT among them — it is rejected at
/// parse time, the alg-downgrade defence).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigAlg {
    /// `ssh-ed25519`.
    Ed25519,
    /// `rsa-sha2-256` (RFC 8332).
    RsaSha256,
    /// `rsa-sha2-512` (RFC 8332).
    RsaSha512,
}

/// Parse an SSH public-key wire blob (`string keytype` then the key params). Bounds-checked + total;
/// trailing data, an unknown key type, or a wrong-length Ed25519 key is a loud refusal.
fn parse_ssh_public_key(blob: &[u8]) -> Result<SshPublicKey, AuthzError> {
    let mut r = SshReader::new(blob);
    let keytype = r.read_string()?;
    let key = match keytype {
        b"ssh-ed25519" => {
            let pk = r.read_string()?;
            if pk.len() != 32 {
                return Err(malformed(format!(
                    "ssh-ed25519 public key must be 32 bytes (got {})",
                    pk.len()
                )));
            }
            SshPublicKey::Ed25519(pk.to_vec())
        }
        // NOTE: the public-key TYPE tag is `ssh-rsa` for ALL RSA keys (RFC 8332 §3 — the SHA-2
        // signature algorithms reuse the `ssh-rsa` key blob); the SHA-1 vs SHA-2 distinction lives in
        // the SIGNATURE algorithm, not the key blob. So `ssh-rsa` is a VALID key type; we reject the
        // weak `ssh-rsa` SIGNATURE algorithm, not the key.
        b"ssh-rsa" => {
            let e = r.read_string()?; // mpint
            let n = r.read_string()?; // mpint
            SshPublicKey::Rsa {
                e: e.to_vec(),
                n: n.to_vec(),
            }
        }
        other => {
            return Err(malformed(format!(
                "unsupported SSH public-key type `{}` (expected ssh-ed25519 or ssh-rsa)",
                String::from_utf8_lossy(other)
            )));
        }
    };
    if r.remaining() != 0 {
        return Err(malformed("SSH public-key blob has trailing data"));
    }
    Ok(key)
}

/// Parse an SSH signature wire blob (`string sigalg` then `string signature`). Bounds-checked +
/// total. The weak SHA-1 `ssh-rsa` algorithm and any unknown algorithm are refused; trailing data is
/// refused.
fn parse_ssh_signature(blob: &[u8]) -> Result<SshSignature, AuthzError> {
    let mut r = SshReader::new(blob);
    let alg_bytes = r.read_string()?;
    let sig = r.read_string()?;
    if r.remaining() != 0 {
        return Err(malformed("SSH signature blob has trailing data"));
    }
    let alg = match alg_bytes {
        b"ssh-ed25519" => SigAlg::Ed25519,
        b"rsa-sha2-256" => SigAlg::RsaSha256,
        b"rsa-sha2-512" => SigAlg::RsaSha512,
        // ALG-DOWNGRADE DEFENCE — the legacy `ssh-rsa` signature algorithm is RSA/SHA-1, which is
        // broken (chosen-prefix collisions). It is refused outright; only rsa-sha2-256/512 are
        // accepted for an RSA key.
        b"ssh-rsa" => {
            return Err(refuse(
                "weak `ssh-rsa` (RSA/SHA-1) signature algorithm rejected — only rsa-sha2-256 / \
                 rsa-sha2-512 are accepted (alg-downgrade defence)",
            ));
        }
        other => {
            return Err(refuse(format!(
                "unknown SSH signature algorithm `{}`",
                String::from_utf8_lossy(other)
            )));
        }
    };
    Ok(SshSignature {
        alg,
        bytes: sig.to_vec(),
    })
}

/// The OpenSSH key **fingerprint** of a public-key wire blob — `SHA256:<base64-no-pad>` over the raw
/// blob bytes (the standard OpenSSH fingerprint the registry keys on). PUBLIC so the registration flow
/// computes the same fingerprint when binding an uploaded key.
pub fn ssh_fingerprint(public_key_blob: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(public_key_blob);
    format!("SHA256:{}", STANDARD_NO_PAD.encode(digest))
}

// ================================================================================================
// Signature verification — vetted primitives only, no hand-rolled signature math.
// ================================================================================================

/// Verify an SSH signature against the presented public key over `msg` (`= signed_payload(nonce)`).
/// The signature algorithm MUST match the key type (an Ed25519 key verifies only an `ssh-ed25519`
/// signature; an RSA key only an `rsa-sha2-*` signature) — a cross-type signature is refused. NO
/// signature math is hand-rolled; each arm calls the vetted crate's `verify`.
fn verify_ssh_signature(
    key: &SshPublicKey,
    sig: &SshSignature,
    msg: &[u8],
) -> Result<(), AuthzError> {
    match (key, sig.alg) {
        (SshPublicKey::Ed25519(pk), SigAlg::Ed25519) => {
            use ring::signature::{UnparsedPublicKey, ED25519};
            // ring validates the signature length internally; a wrong-length sig simply fails to
            // verify (never a panic).
            UnparsedPublicKey::new(&ED25519, pk.as_slice())
                .verify(msg, &sig.bytes)
                .map_err(|_| refuse("ed25519 signature verification failed"))
        }
        (SshPublicKey::Rsa { n, e }, SigAlg::RsaSha256) => verify_rsa_sha256(n, e, msg, &sig.bytes),
        (SshPublicKey::Rsa { n, e }, SigAlg::RsaSha512) => verify_rsa_sha512(n, e, msg, &sig.bytes),
        // The signature algorithm does not match the key type (e.g. an rsa-sha2-* signature presented
        // with an Ed25519 key, or an ssh-ed25519 signature with an RSA key). Refuse.
        _ => Err(refuse(
            "signature algorithm does not match the public-key type (alg/key-type mismatch)",
        )),
    }
}

/// The minimum accepted RSA modulus size, in bits. A smaller key is factorable offline (a 512-bit
/// modulus is trivially factored; ≤1024-bit is within reach of a determined adversary), after which a
/// forger can mint a "valid" signature WITHOUT the owner's private key — a total bypass for that key.
/// So a registered-but-weak RSA key is refused fail-closed, independent of whether the signature
/// itself checks out. (Ed25519 has no such tunable floor — its security level is fixed.)
const MIN_RSA_MODULUS_BITS: u64 = 2048;

/// Build the RSA public key from the SSH wire `n`/`e`, enforcing the [`MIN_RSA_MODULUS_BITS`] floor.
/// A too-small modulus, or a zero/invalid key, is a loud refusal (never a panic). Shared by both the
/// SHA-256 and SHA-512 verify paths so the size floor cannot drift between them.
fn rsa_public_key(n: &[u8], e: &[u8]) -> Result<rsa::RsaPublicKey, AuthzError> {
    use rsa::{BigUint, RsaPublicKey};
    let n_int = BigUint::from_bytes_be(n);
    // KEY-STRENGTH FLOOR — reject a weak (factorable) modulus BEFORE constructing/verifying. `bits()`
    // is the true bit length (leading-zero mpint padding does not inflate it).
    let bits = n_int.bits() as u64;
    if bits < MIN_RSA_MODULUS_BITS {
        return Err(refuse(format!(
            "rsa key too small: {bits} bits, minimum {MIN_RSA_MODULUS_BITS} (a weaker modulus is \
             factorable offline — fail-closed)"
        )));
    }
    RsaPublicKey::new(n_int, BigUint::from_bytes_be(e))
        .map_err(|err| refuse(format!("invalid RSA public key on the wire: {err}")))
}

/// RSA PKCS#1 v1.5 / SHA-256 verification from the SSH wire `n`/`e` (`rsa-sha2-256`). Vetted `rsa` +
/// `sha2` — no hand-rolled bignum/curve math. A weak/zero/invalid modulus is a loud refusal (never a
/// panic).
fn verify_rsa_sha256(n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), AuthzError> {
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::signature::Verifier;
    use sha2::Sha256;
    let vk = VerifyingKey::<Sha256>::new(rsa_public_key(n, e)?);
    let signature = Signature::try_from(sig)
        .map_err(|_| refuse("malformed rsa-sha2-256 signature encoding"))?;
    vk.verify(msg, &signature)
        .map_err(|_| refuse("rsa-sha2-256 signature verification failed"))
}

/// RSA PKCS#1 v1.5 / SHA-512 verification from the SSH wire `n`/`e` (`rsa-sha2-512`). Vetted `rsa` +
/// `sha2`.
fn verify_rsa_sha512(n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), AuthzError> {
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::signature::Verifier;
    use sha2::Sha512;
    let vk = VerifyingKey::<Sha512>::new(rsa_public_key(n, e)?);
    let signature = Signature::try_from(sig)
        .map_err(|_| refuse("malformed rsa-sha2-512 signature encoding"))?;
    vk.verify(msg, &signature)
        .map_err(|_| refuse("rsa-sha2-512 signature verification failed"))
}

// ================================================================================================
// The key→principal binding index (S1 SSO-link, injected).
// ================================================================================================

/// The trust-rooted facts a REGISTERED SSH key binds to. `tenant`/`region`/`subject_key` come ONLY
/// from here — never from the credential wrapper (ID-3, the tenant-injection defence).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredKey {
    /// The tenant the key is registered to (the trust root — never a credential/path value).
    pub tenant: TenantId,
    /// The residency region the principal is pinned to.
    pub region: Region,
    /// The stable subject key the S1 SSO-link index resolves (conventionally the key fingerprint).
    pub subject_key: String,
}

/// **The injected SSH-key→principal binding index (the S1 SSO-link, the key registry).** Maps an
/// OpenSSH key **fingerprint** (`SHA256:…`) to its [`RegisteredKey`] binding. The verifier consults
/// it after parsing the presented public key; an UNREGISTERED key is refused. In tests it is built
/// directly; a live deployment populates it from the S1 store when a user/service registers a key.
#[derive(Clone, Debug, Default)]
pub struct KeyBindingIndex {
    by_fingerprint: BTreeMap<String, RegisteredKey>,
}

impl KeyBindingIndex {
    /// An empty registry.
    pub fn new() -> KeyBindingIndex {
        KeyBindingIndex {
            by_fingerprint: BTreeMap::new(),
        }
    }

    /// Register a key fingerprint → binding (builder form). The tenant/region are the trust root.
    pub fn with_binding(
        mut self,
        fingerprint: impl Into<String>,
        binding: RegisteredKey,
    ) -> KeyBindingIndex {
        self.by_fingerprint.insert(fingerprint.into(), binding);
        self
    }

    /// Register a PUBLIC-KEY BLOB directly (computes the fingerprint), binding it to `tenant`/`region`
    /// with the `subject_key` set to the fingerprint (the conventional S1 SSO-link key). Builder form.
    pub fn with_key_blob(
        self,
        public_key_blob: &[u8],
        tenant: impl Into<String>,
        region: impl Into<String>,
    ) -> KeyBindingIndex {
        let fp = ssh_fingerprint(public_key_blob);
        let binding = RegisteredKey {
            tenant: TenantId(tenant.into()),
            region: Region(region.into()),
            subject_key: fp.clone(),
        };
        self.with_binding(fp, binding)
    }

    /// The binding for a fingerprint, if the key is registered.
    pub fn get(&self, fingerprint: &str) -> Option<&RegisteredKey> {
        self.by_fingerprint.get(fingerprint)
    }

    /// The number of registered keys.
    pub fn len(&self) -> usize {
        self.by_fingerprint.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.by_fingerprint.is_empty()
    }
}

/// **The SSH key→principal binding source the verifier consults (MR-010d).** The verifier looks a
/// presented key's fingerprint up here AFTER parsing it (an unregistered key is refused). Abstracted so
/// the binding can be either the in-memory [`KeyBindingIndex`] (tests / a static config) OR the DURABLE
/// PG [`PrincipalStore`] ([`PrincipalStoreKeyBindings`]) — so a registered key authenticates against the
/// durable binding that SURVIVES RESTART, not an injected stub. The resolution is tenant-scoped: a
/// durable resolver is bound to ONE verified `(tenant, region)` and never reaches another tenant's keys.
pub trait KeyBindingResolver: Send + Sync {
    /// The trust-rooted binding a registered fingerprint maps to, or `None` if the key is unregistered.
    /// `tenant`/`region`/`subject_key` come ONLY from here — never from the credential wrapper (ID-3).
    fn resolve(&self, fingerprint: &str) -> Option<RegisteredKey>;
}

/// The in-memory index is itself a binding source (tests / a static deployment config).
impl KeyBindingResolver for KeyBindingIndex {
    fn resolve(&self, fingerprint: &str) -> Option<RegisteredKey> {
        self.get(fingerprint).cloned()
    }
}

/// **The DURABLE SSH-key→principal binding, resolved from the S1 [`PrincipalStore`] (the MR-010d
/// follow-up).** Replaces the injected in-memory stub: a registered SSH key resolves to its principal
/// via [`PrincipalStore::resolve_credential`] keyed by the OpenSSH fingerprint under the `ssh` scheme,
/// so the binding SURVIVES RESTART (it lives in the durable PG store, MR-007) instead of an in-process
/// map. Bound to ONE verified `(tenant, region)` scope (the candidate tenant the Git smart-transport
/// establishes — GT-006): a key registered under tenant A is invisible to a resolver scoped to tenant B
/// (no cross-tenant key resolution).
///
/// **Registration** is the same durable link: [`Self::register_key`] computes the fingerprint and calls
/// [`PrincipalStore::link_credential`] (on SSH-key add) — the SINGLE link both this resolver and
/// `authenticate`'s downstream principal lookup key on.
///
/// **Boundary note (GT-006).** This is the durable key→principal BINDING + resolution. Issuing the SSH
/// challenge ON the Git transport handshake (the live challenge-response over the wire) is the
/// smart-transport prompt (GT-006); the cryptographic verification + freshness/replay defence in
/// [`SshVerifier`] are unchanged.
#[derive(Clone)]
pub struct PrincipalStoreKeyBindings {
    store: PrincipalStore,
    scope: TenantScope,
}

impl PrincipalStoreKeyBindings {
    /// Bind the resolver to the durable store + the verified `(tenant, region)` scope the keys resolve
    /// within. The scope is the trust-rooted candidate tenant (never a path/arg).
    pub fn new(store: PrincipalStore, scope: TenantScope) -> PrincipalStoreKeyBindings {
        PrincipalStoreKeyBindings { store, scope }
    }

    /// **The registration path (SSH-key add).** Compute the OpenSSH fingerprint of a public-key blob and
    /// durably link it to `principal_id` under the `ssh` scheme (the S1 SSO-link). The principal must
    /// already exist in the scope (a dangling link is refused). Returns the fingerprint it registered.
    pub fn register_key(
        &self,
        public_key_blob: &[u8],
        principal_id: &PrincipalId,
    ) -> Result<String, PrincipalError> {
        let fingerprint = ssh_fingerprint(public_key_blob);
        self.store
            .link_credential(&self.scope, scheme::SSH, &fingerprint, principal_id)?;
        Ok(fingerprint)
    }

    /// Register a pre-computed fingerprint → principal link (when the fingerprint is already known).
    pub fn register_fingerprint(
        &self,
        fingerprint: &str,
        principal_id: &PrincipalId,
    ) -> Result<(), PrincipalError> {
        self.store
            .link_credential(&self.scope, scheme::SSH, fingerprint, principal_id)
    }

    /// The verified scope this resolver is bound to (so a caller can inspect the tenant/region floor).
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }
}

impl KeyBindingResolver for PrincipalStoreKeyBindings {
    fn resolve(&self, fingerprint: &str) -> Option<RegisteredKey> {
        // The durable lookup: a credential link `(ssh, fingerprint) → principal` within the verified
        // partition. The tenant/region come from the DURABLE row (the trust root), never the wrapper;
        // a fingerprint registered under another tenant is not in this scope's partition (no
        // cross-tenant resolution). subject_key = the fingerprint (the conventional S1 SSO-link key the
        // downstream `authenticate` principal lookup re-resolves on).
        let row = self
            .store
            .resolve_credential(&self.scope, scheme::SSH, fingerprint)?;
        Some(RegisteredKey {
            tenant: row.tenant,
            region: row.region,
            subject_key: fingerprint.to_string(),
        })
    }
}

// ================================================================================================
// The challenge store — single-use, time-bounded freshness (replay defence).
// ================================================================================================

/// The "now" source, in Unix seconds — injected so a test can pin/advance the clock across the
/// challenge-expiry boundary (the production default reads the system clock).
type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A server-issued challenge: the opaque `id` the client echoes back, the random `nonce` it must
/// sign, and the `expires_at` bound.
#[derive(Clone, Debug)]
pub struct Challenge {
    /// The opaque challenge id (the client returns it in its credential so the verifier finds the
    /// nonce it must check the signature against).
    pub id: String,
    /// The random nonce the client signs (via [`signed_payload`]).
    pub nonce: Vec<u8>,
    /// The Unix-seconds expiry (a challenge consumed after this is refused).
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
struct StoredChallenge {
    nonce: Vec<u8>,
    expires_at: i64,
    consumed: bool,
}

/// **The challenge store — single-use + time-bounded freshness (the replay defence, CRITICAL).** A
/// server issues a fresh random challenge ([`ChallengeGuard::issue`]); the verifier [`consume`]s it
/// exactly once. A SECOND consume of the same id is rejected (replay), and a consume after the expiry
/// bound is rejected (stale). Cloneable (shared inner map) so the issuing side and the verifier
/// consult ONE seen-set. The clock is injected (testable across the expiry boundary). The nonce is
/// generated with `ring`'s CSPRNG.
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

    /// **Issue a fresh, random, single-use, time-bounded challenge.** The 32-byte nonce + the opaque
    /// id are CSPRNG-generated (`ring`); the entry is stored unconsumed with `expires_at = now + ttl`.
    pub fn issue(&self) -> Result<Challenge, AuthzError> {
        let id = URL_SAFE_NO_PAD.encode(random_bytes(16)?);
        let nonce = random_bytes(32)?;
        let expires_at = self.now().saturating_add(self.ttl_secs);
        self.lock().insert(
            id.clone(),
            StoredChallenge {
                nonce: nonce.clone(),
                expires_at,
                consumed: false,
            },
        );
        Ok(Challenge {
            id,
            nonce,
            expires_at,
        })
    }

    /// Pre-seed an EXPLICIT challenge (the test seam — deterministic id/nonce). Returns the
    /// `expires_at` it was stored with.
    pub fn issue_explicit(&self, id: impl Into<String>, nonce: Vec<u8>) -> i64 {
        let expires_at = self.now().saturating_add(self.ttl_secs);
        self.lock().insert(
            id.into(),
            StoredChallenge {
                nonce,
                expires_at,
                consumed: false,
            },
        );
        expires_at
    }

    /// **Consume the challenge `id` ONCE, returning its server-issued nonce.** This is the single-use
    /// and freshness gate: an UNKNOWN id, an EXPIRED challenge, or an ALREADY-CONSUMED challenge (a
    /// replay) is a loud refusal. On success the entry is marked consumed (a later consume is a
    /// replay). The nonce returned is the SERVER's — never a credential-supplied value.
    pub fn consume(&self, id: &str) -> Result<Vec<u8>, AuthzError> {
        let now = self.now();
        let mut map = self.lock();
        let entry = map.get_mut(id).ok_or_else(|| {
            refuse("unknown SSH challenge (not server-issued, or already expired)")
        })?;
        if now > entry.expires_at {
            // Stale — drop it and refuse.
            map.remove(id);
            return Err(refuse(
                "expired SSH challenge (stale — re-issue a fresh challenge)",
            ));
        }
        if entry.consumed {
            return Err(refuse(
                "replayed SSH challenge (this challenge+signature was already presented — replay \
                 defence)",
            ));
        }
        entry.consumed = true;
        Ok(entry.nonce.clone())
    }
}

/// CSPRNG bytes via `ring`'s `SystemRandom` (the same vetted RNG the OIDC test keygen uses). An RNG
/// failure is a loud `Unavailable` (never a panic / a predictable nonce).
fn random_bytes(n: usize) -> Result<Vec<u8>, AuthzError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf)
        .map_err(|_| AuthzError::Unavailable("CSPRNG failure issuing SSH challenge".into()))?;
    Ok(buf)
}

// ================================================================================================
// The credential envelope (the client → server SSH-auth wire shape).
// ================================================================================================

/// Encode the SSH-auth credential `material` the client presents: the base64 public-key blob, the
/// base64 signature blob, and the challenge id. PUBLIC so the real Git-transport client (and the
/// tests) build the SAME shape the verifier parses. The verifier reads tenant/region from the
/// REGISTERED binding — the envelope carries NO tenant (the tenant-injection defence is structural).
pub fn encode_ssh_credential_material(
    public_key_blob: &[u8],
    signature_blob: &[u8],
    challenge_id: &str,
) -> String {
    serde_json::json!({
        "public_key": B64.encode(public_key_blob),
        "signature": B64.encode(signature_blob),
        "challenge_id": challenge_id,
    })
    .to_string()
}

/// The parsed envelope. Extra/unknown JSON fields (e.g. an injected `"tenant"`) are IGNORED — only
/// these three fields are read, so the tenant is NEVER read from the wrapper (ID-3).
struct SshEnvelope {
    public_key: String,
    signature: String,
    challenge_id: String,
}

impl SshEnvelope {
    /// Parse the credential envelope from JSON via `serde_json::Value` (a missing/non-string field is
    /// a loud structural refusal; unknown fields are ignored — the tenant-injection defence). Total
    /// over attacker bytes.
    fn parse(material: &str) -> Result<SshEnvelope, AuthzError> {
        let v: serde_json::Value = serde_json::from_str(material)
            .map_err(|e| malformed(format!("malformed SSH credential envelope JSON: {e}")))?;
        let field = |name: &str| -> Result<String, AuthzError> {
            v.get(name)
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .ok_or_else(|| malformed(format!("SSH credential envelope missing `{name}`")))
        };
        Ok(SshEnvelope {
            public_key: field("public_key")?,
            signature: field("signature")?,
            challenge_id: field("challenge_id")?,
        })
    }
}

// ================================================================================================
// The verifier.
// ================================================================================================

/// **The REAL SSH public-key challenge-response credential verifier (MR-010d).** Verifies a presented
/// SSH public-key signature over a server-issued challenge nonce with vetted primitives + the
/// alg-downgrade / alg-key-mismatch defences, enforces single-use + time-bounded challenge freshness,
/// and resolves the tenant/region/subject from the REGISTERED key binding — or refuses loudly. `verify`
/// is TOTAL over attacker-controlled bytes (no panic). Plugs into the existing [`CredentialVerifier`]
/// seam; the [`crate::authenticate`] resolution + telemetry body does not change.
#[derive(Clone)]
pub struct SshVerifier {
    registry: Arc<dyn KeyBindingResolver>,
    challenges: ChallengeGuard,
}

impl SshVerifier {
    /// Build the verifier over an in-memory key registry (tests / a static config) + challenge store
    /// (the replay/freshness defence). Wire it as the `ssh`-scheme verifier via
    /// `SchemeDispatchVerifier::route(scheme::SSH, …)`.
    pub fn new(registry: KeyBindingIndex, challenges: ChallengeGuard) -> SshVerifier {
        SshVerifier {
            registry: Arc::new(registry),
            challenges,
        }
    }

    /// **Build the verifier over a DURABLE binding source (MR-010d).** The canonical production
    /// constructor: pass a [`PrincipalStoreKeyBindings`] (or any [`KeyBindingResolver`]) so a registered
    /// SSH key resolves against the durable PG store — surviving restart, tenant-scoped — not an
    /// injected stub.
    pub fn with_resolver(
        registry: Arc<dyn KeyBindingResolver>,
        challenges: ChallengeGuard,
    ) -> SshVerifier {
        SshVerifier {
            registry,
            challenges,
        }
    }

    /// The shared challenge store (so the issuing side and a caller share ONE seen-set / can issue a
    /// fresh challenge).
    pub fn challenges(&self) -> &ChallengeGuard {
        &self.challenges
    }

    /// The injected key-binding source (so a caller can inspect which resolver is wired).
    pub fn bindings(&self) -> &dyn KeyBindingResolver {
        self.registry.as_ref()
    }
}

impl CredentialVerifier for SshVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        // This verifier owns ONLY the ssh scheme; another scheme is a wiring error (the dispatcher
        // routes by scheme). Refuse loudly rather than mis-verify.
        if credential.scheme != scheme::SSH {
            return Err(malformed(format!(
                "SshVerifier received a `{}` credential (expected `ssh`)",
                credential.scheme
            )));
        }

        // (1) Parse the credential envelope (base64 blobs + challenge id). Malformed JSON / base64 is
        //     a loud structural refusal (never coerced).
        let env = SshEnvelope::parse(credential.material.trim())?;
        let public_key_blob = B64
            .decode(env.public_key.as_bytes())
            .map_err(|e| malformed(format!("malformed base64 SSH public key: {e}")))?;
        let signature_blob = B64
            .decode(env.signature.as_bytes())
            .map_err(|e| malformed(format!("malformed base64 SSH signature: {e}")))?;

        // (2) Parse the SSH public-key + signature wire blobs (bounds-checked + total). The signature
        //     parse REJECTS the weak SHA-1 `ssh-rsa` algorithm and any unknown algorithm here.
        let key = parse_ssh_public_key(&public_key_blob)?;
        let sig = parse_ssh_signature(&signature_blob)?;

        // (3) KEY→PRINCIPAL BINDING — the presented public key's fingerprint MUST be REGISTERED. An
        //     unregistered key is refused (no fabricated principal). The trust root (tenant/region/
        //     subject) is read from THIS binding — never the credential wrapper (ID-3). Looked up
        //     BEFORE consuming the challenge so an unregistered probe cannot burn a valid challenge.
        let fingerprint = ssh_fingerprint(&public_key_blob);
        let binding = self.registry.resolve(&fingerprint).ok_or_else(|| {
            refuse(format!(
                "unregistered SSH key fingerprint `{fingerprint}` (no S1 binding — fail-closed, \
                 never a fabricated principal)"
            ))
        })?;

        // (4) CHALLENGE FRESHNESS / REPLAY DEFENCE — consume the server-issued challenge ONCE,
        //     getting the SERVER's nonce. An unknown / expired / replayed challenge is refused here.
        //     The nonce comes from the store (server-issued), NEVER from the credential.
        let nonce = self.challenges.consume(&env.challenge_id)?;

        // (5) SIGNATURE — verify the presented signature against the REGISTERED public key over the
        //     server-issued nonce (with the domain-separation context). A forged signature (a
        //     different/attacker key), or a valid signature over a DIFFERENT nonce than the one
        //     issued, fails here. The alg/key-type match is enforced inside.
        let message = signed_payload(&nonce);
        verify_ssh_signature(&key, &sig, &message)?;

        // (6) THE TRUST-ROOTED ASSERTION — tenant/region/subject come ONLY from the registered
        //     binding. The credential wrapper never supplied them (the tenant-injection defence).
        Ok(VerifiedAssertion {
            tenant: binding.tenant.clone(),
            region: binding.region.clone(),
            scheme: scheme::SSH.to_string(),
            subject_key: binding.subject_key.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticate::StructuralVerifier;
    use crate::oidc::SchemeDispatchVerifier;
    use std::sync::atomic::{AtomicI64, Ordering};

    // ── SSH wire writers (test side — build REAL key/signature blobs) ────────────────────────────

    /// Append an RFC 4251 length-prefixed string.
    fn put_string(out: &mut Vec<u8>, s: &[u8]) {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
        out.extend_from_slice(s);
    }

    /// Encode a big-endian magnitude as an SSH `mpint` (strip leading zeros; prepend a 0x00 if the
    /// high bit is set, to keep the value positive).
    fn mpint(mag: &[u8]) -> Vec<u8> {
        let mut b = mag;
        while b.len() > 1 && b[0] == 0 {
            b = &b[1..];
        }
        let mut out = Vec::new();
        if !b.is_empty() && (b[0] & 0x80) != 0 {
            out.push(0);
        }
        out.extend_from_slice(b);
        out
    }

    // ── Ed25519 (ring) test key ──────────────────────────────────────────────────────────────────
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
        fn pubkey_blob(&self) -> Vec<u8> {
            let mut b = Vec::new();
            put_string(&mut b, b"ssh-ed25519");
            put_string(&mut b, &self.public);
            b
        }
        /// A REAL ed25519 signature blob over `msg`.
        fn sig_blob(&self, msg: &[u8]) -> Vec<u8> {
            let sig = self.pair.sign(msg);
            let mut b = Vec::new();
            put_string(&mut b, b"ssh-ed25519");
            put_string(&mut b, sig.as_ref());
            b
        }
        /// A sig blob whose ALGORITHM LABEL is forced (to forge alg/key-type-mismatch corpus cases).
        fn sig_blob_with_alg(&self, alg: &[u8], msg: &[u8]) -> Vec<u8> {
            let sig = self.pair.sign(msg);
            let mut b = Vec::new();
            put_string(&mut b, alg);
            put_string(&mut b, sig.as_ref());
            b
        }
    }

    // ── RSA (rsa crate) test key ─────────────────────────────────────────────────────────────────
    struct RsaKey {
        priv_key: rsa::RsaPrivateKey,
    }
    impl RsaKey {
        fn generate() -> RsaKey {
            // 2048-bit — the accepted floor; generated once per test that needs RSA.
            RsaKey::generate_bits(2048)
        }
        fn generate_bits(bits: usize) -> RsaKey {
            use rand::rngs::OsRng;
            let priv_key = rsa::RsaPrivateKey::new(&mut OsRng, bits).expect("rsa keygen");
            RsaKey { priv_key }
        }
        fn pubkey_blob(&self) -> Vec<u8> {
            use rsa::traits::PublicKeyParts;
            let pubk = self.priv_key.to_public_key();
            let mut b = Vec::new();
            put_string(&mut b, b"ssh-rsa");
            put_string(&mut b, &mpint(&pubk.e().to_bytes_be()));
            put_string(&mut b, &mpint(&pubk.n().to_bytes_be()));
            b
        }
        fn sig_blob_sha256(&self, msg: &[u8]) -> Vec<u8> {
            use rsa::pkcs1v15::SigningKey;
            use rsa::signature::{SignatureEncoding, Signer};
            use sha2::Sha256;
            let sk = SigningKey::<Sha256>::new(self.priv_key.clone());
            let sig = sk.sign(msg).to_vec();
            let mut b = Vec::new();
            put_string(&mut b, b"rsa-sha2-256");
            put_string(&mut b, &sig);
            b
        }
        fn sig_blob_sha512(&self, msg: &[u8]) -> Vec<u8> {
            use rsa::pkcs1v15::SigningKey;
            use rsa::signature::{SignatureEncoding, Signer};
            use sha2::Sha512;
            let sk = SigningKey::<Sha512>::new(self.priv_key.clone());
            let sig = sk.sign(msg).to_vec();
            let mut b = Vec::new();
            put_string(&mut b, b"rsa-sha2-512");
            put_string(&mut b, &sig);
            b
        }
        /// A legacy/weak `ssh-rsa` (RSA/SHA-1) signature blob — the alg-downgrade attacker token. The
        /// verifier rejects the `ssh-rsa` algorithm at PARSE time (before any signature math), so the
        /// signature bytes themselves are immaterial; we carry a real RSA-sized byte string under the
        /// weak `ssh-rsa` label (no `sha1` dep needed to prove the downgrade is refused).
        fn sig_blob_sha1_legacy(&self, msg: &[u8]) -> Vec<u8> {
            // Reuse a real (sha2-256) signature's bytes purely as plausible RSA-length payload; the
            // LABEL `ssh-rsa` is what the verifier rejects.
            use rsa::pkcs1v15::SigningKey;
            use rsa::signature::{SignatureEncoding, Signer};
            use sha2::Sha256;
            let sk = SigningKey::<Sha256>::new(self.priv_key.clone());
            let sig = sk.sign(msg).to_vec();
            let mut b = Vec::new();
            put_string(&mut b, b"ssh-rsa");
            put_string(&mut b, &sig);
            b
        }
    }

    // ── Harness ──────────────────────────────────────────────────────────────────────────────────

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

    /// A store + verifier where `key`'s fingerprint is registered to acme/eu-west. Returns the
    /// verifier and the registered fingerprint (= subject_key).
    fn verifier_with_ed_key(key: &EdKey) -> (SshVerifier, String) {
        let blob = key.pubkey_blob();
        let fp = ssh_fingerprint(&blob);
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let challenges = ChallengeGuard::new(300);
        (SshVerifier::new(registry, challenges), fp)
    }

    fn cred(material: String) -> Credential {
        Credential {
            scheme: scheme::SSH.into(),
            material,
        }
    }

    /// Issue a fresh challenge, sign it with `sign`, return the assembled credential + the challenge.
    fn signed_cred(
        v: &SshVerifier,
        pubkey_blob: &[u8],
        sign: impl Fn(&[u8]) -> Vec<u8>,
    ) -> Credential {
        let ch = v.challenges().issue().expect("issue challenge");
        let msg = signed_payload(&ch.nonce);
        let sig_blob = sign(&msg);
        cred(encode_ssh_credential_material(
            pubkey_blob,
            &sig_blob,
            &ch.id,
        ))
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // POSITIVE corpus — a correctly-signed ed25519 AND rsa-sha2-256/512 challenge each VERIFY and
    // yield the REGISTERED principal (tenant/region/subject from the binding).
    // ════════════════════════════════════════════════════════════════════════════════════════════

    #[test]
    fn positive_ed25519_verifies_and_yields_registered_principal() {
        let key = EdKey::generate();
        let (v, fp) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        let c = signed_cred(&v, &blob, |m| key.sig_blob(m));

        let a = v
            .verify(&c)
            .expect("a correctly-signed ed25519 challenge must verify");
        assert_eq!(a.tenant, TenantId(TENANT.into()));
        assert_eq!(a.region, Region(REGION.into()));
        assert_eq!(a.scheme, scheme::SSH);
        assert_eq!(
            a.subject_key, fp,
            "subject = the registered key fingerprint"
        );
    }

    #[test]
    fn positive_rsa_sha2_256_verifies_and_yields_registered_principal() {
        let key = RsaKey::generate();
        let blob = key.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        let c = signed_cred(&v, &blob, |m| key.sig_blob_sha256(m));

        let a = v
            .verify(&c)
            .expect("a correctly-signed rsa-sha2-256 challenge must verify");
        assert_eq!(a.tenant, TenantId(TENANT.into()));
        assert_eq!(a.subject_key, ssh_fingerprint(&blob));
    }

    #[test]
    fn positive_rsa_sha2_512_verifies_and_yields_registered_principal() {
        let key = RsaKey::generate();
        let blob = key.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        let c = signed_cred(&v, &blob, |m| key.sig_blob_sha512(m));

        let a = v
            .verify(&c)
            .expect("a correctly-signed rsa-sha2-512 challenge must verify");
        assert_eq!(a.tenant, TenantId(TENANT.into()));
    }

    // ════════════════════════════════════════════════════════════════════════════════════════════
    // NEGATIVE corpus — each forged/invalid credential MUST be refused (the whole point).
    // ════════════════════════════════════════════════════════════════════════════════════════════

    /// (a) FORGED SIGNATURE — the victim's REGISTERED public key is presented (it is public!), but the
    /// challenge is signed by an ATTACKER's key. The signature must fail against the registered key.
    #[test]
    fn negative_forged_signature_by_a_different_key_is_rejected() {
        let victim = EdKey::generate();
        let attacker = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&victim);
        let victim_blob = victim.pubkey_blob();
        // Present the VICTIM's public key, but sign with the ATTACKER's private key.
        let c = signed_cred(&v, &victim_blob, |m| attacker.sig_blob(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a signature by a different key must be refused, got {err:?}"
        );
    }

    /// (b) REPLAY — the SAME challenge+signature presented twice. The first verifies; the second
    /// (same challenge id) is refused (single-use challenge).
    #[test]
    fn negative_replayed_challenge_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        let c = signed_cred(&v, &blob, |m| key.sig_blob(m));
        v.verify(&c).expect("first presentation verifies");
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
            "a replayed challenge must be refused, got {err:?}"
        );
    }

    /// (c) EXPIRED — a challenge consumed after its TTL bound is refused. The clock is advanced past
    /// the expiry between issue and verify.
    #[test]
    fn negative_expired_challenge_is_rejected() {
        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let clock = Arc::new(AtomicI64::new(1_000));
        let c2 = clock.clone();
        let challenges = ChallengeGuard::new(300).with_clock(move || c2.load(Ordering::SeqCst));
        let v = SshVerifier::new(registry, challenges);

        // Issue at t=1000 (expires 1300), sign, then jump the clock past expiry.
        let ch = v.challenges().issue().unwrap();
        let sig_blob = key.sig_blob(&signed_payload(&ch.nonce));
        let c = cred(encode_ssh_credential_material(&blob, &sig_blob, &ch.id));
        clock.store(2_000, Ordering::SeqCst); // well past 1300

        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired challenge must be refused, got {err:?}"
        );
    }

    /// (d) UNREGISTERED KEY — a perfectly valid self-signature, but the presenting key is not in the
    /// registry. Refused (no fabricated principal).
    #[test]
    fn negative_unregistered_key_is_rejected() {
        let registered = EdKey::generate();
        let stranger = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&registered);
        let stranger_blob = stranger.pubkey_blob();
        // The stranger validly signs the challenge with their OWN key — but the key is unregistered.
        let c = signed_cred(&v, &stranger_blob, |m| stranger.sig_blob(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unregistered SSH key")),
            "an unregistered key must be refused, got {err:?}"
        );
    }

    /// (e) SIGNATURE / CHALLENGE MISMATCH — a VALID signature, but over a DIFFERENT nonce than the one
    /// the server issued for this challenge id. The verifier checks the sig over the SERVER's nonce →
    /// it does not match → refused. (Proves the nonce comes from the store, not the credential.)
    #[test]
    fn negative_signature_over_a_different_nonce_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        // Issue a real challenge, but sign a DIFFERENT (attacker-chosen) nonce.
        let ch = v.challenges().issue().unwrap();
        let wrong_nonce = b"a-different-nonce-the-server-never-issued".to_vec();
        let sig_blob = key.sig_blob(&signed_payload(&wrong_nonce));
        let c = cred(encode_ssh_credential_material(&blob, &sig_blob, &ch.id));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a signature over the wrong nonce must be refused, got {err:?}"
        );
    }

    /// (f) WEAK SHA-1 `ssh-rsa` — the legacy RSA/SHA-1 signature algorithm is rejected even with a
    /// REGISTERED key and a genuine SHA-1 signature (the alg-downgrade defence).
    #[test]
    fn negative_weak_ssh_rsa_sha1_is_rejected() {
        let key = RsaKey::generate();
        let blob = key.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        // A GENUINE RSA/SHA-1 signature over the issued challenge, labelled `ssh-rsa`.
        let c = signed_cred(&v, &blob, |m| key.sig_blob_sha1_legacy(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("weak `ssh-rsa`")),
            "the weak SHA-1 ssh-rsa algorithm must be refused, got {err:?}"
        );
    }

    /// (g) ALG / KEY-TYPE MISMATCH — an Ed25519 key, but the signature blob is LABELLED `rsa-sha2-256`.
    /// The alg does not match the key type → refused (never verified against the wrong primitive).
    #[test]
    fn negative_alg_key_type_mismatch_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        // A real ed25519 signature, but mislabelled as rsa-sha2-256.
        let c = signed_cred(&v, &blob, |m| key.sig_blob_with_alg(b"rsa-sha2-256", m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("alg/key-type mismatch")),
            "an alg/key-type mismatch must be refused, got {err:?}"
        );
    }

    /// (g') UNKNOWN SIGNATURE ALGORITHM — a made-up algorithm label is refused.
    #[test]
    fn negative_unknown_signature_algorithm_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        let c = signed_cred(&v, &blob, |m| key.sig_blob_with_alg(b"ssh-dss-haha", m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unknown SSH signature algorithm")),
            "an unknown signature algorithm must be refused, got {err:?}"
        );
    }

    /// (h) MALFORMED WIRE — truncated/garbage/wrong-field-count blobs must be REFUSED, never PANIC.
    /// Covers: non-JSON, bad base64, truncated key/sig blobs, a huge length prefix, and an empty body.
    #[test]
    fn negative_malformed_wire_is_refused_not_panicking() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let good_blob = key.pubkey_blob();

        // A grab-bag of malformed materials (each must return Err, never panic).
        let mut cases: Vec<String> = vec![
            String::new(),
            "not json".into(),
            "{}".into(),
            // valid JSON shape but bad base64 in the fields
            serde_json::json!({"public_key":"!!!","signature":"!!!","challenge_id":"x"})
                .to_string(),
        ];

        // A challenge that exists, paired with structurally-broken wire blobs (so we reach the wire
        // parser). Each blob is deliberately truncated / over-long.
        let ch = v.challenges().issue().unwrap();
        let truncated_key = &good_blob[..good_blob.len() / 2]; // chopped mid-string
        cases.push(encode_ssh_credential_material(
            truncated_key,
            &key.sig_blob(&signed_payload(&ch.nonce)),
            &ch.id,
        ));
        // A blob whose first string claims a huge length (4 GiB) but carries no bytes.
        let huge_len_blob = {
            let mut b = Vec::new();
            b.extend_from_slice(&u32::MAX.to_be_bytes());
            b
        };
        let ch2 = v.challenges().issue().unwrap();
        cases.push(encode_ssh_credential_material(
            &huge_len_blob,
            &huge_len_blob,
            &ch2.id,
        ));
        // Garbage bytes for both blobs.
        let ch3 = v.challenges().issue().unwrap();
        cases.push(encode_ssh_credential_material(
            b"\x00\x01\x02",
            b"\xff\xfe",
            &ch3.id,
        ));

        for (i, material) in cases.iter().enumerate() {
            let r = v.verify(&cred(material.clone()));
            assert!(
                r.is_err(),
                "malformed case {i} must be refused (and must not panic)"
            );
        }
    }

    /// (i) TENANT-INJECTION — the credential wrapper carries an extra `"tenant":"globex"` field. It is
    /// IGNORED; the resolved tenant is the REGISTERED binding's (acme), never the wrapper's. This is
    /// the SSH analogue of the OIDC trust-root property.
    #[test]
    fn negative_tenant_injection_in_the_wrapper_is_ignored() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        let ch = v.challenges().issue().unwrap();
        let sig_blob = key.sig_blob(&signed_payload(&ch.nonce));
        // Hand-build an envelope that ALSO injects a spurious tenant claim.
        let material = serde_json::json!({
            "public_key": B64.encode(&blob),
            "signature": B64.encode(&sig_blob),
            "challenge_id": ch.id,
            "tenant": "globex",   // the injection attempt — must be ignored
            "region": "us-east",
        })
        .to_string();
        let a = v
            .verify(&cred(material))
            .expect("verifies on the real signature");
        assert_eq!(
            a.tenant,
            TenantId(TENANT.into()),
            "tenant is the REGISTERED binding's (acme), never the injected wrapper value (globex)"
        );
        assert_eq!(
            a.region,
            Region(REGION.into()),
            "region is the binding's too"
        );
    }

    /// (j) UNKNOWN CHALLENGE — a valid signature, but the credential names a challenge id the server
    /// never issued. Refused.
    #[test]
    fn negative_unknown_challenge_id_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        // Sign SOME nonce, but reference a challenge id that was never issued.
        let sig_blob = key.sig_blob(&signed_payload(b"whatever"));
        let c = cred(encode_ssh_credential_material(
            &blob,
            &sig_blob,
            "never-issued-id",
        ));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unknown SSH challenge")),
            "an unknown challenge id must be refused, got {err:?}"
        );
    }

    /// (k) WRONG SCHEME — a non-ssh credential routed here is a wiring error; refuse loudly.
    #[test]
    fn negative_wrong_scheme_is_refused() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let r = v.verify(&Credential {
            scheme: scheme::OIDC.into(),
            material: "x".into(),
        });
        assert!(matches!(r, Err(AuthzError::BadRequest(_))));
    }

    /// (l) RSA FORGERY — the victim's REGISTERED RSA key is presented, but an ATTACKER RSA key signs.
    /// The rsa-sha2-256 signature must fail against the registered modulus.
    #[test]
    fn negative_rsa_forged_by_different_key_is_rejected() {
        let victim = RsaKey::generate();
        let attacker = RsaKey::generate();
        let victim_blob = victim.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&victim_blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        // Present victim's public key; sign with attacker's private key.
        let c = signed_cred(&v, &victim_blob, |m| attacker.sig_blob_sha256(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("rsa-sha2-256 signature verification failed")),
            "an RSA signature by a different key must be refused, got {err:?}"
        );
    }

    /// (m) WEAK RSA KEY SIZE — a REGISTERED 1024-bit RSA key with a GENUINELY-CORRECT rsa-sha2-256
    /// signature is REFUSED (key-too-small): a sub-2048-bit modulus is factorable offline, so it
    /// fails closed regardless of the signature. The positive RSA tests above use 2048-bit keys (the
    /// accept side), so this proves the floor bites exactly at the boundary.
    #[test]
    fn negative_weak_rsa_key_size_is_rejected() {
        let weak = RsaKey::generate_bits(1024);
        let blob = weak.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        // A genuinely-correct signature with the weak key — the modulus floor must still refuse it.
        let c = signed_cred(&v, &blob, |m| weak.sig_blob_sha256(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("rsa key too small")),
            "a registered <2048-bit RSA key must be refused (factorable), got {err:?}"
        );
    }

    // ── The dispatch seam (wiring SshVerifier as the ssh-scheme verifier) ─────────────────────────

    /// The dispatcher routes the `ssh` scheme to the REAL [`SshVerifier`] and everything else to the
    /// injected fallback. A forged ssh credential hits the real verifier and is refused; a non-ssh
    /// scheme rides the floor fallback (unchanged). (The floor `StructuralVerifier` here is
    /// `#[cfg(test)]`, so the production-graph scanner admits it.)
    #[test]
    fn dispatch_routes_ssh_to_real_verifier_and_others_to_fallback() {
        let victim = EdKey::generate();
        let attacker = EdKey::generate();
        let (ssh_v, _) = verifier_with_ed_key(&victim);
        let victim_blob = victim.pubkey_blob();
        // Mint a forged ssh credential (attacker signs) THROUGH the shared challenge store.
        let forged = signed_cred(&ssh_v, &victim_blob, |m| attacker.sig_blob(m));

        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::SSH, Arc::new(ssh_v));

        // The forged ssh credential must hit the REAL verifier and be refused (not silently accepted).
        assert!(
            dispatch.verify(&forged).is_err(),
            "a forged ssh credential must reach the real verifier and be refused"
        );

        // A SAML credential (not-yet-real) rides the injected floor fallback (proving routing reached
        // the fallback, unchanged from before).
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

    /// END-TO-END through [`crate::authenticate::HumanSsoAuthenticator`]: a correctly-signed ssh
    /// challenge resolves to the registered Principal over the S1 store, tenant-from-the-binding.
    #[test]
    fn end_to_end_through_authenticator_resolves_registered_principal() {
        use crate::authenticate::HumanSsoAuthenticator;
        use crate::principal_store::PrincipalStore;
        use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
        use myelin_storage::{KmsEngine, TenantScope};

        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let fp = ssh_fingerprint(&blob);

        // Seed S1: a principal in acme/eu-west, with the SSH fingerprint linked as its credential.
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = TenantScope::from_verified_token(
            &Principal::stub(
                PrincipalId("admin".into()),
                PrincipalKind::Human,
                TenantId(TENANT.into()),
            ),
            Region(REGION.into()),
        );
        store
            .put_principal(
                &scope,
                PrincipalId("svc:deploy".into()),
                PrincipalKind::Service,
                DataRole::Processor,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        store
            .link_credential(&scope, scheme::SSH, &fp, &PrincipalId("svc:deploy".into()))
            .unwrap();

        // Wire the REAL ssh verifier behind the authenticator seam.
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let ssh_v = SshVerifier::new(registry, ChallengeGuard::new(300));
        let c = signed_cred(&ssh_v, &blob, |m| key.sig_blob(m));
        let auth = HumanSsoAuthenticator::with_verifier(store, Arc::new(ssh_v));

        let p = auth
            .authenticate(&c, None)
            .expect("ssh challenge resolves the principal");
        assert_eq!(p.principal_id, PrincipalId("svc:deploy".into()));
        assert_eq!(
            p.tenant,
            TenantId(TENANT.into()),
            "tenant from the registered key binding"
        );
        assert_eq!(p.region, Region(REGION.into()));
        assert_eq!(p.kind, PrincipalKind::Service);
    }

    // ── MR-010d: the DURABLE SSH-key→principal binding (PrincipalStoreKeyBindings) ────────────────

    use crate::principal_store::PrincipalStore;
    use myelin_identity::{DataRole, Principal, PrincipalKind, PrincipalStatus};
    use myelin_storage::{KmsEngine, TenantScope};

    fn admin_scope(store_tenant: &str, region: &str) -> TenantScope {
        TenantScope::from_verified_token(
            &Principal::stub(
                PrincipalId("admin".into()),
                PrincipalKind::Human,
                TenantId(store_tenant.into()),
            ),
            Region(region.into()),
        )
    }

    /// Seed a service principal in `(tenant, region)` (the principal the SSH key will bind to).
    fn seed_service(store: &PrincipalStore, scope: &TenantScope, pid: &str) {
        store
            .put_principal(
                scope,
                PrincipalId(pid.into()),
                PrincipalKind::Service,
                DataRole::Processor,
                PrincipalStatus::Active,
                None,
            )
            .expect("seed principal");
    }

    /// **The MR-010d follow-up: a registered SSH key resolves to its principal via the DURABLE
    /// PrincipalStore (not an injected stub), and the binding SURVIVES a fresh verifier instance** (it
    /// lives in the durable store, which a fresh verifier re-reads — the PG backing is the same store
    /// across a real restart, MR-007). The registration path is `PrincipalStoreKeyBindings::register_key`
    /// (→ `link_credential` on SSH-key add).
    #[test]
    fn durable_binding_resolves_and_survives_a_fresh_verifier() {
        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let fp = ssh_fingerprint(&blob);

        // The durable S1 store + the verified acme/eu-west scope.
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = admin_scope(TENANT, REGION);
        seed_service(&store, &scope, "svc:deploy");

        // REGISTRATION (SSH-key add): durably link the key fingerprint → principal.
        let bindings = PrincipalStoreKeyBindings::new(store.clone(), scope.clone());
        let registered_fp = bindings
            .register_key(&blob, &PrincipalId("svc:deploy".into()))
            .unwrap();
        assert_eq!(registered_fp, fp);

        // The verifier resolves the durable binding (NOT an in-memory KeyBindingIndex).
        let v = SshVerifier::with_resolver(Arc::new(bindings), ChallengeGuard::new(300));
        let c = signed_cred(&v, &blob, |m| key.sig_blob(m));
        let a = v
            .verify(&c)
            .expect("a correctly-signed challenge resolves the durable binding");
        assert_eq!(
            a.tenant,
            TenantId(TENANT.into()),
            "tenant from the DURABLE binding"
        );
        assert_eq!(a.region, Region(REGION.into()));
        assert_eq!(a.subject_key, fp);

        // SURVIVES a fresh verifier: a brand-new resolver + verifier over the SAME durable store (the
        // model of a restart — the binding is not in the verifier, it is in the store) still resolves.
        let fresh = SshVerifier::with_resolver(
            Arc::new(PrincipalStoreKeyBindings::new(store.clone(), scope.clone())),
            ChallengeGuard::new(300),
        );
        let c2 = signed_cred(&fresh, &blob, |m| key.sig_blob(m));
        let a2 = fresh
            .verify(&c2)
            .expect("the durable binding survives a fresh verifier instance");
        assert_eq!(a2.subject_key, fp);
    }

    /// An UNREGISTERED key is refused by the durable resolver (no fabricated principal) — a valid
    /// self-signature, but no durable link in the store.
    #[test]
    fn durable_binding_refuses_an_unregistered_key() {
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = admin_scope(TENANT, REGION);
        // Nothing registered.
        let stranger = EdKey::generate();
        let blob = stranger.pubkey_blob();
        let v = SshVerifier::with_resolver(
            Arc::new(PrincipalStoreKeyBindings::new(store, scope)),
            ChallengeGuard::new(300),
        );
        let c = signed_cred(&v, &blob, |m| stranger.sig_blob(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unregistered SSH key")),
            "an unregistered key must be refused by the durable resolver, got {err:?}"
        );
    }

    /// **Tenant-scoped: no cross-tenant key resolution.** A key registered under tenant `acme` does NOT
    /// resolve through a durable resolver scoped to tenant `globex` (the credential link lives in
    /// acme's partition; resolve_credential never crosses partitions).
    #[test]
    fn durable_binding_is_tenant_scoped_no_cross_tenant_resolution() {
        let key = EdKey::generate();
        let blob = key.pubkey_blob();

        // ONE shared durable store; register the key under acme only.
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let acme = admin_scope("acme", REGION);
        seed_service(&store, &acme, "svc:deploy");
        PrincipalStoreKeyBindings::new(store.clone(), acme)
            .register_key(&blob, &PrincipalId("svc:deploy".into()))
            .unwrap();

        // A verifier scoped to GLOBEX cannot resolve acme's key (cross-tenant resolution is impossible).
        let globex = admin_scope("globex", REGION);
        let v = SshVerifier::with_resolver(
            Arc::new(PrincipalStoreKeyBindings::new(store, globex)),
            ChallengeGuard::new(300),
        );
        let c = signed_cred(&v, &blob, |m| key.sig_blob(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unregistered SSH key")),
            "acme's key must NOT resolve under globex's scope (tenant-scoped), got {err:?}"
        );
    }

    /// END-TO-END through the authenticator over the DURABLE resolver: a single `link_credential` serves
    /// BOTH the verifier's binding lookup AND `authenticate`'s downstream principal resolution.
    #[test]
    fn durable_binding_end_to_end_through_authenticator() {
        use crate::authenticate::HumanSsoAuthenticator;

        let key = EdKey::generate();
        let blob = key.pubkey_blob();

        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = admin_scope(TENANT, REGION);
        seed_service(&store, &scope, "svc:deploy");
        PrincipalStoreKeyBindings::new(store.clone(), scope.clone())
            .register_key(&blob, &PrincipalId("svc:deploy".into()))
            .unwrap();

        let v = SshVerifier::with_resolver(
            Arc::new(PrincipalStoreKeyBindings::new(store.clone(), scope)),
            ChallengeGuard::new(300),
        );
        let c = signed_cred(&v, &blob, |m| key.sig_blob(m));
        let auth = HumanSsoAuthenticator::with_verifier(store, Arc::new(v));
        let p = auth
            .authenticate(&c, None)
            .expect("durable ssh binding resolves the principal");
        assert_eq!(p.principal_id, PrincipalId("svc:deploy".into()));
        assert_eq!(p.tenant, TenantId(TENANT.into()));
    }
}
