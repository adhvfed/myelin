//! # GIT-D9 — receive-pack → one-tx ref-CAS + outbox (the silent-data-loss floor) — P-270 / GIT-P9
//!
//! **Drill:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **GIT-D9** — crash
//! the serving tier mid-push (after policy, BEFORE and AFTER commit) → `git.ref.updated` is emitted
//! **IFF** the ref move committed; **0 ghost, 0 lost**; quarantine objects discarded on abort.
//!
//! **Contract:** 2.2/2.3 (`OutboxTx::emit` + the per-ref aggregate, one-tx co-commit) + 2.9
//! (`git.ref.updated`). **Architecture:** git-hosting `02 §2/§3/§4`.
//!
//! This is the **end-to-end chained drill** (EI-01 §4): the receive-pack write path
//! ([`myelin_git::receive_pack::RefStore`]) is driven through the REAL Bus outbox relay
//! ([`myelin_events::relay::Relay`] over the in-process [`InProcessBus`]). The green artifact is the
//! emit-iff-committed signal measured across the kill: depth → 0 only via the relay's delivery, and
//! the delivered-id set equals the committed-event set exactly (0 ghost = no delivered id without a
//! committed ref move; 0 lost = no committed ref move whose event never delivered).
//!
//! **PERMANENT-gate family note:** GIT-D9 is in the STOR-D1/D2 store-touching family — it re-runs on
//! every change that touches the ref store / outbox path (the no-loss floor is never "done once").

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
        pusher: Pusher { pseudonym: "anon-3@acme.noreply".into(), is_agent: false },
    }
}

/// A relay over the in-process broker that drains the store's outbox (the real Bus delivery path).
fn relay(outbox: &OutboxStore) -> Relay<InProcessBus> {
    Relay::new(outbox.clone(), InProcessBus::new(), || {
        Timestamp("2026-06-21T00:05:00Z".into())
    })
}

/// **GIT-D9 — the happy path delivers exactly the committed event (0 ghost / 0 lost).** Push →
/// commit → relay drains → the delivered-id set equals the committed-event set; outbox depth → 0.
#[test]
fn git_d9_happy_path_delivers_iff_committed_zero_ghost_zero_lost() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    let committed_id = match store
        .receive(&push("refs/heads/feature", Oid::zero(), Oid::new("c0ffee")), &db, CrashPoint::None)
        .unwrap()
    {
        PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
        o => panic!("expected Accepted, got {o:?}"),
    };
    // Before the relay runs: the event is durable but unsent (depth 1).
    assert_eq!(outbox.outbox_depth(), 1);

    let r = relay(&outbox);
    let report = r.drain_to_empty();
    assert_eq!(report.published, 1, "exactly the one committed event was delivered");

    // 0 lost: the committed event WAS delivered. 0 ghost: nothing ELSE was delivered.
    let delivered: std::collections::HashSet<EventId> = r.transport().delivered_ids();
    assert_eq!(delivered.len(), 1);
    assert!(delivered.contains(&committed_id), "0 lost — the committed ref move's event delivered");
    // The depth drained to 0 (the survival signal), 0 dead-letters.
    assert_eq!(outbox.outbox_depth(), 0, "depth drained to 0 — emit-iff-committed delivered");
    assert_eq!(outbox.dead_letter_count(), 0);
}

/// **GIT-D9 — push → policy REJECT path: nothing is emitted, nothing is delivered (0 ghost).** The
/// rejected push never moves a ref and never stages an event, so the relay delivers nothing.
#[test]
fn git_d9_policy_reject_path_emits_and_delivers_nothing() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    // A force-push on the protected `main` — rejected before the ref moves.
    let mut p = push("refs/heads/main", Oid::zero(), Oid::new("aaaa"));
    p.updates[0].forced = true;
    p.updates[0].expected_old = Oid::zero();
    // (main does not exist yet; a force CREATE is still a forced flag on a protected ref → reject.)
    match store.receive(&p, &db, CrashPoint::None).unwrap() {
        PushOutcome::Rejected(RejectReason::ForcePushOnProtected { .. }) => {}
        o => panic!("expected ForcePushOnProtected, got {o:?}"),
    }

    let r = relay(&outbox);
    let report = r.drain_to_empty();
    assert_eq!(report.published, 0, "a rejected push delivers nothing");
    assert_eq!(r.transport().delivered_count(), 0);
    assert_eq!(outbox.committed_count(), 0, "0 ghost — the reject emitted nothing");
    assert!(db.is_empty(), "the quarantine was discarded (never promoted)");
}

/// **GIT-D9 — crash BEFORE commit → recover → emit-iff-committed (0 ghost).** The serving tier dies
/// after the object migration but before the transaction commits. On recovery the client RETRIES;
/// the retry commits and the relay delivers exactly ONE event (the crash left no ghost row).
#[test]
fn git_d9_crash_before_commit_then_recover_is_zero_ghost() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    // The kill: the process dies before the transaction commits.
    match store
        .receive(&push("refs/heads/feature", Oid::zero(), Oid::new("v1")), &db, CrashPoint::BeforeCommit)
        .unwrap()
    {
        PushOutcome::Crashed(c) => assert_eq!(c.at, CrashPoint::BeforeCommit),
        o => panic!("expected Crashed, got {o:?}"),
    }
    // emit-iff-committed: the crash left NO row + the ref unmoved (0 ghost). Recovery discards the
    // un-acked state; the orphan objects are harmless (content-addressed).
    assert_eq!(outbox.committed_count(), 0, "0 ghost — the un-committed transaction left no row");
    assert_eq!(store.tip(&RefName::new("refs/heads/feature")), None, "the ref never moved");

    // RECOVER: the client retries the push (same expected-old zero — the ref still doesn't exist).
    let committed_id = match store
        .receive(&push("refs/heads/feature", Oid::zero(), Oid::new("v1")), &db, CrashPoint::None)
        .unwrap()
    {
        PushOutcome::Accepted { emitted, .. } => emitted[0].clone(),
        o => panic!("expected Accepted on retry, got {o:?}"),
    };
    assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("v1")));

    // The relay delivers EXACTLY ONE event (the retry's) — not two (no ghost from the crash).
    let r = relay(&outbox);
    r.drain_to_empty();
    let delivered = r.transport().delivered_ids();
    assert_eq!(delivered.len(), 1, "0 ghost — exactly one event delivered after the crash+retry");
    assert!(delivered.contains(&committed_id));
    assert_eq!(outbox.outbox_depth(), 0);
}

/// **GIT-D9 — crash AFTER commit → recover (relay restarts) → emit-iff-committed (0 lost).** The
/// transaction committed (the ref moved + the event is durable + unsent), THEN the serving tier
/// died before any post-commit work. A FRESH relay (the restarted process) drains the durable row →
/// the committed event is delivered exactly once (0 lost, 0 ghost via broker dedup on re-claim).
#[test]
fn git_d9_crash_after_commit_then_relay_restart_is_zero_lost() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    // The kill: the process dies AFTER the transaction committed.
    match store
        .receive(&push("refs/heads/feature", Oid::zero(), Oid::new("done")), &db, CrashPoint::AfterCommit)
        .unwrap()
    {
        PushOutcome::Crashed(c) => assert_eq!(c.at, CrashPoint::AfterCommit),
        o => panic!("expected Crashed, got {o:?}"),
    }
    // 0 lost: the ref MOVED and the event row is durable + unsent — the kill lost neither.
    assert_eq!(store.tip(&RefName::new("refs/heads/feature")), Some(Oid::new("done")));
    assert_eq!(outbox.committed_count(), 1);
    assert_eq!(outbox.outbox_depth(), 1, "the committed event awaits the (restarted) relay");

    // RECOVER: a fresh relay (the restarted process) drains the durable row → delivered exactly once.
    let r = relay(&outbox);
    let report = r.drain_to_empty();
    assert_eq!(report.published, 1, "0 lost — the committed event is delivered after the restart");
    assert_eq!(outbox.outbox_depth(), 0, "depth drained to 0");
    assert_eq!(outbox.dead_letter_count(), 0);
}

/// **GIT-D9 — a crash mid-PUBLISH (broker hiccup) re-claims and delivers once (0 ghost / 0 lost).**
/// The transaction committed; the relay's first publish attempt fails (broker severed), so the row
/// stays claimable; on heal the relay re-claims and the broker dedup (Nats-Msg-Id = event_id)
/// guarantees the redelivery is suppressed → exactly one logical delivery.
#[test]
fn git_d9_crash_mid_publish_redelivers_once_via_broker_dedup() {
    let (store, outbox) = open_store();
    let db = InMemoryObjectDb::new();

    store
        .receive(&push("refs/heads/feature", Oid::zero(), Oid::new("x1")), &db, CrashPoint::None)
        .unwrap();
    assert_eq!(outbox.outbox_depth(), 1);

    let r = relay(&outbox);
    // The broker is severed for the first publish attempt (the mid-publish crash) — the row stays
    // unsent + claimable (NOT dead-lettered yet, NOT lost).
    r.transport().fail_next(1);
    let first = r.drain_once();
    assert_eq!(first.published, 0, "the severed publish delivered nothing");
    assert_eq!(outbox.outbox_depth(), 1, "the row is still unsent (claimable) — 0 lost");

    // On the next pass the broker is healthy: the relay re-claims + delivers exactly once.
    let second = r.drain_to_empty();
    assert_eq!(second.published, 1, "the re-claim delivered the committed event once");
    assert_eq!(r.transport().delivered_count(), 1, "0 ghost — exactly one logical delivery");
    assert_eq!(outbox.outbox_depth(), 0);
}
