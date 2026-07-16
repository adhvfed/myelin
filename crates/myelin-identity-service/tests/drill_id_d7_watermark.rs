//! # P-ID-12 (global P-070) GATE / DRILL — ID-D7, the revoke-then-reread watermark drill (dated
//! green artifact)
//!
//! **Drill catalogue row ID-D7 (§4.2, F8):** *Revoke, then re-read with the post-revoke zookie → no
//! stale allow ("new enemy").* Survival signal: **zookie-watermark honoured** — the S8 JOIN WAITS or
//! FALLS BACK to per-row `check` rather than serving the stale grant (§7.4/§8.7). Run against the
//! failure-injection harness's telemetry-assertion library; `myelin-harness` is a DEV-dependency
//! only.
//!
//! **The scenario.** alice is a reader of `repo:core`. The grant is then REVOKED (`write_tuples` with
//! a remove delta → a NEWER zookie, stamped on the object). The S8 reverse index is held BEHIND (we
//! do NOT feed the revoke event to the S8 consumer — simulating the index lagging the write). A
//! security-sensitive scan re-reads `list_objects(alice, read, repo)` pinned at the POST-REVOKE
//! zookie. The new-enemy guard must hold:
//! - the lowered `Filter` JOIN's [`watermark_verdict`] is `FallBackToCheck` (the S8 watermark is
//!   BEHIND the required revision — never serve the stale grant);
//! - `list_objects_consistent` therefore falls back to per-row `check` over the authoritative S3
//!   store (which reflects the revoke) → alice does NOT see `repo:core` (no stale allow).
//!
//! A stale allow (alice still seeing `repo:core` after the revoke, served from the behind index)
//! increments the stale-allow counter and the drill aborts LOUDLY (the threshold is NEVER weakened).

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

/// Drain the relay and feed every published envelope to the S8 consumer (the live feed). Used to
/// project the GRANT but deliberately NOT the revoke (so S8 lags the write).
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

    // (1) GRANT: alice reads repo:core. Project it into S8 (the index is up to date for the grant).
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
        "S8 projected the grant — alice is a reader of repo:core in the reverse index"
    );

    // (2) REVOKE: remove the grant (a NEWER zookie, the post-revoke revision). The S3 store now
    //     reflects the revoke; we DO NOT feed the revoke to S8 (the index lags — the new-enemy
    //     window the watermark guard must close).
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
    // (Intentionally NOT calling feed_pending — S8's watermark stays behind z_revoke.)
    assert!(
        index.watermark(&s).0 < z_revoke.0,
        "S8 is BEHIND the revoke revision (the index lags the write — watermark={:?} < revoke={:?})",
        index.watermark(&s),
        z_revoke
    );

    // The lowered Filter JOIN over the STALE S8 would still return repo:core for alice (the index
    // has not seen the revoke). The leak is exactly this — the guard must not let it serve.
    let stale_join = index.objects_for(&s, &repo, &alice.principal_id, &RelName("reader".into()));
    assert_eq!(
        stale_join,
        vec![ObjectId("repo:core".into())],
        "the behind S8 still has the stale grant row — the watermark guard is what prevents serving it"
    );

    let lo = ListObjects::with_cap(store.clone(), namespace, index.clone(), 0); // cap 0 → Filter path

    let mut stale_allows: i64 = 0;
    let mut guard_engaged = false;

    // (3) The security-sensitive re-read PINNED at the post-revoke zookie.
    let post_revoke = Consistency {
        at_least: z_revoke.clone(),
        mode: ConsistencyMode::Strong,
    };

    // (3a) The watermark verdict on the lowered Filter must be FallBackToCheck (the guard engaged).
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
            // The behind index would serve the stale grant — the new-enemy leak.
            stale_allows += 1;
        }
    }

    // (3b) The end-to-end consistent list MUST fall back to check and return NO repo:core (the
    //      authoritative S3 reflects the revoke).
    let consistent = lo.list_objects_consistent(&s, &alice, &read, &repo, &post_revoke);
    match consistent {
        ListObjectsResult::Ids { ids, .. } => {
            if ids.iter().any(|o| o.0 == "repo:core") {
                stale_allows += 1; // a stale allow survived the fall-back — the guard failed
            }
        }
        ListObjectsResult::Filter { .. } => {
            // A Filter returned at the post-revoke pin means the JOIN would serve the stale grant —
            // the guard did NOT fall back (the new-enemy leak).
            stale_allows += 1;
        }
    }

    // The green artifact: 0 stale allows + the watermark guard engaged.
    signals.set_scalar(SignalName::CrossTenantCount, stale_allows);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        stale_allows, 0,
        "0 stale allows post-revoke (ID-D7 — the new-enemy guard holds)"
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
