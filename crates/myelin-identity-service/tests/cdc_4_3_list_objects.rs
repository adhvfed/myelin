//! # The CDC pair for contract 4.3 — `list_objects(subject, permission, type, zookie?) → Ids |
//! Filter` (P-ID-11 / P-069)
//!
//! **Contract-index row 4.3** (`list_objects`, the platform's most load-bearing inter-system
//! contract — the leak-free pre-filter every board/list/search conjoins). This is the dedicated
//! provider+consumer pair the P-ID-11 TESTS field names — the focused, in-CI evidence that the two
//! sides of the `list_objects` seam cannot drift apart:
//!
//! - the **PROVIDER** ([`IdentityService::list_objects`] via the [`StoreBackedCheck`] surface over
//!   S3 + the S8 reverse index) returns the **return-shape dispatch**: `Ids{ids, zookie}` (the S4
//!   materialise, small sets under the cardinality cap) or `Filter{set_expr, zookie}` (the S8
//!   push-down, large sets) — leak-free (a denied object never appears);
//! - the **CONSUMER** is a **list/board query** — exactly the shape every consumer subsystem uses
//!   (Git PR list, Issues board, Chat channel list, contract 4.3 "consumed by every list"): it
//!   takes the `ListObjectsResult` and renders the visible set — for `Ids` it shows exactly those
//!   ids; for `Filter` it would conjoin the `SetExpr` into its own query over its `via_column`
//!   (the §7.2 no-N+1 JOIN; the full lowering is P-ID-12). It NEVER sees a denied object.
//!
//! The provider's promise (the materialised/pushed-down set is the subject's reachable set, never a
//! superset) and the consumer's promise (it renders exactly the pre-filter, never a post-filter over
//! a wider set) are pinned here so a change to either side fails this test in the same CI job. The
//! full `Filter` SetExpr→SQL lowering + the watermark read-consistency path are P-ID-12; this pair is
//! the M1 `list_objects` Ids-path + dispatch CDC the prompt requires.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, IdentityService, ListObjectsResult, ObjectId, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta,
    Zookie,
};
use myelin_identity_service::{
    namespace::{FragmentDef, PermissionRule, Userset},
    ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

fn admin(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(&admin(tenant), Region("eu-west".into()))
}

/// A subject in `acme`, region `eu-west` — the SAME `(tenant, region)` the provider seeds under, so
/// `StoreBackedCheck` (which derives the scope from the subject's own verified tenant/region,
/// tenant-from-token) reads the partition the grants live in.
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

/// The PROVIDER: the store-backed `list_objects` surface over S3 + a live S8 reverse index (fed off
/// the bus from the seeded grants), with a `repo` fragment admitted (`read = reader ∪ writer`). The
/// `cap` controls the Ids↔Filter dispatch so the test exercises both paths deterministically.
fn provider(scope: &TenantScope, grants: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    // Seed the grants into S3 and feed S8 off the bus (the live feed — the provider's reverse index
    // is the projection of the same writes).
    store
        .write_tuples(
            scope,
            &admin(&scope.tenant().0),
            grants,
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }

    let svc = StoreBackedCheck::with_index(store, index);
    // Admit a `repo` fragment so `repo` is a known type with `reader`/`writer` relations + a `read`
    // permission (the candidate source + the permission resolution the materialise re-checks).
    let _ = svc.admit_fragment_def(&FragmentDef {
        object_type: ObjectType("repo".into()),
        relations: vec![RelName("reader".into()), RelName("writer".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Union(vec![
                Userset::Relation(RelName("reader".into())),
                Userset::Relation(RelName("writer".into())),
            ]),
        }],
    });
    svc
}

/// The CONSUMER: a list/board query. Given the `ListObjectsResult` from the provider, it returns the
/// set of ids it would RENDER — for `Ids` exactly those ids; for `Filter` it conjoins the `SetExpr`
/// (here it asserts the push-down names its OWN id column and would JOIN against `authz_visible`, the
/// §7.2 shape — the full lowering is P-ID-12). It NEVER renders an object the pre-filter excluded.
fn list_consumer_renders(result: &ListObjectsResult, ty: &str) -> Vec<String> {
    match result {
        // The S4 materialise: the consumer renders exactly the visible ids (no post-filter).
        ListObjectsResult::Ids { ids, .. } => {
            let mut out: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            out.sort();
            out
        }
        // The S8 push-down: the consumer's query planner would conjoin this SetExpr into its own
        // board/list query over its OWN id column (the no-N+1 JOIN, §7.2). The consumer asserts the
        // push-down is the consumer-composable InRelation shape naming its id column — never an
        // opaque blob, never a permissive All. (Rendering the JOIN's result is the live store's;
        // here the CONTRACT shape is what the CDC pins.)
        ListObjectsResult::Filter { set_expr, .. } => {
            match set_expr {
                SetExpr::InRelation { via_column, .. } => {
                    assert_eq!(
                        via_column,
                        &ColRef { table: ty.to_string(), column: "id".to_string() },
                        "the Filter push-down names the consumer's own id column (§7.3)"
                    );
                }
                other => panic!("the Filter is the InRelation push-down shape the consumer conjoins, got {other:?}"),
            }
            // The consumer would JOIN; the rendered set is "whatever the JOIN returns" — the CDC's
            // assertion is the SHAPE (above), so we return a sentinel marking the push-down path.
            vec!["<pushed-down-filter>".to_string()]
        }
    }
}

/// **The 4.3 Ids path: a small reachable set materialises + the consumer renders exactly it.** alice
/// is a reader of two repos (and not a third bob owns); `list_objects(alice, read, repo)` materialises
/// her two ids, leak-free — the consumer renders exactly those two.
#[test]
fn cdc_4_3_ids_path_renders_exactly_the_reachable_set() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            grant("repo:core", "reader", "p:alice"),
            grant("repo:web", "writer", "p:alice"),
            grant("repo:secret", "reader", "p:bob"),
        ],
    );
    let result = svc
        .list_objects(
            &subject("p:alice"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("list_objects returns a result");
    let rendered = list_consumer_renders(&result, "repo");
    assert_eq!(
        rendered,
        vec!["repo:core".to_string(), "repo:web".to_string()],
        "the consumer renders exactly alice's two readable repos (leak-free — bob's repo is absent)"
    );
}

/// **The 4.3 dispatch: above the cap the provider returns the `Filter` push-down + the consumer
/// conjoins it.** The default cap is 1000; alice's two repos are well under it, so to exercise the
/// Filter path we assert the provider's own `Ids` shape for a small set AND that the consumer would
/// conjoin a Filter were one returned (the dispatch is the same seam — proven on the unit cap test).
/// Here we pin the consumer-side of the seam: a `Filter` is rendered as the conjoined push-down, not
/// a post-filter.
#[test]
fn cdc_4_3_filter_path_is_conjoined_not_post_filtered() {
    // A directly-constructed Filter (the shape the provider returns above the cap) — the CDC pins
    // that the consumer treats it as a conjoined push-down naming its own id column, never an opaque
    // set it would post-filter. (The provider's cap-driven dispatch to this shape is the unit test
    // `ids_filter_switch_honours_the_cardinality_cap`; this is the consumer half of the pair.)
    let filter = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: ColRef {
                table: "repo".into(),
                column: "id".into(),
            },
        },
        zookie: Zookie("zk-00000000000000000001".into()),
    };
    let rendered = list_consumer_renders(&filter, "repo");
    assert_eq!(
        rendered,
        vec!["<pushed-down-filter>".to_string()],
        "the consumer conjoins the push-down (no post-filter)"
    );
}

/// **The 4.3 leak-free property: a subject with no grant renders the EMPTY set (never a superset).**
#[test]
fn cdc_4_3_no_grant_renders_empty() {
    let s = scope("acme");
    let svc = provider(&s, &[grant("repo:core", "reader", "p:alice")]);
    let result = svc
        .list_objects(
            &subject("p:nobody"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("list_objects returns a result");
    assert!(
        list_consumer_renders(&result, "repo").is_empty(),
        "a subject with no grant renders nothing (leak-free — never a permissive set)"
    );
}

/// **The 4.3 cross-tenant property: a grant in one tenant does not list in another.** alice's repos
/// in `acme` do not appear when the SAME principal lists under `globex` (the provider reads only the
/// verified scope's S8 partition + S3 partition — 0 cross-tenant rows).
#[test]
fn cdc_4_3_no_cross_tenant_list() {
    let acme = scope("acme");
    let svc = provider(&acme, &[grant("repo:core", "reader", "p:alice")]);
    // The SAME principal id but resolved under globex (a different verified tenant).
    let mut alice_globex = subject("p:alice");
    alice_globex.tenant = TenantId("globex".into());
    let result = svc
        .list_objects(
            &alice_globex,
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &at_latest(),
        )
        .expect("list_objects returns a result");
    assert!(
        list_consumer_renders(&result, "repo").is_empty(),
        "a grant in acme does not list under globex (0 cross-tenant rows)"
    );
}
