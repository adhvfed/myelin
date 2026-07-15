//! The MR-011 negative + positive corpus for the REAL machine/capability-token crypto. Real cell
//! keypairs, real PASETO v4.public signatures, real macaroon caveat chains, real DPoP proofs — every
//! forgery / amplification / replay is a REAL one, and each is REFUSED. `verify` is proven TOTAL over
//! attacker bytes (no panic on garbage). See the module docs for the construction.

use super::*;
use crate::machine_auth::MachineKind;
use myelin_identity::AuthzError;

const NOW: i64 = 1_700_000_000;

/// The real cell token authority (a deterministic Ed25519 seed + macaroon secret for the corpus).
fn cell() -> CellTokenAuthority {
    CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority")
}

/// A verifier over a cell's trust anchor, clock pinned to `NOW`.
fn verifier(anchor: CellTrustAnchor) -> PasetoCapabilityVerifier {
    PasetoCapabilityVerifier::new(anchor).with_clock(|| NOW)
}

fn spec(tenant: &str, grants: &[&str], jkt: Option<String>) -> CapabilityMintSpec {
    CapabilityMintSpec {
        tenant: tenant.into(),
        region: "eu-west".into(),
        subject_key: "subj-1".into(),
        jti: "jti-1".into(),
        exp_unix: NOW + 300,
        authority: grants.iter().map(|s| s.to_string()).collect(),
        dpop_jkt: jkt,
    }
}

/// Re-sign-free claim tamper: decode the PASETO body, replace the claims JSON, re-attach the ORIGINAL
/// signature, re-encode. This is exactly what an attacker who edited a field but lacks the cell key
/// produces — the signature no longer matches the new claims.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// POSITIVES
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **A correctly-signed token verifies and yields the trust-rooted facts from the VERIFIED body.**
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

/// **A correctly-attenuated child verifies with the NARROWED authority (offline, no secrets).** The
/// holder drops `#write`; the verifier returns only `#read`, a strict subset of the signed root.
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

/// **A valid DPoP-bound PAT + a valid proof verifies (sender-constraint satisfied).**
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// NEGATIVES — every forgery is a REAL one, and each is REFUSED.
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **(a) A forged token signed by a NON-ANCHOR key is refused.** The attacker mints with their OWN
/// Ed25519 cell key; verification against the real cell's public anchor fails the signature.
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

/// **(b) A tampered token (tenant / authority / jti edited AFTER signing) is refused.** The classic
/// IDOR/escalation forgery: you cannot edit the tenant, widen the authority, or swap the jti without
/// re-signing (which needs the cell private key).
#[test]
fn negative_tampered_token_is_refused() {
    let c = cell();
    let material = c.mint(&spec("acme", &["repo:acme/web#read"], None));

    // (b1) tenant acme → globex.
    let t1 = tamper_claims(&material, |v| v["tenant"] = serde_json::json!("globex"));
    // (b2) authority widened to admin.
    let t2 = tamper_claims(&material, |v| {
        v["auth"] = serde_json::json!(["repo:acme/web#admin"])
    });
    // (b3) jti swapped (to dodge a revocation).
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

/// **(c) An AMPLIFIED caveat (a child claiming a grant the parent lacked) is rejected (the macaroon
/// law).** The holder appends a caveat naming `#admin`, which the signed root never granted — the
/// verifier refuses it rather than minting authority a caveat must not add.
#[test]
fn negative_amplified_caveat_is_rejected() {
    let c = cell();
    let root = c.mint(&spec("acme", &["repo:acme/web#read"], None));
    // The "holder" tries to attenuate to a WIDER set (read + admin) — amplification.
    let amplified = attenuate(&root, ["repo:acme/web#read", "repo:acme/web#admin"]).unwrap();
    let err = verifier(c.trust_anchor())
        .verify_material(&amplified, MachineKind::Agent)
        .unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("amplified caveat")),
        "an amplified caveat must be rejected, got {err:?}"
    );
}

/// **(c') Removing / forging a caveat in the chain is rejected (the HMAC tag is bound under the cell
/// secret).** A holder attenuates (adds a caveat → narrower tail), then tries to STRIP the caveat back
/// to the empty chain while keeping the advanced tail. The verifier recomputes `tag_0` from the cell
/// secret (which the holder lacks) and the mismatch is caught — attenuation is one-way.
#[test]
fn negative_removed_or_forged_caveat_is_rejected() {
    let c = cell();
    let root = c.mint(&spec("acme", &["a", "b"], None));
    let attenuated = attenuate(&root, ["a"]).unwrap(); // narrows to {a}, tail advanced

    // Forge: keep the advanced tail but reset the caveat list to empty (try to regain {a,b}).
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

/// **(d) An expired token is refused (exp is a numeric instant — no lexical-compare hazard).**
#[test]
fn negative_expired_token_is_refused() {
    let c = cell();
    let mut s = spec("acme", &["a"], None);
    s.exp_unix = NOW - 1; // already expired as of the pinned clock
    let material = c.mint(&s);
    let err = verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Agent)
        .unwrap_err();
    assert!(
        matches!(&err, AuthzError::FailClosed(m) if m.contains("expired")),
        "an expired token must be refused, got {err:?}"
    );
}

/// **(f) DPoP: a bound PAT presented WITHOUT a proof is refused (bearer-only presentation of a
/// sender-constrained token).**
#[test]
fn negative_dpop_bound_pat_without_proof_is_refused() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt()))); // 3 fields, NO dpop
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

/// **(f) DPoP: a proof signed by the WRONG key is refused (the thumbprint != the bound `cnf.jkt`).** A
/// thief who steals the token and signs a proof with THEIR own key cannot bind it.
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

/// **(f) DPoP: a replayed proof `jti` is refused (single-use).**
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

/// **(f) DPoP: a wrong `htm`/`htu` (a proof minted for another method/URL) is refused.**
#[test]
fn negative_dpop_wrong_htm_htu_is_refused() {
    let c = cell();
    let client = DpopClientKey::from_seed(&[3u8; 32]).unwrap();
    let material = c.mint(&spec("acme", &["a"], Some(client.jkt())));
    let v = verifier(c.trust_anchor()).with_request_binding(DpopBinding {
        htm: "POST".into(),
        htu: "https://api.myelin/x".into(),
    });
    // wrong method
    let p_method = with_dpop(
        &material,
        &client.prove("GET", "https://api.myelin/x", NOW, "h1"),
    );
    let e1 = v.verify_material(&p_method, MachineKind::Pat).unwrap_err();
    assert!(
        matches!(&e1, AuthzError::FailClosed(m) if m.contains("htm")),
        "wrong htm refused: {e1:?}"
    );
    // wrong URL
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

/// **(f) DPoP: a stale `iat` (outside the freshness window) is refused.**
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

/// **(g) Malformed / garbage credential material + garbage DPoP proof are refused, never PANIC.**
/// `verify` is TOTAL over attacker bytes.
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
        "v4.public.@@@|bm90|dGFpbA".to_string(), // bad base64 in the PASETO body
        "v4.public.AAAA|!!!|dGFpbA".to_string(), // bad base64 caveats
        "onlyone|two".to_string(),               // 2 fields
        "a|b|c|d|e".to_string(),                 // 5 fields
        format!("{}|{}|{}|{}", "v4.public.AAAA", "W10", "dA", "Z2FyYmFnZQ"), // garbage dpop on garbage token
        with_dpop(&good, "totally-not-a-dpop-proof"), // garbage proof on a real bound token
        with_dpop(&good, "dpop.v1.@@@.###"),          // bad base64 proof segments
    ];
    for g in garbage {
        // Must return (an Err), never panic. Either BadRequest (structural) or FailClosed (crypto).
        let r = v.verify_material(&g, MachineKind::Pat);
        assert!(
            r.is_err(),
            "garbage `{g}` must be refused (and must not panic)"
        );
    }
    // attenuate over garbage is also total.
    assert!(attenuate("garbage", ["x"]).is_err());
}

/// **(h) Tenant-injection is structurally impossible: the tenant is the VERIFIED token's.** A token
/// minted for `acme` always verifies to `acme`; the only way to assert another tenant is to re-sign,
/// which the attacker cannot (proven by (a)/(b)). And a token with no `tenant` claim is refused.
#[test]
fn tenant_comes_only_from_the_verified_token() {
    let c = cell();
    let material = c.mint(&spec("acme", &["a"], None));
    let token = verifier(c.trust_anchor())
        .verify_material(&material, MachineKind::Agent)
        .unwrap();
    assert_eq!(token.tenant.0, "acme", "the tenant is the signed token's");

    // A token whose `tenant` claim is stripped after signing fails the signature (cannot fabricate).
    let no_tenant = tamper_claims(&material, |v| {
        v.as_object_mut().unwrap().remove("tenant");
    });
    assert!(verifier(c.trust_anchor())
        .verify_material(&no_tenant, MachineKind::Agent)
        .is_err());
}

/// **A multi-step attenuation chain stays monotone and binding.** Two successive caveats narrow the
/// authority further; the chain verifies and the final effective set is the intersection.
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

/// **The mint→verify round-trip through the real `TokenSigner`/`TokenVerifier` seams.** The
/// [`PasetoCapabilitySigner`] (the mint side) and [`PasetoCapabilityVerifier`] (the verify side) are
/// one keypair; a per-run token signed by the signer verifies through the trait.
#[test]
fn signer_verifier_seam_round_trip() {
    use crate::machine_auth::TokenVerifier;
    use crate::mint::TokenSigner;
    let c = std::sync::Arc::new(cell());
    let signer = PasetoCapabilitySigner::new(c.clone(), 300).with_clock(|| NOW);
    let material = signer.sign("acme", "eu-west", "svc:agent", "runtok:1", &["agent:run"]);
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
    assert!(
        !token.dpop_bound,
        "a per-run token is TTL-constrained, not DPoP-bound"
    );
}
