use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    lower,
    namespace::{FragmentDef, PermissionRule, Userset},
    watermark_verdict, ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer,
    TupleStore, WatermarkVerdict,
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

fn subject(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    })
}

fn now() -> Timestamp {
    Timestamp("2026-06-19T00:00:00Z".into())
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> (ListObjects, ReverseIndex) {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    let _ = namespace.admit(&FragmentDef {
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

    store
        .write_tuples(scope, &admin(&scope.tenant().0), grants, None, None, now())
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    (
        ListObjects::with_cap(store.clone(), namespace, index.clone(), cap),
        index,
    )
}

fn run_lowered_join(
    index: &ReverseIndex,
    scope: &TenantScope,
    subject: &Principal,
    relation: &str,
) -> Vec<ObjectId> {
    index.objects_for(
        scope,
        &ObjectType("repo".into()),
        &subject.principal_id,
        &RelName(relation.into()),
    )
}

#[test]
fn id_d4_confidential_object_absent_from_every_list_path() {
    let mut signals = SignalSource::new();
    let s = scope("acme");

    let (lo, index) = wired(
        1000,
        &s,
        &[
            add("repo:secret", "reader", "p:owner"),
            add("repo:public", "reader", "p:intruder"),
        ],
    );

    let intruder = subject("p:intruder", "acme");
    let read = Permission("read".into());
    let repo = ObjectType("repo".into());

    let mut escapes: i64 = 0;

    let ids_result = lo.list_objects(&s, &intruder, &read, &repo, &at_latest());
    if let ListObjectsResult::Ids { ids, .. } = &ids_result {
        if ids.iter().any(|o| o.0 == "repo:secret") {
            escapes += 1;
        }
    } else {
        panic!("under a high cap the small set materialises as Ids");
    }

    let (lo_filter, index_filter) = wired(
        0,
        &s,
        &[
            add("repo:secret", "reader", "p:owner"),
            add("repo:public", "reader", "p:intruder"),
        ],
    );
    let filter_result = lo_filter.list_objects(&s, &intruder, &read, &repo, &at_latest());
    match filter_result {
        ListObjectsResult::Filter { set_expr, .. } => {
            let via = myelin_identity::ColRef {
                table: "repo".into(),
                column: "id".into(),
            };
            let lowered = lower(&set_expr, &intruder, &via);
            assert!(
                lowered.depends_on_reverse_index(),
                "the Filter lowers to an S8 JOIN"
            );
            for rel in ["read", "reader", "writer"] {
                let joined = run_lowered_join(&index_filter, &s, &intruder, rel);
                if joined.iter().any(|o| o.0 == "repo:secret") {
                    escapes += 1;
                }
            }
        }
        ListObjectsResult::Ids { .. } => panic!("cap 0 must dispatch every list to Filter"),
    }
    let _ = index_filter;

    let stale_pin = Consistency {
        at_least: Zookie("zk-00000000000000999999".into()),
        mode: ConsistencyMode::Strong,
    };
    let consistent = lo.list_objects_consistent(&s, &intruder, &read, &repo, &stale_pin);
    if let ListObjectsResult::Ids { ids, .. } = &consistent {
        if ids.iter().any(|o| o.0 == "repo:secret") {
            escapes += 1;
        }
    }
    {
        let via = myelin_identity::ColRef {
            table: "repo".into(),
            column: "id".into(),
        };
        let join_lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via.clone(),
            },
            &intruder,
            &via,
        );
        let verdict = watermark_verdict(&index, &s, &join_lowered, &stale_pin);
        assert!(
            matches!(verdict, WatermarkVerdict::FallBackToCheck { .. }),
            "a scan pinned ahead of the S8 watermark engages the fall-back guard, never serves stale: {verdict:?}"
        );
    }

    signals.set_scalar(SignalName::CrossTenantCount, escapes);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        escapes, 0,
        "0 confidential-object escapes across Ids + Filter + staleness (ID-D4)"
    );

    println!(
        "[P-070 DRILL GREEN 2026-06-19] ID-D4 zero-escape list_objects leak: \
         viewer=p:intruder confidential=repo:secret (owner=p:owner) → zero-escape=0 across the Ids \
         materialise path, the Filter-lowered S8 JOIN (cap=0), and under zookie staleness \
         (fall-back-to-check engaged) - the leak-free pre-filter holds (§7.2; EI-01 §2)"
    );
}
