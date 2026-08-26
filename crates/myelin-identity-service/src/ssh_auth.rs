use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use crate::principal_store::{PrincipalError, PrincipalStore};
use myelin_identity::{AuthzError, Credential, PrincipalId};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::{STANDARD as B64, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine as _;

fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

fn malformed(msg: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(msg.into())
}

const SSH_AUTH_CONTEXT: &[u8] = b"myelin-ssh-auth-challenge-v1\n";

pub fn signed_payload(nonce: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(SSH_AUTH_CONTEXT.len() + nonce.len());
    m.extend_from_slice(SSH_AUTH_CONTEXT);
    m.extend_from_slice(nonce);
    m
}

struct SshReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> SshReader<'a> {
    fn new(buf: &'a [u8]) -> SshReader<'a> {
        SshReader { buf, pos: 0 }
    }

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

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SshPublicKey {
    Ed25519(Vec<u8>),
    Rsa { e: Vec<u8>, n: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SshSignature {
    alg: SigAlg,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigAlg {
    Ed25519,
    RsaSha256,
    RsaSha512,
}

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
        b"ssh-rsa" => {
            let e = r.read_string()?;
            let n = r.read_string()?;
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
        b"ssh-rsa" => {
            return Err(refuse(
                "weak `ssh-rsa` (RSA/SHA-1) signature algorithm rejected - only rsa-sha2-256 / \
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

pub fn ssh_fingerprint(public_key_blob: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(public_key_blob);
    format!("SHA256:{}", STANDARD_NO_PAD.encode(digest))
}

fn verify_ssh_signature(
    key: &SshPublicKey,
    sig: &SshSignature,
    msg: &[u8],
) -> Result<(), AuthzError> {
    match (key, sig.alg) {
        (SshPublicKey::Ed25519(pk), SigAlg::Ed25519) => {
            use ring::signature::{UnparsedPublicKey, ED25519};
            UnparsedPublicKey::new(&ED25519, pk.as_slice())
                .verify(msg, &sig.bytes)
                .map_err(|_| refuse("ed25519 signature verification failed"))
        }
        (SshPublicKey::Rsa { n, e }, SigAlg::RsaSha256) => verify_rsa_sha256(n, e, msg, &sig.bytes),
        (SshPublicKey::Rsa { n, e }, SigAlg::RsaSha512) => verify_rsa_sha512(n, e, msg, &sig.bytes),
        _ => Err(refuse(
            "signature algorithm does not match the public-key type (alg/key-type mismatch)",
        )),
    }
}

const MIN_RSA_MODULUS_BITS: u64 = 2048;

fn rsa_public_key(n: &[u8], e: &[u8]) -> Result<rsa::RsaPublicKey, AuthzError> {
    use rsa::{BigUint, RsaPublicKey};
    let n_int = BigUint::from_bytes_be(n);
    let bits = n_int.bits() as u64;
    if bits < MIN_RSA_MODULUS_BITS {
        return Err(refuse(format!(
            "rsa key too small: {bits} bits, minimum {MIN_RSA_MODULUS_BITS} (a weaker modulus is \
             factorable offline - fail-closed)"
        )));
    }
    RsaPublicKey::new(n_int, BigUint::from_bytes_be(e))
        .map_err(|err| refuse(format!("invalid RSA public key on the wire: {err}")))
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredKey {
    pub tenant: TenantId,
    pub region: Region,
    pub subject_key: String,
}

#[derive(Clone, Debug, Default)]
pub struct KeyBindingIndex {
    by_fingerprint: BTreeMap<String, RegisteredKey>,
}

impl KeyBindingIndex {
    pub fn new() -> KeyBindingIndex {
        KeyBindingIndex {
            by_fingerprint: BTreeMap::new(),
        }
    }

    pub fn with_binding(
        mut self,
        fingerprint: impl Into<String>,
        binding: RegisteredKey,
    ) -> KeyBindingIndex {
        self.by_fingerprint.insert(fingerprint.into(), binding);
        self
    }

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

    pub fn get(&self, fingerprint: &str) -> Option<&RegisteredKey> {
        self.by_fingerprint.get(fingerprint)
    }

    pub fn len(&self) -> usize {
        self.by_fingerprint.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_fingerprint.is_empty()
    }
}

pub trait KeyBindingResolver: Send + Sync {
    fn resolve(&self, fingerprint: &str) -> Result<Option<RegisteredKey>, KeyBindingLookupError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyBindingLookupError {
    Unavailable,
}

impl core::fmt::Display for KeyBindingLookupError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KeyBindingLookupError::Unavailable => {
                formatter.write_str("SSH key binding directory is unavailable")
            }
        }
    }
}

impl std::error::Error for KeyBindingLookupError {}

type KeyBindingLookup = Result<Option<RegisteredKey>, KeyBindingLookupError>;

impl From<PrincipalError> for KeyBindingLookupError {
    fn from(_: PrincipalError) -> Self {
        KeyBindingLookupError::Unavailable
    }
}

impl KeyBindingResolver for KeyBindingIndex {
    fn resolve(&self, fingerprint: &str) -> KeyBindingLookup {
        Ok(self.get(fingerprint).cloned())
    }
}

#[derive(Clone)]
pub struct PrincipalStoreKeyBindings {
    store: PrincipalStore,
    scope: TenantScope,
}

impl PrincipalStoreKeyBindings {
    pub fn new(store: PrincipalStore, scope: TenantScope) -> PrincipalStoreKeyBindings {
        PrincipalStoreKeyBindings { store, scope }
    }

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

    pub fn register_fingerprint(
        &self,
        fingerprint: &str,
        principal_id: &PrincipalId,
    ) -> Result<(), PrincipalError> {
        self.store
            .link_credential(&self.scope, scheme::SSH, fingerprint, principal_id)
    }

    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }
}

impl KeyBindingResolver for PrincipalStoreKeyBindings {
    fn resolve(&self, fingerprint: &str) -> KeyBindingLookup {
        let row = self
            .store
            .resolve_credential(&self.scope, scheme::SSH, fingerprint)?;
        Ok(row.map(|row| RegisteredKey {
            tenant: row.tenant,
            region: row.region,
            subject_key: fingerprint.to_string(),
        }))
    }
}

type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Clone, Debug)]
pub struct Challenge {
    pub id: String,
    pub nonce: Vec<u8>,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
struct StoredChallenge {
    nonce: Vec<u8>,
    expires_at: i64,
    consumed: bool,
}

#[derive(Clone)]
pub struct ChallengeGuard {
    inner: Arc<Mutex<BTreeMap<String, StoredChallenge>>>,
    ttl_secs: i64,
    now: NowFn,
}

impl ChallengeGuard {
    pub fn new(ttl_secs: i64) -> ChallengeGuard {
        ChallengeGuard {
            inner: Arc::new(Mutex::new(BTreeMap::new())),
            ttl_secs,
            now: Arc::new(crate::clock::unix_seconds),
        }
    }

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

    pub fn consume(&self, id: &str) -> Result<Vec<u8>, AuthzError> {
        let now = self.now();
        let mut map = self.lock();
        let entry = map.get_mut(id).ok_or_else(|| {
            refuse("unknown SSH challenge (not server-issued, or already expired)")
        })?;
        if now > entry.expires_at {
            map.remove(id);
            return Err(refuse(
                "expired SSH challenge (stale - re-issue a fresh challenge)",
            ));
        }
        if entry.consumed {
            return Err(refuse(
                "replayed SSH challenge (this challenge+signature was already presented - replay \
                 defence)",
            ));
        }
        entry.consumed = true;
        Ok(entry.nonce.clone())
    }
}

fn random_bytes(n: usize) -> Result<Vec<u8>, AuthzError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf)
        .map_err(|_| AuthzError::Unavailable("CSPRNG failure issuing SSH challenge".into()))?;
    Ok(buf)
}

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

struct SshEnvelope {
    public_key: String,
    signature: String,
    challenge_id: String,
}

impl SshEnvelope {
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

#[derive(Clone)]
pub struct SshVerifier {
    registry: Arc<dyn KeyBindingResolver>,
    challenges: ChallengeGuard,
}

impl SshVerifier {
    pub fn new(registry: KeyBindingIndex, challenges: ChallengeGuard) -> SshVerifier {
        SshVerifier {
            registry: Arc::new(registry),
            challenges,
        }
    }

    pub fn with_resolver(
        registry: Arc<dyn KeyBindingResolver>,
        challenges: ChallengeGuard,
    ) -> SshVerifier {
        SshVerifier {
            registry,
            challenges,
        }
    }

    pub fn challenges(&self) -> &ChallengeGuard {
        &self.challenges
    }

    pub fn bindings(&self) -> &dyn KeyBindingResolver {
        self.registry.as_ref()
    }
}

impl CredentialVerifier for SshVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        if credential.scheme != scheme::SSH {
            return Err(malformed(format!(
                "SshVerifier received a `{}` credential (expected `ssh`)",
                credential.scheme
            )));
        }

        let env = SshEnvelope::parse(credential.material.trim())?;
        let public_key_blob = B64
            .decode(env.public_key.as_bytes())
            .map_err(|e| malformed(format!("malformed base64 SSH public key: {e}")))?;
        let signature_blob = B64
            .decode(env.signature.as_bytes())
            .map_err(|e| malformed(format!("malformed base64 SSH signature: {e}")))?;

        let key = parse_ssh_public_key(&public_key_blob)?;
        let sig = parse_ssh_signature(&signature_blob)?;

        let fingerprint = ssh_fingerprint(&public_key_blob);
        let binding = self
            .registry
            .resolve(&fingerprint)
            .map_err(|_| {
                AuthzError::Unavailable(
                    "SSH key binding directory is unavailable - authentication fails closed".into(),
                )
            })?
            .ok_or_else(|| {
                refuse(format!(
                    "unregistered SSH key fingerprint `{fingerprint}` (no S1 binding - \
                     fail-closed, never a fabricated principal)"
                ))
            })?;

        let nonce = self.challenges.consume(&env.challenge_id)?;

        let message = signed_payload(&nonce);
        verify_ssh_signature(&key, &sig, &message)?;

        Ok(VerifiedAssertion {
            tenant: binding.tenant.clone(),
            region: binding.region.clone(),
            scheme: scheme::SSH.to_string(),
            subject_key: binding.subject_key.clone(),
            expires_at_unix: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authenticate::StructuralVerifier;
    use crate::oidc::SchemeDispatchVerifier;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

    fn put_string(out: &mut Vec<u8>, s: &[u8]) {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
        out.extend_from_slice(s);
    }

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
        fn sig_blob(&self, msg: &[u8]) -> Vec<u8> {
            let sig = self.pair.sign(msg);
            let mut b = Vec::new();
            put_string(&mut b, b"ssh-ed25519");
            put_string(&mut b, sig.as_ref());
            b
        }
        fn sig_blob_with_alg(&self, alg: &[u8], msg: &[u8]) -> Vec<u8> {
            let sig = self.pair.sign(msg);
            let mut b = Vec::new();
            put_string(&mut b, alg);
            put_string(&mut b, sig.as_ref());
            b
        }
    }

    struct RsaKey {
        priv_key: rsa::RsaPrivateKey,
    }
    impl RsaKey {
        fn generate() -> RsaKey {
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
        fn sig_blob_sha1_legacy(&self, msg: &[u8]) -> Vec<u8> {
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

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

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

    #[test]
    fn negative_forged_signature_by_a_different_key_is_rejected() {
        let victim = EdKey::generate();
        let attacker = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&victim);
        let victim_blob = victim.pubkey_blob();
        let c = signed_cred(&v, &victim_blob, |m| attacker.sig_blob(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a signature by a different key must be refused, got {err:?}"
        );
    }

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

    #[test]
    fn negative_expired_challenge_is_rejected() {
        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let clock = Arc::new(AtomicI64::new(1_000));
        let c2 = clock.clone();
        let challenges = ChallengeGuard::new(300).with_clock(move || c2.load(Ordering::SeqCst));
        let v = SshVerifier::new(registry, challenges);

        let ch = v.challenges().issue().unwrap();
        let sig_blob = key.sig_blob(&signed_payload(&ch.nonce));
        let c = cred(encode_ssh_credential_material(&blob, &sig_blob, &ch.id));
        clock.store(2_000, Ordering::SeqCst);

        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired challenge must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_unregistered_key_is_rejected() {
        let registered = EdKey::generate();
        let stranger = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&registered);
        let stranger_blob = stranger.pubkey_blob();
        let c = signed_cred(&v, &stranger_blob, |m| stranger.sig_blob(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unregistered SSH key")),
            "an unregistered key must be refused, got {err:?}"
        );
    }

    #[test]
    fn a_directory_outage_denies_ssh_without_burning_the_challenge() {
        struct RecoveringDirectory {
            available: Arc<AtomicBool>,
            binding: RegisteredKey,
        }

        impl KeyBindingResolver for RecoveringDirectory {
            fn resolve(&self, fingerprint: &str) -> KeyBindingLookup {
                if !self.available.load(Ordering::SeqCst) {
                    return Err(KeyBindingLookupError::Unavailable);
                }
                Ok((self.binding.subject_key == fingerprint).then(|| self.binding.clone()))
            }
        }

        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let fingerprint = ssh_fingerprint(&blob);
        let available = Arc::new(AtomicBool::new(false));
        let directory = RecoveringDirectory {
            available: available.clone(),
            binding: RegisteredKey {
                tenant: TenantId(TENANT.into()),
                region: Region(REGION.into()),
                subject_key: fingerprint.clone(),
            },
        };
        let verifier = SshVerifier::with_resolver(Arc::new(directory), ChallengeGuard::new(300));
        let credential = signed_cred(&verifier, &blob, |message| key.sig_blob(message));

        let outage = verifier.verify(&credential).unwrap_err();
        assert_eq!(
            outage,
            AuthzError::Unavailable(
                "SSH key binding directory is unavailable - authentication fails closed".into()
            ),
            "a directory outage is an ordinary fail-closed authentication result, not a panic"
        );

        available.store(true, Ordering::SeqCst);
        let assertion = verifier
            .verify(&credential)
            .expect("the same signed challenge remains usable after the directory recovers");
        assert_eq!(assertion.subject_key, fingerprint);
    }

    #[test]
    fn negative_signature_over_a_different_nonce_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
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

    #[test]
    fn negative_weak_ssh_rsa_sha1_is_rejected() {
        let key = RsaKey::generate();
        let blob = key.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        let c = signed_cred(&v, &blob, |m| key.sig_blob_sha1_legacy(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("weak `ssh-rsa`")),
            "the weak SHA-1 ssh-rsa algorithm must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_alg_key_type_mismatch_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        let c = signed_cred(&v, &blob, |m| key.sig_blob_with_alg(b"rsa-sha2-256", m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("alg/key-type mismatch")),
            "an alg/key-type mismatch must be refused, got {err:?}"
        );
    }

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

    #[test]
    fn negative_malformed_wire_is_refused_not_panicking() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let good_blob = key.pubkey_blob();

        let mut cases: Vec<String> = vec![
            String::new(),
            "not json".into(),
            "{}".into(),
            serde_json::json!({"public_key":"!!!","signature":"!!!","challenge_id":"x"})
                .to_string(),
        ];

        let ch = v.challenges().issue().unwrap();
        let truncated_key = &good_blob[..good_blob.len() / 2];
        cases.push(encode_ssh_credential_material(
            truncated_key,
            &key.sig_blob(&signed_payload(&ch.nonce)),
            &ch.id,
        ));
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

    #[test]
    fn negative_tenant_injection_in_the_wrapper_is_ignored() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
        let ch = v.challenges().issue().unwrap();
        let sig_blob = key.sig_blob(&signed_payload(&ch.nonce));
        let material = serde_json::json!({
            "public_key": B64.encode(&blob),
            "signature": B64.encode(&sig_blob),
            "challenge_id": ch.id,
            "tenant": "globex",
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

    #[test]
    fn negative_unknown_challenge_id_is_rejected() {
        let key = EdKey::generate();
        let (v, _) = verifier_with_ed_key(&key);
        let blob = key.pubkey_blob();
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

    #[test]
    fn negative_rsa_forged_by_different_key_is_rejected() {
        let victim = RsaKey::generate();
        let attacker = RsaKey::generate();
        let victim_blob = victim.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&victim_blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        let c = signed_cred(&v, &victim_blob, |m| attacker.sig_blob_sha256(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("rsa-sha2-256 signature verification failed")),
            "an RSA signature by a different key must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_weak_rsa_key_size_is_rejected() {
        let weak = RsaKey::generate_bits(1024);
        let blob = weak.pubkey_blob();
        let registry = KeyBindingIndex::new().with_key_blob(&blob, TENANT, REGION);
        let v = SshVerifier::new(registry, ChallengeGuard::new(300));
        let c = signed_cred(&v, &blob, |m| weak.sig_blob_sha256(m));
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("rsa key too small")),
            "a registered <2048-bit RSA key must be refused (factorable), got {err:?}"
        );
    }

    #[test]
    fn dispatch_routes_ssh_to_real_verifier_and_others_to_fallback() {
        let victim = EdKey::generate();
        let attacker = EdKey::generate();
        let (ssh_v, _) = verifier_with_ed_key(&victim);
        let victim_blob = victim.pubkey_blob();
        let forged = signed_cred(&ssh_v, &victim_blob, |m| attacker.sig_blob(m));

        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::SSH, Arc::new(ssh_v));

        assert!(
            dispatch.verify(&forged).is_err(),
            "a forged ssh credential must reach the real verifier and be refused"
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

    #[test]
    fn end_to_end_through_authenticator_resolves_registered_principal() {
        use crate::authenticate::HumanSsoAuthenticator;
        use crate::principal_store::PrincipalStore;
        use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
        use myelin_storage::{KmsEngine, TenantScope};

        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let fp = ssh_fingerprint(&blob);

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

    #[test]
    fn durable_binding_resolves_and_survives_a_fresh_verifier() {
        let key = EdKey::generate();
        let blob = key.pubkey_blob();
        let fp = ssh_fingerprint(&blob);

        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = admin_scope(TENANT, REGION);
        seed_service(&store, &scope, "svc:deploy");

        let bindings = PrincipalStoreKeyBindings::new(store.clone(), scope.clone());
        let registered_fp = bindings
            .register_key(&blob, &PrincipalId("svc:deploy".into()))
            .unwrap();
        assert_eq!(registered_fp, fp);

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

    #[test]
    fn durable_binding_refuses_an_unregistered_key() {
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = admin_scope(TENANT, REGION);
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

    #[test]
    fn durable_binding_is_tenant_scoped_no_cross_tenant_resolution() {
        let key = EdKey::generate();
        let blob = key.pubkey_blob();

        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let acme = admin_scope("acme", REGION);
        seed_service(&store, &acme, "svc:deploy");
        PrincipalStoreKeyBindings::new(store.clone(), acme)
            .register_key(&blob, &PrincipalId("svc:deploy".into()))
            .unwrap();

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
