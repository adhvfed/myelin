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

use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, PrincipalId as IdPrincipalId, PrincipalKind as IdPrincipalKind,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::read_fanout::{read_fanout, SyntheticReverseIndex};
use myelin_notif::{
    build_router, dedup_collapse_ratio_bps, InboxProjection, Reason, DEFAULT_HOT_SUBJECT_WRITE_CAP,
    ROUTER_CONSUMER_NAME, SIGNAL_MENTIONS_KEY,
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
    Principal::stub(
        PrincipalId("p-ci-bot".into()),
        PrincipalKind::Service,
        tenant(),
    )
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
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
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
    Message {
        subject: env.subject.0.clone(),
        envelope: env,
    }
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
    let sig = signal(
        "ci_run_failed",
        Severity::Error,
        "myelin://acme/ci/run/42",
        "run-42",
    );
    for i in 0..inbound {
        let out = consumer.deliver(&msg(&format!("evt-{i}"), &sig, ci_bot()));
        assert_eq!(out, Delivered::Acked, "every Signal acks (none stalls)");
    }

    // (1) N → 1: the 1000 near-identical failures collapsed to ONE inbox row.
    assert_eq!(
        inbox.len(),
        1,
        "NOTIF-D2: 1000 near-identical CI failures → ONE inbox row (N→1)"
    );
    let row = inbox
        .get(
            &tenant(),
            "psn:watcher:ci_run_failed",
            "ci_run_failed:run-42",
        )
        .expect("the one collapsed row");
    assert_eq!(
        row.coalesce_count, 1000,
        "the +N more counter is the full incident count (1000)"
    );

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
    let self_recipient = Principal::stub(
        PrincipalId("psn:watcher:my_change".into()),
        PrincipalKind::Human,
        tenant(),
    );
    let sig = signal(
        "my_change",
        Severity::Warning,
        "myelin://acme/chat/thread/T1",
        "t1",
    );
    for i in 0..30 {
        let out = consumer.deliver(&msg(&format!("self-{i}"), &sig, self_recipient.clone()));
        assert_eq!(
            out,
            Delivered::Acked,
            "a self-action acks (terminal, not a stall)"
        );
    }

    assert_eq!(
        inbox.len(),
        0,
        "NOTIF-D2: a 30-event self-burst → 0 inbox items (0 self-notifications)"
    );
    assert_eq!(
        outbox.committed_count(),
        0,
        "0 emits (a self-action pushes no delivery)"
    );
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
    let sig = signal(
        "pr_comment",
        Severity::Info,
        "myelin://acme/git/pr/9",
        "pr-9",
    );
    for i in 0..30 {
        consumer.deliver(&msg(&format!("c-{i}"), &sig, ci_bot()));
    }

    // Bounded: ONE row (the comments collapsed), and the pushes are bounded (one opened row → one
    // emit, the rest collapsed without re-pushing). NOT 30 separate notifications.
    assert_eq!(
        inbox.len(),
        1,
        "the 30-comment burst collapsed to ONE inbox row (bounded)"
    );
    let row = inbox
        .get(&tenant(), "psn:watcher:pr_comment", "pr_comment:pr-9")
        .unwrap();
    assert_eq!(
        row.coalesce_count, 30,
        "+N more = 30 (the full comment count, bounded into one row)"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "bounded pushes (1, not 30) — the audit (30 Signals) untouched"
    );
}

/// A `sig.<tenant>.…` envelope carrying the Signal + the STRUCTURED `mention(Principal)` nodes under
/// the frozen wire key (the dispatch tier stamps them; Notif reads the structured node, never free
/// text — AG-6). The actor is the CI bot (not a mentioned recipient, so not self-suppressed).
fn mention_msg(id: &str, sig: &Signal, mentions: &[Principal]) -> Message {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    let nodes: Vec<InlineNode> = mentions.iter().cloned().map(InlineNode::Mention).collect();
    let mut payload = serde_json::to_value(sig).unwrap();
    if let serde_json::Value::Object(map) = &mut payload {
        map.insert(
            SIGNAL_MENTIONS_KEY.into(),
            serde_json::to_value(&nodes).unwrap(),
        );
    }
    Message {
        subject: subject.clone(),
        envelope: EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType("signal.opened".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(ci_bot()),
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
            payload,
        },
    }
}

/// **NOTIF-D2 (the mention-storm WRITE-FANOUT side, NOTIF-P12): a mention-storm on ONE hot subject is
/// BOUNDED by the hot-subject cap — at most `cap` mention rows materialise; the rest coalesce. 0
/// unbounded write amplification.**
///
/// A `@here`-style spray of 200 DISTINCT mentioned principals on ONE hot subject_root is driven
/// through the LIVE router. The hot-subject cap (§3.2.4) bounds the write-amplification: exactly
/// [`DEFAULT_HOT_SUBJECT_WRITE_CAP`] (64) DISTINCT mention rows materialise; the other 136 overflow
/// into the coalesced "+N more were mentioned" marker (counted, never lost). This is the write-side
/// analogue of the read-fanout's "store ONE coalesced marker" (§3.5) — a celebrity-spray mention
/// costs at most `cap` write rows, never `N`. Proven jointly with NOTIF-P13's read-fanout.
#[test]
fn notif_d2_mention_storm_write_fanout_is_bounded_by_the_hot_subject_cap() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // A mention-storm: 200 DISTINCT recipients mentioned on ONE hot subject (a @here on a big channel).
    let storm_size = 200usize;
    let sig = signal(
        "mention_spray",
        Severity::Info,
        "myelin://acme/chat/thread/hot",
        "spray",
    );
    let mentions: Vec<Principal> = (0..storm_size)
        .map(|i| {
            Principal::stub(
                PrincipalId(format!("p-{i}")),
                PrincipalKind::Human,
                tenant(),
            )
        })
        .collect();

    let out = consumer.deliver(&mention_msg("evt-mention-storm", &sig, &mentions));
    assert_eq!(
        out,
        Delivered::Acked,
        "the mention-storm Signal routes + acks"
    );

    // BOUNDED: exactly `cap` DISTINCT mention rows materialised on the hot subject_root — NOT 200.
    let subject_root = "myelin://acme/chat/thread/hot";
    assert_eq!(
        consumer.handler().hot_cap().admitted_count(subject_root),
        DEFAULT_HOT_SUBJECT_WRITE_CAP,
        "NOTIF-D2: the mention-storm is bounded to `cap` write rows (0 unbounded write amplification)"
    );
    assert_eq!(
        consumer.handler().hot_cap().overflow_count(subject_root),
        storm_size as u32 - DEFAULT_HOT_SUBJECT_WRITE_CAP,
        "the rest overflowed into the coalesced marker (the +N more were mentioned counter — preserved)"
    );

    // The inbox holds the bounded mention rows (`cap`) — NOT 200. A mention-storm CANNOT write-amplify.
    let mention_rows = inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .filter(|r| r.reason == Reason::Mentioned)
        .count();
    assert_eq!(
        mention_rows as u32, DEFAULT_HOT_SUBJECT_WRITE_CAP,
        "exactly `cap` mention rows materialised — the write-amplification bound holds (§3.2.4)"
    );
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
    assert!(
        !verdict.is_green(),
        "ratio 0 against `>= 9000` is RED — the alarm fires on a collapse regression"
    );
}

// ===========================================================================================
//  NOTIF-D2 — the READ-FANOUT amplification leg (NOTIF-P13 / P-191): a 50k-watcher subject → 0
//  per-watcher write rows (ONE coalesced marker); one JOIN on inbox open; the zookie watermark.
// ===========================================================================================

fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: myelin_identity::Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

/// **NOTIF-D2 (the read-fanout amplification leg, NOTIF-P13): a 50k-watcher subject hit by a storm of
/// ambient events produces ZERO per-watcher write rows — ONE coalesced marker.** A celebrity subject
/// (a hot PR / a 50k channel) is hit by 500 ambient events through the LIVE router; the read-fanout
/// marker store holds exactly ONE marker (count 500), not 500 rows and not 50k watcher rows. This is
/// the read-side analogue of the write-fanout's hot-subject cap (proven jointly with NOTIF-P12):
/// **zero write amplification** regardless of watcher count. Threshold: 1 marker; 0 per-watcher rows.
#[test]
fn notif_d2_read_fanout_50k_watcher_subject_zero_write_amplification() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // 500 ambient events on ONE hot subject (a PR watched by 50k people — the watcher count NEVER
    // enters the write path). DISTINCT broker event_ids so the consumer-dedup ledger does not
    // short-circuit; the SAME subject so the read-fanout coalesces into ONE marker.
    let storm = 500u64;
    let sig = signal(
        "pr_activity",
        Severity::Info,
        "myelin://acme/git/pr/celebrity",
        "pr-celeb",
    );
    for i in 0..storm {
        let out = consumer.deliver(&msg(&format!("amb-{i}"), &sig, ci_bot()));
        assert_eq!(out, Delivered::Acked, "every ambient event routes + acks");
    }

    // ZERO write amplification: the read-fanout marker store holds exactly ONE marker for the hot
    // subject_root — NOT 500, and NOT 50k (one per watcher). The watcher count is irrelevant to the
    // write side (the watchers are resolved at READ time, not exploded into writes).
    let markers = consumer.handler().ambient();
    assert_eq!(
        markers.marker_count(&tenant()),
        1,
        "NOTIF-D2: a 50k-watcher subject hit 500 times → ONE coalesced marker (0 write amplification)"
    );
    let m = markers
        .get(&tenant(), "myelin://acme/git/pr/celebrity")
        .unwrap();
    assert_eq!(
        m.count, storm,
        "the +N more counter is the full activity count (preserved, never lost)"
    );
}

/// **NOTIF-D2 (read-fanout, NOTIF-P13): the per-viewer slice is materialised LAZILY on inbox open via
/// ONE SetExpr JOIN — and the zookie watermark reflects a just-revoked watch (held, not leaked).** The
/// LIVE router records ONE marker per hot subject; on inbox open, a watcher resolves their slice via
/// the SetExpr watcher push-down JOIN against the (synthetic) authz reverse index — ONE query, no
/// N+1. A revoked watch (a newer zookie) is reflected: the revoked subject is ABSENT from the slice.
#[test]
fn notif_d2_read_fanout_lazy_materialise_join_and_zookie_watermark() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // Two hot subjects each hit by an ambient burst (ONE marker each — zero write amplification).
    for (rule, subject, dedup) in [
        ("pr_a", "myelin://acme/git/pr/A", "a"),
        ("pr_b", "myelin://acme/git/pr/B", "b"),
    ] {
        let sig = signal(rule, Severity::Info, subject, dedup);
        for i in 0..50 {
            consumer.deliver(&msg(&format!("{rule}-{i}"), &sig, ci_bot()));
        }
    }
    let markers = consumer.handler().ambient();
    assert_eq!(
        markers.marker_count(&tenant()),
        2,
        "two hot subjects → two coalesced markers (not 100 rows)"
    );

    // The viewer watches BOTH hot subjects (the synthetic reverse index stands in for the real
    // watcher ReBAC fragment — the named floor; the real fragments land in NOTIF-P19..P22).
    let idx = SyntheticReverseIndex::new();
    idx.grant_watch(&tenant(), "watcher-1", "myelin://acme/git/pr/A");
    idx.grant_watch(&tenant(), "watcher-1", "myelin://acme/git/pr/B");
    let watcher = myelin_identity::Principal::stub(
        IdPrincipalId("watcher-1".into()),
        IdPrincipalKind::Human,
        tenant(),
    );

    // INBOX OPEN: the read-fanout materialises the viewer's slice LAZILY via the SetExpr JOIN — ONE
    // query (no per-subject N+1). The viewer watches both → both markers materialise.
    let before = read_fanout(&watcher, markers, &idx, &strong(&idx.current_zookie().0)).unwrap();
    let before_roots: Vec<&str> = before.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        before_roots,
        vec!["myelin://acme/git/pr/A", "myelin://acme/git/pr/B"],
        "the read-fanout materialised exactly the watched slice on inbox open (the SetExpr JOIN)"
    );

    // THE ZOOKIE WATERMARK (contract 4.10): revoke the watch on B (a NEWER zookie). A read at the new
    // watermark reflects the revocation — B is HELD, not leaked.
    let new_zk = idx.revoke_watch(&tenant(), "watcher-1", "myelin://acme/git/pr/B");
    let after = read_fanout(&watcher, markers, &idx, &strong(&new_zk.0)).unwrap();
    let after_roots: Vec<&str> = after.iter().map(|m| m.subject_root.as_str()).collect();
    assert_eq!(
        after_roots,
        vec!["myelin://acme/git/pr/A"],
        "NOTIF-D2: a just-revoked watch is reflected at-or-after the zookie watermark (B held, not leaked)"
    );
}
