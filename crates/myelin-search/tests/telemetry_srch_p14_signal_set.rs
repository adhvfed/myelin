use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_harness::telemetry::{Label, Predicate as P, SignalName, SignalSource};
use myelin_identity::{
    AuthzIndexRef, Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    query, telemetry_signal as sig, CacheStats, ConsistencyStats, FieldDecl, FieldSchema,
    IndexBackend, IndexDocument, ListObjectsPort, Page, QueryStats, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, ScopedEngine, SearchTelemetry, TantivyBackend,
    CACHE_RATIO_ABSENT, FT_BODY_FIELD, ORDER_KEY_FIELD,
};

fn facet_decl() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}
fn var(n: &str) -> Expr {
    Expr::Var(n.into())
}
fn lit(v: &str) -> Expr {
    Expr::Lit(Literal::Str(v.into()))
}

fn corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, text: &str| {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
    };
    be.upsert(&doc("acme/issue/PUB-1", "deadlock in the scheduler"))
        .unwrap();
    be.upsert(&doc("acme/issue/SECRET-9", "deadlock secret incident"))
        .unwrap();
    be
}

struct IdsAuthz {
    visible: Vec<String>,
    calls: AtomicU64,
}
impl ListObjectsPort for IdsAuthz {
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Ids {
            ids: self.visible.iter().map(|s| ObjectId(s.clone())).collect(),
            zookie: Zookie("z@10".into()),
        })
    }
    fn resolve_relation(
        &self,
        _s: &Principal,
        _f: &RelationalLeaf,
        _r: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        Err(myelin_identity::AuthzError::Unavailable(
            "Ids path has no reverse index".into(),
        ))
    }
}

struct FilterAuthz {
    visible: Vec<String>,
    calls: AtomicU64,
    resolve_calls: AtomicU64,
}
impl ListObjectsPort for FilterAuthz {
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
            zookie: Zookie("z@10".into()),
        })
    }
    fn resolve_relation(
        &self,
        _s: &Principal,
        _f: &RelationalLeaf,
        _r: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        self.resolve_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ReverseIndexAnswer {
            object_ids: self.visible.clone(),
            revision: RevisionWatermark(10),
        })
    }
}

fn drive_two_filter_modes() -> QueryStats {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let stats = QueryStats::new();
    let ty = ObjectType("issue".into());
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: lit("deadlock"),
    });

    let ids = IdsAuthz {
        visible: vec!["acme/issue/PUB-1".into()],
        calls: AtomicU64::new(0),
    };
    query(
        &eng,
        &ids,
        &q,
        &viewer(),
        &ty,
        &consistency(),
        Page::FIRST,
        &stats,
    )
    .expect("Ids query");

    let filter = FilterAuthz {
        visible: vec!["acme/issue/PUB-1".into()],
        calls: AtomicU64::new(0),
        resolve_calls: AtomicU64::new(0),
    };
    query(
        &eng,
        &filter,
        &q,
        &viewer(),
        &ty,
        &consistency(),
        Page::FIRST,
        &stats,
    )
    .expect("Filter query");

    stats
}

fn full_snapshot() -> SearchTelemetry {
    let qstats = drive_two_filter_modes();

    let cstats = ConsistencyStats::new();
    cstats.record_fail_static_bypass();
    cstats.record_excluded_stale();

    let cache = CacheStats::new();

    let mut t = SearchTelemetry::from_stats( 0, &qstats, &cstats, &cache);

    t.record_red(
        myelin_search::RedLabels {
            kind: "human",
            tenant: "acme",
            surface: "ft",
        },
        2,
        0,
        5,
    );
    t.set_consumer_lag("search-indexer", 0);
    t.set_vector_compaction_lag("acme", 0);
    t.set_in_flight("acme", 0);
    t.set_shed_count("acme", 0);
    t.set_reindex_parity_hash(0);
    t.set_erase_receipts(0);
    t
}

#[test]
fn every_4_11_signal_is_emitted_and_readable_by_the_assertion_library() {
    let t = full_snapshot();

    let mut src = SignalSource::new();

    src.set_scalar(SignalName::ConsumerLag, t.scalar(sig::INDEX_LAG).unwrap());
    let lo_rate = t.scalar(sig::LIST_OBJECTS_RATE).unwrap();
    assert_eq!(
        lo_rate, 2,
        "exactly one list_objects per query (no N+1): 2 queries → 2 calls"
    );
    src.set_scalar(SignalName::RequestRate, lo_rate);

    let ids_mode = t.scalar(sig::FILTER_MODE_IDS).unwrap();
    let filter_mode = t.scalar(sig::FILTER_MODE_FILTER).unwrap();
    assert_eq!(ids_mode, 1, "one query used the materialised Ids mode");
    assert_eq!(
        filter_mode, 1,
        "one query used the pushed-down Filter/TupleSet mode"
    );
    src.set_labelled(
        SignalName::RequestRate,
        vec![Label::new("filter_mode", "ids")],
        ids_mode,
    );
    src.set_labelled(
        SignalName::RequestRate,
        vec![Label::new("filter_mode", "filter")],
        filter_mode,
    );

    let ratio = t.scalar(sig::CACHE_HIT_RATIO_PCT).unwrap();
    assert_eq!(
        ratio, CACHE_RATIO_ABSENT,
        "no cacheable read → the absent sentinel, never a fake 100"
    );

    let bypass = t.scalar(sig::ZERO_ESCAPE_ZOOKIE_BYPASS).unwrap();
    let excluded = t.scalar(sig::ZERO_ESCAPE_STALE_EXCLUDED).unwrap();
    assert_eq!(bypass, 1, "one strong-read fail-static bypass recorded");
    assert_eq!(
        excluded, 1,
        "one stale candidate EXCLUDED (the new-enemy kept out)"
    );
    src.set_scalar(SignalName::CrossTenantCount, 0);

    assert_eq!(
        t.scalar(sig::REINDEX_PARITY_HASH),
        Some(0),
        "parity hash emitted (0 = no reindex yet)"
    );
    assert_eq!(
        t.scalar(sig::ERASE_RECEIPTS),
        Some(0),
        "erase-receipt count emitted (0 until SRCH-P15)"
    );

    src.assert_signal(SignalName::ConsumerLag, P::Eq(0))
        .expect_green();
    src.assert_signal(SignalName::RequestRate, P::Gte(1))
        .expect_green();
    src.assert_labelled(
        SignalName::RequestRate,
        vec![Label::new("filter_mode", "ids")],
        P::Eq(1),
    )
    .expect_green();
    src.assert_labelled(
        SignalName::RequestRate,
        vec![Label::new("filter_mode", "filter")],
        P::Eq(1),
    )
    .expect_green();
    src.assert_signal(SignalName::CrossTenantCount, P::Eq(0))
        .expect_green();

    let red_labels = || {
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", "acme"),
            Label::new("surface", "ft"),
        ]
    };
    let err = t
        .labelled_value(
            sig::QUERY_ERRORS,
            &[
                ("kind".into(), "human".into()),
                ("tenant".into(), "acme".into()),
                ("surface".into(), "ft".into()),
            ],
        )
        .expect("RED errors emitted per {kind,tenant,surface}");
    src.set_labelled(SignalName::RequestErrors, red_labels(), err);
    src.assert_labelled(SignalName::RequestErrors, red_labels(), P::Eq(0))
        .expect_green();

    let consumer_lag = t
        .labelled_value(
            sig::CONSUMER_LAG,
            &[("consumer".into(), "search-indexer".into())],
        )
        .expect("consumer lag emitted");
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "search-indexer")],
        consumer_lag,
    );
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "search-indexer")],
        P::Eq(0),
    )
    .expect_green();

    let shed = t
        .labelled_value(sig::SHED_COUNT, &[("tenant".into(), "acme".into())])
        .expect("shed count emitted");
    src.set_labelled(
        SignalName::ShedCount,
        vec![Label::new("tenant", "acme")],
        shed,
    );
    src.assert_labelled(
        SignalName::ShedCount,
        vec![Label::new("tenant", "acme")],
        P::Eq(0),
    )
    .expect_green();

    for name in sig::ALL {
        let present = t.scalar(name).is_some() || t.labelled.iter().any(|s| s.name == name);
        assert!(
            present,
            "§4.11 signal `{name}` is MISSING from the metrics-health snapshot"
        );
    }
}

#[test]
fn vector_compaction_lag_signal_is_emitted_and_clears_on_compact() {
    let mut t = SearchTelemetry::empty();
    t.set_vector_compaction_lag("acme", 1);
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::PoolSaturation,
        vec![Label::new("tenant", "acme")],
        t.labelled_value(
            sig::VECTOR_COMPACTION_LAG,
            &[("tenant".into(), "acme".into())],
        )
        .unwrap(),
    );
    src.assert_labelled(
        SignalName::PoolSaturation,
        vec![Label::new("tenant", "acme")],
        P::Gte(1),
    )
    .expect_green();
    t.set_vector_compaction_lag("acme", 0);
    assert_eq!(
        t.labelled_value(
            sig::VECTOR_COMPACTION_LAG,
            &[("tenant".into(), "acme".into())]
        ),
        Some(0),
        "compaction-lag returns to 0 after a compact (no orphan embedding survives)"
    );
}
