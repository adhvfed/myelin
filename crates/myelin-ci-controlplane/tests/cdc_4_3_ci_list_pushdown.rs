//! # The CDC pair for CI's CONSUMED `list_objects` SetExpr push-down — row 4.3 (CI-P25 → P-368, M4)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row **4.3**
//! (`list_objects → Ids | Filter{set_expr, zookie}` — the SetExpr lowered to a SQL JOIN over the
//! consumer's OWN id column via the per-tenant authz reverse index; **no N+1, no post-filter**).
//! Owning architecture:
//! `continuous-integration/architecture/03-events-contracts-and-glue.md` §5.1 (the push-down over
//! `ci_run.run_id`). Reconciliation: `00-reconciliation-decisions.md` §OQ-E.
//!
//! ## What this CDC pins (the PROVIDER ↔ CONSUMER no-drift property)
//! - **PROVIDER** (Identity): returns `Filter { set_expr, zookie }` for the large run space — the
//!   frozen `SetExpr` algebra (modelled here as the `InRelation{read}` the engine returns for
//!   `list_objects(viewer, read, ci_run)`).
//! - **CONSUMER** (CI): LOWERS that `set_expr` over its OWN `ci_run.run_id` column
//!   ([`myelin_ci_controlplane::lower_over_run_id`] / [`compose_run_list_query`]) into ONE leak-free
//!   list query — a JOIN against `authz_visible`, NO N+1 per-row check, NO post-filter; the
//!   `search-requires-acl-filter` lint conjoins the `Filter` BEFORE scoring.
//!
//! The CONSUMER's lowering SHAPE is the wire contract (restated in CI because a producer LEAF cannot
//! depend on the Identity SERVICE crate, §2.9). This CDC proves CI lowers the FROZEN `SetExpr`
//! variants exactly — a drift in the algebra would change the lowered SQL / the surviving row set.

use myelin_ci_controlplane::{
    ci_run_id_colref, compose_run_list_query, lower_over_run_id, AuthzVisibleIndex,
};
use myelin_identity::{ObjectId, Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_tenancy::{Region, TenantId};

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn region() -> Region {
    Region("fr-par".into())
}

/// The `Filter{set_expr}` the PROVIDER (Identity) returns for `list_objects(viewer, read, ci_run)` —
/// the `read` relation reverse-index lookup (`run.read = run.view − is_untrusted_fork` resolves on
/// the engine side; the consumer receives the `InRelation{read}` keyed on its own `run_id`).
fn provider_set_expr() -> SetExpr {
    SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: ci_run_id_colref(),
    }
}

/// **PROVIDER → CONSUMER:** CI lowers the frozen `SetExpr` over `ci_run.run_id` into ONE leak-free
/// statement — the `authz_visible` JOIN in the FROM, the ACL predicate conjoined BEFORE the
/// ORDER BY / LIMIT (pre-filter, never post-filter), the tenant predicate always emitted.
#[test]
fn consumer_lowers_the_provider_filter_into_one_leak_free_query() {
    let q = compose_run_list_query(&provider_set_expr(), &viewer("alice"), &tenant(), &region());

    // ONE query — no N+1, no second statement.
    assert_eq!(q.statement_count(), 1);
    // the reverse-index JOIN over CI's own id column.
    assert!(q
        .sql
        .contains("JOIN authz_visible av0 ON av0.object_id = ci_run.run_id"));
    // the tenant predicate is always emitted (no cross-tenant query path).
    assert!(q.sql.contains("ci_run.tenant_id = :tenant"));
    // the ACL predicate precedes pagination.
    assert!(q.sql.find("WHERE").unwrap() < q.sql.find("ORDER BY").unwrap());
    // bound, never interpolated — the viewer subject lives in params.
    assert!(q.params.iter().any(|p| p.value == "alice"));
}

/// **PROVIDER → CONSUMER (the leak-free property):** a partial-visibility run list returns ONLY the
/// rows the viewer may `read` via the JOIN — 0 leaked rows, revoke reflected. This is the row set the
/// live SQL `WHERE`/JOIN keeps (modelled against the in-memory `authz_visible` index).
#[test]
fn consumer_pushdown_is_leak_free_and_revoke_reflected() {
    let idx = AuthzVisibleIndex::new();
    let alice = viewer("alice");
    idx.grant(&tenant(), &region(), "alice", "read", "r1");
    idx.grant(&tenant(), &region(), "alice", "read", "r3");

    let lowered = lower_over_run_id(&provider_set_expr(), &alice);
    let candidates = vec![
        ObjectId("r1".into()),
        ObjectId("r2".into()), // confidential — alice has no `read` tuple.
        ObjectId("r3".into()),
    ];
    let visible = idx.evaluate(&tenant(), &region(), &alice, &lowered, &candidates);
    assert_eq!(
        visible,
        vec![ObjectId("r1".into()), ObjectId("r3".into())],
        "0 leaked rows — the confidential r2 never survives the JOIN"
    );

    // revoke reflected.
    idx.revoke(&tenant(), &region(), "alice", "read", "r3");
    let after = idx.evaluate(&tenant(), &region(), &alice, &lowered, &candidates);
    assert_eq!(after, vec![ObjectId("r1".into())]);
}
