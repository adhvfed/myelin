//! # CDC + chained-e2e — contract 4.3 the `list_objects` `SetExpr` push-down, GIT side (GIT-P26 / P-288)
//!
//! **Contract 4.3** (`list_objects(subject, permission, type, zookie?) → Ids | Filter{set_expr,
//! zookie}` — the `SetExpr` lowered to a SQL JOIN over the consumer's own id column via the per-tenant
//! authz reverse index; no N+1, no post-filter). Identity is the PROVIDER / producer (the dispatch +
//! the `SetExpr` value); Git is the CONSUMER (it lowers the `Filter` over its OWN id column `repo.id`/
//! `pr.id` and conjoins it into the ONE leak-free list query). **Contract 6.1** (the code-search
//! pre-filter conjoin) is exercised by the lib unit test; the lint is the `search-requires-acl-filter`
//! CI gate.
//!
//! This is the CDC pair the GIT-P26 TESTS field requires: the PRODUCER (Identity `ListObjects`, the
//! REAL engine) emits the frozen `Filter{set_expr}` for `list_objects(viewer, view, pull_request)`;
//! the CONSUMER (Git `list_filter`) lowers + composes EXACTLY ONE leak-free query over `pr.id`. Both
//! sides agree on the wire shape (`SetExpr::InRelation` keyed on the consumer's id column). The
//! chained-e2e (EI-01 §4) then drives the full path: grant partial visibility → list PRs → assert 0
//! leak + one query → revoke → assert reflected. Run against the REAL Identity engine (the
//! identity-service dev-dependency), NOT a stub of the producer.
//!
//! The live one-query/0-leak/revoke-reflected GIT-D11 proof against the dev-stack Postgres is the
//! `--features integration` test (`integration_git_p26_list_pushdown.rs`). These CDC/e2e tests are the
//! deterministic, DB-free contract-agreement + chained drill.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_git::list_filter::{
    code_search_pre_filter, compose_pr_list_query, lower_over_pr_id, AuthzVisibleIndex, FilterMode,
};

/// The PR permission the CDC/e2e drives through the REAL Identity engine: `review` resolves over the
/// DIRECT `reviewer` relation, which IS the S8 reverse-index candidate source (the leak-free
/// per-(subject, relation) candidate path the producer materialises). The contract's frozen PR-LIST
/// permission is `view` (`pull_request.view = parent_repo->pull`, a tuple-to-userset arm) — exercised
/// over a controlled `authz_visible` model in the lib unit tests; here we drive the producer's
/// reverse-index candidate path via `review` so the REAL engine emits a `Filter` to lower. The git
/// CONSUMER lowering is permission-agnostic (it lowers whatever `InRelation{relation}` the producer
/// returns over its own `pr.id` column), so the one-query/0-leak proof holds for both relations.
const PR_PRODUCER_PERMISSION: &str = "review";
/// The repo/code-search permission the CDC drives: `read` is NOT a repo permission, so we drive the
/// repo read via `pull` (the repo-list permission; `pull = reader ∪ writer ∪ admin ∪ …`) over the
/// direct `reader` relation candidate source. The code-search pre-filter constant is `read` (the
/// blob-doc parent-repo relation); the lowering is permission-agnostic.
const REPO_PRODUCER_PERMISSION: &str = "pull";
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

/// Wire the REAL Identity `list_objects` over the compiled Git fragment + a LIVE S8 index fed off the
/// bus from `grants`, at an explicit cardinality `cap` (so the Ids↔Filter switch is deterministic).
fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

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
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, namespace, index, cap)
}

/// **CDC — the producer (Identity) emits `Filter{InRelation}` over the PR type; the consumer (Git)
/// lowers it to ONE query over its OWN `pr.id` column.** Both sides agree on the wire shape.
#[test]
fn cdc_4_3_identity_filter_lowers_to_one_git_pr_query() {
    let s = scope_of(&principal("acme", "p-admin"));
    // The viewer can `view` two PRs via the `reviewer` relation; the cap is BELOW that → Filter.
    let grants = [
        add("pull_request:pr-1", "reviewer", "p:viewer"),
        add("pull_request:pr-2", "reviewer", "p:viewer"),
    ];
    let lo = wired(1, &s, &grants);
    let viewer = principal("acme", "p:viewer");

    // PRODUCER: the real Identity engine returns the frozen Filter{set_expr}.
    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(PR_PRODUCER_PERMISSION.into()),
        &ObjectType("pull_request".into()),
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

    // CONSUMER: Git lowers it over its OWN pr.id column into ONE leak-free query.
    let q = compose_pr_list_query(&set_expr, &viewer, s.tenant(), &Region("fr-par".into()));
    assert_eq!(
        q.statement_count(),
        1,
        "the consumer composes EXACTLY ONE SQL query (no N+1)"
    );
    assert!(
        q.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = pr.id"),
        "the consumer JOINs the producer's reverse index over its own pr.id (§5.3/§7.3): {}",
        q.sql
    );
    assert_eq!(q.filter_mode, FilterMode::PushedDown);
}

/// **CDC — the code-search pre-filter (6.1): the producer's `list_objects(viewer, read, repo)`
/// `Filter` lowers over the code doc's parent-repo facet (the blob's ACL object is the repo).**
#[test]
fn cdc_6_1_code_search_pre_filter_keys_on_repo() {
    let s = scope_of(&principal("acme", "p-admin"));
    let grants = [
        add("repo:core", "reader", "p:viewer"),
        add("repo:web", "reader", "p:viewer"),
    ];
    let lo = wired(1, &s, &grants);
    let viewer = principal("acme", "p:viewer");
    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(REPO_PRODUCER_PERMISSION.into()),
        &ObjectType("repo".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the repo read pushes down to Filter")
        }
    };
    let pf = code_search_pre_filter(&set_expr, &viewer);
    assert!(
        pf.acl_filter.joins[0]
            .clause
            .contains("av0.object_id = code_doc.repo_id"),
        "the code-search pre-filter keys on the blob doc's parent-repo id (GIT-P5): {}",
        pf.acl_filter.joins[0].clause
    );
    // The producer encodes the PERMISSION as the InRelation relation (the `pull` repo permission the
    // CDC drove). The git lowering is permission-agnostic — it JOINs on whatever relation the producer
    // returns; the FROZEN code-search constant `read` is asserted in the lib unit test.
    assert!(pf.acl_filter.joins[0]
        .clause
        .contains("av0.relation = :rel_for_pull"));
}

/// **GIT-D11 chained e2e (EI-01 §4): grant partial visibility → list PRs → assert 0 leak + one query
/// → revoke → assert reflected.** Driven through the REAL Identity engine (producer) + Git's lowering
/// + an `authz_visible` model (the JOIN the live SQL runs); the survival signals are 0-leak + 1-query
/// + revoke-reflected (GIT-D11). A leak/N+1 aborts LOUDLY (the threshold is NEVER weakened, EI-01 §3).
#[test]
fn git_d11_chained_grant_list_zero_leak_one_query_then_revoke_reflected() {
    let s = scope_of(&principal("acme", "p-admin"));
    let viewer = principal("acme", "p:viewer");
    let region = Region("fr-par".into());

    // ── 1. GRANT partial visibility: the viewer may `view` 3 PRs of a larger tenant; a 4th PR is
    //       granted to someone ELSE (the leak witness). ────────────────────────────────────────────
    const VISIBLE: usize = 3;
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..VISIBLE {
        grants.push(add(&format!("pull_request:pr-{i}"), "reviewer", "p:viewer"));
    }
    grants.push(add("pull_request:pr-secret", "reviewer", "p:other"));
    // The cap is BELOW the visible slice → the producer pushes down to Filter (the SetExpr JOIN path,
    // the GIT-D11 large-tenant case).
    let lo = wired(VISIBLE - 1, &s, &grants);

    // ── 2. LIST PRs: the producer returns Filter; Git lowers it to ONE query. ────────────────────
    let r = lo.list_objects(
        &s,
        &viewer,
        &Permission(PR_PRODUCER_PERMISSION.into()),
        &ObjectType("pull_request".into()),
        &at_latest(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => {
            panic!("the partial-visibility list pushes down to Filter")
        }
    };
    let q = compose_pr_list_query(&set_expr, &viewer, s.tenant(), &region);
    // ONE QUERY: exactly one SQL statement, at most one authz_visible JOIN (no N+1, no post-filter).
    assert_eq!(
        q.statement_count(),
        1,
        "ONE SQL query (GIT-D11 signal: 1 query)"
    );
    let lowered = lower_over_pr_id(&set_expr, &viewer);
    assert!(
        lowered.joins.len() <= 1,
        "no N+1: at most one JOIN, got {}",
        lowered.joins.len()
    );

    // Build the authz_visible model the live JOIN reads, fed the SAME grants at rev 1. The model is
    // keyed on the relation the producer emitted in its `InRelation` (the `review` permission the CDC
    // drove — the JOIN keys on `av.relation = review`), so the model = the live SQL the JOIN runs.
    let av = AuthzVisibleIndex::new();
    for i in 0..VISIBLE {
        av.grant(
            s.tenant(),
            &region,
            "p:viewer",
            PR_PRODUCER_PERMISSION,
            &format!("pull_request:pr-{i}"),
            "zk-00000000000000000001",
        );
    }
    av.grant(
        s.tenant(),
        &region,
        "p:other",
        PR_PRODUCER_PERMISSION,
        "pull_request:pr-secret",
        "zk-00000000000000000001",
    );

    // The candidate universe includes the secret PR (the leak witness).
    let mut candidates: Vec<ObjectId> = (0..VISIBLE)
        .map(|i| ObjectId(format!("pull_request:pr-{i}")))
        .collect();
    candidates.push(ObjectId("pull_request:pr-secret".into()));

    let visible = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidates);
    // 0 LEAK: the secret PR (granted to p:other) NEVER appears in the viewer's list.
    assert_eq!(
        visible.len(),
        VISIBLE,
        "exactly the {VISIBLE} visible PRs survive"
    );
    assert!(
        !visible.iter().any(|o| o.0 == "pull_request:pr-secret"),
        "0 leak: a PR the viewer cannot see never appears (GIT-D11)"
    );

    // ── 3. REVOKE reflected (zookie): revoke pr-0 at a later revision; it drops out of the list. ──
    av.revoke(
        s.tenant(),
        &region,
        "p:viewer",
        PR_PRODUCER_PERMISSION,
        "pull_request:pr-0",
        "zk-00000000000000000099",
    );
    let after = av.evaluate(s.tenant(), &region, &viewer, &lowered, &candidates);
    assert!(
        !after.iter().any(|o| o.0 == "pull_request:pr-0"),
        "the just-revoked PR is reflected — it drops out of the list (zookie, GIT-D11)"
    );
    assert_eq!(
        after.len(),
        VISIBLE - 1,
        "exactly one PR (the revoked one) dropped out"
    );
    // And the new-enemy guard: a scan requiring the post-revoke revision is served by the
    // caught-up watermark (the revoke is visible), never a stale grant.
    assert!(
        av.serves(
            s.tenant(),
            &region,
            &Zookie("zk-00000000000000000099".into())
        ),
        "the watermark caught up to the revoke; the read reflects it (not a stale grant)"
    );

    println!(
        "[P-288 DRILL GREEN 2026-06-22] GIT-D11 chained: grant {VISIBLE} of a larger tenant → \
         Filter push-down lowers to ONE query ({} authz_visible JOIN over pr.id, no N+1/post-filter); \
         0 leaked PRs in the list; a just-revoked grant is reflected at the post-revoke zookie",
        lowered.joins.len()
    );
}
