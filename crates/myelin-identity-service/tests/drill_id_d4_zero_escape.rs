//! # P-ID-12 (global P-070) GATE / DRILL — ID-D4, the zero-escape list_objects leak drill (dated
//! green artifact)
//!
//! **Drill catalogue row ID-D4 (§4.2, F1):** *Confidential issue / overridden page / private channel
//! ABSENT from any `list_objects` for an unauthorized viewer — INCLUDING the `Filter`-lowered S8 JOIN
//! result and under zookie staleness.* Survival signal: **zero-escape counter == 0** (the leak-free
//! pre-filter is the stop-the-bleeding floor, EI-01 §2). Run against the failure-injection harness's
//! telemetry-assertion library exactly as the P-068 ID-D3 cross-tenant drill does;
//! `myelin-harness` is a DEV-dependency only.
//!
//! **The scenario.** A confidential object (`repo:secret`) is readable ONLY by its owner (`p:owner`),
//! never by an unauthorized viewer (`p:intruder`). We assert the intruder sees `repo:secret` in NO
//! `list_objects` result across BOTH return shapes:
//! - the **`Ids` materialise path** (cap above the set) — the intruder's reachable set is empty /
//!   excludes `repo:secret`;
//! - the **`Filter` push-down path** (cap forced to 0 so every list lowers to a `Filter`) — the
//!   lowered S8 JOIN, when run against the reverse index, returns NONE of `repo:secret` for the
//!   intruder (the JOIN keys on `av.subject = :intruder AND av.relation = :read` — and there is no
//!   such reverse-index row);
//! - **under zookie staleness** — a scan pinned at a fresher revision than the S8 watermark falls
//!   back to per-row `check` over the authoritative S3 store, which also denies the intruder.
//!
//! A single leaked object (the intruder seeing `repo:secret` in any path) increments the zero-escape
//! counter and the drill aborts LOUDLY (EI-01 §3 — the threshold is NEVER weakened to pass).

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

/// Wire S3 + a live-fed S8 + a `repo` fragment (`read = reader ∪ writer`) at a chosen cardinality
/// cap, returning the `list_objects` evaluator AND the shared S8 index (so the drill can run the
/// lowered JOIN against the same projection the dispatch reads).
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
        consumer.handle(&env);
    }
    (
        ListObjects::with_cap(store.clone(), namespace, index.clone(), cap),
        index,
    )
}

/// Run the lowered `Filter` JOIN against the live S8 projection for `subject` + `relation`, returning
/// the object ids the JOIN would yield. This is the consumer's `... JOIN authz_visible av ON
/// av.object_id = repo.id AND av.subject = :subject AND av.relation = :read` evaluated over the
/// in-memory reverse index (the same `objects_for` the JOIN compiles to). A leak would be
/// `repo:secret` appearing here for the intruder.
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

/// **ID-D4 — a confidential object is ABSENT from every `list_objects` result for an unauthorized
/// viewer (the GATE; zero-escape == 0).** Across the `Ids` path, the `Filter`-lowered S8 JOIN, and
/// under zookie staleness.
#[test]
fn id_d4_confidential_object_absent_from_every_list_path() {
    let mut signals = SignalSource::new();
    let s = scope("acme");

    // The confidential object: `repo:secret` is readable ONLY by p:owner. p:intruder has NO grant on
    // it (intruder has a grant on an unrelated `repo:public`, so they are a real principal with a
    // non-empty reachable set — the leak would be the confidential one bleeding in).
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

    // --- (A) The Ids materialise path: the intruder's reachable set must EXCLUDE repo:secret. ---
    let ids_result = lo.list_objects(&s, &intruder, &read, &repo, &at_latest());
    if let ListObjectsResult::Ids { ids, .. } = &ids_result {
        if ids.iter().any(|o| o.0 == "repo:secret") {
            escapes += 1; // the confidential object leaked into the Ids materialise
        }
    } else {
        panic!("under a high cap the small set materialises as Ids");
    }

    // --- (B) The Filter-lowered S8 JOIN path: force every list to a Filter (cap 0) and run the
    //         lowered JOIN against the live S8 projection — repo:secret must NOT appear. ---
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
            // Lower the Filter to the SQL JOIN (the no-N+1 §7.2 lowering) and evaluate it against the
            // live reverse index for the intruder + the `read` relation.
            let via = myelin_identity::ColRef {
                table: "repo".into(),
                column: "id".into(),
            };
            let lowered = lower(&set_expr, &intruder, &via);
            assert!(
                lowered.depends_on_reverse_index(),
                "the Filter lowers to an S8 JOIN"
            );
            // The JOIN keys on the PERMISSION relation; the `read` permission is `reader ∪ writer`,
            // so the leak-free JOIN must return none of repo:secret for the intruder under EITHER
            // underlying relation.
            for rel in ["read", "reader", "writer"] {
                let joined = run_lowered_join(&index_filter, &s, &intruder, rel);
                if joined.iter().any(|o| o.0 == "repo:secret") {
                    escapes += 1; // the confidential object leaked through the lowered JOIN
                }
            }
        }
        ListObjectsResult::Ids { .. } => panic!("cap 0 must dispatch every list to Filter"),
    }
    let _ = index_filter;

    // --- (C) Under zookie staleness: a scan pinned at a fresher revision than the S8 watermark falls
    //         back to per-row check (which also denies the intruder repo:secret). ---
    let stale_pin = Consistency {
        // A revision far ahead of any watermark the index has reached → forces the fall-back path.
        at_least: Zookie("zk-00000000000000999999".into()),
        mode: ConsistencyMode::Strong,
    };
    let consistent = lo.list_objects_consistent(&s, &intruder, &read, &repo, &stale_pin);
    if let ListObjectsResult::Ids { ids, .. } = &consistent {
        if ids.iter().any(|o| o.0 == "repo:secret") {
            escapes += 1; // the confidential object leaked under staleness (the fall-back failed)
        }
    }
    // Also confirm the lowered Filter under the stale pin yields the FallBackToCheck verdict (the
    // guard engaged — it did not silently serve the stale JOIN).
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

    // Record + assert the zero-escape survival signal (the leak-free pre-filter floor). We reuse the
    // CrossTenantCount scalar as the platform's load-bearing zero-leak counter (the same telemetry
    // library the P-068 ID-D3 drill asserts against); the green artifact is the dated line below.
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
         (fall-back-to-check engaged) — the leak-free pre-filter holds (§7.2; EI-01 §2)"
    );
}
