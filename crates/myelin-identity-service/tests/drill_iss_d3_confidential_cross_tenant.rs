use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ListObjectsResult,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer, StoreBackedCheck, TupleStore,
    CONFIDENTIAL, CONFIDENTIAL_GRANT, ISSUE_VIEW,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn allows(svc: &StoreBackedCheck, actor: &Principal, perm: &str, object: &str) -> bool {
    matches!(
        svc.check(
            actor,
            &Permission(perm.into()),
            &ArtifactRef(object.into()),
            &at_latest(),
            None
        ),
        Ok(Decision::Allow)
    )
}

#[test]
fn iss_d3_confidential_exclusion_zero_leak() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    const FLEET: usize = 64;
    let mut tuples: Vec<TupleDelta> = vec![
        add("issue:secret", "parent_project", "project:proj#view"),
        add("issue:secret", CONFIDENTIAL_GRANT, "p:owner"),
        add("issue:normal", "parent_project", "project:proj#view"),
    ];
    for i in 0..FLEET {
        let r = format!("p:reader-{i}");
        tuples.push(add("project:proj", "reader", &r));
        tuples.push(add("issue:secret", CONFIDENTIAL, &r));
    }
    tuples.push(add("project:proj", "reader", "p:owner"));

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme issue grants");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_issue_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }

    assert!(
        allows(
            &svc,
            &principal("acme", "p:reader-0"),
            ISSUE_VIEW,
            "issue:normal"
        ),
        "a project reader views a normal issue (parent_project->view resolves)"
    );
    assert!(
        allows(&svc, &principal("acme", "p:owner"), ISSUE_VIEW, "issue:secret"),
        "the direct confidential_grant owner views the confidential issue (the ∪ confidential_grant arm)"
    );

    let mut confidential_leaks: i64 = 0;
    for i in 0..FLEET {
        if allows(
            &svc,
            &principal("acme", &format!("p:reader-{i}")),
            ISSUE_VIEW,
            "issue:secret",
        ) {
            confidential_leaks += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, confidential_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        confidential_leaks, 0,
        "0 confidential-issue leaks to a normal project reader (the − confidential Exclusion, ISS-D3)"
    );

    println!(
        "[P-322 DRILL GREEN 2026-06-22] ISS-D3 (F1) confidential-exclusion: \
         fleet={FLEET} project readers attempted view on issue:secret via parent_project->view \
         inheritance (view = (parent_project->view − confidential) ∪ confidential_grant) \
         → confidential-leak count=0; only the direct confidential_grant owner views it (§5, the \
         exclusion removes them BY CONSTRUCTION, never a post-filter)"
    );
}

#[test]
fn iss_d3_cross_tenant_zero_leak() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let globex = scope_of(&principal("globex", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &[
                add("project:acme-proj", "reader", "p:viewer"),
                add(
                    "issue:acme-issue",
                    "parent_project",
                    "project:acme-proj#view",
                ),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme");
    store
        .write_tuples(
            &globex,
            &principal("globex", "p-admin"),
            &[add("project:globex-proj", "reader", "p:viewer")],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed globex");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_issue_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }

    assert!(
        allows(
            &svc,
            &principal("acme", "p:viewer"),
            ISSUE_VIEW,
            "issue:acme-issue"
        ),
        "the acme project reader views the acme issue (in-tenant)"
    );

    let cross_tenant_leak = allows(
        &svc,
        &principal("globex", "p:viewer"),
        ISSUE_VIEW,
        "issue:acme-issue",
    );
    let cross_tenant_leaks: i64 = i64::from(cross_tenant_leak);

    signals.set_scalar(SignalName::CrossTenantCount, cross_tenant_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        cross_tenant_leaks, 0,
        "0 cross-tenant issue-view leaks (the engine + reverse index read only the verified scope)"
    );

    println!(
        "[P-322 DRILL GREEN 2026-06-22] ISS-D3 (F1) cross-tenant: a globex principal with the same \
         id string attempted view on the acme issue:acme-issue → cross-tenant-leak count=0 (the \
         verified (tenant, region) scope is the partition; no cross-tenant read path)"
    );
}

#[test]
fn iss_d3_zero_leak_under_zookie_staleness() {
    let mut signals = SignalSource::new();
    let s = scope_of(&principal("acme", "p-admin"));

    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in myelin_identity_service::issue_fragment::issue_fragment_defs() {
        assert!(matches!(
            namespace.admit(&def),
            FragmentAdmit::Admitted { .. }
        ));
    }

    let feed = |consumer: &ReverseIndexConsumer| {
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
    };

    let _z_grant = store
        .write_tuples(
            &s,
            &principal("acme", "p-admin"),
            &[add("issue:hot", CONFIDENTIAL_GRANT, "p:alice")],
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("grant");
    feed(&consumer);

    let z_mark = store
        .write_tuples(
            &s,
            &principal("acme", "p-admin"),
            &[
                TupleDelta::Remove(RelationTuple {
                    object: ObjectId("issue:hot".into()),
                    relation: RelName(CONFIDENTIAL_GRANT.into()),
                    subject: PrincipalId("p:alice".into()),
                    caveat: None,
                }),
                add("issue:hot", CONFIDENTIAL, "p:alice"),
            ],
            None,
            None,
            Timestamp("2026-06-22T00:01:00Z".into()),
        )
        .expect("mark confidential + revoke");
    assert!(
        index.watermark(&s).0 < z_mark.0,
        "S8 is BEHIND the confidential-marking revision (watermark={:?} < mark={:?})",
        index.watermark(&s),
        z_mark
    );

    let lo = ListObjects::with_cap(store, namespace, index, 0);
    let post_mark = Consistency {
        at_least: z_mark.clone(),
        mode: ConsistencyMode::Strong,
    };
    let result = lo
        .list_objects_consistent(
            &s,
            &principal("acme", "p:alice"),
            &Permission(ISSUE_VIEW.into()),
            &ObjectType("issue".into()),
            &post_mark,
        )
        .expect("read relationships for the post-confidentiality fallback");

    let stale_leaks: i64 = match result {
        ListObjectsResult::Ids { ids, .. } => i64::from(ids.iter().any(|o| o.0 == "issue:hot")),
        ListObjectsResult::Filter { .. } => {
            panic!(
                "the watermark guard must fall back to per-row check under staleness, not serve a \
                    Filter from the behind index"
            )
        }
    };

    signals.set_scalar(SignalName::CrossTenantCount, stale_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        stale_leaks, 0,
        "0 confidential-issue leaks UNDER zookie staleness (the watermark guard falls back to check \
         over the authoritative S3 - ISS-D3 F2, the new-enemy guard)"
    );

    println!(
        "[P-322 DRILL GREEN 2026-06-22] ISS-D3 (F2) under-staleness: issue:hot marked confidential + \
         alice's confidential_grant revoked at a NEWER zookie, S8 held BEHIND; the board scan pinned \
         at the post-marking zookie fell back to per-row check over authoritative S3 (the watermark \
         guard) → stale-confidential-leak count=0 (the confidential exclusion holds under the S8 \
         watermark, never a stale allow)"
    );
}
