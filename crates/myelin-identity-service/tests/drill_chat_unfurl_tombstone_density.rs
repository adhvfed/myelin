use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService, ListObjectsResult,
    ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind, RelName,
    RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    chat_fragment, ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer,
    StoreBackedCheck, TupleStore, CHANNEL_MEMBER, CHANNEL_READ, CONFIDENTIAL, CONFIDENTIAL_GRANT,
    ISSUE_VIEW, WATCHER_RELATION,
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
fn chat_confidential_unfurl_degrades_to_tombstone_zero_leak() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));
    let store = TupleStore::new(OutboxStore::new());

    const FLEET: usize = 64;
    let mut tuples: Vec<TupleDelta> = vec![
        add("channel:general", "parent_project", "project:proj#view"),
        add("message:m1", "parent_channel", "channel:general#read"),
        add("unfurl:u1", "parent_message", "message:m1#view"),
        add("unfurl:u1", "target", "issue:secret"),
        add("issue:secret", "parent_project", "project:proj#view"),
        add("issue:secret", CONFIDENTIAL_GRANT, "p:owner"),
        add("project:proj", "reader", "p:owner"),
    ];
    for i in 0..FLEET {
        let r = format!("p:member-{i}");
        tuples.push(add("project:proj", "reader", &r));
        tuples.push(add("issue:secret", CONFIDENTIAL, &r));
    }

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed acme chat+issue grants");

    let svc = StoreBackedCheck::new(store);
    for admit in svc.admit_chat_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }
    for admit in svc.admit_issue_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }

    assert!(
        allows(
            &svc,
            &principal("acme", "p:member-0"),
            CHANNEL_READ,
            "channel:general"
        ),
        "a project reader reads the channel (the unfurl shell renders; only the TARGET tombstones)"
    );
    assert!(
        allows(
            &svc,
            &principal("acme", "p:owner"),
            ISSUE_VIEW,
            "issue:secret"
        ),
        "the confidential_grant owner renders the real unfurl (the target view resolves)"
    );

    let mut title_leaks: i64 = 0;
    for i in 0..FLEET {
        if allows(
            &svc,
            &principal("acme", &format!("p:member-{i}")),
            ISSUE_VIEW,
            "issue:secret",
        ) {
            title_leaks += 1;
        }
    }

    signals.set_scalar(SignalName::CrossTenantCount, title_leaks);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        title_leaks, 0,
        "0 chat title leaks: every channel member's unfurl of the confidential issue degrades to a \
         tombstone (the Refs check(viewer, view, target) denies via the − confidential exclusion)"
    );

    println!(
        "[P-323 DRILL GREEN 2026-06-22] CHAT (F1) confidential-unfurl-tombstone: \
         fleet={FLEET} channel members attempted the per-viewer unfurl render of issue:secret \
         (the Refs check(viewer, view, target) over the target's own fragment) → chat-title-leak \
         count=0; the unfurl degrades to a tombstone, only the direct confidential_grant owner renders \
         it (§5, unfurls cannot leak)"
    );
}

fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in chat_fragment::chat_fragment_defs() {
        let admit = namespace.admit(&def);
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }
    store
        .write_tuples(
            scope,
            &principal("acme", "p-admin"),
            grants,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, namespace, index, cap)
}

#[test]
fn chat_search_as_non_member_zero_results() {
    let mut signals = SignalSource::new();
    let acme = scope_of(&principal("acme", "p-admin"));

    let grants = vec![
        add("channel:secret", CHANNEL_MEMBER, "p:insider-1"),
        add("channel:secret", CHANNEL_MEMBER, "p:insider-2"),
    ];
    let lo = wired(100, &acme, &grants);

    let result = lo.list_objects(
        &acme,
        &principal("acme", "p:outsider"),
        &Permission(CHANNEL_READ.into()),
        &ObjectType("channel".into()),
        &at_latest(),
    );
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids.into_iter().map(|o| o.0).collect::<Vec<String>>(),
        ListObjectsResult::Filter { .. } => {
            panic!("below the cap the channel list materialises Ids")
        }
    };

    let leaked: i64 = ids.len() as i64;
    signals.set_scalar(SignalName::CrossTenantCount, leaked);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        leaked, 0,
        "a non-member's channel search returns 0 results (the channel is in neither read arm; never a \
         post-filter): {ids:?}"
    );

    println!(
        "[P-323 DRILL GREEN 2026-06-22] CHAT (F1) search-as-non-member: a non-member ran \
         list_objects(read, channel) over a private channel → 0 results (the channel never becomes a \
         candidate - the search-requires-acl-filter leak gate, §5)"
    );
}

#[test]
fn chat_list_subjects_watcher_50k_density_within_budget() {
    let acme = scope_of(&principal("acme", "p-admin"));
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    const DENSITY: usize = 50_000;
    let mut tuples: Vec<TupleDelta> = Vec::with_capacity(DENSITY);
    for i in 0..DENSITY {
        tuples.push(add("channel:wide", WATCHER_RELATION, &format!("p:w-{i}")));
    }
    tuples.push(add("channel:other", WATCHER_RELATION, "p:elsewhere"));

    store
        .write_tuples(
            &acme,
            &principal("acme", "p-admin"),
            &tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed 50k watchers");

    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }

    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_chat_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }));
    }

    let started = std::time::Instant::now();
    let watchers = svc.list_watchers_in(&acme, &ObjectId("channel:wide".into()), &at_latest());
    let elapsed = started.elapsed();

    assert_eq!(
        watchers.relation,
        RelName(WATCHER_RELATION.into()),
        "the fanout expands the watcher relation"
    );
    assert_eq!(
        watchers.members.len(),
        DENSITY,
        "all 50k watchers are returned (the density holds; the Expand is complete)"
    );
    assert!(
        !watchers.members.iter().any(|m| m.0 == "p:elsewhere"),
        "a watcher of a different channel never leaks into channel:wide's fanout"
    );

    let budget = std::time::Duration::from_secs(10);
    assert!(
        elapsed < budget,
        "the 50k-watcher fanout completes within budget: elapsed={elapsed:?}, budget={budget:?}"
    );

    println!(
        "[P-323 DRILL GREEN 2026-06-22] CHAT (F1) list_subjects(channel, watcher) density: \
         channel:wide with {DENSITY} watchers → fanout returned all {DENSITY} members in {elapsed:?} \
         (< {budget:?} budget), 0 cross-channel leak (§7.5, the ordinary S8 Expand at real Chat density; \
         the tighter SLO is the M5 measured tunable P-ID-32)"
    );
}
