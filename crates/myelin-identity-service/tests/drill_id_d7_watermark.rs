use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta,
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

fn tuple(object: &str, relation: &str, subj: &str) -> RelationTuple {
    RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    }
}

fn now() -> Timestamp {
    Timestamp("2026-06-19T00:00:00Z".into())
}

fn feed_pending(outbox: &OutboxStore, consumer: &ReverseIndexConsumer) {
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
}

#[test]
fn id_d7_revoke_then_reread_no_stale_allow() {
    let mut signals = SignalSource::new();
    let s = scope("acme");

    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    let _ = namespace.admit(&FragmentDef {
        object_type: ObjectType("repo".into()),
        relations: vec![RelName("reader".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Relation(RelName("reader".into())),
        }],
    });

    let alice = subject("p:alice", "acme");
    let read = Permission("read".into());
    let repo = ObjectType("repo".into());

    let _z_grant = store
        .write_tuples(
            &s,
            &admin("acme"),
            &[TupleDelta::Add(tuple("repo:core", "reader", "p:alice"))],
            None,
            None,
            now(),
        )
        .expect("grant");
    feed_pending(&outbox, &consumer);
    assert_eq!(
        index.objects_for(&s, &repo, &alice.principal_id, &RelName("reader".into())),
        vec![ObjectId("repo:core".into())],
        "S8 projected the grant - alice is a reader of repo:core in the reverse index"
    );

    let z_revoke = store
        .write_tuples(
            &s,
            &admin("acme"),
            &[TupleDelta::Remove(tuple("repo:core", "reader", "p:alice"))],
            None,
            None,
            now(),
        )
        .expect("revoke");
    assert!(
        index.watermark(&s).0 < z_revoke.0,
        "S8 is BEHIND the revoke revision (the index lags the write - watermark={:?} < revoke={:?})",
        index.watermark(&s),
        z_revoke
    );

    let stale_join = index.objects_for(&s, &repo, &alice.principal_id, &RelName("reader".into()));
    assert_eq!(
        stale_join,
        vec![ObjectId("repo:core".into())],
        "the behind S8 still has the stale grant row - the watermark guard is what prevents serving it"
    );

    let lo = ListObjects::with_cap(store.clone(), namespace, index.clone(), 0);

    let mut stale_allows: i64 = 0;
    let mut guard_engaged = false;

    let post_revoke = Consistency {
        at_least: z_revoke.clone(),
        mode: ConsistencyMode::Strong,
    };

    let via = ColRef {
        table: "repo".into(),
        column: "id".into(),
    };
    let join_lowered = lower(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: via.clone(),
        },
        &alice,
        &via,
    );
    let verdict = watermark_verdict(&index, &s, &join_lowered, &post_revoke);
    match verdict {
        WatermarkVerdict::FallBackToCheck {
            ref required,
            ref watermark,
        } => {
            guard_engaged = true;
            assert_eq!(
                required, &z_revoke,
                "the scan required the post-revoke revision"
            );
            assert!(
                watermark.0 < required.0,
                "the S8 watermark is behind the required revision"
            );
        }
        WatermarkVerdict::JoinServes => {
            stale_allows += 1;
        }
    }

    let consistent = lo.list_objects_consistent(&s, &alice, &read, &repo, &post_revoke);
    match consistent {
        ListObjectsResult::Ids { ids, .. } => {
            if ids.iter().any(|o| o.0 == "repo:core") {
                stale_allows += 1;
            }
        }
        ListObjectsResult::Filter { .. } => {
            stale_allows += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, stale_allows);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        stale_allows, 0,
        "0 stale allows post-revoke (ID-D7 - the new-enemy guard holds)"
    );
    assert!(
        guard_engaged,
        "the S8 watermark fall-back guard engaged (it did not serve the behind JOIN)"
    );

    println!(
        "[P-070 DRILL GREEN 2026-06-19] ID-D7 revoke-then-reread watermark: \
         alice revoked from repo:core (post-revoke zookie={z_revoke:?}), S8 held BEHIND the revoke → \
         the watermark verdict is FallBackToCheck and list_objects_consistent falls back to per-row \
         check over the authoritative S3 → stale-allow=0 (the new-enemy guard, §7.4/§8.7)",
        z_revoke = z_revoke.0
    );
}
