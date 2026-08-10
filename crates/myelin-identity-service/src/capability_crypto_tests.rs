use super::*;
use crate::machine_auth::{MachineKind, TokenVerifier};
use myelin_identity::{
    AuthzError, Credential, DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const NOW: i64 = 1_700_000_000;

fn cell() -> CellTokenAuthority {
    CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority")
}

fn verifier(anchor: CellTrustAnchor) -> PasetoCapabilityVerifier {
    PasetoCapabilityVerifier::new(anchor).with_clock(|| NOW)
}

fn sign_request(
    jti: &str,
    purpose: CredentialPurpose,
    expires_at: &str,
    grants: &[&str],
) -> crate::mint::TokenSignRequest {
    let principal = Principal::new(
        TenantId("acme".into()),
        Region("eu-west".into()),
        PrincipalId("svc:agent".into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    );
    crate::mint::TokenSignRequest::new(
        &TenantScope::from_verified_token(&principal, principal.region.clone()),
        principal.principal_id,
        jti,
        purpose,
        myelin_events::Timestamp(expires_at.into()),
        grants.iter().copied(),
    )
}

fn spec(tenant: &str, grants: &[&str], jkt: Option<String>) -> CapabilityMintSpec {
    let purpose = if jkt.is_some() {
        CredentialPurpose::Pat
    } else {
        CredentialPurpose::OperatorBootstrap
    };
    CapabilityMintSpec {
        tenant: tenant.into(),
        region: "eu-west".into(),
        subject_key: "subj-1".into(),
        jti: "jti-1".into(),
        exp_unix: NOW + 300,
        authority: grants.iter().map(|s| s.to_string()).collect(),
        dpop_jkt: jkt,
        purpose,
        audience: CredentialAudience::Edge,
    }
}

fn tamper_claims(material: &str, mutate: impl Fn(&mut serde_json::Value)) -> String {
    let parts: Vec<&str> = material.split('|').collect();
    let rest = parts[0].strip_prefix("v4.public.").unwrap();
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(rest.as_bytes())
        .unwrap();
    let (claims, sig) = body.split_at(body.len() - 64);
    let mut v: serde_json::Value = serde_json::from_slice(claims).unwrap();
    mutate(&mut v);
    let mut new_body = serde_json::to_vec(&v).unwrap();
    new_body.extend_from_slice(sig);
    let forged_paseto = format!(
        "v4.public.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&new_body)
    );
    let mut out = vec![forged_paseto];
    out.extend(parts[1..].iter().map(|s| s.to_string()));
    out.join("|")
}

fn with_dpop(material: &str, proof: &str) -> String {
    format!(
        "{material}|{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(proof.as_bytes())
    )
}

fn mint_raw_claims(cell: &CellTokenAuthority, claims: serde_json::Value) -> String {
    let bytes = serde_json::to_vec(&claims).unwrap();
    let paseto = paseto_v4_public_sign(&cell.signing_key, &bytes);
    let signature = paseto_root_signature(&paseto).unwrap();
    let tail = macaroon_root_tag(&cell.mac_key, &signature);
    encode_material(&paseto, &[], &tail, None)
}

#[test]
fn positive_signed_token_verifies() {
    let c = cell();
    let material = c.mint(&spec(
        "acme",
        &["repo:acme/web#read", "repo:acme/web#write"],
        None,
    ));
    let token = verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Agent)
        .expect("a correctly-signed token verifies");
    assert_eq!(token.tenant.0, "acme");
    assert_eq!(token.region.0, "eu-west");
    assert_eq!(token.subject_key, "subj-1");
    assert_eq!(token.jti, "jti-1");
    assert!(token.authority.holds("repo:acme/web#read"));
    assert!(token.authority.holds("repo:acme/web#write"));
    assert!(!token.dpop_bound);
}

#[test]
fn signed_purpose_must_match_the_transport_selected_machine_kind() {
    let c = cell();
    let mut ci = spec("acme", &["ci.checks.report"], None);
    ci.purpose = CredentialPurpose::CiJob {
        run_id: "ci-run-1".into(),
    };
    let material = c.mint(&ci);
    assert!(verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Ci)
        .is_ok());
    assert!(matches!(
        verifier(c.trust_anchor()).verify_material(&material, MachineKind::Agent),
        Err(AuthzError::FailClosed(_))
    ));
}

#[test]
fn human_session_purpose_is_accepted_only_under_the_session_scheme() {
    let c = cell();
    let mut session = spec("acme", &["repo.pull"], None);
    session.purpose = CredentialPurpose::HumanSession;
    session.subject_key = "p:alice".into();
    let material = c.mint(&session);
    let verifier = verifier(c.trust_anchor());

    let accepted = verifier
        .verify(&Credential {
            scheme: crate::machine_auth::scheme::SESSION.into(),
            material: material.clone(),
        })
        .expect("a signed human session verifies under the session scheme");
    assert_eq!(accepted.purpose, CredentialPurpose::HumanSession);
    assert!(matches!(
        verifier.verify(&Credential {
            scheme: crate::machine_auth::scheme::AGENT.into(),
            material,
        }),
        Err(AuthzError::FailClosed(_))
    ));
}

#[test]
fn purpose_less_legacy_capability_is_refused_with_rebootstrap_guidance() {
    let c = cell();
    let material = mint_raw_claims(
        &c,
        serde_json::json!({
            "tenant": "acme",
            "region": "eu-west",
            "sub": "subj-1",
            "jti": "legacy-jti",
            "exp": NOW + 300,
            "aud": "edge",
            "auth": ["edge.operator"]
        }),
    );
    let error = verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Agent)
        .expect_err("purpose-less legacy credentials are ambiguous");
    assert!(matches!(
        error,
        AuthzError::FailClosed(message)
            if message.contains("re-mint/re-bootstrap")
    ));
}

#[test]
fn positive_attenuated_child_verifies_narrowed() {
    let c = cell();
    let root = c.mint(&spec(
        "acme",
        &["repo:acme/web#read", "repo:acme/web#write"],
        None,
    ));
    let child = attenuate(&root, ["repo:acme/web#read"]).expect("offline attenuation");
    let token = verifier(c.trust_anchor())
        .verify_material(&child, MachineKind::Agent)
        .expect("the attenuated child verifies");
    assert!(
        token.authority.holds("repo:acme/web#read"),
        "the kept grant survives"
    );
    assert!(
        !token.authority.holds("repo:acme/web#write"),
        "the dropped grant is gone (narrowed)"
    );
    assert_eq!(
        token.authority.len(),
        1,
        "effective authority is a strict subset"
    );
}

#[test]
fn positive_dpop_bound_pat_with_valid_proof_verifies() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["repo:acme/web#read"], Some(client.jkt())));
    let proof = client.prove("POST", "https://api.myelin/x", NOW, "dpop-jti-1");
    let presented = with_dpop(&material, &proof);
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    let token = v
        .verify_material(&presented, MachineKind::Pat)
        .expect("a DPoP-bound PAT with a valid proof verifies");
    assert!(token.dpop_bound, "the PAT is sender-constrained");
    assert_eq!(token.tenant.0, "acme");
}

#[test]
fn per_request_token_verification_supplies_dpop_binding() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[31u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["edge.whoami"], Some(client.jkt())));
    let proof = client.prove(
        "GET",
        "https://myelin.example/v1/whoami",
        NOW,
        "dpop-per-request",
    );
    let credential = Credential {
        scheme: "pat".into(),
        material: with_dpop(&material, &proof),
    };
    let verifier = verifier(c.trust_anchor());
    assert!(
        verifier.verify(&credential).is_err(),
        "a DPoP PAT without trusted request context must fail closed"
    );
    let token = verifier
        .verify_for_request(
            &credential,
            &DpopBinding {
                htm: "GET".into(),
                htu: "https://myelin.example/v1/whoami".into(),
            },
        )
        .expect("the transport-supplied binding reaches the real DPoP verifier");
    assert!(token.dpop_bound);
}

#[test]
fn negative_forged_token_by_non_anchor_key_is_refused() {
    let real = cell();
    let attacker = CellTokenAuthority::from_seed(&[42u8; 32], &[9u8; 32]).unwrap();
    let forged = attacker.mint(&spec("acme", &["repo:acme/web#admin"], None));
    let err = verifier(real.trust_anchor())
        .verify_material(&forged, MachineKind::Agent)
        .unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
        "a token signed by an unknown key must be refused, got {err:?}"
    );
}

#[test]
fn negative_tampered_token_is_refused() {
    let c = cell();
    let material = c.mint(&spec("acme", &["repo:acme/web#read"], None));

    let t1 = tamper_claims(&material, |v| v["tenant"] = serde_json::json!("globex"));
    let t2 = tamper_claims(&material, |v| {
        v["auth"] = serde_json::json!(["repo:acme/web#admin"])
    });
    let t3 = tamper_claims(&material, |v| v["jti"] = serde_json::json!("jti-evil"));
    for (name, forged) in [("tenant", t1), ("authority", t2), ("jti", t3)] {
        let err = verifier(c.trust_anchor())
            .verify_material(&forged, MachineKind::Agent)
            .unwrap_err();
        assert!(
            matches!(&err, AuthzError::FailClosed(m) if m.contains("signature verification failed")),
            "tampering `{name}` after signing must be refused, got {err:?}"
        );
    }
}

#[test]
fn negative_amplified_caveat_is_rejected() {
    let c = cell();
    let root = c.mint(&spec("acme", &["repo:acme/web#read"], None));
    let amplified = attenuate(&root, ["repo:acme/web#read", "repo:acme/web#admin"]).unwrap();
    let err = verifier(c.trust_anchor())
        .verify_material(&amplified, MachineKind::Agent)
        .unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("amplified caveat")),
        "an amplified caveat must be rejected, got {err:?}"
    );
}

#[test]
fn negative_removed_or_forged_caveat_is_rejected() {
    let c = cell();
    let root = c.mint(&spec("acme", &["a", "b"], None));
    let attenuated = attenuate(&root, ["a"]).unwrap();

    let parts: Vec<&str> = attenuated.split('|').collect();
    let empty_caveats = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&Vec::<Vec<String>>::new()).unwrap());
    let forged = format!("{}|{}|{}", parts[0], empty_caveats, parts[2]);
    let err = verifier(c.trust_anchor())
        .verify_material(&forged, MachineKind::Agent)
        .unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("caveat-chain tag mismatch")),
        "stripping a caveat (resetting the chain) must be rejected, got {err:?}"
    );
}

#[test]
fn negative_expired_token_is_refused() {
    let c = cell();
    let mut s = spec("acme", &["a"], None);
    s.exp_unix = NOW - 1;
    let material = c.mint(&s);
    let err = verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Agent)
        .unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
        "an expired token must be refused, got {err:?}"
    );
}

#[test]
fn negative_dpop_bound_pat_without_proof_is_refused() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt())));
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    let err = v.verify_material(&material, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("WITHOUT a DPoP proof")),
        "a DPoP-bound token with no proof must be refused, got {err:?}"
    );
}

#[test]
fn negative_dpop_proof_by_wrong_key_is_refused() {
    let c = cell();
    let bound = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let thief = DpopClientKey::from_seed(&[4u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(bound.jkt())));
    let thief_proof = thief.prove("POST", "https://api.myelin/x", NOW, "dpop-jti-x");
    let presented = with_dpop(&material, &thief_proof);
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    let err = v.verify_material(&presented, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("thumbprint does not match")),
        "a proof by the wrong key must be refused, got {err:?}"
    );
}

#[test]
fn negative_dpop_replayed_jti_is_refused() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt())));
    let proof = client.prove("POST", "https://api.myelin/x", NOW, "dpop-replay");
    let presented = with_dpop(&material, &proof);
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    v.verify_material(&presented, MachineKind::Pat)
        .expect("first presentation verifies");
    let err = v.verify_material(&presented, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("replay")),
        "a replayed DPoP jti must be refused, got {err:?}"
    );
}

#[test]
fn dpop_replay_entries_expire_with_the_freshness_window() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[13u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt())));
    let guard = DpopReplayGuard::new();
    let binding = DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    };
    let first = with_dpop(
        &material,
        &client.prove("POST", "https://api.myelin/x", NOW, "reusable-after-window"),
    );
    verifier(c.trust_anchor())
        .with_request_binding(binding.clone())
        .with_replay_guard(guard.clone())
        .verify_material(&first, MachineKind::Pat)
        .expect("the first proof is fresh");

    let later = with_dpop(
        &material,
        &client.prove(
            "POST",
            "https://api.myelin/x",
            NOW + 61,
            "reusable-after-window",
        ),
    );
    PasetoCapabilityVerifier::new(c.trust_anchor())
        .with_clock(|| NOW + 61)
        .with_request_binding(binding)
        .with_replay_guard(guard)
        .verify_material(&later, MachineKind::Pat)
        .expect("a newly signed proof may reuse the identifier after the old proof window expires");
}

#[test]
fn negative_dpop_wrong_htm_htu_is_refused() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt())));
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    let p_method = with_dpop(
        &material,
        &client.prove("GET", "https://api.myelin/x", NOW, "h1"),
    );
    let e1 = v.verify_material(&p_method, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&e1, AuthzError::FailClosed(m) if m.contains("htm")),
        "wrong htm refused: {e1:?}"
    );
    let p_url = with_dpop(
        &material,
        &client.prove("POST", "https://evil/x", NOW, "h2"),
    );
    let e2 = v.verify_material(&p_url, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&e2, AuthzError::FailClosed(m) if m.contains("htu")),
        "wrong htu refused: {e2:?}"
    );
}

#[test]
fn negative_dpop_stale_iat_is_refused() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt())));
    let stale = client.prove("POST", "https://api.myelin/x", NOW - 1000, "stale-1");
    let presented = with_dpop(&material, &stale);
    let v = verifier(c.trust_anchor())
        .with_dpop_window(60)
        .with_request_binding(DpopBinding {
            htm: "POST".into(),
            htu: "https://api.myelin/x".into(),
        });
    let err = v.verify_material(&presented, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("freshness window")),
        "a stale DPoP iat must be refused, got {err:?}"
    );
}

#[test]
fn negative_garbage_is_refused_not_panicking() {
    let c = cell();
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    let good = c.mint(&spec(
        "acme",
        &["a"],
        Some(DpopClientKey::from_seed(&[3u8; 32]).unwrap().jkt()),
    ));
    let garbage = [
        "".to_string(),
        "not-a-token".to_string(),
        "v4.public.@@@|bm90|dGFpbA".to_string(),
        "v4.public.AAAA|!!!|dGFpbA".to_string(),
        "onlyone|two".to_string(),
        "a|b|c|d|e".to_string(),
        format!("{}|{}|{}|{}", "v4.public.AAAA", "W10", "dA", "Z2FyYmFnZQ"),
        with_dpop(&good, "totally-not-a-dpop-proof"),
        with_dpop(&good, "dpop.v1.@@@.###"),
    ];
    for g in garbage {
        let r = v.verify_material(&g, MachineKind::Pat);
        assert!(
            r.is_err(),
            "garbage `{g}` must be refused (and must not panic)"
        );
    }
    assert!(attenuate("garbage", ["x"]).is_err());
}

#[test]
fn tenant_comes_only_from_the_verified_token() {
    let c = cell();
    let material = c.mint(&spec("acme", &["a"], None));
    let token = verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Agent)
        .unwrap();
    assert_eq!(token.tenant.0, "acme", "the tenant is the signed token's");

    let no_tenant = tamper_claims(&material, |v| {
        v.as_object_mut().unwrap().remove("tenant");
    });
    assert!(verifier(c.trust_anchor())
        .verify_material(&no_tenant, MachineKind::Agent)
        .is_err());
}

#[test]
fn multi_step_attenuation_is_monotone() {
    let c = cell();
    let root = c.mint(&spec("acme", &["a", "b", "c"], None));
    let step1 = attenuate(&root, ["a", "b"]).unwrap();
    let step2 = attenuate(&step1, ["a"]).unwrap();
    let token = verifier(c.trust_anchor())
        .verify_material(&step2, MachineKind::Agent)
        .unwrap();
    assert_eq!(token.authority.len(), 1);
    assert!(token.authority.holds("a"));
    assert!(!token.authority.holds("b") && !token.authority.holds("c"));
}

#[test]
fn signer_verifier_seam_round_trip() {
    use crate::machine_auth::TokenVerifier;
    use crate::mint::TokenSigner;
    let c = std::sync::Arc::new(cell());
    let signer = PasetoCapabilitySigner::new(c.clone()).with_clock(|| NOW);
    let material = signer.sign(&sign_request(
        "runtok:1",
        CredentialPurpose::AgentRun {
            run_id: "run:1".into(),
            delegation_snapshot: Some(42),
        },
        "2023-11-14T22:18:20Z",
        &["agent:run"],
    ));
    let v = verifier(c.trust_anchor());
    let cred = myelin_identity::Credential {
        scheme: "agent".into(),
        material,
    };
    let token = v
        .verify(&cred)
        .expect("the signed per-run token verifies through the seam");
    assert_eq!(token.tenant.0, "acme");
    assert_eq!(token.subject_key, "svc:agent");
    assert!(token.authority.holds("agent:run"));
    assert_eq!(
        token.purpose,
        CredentialPurpose::AgentRun {
            run_id: "run:1".into(),
            delegation_snapshot: Some(42),
        }
    );
    assert_eq!(token.audience, CredentialAudience::Mcp);
    assert!(
        !token.dpop_bound,
        "a per-run token is TTL-constrained, not DPoP-bound"
    );
}

#[test]
fn signer_malformed_run_deadline_produces_unusable_token() {
    use crate::machine_auth::TokenVerifier;
    use crate::mint::TokenSigner;

    let c = std::sync::Arc::new(cell());
    let signer = PasetoCapabilitySigner::new(c.clone()).with_clock(|| NOW);
    let material = signer.sign(&sign_request(
        "runtok:bad-exp",
        CredentialPurpose::AgentRun {
            run_id: "run:bad-exp".into(),
            delegation_snapshot: Some(42),
        },
        "not-a-deadline",
        &["repo.pull"],
    ));
    let error = verifier(c.trust_anchor())
        .verify(&myelin_identity::Credential {
            scheme: "agent".into(),
            material,
        })
        .expect_err("a malformed deadline must not receive the historical fixed TTL");
    assert!(
        matches!(error, AuthzError::FailClosed(message) if message.contains("expired")),
        "malformed deadline should fail as an expired token"
    );
}
