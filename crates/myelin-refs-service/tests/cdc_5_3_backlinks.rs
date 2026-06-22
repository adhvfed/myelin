//! **REF-P11 / P-160 — the permission-filtered backlink read (contract 5.3) provider+consumer CDC
//! pair, plus the consumer CDC for the 4.3 `list_objects` `SetExpr` Refs lowers.**
//!
//! Contract 5.3 is `backlinks(target, viewer, page) -> [Edge]` / `edges(ref, viewer) -> [Edge]`
//! (OWNED by Refs). Like 5.2 it is a per-viewer REQUEST/RESPONSE shape — so this CDC pair pins the
//! **leak-free contract** the provider (Refs) promises and the consumers (the impact view / "what
//! references this" panel / Notif) depend on:
//!
//! - **PROVIDER (Refs):** `backlinks` returns ONLY the inbound edges whose `source_root` the viewer
//!   may `view` — the frozen `list_objects` `SetExpr` (consumed 4.3) lowered over `edge.source_root`,
//!   in ONE query, no N+1, no post-filter, always paginated, tenant-scoped. The provider promises: a
//!   confidential referrer is ABSENT for an unauthorized viewer (0 leak); a cross-tenant edge is never
//!   readable; a just-revoked grant does not read stale (the carried zookie + watermark fall-back).
//! - **CONSUMER (a "what references this" panel / Notif humanisation):** a renderer that lists the
//!   returned [`Backlink`]s — it can render EVERY returned backlink WITHOUT a per-edge permission
//!   re-check, because the provider already excluded everything the viewer cannot see. This is the
//!   load-bearing 5.3 promise (the leak-free backlink read).
//!
//! - **CONSUMER CDC for 4.3:** Refs is one of the five named `SetExpr` consumers — it consumes the
//!   FROZEN `ListObjectsResult` (`Ids{ids, zookie}` | `Filter{set_expr, zookie}`) and lowers it over
//!   its OWN `source_root` id column. This file pins that Refs handles BOTH frozen shapes and lowers
//!   the SetExpr to the §4.4 SQL forms (the consumer contract — no Id signature change).

use myelin_identity::{
    AuthzIndexRef, Consistency, ConsistencyMode, ListObjectsResult, ObjectId, Principal,
    PrincipalId, PrincipalKind, RelName, SetExpr, Zookie,
};
use myelin_refs_service::{
    edge_builder::{EdgeProjection, EdgeRow, RelClass},
    ids_result, lower_over_source_root, source_root_colref, AuthzVisibleIndex, Backlink,
    BacklinkRead, FilterMode,
};
use myelin_tenancy::{Region, TenantId};

use myelin_events::ArtifactRef;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn aref(s: &str) -> ArtifactRef {
    ArtifactRef(s.into())
}
fn at(rev: &str) -> Consistency {
    Consistency {
        at_least: Zookie(rev.into()),
        mode: ConsistencyMode::Strong,
    }
}

fn target() -> ArtifactRef {
    aref("myelin://acme/issue/issue/PUBLIC-1")
}
fn secret() -> ArtifactRef {
    aref("myelin://acme/issue/issue/SECRET-9")
}
fn public() -> ArtifactRef {
    aref("myelin://acme/issue/issue/OPEN-2")
}

fn seeded() -> BacklinkRead {
    let edges = EdgeProjection::new();
    for (eid, src) in [("e-secret", secret()), ("e-public", public())] {
        edges.upsert(
            &tenant(),
            &region(),
            EdgeRow {
                edge_id: eid.into(),
                source: src.clone(),
                source_root: src.clone(),
                target: target(),
                target_root: target(),
                rel: "mentions".into(),
                rel_class: RelClass::Reference,
                origin_event: format!("evt-{eid}"),
                origin_actor: "principal-opaque-1".into(),
                zookie: Some("zk-1".into()),
                tombstoned: false,
            },
        );
    }
    BacklinkRead::new(edges, AuthzVisibleIndex::new())
}

/// **PROVIDER+CONSUMER (5.3): the provider returns ONLY admitted backlinks; a consumer renders them
/// all without a re-check.** The provider hides the confidential referrer; the consumer (a "what
/// references this" panel) renders the returned list directly — and it never holds a leaked title.
#[test]
fn cdc_5_3_provider_returns_only_admitted_backlinks_consumer_renders_without_recheck() {
    let read = seeded();
    // The viewer may view only the public source (the reverse index grant).
    read_grant(&read, "p:viewer", &public().0, "zk-00000000000000000003");

    // The provider lowers the Filter{InRelation} the consumer (Identity 4.3) returned.
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-00000000000000000003".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target(),
            &viewer("p:viewer"),
            &lo,
            &at("zk-00000000000000000003"),
            50,
        )
        .expect("provider serves the read");

    // CONSUMER: a "what references this" panel renders every returned backlink with NO re-check.
    let rendered: Vec<String> = page.edges.iter().map(render_backlink).collect();
    assert_eq!(
        rendered.len(),
        1,
        "the consumer renders exactly the admitted backlinks"
    );
    assert!(
        rendered[0].contains("OPEN-2"),
        "the public referrer is rendered"
    );
    // The leak invariant the consumer relies on: no confidential referrer is in the rendered list.
    assert!(
        !rendered.iter().any(|r| r.contains("SECRET")),
        "0 leak: the consumer never receives the confidential referrer"
    );
}

/// **CONSUMER CDC (4.3): Refs handles BOTH frozen `ListObjectsResult` shapes** — `Ids{}` (materialised)
/// and `Filter{set_expr}` (pushed down) — and reports the filter-mode split. The frozen shape is
/// consumed unchanged (no Id signature change); Refs lowers it over `source_root`.
#[test]
fn cdc_4_3_refs_consumes_both_frozen_list_objects_shapes() {
    let read = seeded();
    // Ids mode: the materialised allow-set.
    let ids = ids_result(&[&public().0], "zk-1");
    let p1 = read
        .backlinks(
            &tenant(),
            &region(),
            &target(),
            &viewer("p:a"),
            &ids,
            &at(""),
            50,
        )
        .expect("Ids-mode read");
    assert_eq!(
        p1.mode,
        FilterMode::Ids,
        "the Ids shape drives the materialised mode"
    );
    assert_eq!(p1.edges.len(), 1);

    // Filter mode: the pushed-down SetExpr.
    read_grant(&read, "p:a", &secret().0, "zk-1");
    let filter = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    };
    let p2 = read
        .backlinks(
            &tenant(),
            &region(),
            &target(),
            &viewer("p:a"),
            &filter,
            &at(""),
            50,
        )
        .expect("Filter-mode read");
    assert_eq!(
        p2.mode,
        FilterMode::PushedDown,
        "the Filter shape drives the pushed-down mode"
    );
}

/// **CONSUMER CDC (4.3): the SetExpr lowering SHAPE Refs composes is the §4.4 frozen form.** Every
/// SetExpr variant lowers to the exact SQL predicate/JOIN over `edge.source_root` — pinned so a
/// downstream change to the frozen encoding is caught at the consumer.
#[test]
fn cdc_4_3_setexpr_lowers_to_the_frozen_source_root_forms() {
    let v = viewer("p:alice");
    assert_eq!(
        lower_over_source_root(&SetExpr::All, &v).sql_predicate,
        "TRUE"
    );
    assert_eq!(
        lower_over_source_root(&SetExpr::None, &v).sql_predicate,
        "FALSE"
    );
    assert_eq!(
        lower_over_source_root(&SetExpr::Ids(vec![ObjectId("s:a".into())]), &v).sql_predicate,
        "edge.source_root IN (:id_0)"
    );
    assert_eq!(
        lower_over_source_root(&SetExpr::NotIds(vec![ObjectId("s:a".into())]), &v).sql_predicate,
        "edge.source_root NOT IN (:id_0)"
    );
    let join = lower_over_source_root(
        &SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        &v,
    );
    assert!(join.joins[0]
        .clause
        .contains("JOIN authz_visible av0 ON av0.object_id = edge.source_root"));
    let tuple = lower_over_source_root(
        &SetExpr::TupleSet {
            index: AuthzIndexRef("view".into()),
        },
        &v,
    );
    assert!(
        tuple.depends_on_reverse_index(),
        "TupleSet JOINs the reverse index"
    );
}

/// Grant `subject` view of `object_id` through the read's reverse index (the public
/// [`BacklinkRead::authz_index`] accessor — the production wiring is the bus consumer that keeps the
/// reverse index fresh; here the CDC projects the grant directly).
fn read_grant(read: &BacklinkRead, subject: &str, object_id: &str, rev: &str) {
    read.authz_index()
        .grant(&tenant(), &region(), subject, "view", object_id, rev);
}

/// Render a backlink the way a "what references this" panel would — an opaque, non-leaking line
/// (the source URN + the relation). The consumer holds only opaque refs (never the third-party name).
fn render_backlink(b: &Backlink) -> String {
    format!("{} {} {}", b.rel, "→", b.source.0)
}
