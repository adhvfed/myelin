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

#[test]
fn producer_issues_real_sla_chain_is_the_three_tier_frozen_shape() {
    let policy = issue_sla_escalation_policy(15, 1);
    assert_eq!(policy.policy_id, SLA_ESCALATION_POLICY_ID);
    assert_ne!(
        policy.policy_id, "esc-test-chain",
        "the REAL chain, not the floor"
    );
    assert_eq!(policy.steps.len(), 3, "team → project → org");
    assert!(policy.step_at(0).is_some());
    assert!(policy.step_at(2).is_some());
    assert!(
        policy.step_at(3).is_none(),
        "exhausted after the 3 tiers (repeat=1)"
    );
}

#[test]
fn consumer_notif_starts_issues_real_chain_on_the_durable_wheel() {
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let engine = EscalationEngine::new(wheel.clone(), outbox);
    let breach = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    let never_quiet = QuietHours::default();

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

    assert_eq!(
        first.principal,
        pid("psn:team-lead"),
        "tier 1 = the team on-call"
    );
    assert_eq!(first.walk, 0);
    assert!(
        wheel.has_timer(&run_id),
        "the ack_window durable timer is armed"
    );
    let run = engine.run(&run_id).expect("run present");
    assert_eq!(run.policy.policy_id, SLA_ESCALATION_POLICY_ID);
}

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

#[test]
fn consumer_notif_read_fans_out_issues_ambient_slice_over_the_real_index() {
    let idx = IssueWatcherIndex::new();
    let hot_issue = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    idx.watch(&tenant(), "psn:alice", &hot_issue.0);
    let at = strong(&idx.current_zookie().0);

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

    let alice_slice = read_fanout(&viewer("psn:alice"), &markers, &idx, &at)
        .expect("read-fanout resolves alice's slice");
    assert_eq!(
        alice_slice.len(),
        1,
        "alice watches the hot issue → she reaches the marker"
    );
    assert_eq!(alice_slice[0].subject, hot_issue);

    let bob_slice = read_fanout(&viewer("psn:bob"), &markers, &idx, &at)
        .expect("read-fanout resolves bob's empty slice");
    assert!(
        bob_slice.is_empty(),
        "bob does not watch → reaches nothing (held, not leaked)"
    );
}

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

    let at_watched = strong(&idx.current_zookie().0);
    assert_eq!(
        read_fanout(&viewer("psn:alice"), &markers, &idx, &at_watched)
            .expect("ok")
            .len(),
        1
    );

    idx.unwatch(&tenant(), "psn:alice", &hot_issue.0);
    let at_revoked = strong(&idx.current_zookie().0);
    assert!(
        read_fanout(&viewer("psn:alice"), &markers, &idx, &at_revoked)
            .expect("ok")
            .is_empty(),
        "the revoked watch is absent from the slice (held, not leaked)"
    );
}
