use crate::authenticate::{scheme, CredentialVerifier, VerifiedAssertion};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ciborium::value::Value as Cbor;
use sha2::{Digest, Sha256};

fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

fn malformed(msg: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(msg.into())
}

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;
const FLAG_ED: u8 = 0x80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoseKey {
    Es256 {
        x: Vec<u8>,
        y: Vec<u8>,
    },
    Rs256 {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    Ed25519 {
        x: Vec<u8>,
    },
}

impl CoseKey {
    fn cose_alg(&self) -> i128 {
        match self {
            CoseKey::Es256 { .. } => -7,
            CoseKey::Rs256 { .. } => -257,
            CoseKey::Ed25519 { .. } => -8,
        }
    }
}

fn cbor_map_int(map: &[(Cbor, Cbor)], label: i128) -> Option<&Cbor> {
    map.iter().find_map(|(k, v)| match k {
        Cbor::Integer(i) if i128::from(*i) == label => Some(v),
        _ => None,
    })
}

fn cbor_map_text<'a>(map: &'a [(Cbor, Cbor)], label: &str) -> Option<&'a Cbor> {
    map.iter().find_map(|(k, v)| match k {
        Cbor::Text(t) if t == label => Some(v),
        _ => None,
    })
}

fn cbor_bytes(v: &Cbor, what: &str) -> Result<Vec<u8>, AuthzError> {
    match v {
        Cbor::Bytes(b) => Ok(b.clone()),
        _ => Err(malformed(format!("CBOR `{what}` is not a byte string"))),
    }
}

fn cbor_int(v: &Cbor, what: &str) -> Result<i128, AuthzError> {
    match v {
        Cbor::Integer(i) => Ok(i128::from(*i)),
        _ => Err(malformed(format!("CBOR `{what}` is not an integer"))),
    }
}

fn parse_cose_key(map: &[(Cbor, Cbor)]) -> Result<CoseKey, AuthzError> {
    let kty = cbor_int(
        cbor_map_int(map, 1).ok_or_else(|| malformed("COSE key missing `kty` (label 1)"))?,
        "kty",
    )?;
    let alg = cbor_int(
        cbor_map_int(map, 3).ok_or_else(|| malformed("COSE key missing `alg` (label 3)"))?,
        "alg",
    )?;
    match kty {
        2 => {
            if alg != -7 {
                return Err(refuse(format!(
                    "COSE EC2 key declares alg {alg} (expected −7 / ES256 - alg-confusion pin)"
                )));
            }
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
        1 => {
            if alg != -8 {
                return Err(refuse(format!(
                    "COSE OKP key declares alg {alg} (expected −8 / EdDSA - alg-confusion pin)"
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
        3 => {
            if alg != -257 {
                return Err(refuse(format!(
                    "COSE RSA key declares alg {alg} (expected −257 / RS256 - alg-confusion pin)"
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

const MIN_RSA_MODULUS_BITS: u64 = 2048;

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
                     offline - fail-closed)"
                )));
            }
            let pubkey = RsaPublicKey::new(n_int, BigUint::from_bytes_be(e))
                .map_err(|err| refuse(format!("invalid RSA COSE key: {err}")))?;
            let vk = VerifyingKey::<Sha256>::new(pubkey);
            let signature = Signature::try_from(sig)
                .map_err(|_| refuse("malformed RS256 signature encoding"))?;
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

#[derive(Clone, Debug)]
struct AuthData {
    rp_id_hash: [u8; 32],
    flags: u8,
    sign_count: u32,
    attested: Option<(Vec<u8>, CoseKey)>,
}

impl AuthData {
    fn up(&self) -> bool {
        self.flags & FLAG_UP != 0
    }
    fn uv(&self) -> bool {
        self.flags & FLAG_UV != 0
    }

    fn parse(buf: &[u8]) -> Result<AuthData, AuthzError> {
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
            let rest = buf
                .get(pos..)
                .ok_or_else(|| malformed("authData: ED flag set but no extensions follow"))?;
            let mut cursor = std::io::Cursor::new(rest);
            let _ext: Cbor = ciborium::from_reader(&mut cursor)
                .map_err(|e| malformed(format!("authData: malformed extensions CBOR: {e}")))?;
            pos = pos.saturating_add(cursor.position() as usize);
        }
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

struct ClientData {
    type_: String,
    challenge: String,
    origin: String,
    cross_origin: bool,
}

impl ClientData {
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
            cross_origin: v
                .get("crossOrigin")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
        })
    }
}

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
            now: Arc::new(system_now),
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

    pub fn consume(&self, value: &str) -> Result<(), AuthzError> {
        let now = self.now();
        let mut map = self.lock();
        let entry = map.get_mut(value).ok_or_else(|| {
            refuse("unknown WebAuthn challenge (not server-issued, or already expired)")
        })?;
        if now > entry.expires_at {
            map.remove(value);
            return Err(refuse(
                "expired WebAuthn challenge (stale - re-issue a fresh challenge)",
            ));
        }
        if entry.consumed {
            return Err(refuse(
                "replayed WebAuthn challenge (this assertion was already presented - replay defence)",
            ));
        }
        entry.consumed = true;
        Ok(())
    }
}

fn random_bytes(n: usize) -> Result<Vec<u8>, AuthzError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; n];
    rng.fill(&mut buf)
        .map_err(|_| AuthzError::Unavailable("CSPRNG failure issuing WebAuthn challenge".into()))?;
    Ok(buf)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredCredential {
    cose_key: CoseKey,
    tenant: TenantId,
    region: Region,
    subject_key: String,
    sign_count: u32,
}

#[derive(Clone, Default)]
pub struct CredentialBindingIndex {
    inner: Arc<Mutex<BTreeMap<Vec<u8>, StoredCredential>>>,
}

impl CredentialBindingIndex {
    pub fn new() -> CredentialBindingIndex {
        CredentialBindingIndex {
            inner: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<Vec<u8>, StoredCredential>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    pub fn sign_count(&self, credential_id: &[u8]) -> Option<u32> {
        self.lock().get(credential_id).map(|c| c.sign_count)
    }

    fn compare_and_advance(
        &self,
        credential_id: &[u8],
        verified: &StoredCredential,
        presented: u32,
    ) -> myelin_identity::Result<StoredCredential> {
        let mut credentials = self.lock();
        let current = credentials.get_mut(credential_id).ok_or_else(|| {
            refuse("passkey credential binding disappeared during assertion verification")
        })?;
        if current.cose_key != verified.cose_key
            || current.tenant != verified.tenant
            || current.region != verified.region
            || current.subject_key != verified.subject_key
        {
            return Err(refuse(
                "passkey credential binding changed during assertion verification",
            ));
        }

        let stored = current.sign_count;
        if presented == 0 && stored == 0 {
        } else if presented > stored {
            current.sign_count = presented;
        } else {
            return Err(refuse(format!(
                "signature counter regression: presented {presented} ≤ stored {stored} \
                 (cloned authenticator / replay - fail-closed)"
            )));
        }
        Ok(current.clone())
    }

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

#[derive(Clone, Debug)]
pub struct WebauthnConfig {
    pub rp_id: String,
    pub origins: BTreeSet<String>,
    pub require_user_verification: bool,
    pub allow_cross_origin: bool,
}

impl WebauthnConfig {
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

    pub fn requiring_user_verification(mut self) -> WebauthnConfig {
        self.require_user_verification = true;
        self
    }

    fn rp_id_hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(self.rp_id.as_bytes());
        h.finalize().into()
    }

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
        challenges.consume(&cd.challenge)?;
        Ok(cd)
    }
}

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

pub fn encode_registration_material(client_data_json: &[u8], attestation_object: &[u8]) -> String {
    serde_json::json!({
        "client_data_json": B64.encode(client_data_json),
        "attestation_object": B64.encode(attestation_object),
    })
    .to_string()
}

fn env_b64(v: &serde_json::Value, name: &str) -> Result<Vec<u8>, AuthzError> {
    let s = v
        .get(name)
        .and_then(|x| x.as_str())
        .ok_or_else(|| malformed(format!("WebAuthn envelope missing `{name}`")))?;
    B64.decode(s.as_bytes()).map_err(|e| {
        malformed(format!(
            "WebAuthn envelope `{name}` is not valid base64: {e}"
        ))
    })
}

#[derive(Clone)]
pub struct WebauthnVerifier {
    config: WebauthnConfig,
    registry: CredentialBindingIndex,
    challenges: ChallengeGuard,
    #[cfg(test)]
    counter_barrier: Option<Arc<std::sync::Barrier>>,
}

impl WebauthnVerifier {
    pub fn new(
        config: WebauthnConfig,
        registry: CredentialBindingIndex,
        challenges: ChallengeGuard,
    ) -> WebauthnVerifier {
        WebauthnVerifier {
            config,
            registry,
            challenges,
            #[cfg(test)]
            counter_barrier: None,
        }
    }

    #[cfg(test)]
    fn with_counter_barrier(mut self, barrier: Arc<std::sync::Barrier>) -> Self {
        self.counter_barrier = Some(barrier);
        self
    }

    pub fn challenges(&self) -> &ChallengeGuard {
        &self.challenges
    }

    pub fn registry(&self) -> &CredentialBindingIndex {
        &self.registry
    }

    pub fn register(
        &self,
        material: &str,
        tenant: &TenantId,
        region: &Region,
    ) -> myelin_identity::Result<Vec<u8>> {
        let env: serde_json::Value = serde_json::from_str(material.trim()).map_err(|e| {
            malformed(format!(
                "malformed WebAuthn registration envelope JSON: {e}"
            ))
        })?;
        let raw_client_data = env_b64(&env, "client_data_json")?;
        let attestation_object = env_b64(&env, "attestation_object")?;

        let _cd = self.config.validate_client_data(
            &raw_client_data,
            "webauthn.create",
            &self.challenges,
        )?;
        let client_data_hash = Sha256::digest(&raw_client_data);

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

        let att_stmt = cbor_map_text(att_map, "attStmt");
        match fmt.as_str() {
            "none" => {
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
                if cbor_map_text(stmt, "x5c").is_some() {
                    return Err(refuse(
                        "packed FULL attestation (x5c) is not supported yet - the X.509 attestation-cert \
                         chain-to-root verification is deferred (refused, never faked). Use `none` or \
                         packed self attestation.",
                    ));
                }
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
                verify_cose_signature(&cose_key, &signed, &sig)
                    .map_err(|_| refuse("packed self-attestation signature verification failed"))?;
            }
            other => {
                return Err(refuse(format!(
                    "attestation format `{other}` is not supported (only `none` + packed self; \
                     tpm/android-key/android-safetynet/apple/fido-u2f are deferred - refused, not faked)"
                )));
            }
        }

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
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
        if credential.scheme != scheme::PASSKEY {
            return Err(malformed(format!(
                "WebauthnVerifier received a `{}` credential (expected `passkey`)",
                credential.scheme
            )));
        }

        let env: serde_json::Value = serde_json::from_str(credential.material.trim())
            .map_err(|e| malformed(format!("malformed WebAuthn assertion envelope JSON: {e}")))?;
        let credential_id = env_b64(&env, "credential_id")?;
        let raw_client_data = env_b64(&env, "client_data_json")?;
        let authenticator_data = env_b64(&env, "authenticator_data")?;
        let signature = env_b64(&env, "signature")?;

        let stored = self
            .registry
            .lock()
            .get(&credential_id)
            .cloned()
            .ok_or_else(|| {
                refuse(
                    "unregistered passkey credential id (no S1 binding - fail-closed, never a \
                     fabricated principal)",
                )
            })?;

        let _cd =
            self.config
                .validate_client_data(&raw_client_data, "webauthn.get", &self.challenges)?;

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

        let client_data_hash = Sha256::digest(&raw_client_data);
        let mut signed = Vec::with_capacity(authenticator_data.len() + client_data_hash.len());
        signed.extend_from_slice(&authenticator_data);
        signed.extend_from_slice(&client_data_hash);
        verify_cose_signature(&stored.cose_key, &signed, &signature)?;

        #[cfg(test)]
        if let Some(barrier) = &self.counter_barrier {
            barrier.wait();
        }

        let presented = auth_data.sign_count;
        let accepted = self
            .registry
            .compare_and_advance(&credential_id, &stored, presented)?;

        Ok(VerifiedAssertion {
            tenant: accepted.tenant,
            region: accepted.region,
            scheme: scheme::PASSKEY.to_string(),
            subject_key: accepted.subject_key,
            expires_at_unix: None,
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

    const RP_ID: &str = "example.com";
    const ORIGIN: &str = "https://example.com";
    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

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
            let pt = self.pair.public_key().as_ref();
            assert_eq!(pt.len(), 65);
            encode_cbor(&Cbor::Map(vec![
                (ci(1), ci(2)),
                (ci(3), ci(-7)),
                (ci(-1), ci(1)),
                (ci(-2), cbytes(&pt[1..33])),
                (ci(-3), cbytes(&pt[33..65])),
            ]))
        }
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            self.pair
                .sign(&self.rng, msg)
                .expect("ec sign")
                .as_ref()
                .to_vec()
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

    fn rp_id_hash(rp: &str) -> [u8; 32] {
        Sha256::digest(rp.as_bytes()).into()
    }

    fn assertion_auth_data(rp: &str, flags: u8, count: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&rp_id_hash(rp));
        b.push(flags);
        b.extend_from_slice(&count.to_be_bytes());
        b
    }

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
        b.extend_from_slice(&[0u8; 16]);
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

    fn attestation_object_none(auth_data: &[u8]) -> Vec<u8> {
        encode_cbor(&Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), cbytes(auth_data)),
        ]))
    }

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

    fn config() -> WebauthnConfig {
        WebauthnConfig::new(RP_ID, [ORIGIN])
    }

    fn fresh_verifier() -> WebauthnVerifier {
        WebauthnVerifier::new(
            config(),
            CredentialBindingIndex::new(),
            ChallengeGuard::new(300),
        )
    }

    fn cred(material: String) -> Credential {
        Credential {
            scheme: scheme::PASSKEY.into(),
            material,
        }
    }

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

    fn positive_for(key: AuthKey) {
        let cred_id = b"cred-positive-001";
        let v = registered_none(&key, cred_id, 0);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        let a = v
            .verify(&c)
            .expect("a correctly-signed assertion must verify");
        assert_eq!(a.tenant, TenantId(TENANT.into()));
        assert_eq!(a.region, Region(REGION.into()));
        assert_eq!(a.scheme, scheme::PASSKEY);
        assert_eq!(a.subject_key, URL_SAFE_NO_PAD.encode(cred_id));
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

    #[test]
    fn positive_packed_self_registration_then_assertion() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-packed-self-1";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let att = attestation_object_packed_self(&ad, &Sha256::digest(&cd), &key);
        let material = encode_registration_material(&cd, &att);
        v.register(&material, &TenantId(TENANT.into()), &Region(REGION.into()))
            .expect("packed self registration must succeed");
        assert_eq!(v.registry().len(), 1);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        let a = v
            .verify(&c)
            .expect("assertion after packed-self registration must verify");
        assert_eq!(a.subject_key, URL_SAFE_NO_PAD.encode(cred_id));
        assert_eq!(a.tenant, TenantId(TENANT.into()));
    }

    #[test]
    fn positive_zero_counter_authenticator_is_accepted() {
        let key = AuthKey::Ed25519(EdKey::generate());
        let cred_id = b"cred-zero-counter";
        let v = registered_none(&key, cred_id, 0);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 0);
        v.verify(&c).expect("a 0/0 counter assertion must verify");
        assert_eq!(v.registry().sign_count(cred_id), Some(0));
    }

    #[test]
    fn negative_forged_signature_by_a_different_key_is_rejected() {
        let victim = AuthKey::Es256(EcKey::generate());
        let attacker = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-forge";
        let v = registered_none(&victim, cred_id, 0);
        let c = signed_assertion(&v, &attacker, cred_id, FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "a signature by a different key must be refused, got {err:?}"
        );
    }

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

    #[test]
    fn negative_unknown_challenge_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-unknown-ch";
        let v = registered_none(&key, cred_id, 0);
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

    #[test]
    fn negative_wrong_origin_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-origin";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
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

    #[test]
    fn negative_wrong_rp_id_hash_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-rpid";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
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

    #[test]
    fn negative_user_present_flag_clear_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-up";
        let v = registered_none(&key, cred_id, 0);
        let c = signed_assertion(&v, &key, cred_id, 0x00, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("User-Present")),
            "a UP-clear assertion must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_user_verification_required_but_clear_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-uv";
        let v = WebauthnVerifier::new(
            config().requiring_user_verification(),
            CredentialBindingIndex::new(),
            ChallengeGuard::new(300),
        );
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP | FLAG_UV, 0, cred_id, &key.cose_key_cbor());
        v.register(
            &encode_registration_material(&cd, &attestation_object_none(&ad)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        )
        .unwrap();
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("User-Verified")),
            "a UV-required-but-clear assertion must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_counter_regression_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-counter";
        let v = registered_none(&key, cred_id, 5);
        let c = signed_assertion(&v, &key, cred_id, FLAG_UP, 3);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("counter regression")),
            "a counter regression must be refused, got {err:?}"
        );
        assert_eq!(v.registry().sign_count(cred_id), Some(5));
        let c_eq = signed_assertion(&v, &key, cred_id, FLAG_UP, 5);
        let err = v.verify(&c_eq).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("counter regression")),
            "an equal counter must be refused, got {err:?}"
        );
    }

    #[test]
    fn concurrent_equal_counters_authenticate_exactly_once() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-counter-race";
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let verifier = registered_none(&key, cred_id, 7).with_counter_barrier(barrier);
        let first = signed_assertion(&verifier, &key, cred_id, FLAG_UP, 8);
        let second = signed_assertion(&verifier, &key, cred_id, FLAG_UP, 8);
        let verifier = Arc::new(verifier);

        let threads = [first, second].map(|assertion| {
            let verifier = Arc::clone(&verifier);
            std::thread::spawn(move || verifier.verify(&assertion))
        });
        let outcomes =
            threads.map(|thread| thread.join().expect("verification thread must not panic"));

        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "only one assertion can claim the same counter advance: {outcomes:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Err(AuthzError::FailClosed(message)) if message.contains("counter regression")))
                .count(),
            1,
            "the losing assertion fails closed as a clone/replay signal: {outcomes:?}"
        );
        assert_eq!(verifier.registry().sign_count(cred_id), Some(8));
    }

    #[test]
    fn negative_signature_over_different_authdata_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-diff-ad";
        let v = registered_none(&key, cred_id, 0);
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
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

    #[test]
    fn negative_alg_confusion_es256_key_rsa_signature_is_rejected() {
        let es = AuthKey::Es256(EcKey::generate());
        let rsa = AuthKey::Rs256(RsaKey::generate());
        let cred_id = b"cred-algconf";
        let v = registered_none(&es, cred_id, 0);
        let c = signed_assertion(&v, &rsa, cred_id, FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("ES256 signature verification failed")),
            "an alg-confused (RSA-for-ES256) signature must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_malformed_inputs_are_refused_not_panicking() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-malformed";
        let v = registered_none(&key, cred_id, 0);

        for bad in ["", "not json", "{}", r#"{"credential_id":"!!!"}"#] {
            let r = v.verify(&cred(bad.to_string()));
            assert!(
                r.is_err(),
                "malformed envelope `{bad}` must be refused (not panic)"
            );
        }

        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        let c = cred(encode_assertion_material(
            cred_id,
            &cd,
            b"\x00\x01\x02",
            b"sig",
        ));
        assert!(v.verify(&c).is_err(), "truncated authData must be refused");

        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let c = cred(encode_assertion_material(
            cred_id,
            b"\xff\xff not json",
            &ad,
            b"sig",
        ));
        assert!(
            v.verify(&c).is_err(),
            "garbage clientDataJSON must be refused"
        );

        let mut ad_huge = Vec::new();
        ad_huge.extend_from_slice(&rp_id_hash(RP_ID));
        ad_huge.push(FLAG_UP | FLAG_AT);
        ad_huge.extend_from_slice(&0u32.to_be_bytes());
        ad_huge.extend_from_slice(&[0u8; 16]);
        ad_huge.extend_from_slice(&0xFFFFu16.to_be_bytes());
        let v2 = fresh_verifier();
        let challenge2 = v2.challenges().issue().unwrap();
        let cd2 = client_data("webauthn.create", &challenge2, ORIGIN);
        let r = v2.register(
            &encode_registration_material(&cd2, &attestation_object_none(&ad_huge)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        assert!(r.is_err(), "a huge credIdLen must be refused (not panic)");

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
        let material = serde_json::json!({
            "credential_id": B64.encode(cred_id),
            "client_data_json": B64.encode(&cd),
            "authenticator_data": B64.encode(&ad),
            "signature": B64.encode(&sig),
            "tenant": "globex",
            "region": "us-east",
        })
        .to_string();
        let a = v
            .verify(&cred(material))
            .expect("the assertion itself is valid");
        assert_eq!(
            a.tenant,
            TenantId(TENANT.into()),
            "the resolved tenant is the REGISTERED binding's (acme), never the wrapper's (globex)"
        );
        assert_eq!(a.region, Region(REGION.into()));
    }

    #[test]
    fn negative_unregistered_credential_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let v = registered_none(&key, b"cred-known", 0);
        let c = signed_assertion(&v, &key, b"cred-UNKNOWN", FLAG_UP, 1);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("unregistered passkey")),
            "an unregistered credential must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_registration_invalid_packed_self_attestation_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let attacker = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-bad-att";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.create", &challenge, ORIGIN);
        let ad = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        let att = attestation_object_packed_self(&ad, &Sha256::digest(&cd), &attacker);
        let r = v.register(
            &att_material(&cd, &att),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("self-attestation")),
            "an invalid packed self-attestation must be refused, got {err:?}"
        );
        assert_eq!(
            v.registry().len(),
            0,
            "no binding stored on a failed attestation"
        );
    }

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
        let r = v.register(
            &att_material(&cd, &att),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("FULL attestation")),
            "packed full (x5c) must be refused-as-unsupported, got {err:?}"
        );
    }

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
        let r = v.register(
            &att_material(&cd, &att),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        );
        let err = r.unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("not supported")),
            "an unsupported format must be refused, got {err:?}"
        );
    }

    #[test]
    fn negative_registration_wrong_type_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-wrongtype";
        let v = fresh_verifier();
        let challenge = v.challenges().issue().unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
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

    #[test]
    fn negative_registration_wrong_origin_and_challenge_are_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-reg-origin";
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

    #[test]
    fn dispatch_routes_passkey_to_real_verifier_and_others_to_fallback() {
        let victim = AuthKey::Es256(EcKey::generate());
        let attacker = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-dispatch";
        let webauthn = registered_none(&victim, cred_id, 0);
        let forged = signed_assertion(&webauthn, &attacker, cred_id, FLAG_UP, 1);

        let dispatch = SchemeDispatchVerifier::new(Arc::new(StructuralVerifier::new()))
            .route(scheme::PASSKEY, Arc::new(webauthn));

        assert!(
            dispatch.verify(&forged).is_err(),
            "a forged passkey assertion must hit the real verifier and be refused"
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
    fn negative_expired_challenge_is_rejected() {
        let key = AuthKey::Es256(EcKey::generate());
        let cred_id = b"cred-expiry";
        let clock = Arc::new(AtomicI64::new(1_000));
        let c2 = clock.clone();
        let challenges = ChallengeGuard::new(300).with_clock(move || c2.load(Ordering::SeqCst));
        let v = WebauthnVerifier::new(config(), CredentialBindingIndex::new(), challenges);
        let challenge = v.challenges().issue().unwrap();
        let cd_reg = client_data("webauthn.create", &v.challenges().issue().unwrap(), ORIGIN);
        let ad_reg = registration_auth_data(RP_ID, FLAG_UP, 0, cred_id, &key.cose_key_cbor());
        v.register(
            &encode_registration_material(&cd_reg, &attestation_object_none(&ad_reg)),
            &TenantId(TENANT.into()),
            &Region(REGION.into()),
        )
        .unwrap();
        let cd = client_data("webauthn.get", &challenge, ORIGIN);
        let ad = assertion_auth_data(RP_ID, FLAG_UP, 1);
        let mut signed = Vec::new();
        signed.extend_from_slice(&ad);
        signed.extend_from_slice(&Sha256::digest(&cd));
        let sig = key.sign(&signed);
        let c = cred(encode_assertion_material(cred_id, &cd, &ad, &sig));
        clock.store(2_000, Ordering::SeqCst);
        let err = v.verify(&c).unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
            "an expired challenge must be refused, got {err:?}"
        );
    }
}
