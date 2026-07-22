//! Unit tests for the in-boundary secret broker (CI-P24 / P-367) — the fork-no-secrets boundary
//! (CI-D7) + the trusted-tier DIRECT-NARROW scope + the OIDC short-lived mint.
//!
//! These are the security-load-bearing (mandatory-core) tests; they pin every branch of the broker so
//! the `cargo-mutants` ≥ 90% floor (the module doc) is killable: the fork short-circuit, the
//! `Decision::Allow` gate, the per-job scope filter, and the OIDC fork-refusal.

use super::*;
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
    RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Test doubles (the FROZEN consumed surfaces — never a second real impl).
// ---------------------------------------------------------------------------

/// A secret capability that resolves any KNOWN handle to a deterministic material; an unknown handle
/// resolves to `None` (a stale/absent binding → withheld, never an error).
struct FakeCapability {
    known: HashSet<String>,
}
impl FakeCapability {
    fn with(handles: &[&str]) -> Self {
        FakeCapability {
            known: handles.iter().map(|h| h.to_string()).collect(),
        }
    }
}
impl SecretCapability for FakeCapability {
    fn resolve_handle(&self, handle: &str) -> Option<String> {
        if self.known.contains(handle) {
            Some(format!("material:{handle}"))
        } else {
            None
        }
    }
}

/// An Identity gate that `Allow`s `read` ONLY for the secret objects in `granted` (the DIRECT NARROW
/// `secret#direct_reader` grant). Everything else is `Deny` (fail-closed). It also records every
/// `check` it received, so a test can prove a fork-tier resolution made ZERO authz calls (the
/// structural short-circuit).
struct FakeIdentity {
    granted: HashSet<String>,
    checks: std::cell::RefCell<Vec<String>>,
}
impl FakeIdentity {
    fn granting(objects: &[&str]) -> Self {
        FakeIdentity {
            granted: objects.iter().map(|o| o.to_string()).collect(),
            checks: std::cell::RefCell::new(Vec::new()),
        }
    }
    fn check_count(&self) -> usize {
        self.checks.borrow().len()
    }
}
impl IdentityService for FakeIdentity {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn check(
        &self,
        _s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        self.checks.borrow_mut().push(o.0.clone());
        // The broker only ever asks for `read` on a secret object.
        assert_eq!(
            p.0, SECRET_READ_PERMISSION,
            "broker checks only `secret.read`"
        );
        if self.granted.contains(&o.0) {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
}

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

fn subject() -> Principal {
    Principal::stub(
        PrincipalId("u:dev".into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

fn at() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::Strong,
    }
}

/// The job's referenced secrets (NAMES + opaque handles — never material).
fn refs() -> Vec<SecretRef> {
    vec![
        SecretRef {
            name: "REGISTRY_TOKEN".into(),
            handle: "h:registry".into(),
        },
        SecretRef {
            name: "DEPLOY_KEY".into(),
            handle: "h:deploy".into(),
        },
    ]
}

/// Map a `SecretRef` to its `ci_secret` ArtifactRef (the gate object) — the per-job scope is the
/// handle, so the object id is derived from it (deterministic).
fn secret_object_of(r: &SecretRef) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/ci/secret/{}", r.handle))
}

// ---------------------------------------------------------------------------
// DEFENCE #1 — the structural fork short-circuit (CI-D7).
// ---------------------------------------------------------------------------

#[test]
fn fork_resolves_to_zero_secrets() {
    // A fork run whose subject WOULD be granted both secrets if it were trusted — the grant is a
    // misconfiguration the structural defence must survive.
    let cap = FakeCapability::with(&["h:registry", "h:deploy"]);
    let id = FakeIdentity::granting(&[
        "myelin://acme/ci/secret/h:registry",
        "myelin://acme/ci/secret/h:deploy",
    ]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::UntrustedFork,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    // The quantified CI-D7 gate: 0 secrets resolved by a fork-tier run.
    assert_eq!(
        res.secret_count(),
        0,
        "a fork resolves ZERO secrets (CI-D7)"
    );
    assert!(res.is_empty());
    assert!(!res.all_resolved());
    // Every referenced name is withheld with the structural reason.
    for o in &res.outcomes {
        assert!(matches!(
            o,
            SecretOutcome::Withheld {
                reason: WithholdReason::UntrustedFork,
                ..
            }
        ));
    }
    // The STRUCTURAL property: the fork never reached the authz gate at all (0 checks) — a
    // misconfigured grant cannot leak because a fork never asks.
    assert_eq!(
        id.check_count(),
        0,
        "a fork short-circuits BEFORE any authz check (the `!is_untrusted_fork` arm by construction)"
    );
}

// ---------------------------------------------------------------------------
// DEFENCE #2 — the trusted-tier DIRECT-NARROW authz gate.
// ---------------------------------------------------------------------------

#[test]
fn trusted_resolves_only_referenced_granted_names() {
    let cap = FakeCapability::with(&["h:registry", "h:deploy"]);
    // Grant ONLY the registry secret (not the deploy key) — the DIRECT NARROW grant.
    let id = FakeIdentity::granting(&["myelin://acme/ci/secret/h:registry"]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    // Exactly ONE secret resolves (the granted one); the ungranted one is withheld.
    assert_eq!(res.secret_count(), 1);
    let resolved: Vec<_> = res.resolved().collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].name, "REGISTRY_TOKEN");
    assert_eq!(resolved[0].value, "material:h:registry");

    // The ungranted deploy key is withheld (NotGranted), never leaked.
    assert!(matches!(
        res.outcomes[1],
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    ));
    // The scope is EXACTLY the 2 referenced names — the broker never enumerated the project.
    assert_eq!(
        id.check_count(),
        2,
        "the broker checks ONLY the referenced names (per-job scope)"
    );
}

#[test]
fn protected_without_grant_withholds_all() {
    let cap = FakeCapability::with(&["h:registry", "h:deploy"]);
    // No grants at all — a protected secret without an explicit DIRECT grant.
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    assert_eq!(
        res.secret_count(),
        0,
        "no grant → no secret (DIRECT NARROW, CI-1)"
    );
    assert!(res.outcomes.iter().all(|o| matches!(
        o,
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    )));
}

#[test]
fn granted_but_stale_handle_withholds_observably() {
    // The subject IS granted, but the handle does not resolve (a stale binding) — withheld, never a
    // panic or a silent leak.
    let cap = FakeCapability::with(&[]); // no handle resolves
    let id = FakeIdentity::granting(&["myelin://acme/ci/secret/h:registry"]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::Trusted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    assert_eq!(res.secret_count(), 0);
    assert!(matches!(
        res.outcomes[0],
        SecretOutcome::Withheld {
            reason: WithholdReason::NotGranted,
            ..
        }
    ));
}

#[test]
fn self_hosted_is_trusted_for_secret_resolution() {
    // A self-hosted member run is trusted CODE — it resolves its granted secrets (it is NOT a fork).
    let cap = FakeCapability::with(&["h:registry", "h:deploy"]);
    let id = FakeIdentity::granting(&[
        "myelin://acme/ci/secret/h:registry",
        "myelin://acme/ci/secret/h:deploy",
    ]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(
            TrustTier::SelfHosted,
            &subject(),
            secret_object_of,
            &refs(),
            &at(),
        )
        .expect("resolution does not error");

    assert!(
        res.all_resolved(),
        "a self-hosted run resolves its granted secrets"
    );
    assert_eq!(res.secret_count(), 2);
    assert!(
        id.check_count() > 0,
        "self-hosted goes THROUGH the authz gate (it is not a fork)"
    );
}

#[test]
fn empty_refs_resolve_to_empty() {
    // A job referencing no secrets resolves to an empty (vacuously 0) resolution.
    let cap = FakeCapability::with(&[]);
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    let res = broker
        .resolve(TrustTier::Trusted, &subject(), secret_object_of, &[], &at())
        .expect("resolution does not error");

    assert_eq!(res.secret_count(), 0);
    assert!(res.is_empty());
    assert!(
        res.all_resolved(),
        "vacuously all-resolved when there are no refs"
    );
}

// ---------------------------------------------------------------------------
// OIDC short-lived audience-scoped credentials (contract 4.7).
// ---------------------------------------------------------------------------

#[test]
fn trusted_mints_audience_scoped_oidc() {
    let cap = FakeCapability::with(&[]);
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    let cred = broker
        .mint_oidc(TrustTier::Trusted, "registry.fr-par", 900, |aud, ttl| {
            Some(format!("oidc:{aud}:{ttl}"))
        })
        .expect("a trusted run mints an OIDC credential");

    assert_eq!(cred.audience, "registry.fr-par");
    assert_eq!(cred.ttl_secs, 900);
    assert_eq!(cred.token, "oidc:registry.fr-par:900");
}

#[test]
fn fork_is_refused_an_oidc_credential() {
    let cap = FakeCapability::with(&[]);
    let id = FakeIdentity::granting(&[]);
    let broker = SecretBroker::new(&cap, &id);

    // The mint closure would succeed — but a fork never reaches it (the same boundary).
    let cred = broker.mint_oidc(
        TrustTier::UntrustedFork,
        "registry.fr-par",
        900,
        |aud, ttl| Some(format!("oidc:{aud}:{ttl}")),
    );
    assert!(
        cred.is_none(),
        "a fork gets NO audience-scoped cloud credential (CI-D7)"
    );
}

#[test]
fn withhold_reason_tokens_are_stable() {
    assert_eq!(WithholdReason::UntrustedFork.as_token(), "untrusted_fork");
    assert_eq!(WithholdReason::NotGranted.as_token(), "not_granted");
}

#[test]
fn debug_output_redacts_secret_and_oidc_material_recursively() {
    let material = "unique-static-secret-material";
    let token = "unique-short-lived-oidc-token";
    let resolved = ResolvedSecret {
        name: "DEPLOY_KEY".into(),
        value: material.into(),
    };
    let resolution = SecretResolution {
        outcomes: vec![SecretOutcome::Resolved(resolved.clone())],
    };
    let credential = OidcCredential {
        audience: "registry.fr-par".into(),
        token: token.into(),
        ttl_secs: 900,
    };

    for rendered in [
        format!("{resolved:?}"),
        format!("{:?}", resolution.outcomes[0]),
        format!("{resolution:?}"),
        format!("{credential:?}"),
    ] {
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(material));
        assert!(!rendered.contains(token));
    }
}
