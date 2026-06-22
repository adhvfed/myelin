//! # CDC — contracts 4.3 `list_objects` (SetExpr push-down) + 4.4 `list_subjects` + 4.10 zookie:
//! Notif's read-fanout consumption (NOTIF-P13 / P-191)
//!
//! **Architecture:** `notifications.md` §3.5 (the read-fanout for the unbounded ambient set: store
//! ONE coalesced marker, materialise per-watcher LAZILY on inbox open; the watcher resolution is the
//! frozen `list_objects(recipient, watch, type) → Filter{set_expr, zookie}` push-down lowered into a
//! SQL JOIN against the `authz_visible` reverse index over Notif's own `subject_root` column — one
//! query, no N+1, no post-filter; the zookie watermark; held, not leaked). **Contracts:** **4.3**
//! `list_objects` (the SetExpr push-down — the highest-fan-in dependency), **4.4** `list_subjects`
//! (50k-member density), **4.10** zookie (a just-revoked watch reflected at-or-after the watermark).
//!
//! Notif is a CONSUMER of 4.3/4.4 (NO Id signature change — it implements to the frozen `SetExpr`
//! lowering, no local re-invention of a watcher resolution path). This CDC pins the seam from both
//! sides:
//!
//! - **PROVIDER (Identity owns 4.3/4.4):** `list_objects(viewer, watch, subject_root, at)` returns
//!   the leak-free pre-filter `Ids | Filter{set_expr, zookie}`; the relational `SetExpr`
//!   (`InRelation{watcher}` / `TupleSet`) resolves against the per-tenant `authz_visible` reverse
//!   index at-or-after the zookie's revision watermark (4.10).
//! - **CONSUMER (Notif read-fanout):** lowers that EXACT `SetExpr` into the JOIN over Notif's OWN
//!   `subject_root` column (one query, no N+1, no post-filter), projects the ONE coalesced marker per
//!   subject_root down to the viewer's reachable set, and HOLDS (does not leak) on a stale revision /
//!   an unavailable resolver.
//!
//! The two halves agree on the WIRE: the frozen [`myelin_identity::SetExpr`] algebra + the
//! [`myelin_identity::ListObjectsResult`] return shape + the [`myelin_identity::Zookie`] watermark. A
//! drift on either side (a new SetExpr variant Notif cannot lower, a zookie that stops gating)
//! breaks THIS build.

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

/// **The PROVIDER (Identity 4.3/4.4) — a fake honouring the frozen return shapes.** Returns the
/// pushed-down `Filter{InRelation{watcher}, zookie}` (the S8 50k-density path) and resolves the
/// relational leaf against a watched set at the current revision (the `authz_visible` reverse index).
/// The zookie's revision is the watermark (4.10). This is the EXACT shape the production Identity
/// `list_objects` serves; the CDC asserts Notif consumes it without re-inventing a resolution path.
struct IdentityProvider {
    /// The subject_roots the viewer watches (the reverse-index answer).
    watched: Vec<String>,
    /// The current revision the index serves at (the zookie watermark).
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
        // The CONSUMER calls with the frozen (watch, subject_root) shape — assert the wire.
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
        // The S8 PUSHED-DOWN path: the relational watcher Filter (the 50k-density answer) + the zookie.
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
        // The CONSUMER resolves ONE relational leaf — the watcher relation, keyed by Notif's own col.
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
        // The index serves at its current revision; the watermark gate (4.10) is the CONSUMER's.
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

/// **PROVIDER + CONSUMER agree on the 4.3 SetExpr push-down: Notif lowers `Filter{InRelation{watcher}}`
/// into the JOIN over its own subject_root column and materialises ONLY the watched slice (one query,
/// no N+1, no post-filter).** The viewer watches a + c; the read-fanout returns exactly those markers.
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

/// **CONSUMER honours the 4.10 zookie watermark: a stale reverse-index revision is REJECTED (held,
/// not leaked).** The provider's `list_objects` zookie is `zk-9` (the watermark), but `resolve` serves
/// revision 3 (a lagging index) — the consumer rejects it as `StaleReverseIndex`, never serving stale.
#[test]
fn cdc_4_10_consumer_rejects_a_stale_reverse_index_revision() {
    // The provider's list_objects pins watermark 9, but the resolver serves an old revision (3).
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
            // Serve a STALE revision (3) below the required watermark (9).
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

/// **CONSUMER holds, not leaks, when the PROVIDER is unavailable (4.3 deny-when-unsure, §5.3).** An Id
/// hiccup makes `list_objects` unavailable; Notif returns a loud Unavailable, never the whole store.
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

/// **The bounded (4.3 S4) `Ids` materialised path: a viewer with a small watched set is returned
/// directly, with NO relational JOIN (the no-N+1 invariant — 0 resolve_relation calls).** The
/// provider answers with `Ids{root/a}`; the consumer materialises it without a reverse-index probe.
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
