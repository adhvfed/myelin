//! # The CDC pair for contract 4.2 — `check(subject, permission, object, zookie?, caveat?) →
//! {Allow | Deny | Conditional}` (P-ID-09 / P-067)
//!
//! **Contract-index row 4.2** (`check`, the per-action fail-closed gate, + the `CaveatContext`
//! field/transition ABAC rider evaluated off the hot `list_objects` path). This is the dedicated
//! provider+consumer pair the P-ID-09 TESTS field names — the focused, in-CI evidence that the two
//! sides of the `check` seam cannot drift apart:
//!
//! - the **PROVIDER** ([`CheckEngine::check`] via the [`StoreBackedCheck`] surface) evaluates the
//!   depth-bounded Zanzibar userset-rewrite over the raw S3 tuples at the zookie snapshot and
//!   returns `Allow | Deny | Conditional`, fail-closed on uncertainty;
//! - the **CONSUMER** is a **write-path caller gating an action** — exactly the shape every write
//!   path / `EffectApi` / gateway uses (contract 4.2 "consumed by every write path"): before it
//!   performs the mutation it calls `check(actor, permission, object)` and proceeds ONLY on
//!   `Allow`; a `Deny`/`Conditional`/error refuses the action (fail-closed).
//!
//! The provider's promise (a grant ⇒ `Allow`; no grant / uncertainty ⇒ not-`Allow`) and the
//! consumer's promise (it performs the mutation iff `check` returned `Allow`, and refuses otherwise)
//! are pinned here so a change to either side fails this test in the same CI job. The full compiled
//! permission/namespace engine is P-ID-10; the literal-only caveat → the full `QueryAst` core is
//! P-ID-22 — this pair is the M1 `check`-gate CDC the prompt requires.

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, IdentityService, Literal, ObjectId,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::collections::BTreeMap;

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

/// A subject in tenant `acme`, region `eu-west` — the SAME `(tenant, region)` the provider seeds
/// under, so `StoreBackedCheck` (which derives the scope from the subject's own verified
/// tenant/region, tenant-from-token) reads the partition the grant lives in.
fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// The PROVIDER: the store-backed `check` surface over the S3 tuples, seeded with `tuples`.
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed tuples");
    StoreBackedCheck::new(store)
}

/// The CONSUMER: a write-path caller that performs a mutation ONLY if `check` returned `Allow`.
/// Returns `true` iff the guarded action proceeded. This is the canonical 4.2 consumer shape (every
/// write path / `EffectApi` gates on `check` before mutating, fail-closed).
fn write_path_gated_action<S: IdentityService>(
    svc: &S,
    actor: &Principal,
    permission: &str,
    object: &ArtifactRef,
    caveat: Option<&CaveatContext>,
) -> bool {
    let decision = svc.check(
        actor,
        &Permission(permission.to_string()),
        object,
        &at_latest(),
        caveat,
    );
    // The write proceeds ONLY on an explicit Allow. Deny, Conditional, and any error all refuse the
    // mutation (fail-closed — the write path never opens on uncertainty).
    matches!(decision, Ok(Decision::Allow))
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// **The 4.2 happy path: a granted write-path action proceeds.** alice has `write` on the repo ⇒
/// the consumer's guarded mutation proceeds (the provider returned `Allow`).
#[test]
fn cdc_4_2_granted_action_proceeds() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("repo:core", "write", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/git/repo/repo:core".into());
    assert!(
        write_path_gated_action(&svc, &subject("p:alice"), "write", &obj, None),
        "the write-path caller proceeds when check Allows the granted action"
    );
}

/// **The 4.2 fail-closed path: an un-granted write-path action is refused.** bob has no `write`
/// grant ⇒ the consumer refuses the mutation (the provider returned `Deny`, not `Allow`).
#[test]
fn cdc_4_2_ungranted_action_refused() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("repo:core", "write", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/git/repo/repo:core".into());
    assert!(
        !write_path_gated_action(&svc, &subject("p:bob"), "write", &obj, None),
        "the write-path caller refuses the action when check does not Allow (fail-closed)"
    );
}

/// **The 4.2 caveat path: a violated field caveat redacts (refuses) the gated action.** alice has
/// the `view_field` relation, but the literal caveat `severity(9) < threshold(5)` is violated ⇒
/// the provider returns `Deny` ⇒ the consumer refuses (the field is hidden). A satisfied caveat
/// (`3 < 5`) proceeds.
#[test]
fn cdc_4_2_caveat_gates_the_action() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "view_field", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/issues/issue/issue:PROJ-1".into());

    // satisfied: 3 < 5 ⇒ visible ⇒ proceeds.
    let mut ok = BTreeMap::new();
    ok.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    ok.insert("__caveat_lhs".to_string(), Literal::Int(3));
    ok.insert("__caveat_rhs".to_string(), Literal::Int(5));
    let cav_ok = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs: ok,
    };
    assert!(
        write_path_gated_action(&svc, &subject("p:alice"), "view_field", &obj, Some(&cav_ok)),
        "a satisfied literal caveat proceeds (the field is visible)"
    );

    // violated: 9 < 5 is false ⇒ redacted ⇒ refused.
    let mut bad = BTreeMap::new();
    bad.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    bad.insert("__caveat_lhs".to_string(), Literal::Int(9));
    bad.insert("__caveat_rhs".to_string(), Literal::Int(5));
    let cav_bad = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs: bad,
    };
    assert!(
        !write_path_gated_action(&svc, &subject("p:alice"), "view_field", &obj, Some(&cav_bad)),
        "a violated literal caveat refuses (the field is redacted)"
    );
}

/// **The 4.2 mandatory-core branch: a missing-context caveat does NOT proceed.** The consumer
/// gates on `Allow` only; a `Conditional` (missing context) refuses the silent action — the write
/// path never opens on a caveat it could not evaluate.
#[test]
fn cdc_4_2_missing_context_caveat_does_not_proceed() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("issue:PROJ-1", "view_field", "p:alice")]);
    let obj = ArtifactRef("myelin://acme/issues/issue/issue:PROJ-1".into());
    // An op with no operands → the provider returns Conditional → the consumer does NOT proceed.
    let mut attrs = BTreeMap::new();
    attrs.insert("__caveat_op".to_string(), Literal::Str("lt".into()));
    let cav = CaveatContext {
        object: obj.clone(),
        field: Some(myelin_identity::FieldId("salary".into())),
        transition: None,
        attrs,
    };
    // Directly assert the provider returns Conditional (not Allow), and the consumer refuses.
    let decision = svc.check(
        &subject("p:alice"),
        &Permission("view_field".into()),
        &obj,
        &at_latest(),
        Some(&cav),
    );
    assert_eq!(
        decision,
        Ok(Decision::Conditional),
        "a missing-context caveat is Conditional (never a silent Allow)"
    );
    assert!(
        !write_path_gated_action(&svc, &subject("p:alice"), "view_field", &obj, Some(&cav)),
        "the write path does not proceed on Conditional (fail-closed)"
    );
}
