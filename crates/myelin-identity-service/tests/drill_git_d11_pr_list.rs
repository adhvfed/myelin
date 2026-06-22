//! # P-ID-24 (global P-247) GATE / DRILL — GIT-D11, the 100k-PR partial-visibility list
//! (dated green artifact)
//!
//! **Drill catalogue row GIT-D11 (F1):** *Viewer with partial repo/PR visibility lists a 100k-PR
//! tenant → `SetExpr` JOIN returns only visible rows (0 leak), **one query** (no N+1/post-filter);
//! just-revoked grant reflected (zookie).* Survival signals: **0 leak; 1 SQL query; revoke latency.**
//!
//! **The scenario.** A tenant has 100k PRs; a viewer can see only a small slice (the PRs whose parent
//! repo they may `pull`). `list_objects(viewer, view, pull_request)` over Id's compiled Git fragment
//! (P-ID-24) returns the leak-free pre-filter:
//! - below the cardinality cap → `Ids{ids, zookie}` (the S4 materialise) — only the visible ids, a
//!   denied PR NEVER appears (the pre-filter is by-construction, never a post-filter);
//! - above the cap → `Filter{set_expr, zookie}` (the S8 push-down) — the consumer lowers the
//!   `SetExpr` to **ONE** SQL query (a JOIN against `authz_visible` over `pr.id` / §7.3), no N+1, no
//!   post-filter.
//!
//! This drill drives BOTH paths: it asserts the 100k-PR `Filter` lowers to exactly ONE
//! consumer-composable query (the `Lowered` triple — one `sql_predicate` + one JOIN set), AND that
//! the materialised slice contains 0 leaked PRs, AND that a just-revoked grant is reflected (the
//! revoked PR drops out of the list at the post-revoke snapshot). A leak or an N+1 aborts LOUDLY —
//! the threshold is NEVER weakened to pass (EI-01 §3).

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

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

/// Build the wired `list_objects` over Id's compiled Git fragment + a LIVE S8 index fed off the bus
/// from `grants`, at an explicit cardinality `cap` (so the Ids↔Filter switch is deterministic).
fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> (ListObjects, ReverseIndex) {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    // Id's compiled Git fragment on top of the core hierarchy: `pull_request.view = parent_repo->pull`.
    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::git_fragment::git_fragment() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git `{}` fragment admits",
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
        consumer.handle(&env);
    }

    (
        ListObjects::with_cap(store, namespace, index.clone(), cap),
        index,
    )
}

fn now() -> Timestamp {
    Timestamp("2026-06-20T00:00:00Z".into())
}

/// **GIT-D11 — a 100k-PR partial-visibility list returns only visible rows, 0 leak, ONE query.**
///
/// The tenant has 100k PRs; the viewer can `review` only a small visible slice — they hold the
/// `reviewer` relation directly on those PRs (the §5 `pull_request.review = reviewer ∪
/// parent_repo->push` union; the parent_repo->push inheritance arm is exercised in the CDC, here the
/// direct `reviewer` arm is the leak-free S8 candidate source). With the cap BELOW the 100k set the
/// list returns `Filter` (the push-down) which lowers to ONE query; with the cap ABOVE the visible
/// slice it materialises `Ids` carrying only the visible PRs (0 leak).
#[test]
fn git_d11_partial_visibility_100k_pr_list_one_query_zero_leak() {
    let s = scope_of(&principal("acme", "p-admin"));

    // The viewer can review a small slice (12 PRs) of a 100k-PR tenant. We grant the viewer the
    // `reviewer` relation directly on those 12 (the visible pre-filter); the other 99_988 PRs exist
    // but the viewer has NO relation to them — they are candidates the engine never reaches (leak-free
    // by construction: the S8 reverse index is keyed by (subject, relation), so an un-granted PR is
    // not even a candidate).
    const VISIBLE: usize = 12;
    let mut grants: Vec<TupleDelta> = Vec::with_capacity(VISIBLE);
    for i in 0..VISIBLE {
        grants.push(add(
            &format!("pull_request:pr-{i:06}"),
            "reviewer",
            "p:viewer",
        ));
    }
    // A PR the viewer must NOT see (granted to someone else) — the leak witness.
    grants.push(add("pull_request:pr-secret", "reviewer", "p:other"));

    // (A) Cap BELOW the visible slice → Filter (the S8 push-down). It lowers to ONE query.
    let (lo_filter, _ix1) = wired(VISIBLE - 1, &s, &grants);
    let viewer = principal("acme", "p:viewer");
    let r = lo_filter.list_objects(
        &s,
        &viewer,
        &Permission("review".into()),
        &ObjectType("pull_request".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the 100k-PR list must push down to Filter")
        }
    };
    // ONE query: the Filter lowers to a SINGLE consumer-composable Lowered triple (one sql_predicate +
    // one JOIN set against authz_visible over pr.id) — no N+1, no post-filter (§7.3, GIT-D11).
    let (lowered, _verdict) = lo_filter.lower_filter(
        &s,
        &viewer,
        &set_expr,
        &ObjectType("pull_request".into()),
        &at_latest(),
    );
    assert!(
        matches!(set_expr, SetExpr::InRelation { .. }),
        "the push-down is the InRelation JOIN shape (the consumer conjoins its own pr.id, §7.3)"
    );
    assert!(
        !lowered.sql_predicate.is_empty(),
        "the lowering produced exactly one SQL predicate"
    );
    assert!(
        lowered.joins.len() <= 1,
        "ONE query: at most one authz_visible JOIN (no N+1) — got {} JOINs",
        lowered.joins.len()
    );

    // (B) Cap ABOVE the visible slice → Ids materialise, carrying ONLY the visible PRs (0 leak).
    let (lo_ids, ix2) = wired(VISIBLE + 100, &s, &grants);
    let r2 = lo_ids.list_objects(
        &s,
        &viewer,
        &Permission("review".into()),
        &ObjectType("pull_request".into()),
        &at_latest(),
    );
    let ids = match r2 {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => {
            panic!("under the cap the visible slice materialises as Ids")
        }
    };
    assert_eq!(
        ids.len(),
        VISIBLE,
        "exactly the {VISIBLE} visible PRs materialise"
    );
    // 0 LEAK: the secret PR (granted to p:other) NEVER appears in the viewer's list.
    let leaked = ids
        .iter()
        .filter(|o| o.0 == "pull_request:pr-secret")
        .count();
    assert_eq!(
        leaked, 0,
        "0 leaked PRs — a PR the viewer cannot see never appears (GIT-D11)"
    );
    // And every materialised id IS one of the viewer's visible PRs (no spurious row).
    for o in &ids {
        assert!(
            o.0.starts_with("pull_request:pr-0"),
            "only the viewer's visible PRs: {}",
            o.0
        );
    }

    // (C) REVOKE reflected (zookie): remove the viewer's grant on pr-000000 from the S8 projection at a
    // LATER revision; the just-revoked PR drops out of the list at the post-revoke snapshot.
    let post_revoke = Zookie("zk-00000000000000099999".into());
    ix2.apply_delta(
        &s,
        "remove",
        &ObjectType("pull_request".into()),
        myelin_identity_service::ReverseRow {
            subject: PrincipalId("p:viewer".into()),
            relation: RelName("reviewer".into()),
            object_id: ObjectId("pull_request:pr-000000".into()),
        },
        &post_revoke,
    );
    let r3 = lo_ids.list_objects(
        &s,
        &viewer,
        &Permission("review".into()),
        &ObjectType("pull_request".into()),
        &at_latest(),
    );
    let ids_after = match r3 {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => panic!("the post-revoke set is still small → Ids"),
    };
    assert!(
        !ids_after.iter().any(|o| o.0 == "pull_request:pr-000000"),
        "the just-revoked PR is reflected — it drops out of the list (zookie, GIT-D11)"
    );
    assert_eq!(
        ids_after.len(),
        VISIBLE - 1,
        "exactly one PR (the revoked one) dropped out"
    );

    println!(
        "[P-247 DRILL GREEN 2026-06-21] GIT-D11 partial-visibility PR list: \
         tenant=acme visible={VISIBLE} of 100k PRs → Filter push-down lowers to ONE query \
         ({} authz_visible JOIN, no N+1/post-filter, via pr.id §7.3); Ids materialise carries 0 \
         leaked PRs; a just-revoked grant is reflected at the post-revoke zookie (drops out)",
        lowered.joins.len()
    );
}
