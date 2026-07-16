//! # CDC + chained-e2e — contract 4.3 the `list_objects` `SetExpr` push-down, KNOWLEDGE side (KN-P16 / P-306)
//!
//! **Contract 4.3** (`list_objects(subject, permission, type, zookie?) → Ids | Filter{set_expr,
//! zookie}` — the `SetExpr` lowered to a SQL JOIN over the consumer's own id column via the per-tenant
//! authz reverse index; no N+1, no post-filter). Identity is the PROVIDER / producer (the dispatch +
//! the `SetExpr` value); Knowledge is the CONSUMER (it lowers the returned `Filter` over its OWN
//! `db_row.id` column and conjoins it into the ONE leak-free db view query AND the permission-correct
//! `COUNT(*)` — the KN-D5 count-leak-closed shape that distinguishes the Knowledge consumer from the
//! sibling git push-down).
//!
//! This is the CDC pair the KN-P16 TESTS field requires: the PRODUCER (Identity `ListObjects`, the
//! REAL engine over the compiled Knowledge `database_row` fragment) emits the frozen
//! `Filter{set_expr}` for `list_objects(viewer, read, database_row)`; the CONSUMER (Knowledge
//! `list_filter`) lowers + composes EXACTLY ONE leak-free view query AND ONE permission-correct COUNT
//! over `db_row.id`. Both sides agree on the wire shape (`SetExpr::InRelation` keyed on the consumer's
//! `db_row.id` column — the §7.3 mapping the producer's `via_column_for(database_row)` and the
//! consumer's `db_row_id_colref()` BOTH name). The chained-e2e (EI-01 §4) then drives the full KN-D5
//! path: grant partial row visibility → list rows + COUNT → assert 0 leak + 0 count-leak + one query
//! → revoke → assert reflected.
//!
//! Run against the REAL Identity engine (the identity-service dev-dependency), NOT a stub of the
//! producer. The live one-query / 0-leak / 0-count-leak / revoke-reflected KN-D5 proof against the
//! dev-stack Postgres is the `--features integration` test
//! (`integration_kn_d5_list_pushdown.rs`). These CDC/e2e tests are the deterministic, DB-free
//! contract-agreement + chained drill.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer, TupleStore,
};
use myelin_knowledge::{
    compose_db_count_query, compose_db_view_query, db_row_id_colref, lower_over_db_row_id,
    AuthzVisibleIndex, FilterMode,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

/// The row-list permission the producer resolves over the compiled `database_row` fragment: `read`
/// (`direct_reader ∪ parent_page->read`). The producer encodes the permission as the `InRelation`
/// relation; the Knowledge consumer lowering is permission-agnostic (it JOINs on whatever relation the
/// producer returns over `db_row.id`).
const ROW_PRODUCER_PERMISSION: &str = "read";

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("fr-par".into());
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

fn now() -> Timestamp {
    Timestamp("2026-06-22T00:00:00Z".into())
}

/// Wire the REAL Identity `list_objects` over the compiled Knowledge fragment + a LIVE S8 index fed
/// off the bus from `grants`, at an explicit cardinality `cap` (so the Ids↔Filter switch is
/// deterministic). Mirrors the `cdc_4_3_git_list_pushdown.rs` `wired` helper — the SAME producer
/// machinery, the Knowledge fragment.
fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::knowledge_fragment::knowledge_fragment() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Knowledge `{}` fragment admits",
            def.object_type.0
        );
    }

    store
        .write_tuples(
            scope,
            &principal(&scope.tenant().0, "p-admin"),
            grants,
            None,
            None,
            now(),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, namespace, index, cap)
}

/// **CDC — the producer (Identity) emits `Filter{InRelation}` over the `database_row` type; the
/// consumer (Knowledge) lowers it to ONE view query AND ONE permission-correct COUNT over its OWN
/// `db_row.id` column.** Both sides agree on the wire shape (the §7.3 `db_row.id` via_column).
#[test]
fn cdc_4_3_identity_filter_lowers_to_one_knowledge_view_and_count_query() {
    let s = scope_of(&principal("acme", "p-admin"));
    // The viewer reads two rows via `direct_reader`; the cap is BELOW that → the producer pushes down
    // to `Filter` (the SetExpr JOIN path).
    let grants = [
        add("database_row:row-1", "direct_reader", "p:viewer"),
        add("database_row:row-2", "direct_reader", "p:viewer"),
    ];
    let lo = wired(1, &s, &grants);
    let viewer = principal("acme", "p:viewer");

    // PRODUCER: the real Identity engine returns the frozen Filter{set_expr}.
    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(ROW_PRODUCER_PERMISSION.into()),
        &ObjectType("database_row".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => panic!("above the cap the producer pushes down to Filter"),
    };
    assert!(
        matches!(set_expr, SetExpr::InRelation { .. }),
        "the producer emits the InRelation push-down shape"
    );
    // The producer names the consumer's OWN id column (§7.3): `db_row.id`, NOT `database_row.id`.
    if let SetExpr::InRelation { ref via_column, .. } = set_expr {
        assert_eq!(
            via_column,
            &db_row_id_colref(),
            "the producer + consumer agree on the §7.3 db_row.id via_column"
        );
    }

    // CONSUMER (the VIEW): Knowledge lowers it over its OWN db_row.id column into ONE leak-free query.
    let view = compose_db_view_query(&set_expr, &viewer, s.tenant(), "db:projects");
    assert_eq!(
        view.statement_count(),
        1,
        "the consumer composes EXACTLY ONE view query (no N+1)"
    );
    assert!(
        view.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = db_row.id"),
        "the consumer JOINs the producer's reverse index over its own db_row.id (§4.1/§7.3): {}",
        view.sql
    );
    assert_eq!(view.filter_mode, FilterMode::PushedDown);

    // CONSUMER (the COUNT — the KN-D5 headline): the SAME ACL conjunct INSIDE a SELECT COUNT(*).
    let count = compose_db_count_query(&set_expr, &viewer, s.tenant(), "db:projects");
    assert_eq!(
        count.statement_count(),
        1,
        "the consumer composes EXACTLY ONE COUNT query"
    );
    assert!(count.is_count && count.sql.starts_with("SELECT COUNT(*) FROM db_row"));
    assert!(
        count.sql.contains("AND (av0.object_id IS NOT NULL)"),
        "the ACL is conjoined INSIDE the COUNT (the count-leak closed by construction): {}",
        count.sql
    );
}

/// **KN-D5 chained e2e (EI-01 §4): grant partial row visibility → list rows + COUNT → assert 0 leak +
/// 0 count-leak + one query → revoke → assert reflected.** Driven through the REAL Identity engine
/// (producer) + Knowledge's lowering + an `authz_visible` model (the JOIN the live SQL runs); the
/// survival signals are 0-leak + 0-count-leak + 1-query + revoke-reflected (KN-D5). A leak / count-leak
/// / N+1 aborts LOUDLY (the threshold is NEVER weakened, EI-01 §3).
#[test]
fn kn_d5_chained_grant_list_zero_leak_zero_count_leak_one_query_then_revoke_reflected() {
    let s = scope_of(&principal("acme", "p-admin"));
    let viewer = principal("acme", "p:viewer");
    let region = Region("fr-par".into());

    // ── 1. GRANT partial visibility: the viewer may `read` 3 rows of a larger db; a 4th row is granted
    //       to someone ELSE (the leak witness). ──────────────────────────────────────────────────────
    const VISIBLE: usize = 3;
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..VISIBLE {
        grants.push(add(
            &format!("database_row:row-{i}"),
            "direct_reader",
            "p:viewer",
        ));
    }
    grants.push(add("database_row:row-secret", "direct_reader", "p:other"));
    // The cap is BELOW the visible slice → the producer pushes down to Filter (the SetExpr JOIN path).
    let lo = wired(VISIBLE - 1, &s, &grants);

    // ── 2. LIST rows + COUNT: the producer returns Filter; Knowledge lowers it to ONE query each. ────
    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(ROW_PRODUCER_PERMISSION.into()),
        &ObjectType("database_row".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("the partial-visibility list pushes down to Filter")
        }
    };
    let view = compose_db_view_query(&set_expr, &viewer, s.tenant(), "db:projects");
    let count_q = compose_db_count_query(&set_expr, &viewer, s.tenant(), "db:projects");
    assert_eq!(
        view.statement_count(),
        1,
        "ONE view query (KN-D5 signal: 1 query)"
    );
    assert_eq!(count_q.statement_count(), 1, "ONE COUNT query");
    let lowered = lower_over_db_row_id(&set_expr, &viewer);
    assert!(
        lowered.joins.len() <= 1,
        "no N+1: at most one JOIN, got {}",
        lowered.joins.len()
    );

    // Build the authz_visible model the live JOIN reads, fed the SAME grants at rev 1. The model is
    // keyed on the relation the producer emitted in its `InRelation` (the `read` permission — the JOIN
    // keys on `av.relation = read`), so the model = the live SQL the JOIN runs.
    let av = AuthzVisibleIndex::new();
    for i in 0..VISIBLE {
        av.grant(
            s.tenant(),
            &region,
            "p:viewer",
            ROW_PRODUCER_PERMISSION,
            &format!("database_row:row-{i}"),
            "zk-0000000001",
        );
    }
    av.grant(
        s.tenant(),
        &region,
        "p:other",
        ROW_PRODUCER_PERMISSION,
        "database_row:row-secret",
        "zk-0000000001",
    );

    // The candidate universe includes the secret row (the leak witness).
    let mut candidates: Vec<String> = (0..VISIBLE)
        .map(|i| format!("database_row:row-{i}"))
        .collect();
    candidates.push("database_row:row-secret".into());
    let candidate_refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();

    let visible = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    // 0 LEAK: the secret row (granted to p:other) NEVER appears in the viewer's list.
    assert_eq!(
        visible.len(),
        VISIBLE,
        "exactly the {VISIBLE} visible rows survive"
    );
    assert!(
        !visible.iter().any(|o| o == "database_row:row-secret"),
        "0 leak: a row the viewer cannot see never appears (KN-D5)"
    );
    // 0 COUNT-LEAK: the permission-correct COUNT is exactly the visible cardinality, NOT the universe.
    let n = av.count_visible(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert_eq!(n, VISIBLE, "0 count-leak: COUNT = {VISIBLE} (the visible rows), NOT {} (the universe incl. the secret)", candidate_refs.len());
    assert_eq!(
        n,
        visible.len(),
        "the COUNT equals the listed cardinality — no second path can diverge"
    );

    // ── 3. REVOKE reflected (zookie / read-your-writes): revoke row-0 at a later revision; it drops out
    //       of BOTH the list and the COUNT (the new-enemy guard, KN-D5/4.10). ──────────────────────────
    av.revoke(
        s.tenant(),
        &region,
        "p:viewer",
        ROW_PRODUCER_PERMISSION,
        "database_row:row-0",
        "zk-0000000099",
    );
    let after = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert!(
        !after.iter().any(|o| o == "database_row:row-0"),
        "the just-revoked row is reflected — it drops out of the list (zookie, KN-D5)"
    );
    assert_eq!(
        after.len(),
        VISIBLE - 1,
        "exactly one row (the revoked one) dropped out"
    );
    let n_after = av.count_visible(s.tenant(), &region, &viewer, &lowered, &candidate_refs);
    assert_eq!(n_after, VISIBLE - 1, "0 count-leak after revoke: the COUNT decremented — a revoked grant cannot be counted stale");

    // The new-enemy guard: a scan requiring the post-revoke revision is served by the caught-up
    // watermark (the revoke is visible), never a stale grant.
    assert!(
        av.serves(s.tenant(), &region, &Zookie("zk-0000000099".into())),
        "the watermark caught up to the revoke's revision → the JOIN serves (no stale grant)"
    );

    println!(
        "[P-306 CDC GREEN] KN-D5 db-row-list + COUNT SetExpr push-down: the REAL Identity producer \
         emits Filter{{InRelation read, db_row.id}}; Knowledge lowers it to ONE view + ONE COUNT over \
         db_row.id — {VISIBLE} visible of {} rows (0 leak: row-secret absent), COUNT={n} (0 count-leak); \
         a revoke drops row-0 from BOTH the list AND the COUNT (read-your-writes).",
        candidate_refs.len()
    );
}
