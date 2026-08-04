use super::*;
use myelin_identity::{
    AuthzIndexRef, ConsistencyMode, ObjectId, PrincipalId, PrincipalKind, Zookie,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn subject(root: &str) -> ArtifactRef {
    ArtifactRef(root.into())
}
fn origin(id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/bus/event/{id}"))
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

#[test]
fn record_stores_one_marker_per_subject_root_zero_write_amplification() {
    let store = AmbientMarkerStore::new();
    let subj = subject("myelin://acme/chat/thread/celebrity");
    for i in 0..1000 {
        store.record(
            &tenant(),
            &subj,
            crate::Reason::Watched,
            &origin(&format!("e{i}")),
        );
    }
    assert_eq!(
        store.marker_count(&tenant()),
        1,
        "NOTIF-P13: a 50k-watcher subject hit 1000 times → ONE coalesced marker (0 write amplification)"
    );
    let m = store
        .get(&tenant(), "myelin://acme/chat/thread/celebrity")
        .unwrap();
    assert_eq!(
        m.count, 1000,
        "the +N more counter accumulated the full activity (preserved, never lost)"
    );
    assert_eq!(m.reason, crate::Reason::Watched);
}

#[test]
fn record_coalesces_subthreads_into_the_root_marker() {
    let store = AmbientMarkerStore::new();
    store.record(
        &tenant(),
        &subject("myelin://acme/chat/thread/T1#c1"),
        crate::Reason::Watched,
        &origin("a"),
    );
    store.record(
        &tenant(),
        &subject("myelin://acme/chat/thread/T1#c2"),
        crate::Reason::Watched,
        &origin("b"),
    );
    assert_eq!(
        store.marker_count(&tenant()),
        1,
        "sub-thread activity coalesces into the ONE root marker"
    );
    let m = store
        .get(&tenant(), "myelin://acme/chat/thread/T1")
        .unwrap();
    assert_eq!(m.count, 2);
}

#[test]
fn markers_are_tenant_partitioned() {
    let store = AmbientMarkerStore::new();
    store.record(
        &tenant(),
        &subject("myelin://acme/git/pr/9"),
        crate::Reason::Watched,
        &origin("a"),
    );
    let other = TenantId("globex".into());
    assert_eq!(store.marker_count(&tenant()), 1);
    assert_eq!(
        store.marker_count(&other),
        0,
        "another tenant's read sees 0 markers (partition key)"
    );
    assert!(store.get(&other, "myelin://acme/git/pr/9").is_none());
}

fn seeded() -> (AmbientMarkerStore, SyntheticReverseIndex) {
    let store = AmbientMarkerStore::new();
    store.record(
        &tenant(),
        &subject("root/a"),
        crate::Reason::Watched,
        &origin("a"),
    );
    store.record(
        &tenant(),
        &subject("root/b"),
        crate::Reason::Watched,
        &origin("b"),
    );
    store.record(
        &tenant(),
        &subject("root/c"),
        crate::Reason::Watched,
        &origin("c"),
    );
    let idx = SyntheticReverseIndex::new();
    idx.grant_watch(&tenant(), "u1", "root/a");
    idx.grant_watch(&tenant(), "u1", "root/c");
    (store, idx)
}

#[test]
fn read_fanout_materialises_only_the_watched_slice_via_the_join() {
    let (store, idx) = seeded();
    let zk = idx.current_zookie();
    let slice = read_fanout(&viewer("u1"), &store, &idx, &strong(&zk.0)).unwrap();
    let roots: Vec<&str> = slice.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        roots,
        vec!["root/a", "root/c"],
        "the viewer sees exactly the roots they watch (a, c), stable order"
    );
    assert!(
        !roots.contains(&"root/b"),
        "a non-watched subject's marker is NOT in the viewer's slice"
    );
}

#[test]
fn read_fanout_for_a_non_watcher_is_empty() {
    let (store, idx) = seeded();
    let zk = idx.current_zookie();
    let slice = read_fanout(&viewer("nobody"), &store, &idx, &strong(&zk.0)).unwrap();
    assert!(
        slice.is_empty(),
        "a principal who watches nothing materialises 0 ambient markers"
    );
}

#[test]
fn setexpr_lowering_covers_every_algebra_arm() {
    let store = AmbientMarkerStore::new();
    for r in ["root/a", "root/b", "root/c", "root/d"] {
        store.record(&tenant(), &subject(r), crate::Reason::Watched, &origin(r));
    }
    struct ExprPort(SetExpr);
    impl WatcherResolvePort for ExprPort {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<myelin_identity::ListObjectsResult> {
            Ok(myelin_identity::ListObjectsResult::Filter {
                set_expr: self.0.clone(),
                zookie: Zookie("zk-0".into()),
            })
        }
    }
    let roots = |expr: SetExpr| -> Vec<String> {
        let port = ExprPort(expr);
        read_fanout(&viewer("u1"), &store, &port, &strong("zk-0"))
            .unwrap()
            .into_iter()
            .map(|m| m.subject_root)
            .collect()
    };
    let id = |s: &str| ObjectId(s.into());

    assert_eq!(
        roots(SetExpr::All).len(),
        4,
        "All → every marker (type+tenant scoped)"
    );
    assert!(roots(SetExpr::None).is_empty(), "None → ∅ (WHERE false)");
    assert_eq!(
        roots(SetExpr::Ids(vec![id("root/a"), id("root/c")])),
        vec!["root/a", "root/c"]
    );
    assert_eq!(
        roots(SetExpr::NotIds(vec![id("root/a")])),
        vec!["root/b", "root/c", "root/d"]
    );
    assert_eq!(
        roots(SetExpr::Union(vec![
            SetExpr::Ids(vec![id("root/a")]),
            SetExpr::Ids(vec![id("root/b")])
        ])),
        vec!["root/a", "root/b"]
    );
    assert_eq!(
        roots(SetExpr::Intersect(vec![
            SetExpr::NotIds(vec![id("root/a")]),
            SetExpr::Ids(vec![id("root/a"), id("root/b")]),
        ])),
        vec!["root/b"]
    );
    assert_eq!(
        roots(SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![id("root/a"), id("root/b"), id("root/c")])),
        )),
        vec!["root/d"]
    );
}

#[test]
fn tupleset_relational_form_resolves_via_the_join() {
    let (store, idx) = seeded();
    let zk = idx.current_zookie();
    struct TupleSetPort(SyntheticReverseIndex);
    impl WatcherResolvePort for TupleSetPort {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<myelin_identity::ListObjectsResult> {
            Ok(myelin_identity::ListObjectsResult::Filter {
                set_expr: SetExpr::TupleSet {
                    index: AuthzIndexRef("watcher-idx".into()),
                },
                zookie: self.0.current_zookie(),
            })
        }
        fn resolve_relation(
            &self,
            s: &Principal,
            leaf: &RelationalLeaf,
            req: RevisionWatermark,
        ) -> AuthzResult<ReverseIndexAnswer> {
            self.0.resolve_relation(s, leaf, req)
        }
    }
    let port = TupleSetPort(idx);
    let slice = read_fanout(&viewer("u1"), &store, &port, &strong(&zk.0)).unwrap();
    let roots: Vec<&str> = slice.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        roots,
        vec!["root/a", "root/c"],
        "the TupleSet big-result path resolves the same watched slice"
    );
}

#[test]
fn revoked_watch_is_reflected_held_not_leaked() {
    let (store, idx) = seeded();
    let before = read_fanout(
        &viewer("u1"),
        &store,
        &idx,
        &strong(&idx.current_zookie().0),
    )
    .unwrap();
    let before_roots: Vec<&str> = before.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        before_roots,
        vec!["root/a", "root/c"],
        "before revoke: u1 sees a + c"
    );

    let new_zk = idx.revoke_watch(&tenant(), "u1", "root/c");

    let after = read_fanout(&viewer("u1"), &store, &idx, &strong(&new_zk.0)).unwrap();
    let after_roots: Vec<&str> = after.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        after_roots,
        vec!["root/a"],
        "after revoke: the revoked subject c is ABSENT (held, not leaked)"
    );
    assert!(
        !after_roots.contains(&"root/c"),
        "0 leaked item on a revoked watch (contract 4.10)"
    );
}

#[test]
fn stale_reverse_index_revision_is_rejected_not_served() {
    let (store, idx) = seeded();
    let current = idx.grant_watch(&tenant(), "other", "root/x");
    idx.pin_served_revision(Some(1));
    let err = read_fanout(&viewer("u1"), &store, &idx, &strong(&current.0)).unwrap_err();
    match err {
        ReadFanoutError::StaleReverseIndex { required, served } => {
            assert!(
                served < required,
                "the served revision is below the required watermark (rejected, not served)"
            );
        }
        other => panic!("expected StaleReverseIndex, got {other:?}"),
    }
}

#[test]
fn unavailable_resolver_holds_not_leaks() {
    let (store, idx) = seeded();
    idx.set_unavailable(true);
    let err = read_fanout(
        &viewer("u1"),
        &store,
        &idx,
        &strong(&idx.current_zookie().0),
    )
    .unwrap_err();
    assert!(
        matches!(err, ReadFanoutError::Unavailable(_)),
        "an Id hiccup is held, not leaked (never fall open)"
    );
}

#[test]
fn bounded_stale_read_serves_from_a_lower_watermark() {
    let (store, idx) = seeded();
    let current = idx.grant_watch(&tenant(), "other", "root/x");
    idx.pin_served_revision(Some(1));
    let bounded = Consistency {
        at_least: current,
        mode: ConsistencyMode::BoundedStale,
    };
    let slice = read_fanout(&viewer("u1"), &store, &idx, &bounded).unwrap();
    let roots: Vec<&str> = slice.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        roots,
        vec!["root/a", "root/c"],
        "a BoundedStale read serves from the lower watermark (no reject)"
    );
}

#[test]
fn ids_materialised_path_needs_no_join() {
    let store = AmbientMarkerStore::new();
    store.record(
        &tenant(),
        &subject("root/a"),
        crate::Reason::Watched,
        &origin("a"),
    );
    store.record(
        &tenant(),
        &subject("root/b"),
        crate::Reason::Watched,
        &origin("b"),
    );
    struct IdsPort;
    impl WatcherResolvePort for IdsPort {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<myelin_identity::ListObjectsResult> {
            Ok(myelin_identity::ListObjectsResult::Ids {
                ids: vec![ObjectId("root/a".into())],
                zookie: Zookie("zk-0".into()),
            })
        }
        fn resolve_relation(
            &self,
            _s: &Principal,
            _l: &RelationalLeaf,
            _r: RevisionWatermark,
        ) -> AuthzResult<ReverseIndexAnswer> {
            panic!("the S4 Ids path must NOT call the reverse-index JOIN (no N+1)");
        }
    }
    let slice = read_fanout(&viewer("u1"), &store, &IdsPort, &strong("zk-0")).unwrap();
    assert_eq!(
        slice.len(),
        1,
        "the bounded Ids watch set materialised directly (S4, no JOIN)"
    );
    assert_eq!(slice[0].subject_root, "root/a");
}

#[test]
fn honours_is_revision_ge_watermark() {
    let answer = |rev: u64| ReverseIndexAnswer {
        subject_roots: BTreeSet::new(),
        revision: RevisionWatermark(rev),
    };
    assert!(
        answer(5).honours(RevisionWatermark(5)),
        "revision == watermark honours (the boundary)"
    );
    assert!(
        answer(6).honours(RevisionWatermark(5)),
        "a newer revision honours"
    );
    assert!(
        !answer(4).honours(RevisionWatermark(5)),
        "a revision below the watermark does NOT honour (rejected)"
    );
}

#[test]
fn default_resolve_relation_is_unavailable() {
    struct BoundedOnlyPort;
    impl WatcherResolvePort for BoundedOnlyPort {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<myelin_identity::ListObjectsResult> {
            Ok(myelin_identity::ListObjectsResult::Filter {
                set_expr: SetExpr::InRelation {
                    relation: RelName(WATCHER_RELATION.into()),
                    via_column: subject_root_col(),
                },
                zookie: Zookie("zk-0".into()),
            })
        }
    }
    let store = AmbientMarkerStore::new();
    store.record(
        &tenant(),
        &subject("root/a"),
        crate::Reason::Watched,
        &origin("a"),
    );
    let err = read_fanout(&viewer("u1"), &store, &BoundedOnlyPort, &strong("zk-0")).unwrap_err();
    assert!(
        matches!(err, ReadFanoutError::Unavailable(_)),
        "no reverse index → held, not leaked (default unavailable)"
    );
}

#[test]
fn synthetic_revoke_bumps_the_revision_strictly() {
    let idx = SyntheticReverseIndex::new();
    let after_grant = parse_zk(&idx.grant_watch(&tenant(), "u1", "root/a").0);
    let after_revoke = parse_zk(&idx.revoke_watch(&tenant(), "u1", "root/a").0);
    assert!(
        after_revoke > after_grant,
        "revoke advances the monotone revision strictly (a newer zookie)"
    );
    let after_second = parse_zk(&idx.revoke_watch(&tenant(), "u1", "root/a").0);
    assert!(
        after_second > after_revoke,
        "every revoke advances the revision (a mutant flattening it is caught)"
    );
}

#[test]
fn synthetic_serves_only_the_watcher_relation() {
    let idx = SyntheticReverseIndex::new();
    idx.grant_watch(&tenant(), "u1", "root/a");
    let req = RevisionWatermark(0);
    let watcher_leaf = RelationalLeaf::InRelation {
        relation: RelName(WATCHER_RELATION.into()),
        via_column: subject_root_col(),
    };
    let watched = idx
        .resolve_relation(&viewer("u1"), &watcher_leaf, req)
        .unwrap();
    assert_eq!(
        watched.subject_roots.len(),
        1,
        "the watcher relation resolves the watched set"
    );
    let other_leaf = RelationalLeaf::InRelation {
        relation: RelName("editor".into()),
        via_column: subject_root_col(),
    };
    let other = idx
        .resolve_relation(&viewer("u1"), &other_leaf, req)
        .unwrap();
    assert!(
        other.subject_roots.is_empty(),
        "a non-watcher relation resolves to ∅ (the relation guard bites)"
    );
}

fn parse_zk(zk: &str) -> u64 {
    zk.strip_prefix("zk-").unwrap().parse().unwrap()
}

#[test]
fn marker_carries_refs_not_payloads() {
    let store = AmbientMarkerStore::new();
    store.record(
        &tenant(),
        &subject("myelin://acme/git/pr/9"),
        crate::Reason::Watched,
        &origin("e1"),
    );
    let m = store.get(&tenant(), "myelin://acme/git/pr/9").unwrap();
    assert_eq!(
        m.subject,
        ArtifactRef("myelin://acme/git/pr/9".into()),
        "the subject is a ref, never a payload"
    );
    assert_eq!(
        m.latest_origin,
        origin("e1"),
        "the origin is a ref (the NOTIF-2 provenance)"
    );
}
