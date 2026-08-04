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

#[test]
fn cdc_5_3_provider_returns_only_admitted_backlinks_consumer_renders_without_recheck() {
    let read = seeded();
    read_grant(&read, "p:viewer", &public().0, "zk-00000000000000000003");

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
    assert!(
        !rendered.iter().any(|r| r.contains("SECRET")),
        "0 leak: the consumer never receives the confidential referrer"
    );
}

#[test]
fn cdc_4_3_refs_consumes_both_frozen_list_objects_shapes() {
    let read = seeded();
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

fn read_grant(read: &BacklinkRead, subject: &str, object_id: &str, rev: &str) {
    read.authz_index()
        .grant(&tenant(), &region(), subject, "view", object_id, rev);
}

fn render_backlink(b: &Backlink) -> String {
    format!("{} {} {}", b.rel, "→", b.source.0)
}
