//! Contract 11.6-C5 CDC pair — the OLAP restriction-flag gate (the Issues analytics consumer).
//!
//! **Prompt:** P-ST-29 → global **P-331** (M4). The prompt requires "the provider+consumer pair for
//! 11.6-C5 (the Issues analytics consumer)".
//!
//! The PROVIDER is `myelin-storage` (the C5 gate this prompt ships — [`OlapAnalytics`] over the OLAP
//! read model, which excludes a restricted subject's rows from every analytics aggregate). The
//! CONSUMER is the **Issues analytics consumer** — modelled here as a tiny `IssuesAnalyticsReport`
//! that asks the gate for CFD/cycle-time/velocity/delivery-health and renders the report the Issues
//! subsystem serves (CR §8: `issue.*`/`sla.*`/`cycle.*` reports depend on T4). The contract the
//! consumer relies on: **a restricted subject's contribution is ABSENT from the aggregates it reads,
//! and `olap_restricted_subject_leak == 0`.** A restricted subject leaking into an Issues analytics
//! report is the §3.4 C5 breach this CDC pins.
//!
//! If 11.6-C5's surface drifts (the aggregate filter, the leak audit, or the restriction read), this
//! stops compiling/passing — that is the contract.

use myelin_storage::{
    AnalyticsAggregate, OlapAnalytics, OlapEvent, OlapReadStore, RestrictionGateSignal,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

/// Feed a fact into the OLAP read model (the live consumer path — never an OLTP scan).
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

/// **The CONSUMER of 11.6-C5: the Issues analytics report.** It reads the C5-gated aggregates and
/// renders the per-subsystem report. It does NOT re-implement the read model or the filter — it
/// consults the provider's [`OlapAnalytics`] gate, which guarantees a restricted subject is excluded.
struct IssuesAnalyticsReport {
    cfd_items: usize,
    velocity: u64,
    cycle_time_n: u64,
    delivery_health_wip: u64,
    leak: u64,
}

impl IssuesAnalyticsReport {
    /// Render the Issues analytics report over the C5-gated OLAP store (the consumer's read).
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

/// The provider+consumer happy path: the Issues analytics report over an UNRESTRICTED store sees
/// every subject's contribution; after `restrict(subject)` it sees the restricted subject's
/// contribution ABSENT from every aggregate, with `olap_restricted_subject_leak == 0`. If the gate's
/// surface drifts, this stops compiling/passing — that is the contract.
#[test]
fn cdc_11_6_c5_issues_report_excludes_a_restricted_subject() {
    let mut store = OlapReadStore::pinned_to(region());
    project(&mut store, "e1", "issue:PROJ-1", "subj:alice");
    project(&mut store, "e2", "issue:PROJ-2", "subj:bob");
    project(&mut store, "e3", "issue:PROJ-3", "subj:alice");

    // The consumer's report unrestricted — every subject contributes.
    let before = IssuesAnalyticsReport::render(&store);
    assert_eq!(before.cfd_items, 3, "three CFD items unrestricted");
    assert_eq!(before.velocity, 3, "three contribute to velocity");
    assert_eq!(before.leak, 0, "no restriction → no leak");

    // The provider applies `restrict(subject)` (the contract Storage owns).
    store.set_restricted("subj:alice", true);

    // The consumer re-renders — alice's contribution is ABSENT from every aggregate.
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
        "GATE: olap_restricted_subject_leak == 0 — no restricted subject leaks into the Issues report"
    );
}

/// The consumer's contract REQUIRES the gate to run over EVERY C5 aggregate (CFD/cycle-time/velocity/
/// delivery-health) — a gate that skipped one would let a restricted subject leak into the skipped
/// report. The signal pins `aggregates_checked == 4`.
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
    // Each named aggregate appears in the per-aggregate breakdown.
    for agg in AnalyticsAggregate::ALL {
        assert!(
            audit.per_aggregate.contains_key(agg.name()),
            "the gate covers {}",
            agg.name()
        );
    }
}
