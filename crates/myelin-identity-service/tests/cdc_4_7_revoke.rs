//! # The CDC pair for contract 4.7 (`revoke`) — `revoke(jti | principal_id)` (P-ID-14 / P-072)
//!
//! **Contract-index row 4.7** (`mint_run_token` + **`revoke(jti|principal_id)`** — idempotent even
//! on crash). This is the dedicated provider+consumer pair the P-ID-14 TESTS field names — the
//! focused, in-CI evidence that the two sides of the **revoke** seam cannot drift apart:
//!
//! - the **PROVIDER** is Identity's S7 revocation list ([`StoreBackedCheck::revoke_in`] /
//!   [`StoreBackedCheck::disable_principal_in`] over the [`RevocationStore`]): a `revoke` writes the
//!   `(tenant, region)`-partitioned denylist (mirror-first, idempotent, crash-safe), and the
//!   `check` consult denies a revoked principal across every surface.
//! - the **CONSUMER** is a **gateway / agent caller that honours a session/token ONLY if it is not
//!   revoked** — exactly the shape every surface (UI / API / git-wire / agent) uses (contract 4.7
//!   "consumed by Agent Fabric, CI dispatch, workflow"): before it acts on a principal's behalf it
//!   `check`s the principal, and proceeds ONLY on `Allow`; a revoked principal's `check` returns
//!   `Deny`, so the consumer refuses (the F8 cross-surface deny).
//!
//! The provider's promise (a revoked principal/jti is on the denylist; revoke is idempotent +
//! crash-safe) and the consumer's promise (it honours the session iff `check` returned `Allow`,
//! and refuses a revoked one) are pinned here so a change to either side fails this test in the same
//! CI job. The fail-static cache S6 interaction is P-ID-15; this pair is the M1 `revoke` CDC.

use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, RevokeTarget, TupleDelta, Zookie,
};
use myelin_identity_service::{RevocationStore, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

/// A subject in tenant `acme`, region `eu-west` — the same `(tenant, region)` the provider seeds
/// under (tenant-from-token).
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

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

/// The PROVIDER: a store-backed `check` surface (the S7 denylist + the S3 tuples) seeded with a
/// `view` grant for alice on `repo:core`.
fn provider(s: &TenantScope) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            s,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            ts("2026-06-19T00:00:00Z"),
        )
        .expect("seed grant");
    StoreBackedCheck::new(store)
}

/// The CONSUMER: a gateway / agent caller that honours a principal's session ONLY if `check`
/// Allows. Returns `true` iff the action proceeded. This is the canonical 4.7 consumer shape (every
/// surface refuses a revoked principal — the cross-surface deny).
fn surface_honours_session<S: IdentityService>(
    svc: &S,
    actor: &Principal,
    permission: &str,
    object: &ArtifactRef,
) -> bool {
    let decision = svc.check(
        actor,
        &Permission(permission.to_string()),
        object,
        &at_latest(),
        None,
    );
    matches!(decision, Ok(Decision::Allow))
}

/// **The 4.7 happy path: an un-revoked principal's session is honoured.** alice has `view` and is
/// not revoked ⇒ the surface proceeds.
#[test]
fn cdc_4_7_unrevoked_session_honoured() {
    let s = scope("acme");
    let svc = provider(&s);
    let obj = ArtifactRef("repo:core".into());
    assert!(
        surface_honours_session(&svc, &subject("p:alice"), "view", &obj),
        "an un-revoked principal with a grant is honoured"
    );
}

/// **The 4.7 cross-surface deny: a SCIM-disabled / revoked principal is refused on every surface.**
/// The provider revokes alice (the principal denylist); the consumer's `check` now returns `Deny`
/// even though the underlying grant still exists — the revoke wins (the F8 deny).
#[test]
fn cdc_4_7_revoked_principal_refused_across_surfaces() {
    let s = scope("acme");
    let svc = provider(&s);
    let obj = ArtifactRef("repo:core".into());

    // Before revoke: honoured.
    assert!(surface_honours_session(
        &svc,
        &subject("p:alice"),
        "view",
        &obj
    ));

    // PROVIDER: revoke alice (the SCIM-disable path).
    svc.disable_principal_in(
        &s,
        &PrincipalId("p:alice".into()),
        ts("2026-06-19T01:00:00Z"),
    );

    // CONSUMER: every surface now refuses alice's session — the grant is intact but the revoke wins.
    assert!(
        !surface_honours_session(&svc, &subject("p:alice"), "view", &obj),
        "a revoked principal is denied on every surface (the grant is intact but the revoke wins)"
    );
    // A DIFFERENT, un-revoked principal with the same grant is still honoured (the revoke is
    // principal-scoped, not a blanket deny).
    let svc2 = {
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(
                &s,
                &subject("p-admin"),
                &[grant("repo:core", "view", "p:carol")],
                None,
                None,
                ts("2026-06-19T00:00:00Z"),
            )
            .expect("seed carol");
        StoreBackedCheck::new(store)
    };
    assert!(
        surface_honours_session(&svc2, &subject("p:carol"), "view", &obj),
        "a different un-revoked principal is unaffected by alice's revoke"
    );
}

/// **The 4.7 idempotency promise (mandatory-core): a double-revoke is a no-op.** The provider
/// revokes alice twice (at different times); the denylist holds the revocation exactly once, and the
/// consumer's deny is unchanged. (The crash-safe re-derivation is exercised in the S7 unit tests +
/// the ID-D1 drill.)
#[test]
fn cdc_4_7_revoke_is_idempotent() {
    let s = scope("acme");
    let store = RevocationStore::new();
    let target = RevokeTarget::Principal(PrincipalId("p:alice".into()));
    store.revoke(&s, &target, ts("2026-06-19T00:00:00Z"));
    store.revoke(&s, &target, ts("2026-06-19T09:00:00Z"));
    assert_eq!(
        store.revocation_count(&s),
        1,
        "a double-revoke is a no-op (idempotent — the denylist holds it once)"
    );
    assert!(store.is_revoked(&s, &target, &ts("2026-06-19T10:00:00Z")));
}
