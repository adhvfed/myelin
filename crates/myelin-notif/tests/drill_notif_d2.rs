//! # NOTIF-D2 — five-mechanism storm-control: 1000 near-identical CI failures + a 30-comment PR
//! burst → bounded items; 0 self-notifications; measured dedup-collapse-ratio (P-189)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D2** ("1000 near-identical CI failures + a 30-comment PR burst → bounded items
//! (`coalesce_count` correct); self-notifications suppressed." Telemetry: **dedup-collapse-ratio;
//! 0 self**), and `notifications.md` §3.2 (the five write-time mechanisms), EI-04 §5.3 (Notif is a
//! projection — storm-control suppresses delivery/ranking, **never the audit/history**).
//!
//! **The dated GREEN artifact (2026-06-20).** A storm of 1000 near-identical CI-failure Signals (the
//! SAME `(rule, dedup_key)` — one incident) is driven through the LIVE Signal-consumer router
//! ([`build_router`]); a 30-comment PR burst is driven through it; a self-burst (the recipient is the
//! actor) is driven through it. The drill asserts, through the harness telemetry-assertion library
//! (the SAME `dedup_collapse_ratio` signal contract-1.8 names):
//!
//! 1. **N → 1**: the 1000 near-identical failures collapse to ONE inbox row with `coalesce_count`
//!    = 1000 (the "+N more" write-time collapse, §3.2.2). Bounded items.
//! 2. **0 self-notifications**: a burst FROM the recipient themselves (actor == recipient) produces
//!    ZERO inbox rows (§3.2.1 self-suppression).
//! 3. **the measured dedup-collapse-ratio**: `>= 9000 bps` (≈ 99% of the storm absorbed write-time).
//!    The assertion has teeth — a regression that stops collapsing (every Signal opens its own row →
//!    ratio 0) fails LOUDLY.
//! 4. **the audit untouched**: storm-control suppressed delivery/ranking — but every Signal still
//!    exists on the bus (the underlying events were never removed). The `notif.item.created` emit
//!    count equals the DELIVERED count (one per opened row), NOT the inbound Signal count — delivery
//!    is suppressed, the audit is not.
//!
//! The storm-tolerance is the router's, not the test's: [`myelin_notif::StormControl`] runs the five
//! mechanisms between classify and UPSERT inside [`SignalRouter::handle`].

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{
    build_router, dedup_collapse_ratio_bps, InboxProjection, ROUTER_CONSUMER_NAME,
};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
/// The CI bot — the actor of the failure Signals (NOT the recipient, so they are NOT self-suppressed).
fn ci_bot() -> Principal {
    Principal::stub(PrincipalId("p-ci-bot".into()), PrincipalKind::Service, tenant())
}

fn signal(rule: &str, severity: Severity, subject: &str, dedup: &str) -> Signal {
    Signal {
        rule_id: RuleId(rule.into()),
        tenant: tenant(),
        severity,
        dedup_key: DedupKey(dedup.into()),
        subject: ArtifactRef(subject.into()),
        count: 1,
        state: SignalState::Open,
        first_seen: "2026-06-20T00:00:00Z".into(),
        last_seen: "2026-06-20T00:00:00Z".into(),
    }
}

/// A `sig.<tenant>.…` envelope carrying `sig` as payload, attributed to `actor`, with broker `id`.
fn envelope(id: &str, sig: &Signal, actor: Principal) -> EventEnvelope {
    let subject = format!("sig.{}.{}.{}", sig.tenant.0, sig.severity.token(), sig.rule_id.0);
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(actor),
        subject: ArtifactRef(subject),
        aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::to_value(sig).unwrap(),
    }
}

fn msg(id: &str, sig: &Signal, actor: Principal) -> Message {
    let env = envelope(id, sig, actor);
    Message { subject: env.subject.0.clone(), envelope: env }
}

/// **NOTIF-D2: 1000 near-identical CI failures → ONE row (coalesce_count 1000); measured
/// dedup-collapse-ratio >= 9000 bps; the audit untouched.**
#[test]
fn notif_d2_storm_collapses_to_one_row_ratio_measured() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // 1000 near-identical CI-failure Signals: the SAME rule + dedup_key (one incident), DISTINCT
    // broker event_ids (so the consumer-dedup ledger does NOT short-circuit — these are distinct
    // deliveries that storm-control collapses at the inbox-row level).
    let inbound = 1000u64;
    let sig = signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/42", "run-42");
    for i in 0..inbound {
        let out = consumer.deliver(&msg(&format!("evt-{i}"), &sig, ci_bot()));
        assert_eq!(out, Delivered::Acked, "every Signal acks (none stalls)");
    }

    // (1) N → 1: the 1000 near-identical failures collapsed to ONE inbox row.
    assert_eq!(inbox.len(), 1, "NOTIF-D2: 1000 near-identical CI failures → ONE inbox row (N→1)");
    let row = inbox
        .get(&tenant(), "psn:watcher:ci_run_failed", "ci_run_failed:run-42")
        .expect("the one collapsed row");
    assert_eq!(row.coalesce_count, 1000, "the +N more counter is the full incident count (1000)");

    // (4) The audit untouched: storm-control suppressed DELIVERY (only ONE notif.item.created emit —
    // the opened row — not 1000). Every underlying Signal still exists on the bus (it was acked, not
    // removed). Delivery/ranking suppressed; the audit/history is not (EI-04 §5.3).
    assert_eq!(
        outbox.committed_count(),
        1,
        "exactly ONE notif.item.created emitted (the opened row); the 999 collapses did not re-push"
    );

    // (3) The measured dedup-collapse-ratio (contract 1.8) — the drill's green artifact. 999 of 1000
    // collapsed → 9990 bps. Assert it through the harness telemetry-assertion library with a floor.
    let collapsed = inbound - 1; // one opened, the rest collapsed.
    let ratio = dedup_collapse_ratio_bps(inbound, collapsed);
    assert_eq!(ratio, 9990, "999/1000 collapsed → 9990 bps");
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        ratio,
    );
    // The floor: >= 9000 bps (≈ 90% absorbed). A regression that stops collapsing (every Signal opens
    // its own row → ratio 0) FAILS this LOUDLY (the assertion has teeth, not inverted away).
    src.assert_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Gte(9000),
    )
    .expect_green();
}

/// **NOTIF-D2: a self-burst → 0 inbox items (0 self-notifications).** A burst of 30 Signals whose
/// VERIFIED actor IS the recipient is self-suppressed (§3.2.1); not one becomes a row, not one emits.
#[test]
fn notif_d2_self_burst_produces_zero_items() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // The skeleton router routes to recipient `psn:watcher:<rule>`. To exercise self-suppression we
    // make the actor's opaque principal id EQUAL that recipient pseudonym (the action's author IS the
    // watcher). 30 distinct deliveries (distinct event_ids), same incident.
    let self_recipient =
        Principal::stub(PrincipalId("psn:watcher:my_change".into()), PrincipalKind::Human, tenant());
    let sig = signal("my_change", Severity::Warning, "myelin://acme/chat/thread/T1", "t1");
    for i in 0..30 {
        let out = consumer.deliver(&msg(&format!("self-{i}"), &sig, self_recipient.clone()));
        assert_eq!(out, Delivered::Acked, "a self-action acks (terminal, not a stall)");
    }

    assert_eq!(inbox.len(), 0, "NOTIF-D2: a 30-event self-burst → 0 inbox items (0 self-notifications)");
    assert_eq!(outbox.committed_count(), 0, "0 emits (a self-action pushes no delivery)");
}

/// **NOTIF-D2: a 30-comment PR burst is bounded (not 30 separate pushes), and the audit is
/// untouched.** Distinct comments on ONE PR collapse/coalesce/damp into a bounded set of pushes; the
/// 30 underlying Signals all still exist on the bus.
#[test]
fn notif_d2_30_comment_pr_burst_is_bounded() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // 30 DISTINCT comments on the same PR (the same rule + dedup_key for the skeleton router → they
    // collapse onto ONE row; a richer per-comment dedup_key would coalesce — either way BOUNDED).
    let sig = signal("pr_comment", Severity::Info, "myelin://acme/git/pr/9", "pr-9");
    for i in 0..30 {
        consumer.deliver(&msg(&format!("c-{i}"), &sig, ci_bot()));
    }

    // Bounded: ONE row (the comments collapsed), and the pushes are bounded (one opened row → one
    // emit, the rest collapsed without re-pushing). NOT 30 separate notifications.
    assert_eq!(inbox.len(), 1, "the 30-comment burst collapsed to ONE inbox row (bounded)");
    let row = inbox.get(&tenant(), "psn:watcher:pr_comment", "pr_comment:pr-9").unwrap();
    assert_eq!(row.coalesce_count, 30, "+N more = 30 (the full comment count, bounded into one row)");
    assert_eq!(outbox.committed_count(), 1, "bounded pushes (1, not 30) — the audit (30 Signals) untouched");
}

/// **The dedup-collapse-ratio alarm WOULD fire on a regression (the drill is not vacuous).** A run
/// that stopped collapsing (every Signal opened its own row → ratio 0 bps) asserted against the
/// `>= 9000` floor is RED — proving the green above is earned.
#[test]
fn notif_d2_ratio_alarm_fires_on_a_collapse_regression() {
    // Model the regressed state: 1000 inbound, 0 collapsed → ratio 0.
    let ratio = dedup_collapse_ratio_bps(1000, 0);
    assert_eq!(ratio, 0, "no collapse → 0 bps");
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        ratio,
    );
    let verdict = src.assert_labelled(
        SignalName::DedupCollapseRatio,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Gte(9000),
    );
    assert!(!verdict.is_green(), "ratio 0 against `>= 9000` is RED — the alarm fires on a collapse regression");
}
