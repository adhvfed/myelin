use myelin_events::relay::{InProcessBus, Relay};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, EventId, IdMinter, MonotonicMinter, OutboxStore, Region,
    TenantId, Timestamp,
};
use myelin_git::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid, ProposedRefUpdate, PushOutcome, PushSession, Pusher,
    QuarantineObject, RefName, RefStore, RejectReason,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::sync::Arc;

const TENANT: &str = "acme";
const REGION: &str = "fr-par";

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(TENANT.into()),
        region: Region(REGION.into()),
        actor: Actor(Principal::stub(
            PrincipalId("dev-1".into()),
            PrincipalKind::Human,
            TenantId(TENANT.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:push".into())),
    }
}

fn open_store() -> (RefStore, OutboxStore) {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let store = RefStore::open("core", ctx_base(), outbox.clone(), minter);
    (store, outbox)
}

fn push(ref_name: &str, old: Oid, new: Oid) -> PushSession {
    PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: RefName::new(ref_name),
            expected_old: old,
            new_oid: new.clone(),
            forced: false,
            commit_oids: vec![new],
        }],
        quarantine: vec![QuarantineObject {
            oid: Oid::new("feed"),
            bytes: b"a benign commit object".to_vec(),
        }],
        pusher: Pusher {
            pseudonym: "anon-3@acme.noreply".into(),
            is_agent: false,
        },
    }
}

fn relay(outbox: &OutboxStore) -> Relay<InProcessBus> {
    Relay::new(outbox.clone(), InProcessBus::new(), || {
        Timestamp("2026-06-21T00:05:00Z".into())
    })
}

#[test]
fn git_d9_happy_path_delivers_iff_committed_zero_ghost_zero_lost() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    let committed_id = match store
        .receive(
            &push("refs/heads/feature", Oid::zero(), Oid::new("c0ffee")),
            &db,
            CrashPoint::None,
        )
        .unwrap()
    {
        PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
        o => panic!("expected Accepted, got {o:?}"),
    };
    assert_eq!(outbox.outbox_depth(), 1);

    let r = relay(&outbox);
    let report = r.drain_to_empty();
    assert_eq!(
        report.published, 1,
        "exactly the one committed event was delivered"
    );

    let delivered: std::collections::HashSet<EventId> = r.transport().delivered_ids();
    assert_eq!(delivered.len(), 1);
    assert!(
        delivered.contains(&committed_id),
        "0 lost - the committed ref move's event delivered"
    );
    assert_eq!(
        outbox.outbox_depth(),
        0,
        "depth drained to 0 - emit-iff-committed delivered"
    );
    assert_eq!(outbox.dead_letter_count(), 0);
}

#[test]
fn git_d9_policy_reject_path_emits_and_delivers_nothing() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    let mut p = push("refs/heads/main", Oid::zero(), Oid::new("aaaa"));
    p.updates[0].forced = true;
    p.updates[0].expected_old = Oid::zero();
    match store.receive(&p, &db, CrashPoint::None).unwrap() {
        PushOutcome::Rejected(RejectReason::ForcePushOnProtected { .. }) => {}
        o => panic!("expected ForcePushOnProtected, got {o:?}"),
    }

    let r = relay(&outbox);
    let report = r.drain_to_empty();
    assert_eq!(report.published, 0, "a rejected push delivers nothing");
    assert_eq!(r.transport().delivered_count(), 0);
    assert_eq!(
        outbox.committed_count(),
        0,
        "0 ghost - the reject emitted nothing"
    );
    assert!(
        db.is_empty(),
        "the quarantine was discarded (never promoted)"
    );
}

#[test]
fn git_d9_crash_before_commit_then_recover_is_zero_ghost() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    match store
        .receive(
            &push("refs/heads/feature", Oid::zero(), Oid::new("v1")),
            &db,
            CrashPoint::BeforeCommit,
        )
        .unwrap()
    {
        PushOutcome::Crashed(c) => assert_eq!(c.at, CrashPoint::BeforeCommit),
        o => panic!("expected Crashed, got {o:?}"),
    }
    assert_eq!(
        outbox.committed_count(),
        0,
        "0 ghost - the un-committed transaction left no row"
    );
    assert_eq!(
        store.tip(&RefName::new("refs/heads/feature")),
        None,
        "the ref never moved"
    );

    let committed_id = match store
        .receive(
            &push("refs/heads/feature", Oid::zero(), Oid::new("v1")),
            &db,
            CrashPoint::None,
        )
        .unwrap()
    {
        PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
        o => panic!("expected Accepted on retry, got {o:?}"),
    };
    assert_eq!(
        store.tip(&RefName::new("refs/heads/feature")),
        Some(Oid::new("v1"))
    );

    let r = relay(&outbox);
    r.drain_to_empty();
    let delivered = r.transport().delivered_ids();
    assert_eq!(
        delivered.len(),
        1,
        "0 ghost - exactly one event delivered after the crash+retry"
    );
    assert!(delivered.contains(&committed_id));
    assert_eq!(outbox.outbox_depth(), 0);
}

#[test]
fn git_d9_crash_after_commit_then_relay_restart_is_zero_lost() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    match store
        .receive(
            &push("refs/heads/feature", Oid::zero(), Oid::new("done")),
            &db,
            CrashPoint::AfterCommit,
        )
        .unwrap()
    {
        PushOutcome::Crashed(c) => assert_eq!(c.at, CrashPoint::AfterCommit),
        o => panic!("expected Crashed, got {o:?}"),
    }
    assert_eq!(
        store.tip(&RefName::new("refs/heads/feature")),
        Some(Oid::new("done"))
    );
    assert_eq!(outbox.committed_count(), 1);
    assert_eq!(
        outbox.outbox_depth(),
        1,
        "the committed event awaits the (restarted) relay"
    );

    let r = relay(&outbox);
    let report = r.drain_to_empty();
    assert_eq!(
        report.published, 1,
        "0 lost - the committed event is delivered after the restart"
    );
    assert_eq!(outbox.outbox_depth(), 0, "depth drained to 0");
    assert_eq!(outbox.dead_letter_count(), 0);
}

#[test]
fn git_d9_crash_mid_publish_redelivers_once_via_broker_dedup() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    store
        .receive(
            &push("refs/heads/feature", Oid::zero(), Oid::new("x1")),
            &db,
            CrashPoint::None,
        )
        .unwrap();
    assert_eq!(outbox.outbox_depth(), 1);

    let r = relay(&outbox);
    r.transport().fail_next(1);
    let first = r.drain_once();
    assert_eq!(first.published, 0, "the severed publish delivered nothing");
    assert_eq!(
        outbox.outbox_depth(),
        1,
        "the row is still unsent (claimable) - 0 lost"
    );

    let second = r.drain_to_empty();
    assert_eq!(
        second.published, 1,
        "the re-claim delivered the committed event once"
    );
    assert_eq!(
        r.transport().delivered_count(),
        1,
        "0 ghost - exactly one logical delivery"
    );
    assert_eq!(outbox.outbox_depth(), 0);
}
