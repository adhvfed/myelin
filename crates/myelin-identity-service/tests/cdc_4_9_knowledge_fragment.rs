//! # The CDC pair for contract 4.9 — Id's compiled **Knowledge** ReBAC fragment (P-ID-26 / P-249)
//!
//! **Contract-index row 4.9** (per-subsystem ReBAC namespace fragment — each subsystem declares
//! relations and permissions, compiled into ONE cell schema; Identity owns the engine, the
//! admit-contract, and the core hierarchy and never invents object ids). The engine half is pinned by
//! `cdc_4_9_namespace_engine.rs` (P-068); the Git fragment by `cdc_4_9_git_fragment.rs` (P-247). THIS
//! file pins the Identity-side compiled **Knowledge** fragment (the rich rewrites Id owns, P-ID-26):
//! the page-tree-with-overrides rewrite, the row-level ACL, and the field caveat.
//!
//! - The **PROVIDER** is Identity's namespace engine ([`StoreBackedCheck`] over `with_core_hierarchy`):
//!   it admits Id's compiled Knowledge [`FragmentDef`]s, resolves the Knowledge permissions through
//!   the four userset operators (the `- direct_block` exclusion is the headline), and never invents
//!   an id.
//! - The **CONSUMER** is the Knowledge subsystem, which gates page/row reads ONLY on a resolved grant
//!   — modelled here through the 4.2 `check` surface (`page.read`, `database_row.read`, the
//!   `view_field` field caveat) + the 4.3 `list_objects` row conjoin.
//!
//! `myelin-identity-service` does NOT depend on a Knowledge leaf crate (the §2.9 DAG floor); the
//! name-agreement is asserted against the architecture §5 frozen vocabulary here.

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, Literal, ObjectId,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    knowledge_fragment, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

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

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

/// The PROVIDER surface seeded with `tuples` (the core hierarchy is preloaded; Id's compiled
/// Knowledge fragment is admitted on top).
fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-21T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env);
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_knowledge_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Knowledge fragment admits: {admit:?}"
        );
    }
    svc
}

/// **PROVIDER → the compiled Knowledge fragment ADMITS into the cell schema (the engine-only-floor
/// progression).** Id declares + compiles its Knowledge fragment via the fragment-admit contract;
/// every Knowledge object type admits on top of the core hierarchy.
#[test]
fn cdc_4_9_id_compiled_knowledge_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["space", "page", "block", "database_row"] {
        assert!(ns.object_types().contains(&ty.to_string()), "`{ty}` admitted into the cell schema");
    }
    // page.read (the page-tree-with-overrides) + database_row.read (the row ACL) are compiled.
    assert!(ns.resolve_permission("page", "read").is_some());
    assert!(ns.resolve_permission("database_row", "read").is_some());
}

/// **CONSUMER → PROVIDER: page-tree inheritance resolves (the `parent_page->read` rewrite).** A child
/// page inherits its parent's readers; a direct reader on the child reads; an outsider is denied
/// (fail-closed).
#[test]
fn cdc_4_9_page_tree_inheritance_resolves() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // page:child inherits read from page:parent (parent_page->read).
            add("page:child", "parent_page", "page:parent#read"),
            // alice is a direct reader of the parent → she inherits the child too.
            add("page:parent", "direct_reader", "p:alice"),
            // bob is a direct reader of the child only.
            add("page:child", "direct_reader", "p:bob"),
        ],
    );
    let child = ArtifactRef("page:child".into());
    let can_read = |actor: &Principal| {
        matches!(
            svc.check(actor, &Permission("read".into()), &child, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    assert!(can_read(&subject("p:alice")), "a parent-page reader inherits the child (parent_page->read)");
    assert!(can_read(&subject("p:bob")), "a direct reader of the child reads it");
    assert!(!can_read(&subject("p:carol")), "an outsider cannot read (fail-closed)");
}

/// **CONSUMER → PROVIDER: an OVERRIDE (`direct_block`) narrows inherited access (§5, the headline
/// rewrite).** A subject who inherits read from the parent page is REMOVED from the child's read set
/// by a `- direct_block` tuple on the child — by construction, not a post-filter.
#[test]
fn cdc_4_9_direct_block_override_narrows_inherited_access() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("page:child", "parent_page", "page:parent#read"),
            // alice inherits read from the parent.
            add("page:parent", "direct_reader", "p:alice"),
            // ...but the child BLOCKS alice (the override): she must NOT read the sub-page.
            add("page:child", "direct_block", "p:alice"),
            // bob inherits and is NOT blocked → he still reads the child.
            add("page:parent", "direct_reader", "p:bob"),
        ],
    );
    let parent = ArtifactRef("page:parent".into());
    let child = ArtifactRef("page:child".into());
    let can_read = |actor: &Principal, obj: &ArtifactRef| {
        matches!(
            svc.check(actor, &Permission("read".into()), obj, &at_latest(), None),
            Ok(Decision::Allow)
        )
    };
    // alice reads the PARENT (she is a direct reader there)...
    assert!(can_read(&subject("p:alice"), &parent), "alice reads the parent (direct_reader)");
    // ...but the child's `- direct_block` OVERRIDE removes her from the child's read set.
    assert!(
        !can_read(&subject("p:alice"), &child),
        "the - direct_block override narrows alice's inherited access (she does NOT read the sub-page)"
    );
    // bob is not blocked → he still inherits read on the child.
    assert!(can_read(&subject("p:bob"), &child), "an un-blocked inheriting reader still reads the child");
}

/// **CONSUMER → PROVIDER: the row-level ACL conjoins via `list_objects` (`database_row.read`).** A
/// `list_objects(viewer, read, database_row)` returns ONLY the rows the viewer may read — an
/// un-readable row is ABSENT (the row-level pre-filter, not a post-filter).
#[test]
fn cdc_4_9_row_level_acl_conjoins_via_list_objects() {
    use myelin_identity::{ListObjectsResult, ObjectType};
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            // viewer is a direct reader of two rows; a third row is granted to someone else.
            add("database_row:r1", "direct_reader", "p:viewer"),
            add("database_row:r2", "direct_reader", "p:viewer"),
            add("database_row:r-secret", "direct_reader", "p:other"),
        ],
    );
    let result = svc
        .list_objects(
            &subject("p:viewer"),
            &Permission("read".into()),
            &ObjectType("database_row".into()),
            &at_latest(),
        )
        .expect("list_objects over the row ACL");
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids,
        ListObjectsResult::Filter { .. } => panic!("a small visible set materialises as Ids"),
    };
    assert_eq!(ids.len(), 2, "exactly the viewer's two readable rows");
    assert!(
        !ids.iter().any(|o| o.0 == "database_row:r-secret"),
        "0 leak: the row granted to someone else NEVER appears in the viewer's list"
    );
}

/// **CONSUMER → PROVIDER: a field caveat hides a column through the ONE `QueryAst` core (§8.6, C3).**
/// The row is visible (the row ACL above); `check(viewer, view_field, row, caveat)` then redacts the
/// `salary` column for an under-cleared viewer (Deny) and reveals it for a cleared one (Allow) — a
/// missing-clearance viewer is Conditional, never a silent allow.
#[test]
fn cdc_4_9_field_caveat_hides_a_column() {
    let s = scope("acme");
    // The row itself is readable by the viewer (the row ACL); the field caveat gates a column on top.
    let svc = provider(&s, &[add("database_row:emp-1", "direct_reader", "p:viewer")]);
    let row = ArtifactRef("database_row:emp-1".into());

    // Sanity: the viewer reads the ROW (the row ACL holds).
    assert_eq!(
        svc.check(&subject("p:viewer"), &Permission("read".into()), &row, &at_latest(), None),
        Ok(Decision::Allow),
        "the viewer reads the row (the row-level ACL); the field caveat gates a column on top"
    );

    // The salary column is "visible iff clearance ≥ 3" — a non-literal predicate over the ONE core.
    let cleared = knowledge_fragment::field_view_caveat(
        "database_row:emp-1",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(5))],
    );
    assert_eq!(
        svc.check(&subject("p:viewer"), &Permission("view_field".into()), &row, &at_latest(), Some(&cleared)),
        Ok(Decision::Allow),
        "a cleared viewer sees the salary column"
    );

    let under = knowledge_fragment::field_view_caveat(
        "database_row:emp-1",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[("clearance", Literal::Int(1))],
    );
    assert_eq!(
        svc.check(&subject("p:viewer"), &Permission("view_field".into()), &row, &at_latest(), Some(&under)),
        Ok(Decision::Deny),
        "an under-cleared viewer's salary column is redacted (Deny) — absent, not a post-filter"
    );

    let missing = knowledge_fragment::field_view_caveat(
        "database_row:emp-1",
        "salary",
        "ge",
        "clearance",
        Literal::Int(3),
        &[],
    );
    assert_eq!(
        svc.check(&subject("p:viewer"), &Permission("view_field".into()), &row, &at_latest(), Some(&missing)),
        Ok(Decision::Conditional),
        "a field caveat needing missing context is Conditional, never a silent allow (§8.6)"
    );
}
