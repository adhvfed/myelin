//! # ISS-D3 — cross-tenant + confidential-issue IDOR → 0 leak through the `SetExpr` JOIN (ISS-P13 /
//! P-379; the F1 leak-free family — Issues' highest-stakes property).
//!
//! **Drill catalogue row ISS-D3** (testing-strategy
//! `01-whole-system-e2e-and-drill-catalogue.md`): *"Cross-tenant + confidential-issue IDOR → not in
//! any board/`SetExpr` JOIN/search/backlink/context-pane for an unauthorized viewer, incl. under
//! zookie staleness. 0 leak."* The green artifact is the **zero-escape counter = 0**.
//!
//! This is the DB-free deterministic drill over the Issues planner ([`myelin_issues::planner`]) — the
//! `SetExpr` lowering is the leak seam (mandatory-core). It proves, with a counter that MUST be 0:
//!
//! 1. **Cross-tenant IDOR:** a viewer with a `view` tuple in tenant `acme` sees NOTHING when the board
//!    scan is scoped to tenant `globex` (the per-tenant index key — no cross-tenant query path).
//! 2. **Confidential IDOR:** a confidential issue (the `- confidential` set-difference, no grant) is
//!    ABSENT from a non-grantee's board — never a count/"N hidden" leak.
//! 3. **The chained-mutation e2e (grant→revoke under zookie staleness):** a granted issue becomes
//!    visible; a revoke advances the watermark; the next zookie-bounded read reflects the revoke (the
//!    revoked issue is absent — read-your-writes, the new-enemy guard).
//!
//! The drill is registered red-until-proven and flips green ONLY when the zero-escape counter is 0
//! across every leak vector. The cross-system board/search/backlink/context-pane re-runs of this same
//! family are ISS-P16/P17 + the surge family ISS-P32/P33; THIS file is the planner-seam artifact.
//!
//! The LIVE-Postgres witness (the same 0-leak property over the real `authz_visible` JOIN) is
//! `tests/integration_iss_p13_setexpr_pushdown.rs` (the `--features integration` proof).

use myelin_identity::{ObjectId, Principal, PrincipalId, PrincipalKind, RelName, SetExpr, Zookie};
use myelin_issues::planner::{issue_id_colref, lower_over_issue_id, AuthzVisibleIndex};
use myelin_tenancy::{Region, TenantId};

fn viewer(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}
fn region() -> Region {
    Region("fr-par".into())
}
fn oid(s: &str) -> ObjectId {
    ObjectId(s.into())
}

/// The frozen `view` set-expr shape (the confidential set-difference, §6.1 / `rebac_fragment`):
/// `(read − confidential) + confidential_grant`.
fn view_set_expr() -> SetExpr {
    let in_rel = |r: &str| SetExpr::InRelation {
        relation: RelName(r.into()),
        via_column: issue_id_colref(),
    };
    SetExpr::Union(vec![
        SetExpr::Difference(Box::new(in_rel("read")), Box::new(in_rel("confidential"))),
        in_rel("confidential_grant"),
    ])
}

/// **ISS-D3 — the zero-escape counter is 0 across cross-tenant + confidential + revoke-under-staleness
/// leak vectors.** The green artifact: `escapes == 0`.
#[test]
fn iss_d3_zero_escape_counter_is_zero() {
    let idx = AuthzVisibleIndex::new();
    let acme = TenantId("acme".into());
    let globex = TenantId("globex".into());

    // ── tenant acme: alice reads ENG-1 (normal) + ENG-2 (confidential, no grant) + ENG-3 (granted).
    idx.grant(&acme, &region(), "alice", "read", "ENG-1", "zk-001");
    idx.grant(&acme, &region(), "alice", "read", "ENG-2", "zk-001");
    idx.grant(&acme, &region(), "alice", "confidential", "ENG-2", "zk-001");
    idx.grant(&acme, &region(), "alice", "read", "ENG-3", "zk-001");
    idx.grant(&acme, &region(), "alice", "confidential", "ENG-3", "zk-001");
    idx.grant(
        &acme,
        &region(),
        "alice",
        "confidential_grant",
        "ENG-3",
        "zk-001",
    );

    let alice = viewer("alice", "acme");
    let lowered = lower_over_issue_id(&view_set_expr(), &alice);
    let universe = vec![oid("ENG-1"), oid("ENG-2"), oid("ENG-3"), oid("ENG-4")];

    // The zero-escape counter: every issue that survives the board JOIN but MUST NOT be visible.
    let mut escapes = 0usize;

    let visible = idx.evaluate(&acme, &region(), &alice, &lowered, &universe);

    // Vector 1 — confidential IDOR: ENG-2 (confidential, no grant) MUST be absent.
    if visible.contains(&oid("ENG-2")) {
        escapes += 1;
    }
    // ENG-4 (no tuple at all) MUST be absent.
    if visible.contains(&oid("ENG-4")) {
        escapes += 1;
    }
    // The legitimate set is exactly {ENG-1, ENG-3} (read-and-not-confidential ∪ granted).
    assert_eq!(
        visible,
        vec![oid("ENG-1"), oid("ENG-3")],
        "the board shows exactly the legitimately-visible issues"
    );

    // Vector 2 — cross-tenant IDOR: the SAME viewer, scoped to globex, sees NOTHING (no cross-tenant
    // query path — the per-tenant index key). Every acme issue surviving a globex-scoped board is an
    // escape.
    let cross = idx.evaluate(&globex, &region(), &alice, &lowered, &universe);
    escapes += cross.len();
    assert!(cross.is_empty(), "no cross-tenant leak");

    // THE GREEN ARTIFACT: the zero-escape counter is 0.
    assert_eq!(escapes, 0, "ISS-D3 zero-escape counter MUST be 0 (0 leak)");
}

/// **ISS-D3 chained-mutation e2e — grant then revoke a confidential grant; the revoke reflects in the
/// next zookie-bounded read; 0 leak.** This is the explicit chained-mutation test the prompt names.
#[test]
fn iss_d3_grant_then_revoke_reflects_under_zookie_zero_leak() {
    let idx = AuthzVisibleIndex::new();
    let acme = TenantId("acme".into());
    let bob = viewer("bob", "acme");

    // bob reads ENG-7, which is confidential. Initially NO grant → ENG-7 absent.
    idx.grant(&acme, &region(), "bob", "read", "ENG-7", "zk-010");
    idx.grant(&acme, &region(), "bob", "confidential", "ENG-7", "zk-010");
    let universe = [oid("ENG-7")];

    let before = idx.evaluate(
        &acme,
        &region(),
        &bob,
        &lower_over_issue_id(&view_set_expr(), &bob),
        &universe,
    );
    assert!(
        before.is_empty(),
        "confidential, ungranted → absent (0 leak)"
    );

    // GRANT the confidential_grant → ENG-7 becomes visible; the watermark advances.
    idx.grant(
        &acme,
        &region(),
        "bob",
        "confidential_grant",
        "ENG-7",
        "zk-011",
    );
    let granted_zookie = idx.watermark(&acme, &region());
    assert_eq!(granted_zookie.0, "zk-011");
    let after_grant = idx.evaluate(
        &acme,
        &region(),
        &bob,
        &lower_over_issue_id(&view_set_expr(), &bob),
        &universe,
    );
    assert_eq!(after_grant, vec![oid("ENG-7")], "the grant re-admits ENG-7");

    // REVOKE the grant → the watermark advances; the next zookie-bounded read MUST reflect it.
    idx.revoke(
        &acme,
        &region(),
        "bob",
        "confidential_grant",
        "ENG-7",
        "zk-012",
    );
    let post_revoke_zookie = idx.watermark(&acme, &region());
    assert_eq!(post_revoke_zookie.0, "zk-012");

    // A security-sensitive scan passes the post-revoke zookie: the index serves (at-or-after) and the
    // revoked issue is ABSENT (read-your-writes, the new-enemy guard — 0 leak under staleness).
    assert!(
        idx.serves(&acme, &region(), &post_revoke_zookie),
        "the index is at-or-after the post-revoke zookie → serves"
    );
    let after_revoke = idx.evaluate(
        &acme,
        &region(),
        &bob,
        &lower_over_issue_id(&view_set_expr(), &bob),
        &universe,
    );
    assert!(
        after_revoke.is_empty(),
        "ISS-D3: the just-revoked grant is absent in the next zookie-bounded read (0 leak)"
    );

    // The new-enemy boundary: a scan requiring a FRESHER revision than the watermark falls back to
    // per-row check (never serves a stale grant).
    assert!(
        !idx.serves(&acme, &region(), &Zookie("zk-013".into())),
        "a scan needing a fresher revision than the watermark falls back to check (never stale)"
    );
}
