use crate::machine_auth::{
    Authority, CapabilityToken, CredentialAudience, CredentialPurpose, MachineKind,
};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use myelin_identity::{AuthzError, Credential};
use myelin_tenancy::{Region, TenantId};
use ring::signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type HmacSha256 = Hmac<Sha256>;

const V4_PUBLIC_HEADER: &str = "v4.public.";
const DPOP_HEADER: &str = "dpop.v1.";
const MACAROON_DOMAIN: &[u8] = b"myelin.cap.macaroon.v1";
const SHA256_BYTES: usize = 32;

fn b64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
fn b64url_decode(s: &str) -> Result<Vec<u8>, AuthzError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|e| AuthzError::BadRequest(format!("malformed base64url segment: {e}")))
}

fn refuse(msg: impl Into<String>) -> AuthzError {
    AuthzError::FailClosed(msg.into())
}

fn le64(n: u64) -> [u8; 8] {
    let mut out = n.to_le_bytes();
    out[7] &= 0x7f;
    out
}

fn pae(pieces: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&le64(pieces.len() as u64));
    for p in pieces {
        out.extend_from_slice(&le64(p.len() as u64));
        out.extend_from_slice(p);
    }
    out
}

fn paseto_v4_public_sign(key: &Ed25519KeyPair, claims: &[u8]) -> String {
    let m2 = pae(&[V4_PUBLIC_HEADER.as_bytes(), claims, b"", b""]);
    let sig = key.sign(&m2);
    let mut body = claims.to_vec();
    body.extend_from_slice(sig.as_ref());
    format!("{V4_PUBLIC_HEADER}{}", b64url_encode(&body))
}

fn paseto_v4_public_verify(
    public_key: &[u8],
    token: &str,
) -> Result<(Vec<u8>, Vec<u8>), AuthzError> {
    let rest = token.strip_prefix(V4_PUBLIC_HEADER).ok_or_else(|| {
        AuthzError::BadRequest("not a v4.public token (bad header/version)".into())
    })?;
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
        .map_err(|_| {
            refuse("capability-token signature verification failed (forged or tampered)")
        })?;
    Ok((claims.to_vec(), sig.to_vec()))
}

pub struct CellTokenAuthority {
    signing_key: Ed25519KeyPair,
    public_key: [u8; 32],
    mac_key: [u8; 32],
}

impl CellTokenAuthority {
    pub fn from_seed(
        ed25519_seed: &[u8; 32],
        mac_key: &[u8; 32],
    ) -> Result<CellTokenAuthority, AuthzError> {
        let signing_key = Ed25519KeyPair::from_seed_unchecked(ed25519_seed)
            .map_err(|e| AuthzError::BadRequest(format!("invalid Ed25519 cell seed: {e}")))?;
        use ring::signature::KeyPair;
        let mut public_key = [0u8; 32];
        let pk = signing_key.public_key().as_ref();
        if pk.len() != 32 {
            return Err(AuthzError::BadRequest(
                "unexpected Ed25519 public-key length".into(),
            ));
        }
        public_key.copy_from_slice(pk);
        Ok(CellTokenAuthority {
            signing_key,
            public_key,
            mac_key: *mac_key,
        })
    }

    pub fn generate() -> CellTokenAuthority {
        use ring::rand::SecureRandom;
        let rng = ring::rand::SystemRandom::new();
        let mut seed = [0u8; 32];
        let mut mac = [0u8; 32];
        rng.fill(&mut seed)
            .expect("OS CSPRNG fills the Ed25519 seed");
        rng.fill(&mut mac)
            .expect("OS CSPRNG fills the macaroon secret");
        CellTokenAuthority::from_seed(&seed, &mac)
            .expect("a random 32-byte Ed25519 seed is always a valid cell authority")
    }

    pub fn from_material(
        material: &myelin_storage::CellRootMaterial,
    ) -> Result<CellTokenAuthority, AuthzError> {
        CellTokenAuthority::from_seed(&material.ed25519_seed, &material.mac_key)
    }

    pub fn trust_anchor(&self) -> CellTrustAnchor {
        CellTrustAnchor {
            public_key: self.public_key,
            mac_key: self.mac_key,
        }
    }

    pub fn mint(&self, spec: &CapabilityMintSpec) -> String {
        let mut claims = serde_json::Map::new();
        claims.insert("tenant".into(), spec.tenant.clone().into());
        claims.insert("region".into(), spec.region.clone().into());
        claims.insert("sub".into(), spec.subject_key.clone().into());
        claims.insert("jti".into(), spec.jti.clone().into());
        claims.insert("exp".into(), spec.exp_unix.into());
        claims.insert("purpose".into(), spec.purpose.claim().into());
        claims.insert("aud".into(), spec.audience.claim().into());
        if let Some(run_id) = spec.purpose.run_id() {
            claims.insert("run_id".into(), run_id.into());
        }
        if let CredentialPurpose::AgentRun {
            delegation_snapshot: Some(snapshot),
            ..
        } = &spec.purpose
        {
            claims.insert("delegation_snapshot".into(), (*snapshot).into());
        }
        let auth: Vec<serde_json::Value> = spec
            .authority
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|g| serde_json::Value::String(g.clone()))
            .collect();
        claims.insert("auth".into(), auth.into());
        if let Some(jkt) = &spec.dpop_jkt {
            let mut cnf = serde_json::Map::new();
            cnf.insert("jkt".into(), jkt.clone().into());
            claims.insert("cnf".into(), cnf.into());
        }
        let claims_bytes =
            serde_json::to_vec(&serde_json::Value::Object(claims)).expect("claims serialize");
        let paseto = paseto_v4_public_sign(&self.signing_key, &claims_bytes);
        let sig = paseto_root_signature(&paseto).expect("freshly-minted token has a signature");
        let tail = macaroon_root_tag(&self.mac_key, &sig);
        encode_material(&paseto, &[], &tail, None)
    }
}

#[derive(Clone)]
pub struct CellTrustAnchor {
    public_key: [u8; 32],
    mac_key: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct CapabilityMintSpec {
    pub tenant: String,
    pub region: String,
    pub subject_key: String,
    pub jti: String,
    pub exp_unix: i64,
    pub authority: Vec<String>,
    pub dpop_jkt: Option<String>,
    pub purpose: CredentialPurpose,
    pub audience: CredentialAudience,
}

fn encode_material(
    paseto: &str,
    caveats: &[BTreeSet<String>],
    tail: &[u8],
    dpop: Option<&str>,
) -> String {
    let caveats_json: Vec<Vec<String>> = caveats
        .iter()
        .map(|c| c.iter().cloned().collect())
        .collect();
    let caveats_b64 = b64url_encode(&serde_json::to_vec(&caveats_json).expect("caveats serialize"));
    let tail_b64 = b64url_encode(tail);
    match dpop {
        Some(d) => format!(
            "{paseto}|{caveats_b64}|{tail_b64}|{}",
            b64url_encode(d.as_bytes())
        ),
        None => format!("{paseto}|{caveats_b64}|{tail_b64}"),
    }
}

fn paseto_root_signature(paseto: &str) -> Result<Vec<u8>, AuthzError> {
    let rest = paseto
        .strip_prefix(V4_PUBLIC_HEADER)
        .ok_or_else(|| AuthzError::BadRequest("not a v4.public token".into()))?;
    let body = b64url_decode(rest.split('.').next().unwrap_or(""))?;
    if body.len() < 64 {
        return Err(AuthzError::BadRequest(
            "token body too short for a signature".into(),
        ));
    }
    Ok(body[body.len() - 64..].to_vec())
}

fn macaroon_root_tag(mac_key: &[u8; 32], sig: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(mac_key).expect("HMAC accepts a 32-byte key");
    mac.update(MACAROON_DOMAIN);
    mac.update(sig);
    mac.finalize().into_bytes().to_vec()
}

fn caveat_bytes(caveat: &BTreeSet<String>) -> Vec<u8> {
    let sorted: Vec<&String> = caveat.iter().collect();
    serde_json::to_vec(&sorted).expect("caveat serialize")
}

fn macaroon_fold(prev_tag: &[u8], caveat: &BTreeSet<String>) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(prev_tag).expect("HMAC accepts any key length");
    mac.update(&caveat_bytes(caveat));
    mac.finalize().into_bytes().to_vec()
}

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
                "malformed capability credential (expected `<paseto>|<caveats>|<tail>[|<dpop>]`)"
                    .into(),
            ));
        }
        let paseto = parts[0].to_string();
        if !paseto.starts_with(V4_PUBLIC_HEADER) {
            return Err(AuthzError::BadRequest(
                "credential token is not a v4.public PASETO".into(),
            ));
        }
        let caveats_raw: serde_json::Value = serde_json::from_slice(&b64url_decode(parts[1])?)
            .map_err(|e| AuthzError::BadRequest(format!("malformed caveat chain: {e}")))?;
        let caveats = match caveats_raw {
            serde_json::Value::Array(arr) => {
                let mut out = Vec::with_capacity(arr.len());
                for c in arr {
                    let set = parse_grant_set(&c, "caveat").map_err(AuthzError::BadRequest)?;
                    out.push(set);
                }
                out
            }
            _ => {
                return Err(AuthzError::BadRequest(
                    "the caveat chain must be a JSON array".into(),
                ))
            }
        };
        let tail = b64url_decode(parts[2])?;
        if tail.len() != SHA256_BYTES {
            return Err(AuthzError::BadRequest(format!(
                "capability caveat-chain tag must be {SHA256_BYTES} bytes"
            )));
        }
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

pub struct DpopClientKey {
    key: Ed25519KeyPair,
    public_key: [u8; 32],
}

impl DpopClientKey {
    pub fn from_seed(seed: &[u8; 32]) -> Result<DpopClientKey, AuthzError> {
        let key = Ed25519KeyPair::from_seed_unchecked(seed)
            .map_err(|e| AuthzError::BadRequest(format!("invalid DPoP client seed: {e}")))?;
        use ring::signature::KeyPair;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(key.public_key().as_ref());
        Ok(DpopClientKey { key, public_key })
    }

    pub fn jkt(&self) -> String {
        dpop_jkt(&self.public_key)
    }

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

fn dpop_jkt(public_key: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(public_key);
    b64url_encode(&h.finalize())
}

#[derive(Clone, Debug)]
pub struct DpopBinding {
    pub htm: String,
    pub htu: String,
}

#[derive(Clone)]
pub struct DpopReplayGuard {
    replay: crate::oidc::ReplayGuard,
}

impl Default for DpopReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl DpopReplayGuard {
    pub fn new() -> DpopReplayGuard {
        DpopReplayGuard {
            replay: crate::oidc::ReplayGuard::new(),
        }
    }

    pub fn with_pg(
        backing: myelin_storage::DurableReplayBacking,
        rt: tokio::runtime::Handle,
    ) -> DpopReplayGuard {
        DpopReplayGuard {
            replay: crate::oidc::ReplayGuard::with_pg(backing, rt),
        }
    }

    fn consume(
        &self,
        tenant: &str,
        region: &str,
        bound_jkt: &str,
        jti: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<bool, AuthzError> {
        let namespace = serde_json::json!(["dpop", region, bound_jkt]).to_string();
        self.replay
            .consume_scoped(tenant, &namespace, jti, expires_at, now)
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_dpop_proof(
    proof: &str,
    bound_jkt: &str,
    tenant: &str,
    region: &str,
    binding: &DpopBinding,
    now: i64,
    window_secs: i64,
    replay: &DpopReplayGuard,
) -> Result<(), AuthzError> {
    if window_secs < 0 {
        return Err(refuse("DPoP freshness window must not be negative"));
    }
    let rest = proof
        .strip_prefix(DPOP_HEADER)
        .ok_or_else(|| AuthzError::BadRequest("DPoP proof has a bad header".into()))?;
    let mut segs = rest.split('.');
    let payload_b64 = segs.next().unwrap_or("");
    let sig_b64 = segs
        .next()
        .ok_or_else(|| AuthzError::BadRequest("DPoP proof missing signature segment".into()))?;
    if segs.next().is_some() {
        return Err(AuthzError::BadRequest(
            "DPoP proof has trailing segments".into(),
        ));
    }
    let payload_bytes = b64url_decode(payload_b64)?;
    let sig = b64url_decode(sig_b64)?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| AuthzError::BadRequest(format!("malformed DPoP payload JSON: {e}")))?;

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

    let proof_jkt = dpop_jkt(&pub_key);
    if proof_jkt != bound_jkt {
        return Err(refuse(
            "DPoP proof key thumbprint does not match the token's bound `cnf.jkt` (sender-constraint \
             violated - the proof was signed by a different key than the token is bound to)",
        ));
    }

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

    let iat = payload
        .get("iat")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| refuse("DPoP proof missing integer `iat`"))?;
    if iat.saturating_add(window_secs) < now || iat.saturating_sub(window_secs) > now {
        return Err(refuse(format!(
            "DPoP proof `iat`={iat} is outside the ±{window_secs}s freshness window (now={now})"
        )));
    }

    let jti = payload
        .get("jti")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| refuse("DPoP proof missing `jti`"))?;
    if !replay.consume(
        tenant,
        region,
        bound_jkt,
        jti,
        iat.saturating_add(window_secs),
        now,
    )? {
        return Err(refuse(
            "DPoP proof was already presented (durable replay defence)",
        ));
    }
    Ok(())
}

type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Clone)]
pub struct PasetoCapabilityVerifier {
    anchor: CellTrustAnchor,
    now: NowFn,
    binding: Option<DpopBinding>,
    dpop_window_secs: i64,
    replay: DpopReplayGuard,
}

impl PasetoCapabilityVerifier {
    pub fn new(anchor: CellTrustAnchor) -> PasetoCapabilityVerifier {
        PasetoCapabilityVerifier {
            anchor,
            now: Arc::new(crate::clock::unix_seconds),
            binding: None,
            dpop_window_secs: 60,
            replay: DpopReplayGuard::new(),
        }
    }

    pub fn with_request_binding(mut self, binding: DpopBinding) -> PasetoCapabilityVerifier {
        self.binding = Some(binding);
        self
    }

    pub fn with_clock(
        mut self,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> PasetoCapabilityVerifier {
        self.now = Arc::new(now);
        self
    }

    pub fn with_replay_guard(mut self, replay: DpopReplayGuard) -> PasetoCapabilityVerifier {
        self.replay = replay;
        self
    }

    pub fn with_dpop_window(mut self, secs: i64) -> PasetoCapabilityVerifier {
        self.dpop_window_secs = secs;
        self
    }

    pub fn verify_material(
        &self,
        material: &str,
        kind: MachineKind,
    ) -> myelin_identity::Result<CapabilityToken> {
        self.verify_material_with_binding(material, kind, self.binding.as_ref())
    }

    fn verify_material_with_binding(
        &self,
        material: &str,
        kind: MachineKind,
        request_binding: Option<&DpopBinding>,
    ) -> myelin_identity::Result<CapabilityToken> {
        let parsed = ParsedMaterial::parse(material)?;

        let (claims_bytes, sig) = paseto_v4_public_verify(&self.anchor.public_key, &parsed.paseto)?;
        let claims: serde_json::Value = serde_json::from_slice(&claims_bytes)
            .map_err(|e| AuthzError::BadRequest(format!("malformed verified claims JSON: {e}")))?;

        let tenant = str_claim(&claims, "tenant")?;
        let region = str_claim(&claims, "region")?;
        let subject_key = str_claim(&claims, "sub")?;
        let jti = str_claim(&claims, "jti")?;
        let exp = claims
            .get("exp")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| refuse("verified token missing integer `exp`"))?;
        let purpose_claim = str_claim(&claims, "purpose").map_err(|_| {
            refuse(
                "capability token has no signed credential purpose; ambiguous legacy credentials are \
                 refused - re-mint/re-bootstrap the credential",
            )
        })?;
        let purpose = match purpose_claim.as_str() {
            "human_session" => CredentialPurpose::HumanSession,
            "operator_bootstrap" => CredentialPurpose::OperatorBootstrap,
            "agent_run" => CredentialPurpose::AgentRun {
                run_id: str_claim(&claims, "run_id").map_err(|_| {
                    refuse("signed agent-run credential is missing its non-empty `run_id` binding")
                })?,
                delegation_snapshot: claims
                    .get("delegation_snapshot")
                    .map(|value| {
                        value.as_i64().ok_or_else(|| {
                            refuse("signed `delegation_snapshot` must be an integer")
                        })
                    })
                    .transpose()?,
            },
            "pat" => CredentialPurpose::Pat,
            "ci_job" => CredentialPurpose::CiJob {
                run_id: str_claim(&claims, "run_id").map_err(|_| {
                    refuse("signed CI-job credential is missing its non-empty `run_id` binding")
                })?,
            },
            "deploy_key" => CredentialPurpose::DeployKey,
            "per_job" => CredentialPurpose::PerJob {
                run_id: str_claim(&claims, "run_id").map_err(|_| {
                    refuse("signed per-job credential is missing its non-empty `run_id` binding")
                })?,
            },
            other => {
                return Err(refuse(format!(
                    "unknown signed credential purpose `{other}` - refused"
                )))
            }
        };
        if purpose.machine_kind() != kind {
            return Err(refuse(format!(
                "credential scheme selects kind `{kind:?}` but the signed purpose `{}` requires \
                 `{:?}` - refused",
                purpose.claim(),
                purpose.machine_kind()
            )));
        }
        let audience = match str_claim(&claims, "aud")?.as_str() {
            "edge" => CredentialAudience::Edge,
            "mcp" => CredentialAudience::Mcp,
            other => {
                return Err(refuse(format!(
                    "unknown signed audience `{other}` - refused"
                )))
            }
        };
        if !purpose.is_run_scoped() && claims.get("run_id").is_some() {
            return Err(refuse(
                "a non-run credential carries a `run_id` binding - ambiguous purpose refused",
            ));
        }
        if !purpose.is_agent_run() && claims.get("delegation_snapshot").is_some() {
            return Err(refuse(
                "a non-run credential carries a delegation snapshot - ambiguous purpose refused",
            ));
        }
        let root_grants = claims
            .get("auth")
            .ok_or_else(|| refuse("verified token is missing its signed `auth` authority"))
            .and_then(|auth| parse_grant_set(auth, "signed `auth`").map_err(refuse))?;
        let bound_jkt = verified_confirmation_jkt(&claims)?;

        let now = (self.now)();
        if exp <= now {
            return Err(refuse(format!(
                "capability token expired: exp={exp} <= now={now}"
            )));
        }

        let mut tag = macaroon_root_tag(&self.anchor.mac_key, &sig);
        let mut effective = root_grants.clone();
        for caveat in &parsed.caveats {
            for g in caveat {
                if !effective.contains(g) {
                    return Err(refuse(format!(
                        "amplified caveat: grant `{g}` is not held by the parent authority - a caveat \
                         may only NARROW (monotone attenuation), never widen - refused"
                    )));
                }
            }
            effective = effective.intersection(caveat).cloned().collect();
            tag = macaroon_fold(&tag, caveat);
        }
        if !ct_eq(&parsed.tail, &tag) {
            return Err(refuse(
                "macaroon caveat-chain tag mismatch - the caveat chain was forged, reordered, or a \
                 caveat was removed (the chain is bound under the cell secret) - refused",
            ));
        }

        let dpop_bound = match (&bound_jkt, &parsed.dpop) {
            (Some(jkt), Some(proof)) => {
                let binding = request_binding.ok_or_else(|| {
                    refuse(
                        "a DPoP-bound token requires a request binding (htm/htu) to verify the proof \
                         against - none injected - fail-closed",
                    )
                })?;
                verify_dpop_proof(
                    proof,
                    jkt,
                    &tenant,
                    &region,
                    binding,
                    now,
                    self.dpop_window_secs,
                    &self.replay,
                )?;
                true
            }
            (Some(_), None) => {
                return Err(refuse(
                    "a DPoP-bound token (cnf.jkt present) was presented WITHOUT a DPoP proof - a \
                     bearer-only presentation of a sender-constrained token is refused (RFC 9449)",
                ))
            }
            (None, Some(_)) => {
                return Err(refuse(
                    "a DPoP proof was presented for a token that carries no `cnf.jkt` binding - refused",
                ));
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
            purpose,
            audience,
            exp_unix: exp,
        })
    }
}

impl crate::machine_auth::TokenVerifier for PasetoCapabilityVerifier {
    fn verify(&self, credential: &Credential) -> myelin_identity::Result<CapabilityToken> {
        let kind = MachineKind::from_scheme(&credential.scheme).ok_or_else(|| {
            AuthzError::BadRequest(format!(
                "scheme `{}` is not a capability-token / machine-identity surface \
                 (session/pat/ci/agent/deploy_key/per_job)",
                credential.scheme
            ))
        })?;
        let token = self.verify_material(&credential.material, kind)?;
        enforce_session_purpose(&credential.scheme, &token)?;
        Ok(token)
    }

    fn verify_for_request(
        &self,
        credential: &Credential,
        binding: &DpopBinding,
    ) -> myelin_identity::Result<CapabilityToken> {
        let kind = MachineKind::from_scheme(&credential.scheme).ok_or_else(|| {
            AuthzError::BadRequest(format!(
                "scheme `{}` is not a capability-token / machine-identity surface \
                 (pat/ci/agent/deploy_key/per_job)",
                credential.scheme
            ))
        })?;
        let token = self.verify_material_with_binding(&credential.material, kind, Some(binding))?;
        enforce_session_purpose(&credential.scheme, &token)?;
        Ok(token)
    }
}

fn enforce_session_purpose(scheme: &str, token: &CapabilityToken) -> Result<(), AuthzError> {
    if (scheme == crate::machine_auth::scheme::SESSION)
        != matches!(token.purpose, CredentialPurpose::HumanSession)
    {
        return Err(refuse(
            "the `session` scheme and signed `human_session` purpose must be used together",
        ));
    }
    Ok(())
}

fn str_claim(claims: &serde_json::Value, key: &str) -> Result<String, AuthzError> {
    claims
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| refuse(format!("verified token missing/empty `{key}` claim")))
}

fn parse_grant_set(value: &serde_json::Value, label: &str) -> Result<BTreeSet<String>, String> {
    let grants = value
        .as_array()
        .ok_or_else(|| format!("{label} must be a JSON array of grant strings"))?;
    let mut parsed = BTreeSet::new();
    for (index, grant) in grants.iter().enumerate() {
        let grant = grant
            .as_str()
            .ok_or_else(|| format!("{label} grant at index {index} must be a string"))?;
        if !parsed.insert(grant.to_string()) {
            return Err(format!("{label} contains duplicate grant `{grant}`"));
        }
    }
    Ok(parsed)
}

fn verified_confirmation_jkt(claims: &serde_json::Value) -> Result<Option<String>, AuthzError> {
    let Some(confirmation) = claims.get("cnf") else {
        return Ok(None);
    };
    let object = confirmation
        .as_object()
        .ok_or_else(|| refuse("signed `cnf` claim must be an object containing only `jkt`"))?;
    if object.len() != 1 {
        return Err(refuse(
            "signed `cnf` claim must contain exactly one `jkt` binding",
        ));
    }
    let jkt = object
        .get("jkt")
        .and_then(serde_json::Value::as_str)
        .filter(|jkt| !jkt.is_empty())
        .ok_or_else(|| refuse("signed `cnf.jkt` must be a non-empty string"))?;
    let thumbprint = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(jkt.as_bytes())
        .map_err(|_| refuse("signed `cnf.jkt` must be a base64url SHA-256 thumbprint"))?;
    if thumbprint.len() != SHA256_BYTES {
        return Err(refuse(format!(
            "signed `cnf.jkt` must decode to {SHA256_BYTES} bytes"
        )));
    }
    Ok(Some(jkt.to_string()))
}

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

#[derive(Clone)]
pub struct PasetoCapabilitySigner {
    authority: Arc<CellTokenAuthority>,
    now: NowFn,
}

impl PasetoCapabilitySigner {
    pub fn new(authority: Arc<CellTokenAuthority>) -> PasetoCapabilitySigner {
        PasetoCapabilitySigner {
            authority,
            now: Arc::new(crate::clock::unix_seconds),
        }
    }

    pub fn with_clock(
        mut self,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> PasetoCapabilitySigner {
        self.now = Arc::new(now);
        self
    }
}

impl crate::mint::TokenSigner for PasetoCapabilitySigner {
    fn sign(&self, request: &crate::mint::TokenSignRequest) -> String {
        let exp = chrono::DateTime::parse_from_rfc3339(&request.expires_at().0)
            .map(|instant| instant.timestamp())
            .unwrap_or_else(|_| (self.now)());
        let audience = match request.purpose() {
            CredentialPurpose::AgentRun { .. } => CredentialAudience::Mcp,
            _ => CredentialAudience::Edge,
        };
        self.authority.mint(&CapabilityMintSpec {
            tenant: request.scope().tenant().0.clone(),
            region: request.scope().region().0.clone(),
            subject_key: request.subject().0.clone(),
            jti: request.jti().to_string(),
            exp_unix: exp,
            authority: request.grants().to_vec(),
            dpop_jkt: None,
            purpose: request.purpose().clone(),
            audience,
        })
    }

    fn attenuate(&self, material: &str, grants: &[String]) -> Result<String, String> {
        attenuate(material, grants.iter().cloned()).map_err(|error| match error {
            AuthzError::BadRequest(reason)
            | AuthzError::Unavailable(reason)
            | AuthzError::FailClosed(reason) => reason,
            AuthzError::NotYetImplemented(reason) => reason.to_string(),
        })
    }
}

#[derive(Clone, Default)]
pub struct CellAnchorSet {
    by_cell: BTreeMap<String, CellTrustAnchor>,
}

impl CellAnchorSet {
    pub fn new() -> CellAnchorSet {
        CellAnchorSet {
            by_cell: BTreeMap::new(),
        }
    }
    pub fn with_anchor(
        mut self,
        cell_id: impl Into<String>,
        anchor: CellTrustAnchor,
    ) -> CellAnchorSet {
        self.by_cell.insert(cell_id.into(), anchor);
        self
    }
    pub fn get(&self, cell_id: &str) -> Option<&CellTrustAnchor> {
        self.by_cell.get(cell_id)
    }
    pub fn len(&self) -> usize {
        self.by_cell.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_cell.is_empty()
    }
}

#[cfg(test)]
#[path = "capability_crypto_tests.rs"]
mod tests;
