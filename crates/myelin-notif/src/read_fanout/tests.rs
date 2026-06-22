//! Unit tests for the read-fanout (NOTIF-P13 / P-191): the §3.5 read-fanout — ONE coalesced marker
//! per subject_root (zero write amplification), the `SetExpr` watcher push-down JOIN (one query, no
//! N+1, no post-filter), and the zookie watermark (a just-revoked watch reflected; held, not leaked).
//!
//! The mutation floor's load-bearing surfaces — [`AmbientMarkerStore::record`] (ONE marker, never N),
//! the [`SetExpr`] lowering (`InRelation`/`TupleSet`/boolean/`Ids`/`NotIds`/`All`/`None`), and the
//! watermark gate ([`ReverseIndexAnswer::honours`]) — are each pinned by an assertion below.

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

// --- (1) ONE coalesced marker per subject_root — ZERO write amplification ---

/// **`AmbientMarkerStore::record`: N ambient events on a subject coalesce into ONE marker (count
/// accumulates), NEVER N markers and NEVER one row per watcher (§3.5, zero write amplification).** A
/// 50k-watcher celebrity subject hit 1000 times produces exactly ONE marker with `count = 1000`. A
/// mutant that opens a new marker per event (or per watcher) is caught.
#[test]
fn record_stores_one_marker_per_subject_root_zero_write_amplification() {
    let store = AmbientMarkerStore::new();
    let subj = subject("myelin://acme/chat/thread/celebrity");
    // 1000 ambient events on the ONE hot subject (a 50k-watcher celebrity channel).
    for i in 0..1000 {
        store.record(
            &tenant(),
            &subj,
            crate::Reason::Watched,
            &origin(&format!("e{i}")),
        );
    }
    // ZERO write amplification: exactly ONE marker (not 1000, not 50k).
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

/// **The `#sub` fragment is stripped to the root, so sub-thread activity coalesces into the ONE
/// root marker (§3.2.3).** Two events on `…/T1#c1` and `…/T1#c2` share the ONE `…/T1` marker.
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

/// **Markers are tenant-partitioned: another tenant's marker is never counted/read** (the partition
/// key, §3.4). A marker under `acme` is invisible to a `globex` read.
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

// --- (2) the SetExpr watcher push-down JOIN: one query, no N+1, no post-filter ---

/// Seed three ambient markers + an index where the viewer watches exactly two of the three roots.
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

/// **The read-fanout resolves the viewer's slice via the `SetExpr` JOIN — only the roots the viewer
/// WATCHES are materialised (the markers ⋈ reachable_roots JOIN, no post-filter).** u1 watches a + c
/// (not b); the read-fanout returns exactly the a + c markers from the THREE-marker store. A mutant
/// that returns all markers (skips the JOIN) or the wrong slice is caught.
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
    // the un-watched root b is NOT in the slice (the JOIN excluded it — not a post-filter over all).
    assert!(
        !roots.contains(&"root/b"),
        "a non-watched subject's marker is NOT in the viewer's slice"
    );
}

/// **A viewer who watches NOTHING gets an empty slice (the JOIN yields ∅, not the whole store).**
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

/// **The `SetExpr` boolean + bounded forms lower correctly over Notif's own subject_root column.**
/// Exercises every algebra arm against the marker set directly (the leak-critical lowering surface):
/// `All`, `None`, `Ids`, `NotIds`, `Union`, `Intersect`, `Difference`. A mutant that widens `None`/
/// `NotIds`/`Difference` or narrows `All`/`Union` is caught.
#[test]
fn setexpr_lowering_covers_every_algebra_arm() {
    let store = AmbientMarkerStore::new();
    for r in ["root/a", "root/b", "root/c", "root/d"] {
        store.record(&tenant(), &subject(r), crate::Reason::Watched, &origin(r));
    }
    // A resolver that returns the given SetExpr as the pushed-down filter, watermark always honoured.
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
        // No relational leaf in these exprs → resolve_relation is never called; the default would error.
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

    // All → every marker of the tenant.
    assert_eq!(
        roots(SetExpr::All).len(),
        4,
        "All → every marker (type+tenant scoped)"
    );
    // None → ∅ (WHERE false) — the deny short-circuit.
    assert!(roots(SetExpr::None).is_empty(), "None → ∅ (WHERE false)");
    // Ids → exactly the listed roots.
    assert_eq!(
        roots(SetExpr::Ids(vec![id("root/a"), id("root/c")])),
        vec!["root/a", "root/c"]
    );
    // NotIds → every root EXCEPT the deny-set.
    assert_eq!(
        roots(SetExpr::NotIds(vec![id("root/a")])),
        vec!["root/b", "root/c", "root/d"]
    );
    // Union(Ids{a}, Ids{b}) → a ∪ b.
    assert_eq!(
        roots(SetExpr::Union(vec![
            SetExpr::Ids(vec![id("root/a")]),
            SetExpr::Ids(vec![id("root/b")])
        ])),
        vec!["root/a", "root/b"]
    );
    // Intersect(NotIds{a}, Ids{a,b}) → b (the intersection bites).
    assert_eq!(
        roots(SetExpr::Intersect(vec![
            SetExpr::NotIds(vec![id("root/a")]),
            SetExpr::Ids(vec![id("root/a"), id("root/b")]),
        ])),
        vec!["root/b"]
    );
    // Difference(All, Ids{a,b,c}) → d (everything except a,b,c).
    assert_eq!(
        roots(SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![id("root/a"), id("root/b"), id("root/c")])),
        )),
        vec!["root/d"]
    );
}

/// **The `TupleSet` relational form also resolves via the reverse-index JOIN (the big-result path).**
/// The synthetic index serves the same watched set for a `TupleSet` leaf as for `InRelation{watcher}`.
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

// --- (3) the zookie watermark: a just-revoked watch reflected; held, not leaked ---

/// **THE ZOOKIE WATERMARK (contract 4.10): a just-revoked watch is reflected at-or-after the new
/// zookie — the revoked subject's ambient marker is ABSENT from the viewer's slice (held, not
/// leaked).** u1 watches a + c; the watch on c is REVOKED (a newer zookie); a read at the new
/// watermark returns ONLY a (c is held, not leaked). The chained drill the prompt names.
#[test]
fn revoked_watch_is_reflected_held_not_leaked() {
    let (store, idx) = seeded();
    // Before: u1 watches a + c → both materialise.
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

    // Revoke u1's watch on c → a NEWER zookie (the watermark a strong read must honour).
    let new_zk = idx.revoke_watch(&tenant(), "u1", "root/c");

    // After: a read at the NEW watermark reflects the revocation — c is HELD, not leaked.
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

/// **The watermark GATE rejects a STALE reverse-index revision (never read stale → held, not
/// leaked).** A resolver pinned to serve an OLD revision below the required watermark is rejected as
/// [`ReadFanoutError::StaleReverseIndex`] — a stale revision could re-admit a just-revoked watch (the
/// new-enemy problem). A mutant that skips `honours()` (serves the stale revision) is caught.
#[test]
fn stale_reverse_index_revision_is_rejected_not_served() {
    let (store, idx) = seeded();
    // Advance the revision (more writes) so the current watermark is high…
    let current = idx.grant_watch(&tenant(), "other", "root/x"); // bumps revision
                                                                 // …but pin the index to SERVE an old revision (1) below the required watermark.
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

/// **An UNAVAILABLE resolver → held, not leaked (ADR-03 deny-when-unsure, §5.3).** An Id hiccup makes
/// `list_objects` unavailable; the read-fanout returns a loud [`ReadFanoutError::Unavailable`] — it
/// NEVER falls open and serves the whole store. A mutant that swallows the error and widens is caught.
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

/// **A `BoundedStale` read may serve from a lower watermark (the fail-static path, §10) — it does NOT
/// reject a lagging revision.** The mode discriminates: Strong pins the watermark, BoundedStale sets
/// it to 0 (any non-stale revision satisfies). A mutant that ignores the mode is caught.
#[test]
fn bounded_stale_read_serves_from_a_lower_watermark() {
    let (store, idx) = seeded();
    let current = idx.grant_watch(&tenant(), "other", "root/x"); // a high current revision
    idx.pin_served_revision(Some(1)); // serve an old revision
    let bounded = Consistency {
        at_least: current,
        mode: ConsistencyMode::BoundedStale,
    };
    // BoundedStale → watermark 0 → the old revision is acceptable (the fail-static path serves it).
    let slice = read_fanout(&viewer("u1"), &store, &idx, &bounded).unwrap();
    let roots: Vec<&str> = slice.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        roots,
        vec!["root/a", "root/c"],
        "a BoundedStale read serves from the lower watermark (no reject)"
    );
}

/// **The `Ids` (S4) materialised path needs NO JOIN — a bounded watched set is returned directly.** A
/// resolver answering with `Ids{watched}` (a viewer with a small bounded watch set) materialises that
/// slice without a `resolve_relation` call (the no-N+1 invariant: 0 relational resolves on the S4 path).
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
    // A resolver that returns the S4 materialised Ids path — and PANICS if resolve_relation is called
    // (proving the S4 path makes ZERO reverse-index JOIN calls — the no-N+1 invariant).
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

// --- the watermark gate predicate (the load-bearing comparison the mutation floor pins) ---

/// **`ReverseIndexAnswer::honours`: a revision ≥ the watermark honours; below is rejected.** The exact
/// boundary the watermark gate turns on. A mutant that flips `>=` to `>` or `<` is caught.
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

/// **The default `resolve_relation` is UNAVAILABLE (deny-when-unsure).** A port wired only for the
/// bounded path has no reverse index — a relational leaf is a loud error, never a silent widen.
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
    // The list_objects returns a relational Filter, but the port has no resolver → Unavailable (held).
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

/// **The synthetic fixture's `revoke_watch` BUMPS the monotone revision (a newer zookie).** The
/// watermark drill depends on a revoke advancing the revision strictly past the grant's — assert it
/// (a mutant that fails to bump the revision on revoke would let a stale read mask the revocation).
#[test]
fn synthetic_revoke_bumps_the_revision_strictly() {
    let idx = SyntheticReverseIndex::new();
    let after_grant = parse_zk(&idx.grant_watch(&tenant(), "u1", "root/a").0);
    let after_revoke = parse_zk(&idx.revoke_watch(&tenant(), "u1", "root/a").0);
    assert!(
        after_revoke > after_grant,
        "revoke advances the monotone revision strictly (a newer zookie)"
    );
    // and a second revoke advances it again (strictly monotone, never flat/decreasing).
    let after_second = parse_zk(&idx.revoke_watch(&tenant(), "u1", "root/a").0);
    assert!(
        after_second > after_revoke,
        "every revoke advances the revision (a mutant flattening it is caught)"
    );
}

/// **The synthetic fixture serves ONLY the `watcher` relation — a different relation resolves to the
/// EMPTY set (never the watched set).** A mutant that drops the relation guard (serving the watched
/// set for ANY relation) is caught.
#[test]
fn synthetic_serves_only_the_watcher_relation() {
    let idx = SyntheticReverseIndex::new();
    idx.grant_watch(&tenant(), "u1", "root/a");
    let req = RevisionWatermark(0);
    // the watcher relation → the watched set.
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
    // a DIFFERENT relation (e.g. `editor`) → the EMPTY set (the guard bites; never the watched set).
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

/// **The marker carries REFS, never payloads (NOTIF-1, references-not-payloads).** The subject +
/// latest-origin are `ArtifactRef`s, resolved per-viewer at humanise time — so erasing a person
/// tombstones their appearance for free (the X-7 erasure posture applies to the read-fanout too).
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
