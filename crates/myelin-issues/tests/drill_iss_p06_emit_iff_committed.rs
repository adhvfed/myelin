use myelin_events::{
    Actor, ArtifactRef, ConsumerName, DedupLedger, EmitContextBase, InProcessBus, MonotonicMinter,
    OutboxStore, Region, Relay, TenantId, Timestamp,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, FragmentAdmit,
    IdentityService, ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission,
    Precondition, Principal, PrincipalId, PrincipalKind, RewriteTrace, RunId, RunToken,
    SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::{apply_mutation, events, issue_aggregate_key, IssueDraft, MutationKind};
use std::sync::Arc;

struct AllowId;
type IdResult<T> = myelin_identity::Result<T>;
impl IdentityService for AllowId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _a: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(Decision::Allow)
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _a: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _a: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Ok(Zookie("zk-drill".into()))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("u-1".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
        caused_by: None,
    }
}

fn actor() -> Principal {
    Principal::stub(
        PrincipalId("u-1".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn draft(project_id: u128) -> IssueDraft {
    IssueDraft {
        project_id,
        title: "fix the charge bug".into(),
        props: b"{}".to_vec(),
        reporter_pseudonym: "psn:abc".into(),
    }
}

#[test]
fn issue_write_path_emit_iff_committed_kill_between_commit_and_publish() {
    let store = OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
    let id = AllowId;

    let out = apply_mutation(
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &id,
        &actor(),
        "ENG-1",
        &MutationKind::Create(draft(7)),
        None,
    )
    .expect("an allowed create commits");
    let eid = out.event_id.expect("create emits a lifecycle event");
    assert_eq!(
        store.outbox_depth(),
        1,
        "the issue.created event co-committed (unsent)"
    );
    assert_eq!(store.committed_count(), 1);

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus, || {
        Timestamp("2026-06-21T10:00:02Z".into())
    });

    relay.transport().sever();
    let report = relay.drain_once();
    assert_eq!(report.published, 0, "a severed broker delivers nothing");
    assert_eq!(
        store.outbox_depth(),
        1,
        "0 LOST: the committed event survives the kill (still unsent, not dropped)"
    );
    assert_eq!(relay.transport().delivered_count(), 0);

    relay.transport().heal();
    relay.drain_to_empty();
    assert_eq!(
        store.outbox_depth(),
        0,
        "0 LOST: the committed event drains to delivered after heal"
    );
    assert_eq!(
        relay.transport().delivered_count(),
        1,
        "0 GHOST: the event is delivered EXACTLY once (its row committed)"
    );
    assert!(
        relay.transport().delivered_ids().contains(&eid),
        "the delivered id is the committed event's stable id"
    );
    assert_eq!(
        store.dead_letter_count(),
        0,
        "no poison row on the no-loss path"
    );
}

#[test]
fn denied_write_leaves_the_outbox_empty_zero_ghost() {
    let store = OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());

    struct DenyId;
    impl IdentityService for DenyId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ArtifactRef,
            _a: &Consistency,
            _c: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Ok(Decision::Deny)
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    let r = apply_mutation(
        &store,
        minter,
        ctx_base(),
        &DenyId,
        &actor(),
        "ENG-9",
        &MutationKind::Create(draft(7)),
        None,
    );
    assert!(r.is_err(), "a denied write fails");
    assert_eq!(
        store.committed_count(),
        0,
        "a denied write co-commits nothing"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus, || {
        Timestamp("2026-06-21T10:00:02Z".into())
    });
    relay.drain_to_empty();
    assert_eq!(
        relay.transport().delivered_count(),
        0,
        "0 GHOST: a denied write delivers nothing"
    );
}

#[test]
fn cdc_provider_issue_events_are_per_aggregate_ordered() {
    let store = OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
    let id = AllowId;
    let agg = issue_aggregate_key(7, "ENG-1");

    let create = apply_mutation(
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &id,
        &actor(),
        "ENG-1",
        &MutationKind::Create(draft(7)),
        None,
    )
    .unwrap();
    let second = apply_mutation(
        &store,
        Arc::clone(&minter),
        ctx_base(),
        &id,
        &actor(),
        "ENG-1",
        &MutationKind::Create(draft(7)),
        None,
    )
    .unwrap();

    let r0 = store.row(&create.event_id.unwrap()).unwrap();
    let r1 = store.row(&second.event_id.unwrap()).unwrap();
    assert_eq!(r0.aggregate, agg);
    assert_eq!(r1.aggregate, agg);
    assert_eq!(r0.seq, 0, "first event for the issue aggregate is seq 0");
    assert_eq!(r1.seq, 1, "second event is seq 1 (monotonic, gap-free)");
    assert_eq!(r0.envelope.type_.0, events::ISSUE_CREATED);
    assert!(events::ISSUE_EVENT_TOKENS.contains(&r0.envelope.type_.0.as_str()));
}

#[test]
fn cdc_consumer_dedup_suppresses_a_replayed_issue_event() {
    let store = OutboxStore::new();
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
    let id = AllowId;

    let out = apply_mutation(
        &store,
        minter,
        ctx_base(),
        &id,
        &actor(),
        "ENG-2",
        &MutationKind::Create(draft(7)),
        None,
    )
    .unwrap();
    let eid = out.event_id.unwrap();

    let ledger = DedupLedger::new();
    let consumer = ConsumerName("issues-rollup".into());

    assert!(
        ledger
            .mark_handled(&consumer, &eid)
            .expect("in-memory dedup storage is available"),
        "first delivery is newly handled"
    );
    assert!(ledger
        .is_handled(&consumer, &eid)
        .expect("in-memory dedup storage is available"));

    assert!(
        !ledger
            .mark_handled(&consumer, &eid)
            .expect("in-memory dedup storage is available"),
        "a replay of the same stable event_id is dedup-suppressed (0 double-handle)"
    );
}
