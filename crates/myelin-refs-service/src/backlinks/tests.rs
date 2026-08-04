use super::*;
use myelin_identity::{
    AuthzIndexRef, ConsistencyMode, ObjectId, PrincipalId, PrincipalKind, RelName, Zookie,
};
use myelin_refs::strip_sub;

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
fn pinned(rev: &str) -> Consistency {
    Consistency {
        at_least: Zookie(rev.into()),
        mode: ConsistencyMode::Strong,
    }
}
fn latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn target_root() -> ArtifactRef {
    aref("myelin://acme/issue/issue/PUBLIC-1")
}

fn secret_source() -> ArtifactRef {
    aref("myelin://acme/issue/issue/SECRET-9")
}
fn public_source() -> ArtifactRef {
    aref("myelin://acme/issue/issue/OPEN-2")
}

fn seeded_read() -> BacklinkRead {
    let edges = EdgeProjection::new();
    for (eid, src) in [("e-secret", secret_source()), ("e-public", public_source())] {
        edges.upsert(
            &tenant(),
            &region(),
            EdgeRow {
                edge_id: eid.into(),
                source: src.clone(),
                source_root: src.clone(),
                target: target_root(),
                target_root: target_root(),
                rel: "mentions".into(),
                rel_class: crate::edge_builder::RelClass::Reference,
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
fn all_lowers_to_true_no_predicate() {
    let l = lower_over_source_root(&SetExpr::All, &viewer("p:a"));
    assert_eq!(l.sql_predicate, "TRUE");
    assert!(l.joins.is_empty() && l.params.is_empty());
    assert!(!l.depends_on_reverse_index());
}

#[test]
fn none_lowers_to_false_deny() {
    let l = lower_over_source_root(&SetExpr::None, &viewer("p:a"));
    assert_eq!(l.sql_predicate, "FALSE");
}

#[test]
fn ids_lowers_to_in_over_source_root_with_bound_params() {
    let l = lower_over_source_root(
        &SetExpr::Ids(vec![ObjectId("s:a".into()), ObjectId("s:b".into())]),
        &viewer("p:a"),
    );
    assert_eq!(l.sql_predicate, "edge.source_root IN (:id_0, :id_1)");
    assert_eq!(
        l.params,
        vec![
            BoundParam {
                placeholder: ":id_0".into(),
                value: "s:a".into()
            },
            BoundParam {
                placeholder: ":id_1".into(),
                value: "s:b".into()
            },
        ],
        "the ids are BOUND params over source_root, never interpolated"
    );
    assert!(l.joins.is_empty());
}

#[test]
fn empty_ids_lowers_to_false() {
    let l = lower_over_source_root(&SetExpr::Ids(vec![]), &viewer("p:a"));
    assert_eq!(l.sql_predicate, "FALSE", "an empty allow-set sees nothing");
}

#[test]
fn not_ids_lowers_to_not_in_over_source_root() {
    let l = lower_over_source_root(
        &SetExpr::NotIds(vec![ObjectId("s:secret".into())]),
        &viewer("p:a"),
    );
    assert_eq!(l.sql_predicate, "edge.source_root NOT IN (:id_0)");
    let empty = lower_over_source_root(&SetExpr::NotIds(vec![]), &viewer("p:a"));
    assert_eq!(
        empty.sql_predicate, "TRUE",
        "an empty deny-set excludes nothing"
    );
}

#[test]
fn in_relation_lowers_to_authz_visible_join_keyed_on_source_root() {
    let l = lower_over_source_root(
        &SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        &viewer("p:alice"),
    );
    assert_eq!(l.joins.len(), 1, "exactly one reverse-index JOIN (no N+1)");
    let j = &l.joins[0];
    assert!(
        j.clause
            .contains("JOIN authz_visible av0 ON av0.object_id = edge.source_root"),
        "the JOIN keys on edge.source_root (the C-4 filter column): {}",
        j.clause
    );
    assert!(
        j.clause.contains("av0.subject = :subject_0"),
        "binds the viewer: {}",
        j.clause
    );
    assert!(
        j.clause.contains("av0.relation = :rel_for_view"),
        "binds the relation: {}",
        j.clause
    );
    assert_eq!(l.sql_predicate, "av0.object_id IS NOT NULL");
    assert_eq!(j.relation, "view");
    assert!(l
        .params
        .iter()
        .any(|p| p.placeholder == ":subject_0" && p.value == "p:alice"));
    assert!(l.depends_on_reverse_index());
}

#[test]
fn tuple_set_lowers_to_authz_visible_join() {
    let l = lower_over_source_root(
        &SetExpr::TupleSet {
            index: AuthzIndexRef("view".into()),
        },
        &viewer("p:alice"),
    );
    assert_eq!(l.joins.len(), 1);
    assert!(l.joins[0].clause.contains("av0.relation = :rel_for_view"));
    assert!(l.depends_on_reverse_index());
}

#[test]
fn boolean_composition_lowers_to_or_and_and_not() {
    let u = lower_over_source_root(
        &SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("s:a".into())]),
            SetExpr::Ids(vec![ObjectId("s:b".into())]),
        ]),
        &viewer("p:a"),
    );
    assert_eq!(
        u.sql_predicate,
        "(edge.source_root IN (:id_0) OR edge.source_root IN (:id_1))"
    );

    let i = lower_over_source_root(
        &SetExpr::Intersect(vec![
            SetExpr::All,
            SetExpr::NotIds(vec![ObjectId("s:x".into())]),
        ]),
        &viewer("p:a"),
    );
    assert_eq!(
        i.sql_predicate,
        "(TRUE AND edge.source_root NOT IN (:id_0))"
    );

    let d = lower_over_source_root(
        &SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("s:secret".into())])),
        ),
        &viewer("p:a"),
    );
    assert_eq!(
        d.sql_predicate,
        "(TRUE AND NOT edge.source_root IN (:id_0))"
    );
}

#[test]
fn repeated_relation_emits_one_join_no_n_plus_1() {
    let l = lower_over_source_root(
        &SetExpr::Union(vec![
            SetExpr::InRelation {
                relation: RelName("view".into()),
                via_column: source_root_colref(),
            },
            SetExpr::InRelation {
                relation: RelName("view".into()),
                via_column: source_root_colref(),
            },
        ]),
        &viewer("p:alice"),
    );
    assert_eq!(
        l.joins.len(),
        1,
        "the same (viewer, relation) JOIN is emitted once, however nested"
    );
    assert_eq!(
        l.sql_predicate,
        "(av0.object_id IS NOT NULL OR av0.object_id IS NOT NULL)"
    );
}

#[test]
fn admit_all_and_none() {
    let authz = AuthzVisibleIndex::new();
    let v = viewer("p:a");
    assert!(set_expr_admits(
        &SetExpr::All,
        &authz,
        &v,
        &tenant(),
        &region(),
        &public_source()
    ));
    assert!(!set_expr_admits(
        &SetExpr::None,
        &authz,
        &v,
        &tenant(),
        &region(),
        &public_source()
    ));
}

#[test]
fn admit_ids_and_not_ids() {
    let authz = AuthzVisibleIndex::new();
    let v = viewer("p:a");
    let allow = SetExpr::Ids(vec![ObjectId(public_source().0)]);
    assert!(set_expr_admits(
        &allow,
        &authz,
        &v,
        &tenant(),
        &region(),
        &public_source()
    ));
    assert!(!set_expr_admits(
        &allow,
        &authz,
        &v,
        &tenant(),
        &region(),
        &secret_source()
    ));
    let deny = SetExpr::NotIds(vec![ObjectId(secret_source().0)]);
    assert!(set_expr_admits(
        &deny,
        &authz,
        &v,
        &tenant(),
        &region(),
        &public_source()
    ));
    assert!(!set_expr_admits(
        &deny,
        &authz,
        &v,
        &tenant(),
        &region(),
        &secret_source()
    ));
}

#[test]
fn admit_in_relation_reads_the_reverse_index() {
    let authz = AuthzVisibleIndex::new();
    authz.grant(
        &tenant(),
        &region(),
        "p:a",
        "view",
        &public_source().0,
        "zk-00000000000000000005",
    );
    let expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: source_root_colref(),
    };
    let v = viewer("p:a");
    assert!(
        set_expr_admits(&expr, &authz, &v, &tenant(), &region(), &public_source()),
        "the granted source is visible via the reverse index"
    );
    assert!(
        !set_expr_admits(&expr, &authz, &v, &tenant(), &region(), &secret_source()),
        "the ungranted source is NOT visible (0 leak)"
    );
}

#[test]
fn admit_difference_a_except_b() {
    let authz = AuthzVisibleIndex::new();
    let v = viewer("p:a");
    let expr = SetExpr::Difference(
        Box::new(SetExpr::All),
        Box::new(SetExpr::Ids(vec![ObjectId(secret_source().0)])),
    );
    assert!(set_expr_admits(
        &expr,
        &authz,
        &v,
        &tenant(),
        &region(),
        &public_source()
    ));
    assert!(!set_expr_admits(
        &expr,
        &authz,
        &v,
        &tenant(),
        &region(),
        &secret_source()
    ));
}

#[test]
fn ref_d1_confidential_referrer_absent_for_unauthorized_viewer() {
    let read = seeded_read();
    read.authz.grant(
        &tenant(),
        &region(),
        "p:viewer",
        "view",
        &public_source().0,
        "zk-00000000000000000003",
    );

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
            &target_root(),
            &viewer("p:viewer"),
            &lo,
            &pinned("zk-00000000000000000003"),
            50,
        )
        .expect("read succeeds");

    assert_eq!(
        page.edges.len(),
        1,
        "exactly the ONE authorized (public) backlink is returned"
    );
    assert_eq!(
        page.edges[0].source,
        public_source(),
        "the public backlink is present"
    );
    assert!(
        !page.edges.iter().any(|b| b.source == secret_source()),
        "0 leak: the confidential referrer must be ABSENT from backlinks"
    );
    assert!(
        !format!("{:?}", page.edges).contains("SECRET"),
        "0 leak: the secret source URN must not appear anywhere in the result"
    );
    assert_eq!(
        page.mode,
        FilterMode::PushedDown,
        "the InRelation drove the pushed-down filter mode"
    );
}

#[test]
fn ref_d1_holds_in_ids_filter_mode() {
    let read = seeded_read();
    let lo = ids_result(&[&public_source().0], "zk-1");
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:viewer"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(page.edges.len(), 1);
    assert_eq!(page.edges[0].source, public_source());
    assert!(
        !page.edges.iter().any(|b| b.source == secret_source()),
        "0 leak in Ids mode"
    );
    assert_eq!(
        page.mode,
        FilterMode::Ids,
        "the Ids result drove the materialised filter mode"
    );
}

#[test]
fn none_denies_all_backlinks() {
    let read = seeded_read();
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::None,
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:nobody"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(page.edges.len(), 0, "None → 0 backlinks (WHERE false)");
}

#[test]
fn all_admits_every_backlink() {
    let read = seeded_read();
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::All,
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:admin"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(page.edges.len(), 2, "All → every backlink (admin)");
}

#[test]
fn ref_d2_cross_tenant_edge_is_not_readable() {
    let edges = EdgeProjection::new();
    let tenant_b = TenantId("evilcorp".into());
    edges.upsert(
        &tenant_b,
        &region(),
        EdgeRow {
            edge_id: "e-bbb".into(),
            source: aref("myelin://evilcorp/issue/issue/X-1"),
            source_root: aref("myelin://evilcorp/issue/issue/X-1"),
            target: target_root(),
            target_root: target_root(),
            rel: "mentions".into(),
            rel_class: crate::edge_builder::RelClass::Reference,
            origin_event: "evt-b".into(),
            origin_actor: "principal-opaque-b".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );
    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::All,
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(
        page.edges.len(),
        0,
        "0 cross-tenant edge readable (no cross-tenant query path, ID-3)"
    );
}

#[test]
fn ref_d6_new_enemy_grant_read_revoke_reread_absent() {
    let edges = EdgeProjection::new();
    edges.upsert(
        &tenant(),
        &region(),
        EdgeRow {
            edge_id: "e-secret".into(),
            source: secret_source(),
            source_root: secret_source(),
            target: target_root(),
            target_root: target_root(),
            rel: "mentions".into(),
            rel_class: crate::edge_builder::RelClass::Reference,
            origin_event: "evt-1".into(),
            origin_actor: "principal-opaque-1".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );
    let authz = AuthzVisibleIndex::new();
    let read = BacklinkRead::new(edges, authz.clone());
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-00000000000000000005".into()),
    };

    authz.grant(
        &tenant(),
        &region(),
        "p:enemy",
        "view",
        &secret_source().0,
        "zk-00000000000000000005",
    );
    let visible = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:enemy"),
            &lo,
            &pinned("zk-00000000000000000005"),
            50,
        )
        .expect("read succeeds");
    assert_eq!(visible.edges.len(), 1, "post-grant the backlink is visible");
    assert!(
        !visible.fell_back_to_check,
        "the index is at-or-after the read revision → JOIN serves"
    );

    authz.revoke(
        &tenant(),
        &region(),
        "p:enemy",
        "view",
        &secret_source().0,
        "zk-00000000000000000009",
    );

    let absent = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:enemy"),
            &lo,
            &pinned("zk-00000000000000000009"),
            50,
        )
        .expect("read succeeds");
    assert_eq!(
        absent.edges.len(),
        0,
        "post-revoke the just-revoked grant does NOT read stale (no stale allow)"
    );
}

#[test]
fn watermark_behind_falls_back_to_check_branch_observable() {
    let read = seeded_read();
    read.authz.grant(
        &tenant(),
        &region(),
        "p:viewer",
        "view",
        &public_source().0,
        "zk-00000000000000000003",
    );
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-00000000000000000007".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:viewer"),
            &lo,
            &pinned("zk-00000000000000000007"),
            50,
        )
        .expect("read succeeds");
    assert!(
        page.fell_back_to_check,
        "a behind index falls back to per-source check, never serves stale"
    );
    assert_eq!(page.edges.len(), 1);
    assert_eq!(page.edges[0].source, public_source());

    let ids = ids_result(&[&public_source().0], "zk-00000000000000000007");
    let ids_page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:viewer"),
            &ids,
            &pinned("zk-00000000000000000099"),
            50,
        )
        .expect("read succeeds");
    assert!(
        !ids_page.fell_back_to_check,
        "a materialised Ids set is watermark-independent - JOIN-serves"
    );
}

#[test]
fn no_n_plus_1_one_query_and_filter_mode_split_fires() {
    let read = seeded_read();
    read.authz.grant(
        &tenant(),
        &region(),
        "p:a",
        "view",
        &public_source().0,
        "zk-1",
    );
    read.authz.grant(
        &tenant(),
        &region(),
        "p:a",
        "view",
        &secret_source().0,
        "zk-1",
    );

    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(
        page.edges.len(),
        2,
        "both authorized backlinks (2 inbound edges)"
    );
    assert_eq!(read.query_count(), 1, "the read issues ONE query (no N+1)");
    assert_eq!(read.filter_mode_split(), (0, 1));

    let ids = ids_result(&[&public_source().0], "zk-1");
    let _ = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &ids,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(read.query_count(), 2, "the second read is also ONE query");
    assert_eq!(
        read.filter_mode_split(),
        (1, 1),
        "the Ids vs pushed-down split is observable"
    );
}

#[test]
fn pagination_zero_page_rejected_and_limit_applied() {
    let read = seeded_read();
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::All,
        zookie: Zookie("zk-1".into()),
    };
    let err = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            0,
        )
        .unwrap_err();
    assert_eq!(err, BacklinkError::InvalidPage);
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            1,
        )
        .expect("read succeeds");
    assert_eq!(
        page.edges.len(),
        1,
        "LIMIT :page bounds the result (hot-artifact safety)"
    );
}

#[test]
fn edges_is_the_same_permission_filtered_read() {
    let read = seeded_read();
    let lo = ids_result(&[&public_source().0], "zk-1");
    let page = read
        .edges(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(page.edges.len(), 1);
    assert_eq!(page.edges[0].source, public_source());
}

#[test]
fn watermark_advances_monotonically_stale_never_regresses() {
    let authz = AuthzVisibleIndex::new();
    authz.advance_watermark(&tenant(), &region(), "zk-00000000000000000005");
    assert_eq!(
        authz.watermark(&tenant(), &region()),
        "zk-00000000000000000005"
    );
    authz.advance_watermark(&tenant(), &region(), "zk-00000000000000000003");
    assert_eq!(
        authz.watermark(&tenant(), &region()),
        "zk-00000000000000000005",
        "a stale advance never regresses the watermark"
    );
    authz.advance_watermark(&tenant(), &region(), "zk-00000000000000000005");
    assert_eq!(
        authz.watermark(&tenant(), &region()),
        "zk-00000000000000000005"
    );
    authz.advance_watermark(&tenant(), &region(), "zk-00000000000000000009");
    assert_eq!(
        authz.watermark(&tenant(), &region()),
        "zk-00000000000000000009"
    );
}

#[test]
fn backlink_error_display_is_descriptive() {
    let msg = format!("{}", BacklinkError::InvalidPage);
    assert!(
        msg.contains("paginated"),
        "the error explains the pagination requirement: {msg}"
    );
    assert!(!msg.is_empty());
}

#[test]
fn accessors_return_the_live_stores_the_read_scans() {
    let read = BacklinkRead::new(EdgeProjection::new(), AuthzVisibleIndex::new());
    read.edge_projection().upsert(
        &tenant(),
        &region(),
        EdgeRow {
            edge_id: "e-acc".into(),
            source: public_source(),
            source_root: public_source(),
            target: target_root(),
            target_root: target_root(),
            rel: "links".into(),
            rel_class: crate::edge_builder::RelClass::Reference,
            origin_event: "evt-acc".into(),
            origin_actor: "principal-opaque-1".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );
    read.authz_index().grant(
        &tenant(),
        &region(),
        "p:a",
        "view",
        &public_source().0,
        "zk-1",
    );
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(
        page.edges.len(),
        1,
        "the accessor-seeded edge + grant are observed by the read"
    );
}

#[test]
fn sub_artifact_backlinks_roll_up_to_the_root() {
    let edges = EdgeProjection::new();
    let target_sub = aref("myelin://acme/issue/issue/PUBLIC-1#L10-20");
    let root = strip_sub(&target_sub);
    assert_eq!(root, target_root(), "strip_sub gives the parent");
    edges.upsert(
        &tenant(),
        &region(),
        EdgeRow {
            edge_id: "e-sub".into(),
            source: public_source(),
            source_root: public_source(),
            target: target_sub.clone(),
            target_root: root.clone(),
            rel: "embeds".into(),
            rel_class: crate::edge_builder::RelClass::Reference,
            origin_event: "evt-sub".into(),
            origin_actor: "principal-opaque-1".into(),
            zookie: Some("zk-1".into()),
            tombstoned: false,
        },
    );
    let read = BacklinkRead::new(edges, AuthzVisibleIndex::new());
    let lo = ListObjectsResult::Filter {
        set_expr: SetExpr::All,
        zookie: Zookie("zk-1".into()),
    };
    let page = read
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &viewer("p:a"),
            &lo,
            &latest(),
            50,
        )
        .expect("read succeeds");
    assert_eq!(
        page.edges.len(),
        1,
        "a backlink to target#sub is found by the parent root"
    );
}
