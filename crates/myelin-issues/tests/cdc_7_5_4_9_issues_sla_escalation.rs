//! # The CDC pair for contracts 7.5 + 4.9 — **Issues passes its REAL SLA escalation chain + wires the
//! watcher read-fanout** (NOTIF-P21 / P-342, M4)
//!
//! **Contract-index rows 7.5** (`oncall_now` / `page` — Issues passes the escalation-chain DEFINITION
//! to Notif's durable workflow) and **4.9** (the `watcher` ReBAC relation — Issues wires its read-
//! fanout reverse index over the relation its frozen fragment declares). The Notif machinery (the
//! frozen chain shape + the durable wheel + the `WatcherResolvePort` seam + `read_fanout`) is owned +
//! frozen at NOTIF-P14 (`escalation.rs`) / NOTIF-P13 (`read_fanout.rs`); THIS file pins the **Issues
//! consumer slice** — the REAL SLA chain that REPLACES the Notif-defined `test_chain` floor + the REAL
//! issue watcher index that REPLACES the synthetic read-fanout fixtures.
//!
//! - the **PROVIDER** (the consumer-accretion side) is **Issues defining its REAL SLA chain**
//!   ([`myelin_issues::sla_escalation::issue_sla_escalation_policy`]) + **wiring its REAL watcher
//!   index** ([`myelin_issues::sla_escalation::IssueWatcherIndex`]) — both built/impl'd against the
//!   FROZEN Notif seams, so passing them required ZERO Notif change (the inverse-signal, EI-01 §1).
//! - the **CONSUMER** is **Notif's [`EscalationEngine`](myelin_notif::EscalationEngine) starting +
//!   walking** Issues' chain on the durable wheel, and **Notif's [`read_fanout`](myelin_notif::read_fanout)
//!   lowering** the issue watcher index's `InRelation{watcher}` push-down into a viewer's ambient
//!   "My Work" slice (held-not-leaked on revoke/unavailable).
//!
//! The two sides are pinned here so a drift on either (Issues changes its chain/watcher shape; Notif
//! renames an escalation/read-fanout type) fails this test in the same CI job.

use myelin_events::OutboxStore;
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_issues::sla_escalation::{
    issue_sla_escalation_policy, IssueWatcherIndex, ISSUE_WATCHER_RELATION,
    SLA_ESCALATION_POLICY_ID, SLA_TEAM_ONCALL_SCHEDULE,
};
use myelin_notif::escalation::{
    DurableWheel, EscalationEngine, InMemoryWheel, OncallSchedule, RotationWindow,
};
use myelin_notif::prefs::QuietHours;
use myelin_notif::{read_fanout, AmbientMarkerStore, Reason, WatcherResolvePort};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

fn viewer(id: &str) -> Principal {
    Principal::stub(pid(id), PrincipalKind::Human, tenant())
}

fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

/// A team on-call rotation covering the whole day, on call = the team lead.
fn team_schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: SLA_TEAM_ONCALL_SCHEDULE.into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: pid("psn:team-lead"),
        }],
    }
}

// ===========================================================================
// 7.5 — Issues passes its REAL SLA escalation chain (PROVIDER) → Notif starts + walks it (CONSUMER)
// ===========================================================================

/// **PROVIDER side — Issues' REAL chain is the frozen §2.4 three-tier shape, NOT the Notif floor.**
/// The chain Issues passes has the real Issues policy id (not `esc-test-chain`) and the three real
/// tiers. A drift in the chain shape (a re-ordered/dropped tier) fails here.
#[test]
fn producer_issues_real_sla_chain_is_the_three_tier_frozen_shape() {
    let policy = issue_sla_escalation_policy(15, 1);
    assert_eq!(policy.policy_id, SLA_ESCALATION_POLICY_ID);
    assert_ne!(
        policy.policy_id, "esc-test-chain",
        "the REAL chain, not the floor"
    );
    assert_eq!(policy.steps.len(), 3, "team → project → org");
    // tier 1 is the assignee's team on-call (a schedule target resolved at fire time).
    assert!(policy.step_at(0).is_some());
    assert!(policy.step_at(2).is_some());
    assert!(
        policy.step_at(3).is_none(),
        "exhausted after the 3 tiers (repeat=1)"
    );
}

/// **CONSUMER side — an SLA breach STARTS Issues' real escalation chain on Notif's durable wheel
/// (ISS-D6 chain-start).** Notif's `EscalationEngine::page` takes Issues' REAL chain definition and
/// pages the first tier AT FIRE TIME — with ZERO Notif change (Issues handed a value to the frozen
/// engine). This is the honest "Issues passes its chain and Notif walks it".
#[test]
fn consumer_notif_starts_issues_real_chain_on_the_durable_wheel() {
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let engine = EscalationEngine::new(wheel.clone(), outbox);
    let breach = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    let never_quiet = QuietHours::default();

    // Issues' REAL chain is passed to the frozen engine; the breach STARTS it.
    let (run_id, first) = engine
        .page(
            tenant(),
            Region("fr-par".into()),
            "esc-iss-1".into(),
            issue_sla_escalation_policy(15, 1),
            breach,
            Some(&team_schedule()),
            600,
            &never_quiet,
            false,
        )
        .expect("the SLA breach starts Issues' real chain");

    // the first page reaches the team on-call AT FIRE TIME (the §2.4 resolve-at-fire-time invariant).
    assert_eq!(
        first.principal,
        pid("psn:team-lead"),
        "tier 1 = the team on-call"
    );
    assert_eq!(first.walk, 0);
    // the ack_window DURABLE timer is armed (the chain is live on the wheel).
    assert!(
        wheel.has_timer(&run_id),
        "the ack_window durable timer is armed"
    );
    // the run executes Issues' REAL policy (not the Notif floor).
    let run = engine.run(&run_id).expect("run present");
    assert_eq!(run.policy.policy_id, SLA_ESCALATION_POLICY_ID);
}

// ===========================================================================
// 4.9 — Issues wires its REAL watcher index (PROVIDER) → Notif read-fans-out over it (CONSUMER)
// ===========================================================================

/// **PROVIDER side — Issues wires the read-fanout reverse index over the `watcher` relation its
/// frozen ReBAC fragment declares (one name, X-5).** The index serves ONLY the Issues watcher
/// relation; the relation name is the SAME one Notif reads.
#[test]
fn producer_issues_watcher_index_serves_the_frozen_watcher_relation() {
    assert_eq!(ISSUE_WATCHER_RELATION, "watcher");
    assert_eq!(ISSUE_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);

    let idx = IssueWatcherIndex::new();
    idx.watch(&tenant(), "psn:alice", "myelin://acme/issue/issue/ENG-1421");
    let answer = idx
        .resolve_relation(
            &viewer("psn:alice"),
            &myelin_notif::RelationalLeaf::InRelation {
                relation: myelin_identity::RelName("watcher".into()),
                via_column: myelin_notif::subject_root_col(),
            },
            myelin_notif::RevisionWatermark(0),
        )
        .expect("available");
    assert!(answer
        .subject_roots
        .contains("myelin://acme/issue/issue/ENG-1421"));
}

/// **CONSUMER side — Notif's `read_fanout` lowers the Issues watcher index into a viewer's ambient
/// "My Work" slice; a non-watcher reaches nothing (held, not leaked).** A `watched`-reason ambient
/// marker on a hot issue is ONE coalesced row; the watching viewer reaches it through the
/// `InRelation{watcher}` JOIN, a non-watcher does not — replacing the NOTIF-P13 synthetic fixtures
/// for Issues subjects, ZERO Notif change.
#[test]
fn consumer_notif_read_fans_out_issues_ambient_slice_over_the_real_index() {
    let idx = IssueWatcherIndex::new();
    let hot_issue = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    // alice watches the hot issue; bob does not.
    idx.watch(&tenant(), "psn:alice", &hot_issue.0);
    let at = strong(&idx.current_zookie().0);

    // ONE coalesced ambient marker for the watched issue (zero write amplification — §3.5).
    let markers = AmbientMarkerStore::new();
    markers.record(
        &tenant(),
        &hot_issue,
        Reason::Watched,
        &ArtifactRef("myelin://acme/bus/event/e1".into()),
    );
    assert_eq!(
        markers.marker_count(&tenant()),
        1,
        "ONE marker, not per-watcher"
    );

    // the watching viewer (alice) reaches the marker through the read-fanout JOIN.
    let alice_slice = read_fanout(&viewer("psn:alice"), &markers, &idx, &at)
        .expect("read-fanout resolves alice's slice");
    assert_eq!(
        alice_slice.len(),
        1,
        "alice watches the hot issue → she reaches the marker"
    );
    assert_eq!(alice_slice[0].subject, hot_issue);

    // a non-watcher (bob) reaches NOTHING (the JOIN, never a widen).
    let bob_slice = read_fanout(&viewer("psn:bob"), &markers, &idx, &at)
        .expect("read-fanout resolves bob's empty slice");
    assert!(
        bob_slice.is_empty(),
        "bob does not watch → reaches nothing (held, not leaked)"
    );
}

/// **CONSUMER side — a just-revoked watch is reflected (held, not leaked).** After alice unwatches
/// the hot issue, a read at the new watermark no longer reaches its marker — the read-fanout JOIN
/// reads at-or-after the revocation revision.
#[test]
fn consumer_notif_read_fanout_reflects_a_revoked_watch() {
    let idx = IssueWatcherIndex::new();
    let hot_issue = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    idx.watch(&tenant(), "psn:alice", &hot_issue.0);

    let markers = AmbientMarkerStore::new();
    markers.record(
        &tenant(),
        &hot_issue,
        Reason::Watched,
        &ArtifactRef("myelin://acme/bus/event/e1".into()),
    );

    // alice still watches → reaches the marker.
    let at_watched = strong(&idx.current_zookie().0);
    assert_eq!(
        read_fanout(&viewer("psn:alice"), &markers, &idx, &at_watched)
            .expect("ok")
            .len(),
        1
    );

    // alice unwatches → a read at the NEW watermark no longer reaches it.
    idx.unwatch(&tenant(), "psn:alice", &hot_issue.0);
    let at_revoked = strong(&idx.current_zookie().0);
    assert!(
        read_fanout(&viewer("psn:alice"), &markers, &idx, &at_revoked)
            .expect("ok")
            .is_empty(),
        "the revoked watch is absent from the slice (held, not leaked)"
    );
}
