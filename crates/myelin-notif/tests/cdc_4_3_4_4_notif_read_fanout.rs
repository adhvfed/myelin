use myelin_events::ArtifactRef;
use myelin_identity::{
    AuthzError, ColRef, Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, RelName, Result as AuthzResult, SetExpr,
    Zookie,
};
use myelin_notif::read_fanout::{
    read_fanout, subject_root_col, AmbientMarkerStore, ReadFanoutError, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, WatcherResolvePort, SUBJECT_ROOT_TYPE, WATCHER_RELATION,
    WATCH_PERMISSION,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}
fn subj(root: &str) -> ArtifactRef {
    ArtifactRef(root.into())
}

fn seeded_markers() -> AmbientMarkerStore {
    let store = AmbientMarkerStore::new();
    for r in ["root/a", "root/b", "root/c"] {
        store.record(
            &tenant(),
            &subj(r),
            myelin_notif::Reason::Watched,
            &ArtifactRef(format!("myelin://acme/bus/event/{r}")),
        );
    }
    store
}

struct IdentityProvider {
    watched: Vec<String>,
    revision: u64,
}

impl WatcherResolvePort for IdentityProvider {
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        assert_eq!(
            permission.0, WATCH_PERMISSION,
            "Notif lists for the frozen `watch` permission (4.3)"
        );
        assert_eq!(
            ty.0, SUBJECT_ROOT_TYPE,
            "Notif lists over its own subject_root id space"
        );
        assert_eq!(
            at.mode,
            ConsistencyMode::Strong,
            "a security-sensitive read is Strong (4.10)"
        );
        assert_eq!(subject.tenant, tenant());
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(WATCHER_RELATION.into()),
                via_column: subject_root_col(),
            },
            zookie: Zookie(format!("zk-{}", self.revision)),
        })
    }

    fn resolve_relation(
        &self,
        _subject: &Principal,
        leaf: &RelationalLeaf,
        required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        match leaf {
            RelationalLeaf::InRelation {
                relation,
                via_column,
            } => {
                assert_eq!(
                    relation.0, WATCHER_RELATION,
                    "the leaf is the frozen `watcher` relation (4.9)"
                );
                assert_eq!(
                    *via_column,
                    ColRef {
                        table: "notif_inbox_item".into(),
                        column: "subject_root".into()
                    },
                    "the JOIN is keyed by Notif's OWN subject_root column (§3.5, no N+1)"
                );
            }
            RelationalLeaf::TupleSet { .. } => {}
        }
        assert!(
            required.0 <= self.revision || required.0 == 0,
            "the CONSUMER passes the watermark it requires"
        );
        Ok(ReverseIndexAnswer {
            subject_roots: self.watched.iter().cloned().collect(),
            revision: RevisionWatermark(self.revision),
        })
    }
}

#[test]
fn cdc_4_3_setexpr_pushdown_lowers_to_the_join_over_notifs_own_column() {
    let store = seeded_markers();
    let provider = IdentityProvider {
        watched: vec!["root/a".into(), "root/c".into()],
        revision: 7,
    };
    let slice = read_fanout(&viewer("u1"), &store, &provider, &strong("zk-7")).unwrap();
    let roots: Vec<&str> = slice.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        roots,
        vec!["root/a", "root/c"],
        "the SetExpr JOIN materialised exactly the watched slice (no post-filter over all 3)"
    );
}

#[test]
fn cdc_4_10_consumer_rejects_a_stale_reverse_index_revision() {
    struct LaggingProvider;
    impl WatcherResolvePort for LaggingProvider {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            Ok(ListObjectsResult::Filter {
                set_expr: SetExpr::InRelation {
                    relation: RelName(WATCHER_RELATION.into()),
                    via_column: subject_root_col(),
                },
                zookie: Zookie("zk-9".into()),
            })
        }
        fn resolve_relation(
            &self,
            _s: &Principal,
            _l: &RelationalLeaf,
            _required: RevisionWatermark,
        ) -> AuthzResult<ReverseIndexAnswer> {
            Ok(ReverseIndexAnswer {
                subject_roots: ["root/a".to_string()].into_iter().collect(),
                revision: RevisionWatermark(3),
            })
        }
    }
    let store = seeded_markers();
    let err = read_fanout(&viewer("u1"), &store, &LaggingProvider, &strong("zk-9")).unwrap_err();
    match err {
        ReadFanoutError::StaleReverseIndex { required, served } => {
            assert_eq!(required, RevisionWatermark(9));
            assert_eq!(served, RevisionWatermark(3));
        }
        other => panic!("expected StaleReverseIndex (held, not leaked), got {other:?}"),
    }
}

#[test]
fn cdc_4_3_consumer_holds_not_leaks_on_unavailable_provider() {
    struct DownProvider;
    impl WatcherResolvePort for DownProvider {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            Err(AuthzError::Unavailable("identity hiccup".into()))
        }
    }
    let store = seeded_markers();
    let err = read_fanout(&viewer("u1"), &store, &DownProvider, &strong("zk-1")).unwrap_err();
    assert!(
        matches!(err, ReadFanoutError::Unavailable(_)),
        "an unavailable provider holds, never falls open"
    );
}

#[test]
fn cdc_4_3_s4_ids_path_makes_zero_join_calls() {
    struct S4Provider;
    impl WatcherResolvePort for S4Provider {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            Ok(ListObjectsResult::Ids {
                ids: vec![ObjectId("root/b".into())],
                zookie: Zookie("zk-1".into()),
            })
        }
        fn resolve_relation(
            &self,
            _s: &Principal,
            _l: &RelationalLeaf,
            _r: RevisionWatermark,
        ) -> AuthzResult<ReverseIndexAnswer> {
            panic!("the S4 Ids path must make ZERO reverse-index JOIN calls (no N+1)");
        }
    }
    let store = seeded_markers();
    let slice = read_fanout(&viewer("u1"), &store, &S4Provider, &strong("zk-1")).unwrap();
    let roots: Vec<&str> = slice.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        roots,
        vec!["root/b"],
        "the bounded Ids watch set materialised directly (S4, no JOIN)"
    );
}
