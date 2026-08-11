use std::collections::BTreeMap;

use myelin_ci_sandbox::hardening::HardeningProfile;
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ResourceLimits,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_flow::SignalStore;
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId as LedgerRunId};
use myelin_tenancy::{Region, TenantId};

use crate::check_emitter::{
    assemble_check_status, details_ref, CheckEmitContext, CheckProvider, CheckState, CostPosture,
    TrustTier as CheckTrustTier,
};
use crate::ci_pipeline::structured_failure;
use crate::ci_result_signal::{CiResultSignal, RollupDelivery};
use crate::e2e_wedge::E2eArtifact;

pub const E2E_FLAGSHIP_SCENARIO: &str = "E2E-2";

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn region() -> Region {
    Region("fr-par".into())
}

const REPO: &str = "myelin://acme/git/repo/payments";

const FAIL_COMMIT: &str = "f00dcafe";

const FIX_COMMIT: &str = "900dbeef";

const FAIL_RUN: &str = "myelin://acme/ci/run/run-payments-fail";

const FIX_RUN: &str = "myelin://acme/ci/run/run-payments-fix";

const CONTEXT: &str = "build-and-test";

const FAILED_STAGE: &str = "test";
const FAILED_STEP: u32 = 3;
const FAILED_TEST: &str = "myelin_payments::charge::test_refund_idempotent";

const MERGE_IDEM_TOKEN: &str = "merge-attempt:payments:42";

const MERGE_QUEUE_RUN: &str = "run:merge-queue:payments";

fn triage_agent_job() -> JobSpec {
    JobSpec::new(
        JobKind::Agent,
        ImageRef::pinned(
            "registry.myelin.internal/agent-runtime@sha256:\
             0000000000000000000000000000000000000000000000000000000000000001",
        )
        .expect("the agent runtime image is digest-pinned (CI-1, fail-closed)"),
        vec![
            "/usr/bin/triage-agent".to_string(),
            "--use-mock".to_string(),
        ],
        vec![],
        vec![],
        EgressPolicy { allow: vec![] },
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 256 * 1024 * 1024,
            tmpfs_bytes: 256 * 1024 * 1024,
            pids_max: 256,
            timeout_secs: 300,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("triage-agent-bearer", "jti:triage-agent:run-triage", 300)
            .expect("static flagship credential is valid"),
        MeterTarget {
            reserve_id: "reserve:run-triage".to_string(),
        },
        IdemToken("idem:triage-agent:run-triage".to_string()),
    )
    .expect("the triage agent job is a valid hardened JobSpec (digest-pinned, pids_max, timeout)")
}

pub fn run_e2e2_ci_flagship_slice() -> E2eArtifact {
    let mut leaks: u64 = 0;

    let log_excerpt_ref = format!("{FAIL_RUN}#log/step-{FAILED_STEP}");
    let failure = structured_failure(
        FAILED_STAGE,
        Some(FAILED_STEP),
        Some(FAILED_TEST),
        Some(&log_excerpt_ref),
    );
    let failure_payload = failure.to_payload();
    let triage_hook_structured = failure_payload.get("failed_stage").and_then(|v| v.as_str())
        == Some(FAILED_STAGE)
        && failure_payload.get("failed_step").and_then(|v| v.as_u64()) == Some(FAILED_STEP as u64)
        && failure_payload.get("failed_test").and_then(|v| v.as_str()) == Some(FAILED_TEST)
        && failure_payload
            .get("log_excerpt_ref")
            .and_then(|v| v.as_str())
            == Some(log_excerpt_ref.as_str());
    if !triage_hook_structured {
        leaks += 1;
    }
    let fail_ctx = CheckEmitContext {
        tenant: tenant().0,
        repo: REPO.to_string(),
        commit_oid: FAIL_COMMIT.to_string(),
        run_ref: FAIL_RUN.to_string(),
        run_attempt: 1,
        trust_tier: CheckTrustTier::Trusted,
        started_at: "2026-06-25T00:00:00Z".to_string(),
        completed_at: Some("2026-06-25T00:01:00Z".to_string()),
    };
    let fail_check = assemble_check_status(
        &fail_ctx,
        CheckProvider::Ci,
        CONTEXT,
        CheckState::Failure,
        true,
        CostPosture::Settled,
        Some(FAILED_STEP),
    );
    let expected_anchor = details_ref(FAIL_RUN, CheckState::Failure, Some(FAILED_STEP));
    let fail_check_state =
        fail_check.payload.get("state").and_then(|v| v.as_str()) == Some("failure");
    let anchor_resolves =
        expected_anchor.ends_with(&format!("#step-{FAILED_STEP}")) && fail_check_state;
    if !anchor_resolves {
        leaks += 1;
    }
    let no_log_bytes_on_bus = failure_payload
        .get("log_excerpt_ref")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.starts_with("myelin://") && !s.contains('\n'));
    if !no_log_bytes_on_bus {
        leaks += 1;
    }

    let agent_job = triage_agent_job();
    let runner_is_agent_kind = agent_job.kind == JobKind::Agent;
    let profile = HardeningProfile::derive(&agent_job);
    let ag_d4_gated = profile.assert_enforced().is_ok()
        && profile.egress_default_deny
        && !profile.network_device
        && profile.drop_all_caps
        && profile.no_new_privileges
        && profile.seccomp
        && profile.read_only_root
        && profile.ephemeral_one_job
        && profile.pids_max > 0;
    if !runner_is_agent_kind || !ag_d4_gated {
        leaks += 1;
    }

    let fix_ctx = CheckEmitContext {
        tenant: tenant().0,
        repo: REPO.to_string(),
        commit_oid: FIX_COMMIT.to_string(),
        run_ref: FIX_RUN.to_string(),
        run_attempt: 2,
        trust_tier: CheckTrustTier::Trusted,
        started_at: "2026-06-25T00:00:00Z".to_string(),
        completed_at: Some("2026-06-25T00:02:00Z".to_string()),
    };
    let green_check = assemble_check_status(
        &fix_ctx,
        CheckProvider::Ci,
        CONTEXT,
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );
    let fix_greens = green_check
        .payload
        .get("state")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == "success");
    if !fix_greens {
        leaks += 1;
    }

    let signals = SignalStore::new();
    let rollup = CiResultSignal::new(&signals, tenant(), region(), MERGE_QUEUE_RUN);
    let current: BTreeMap<String, bool> = [(CONTEXT.to_string(), true)].into_iter().collect();
    let required = vec![CONTEXT.to_string()];
    let first = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
    let duplicate = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
    let merge_wakes_exactly_once =
        first == RollupDelivery::Woke && duplicate == RollupDelivery::Duplicate;
    if !merge_wakes_exactly_once {
        leaks += 1;
    }
    let rollup_verdict = rollup.rollup(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
    let rollup_is_success = CiResultSignal::is_success(&rollup_verdict);
    if !rollup_is_success {
        leaks += 1;
    }
    let merge_count = u64::from(
        signals
            .get(
                &tenant(),
                MERGE_QUEUE_RUN,
                myelin_flow::CI_RESULT_SIGNAL,
                MERGE_IDEM_TOKEN,
            )
            .is_some(),
    );
    if merge_count != 1 {
        leaks += 1;
    }

    let mut ledger = CostLedger::new();
    let run = LedgerRunId::new(FIX_RUN);
    const ESTIMATE: u64 = 20;
    const WALLET: u64 = 100;
    let reservation = ledger
        .reserve(tenant(), run.clone(), MicroUsd(ESTIMATE), MicroUsd(WALLET))
        .expect("a funded wallet reserves the CI run at dispatch (no balance → no run)");
    let reserved_estimate = reservation.reserved == MicroUsd(ESTIMATE);
    ledger
        .begin(&tenant(), &run)
        .expect("the reserved run begins flight (the reservation's only exit is settle)");
    let units = vec![
        MeteredUnit {
            unit: "ci.cpu_second",
            wholesale: MicroUsd(8),
            markup: MicroUsd(2),
        },
        MeteredUnit {
            unit: "ci.artifact_byte",
            wholesale: MicroUsd(3),
            markup: MicroUsd(1),
        },
    ];
    let settle = ledger
        .settle(&tenant(), &run, &units)
        .expect("the in-flight CI run settles on completion");
    let billed = settle.billed_total.0;
    let refunded = settle.refunded.0;
    let reserve_settle_balanced = reserved_estimate
        && billed == 14
        && billed + refunded == ESTIMATE
        && ledger
            .cost_events_for(&tenant(), &run)
            .is_ok_and(|events| events.len() == units.len())
        && ledger.inflight_interrupt_count() == 0;
    if !reserve_settle_balanced {
        leaks += 1;
    }

    let green = triage_hook_structured
        && anchor_resolves
        && no_log_bytes_on_bus
        && runner_is_agent_kind
        && ag_d4_gated
        && fix_greens
        && merge_wakes_exactly_once
        && rollup_is_success
        && merge_count == 1
        && reserve_settle_balanced;
    E2eArtifact::sealed(
        E2E_FLAGSHIP_SCENARIO,
        green,
        leaks,
        format!(
            "CI-fail→triage→issue→chat→fix-PR (CI slice): structured ci.run.failed \
             (stage={FAILED_STAGE},step={FAILED_STEP},test={FAILED_TEST},log-ref) \
             structured={triage_hook_structured}; triage-agent compute AG-D4-gated \
             (kind=Agent,no-NIC,caps-dropped,seccomp)={ag_d4_gated}; fix-PR CI greens={fix_greens}; \
             merge-queue wakes EXACTLY ONCE on ci.result (dup absorbed)={merge_wakes_exactly_once}, \
             merge-count={merge_count}; reserve/settle balanced (reserved {ESTIMATE} == billed \
             {billed} + refunded {refunded})={reserve_settle_balanced}"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e2_ci_flagship_slice_green_end_to_end() {
        let art = run_e2e2_ci_flagship_slice();
        assert_eq!(art.scenario, "E2E-2");
        assert_eq!(
            art.leaks, 0,
            "0 leak/double-merge across CI's flagship slice: {art:?}"
        );
        assert!(art.is_green(), "E2E-2 (CI slice) green not earned: {art:?}");
        assert!(art.seal.starts_with("blake3:"));
    }

    #[test]
    fn structured_ci_run_failed_carries_step_test_and_log_ref() {
        let log_ref = format!("{FAIL_RUN}#log/step-{FAILED_STEP}");
        let failure = structured_failure(
            FAILED_STAGE,
            Some(FAILED_STEP),
            Some(FAILED_TEST),
            Some(&log_ref),
        );
        let p = failure.to_payload();
        assert_eq!(
            p.get("failed_stage").and_then(|v| v.as_str()),
            Some(FAILED_STAGE)
        );
        assert_eq!(
            p.get("failed_step").and_then(|v| v.as_u64()),
            Some(FAILED_STEP as u64)
        );
        assert_eq!(
            p.get("failed_test").and_then(|v| v.as_str()),
            Some(FAILED_TEST)
        );
        assert_eq!(
            p.get("log_excerpt_ref").and_then(|v| v.as_str()),
            Some(log_ref.as_str())
        );
        assert!(log_ref.starts_with("myelin://") && !log_ref.contains('\n'));
        let bare = structured_failure(FAILED_STAGE, None, None, None).to_payload();
        assert_eq!(bare.as_object().map(|o| o.len()), Some(1));
        assert!(
            bare.get("failed_step").is_none(),
            "absent detail is an absent key, not null"
        );
    }

    #[test]
    fn triage_agent_compute_is_ag_d4_gated() {
        let job = triage_agent_job();
        assert_eq!(
            job.kind,
            JobKind::Agent,
            "the agent compute is a kind=Agent job"
        );
        let profile = HardeningProfile::derive(&job);
        assert!(
            profile.assert_enforced().is_ok(),
            "the agent job's hardening profile is fully enforced: {:?}",
            profile.assert_enforced()
        );
        assert!(
            !profile.network_device,
            "the agent guest gets no NIC (egress closed at the device)"
        );
        assert!(profile.drop_all_caps && profile.no_new_privileges && profile.seccomp);
        assert!(
            profile.ephemeral_one_job,
            "one-job-per-sandbox, never reused"
        );
    }

    #[test]
    fn merge_queue_wakes_exactly_once_on_ci_result() {
        let signals = SignalStore::new();
        let rollup = CiResultSignal::new(&signals, tenant(), region(), MERGE_QUEUE_RUN);
        let current: BTreeMap<String, bool> = [(CONTEXT.to_string(), true)].into_iter().collect();
        let required = vec![CONTEXT.to_string()];
        let first = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
        let dup = rollup.signal_ci_result(FIX_COMMIT, &current, &required, MERGE_IDEM_TOKEN);
        assert_eq!(
            first,
            RollupDelivery::Woke,
            "the first delivery wakes the merge queue"
        );
        assert_eq!(
            dup,
            RollupDelivery::Duplicate,
            "the duplicate is absorbed (0 double-merge)"
        );
        assert!(signals
            .get(
                &tenant(),
                MERGE_QUEUE_RUN,
                myelin_flow::CI_RESULT_SIGNAL,
                MERGE_IDEM_TOKEN
            )
            .is_some());
    }

    #[test]
    fn reserve_settle_balanced_in_isolation() {
        let mut ledger = CostLedger::new();
        let run = LedgerRunId::new(FIX_RUN);
        let r = ledger
            .reserve(tenant(), run.clone(), MicroUsd(20), MicroUsd(100))
            .unwrap();
        assert_eq!(r.reserved, MicroUsd(20));
        ledger.begin(&tenant(), &run).unwrap();
        let units = vec![
            MeteredUnit {
                unit: "ci.cpu_second",
                wholesale: MicroUsd(8),
                markup: MicroUsd(2),
            },
            MeteredUnit {
                unit: "ci.artifact_byte",
                wholesale: MicroUsd(3),
                markup: MicroUsd(1),
            },
        ];
        let s = ledger.settle(&tenant(), &run, &units).unwrap();
        assert_eq!(s.billed_total.0, 14);
        assert_eq!(
            s.billed_total.0 + s.refunded.0,
            20,
            "reserved == billed + refunded"
        );
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run).unwrap().len(),
            2,
            "one cost event per unit"
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "0 in-flight interrupt"
        );
    }
}
