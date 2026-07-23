//! # The CDC pair for the check-seam CARRIAGE — contract 5.9 (Bus half) + 9.4 consumed (EB-24)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.9
//! (the Git↔CI `CheckStatus` seam — CI producer + Git gate; **the Bus CARRIES it**) + row 9.4
//! (the durable signal — the merge-queue `ci.result` wait, **CONSUMED**). Owning architecture:
//! `event-bus.md` §4.12 (the Bus's NARROW role). Reconciliation: `00-reconciliation-decisions.md`
//! X-1 (the most load-bearing cross-subsystem seam).
//!
//! ## What this pair pins (the Bus's NARROW half — never more)
//! The X-1 seam is OWNED by CI (producer) + Git (gate). The Bus carries two flows + one wait
//! substrate. The CDC pair pins exactly the carriage half both sides agree the Bus provides:
//!
//! **5.9 carriage — the PROVIDER (CI) ↔ CONSUMER (Git) agreement the BUS guarantees:**
//! - the PROVIDER (CI) emits `ci.check.updated` with the §4.12 envelope shape: `type` =
//!   `ci.check.updated`, `subject` = `repo#commit-<oid>/check-<context>` (the `#sub` sub-anchor),
//!   `aggregate` = `(repo, commit_oid)`, the CI-owned `CheckStatus` carried OPAQUE in `payload`;
//! - the CONSUMER (Git) receives those events PER-AGGREGATE ORDERED on `(repo, commit_oid)` +
//!   at-least-once — the ordering substrate its `run_attempt` supersession rule rests on. The Bus
//!   guarantees the order; it does NOT evaluate the rule (CI/Git own the `CheckStatus` shape, the
//!   supersession, the trust-tier gating, the merge gate).
//!
//! **9.4 consumed — the PROVIDER (CI) ↔ CONSUMER (the merge-queue workflow) signal handshake:**
//! - the PROVIDER (CI) emits the rollup `ci.result` signal `{ commit_oid, overall, contexts,
//!   idem_token }`;
//! - the CONSUMER (the merge-queue durable workflow) `wait_for_signal("ci.result", idem_key)` —
//!   and a doubly-delivered signal wakes it EXACTLY once (idempotent on `idem_key`).
//!
//! The consumer LEG (Git's `check_status` projection) lands EB-26/M3; the producer LEG (CI's real
//! emit) lands EB-27/M4 (the seam goes end-to-end there). This pins the Bus's carriage NOW.

use myelin_events::{
    check_aggregate, check_subject, check_updated_draft, validate_event_type, Actor,
    CheckSeamOrder, CiOverall, CiResult, CiResultWaitSubstrate, CorrelationId, DataRole,
    EventEnvelope, EventId, EventType, Timestamp, Visibility, WakeOutcome,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "abc123def";

/// **PROVIDER side of 5.9 (CI emits `ci.check.updated`).** CI builds the envelope draft via the
/// Bus's [`check_updated_draft`] carriage helper. The provider's promise: the `type`, `subject`
/// token grammar, and `(repo, commit_oid)` aggregate are the §4.12 shape, and the CI-owned
/// `CheckStatus` rides OPAQUE in the payload (the Bus does not name its fields).
#[test]
fn provider_ci_emits_ci_check_updated_with_the_412_envelope_shape() {
    // The CI-owned CheckStatus — OPAQUE to the Bus (CI/Git own its fields, contract 5.9).
    let check_status = serde_json::json!({
        "context": "build",
        "state": "success",
        "required": true,
        "run": "myelin://acme/ci/run/01J",
        "run_attempt": 2,
        "trust_tier": "trusted",
        "details_ref": "myelin://acme/ci/run/01J#step-3",
    });
    let draft = check_updated_draft(REPO, COMMIT, "build", check_status.clone());

    // The PROVIDER's promise: a grammar-conformant `type` (the CONSUMER validator admits it).
    assert_eq!(draft.type_.0, "ci.check.updated");
    assert!(
        validate_event_type("ci.check.updated").is_ok(),
        "the type is a §6.1 canonical name"
    );

    // The §4.12 subject token grammar: repo#commit-<oid>/check-<context> (the #sub sub-anchor).
    assert_eq!(
        draft.subject.0,
        format!("{REPO}#commit-{COMMIT}/check-build")
    );

    // The aggregate is (repo, commit_oid) — the per-commit ordering partition all contexts share.
    assert_eq!(draft.aggregate, check_aggregate(REPO, COMMIT));

    // The CI-owned CheckStatus is carried OPAQUE — every field round-trips untouched (the Bus does
    // NOT interpret it; references-not-payloads, so no inline PII).
    assert_eq!(draft.payload, check_status);
    assert!(!draft.contains_personal_data);
}

/// Build a delivered `ci.check.updated` envelope for `(REPO, COMMIT, context)` at run_attempt.
fn delivered(context: &str, run_attempt: u64, state: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt-{context}-a{run_attempt}")),
        type_: EventType("ci.check.updated".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: check_subject(REPO, COMMIT, context),
        aggregate: check_aggregate(REPO, COMMIT),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{COMMIT}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "context": context, "run_attempt": run_attempt, "state": state }),
    }
}

/// **CONSUMER side of 5.9 (Git receives per-aggregate-ordered `ci.check.updated`).** The Bus's
/// carriage delivers the per-context facts PER-AGGREGATE ORDERED on `(repo, commit_oid)` regardless
/// of physical arrival interleaving/lateness — the substrate Git's supersession rule consumes. The
/// CONSUMER agrees the order it reads is the per-aggregate `seq` order, never the arrival order.
#[test]
fn consumer_git_receives_ci_check_updated_per_aggregate_ordered() {
    let mut order = CheckSeamOrder::new(REPO, COMMIT);

    // The at-least-once transport delivers two contexts + a re-run, scrambled: seq 3, 1, 2.
    assert!(order.ingest(&delivered("build", 2, "success"), 3).unwrap()); // a re-run, arrives first
    assert!(order.ingest(&delivered("build", 1, "failure"), 1).unwrap());
    assert!(order.ingest(&delivered("test", 1, "success"), 2).unwrap());

    // The CONSUMER reads the per-aggregate seq order (1,2,3), NOT the arrival order — the Bus's
    // carriage promise. Git's supersession is well-defined over this order.
    let seqs: Vec<u64> = order.in_order().iter().map(|c| c.seq).collect();
    assert_eq!(
        seqs,
        vec![1, 2, 3],
        "per-aggregate ordered on (repo, commit_oid)"
    );
    assert_eq!(
        order.ordering_gap(),
        0,
        "contiguous: at-least-once delivered every op (0 lost)"
    );
}

/// **PROVIDER + CONSUMER of 9.4 (the merge-queue `ci.result` wait).** PROVIDER: CI emits the rollup
/// `ci.result` `{ commit_oid, overall, contexts, idem_token }`. CONSUMER: the merge-queue durable
/// workflow `wait_for_signal("ci.result", idem_key)` — a doubly-delivered signal wakes it EXACTLY
/// once (idempotent on `idem_key`, contract 9.1/9.4). The Bus provides the substrate; Git decides
/// the merge.
#[test]
fn cdc_9_4_ci_result_rollup_signal_wakes_merge_queue_exactly_once() {
    // PROVIDER (CI) — the rollup signal payload, the §4.12 / X-1 frozen shape.
    let result = CiResult {
        commit_oid: COMMIT.into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into(), "test".into()],
        idem_token: "merge-attempt-99".into(),
    };
    // The signal NAME the workflow waits on is the NAMED `ci.result` token.
    assert_eq!(CiResultWaitSubstrate::SIGNAL_NAME, "ci.result");
    assert!(
        validate_event_type("ci.result").is_ok(),
        "ci.result is a §6.1 canonical name"
    );

    // CONSUMER (the merge-queue durable workflow) — parks on wait_for_signal, holds NO runtime.
    let mut sub = CiResultWaitSubstrate::new();
    assert_eq!(
        sub.wait_for_signal("merge-attempt-99"),
        None,
        "pending while CI runs (9.4)"
    );

    // CI delivers the rollup TWICE (at-least-once) — the waiter wakes EXACTLY once.
    assert_eq!(sub.deliver(result.clone()), WakeOutcome::Woke);
    assert_eq!(sub.deliver(result.clone()), WakeOutcome::Duplicate);
    assert_eq!(
        sub.wake_count("merge-attempt-99"),
        1,
        "exactly one wake (9.1 idem on idem_key)"
    );

    // The workflow re-leases + reads the rollup; Git decides the merge off `overall` (the Bus does
    // not — it carries the signal).
    let read = sub.wait_for_signal("merge-attempt-99").unwrap();
    assert_eq!(read.overall, CiOverall::Success);
    assert_eq!(read, result);
}
