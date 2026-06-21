//! # P-ID-24 (global P-247) GATE / DRILL — GIT-D8, cross-tenant repo access at the front door
//! (dated green artifact)
//!
//! **Drill catalogue row GIT-D8 (F2):** *Cross-tenant repo access via token tenant ≠ URL-path tenant
//! → tenant from token; 0 cross-tenant read; rejected at front door.* Survival signal:
//! **cross-tenant count = 0**. Run against the failure-injection harness's telemetry-assertion
//! library (the contract-1.8 survival-signal set), exactly as ID-D3 (P-068) does. `myelin-harness`
//! is a DEV-dependency only — it never enters the identity-service production DAG.
//!
//! **The scenario.** Id's compiled Git fragment (P-ID-24) resolves a Git permission (`pull`) through
//! the four userset operators over the raw S3 tuples — but ALWAYS within the verified `(tenant,
//! region)` partition (the `tenant-predicate` floor, identity §6 — no cross-tenant query path; the
//! scope is the SUBJECT's own verified tenant, **never the URL path**, ID-3). An attacker in tenant
//! `evil-corp` resolves `pull` over a SPOOFED reference to victim tenant `acme`'s `repo:core`: the
//! engine reads `evil-corp`'s partition, sees 0 of `acme`'s tuples, and DENIES — the
//! `cross-tenant-count` signal reads `== 0`.
//!
//! A non-zero would mean a Git tuple from another tenant was readable — the cross-tenant repo IDOR
//! GIT-D8 forbids — and the drill aborts LOUDLY (EI-01 §3: loud, never swallowed; the threshold is
//! NEVER weakened to pass).

use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// **GIT-D8 — cross-tenant repo access via token-tenant ≠ path-tenant → 0 cross-tenant reads.**
///
/// Seed victim tenant `acme` with a real grant: alice is `admin` on `repo:core` (so she can `pull`
/// through Id's compiled Git fragment). Then a batch of attackers in `evil-corp` resolve the SAME
/// `pull` permission over a SPOOFED reference to `acme`'s `repo:core` — even spoofing alice's id.
/// Because [`StoreBackedCheck`] derives the read scope from the SUBJECT's own verified `(tenant,
/// region)` (tenant-from-token, never the path), every attacker reads `evil-corp`'s empty partition
/// → Deny. We assert `CrossTenantCount == 0`.
#[test]
fn git_d8_cross_tenant_repo_access_reads_zero() {
    let mut signals = SignalSource::new();

    // The VICTIM tenant `acme`: alice is a real repo admin.
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[add("repo:core", "admin", "p:alice")],
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed acme grant");

    let svc = StoreBackedCheck::new(store);
    // Admit Id's compiled Git fragment so `pull` resolves through the four operators.
    for admit in svc.admit_git_fragment() {
        assert!(matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }));
    }
    let spoofed_repo = ArtifactRef("repo:core".into()); // acme's repo, spoofed by the attacker

    // Sanity: the legitimate acme principal (alice) DOES pull (the engine resolves within acme's
    // partition).
    let alice = principal("acme", "p:alice");
    assert_eq!(
        svc.check(&alice, &Permission("pull".into()), &spoofed_repo, &at_latest(), None),
        Ok(Decision::Allow),
        "the legitimate acme admin pulls (Id resolves within acme's partition)"
    );

    // THE ATTACK: a batch of attackers in `evil-corp` resolve `pull` over the spoofed reference to
    // acme's repo, each spoofing alice's id. Each reads evil-corp's partition (0 acme tuples) → Deny.
    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for i in 0..BATCH {
        let mut attacker = principal("evil-corp", &format!("p:mallory-{i}"));
        // The attacker copies alice's id (identity spoof) AND points the request at acme's repo —
        // but the VERIFIED tenant is still evil-corp, so acme's partition is unreachable.
        attacker.principal_id = PrincipalId("p:alice".into());
        attacker.tenant = TenantId("evil-corp".into());
        let decision = svc.check(&attacker, &Permission("pull".into()), &spoofed_repo, &at_latest(), None);
        if decision == Ok(Decision::Allow) {
            cross_tenant_reads += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant repo reads on a spoofed token-tenant ≠ path-tenant request (GIT-D8)"
    );

    println!(
        "[P-247 DRILL GREEN 2026-06-21] GIT-D8 cross-tenant repo access: \
         victim=acme attacker=evil-corp batch={BATCH} spoofed pull attempts on repo:core (Id's \
         compiled Git fragment, pull = reader∪writer∪admin∪parent_project->view) → \
         CrossTenantCount=0 (tenant-from-token, never the URL path — no cross-tenant query path, \
         identity §6 / ID-3)"
    );
}

/// **GIT-D8 corollary — the `approve_untrusted_ci` fork-endorsement gate does not cross tenant.** A
/// maintainer endorsement in `acme` is invisible to an `evil-corp` principal: the X-1 endorsement is
/// a plain relation check resolved only within the verified scope.
#[test]
fn git_d8_approve_untrusted_ci_does_not_cross_tenant() {
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[add("repo:core", "approve_untrusted_ci", "p:maintainer")],
            None,
            None,
            Timestamp("2026-06-20T00:00:00Z".into()),
        )
        .expect("seed acme endorsement");
    let svc = StoreBackedCheck::new(store);
    for _ in svc.admit_git_fragment() {}
    let repo = ArtifactRef("repo:core".into());

    // acme's maintainer endorses (within acme's partition).
    assert_eq!(
        svc.check(
            &principal("acme", "p:maintainer"),
            &Permission("approve_untrusted_ci".into()),
            &repo,
            &at_latest(),
            None,
        ),
        Ok(Decision::Allow),
        "acme's maintainer endorses within acme's partition"
    );
    // An evil-corp principal (even named maintainer) reading evil-corp's empty partition → Deny.
    assert_eq!(
        svc.check(
            &principal("evil-corp", "p:maintainer"),
            &Permission("approve_untrusted_ci".into()),
            &repo,
            &at_latest(),
            None,
        ),
        Ok(Decision::Deny),
        "a cross-tenant principal cannot read the acme endorsement (X-1 gate is scope-local)"
    );
}
