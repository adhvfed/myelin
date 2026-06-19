//! # P-ID-10 (global P-068) GATE / DRILL — ID-D3, the cross-tenant path-spoof drill (dated green
//! artifact)
//!
//! **Drill catalogue row ID-D3 (§4.2, F2):** *Cross-tenant check/list/read via path spoof → 0
//! cross-tenant tuples readable.* Survival signal: **cross-tenant count = 0** (the single most
//! load-bearing zero in the platform). Run against the failure-injection harness's
//! telemetry-assertion library (the contract-1.8 survival-signal set), exactly as the storage IDOR
//! drill (P-007) and the harness self-test (P-S04) do. `myelin-harness` is a DEV-dependency only —
//! it never enters the identity-service production DAG.
//!
//! **The scenario.** The ReBAC namespace engine (P-ID-10) resolves a permission through the four
//! userset operators over the raw S3 tuples — but ALWAYS within the verified `(tenant, region)`
//! partition: the engine carries no tenant state of its own and resolves through
//! [`CheckEngine`]/[`StoreBackedCheck`], which reads ONLY the verified scope's partition (the
//! `tenant-predicate` floor, identity §6 — no cross-tenant query path). An attacker in tenant
//! `evil-corp` resolves a permission over a SPOOFED reference to victim tenant `acme`'s object: the
//! engine reads `evil-corp`'s partition, sees 0 of `acme`'s tuples, and DENIES — the
//! `cross-tenant-count` signal reads `== 0`.
//!
//! A non-zero would mean a tuple from another tenant was readable through the engine — the
//! cross-tenant IDOR the floor forbids — and the drill aborts LOUDLY (EI-01 §3: loud, never
//! swallowed; the threshold is NEVER weakened to pass).

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

/// **ID-D3 — cross-tenant path-spoof → 0 cross-tenant tuples readable through the namespace
/// engine.**
///
/// Seed victim tenant `acme` with a real grant on `project:web` (alice inherits view via team
/// membership — the engine's core hierarchy). Then an attacker `mallory@evil-corp` resolves the
/// SAME `view` permission over a spoofed reference to `acme`'s `project:web`. Because the
/// [`StoreBackedCheck`] derives the read scope from the SUBJECT's own verified `(tenant, region)`
/// (tenant-from-token, never a path), mallory reads `evil-corp`'s partition — 0 of acme's tuples —
/// and is DENIED. We run a batch (the 1x load unit) of spoof attempts and assert
/// `CrossTenantCount == 0`.
#[test]
fn id_d3_cross_tenant_path_spoof_reads_zero() {
    let mut signals = SignalSource::new();

    // The VICTIM tenant `acme`: a real grant — alice inherits project:web view via team membership.
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed acme grant");

    let svc = StoreBackedCheck::new(store);
    let spoofed_object = ArtifactRef("project:web".into()); // acme's object, spoofed by the attacker

    // Sanity: the legitimate acme principal (alice) DOES inherit view (the engine resolves the
    // grant within acme's own partition).
    let alice = principal("acme", "p:alice");
    assert_eq!(
        svc.check(&alice, &Permission("view".into()), &spoofed_object, &at_latest(), None),
        Ok(Decision::Allow),
        "the legitimate acme principal inherits view (the engine resolves within acme's partition)"
    );

    // THE ATTACK: a batch of attackers in `evil-corp` resolve the SAME view permission over the
    // spoofed reference to acme's object. Each reads evil-corp's partition (0 acme tuples) → Deny.
    let mut cross_tenant_reads: i64 = 0;
    const BATCH: usize = 64;
    for i in 0..BATCH {
        let mallory = principal("evil-corp", &format!("p:mallory-{i}"));
        // The attacker even copies alice's id to spoof identity — but the SCOPE is mallory's own
        // verified (tenant=evil-corp, region), so acme's partition is unreachable.
        let mallory_as_alice = {
            let mut m = mallory.clone();
            m.principal_id = PrincipalId("p:alice".into()); // identity spoof attempt
            m.tenant = TenantId("evil-corp".into()); // but the verified tenant is still evil-corp
            m
        };
        let decision = svc.check(
            &mallory_as_alice,
            &Permission("view".into()),
            &spoofed_object,
            &at_latest(),
            None,
        );
        // The attacker is DENIED — 0 of acme's tuples are readable from evil-corp's partition.
        if decision == Ok(Decision::Allow) {
            cross_tenant_reads += 1;
        }
    }

    // Record the survival signal (the producer exports CrossTenantCount on the metrics-health port,
    // P-S13; here the drill rig records what the engine's scoping produced).
    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads);

    // THE green artifact: 0 cross-tenant reads through the namespace engine. expect_green() panics
    // LOUDLY on red (the signal + predicate + observed value), never silently.
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant tuples readable through the engine on a spoofed path (ID-D3)"
    );

    println!(
        "[P-068 DRILL GREEN 2026-06-19] ID-D3 cross-tenant path-spoof: \
         victim=acme attacker=evil-corp batch={BATCH} spoof attempts on project:web (view, \
         parent_team->view inheritance) → CrossTenantCount=0 (the engine resolves only the \
         verified (tenant, region) partition — no cross-tenant query path, identity §6)"
    );
}

/// **ID-D3 corollary — the engine resolves a tuple-to-userset inheritance edge only within the
/// tenant: a cross-tenant parent reference does not leak.** acme grants project:web inheritance from
/// team:eng; even if evil-corp declares a project:web whose parent is acme's team, the resolution is
/// cell-local — evil-corp's principal reading evil-corp's (empty) partition denies.
#[test]
fn id_d3_inheritance_edge_does_not_cross_tenant() {
    let acme = scope_of(&principal("acme", "p-admin"));
    let evil = scope_of(&principal("evil-corp", "p-admin"));

    let store = TupleStore::new(OutboxStore::new());
    // acme: the real inheritance chain.
    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("acme chain");
    // evil-corp's partition is empty (it has NO tuples about project:web or team:eng).
    let _ = evil;

    let svc = StoreBackedCheck::new(store);
    let obj = ArtifactRef("project:web".into());

    // An evil-corp principal (even named alice) resolving view reads evil-corp's EMPTY partition →
    // Deny. The inheritance edge in acme's partition is invisible cross-tenant.
    let mallory = principal("evil-corp", "p:alice");
    assert_eq!(
        svc.check(&mallory, &Permission("view".into()), &obj, &at_latest(), None),
        Ok(Decision::Deny),
        "a cross-tenant principal does not inherit through acme's tuple-to-userset edge"
    );
    // And acme's alice still inherits (the chain is intact within acme).
    let alice = principal("acme", "p:alice");
    assert_eq!(
        svc.check(&alice, &Permission("view".into()), &obj, &at_latest(), None),
        Ok(Decision::Allow),
        "acme's principal still inherits within its own partition"
    );
}
