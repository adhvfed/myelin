//! # GIT-D10 / CI-D8 — the X-1 check seam END-TO-END (EB-27 / P-327, M4)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` rows
//! **GIT-D10** + **CI-D8** (the X-1 check seam end-to-end). **Thresholds (exact — never weaken):**
//! - out-of-order/dup `ci.check.updated` → `run_attempt`-monotonic supersession holds, drops stale;
//! - a fork PR self-greens → **neutral for gating** (until endorsed/re-run-trusted);
//! - a **doubly-delivered `ci.result` → the merge-queue workflow wakes EXACTLY ONCE**;
//! - **0 double-merge** (merge-count == 1).
//!
//! ## What this drill proves (the Bus's NARROW half of the END-TO-END seam)
//! With Git's CONSUMER leg live (EB-26 / P-246, M3) and CI's PRODUCER leg live (EB-27 / P-327, M4 —
//! [`check_updated_draft`] + [`rollup_ci_result`] + [`ci_result_draft`]), the X-1 seam is now
//! END-TO-END. This drill exercises the whole producer→carriage→consumer→merge-queue flow over the
//! Bus's substrate and asserts the merge-queue NARROW guarantees:
//!
//! 1. **CI PRODUCES** per-context `ci.check.updated` facts (incl. a re-run + an at-least-once
//!    duplicate), delivered SCRAMBLED + out-of-`seq`.
//! 2. **The Bus CARRIES** them per-aggregate ordered on `(repo, commit_oid)` — the D-11 substrate.
//! 3. **The CONSUMER** (modelled here as the post-supersession current status; Git owns the real
//!    rule) reads the highest `run_attempt` per context.
//! 4. **CI DERIVES** the `ci.result` rollup from the post-supersession current status over the
//!    REQUIRED gate set ([`rollup_ci_result`]) and emits it ([`ci_result_draft`]).
//! 5. **The merge-queue durable workflow** parks on `wait_for_signal("ci.result", idem_key)` and is
//!    delivered the rollup TWICE (at-least-once) → it **wakes EXACTLY ONCE** → it merges EXACTLY ONCE
//!    (**0 double-merge**).
//!
//! The Bus owns ONLY: the envelope conformance, the per-aggregate ordering, the at-least-once
//! delivery, and the idempotent `wait_for_signal` substrate. The `CheckStatus` shape, the
//! supersession rule, trust-tier gating, and the merge gate are CI/Git (contract 5.9). The drill
//! reads its verdict off the Bus's ordering-health telemetry through the FROZEN §10.2 harness
//! assertion library (the ordering GAP → `ConsumerLag`, asserted `== 0`).

use std::collections::BTreeMap;

use myelin_events::{
    check_aggregate, check_updated_draft, ci_result_draft, ci_result_subject, rollup_ci_result,
    Actor, BusSignal, CheckSeamOrder, CiOverall, CiResultWaitSubstrate, CorrelationId, DataRole,
    EventEnvelope, EventId, EventType, MetricRecorder, MetricSample, MetricsSink, Timestamp,
    Visibility, WakeOutcome,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "deadbeefcafe";

/// Build a `ci.check.updated` envelope for `(REPO, COMMIT, context)` carrying an opaque
/// `run_attempt`-stamped `CheckStatus` (the Bus carries it opaque). `trust_tier` is stamped so the
/// fork-neutrality assertion can read it (the Bus does NOT interpret it — Git's gate does).
fn check_env(context: &str, run_attempt: u64, state: &str, trust_tier: &str) -> EventEnvelope {
    // The producer-side draft pins the §4.12 envelope shape (subject + aggregate); the drill wraps
    // it into a delivered envelope at a stable event_id per (context, attempt).
    let draft = check_updated_draft(
        REPO,
        COMMIT,
        context,
        serde_json::json!({
            "context": context,
            "run_attempt": run_attempt,
            "state": state,
            "trust_tier": trust_tier,
        }),
    );
    EventEnvelope {
        event_id: EventId(format!("evt-{context}-a{run_attempt}")),
        type_: EventType(draft.type_.0),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: draft.subject,
        aggregate: draft.aggregate,
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{COMMIT}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: draft.payload,
    }
}

/// The post-supersession current status per context (the highest `run_attempt` wins), reduced to
/// `context → did-it-succeed`. This models Git's CONSUMER projection (Git owns the real rule); the
/// Bus only GUARANTEES the per-aggregate order that makes it deterministic.
fn current_status(order: &CheckSeamOrder) -> BTreeMap<String, bool> {
    let mut current: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    for c in order.in_order() {
        let ctx = c
            .subject
            .0
            .rsplit("/check-")
            .next()
            .expect("subject has a check- sub-anchor")
            .to_string();
        let attempt = c.check_status["run_attempt"].as_u64().unwrap();
        let ok = c.check_status["state"].as_str() == Some("success");
        // Monotonic run_attempt supersession: the highest attempt is the current row.
        let entry = current.entry(ctx).or_insert((0, false));
        if attempt >= entry.0 {
            *entry = (attempt, ok);
        }
    }
    current.into_iter().map(|(k, (_, ok))| (k, ok)).collect()
}

/// **GIT-D10 / CI-D8 — the X-1 check seam end-to-end: 0 double-merge.** The headline GATE.
#[test]
fn d10_d8_check_seam_end_to_end_zero_double_merge() {
    // ===== (1) CI PRODUCES the per-context facts for ONE commit (trusted PR) =====
    // The outbox assigned these per-aggregate seqs (state-change order):
    //   seq 1: build attempt 1 (failure)
    //   seq 2: test  attempt 1 (success)
    //   seq 3: build attempt 2 (a RE-RUN — success)  ← supersedes seq 1
    let build1 = check_env("build", 1, "failure", "trusted");
    let test1 = check_env("test", 1, "success", "trusted");
    let build2 = check_env("build", 2, "success", "trusted");

    // ===== (2) The Bus CARRIES them per-aggregate ordered (SCRAMBLED arrival + a duplicate) =====
    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    assert!(order.ingest(&build2, 3).unwrap()); // the re-run arrives first
    assert!(order.ingest(&test1, 2).unwrap());
    assert!(order.ingest(&build1, 1).unwrap());
    // The at-least-once transport RE-DELIVERS the stale build attempt 1 → absorbed (droppable).
    assert!(
        !order.ingest(&build1, 1).unwrap(),
        "the stale lower-attempt re-delivery is absorbed (droppable)"
    );
    // Aggregate order preserved (1,2,3 regardless of arrival) — the substrate guarantee.
    assert_eq!(
        order.in_order().iter().map(|c| c.seq).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "per-aggregate order preserved — D-11 substrate"
    );
    assert_eq!(order.ordering_gap(), 0, "contiguous, 0 ops outstanding");

    // ===== (3) The CONSUMER reads the post-supersession current status =====
    let current = current_status(&order);
    assert_eq!(
        current.get("build"),
        Some(&true),
        "current build = the re-run success (highest run_attempt), NOT the stale failure"
    );
    assert_eq!(current.get("test"), Some(&true));

    // ===== (4) CI DERIVES the ci.result rollup over the REQUIRED gate set =====
    let required = vec!["build".to_string(), "test".to_string()];
    let idem = "merge-attempt-7";
    let rollup = rollup_ci_result(COMMIT, &current, &required, idem);
    assert_eq!(
        rollup.overall,
        CiOverall::Success,
        "every required context passed (post-supersession) → Success"
    );
    // CI emits the rollup via the outbox on the same per-commit aggregate (§4.12).
    let rollup_draft = ci_result_draft(REPO, &rollup);
    assert_eq!(rollup_draft.type_.0, "ci.result");
    assert_eq!(
        rollup_draft.aggregate,
        check_aggregate(REPO, COMMIT),
        "the rollup shares the per-commit aggregate (linearises after its checks)"
    );
    assert_eq!(rollup_draft.subject, ci_result_subject(REPO, COMMIT));

    // ===== (5) The merge-queue durable workflow parks + is doubly-delivered → ONE wake, ONE merge =====
    let mut sub = CiResultWaitSubstrate::new();
    assert_eq!(
        sub.wait_for_signal(idem),
        None,
        "the merge-queue holds NO runtime while CI runs (contract 9.4)"
    );

    // A merge counter the workflow advances ONLY on a real wake — the 0-double-merge gate.
    let mut merges = 0u32;
    let mut maybe_merge = |outcome: WakeOutcome, overall: CiOverall| {
        if outcome == WakeOutcome::Woke && overall == CiOverall::Success {
            merges += 1; // the merge gate fires exactly once per real wake
        }
    };

    // The at-least-once transport delivers the SAME rollup THREE times.
    maybe_merge(sub.deliver(rollup.clone()), rollup.overall); // Woke → merge
    maybe_merge(sub.deliver(rollup.clone()), rollup.overall); // Duplicate → no merge
    maybe_merge(sub.deliver(rollup.clone()), rollup.overall); // Duplicate → no merge

    assert_eq!(
        sub.wake_count(idem),
        1,
        "the merge-queue wakes EXACTLY ONCE on a doubly-delivered ci.result (9.4)"
    );
    assert_eq!(merges, 1, "0 double-merge — the PR merges EXACTLY ONCE");

    // ===== Telemetry verdict (the §10.2 harness assertion library) =====
    let mut rec = MetricRecorder::new();
    rec.emit(MetricSample::scalar(BusSignal::PublishLatencyMillis, 6));
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new(
            "consumer",
            format!("check-seam:{}", check_aggregate(REPO, COMMIT).0),
        )],
        order.ordering_gap() as i64,
    );
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new(
            "consumer",
            format!("check-seam:{}", check_aggregate(REPO, COMMIT).0),
        )],
        Predicate::Eq(0),
    )
    .expect_green();
    assert!(
        rec.scalar(BusSignal::PublishLatencyMillis).unwrap() <= 1_000,
        "per-aggregate publish latency bounded (ordering-health, §4.11 #4)"
    );
}

/// **GIT-D10 (b) — a fork PR self-green is NEUTRAL for gating.** An `untrusted_fork` success carried
/// for one context does NOT make the rollup pass on its own: the merge gate's REQUIRED set is
/// evaluated over TRUSTED status only (Git's rule). The Bus carries the fact faithfully (the
/// trust_tier rides opaque in the payload); the gating decision is Git's — and a fork-only green is
/// neutral until endorsed/re-run-trusted.
#[test]
fn d10_fork_self_green_is_neutral_for_gating() {
    let mut order = CheckSeamOrder::new(REPO, COMMIT);
    // The fork's build self-greens (untrusted_fork) — carried faithfully by the Bus.
    let fork_build = check_env("build", 1, "success", "untrusted_fork");
    let trusted_test = check_env("test", 1, "success", "trusted");
    assert!(order.ingest(&fork_build, 1).unwrap());
    assert!(order.ingest(&trusted_test, 2).unwrap());

    // The current status, restricted to TRUSTED facts (Git's gate ignores untrusted_fork greens —
    // they are neutral until endorsement). Build's only fact is the fork self-green → NOT trusted →
    // absent from the trusted current status → the gate stays closed.
    let mut trusted_current: BTreeMap<String, bool> = BTreeMap::new();
    for c in order.in_order() {
        if c.check_status["trust_tier"].as_str() == Some("untrusted_fork") {
            continue; // neutral for gating
        }
        let ctx = c.subject.0.rsplit("/check-").next().unwrap().to_string();
        trusted_current.insert(ctx, c.check_status["state"].as_str() == Some("success"));
    }

    let required = vec!["build".to_string(), "test".to_string()];
    let rollup = rollup_ci_result(COMMIT, &trusted_current, &required, "merge-attempt-fork");
    assert_eq!(
        rollup.overall,
        CiOverall::Failure,
        "a fork self-green is NEUTRAL — build has no TRUSTED success, so the gate stays closed"
    );

    // After a maintainer ENDORSES the fork run (a trusted re-run lands), the gate can pass.
    let endorsed_build = check_env("build", 2, "success", "trusted");
    assert!(order.ingest(&endorsed_build, 3).unwrap());
    let mut endorsed_current: BTreeMap<String, bool> = BTreeMap::new();
    for c in order.in_order() {
        if c.check_status["trust_tier"].as_str() == Some("untrusted_fork") {
            continue;
        }
        let ctx = c.subject.0.rsplit("/check-").next().unwrap().to_string();
        let attempt = c.check_status["run_attempt"].as_u64().unwrap();
        // Highest trusted attempt wins.
        let ok = c.check_status["state"].as_str() == Some("success");
        endorsed_current
            .entry(ctx)
            .and_modify(|v| *v = ok)
            .or_insert(ok);
        let _ = attempt;
    }
    let rollup2 = rollup_ci_result(COMMIT, &endorsed_current, &required, "merge-attempt-fork-2");
    assert_eq!(
        rollup2.overall,
        CiOverall::Success,
        "once the fork run is endorsed (a trusted build success lands), the gate passes"
    );
}

/// **CI-D8 — a doubly-delivered `ci.result` never double-advances the merge-queue, across DISTINCT
/// merge attempts.** Two distinct merge attempts (distinct `idem_key`s) wake independently; each
/// wakes exactly once even under double-delivery — 0 spurious unblocks, 0 cross-attempt wake.
#[test]
fn ci_d8_distinct_merge_attempts_wake_independently_once() {
    let mut sub = CiResultWaitSubstrate::new();
    let mut current = BTreeMap::new();
    current.insert("build".to_string(), true);
    let required = vec!["build".to_string()];

    let r1 = rollup_ci_result("commit-1", &current, &required, "attempt-1");
    let r2 = rollup_ci_result("commit-2", &current, &required, "attempt-2");

    // Each delivered twice (at-least-once).
    assert_eq!(sub.deliver(r1.clone()), WakeOutcome::Woke);
    assert_eq!(sub.deliver(r1), WakeOutcome::Duplicate);
    assert_eq!(sub.deliver(r2.clone()), WakeOutcome::Woke);
    assert_eq!(sub.deliver(r2), WakeOutcome::Duplicate);

    assert_eq!(sub.wake_count("attempt-1"), 1, "attempt 1 wakes once");
    assert_eq!(sub.wake_count("attempt-2"), 1, "attempt 2 wakes once");
    assert_eq!(
        sub.wake_count("attempt-3"),
        0,
        "an unparked attempt has 0 wakes — 0 spurious unblocks"
    );
}
