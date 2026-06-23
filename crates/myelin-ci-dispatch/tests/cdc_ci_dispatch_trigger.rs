//! **CDC — the CI Trigger & Dispatch CONSUMER side of the trigger seam (CI-P10 / P-353, M4).**
//!
//! The contract-coverage scanner requires every covered row to name a CDC file carrying BOTH a
//! provider-side and a consumer-side marker. This file is the **CONSUMER** half for the three rows
//! CI Trigger & Dispatch CONSUMES:
//!   - **3.4** — the `EventMatcher` = the frozen `QueryAst`. PROVIDER: `myelin-query` (the one
//!     bounded predicate engine + the matcher). CONSUMER: CI dispatch — an `on: pull_request`
//!     trigger compiles to that one `EventMatcher` over the projected envelope (NOT a CI DSL, NOT
//!     CEL), and the matcher's permission compose is leak-free by construction.
//!   - **2.5** — the `consumer_dedup` ledger (exactly-once effect). PROVIDER: `myelin-events` (the
//!     platform consumer template + the ledger schema, CI-P6). CONSUMER: CI dispatch — the
//!     `(consumer, event_id)` first-write-wins guard makes one push = exactly one run under
//!     at-least-once redelivery.
//!   - **4.9** — the `read & !is_untrusted_fork` ABAC edge (the trust classification). PROVIDER:
//!     `myelin-identity-service` (the compiled `run` fragment's `read = view − is_untrusted_fork`
//!     Exclusion). CONSUMER: CI dispatch — it reads the edge's verdict into the trust-tier
//!     classification and stamps the SAME tier onto BOTH `JobSpec.trust_tier` AND
//!     `CheckStatus.trust_tier` (X-1, 0 divergence).
//!
//! These are the PUBLIC-surface, deterministic consumer assertions over the frozen shapes; the
//! provider-side CDCs (`cdc_3_4_*`, `cdc_2_5_consumer_dedup`, `cdc_4_9_ci_fragment`) prove the other
//! half. No DB, no network — the dedup ledger's live forward-only apply is CI-P6's integration test.

use myelin_ci_dispatch::dispatch::{
    classify_trust, compile_trigger, stamp_trust, trigger_matches, DedupLedger, OnTrigger,
    RunProvenance, TrustTier, RUN_OBJECT_TYPE, TRIGGER_CONSUMER,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_git::check_status::TrustTier as GitTrustTier;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_tenancy::{Region, TenantId};

fn envelope(type_: &str, run_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev-{run_id}")),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("t1".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )),
        subject: ArtifactRef(format!("myelin://t1/ci/run/{run_id}")),
        aggregate: AggregateKey("agg".into()),
        causation_id: None,
        correlation_id: CorrelationId("corr".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **3.4 CONSUMER — the trigger compiles to the ONE `EventMatcher` over the `run` object type and
/// fires on the right events.** The PROVIDER (`myelin-query`) owns the matcher engine; the CONSUMER
/// (CI dispatch) compiles `on: pull_request` to it and gets a leak-free, type-pinned match.
#[test]
fn cdc_3_4_consumer_trigger_is_the_one_event_matcher() {
    let m = compile_trigger(&OnTrigger::PullRequest).expect("compiles to the frozen QueryAst");
    assert_eq!(m.object_type().0, RUN_OBJECT_TYPE, "selects the run object");
    assert!(
        trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_PR_OPENED, "r1"),
            &SetExpr::All,
            &|_| false,
        )
        .unwrap(),
        "the consumer's pull_request trigger fires on git.pr.opened"
    );
    assert!(
        !trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
            &SetExpr::All,
            &|_| false,
        )
        .unwrap(),
        "and NOT on a push (the matcher discriminates by event.type)"
    );
}

/// **2.5 CONSUMER — exactly-once effect: deliver the SAME `event_id` twice → one effect.** The
/// PROVIDER (`myelin-events` + CI-P6's ledger schema) owns the `(consumer, event_id)` template; the
/// CONSUMER (CI dispatch) records under [`TRIGGER_CONSUMER`] and absorbs the redelivery.
#[test]
fn cdc_2_5_consumer_dedup_yields_exactly_one_run() {
    let mut ledger = DedupLedger::new();
    let ev = EventId("ev-push-1".into());
    let mut runs = 0u32;
    for _ in 0..3 {
        if ledger.record(TRIGGER_CONSUMER, &ev) {
            runs += 1;
        }
    }
    assert_eq!(
        runs, 1,
        "one push = exactly one run under at-least-once delivery"
    );
    assert_eq!(ledger.effect_count(), 1, "0 duplicate runs");
}

/// **4.9 CONSUMER — the `read & !is_untrusted_fork` verdict drives the trust classification, stamped
/// consistently onto both halves (X-1).** The PROVIDER (`myelin-identity-service`'s compiled `run`
/// fragment) owns the `read = view − is_untrusted_fork` Exclusion; the CONSUMER (CI dispatch) reads
/// its verdict (`read_excludes_fork`) into [`classify_trust`] and stamps the SAME tier onto the
/// `JobSpec` AND the `CheckStatus` (0 divergence).
#[test]
fn cdc_4_9_consumer_trust_tier_stamped_consistently() {
    // A fork run the edge does NOT admit → UntrustedFork on BOTH halves.
    let fork = RunProvenance {
        is_fork: true,
        targets_self_hosted: false,
        read_excludes_fork: false,
    };
    assert_eq!(classify_trust(&fork), TrustTier::UntrustedFork);
    let stamp = stamp_trust(&fork);
    assert_eq!(stamp.job_tier, TrustTier::UntrustedFork, "JobSpec tier");
    assert_eq!(
        stamp.check_tier,
        GitTrustTier::UntrustedFork,
        "CheckStatus tier — the SAME value (X-1, 0 divergence)"
    );
    assert!(stamp.is_consistent());

    // A member run the edge admits → Trusted on the JobSpec, git-Trusted on the CheckStatus.
    let member = RunProvenance {
        is_fork: false,
        targets_self_hosted: false,
        read_excludes_fork: true,
    };
    let stamp = stamp_trust(&member);
    assert_eq!(stamp.job_tier, TrustTier::Trusted);
    assert_eq!(stamp.check_tier, GitTrustTier::Trusted);
    assert!(stamp.is_consistent());
}
