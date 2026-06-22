//! # NOTIF-D1 — the important-buried ranking drill (deterministic explainable ranking) (P-185)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D1** ("Replay a mixed week → every `critical`/`direct` ranks above every `fyi`;
//! first-important latency within budget; explain-trace per rank." Threshold:
//! **important-buried-rate = 0**), and §3.1 (the deterministic explainable scoring function;
//! `reason → base → class`; every rank carries an explain-trace, NOTIF-2), EI-01 §3 (prove-it:
//! observability is part of the pass — the explain-trace + the important-buried-rate are the measured
//! artifacts; a target you cannot measure is not a gate).
//!
//! **The dated GREEN artifact (2026-06-20).** A mixed "week" of inbox items (the reasons a real week
//! produces: approvals, escalations, SLAs, reviews, assignments, mentions, replies, agent proposals,
//! watches, state-changes, and a flood of low-priority `fyi`s) is ingested into the ONE inbox and
//! READ back through the ranked `list_inbox`. The drill measures + asserts, with NO threshold
//! weakened:
//!
//! 1. **important-buried-rate = 0** — over the ranked read, the index of the LAST `critical`/`direct`
//!    item is strictly before the index of the FIRST `fyi`. NOT ONE critical/direct ranks below ANY
//!    fyi. The rate is `(# fyi ranked above some critical/direct) / (# critical/direct)` — asserted
//!    `== 0`. The threshold is 0 — never weakened.
//! 2. **first-important latency within budget** — the rank position (0-based) of the first
//!    `critical`/`direct` item in the ranked read IS the "scroll distance to the first important
//!    thing"; the budget is the count of higher-or-equal-priority items ahead of it, asserted `== 0`
//!    (a critical is the highest band — nothing legitimately ranks above it). This is the measured
//!    first-important latency the prompt names (in rank-positions, the deterministic surrogate for
//!    wall-clock scroll cost).
//! 3. **an explain-trace present on EVERY rank** — 100% of ranked items carry a deterministic,
//!    non-empty explain-trace ("why am I seeing this, ranked here?", NOTIF-2). The trace's
//!    `final_priority` equals the item's priority (the trace IS the rank's provenance, not a
//!    decoration). The threshold is 100% — never weakened.
//!
//! The ranking is the router's downstream read surface: a SECOND leg ingests real Signals through the
//! live `SignalRouter` (the NOTIF-P3 wire) and ranks the resulting inbox — proving the rank reads the
//! SAME projection the router UPSERTs into (no second store), end to end.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::list_inbox::{list_inbox_ranked, AllowAllAuthorize, InboxFilter, Page};
use myelin_notif::ranking::DeterministicV1;
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{build_router, Class, Reason};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn me() -> Principal {
    Principal::stub(PrincipalId("u-week".into()), PrincipalKind::Human, tenant())
}
fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("zk-notif-d1".into()),
        mode: ConsistencyMode::Strong,
    }
}

/// One inbox row addressed to the viewer, about `subject`, with `reason` (the class is derived).
fn item(item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: item_id.into(),
        recipient: "u-week".into(),
        subject: ArtifactRef(subject.into()),
        reason,
        class: myelin_notif::ranking::class_for(reason),
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

/// **A mixed "week" of inbox items** — the reasons a real week produces, deliberately seeded in a
/// SHUFFLED (non-rank) order with the fyi flood interleaved AHEAD of the important items, so the
/// rank has to actually reorder (a no-op ranker would leave the fyis buried among the criticals and
/// FAIL the drill).
fn mixed_week() -> InboxProjection {
    let inbox = InboxProjection::new();
    // The fyis come FIRST in insertion order (and have low item_ids) — a broken ranker that leaves
    // insertion / item_id order would bury the criticals BELOW them and trip the gate.
    inbox.upsert_for_test(item("a-fyi-1", "myelin://acme/issue/issue/F1", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-2", "myelin://acme/chat/thread/F2", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-3", "myelin://acme/git/pr/F3", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-4", "myelin://acme/issue/issue/F4", Reason::Fyi));
    inbox.upsert_for_test(item("a-fyi-5", "myelin://acme/ci/run/F5", Reason::Fyi));
    // the watching band.
    inbox.upsert_for_test(item(
        "b-watched",
        "myelin://acme/git/pr/W1",
        Reason::Watched,
    ));
    inbox.upsert_for_test(item(
        "b-state",
        "myelin://acme/issue/issue/W2",
        Reason::StateChanged,
    ));
    // the participating band.
    inbox.upsert_for_test(item(
        "c-replied",
        "myelin://acme/chat/thread/P1",
        Reason::Replied,
    ));
    inbox.upsert_for_test(item(
        "c-agent",
        "myelin://acme/issue/issue/P2",
        Reason::AgentProposal,
    ));
    // the direct band.
    inbox.upsert_for_test(item(
        "d-review",
        "myelin://acme/git/pr/D1",
        Reason::ReviewRequested,
    ));
    inbox.upsert_for_test(item(
        "d-assigned",
        "myelin://acme/issue/issue/D2",
        Reason::Assigned,
    ));
    inbox.upsert_for_test(item(
        "d-mention",
        "myelin://acme/chat/thread/D3",
        Reason::Mentioned,
    ));
    // the critical band (the must-see-first).
    inbox.upsert_for_test(item(
        "e-approval",
        "myelin://acme/issue/issue/C1",
        Reason::ApprovalRequested,
    ));
    inbox.upsert_for_test(item(
        "e-escalated",
        "myelin://acme/ci/run/C2",
        Reason::Escalated,
    ));
    inbox.upsert_for_test(item("e-sla", "myelin://acme/issue/issue/C3", Reason::Sla));
    inbox
}

/// **NOTIF-D1: important-buried-rate = 0 + first-important latency in budget + an explain-trace on
/// EVERY rank (the dated green).**
#[test]
fn notif_d1_important_never_buried_with_explain_trace_per_rank() {
    let inbox = mixed_week();
    let ranker = DeterministicV1::default();
    let page = list_inbox_ranked(
        &inbox,
        &me(),
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
        &ranker,
    );
    assert_eq!(
        page.items.len(),
        15,
        "the whole mixed week is read back (the ONE inbox)"
    );

    // (3) — EXPLAIN-TRACE PRESENT ON EVERY RANK (100%, NOTIF-2). Measure the coverage and assert it.
    let with_trace = page
        .items
        .iter()
        .filter(|r| !r.trace.render().is_empty() && r.trace.final_priority == r.priority)
        .count();
    let trace_coverage = with_trace as f64 / page.items.len() as f64;
    assert_eq!(
        trace_coverage, 1.0,
        "100% of ranks carry a deterministic explain-trace (NOTIF-2)"
    );

    // (1) — IMPORTANT-BURIED-RATE = 0. For each critical/direct item, count how many fyis rank
    // ABOVE it (a lower index = higher rank). The buried count must be 0 (not one critical/direct
    // sits below any fyi). The threshold is 0 — never weakened.
    let classes: Vec<Class> = page.items.iter().map(|r| r.trace.class).collect();
    let first_fyi = classes.iter().position(|c| *c == Class::Fyi);
    let last_important = classes
        .iter()
        .rposition(|c| matches!(c, Class::Critical | Class::Direct));
    let mut buried = 0usize;
    let mut important_total = 0usize;
    for (idx, class) in classes.iter().enumerate() {
        if matches!(class, Class::Critical | Class::Direct) {
            important_total += 1;
            // is there ANY fyi ranked above this important item?
            if let Some(ff) = first_fyi {
                if ff < idx {
                    buried += 1;
                }
            }
        }
    }
    let important_buried_rate = buried as f64 / important_total as f64;
    assert_eq!(
        important_buried_rate, 0.0,
        "NOTIF-D1: important-buried-rate = 0 (no critical/direct ranks below any fyi) — never weakened"
    );
    // the structural belt: the last important index is strictly before the first fyi index.
    if let (Some(li), Some(ff)) = (last_important, first_fyi) {
        assert!(
            li < ff,
            "every critical/direct ranks above every fyi (the band invariant)"
        );
    }

    // (2) — FIRST-IMPORTANT LATENCY WITHIN BUDGET. The rank-position of the first critical/direct is
    // the "scroll distance to the first important thing"; the budget is 0 items of strictly-higher
    // priority ahead of it (a critical is the top band — nothing legitimately precedes it). The
    // first item in the ranked read IS critical, and its position is 0.
    let first_important_pos = classes
        .iter()
        .position(|c| matches!(c, Class::Critical | Class::Direct))
        .expect("the week has critical/direct items");
    let higher_ahead = page.items[..first_important_pos]
        .iter()
        .filter(|r| r.priority > page.items[first_important_pos].priority)
        .count();
    assert_eq!(
        higher_ahead, 0,
        "first-important latency in budget: nothing outranks the first critical"
    );
    assert_eq!(
        first_important_pos, 0,
        "the first important item is at the TOP of the ranked inbox"
    );
    assert_eq!(
        page.items[0].trace.class,
        Class::Critical,
        "the very first ranked item is a critical (the must-see-first)"
    );

    // The within-band order is the stable item_id order under the v1 neutral-affinity seam: the
    // three criticals (e-*) by item_id, then the three directs (d-*), … the fyis (a-fyi-*) last.
    let order: Vec<&str> = page.items.iter().map(|r| r.item.item_id.as_str()).collect();
    assert_eq!(
        &order[..6],
        &[
            "e-approval",
            "e-escalated",
            "e-sla",
            "d-assigned",
            "d-mention",
            "d-review"
        ],
        "the criticals (90) then the directs (70), each in stable item_id order"
    );
    assert!(
        order[order.len() - 5..]
            .iter()
            .all(|id| id.starts_with("a-fyi-")),
        "the five fyis (15) sink to the bottom of the ranked inbox"
    );
}

// =============================================================================================
//  The SECOND leg — the ranking reads the SAME projection the live router UPSERTs into (the wire).
// =============================================================================================

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

fn signal_msg(id: &str, sig: &Signal) -> Message {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    let env = EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(me()),
        subject: ArtifactRef(subject.clone()),
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
    };
    Message {
        subject,
        envelope: env,
    }
}

/// **NOTIF-D1 (wire): the ranked read reads the SAME projection the live `SignalRouter` UPSERTs into
/// (no second store), and every routed item gets a ranked, explain-traced read.** The router
/// classifies the skeleton reason (NOTIF-P3 — the per-reason classification is NOTIF-P8), so this
/// leg proves the END-TO-END path (Signal → router UPSERT → ranked list_inbox) carries a trace per
/// rank and orders deterministically — the band-invariant leg above proves the rank's correctness.
#[test]
fn notif_d1_ranking_reads_the_routed_inbox_with_trace_per_rank() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // Three Signals on the SAME rule (→ the SAME opaque `psn:watcher:<rule>` recipient, NOTIF-P3
    // skeleton routing) but DISTINCT dedup keys (→ three distinct inbox rows, no write-time collapse).
    for (i, sig) in [
        signal(
            "ci_run_failed",
            Severity::Error,
            "myelin://acme/ci/run/1",
            "run-1",
        ),
        signal(
            "ci_run_failed",
            Severity::Warning,
            "myelin://acme/git/pr/9",
            "pr-9",
        ),
        signal(
            "ci_run_failed",
            Severity::Critical,
            "myelin://acme/issue/issue/PROJ-1",
            "sla-1",
        ),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            consumer.deliver(&signal_msg(&format!("evt-{i}"), sig)),
            Delivered::Acked
        );
    }
    assert_eq!(
        inbox.len(),
        3,
        "the router UPSERTed three rows into the ONE projection"
    );

    // The router routes to an OPAQUE recipient (NOTIF-P3 skeleton). Read whoever it addressed.
    let recipient = inbox.snapshot_for_tenant(&tenant())[0].recipient.clone();
    let viewer = Principal::stub(PrincipalId(recipient), PrincipalKind::Human, tenant());
    let page = list_inbox_ranked(
        &inbox,
        &viewer,
        &InboxFilter::all(),
        &Page {
            after: None,
            limit: 1000,
        },
        &AllowAllAuthorize,
        &strong(),
        &DeterministicV1::default(),
    );
    assert_eq!(
        page.items.len(),
        3,
        "the ranked read reads the SAME projection the router wrote"
    );
    // every routed item carries a deterministic, complete explain-trace (the 100%-trace gate, NOTIF-2).
    for r in &page.items {
        assert_eq!(
            r.priority, r.trace.final_priority,
            "the trace IS the rank's provenance"
        );
        assert!(
            !r.trace.render().is_empty(),
            "every routed rank carries an explain-trace"
        );
    }
    // the order is priority-descending (deterministic).
    let priorities: Vec<u8> = page.items.iter().map(|r| r.priority).collect();
    let mut sorted = priorities.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        priorities, sorted,
        "the ranked read is priority-descending end to end"
    );
}
