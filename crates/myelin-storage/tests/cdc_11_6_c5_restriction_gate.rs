use myelin_storage::{
    AnalyticsAggregate, OlapAnalytics, OlapEvent, OlapReadStore, RestrictionGateSignal,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn project(store: &mut OlapReadStore, event_id: &str, aggregate: &str, subject: &str) {
    store
        .apply(&OlapEvent {
            event_id: event_id.into(),
            tenant: TenantId::from_token("acme"),
            region: region(),
            aggregate_row: aggregate.into(),
            subject: Some(subject.into()),
        })
        .expect("an in-region event is admitted");
}

struct IssuesAnalyticsReport {
    cfd_items: usize,
    velocity: u64,
    cycle_time_n: u64,
    delivery_health_wip: u64,
    leak: u64,
}

impl IssuesAnalyticsReport {
    fn render(store: &OlapReadStore) -> IssuesAnalyticsReport {
        let analytics = OlapAnalytics::over(store);
        let audit = analytics.leak_audit();
        IssuesAnalyticsReport {
            cfd_items: analytics.cfd().len(),
            velocity: analytics.velocity(),
            cycle_time_n: analytics.cycle_time_sample_size(),
            delivery_health_wip: analytics.delivery_health_wip(),
            leak: audit.olap_restricted_subject_leak,
        }
    }
}

#[test]
fn cdc_11_6_c5_issues_report_excludes_a_restricted_subject() {
    let mut store = OlapReadStore::pinned_to(region());
    project(&mut store, "e1", "issue:PROJ-1", "subj:alice");
    project(&mut store, "e2", "issue:PROJ-2", "subj:bob");
    project(&mut store, "e3", "issue:PROJ-3", "subj:alice");

    let before = IssuesAnalyticsReport::render(&store);
    assert_eq!(before.cfd_items, 3, "three CFD items unrestricted");
    assert_eq!(before.velocity, 3, "three contribute to velocity");
    assert_eq!(before.leak, 0, "no restriction → no leak");

    store.set_restricted("subj:alice", true);

    let after = IssuesAnalyticsReport::render(&store);
    assert_eq!(after.cfd_items, 1, "only bob's CFD item survives");
    assert_eq!(after.velocity, 1, "alice excluded from velocity");
    assert_eq!(after.cycle_time_n, 1, "alice excluded from cycle-time");
    assert_eq!(
        after.delivery_health_wip, 1,
        "alice excluded from delivery-health"
    );
    assert_eq!(
        after.leak, 0,
        "GATE: olap_restricted_subject_leak == 0 - no restricted subject leaks into the Issues report"
    );
}

#[test]
fn cdc_11_6_c5_gate_covers_every_issues_aggregate() {
    let mut store = OlapReadStore::pinned_to(region());
    project(&mut store, "e1", "issue:PROJ-1", "subj:alice");
    store.set_restricted("subj:alice", true);

    let audit = OlapAnalytics::over(&store).leak_audit();
    let signal = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 1);
    assert!(
        signal.is_green(),
        "the D-S12 gate is green for the Issues consumer"
    );
    assert_eq!(
        signal.aggregates_checked,
        AnalyticsAggregate::ALL.len() as u64,
        "every C5 aggregate the Issues consumer reads is gated"
    );
    for agg in AnalyticsAggregate::ALL {
        assert!(
            audit.per_aggregate.contains_key(agg.name()),
            "the gate covers {}",
            agg.name()
        );
    }
}
