//! DB-free unit tests for the CT-004d.2 culmination driver (chunks 2/3/5). The live-PG + real-`runsc`
//! end-to-end is `tests/integration_ci_ct004d2_culmination.rs`; these prove the PURE, security-critical
//! halves without a pool: the trust-tier/region forwarding, the deterministic job-id, and the
//! verdict-vocabulary bridge reporter.

use super::*;
use myelin_flow::{JobKind, JobSpec as FlowJobSpec};
use myelin_tenancy::TenantId;

fn terms(trust: TrustTier, region: &str) -> JobScheduleTerms {
    JobScheduleTerms {
        tenant_id: "acme".into(),
        region: region.into(),
        run_id: "11111111-1111-1111-1111-111111111111".into(),
        lane: Lane::Interactive,
        labels: vec!["linux".into()],
        trust_tier: trust,
        concurrency_group: None,
        fair_key: "acme".into(),
    }
}

fn flow_spec(target: &str, idem: &str) -> FlowJobSpec {
    let mut s = FlowJobSpec::new(JobKind::Ci, target);
    s.idem_token = idem.into();
    s
}

fn ci_run_record(tenant_id: &str) -> CiRunRecord {
    CiRunRecord {
        tenant_id: tenant_id.into(),
        run_id: "11111111-1111-1111-1111-111111111111".into(),
        region: "fr-par".into(),
        project_id: "22222222-2222-2222-2222-222222222222".into(),
        pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
        wf_run_id: "44444444-4444-4444-4444-444444444444".into(),
        repo_ref: Some("repo".into()),
        commit_oid: Some("deadbeef".into()),
        cause_event_id: Some("event-1".into()),
        cause_depth: 0,
        caused_by: None,
        definition_snapshot: "blake3:abcd".into(),
        trigger_kind: "push".into(),
        concurrency_group: None,
        pr_head_generation: None,
        trust_tier: "trusted".into(),
        state: "queued".into(),
        correlation_id: "corr-1".into(),
    }
}

/// A region-wide starter must route a durable row to a driver for that exact tenant. The check runs
/// before plan registration/start, preventing the former fixed `ci-controlplane` tenant from being
/// stamped onto arbitrary queued runs.
#[test]
fn driver_refuses_a_durable_run_from_another_tenant() {
    let record = ci_run_record("tenant-b");
    let err = validate_driver_tenant(&TenantId("tenant-a".into()), &record)
        .expect_err("cross-tenant run must be refused");
    assert!(matches!(
        err,
        StartRunError::TenantMismatch {
            driver_tenant,
            record_tenant
        } if driver_tenant == "tenant-a" && record_tenant == "tenant-b"
    ));
}

#[test]
fn driver_accepts_a_durable_run_for_its_exact_tenant() {
    let record = ci_run_record("acme");
    validate_driver_tenant(&TenantId("acme".into()), &record).expect("same-tenant run is admitted");
}

/// **THE SECURITY INVARIANT (unit half): the enqueue's trust_tier + region come from the run's terms,
/// forwarded UNCHANGED, and the spec's trust_tier is STAMPED to match — a builder that returns a WIDER
/// tier is overwritten.** An `untrusted_fork` run enqueues an `untrusted_fork` stage behind an
/// `untrusted_fork` gate, no matter what the spec builder tried to set.
#[test]
fn build_dispatch_forwards_trust_and_region_unchanged_and_overwrites_a_widened_spec() {
    // The builder deliberately tries to WIDEN the tier to Trusted — dispatch must overwrite it.
    let widening_builder: StageSpecBuilder = fixed_command_spec_builder(
        "registry/x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        vec!["true".into()],
        60,
    )
    .expect("pinned image");
    let (enq, spec) = build_dispatch_parts(
        &terms(TrustTier::UntrustedFork, "de-fra"),
        &widening_builder,
        &flow_spec("pipeline://acme/ci#build", "run/ci.pipeline:0/job"),
    )
    .expect("build the dispatch");

    // trust_tier forwarded UNCHANGED from the run's terms (never the builder's Trusted).
    assert_eq!(
        enq.trust_tier,
        TrustTier::UntrustedFork,
        "enqueue tier = the run's stamped tier"
    );
    assert_eq!(
        spec.trust_tier,
        TrustTier::UntrustedFork,
        "the spec tier is STAMPED to the run's tier — the builder's widened Trusted is overwritten"
    );
    // co_persist_dispatch's enq.trust_tier == spec.trust_tier holds BY CONSTRUCTION.
    assert_eq!(
        enq.trust_tier, spec.trust_tier,
        "the claim-gating tier == the executing spec's tier"
    );
    // region forwarded UNCHANGED from the run's terms.
    assert_eq!(
        enq.region, "de-fra",
        "region = the run's residency pin, forwarded unchanged"
    );
    // the echo idem_token is stamped on the spec (the runner echoes it on job.done).
    assert_eq!(spec.idem_token, IdemToken("run/ci.pipeline:0/job".into()));
    assert_eq!(
        enq.idem_token, "run/ci.pipeline:0/job",
        "the jq_idem key = the dispatch token"
    );
}

/// A `Trusted` run forwards `Trusted` (the control that the gate is exact, not a blanket narrow).
#[test]
fn build_dispatch_forwards_a_trusted_run_as_trusted() {
    let builder = fixed_command_spec_builder(
        "registry/x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        vec!["true".into()],
        60,
    )
    .expect("pinned image");
    let (enq, spec) = build_dispatch_parts(
        &terms(TrustTier::Trusted, "fr-par"),
        &builder,
        &flow_spec("pipeline://acme/ci#build", "run/ci.pipeline:0/job"),
    )
    .unwrap();
    assert_eq!(enq.trust_tier, TrustTier::Trusted);
    assert_eq!(spec.trust_tier, TrustTier::Trusted);
    assert_eq!(enq.region, "fr-par");
}

/// The durable `job_queue.job_id` is a real uuid, deterministic on the idem_token (the `(tenant,
/// job_id)` PK idempotency anchor), and DISTINCT per stage (distinct idem_tokens → distinct rows).
#[test]
fn stage_job_id_is_a_deterministic_distinct_uuid() {
    let a = DurableJobRunner::stage_job_id("run/ci.pipeline:0/job");
    let a2 = DurableJobRunner::stage_job_id("run/ci.pipeline:0/job");
    let b = DurableJobRunner::stage_job_id("run/ci.pipeline:2/job");
    assert_eq!(
        a, a2,
        "deterministic on the idem_token (a re-dispatch re-derives the SAME job_id)"
    );
    assert_ne!(a, b, "distinct stages get distinct job_ids");
    assert!(
        sqlx::types::Uuid::parse_str(&a).is_ok(),
        "the job_id is a real uuid"
    );
}

/// The dispatched flow spec's timeout beyond the store ceiling is clamped (a legitimate stage never
/// trips the fail-closed TimeoutTooLong).
#[test]
fn build_dispatch_clamps_the_timeout_to_the_store_ceiling() {
    let builder = fixed_command_spec_builder(
        "registry/x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        vec!["true".into()],
        MAX_JOB_TIMEOUT_SECS + 10_000, // over the ceiling
    )
    .expect("pinned image");
    let (_enq, spec) = build_dispatch_parts(
        &terms(TrustTier::Trusted, "fr-par"),
        &builder,
        &flow_spec("pipeline://acme/ci#build", "run/ci.pipeline:0/job"),
    )
    .unwrap();
    assert_eq!(
        spec.limits.timeout_secs, MAX_JOB_TIMEOUT_SECS,
        "clamped to the ceiling"
    );
}

// =================================================================================================
// The durable-completion-authority verification core (DB-free — the security half is provable with no
// pool). The DB-backed signal proof + the forged-completion refusal end-to-end live in the live-PG
// integration tests.
// =================================================================================================

/// A durable dispatch identity for a stage dispatched under `idem_token` in `run_id`.
fn durable_identity(run_id: &str, idem_token: &str, stage: &str) -> ClaimedDispatchIdentity {
    ClaimedDispatchIdentity {
        run_id: run_id.into(),
        idem_token: idem_token.into(),
        stage: stage.into(),
        reserve_handle: "reserve:test".into(),
    }
}

/// **The happy path: a completion whose claimed `(tenant, run, job_id, idem_token)` ALL match the
/// durable dispatch record resolves the durable stage** — the verdict the reporter then signals.
#[test]
fn verify_admits_a_fully_matching_claim_and_returns_the_durable_stage() {
    let reporter_tenant = TenantId("acme".into());
    let idem = "44444444-4444-4444-4444-444444444444/ci.pipeline:0/job";
    let job_id = DurableJobRunner::stage_job_id(idem);
    let stage = verify_claimed_identity(
        &reporter_tenant,
        &TenantId("acme".into()),
        "44444444-4444-4444-4444-444444444444",
        &job_id,
        idem,
        Some(durable_identity(
            "44444444-4444-4444-4444-444444444444",
            idem,
            "build",
        )),
    )
    .expect("a fully-matching claim is admitted");
    assert_eq!(
        stage, "build",
        "the verdict is attributed to the DURABLE stage (a restart-safe read)"
    );
}

/// **A completion for another tenant is refused** (the reporter is tenant-bound; a region runner claims
/// cross-tenant, but a mis-routed completion never signals the wrong tenant's run).
#[test]
fn verify_refuses_a_cross_tenant_claim() {
    let idem = "R/ci.pipeline:0/job";
    let job_id = DurableJobRunner::stage_job_id(idem);
    let err = verify_claimed_identity(
        &TenantId("acme".into()),
        &TenantId("evil".into()),
        "R",
        &job_id,
        idem,
        Some(durable_identity("R", idem, "build")),
    )
    .expect_err("a cross-tenant completion is refused");
    assert!(matches!(err, ClaimRefusal::TenantMismatch { .. }));
}

/// Manifest job UUIDs are independent of Flow's idempotency token. The durable row, not a second
/// derivation scheme, binds the two identities.
#[test]
fn verify_admits_an_exact_manifest_job_id_bound_by_the_durable_record() {
    let idem = "R/ci.pipeline:0/job";
    let manifest_job_id = "aaaaaaaa-aaaa-8aaa-8aaa-aaaaaaaaaaaa";
    let stage = verify_claimed_identity(
        &TenantId("acme".into()),
        &TenantId("acme".into()),
        "R",
        manifest_job_id,
        idem,
        Some(durable_identity("R", idem, "build")),
    )
    .expect("the durable dispatch record binds an exact manifest job UUID");
    assert_eq!(stage, "build");
}

/// **A completion with NO durable dispatch record is refused** — a fabricated `job.done` for a job that
/// was never dispatched/claimed changes nothing durable.
#[test]
fn verify_refuses_a_completion_with_no_durable_dispatch_record() {
    let idem = "R/ci.pipeline:0/job";
    let job_id = DurableJobRunner::stage_job_id(idem);
    let err = verify_claimed_identity(
        &TenantId("acme".into()),
        &TenantId("acme".into()),
        "R",
        &job_id,
        idem,
        None,
    )
    .expect_err("no durable record → refused");
    assert!(matches!(err, ClaimRefusal::NoDispatchRecord { .. }));
}

/// **A completion whose durable record names a DIFFERENT run (or a different idem_token) is refused** —
/// the claim must match the exact dispatched identity.
#[test]
fn verify_refuses_a_run_or_idem_that_diverges_from_the_durable_record() {
    let idem = "R/ci.pipeline:0/job";
    let job_id = DurableJobRunner::stage_job_id(idem);
    // durable record is for a DIFFERENT run.
    let run_err = verify_claimed_identity(
        &TenantId("acme".into()),
        &TenantId("acme".into()),
        "R",
        &job_id,
        idem,
        Some(durable_identity("OTHER-RUN", idem, "build")),
    )
    .expect_err("a divergent durable run_id is refused");
    assert!(matches!(run_err, ClaimRefusal::RunMismatch { .. }));
    // durable record carries a DIFFERENT idem_token.
    let idem_err = verify_claimed_identity(
        &TenantId("acme".into()),
        &TenantId("acme".into()),
        "R",
        &job_id,
        idem,
        Some(durable_identity("R", "R/ci.pipeline:7/job", "build")),
    )
    .expect_err("a divergent durable idem_token is refused");
    assert!(matches!(idem_err, ClaimRefusal::IdemMismatch { .. }));
}

#[test]
fn completion_receipt_binds_claim_authority_stage_accounting_and_ordered_refs() {
    let tenant = TenantId("acme".into());
    let run = RunId("11111111-1111-1111-1111-111111111111".into());
    let nonce = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let refs = vec![
        ArtifactRef("myelin://acme/ci/artifact/first".into()),
        ArtifactRef("myelin://acme/ci/artifact/second".into()),
    ];
    let usage = ResourceUsage {
        cpu_seconds: 17,
        mem_byte_seconds: 4_096,
    };
    let receipt = completion_receipt(CompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        run: &run,
        job_id: "22222222-2222-2222-2222-222222222222",
        idem_token: "idem-1",
        stage: "build",
        passed: true,
        timed_out: false,
        usage,
        result_refs: &refs,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: nonce,
    });
    assert_eq!(
        receipt, "v3:0e5c53438ce1802a45aef3077bd7864a97594051e0fb383f70f1e1546c4a782d",
        "the legacy v3 encoder is byte-frozen for durable replay"
    );
    let mut reversed = refs.clone();
    reversed.reverse();
    let changed = completion_receipt(CompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        run: &run,
        job_id: "22222222-2222-2222-2222-222222222222",
        idem_token: "idem-1",
        stage: "build",
        passed: true,
        timed_out: false,
        usage,
        result_refs: &reversed,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: nonce,
    });
    assert!(receipt.starts_with("v3:"));
    assert_ne!(receipt, changed, "result-ref order is receipt authority");

    let timed_out = completion_receipt(CompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        run: &run,
        job_id: "22222222-2222-2222-2222-222222222222",
        idem_token: "idem-1",
        stage: "build",
        passed: false,
        timed_out: true,
        usage,
        result_refs: &refs,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: nonce,
    });
    assert_ne!(receipt, timed_out, "timeout status is receipt authority");

    let changed_usage = completion_receipt(CompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        run: &run,
        job_id: "22222222-2222-2222-2222-222222222222",
        idem_token: "idem-1",
        stage: "build",
        passed: true,
        timed_out: false,
        usage: ResourceUsage {
            cpu_seconds: usage.cpu_seconds + 1,
            ..usage
        },
        result_refs: &refs,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: nonce,
    });
    assert_ne!(receipt, changed_usage, "actual usage is receipt authority");
}

#[test]
fn v4_receipt_and_result_summary_bind_the_closed_disposition() {
    let tenant = TenantId("acme".into());
    let run = RunId("11111111-1111-1111-1111-111111111111".into());
    let refs = [ArtifactRef("myelin://acme/ci/artifact/first".into())];
    let input = CompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        run: &run,
        job_id: "22222222-2222-2222-2222-222222222222",
        idem_token: "idem-1",
        stage: "build",
        passed: false,
        timed_out: false,
        usage: ResourceUsage {
            cpu_seconds: 17,
            mem_byte_seconds: 4_096,
        },
        result_refs: &refs,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    };
    let failed = completion_receipts_v4(input, CiJobTerminalDisposition::WorkloadFailed);
    let cancelled = completion_receipts_v4(
        input,
        CiJobTerminalDisposition::CancelledAfterWorkloadLaunch,
    );
    assert_eq!(failed.legacy_v3, cancelled.legacy_v3);
    assert_ne!(failed.current_v4, cancelled.current_v4);
    assert_eq!(
        failed.current_v4, "v4:5a3efebb3fe3a1aebebfcb23146b871674c2b795fef9ad40c8f44a1e557a7cea",
        "the disposition-bound v4 encoder is externally pinned"
    );

    let report = TerminalReport {
        passed: false,
        timed_out: false,
        usage: input.usage,
        result_refs: refs.to_vec(),
    };
    assert_eq!(
        terminal_result_summary(&report, Some(CiJobTerminalDisposition::WorkloadFailed)),
        serde_json::json!({
            "passed": false,
            "timed_out": false,
            "disposition": "workload_failed",
            "workload_started": true,
        })
    );
    assert_eq!(
        terminal_result_summary(&report, None),
        serde_json::json!({"passed": false, "timed_out": false}),
        "a historical v3 replay retains the exact legacy summary shape"
    );
    assert_eq!(
        preparation_terminal_result_summary(
            myelin_ci_sandbox::PreparationTerminalDisposition::TimedOut {
                phase: myelin_ci_sandbox::PreparationPhase::CheckoutTransport,
            }
        ),
        serde_json::json!({
            "passed": false,
            "timed_out": true,
            "disposition": "checkout_transport_timed_out",
            "workload_started": false,
        })
    );
}

#[test]
fn immutable_pricing_projects_exact_raw_cpu_and_split_memory_costs() {
    let tenant = TenantId::from_token("acme");
    let usage = ResourceUsage {
        cpu_seconds: 17,
        mem_byte_seconds: 3 * 1_073_741_824,
    };
    let priced = PricedCiJobUsage {
        pricing_revision: "commercial:2026-07-21".into(),
        memory_gb_seconds: 3,
        cpu_wholesale: MinorUnits(11),
        cpu_markup: MinorUnits(2),
        memory_wholesale: MinorUnits(7),
        memory_markup: MinorUnits(1),
    };
    let rows = priced_cost_rows(
        &tenant,
        "11111111-1111-1111-1111-111111111111",
        "22222222-2222-2222-2222-222222222222",
        usage,
        &priced,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].meter, Meter::CpuSeconds);
    assert_eq!(rows[0].amount, usage.cpu_seconds);
    assert_eq!(rows[0].wholesale, MinorUnits(11));
    assert_eq!(rows[0].markup, MinorUnits(2));
    assert_eq!(rows[1].meter, Meter::MemGbSeconds);
    assert_eq!(rows[1].amount, 3);
    assert_eq!(rows[1].wholesale, MinorUnits(7));
    assert_eq!(rows[1].markup, MinorUnits(1));

    let mut invalid = priced;
    invalid.pricing_revision.clear();
    assert_eq!(
        priced_cost_rows(&tenant, "run", "job", usage, &invalid),
        Err(CiJobPricingError::InvalidOutput)
    );
}

#[test]
fn tier_p_reservation_structurally_requires_its_exact_operational_settlement_policy() {
    let usage = ResourceUsage {
        cpu_seconds: 17,
        mem_byte_seconds: 3 * 1_073_741_824 + 1,
    };
    let mut priced = PricedCiJobUsage {
        pricing_revision: TIER_P_OPERATIONAL_PRICING_REVISION.into(),
        memory_gb_seconds: 4,
        cpu_wholesale: MinorUnits(17),
        cpu_markup: MinorUnits::ZERO,
        memory_wholesale: MinorUnits(4),
        memory_markup: MinorUnits::ZERO,
    };
    let handle = "ci-reserve:v1:run:batch:job:item";
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Ok(())
    );

    priced.pricing_revision = "commercial:2026-07-21".into();
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Err(CiJobPricingError::InvalidOutput)
    );
    priced.pricing_revision = TIER_P_OPERATIONAL_PRICING_REVISION.into();
    priced.memory_gb_seconds = 3;
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Err(CiJobPricingError::InvalidOutput)
    );
    priced.memory_gb_seconds = 4;
    priced.cpu_wholesale = MinorUnits(16);
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Err(CiJobPricingError::InvalidOutput)
    );
    priced.cpu_wholesale = MinorUnits(17);
    priced.cpu_markup = MinorUnits(1);
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Err(CiJobPricingError::InvalidOutput)
    );
    priced.cpu_markup = MinorUnits::ZERO;
    priced.memory_wholesale = MinorUnits(3);
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Err(CiJobPricingError::InvalidOutput)
    );
    priced.memory_wholesale = MinorUnits(4);
    priced.memory_markup = MinorUnits(1);
    assert_eq!(
        validate_reservation_pricing_policy(handle, usage, &priced),
        Err(CiJobPricingError::InvalidOutput)
    );

    assert_eq!(
        validate_reservation_pricing_policy("commercial-reserve:v1:job", usage, &priced),
        Ok(()),
        "non-Tier-P reservation authorities retain their own revisioned policy"
    );
}

#[test]
fn tier_p_v2_reservation_gate_requires_the_same_exact_settlement_policy_as_v1() {
    // CT-007 slice 5b.3-4a.1b: `v2` handles are still Tier-P operational -- same zero-markup
    // pricing formula, only the reservation-amount topology differs -- so the gate must widen to
    // recognize the `v2` prefix rather than silently skip validation for `v2`-handled jobs.
    let usage = ResourceUsage {
        cpu_seconds: 17,
        mem_byte_seconds: 3 * 1_073_741_824 + 1,
    };
    let priced = PricedCiJobUsage {
        pricing_revision: TIER_P_OPERATIONAL_PRICING_REVISION.into(),
        memory_gb_seconds: 4,
        cpu_wholesale: MinorUnits(17),
        cpu_markup: MinorUnits::ZERO,
        memory_wholesale: MinorUnits(4),
        memory_markup: MinorUnits::ZERO,
    };
    let v2_handle = "ci-reserve:v2:run:budget-v1:a5:batch:job:item";
    assert_eq!(
        validate_reservation_pricing_policy(v2_handle, usage, &priced),
        Ok(())
    );

    let mut tampered = priced.clone();
    tampered.cpu_markup = MinorUnits(1);
    assert_eq!(
        validate_reservation_pricing_policy(v2_handle, usage, &tampered),
        Err(CiJobPricingError::InvalidOutput),
        "a v2 handle must still enforce zero markup, not bypass validation"
    );
}

#[test]
fn retry_attempt_accrual_is_fixed_size_and_projects_exact_usage() {
    let accrual = serde_json::json!({
        "version": 1,
        "attempts": 3,
        "cpu_seconds": 17,
        "mem_byte_seconds": 23,
        "last": {
            "lease_epoch": 3,
            "claim_nonce": "11111111-1111-1111-1111-111111111111",
            "lease_owner": "runner-1",
            "cause": "output_persistence",
            "cpu_seconds": 5,
            "mem_byte_seconds": 7,
            "receipt": format!("retry-v1:{}", "a".repeat(64)),
        }
    });
    assert_eq!(
        decode_retry_attempt_usage(accrual),
        Ok(Some(ResourceUsage {
            cpu_seconds: 17,
            mem_byte_seconds: 23,
        }))
    );
    assert_eq!(decode_retry_attempt_usage(serde_json::json!({})), Ok(None));
    let corrupt = decode_retry_attempts(serde_json::json!({
        "version": 1,
        "attempts": 4,
        "cpu_seconds": 17,
        "mem_byte_seconds": 23,
        "last": {
            "lease_epoch": 3,
            "claim_nonce": "11111111-1111-1111-1111-111111111111",
            "lease_owner": "runner-1",
            "cause": "output_persistence",
            "cpu_seconds": 5,
            "mem_byte_seconds": 7,
            "receipt": format!("retry-v1:{}", "a".repeat(64)),
        }
    }));
    assert!(
        matches!(corrupt, Err(CompletionTxError::RetryCorrupt)),
        "an impossible attempt count is corrupt state, never zero usage"
    );
}

/// The `sandbox_infrastructure` cause (added alongside `RetryableAttemptCause::
/// SandboxInfrastructure`, the CT-007 gVisor launch-failure fix) must decode exactly like
/// `output_persistence` — proving `decode_retry_attempts`'s validation was generalized from a
/// single hardcoded literal to `RetryableAttemptCause::from_storage_token(...).is_some()` (Sol's
/// review caught the original hardcoded-cause bug at the persist site; this proves the read-side
/// validation was fixed too, not just the write side). An unrecognized cause token must still be
/// rejected as corrupt, never silently accepted.
#[test]
fn retry_attempt_accrual_accepts_sandbox_infrastructure_and_rejects_an_unknown_cause() {
    let accrual_with = |cause: &str| {
        serde_json::json!({
            "version": 1,
            "attempts": 1,
            "cpu_seconds": 9,
            "mem_byte_seconds": 900,
            "last": {
                "lease_epoch": 1,
                "claim_nonce": "22222222-2222-2222-2222-222222222222",
                "lease_owner": "runner-2",
                "cause": cause,
                "cpu_seconds": 9,
                "mem_byte_seconds": 900,
                "receipt": format!("retry-v1:{}", "b".repeat(64)),
            }
        })
    };
    assert_eq!(
        decode_retry_attempt_usage(accrual_with("sandbox_infrastructure")),
        Ok(Some(ResourceUsage {
            cpu_seconds: 9,
            mem_byte_seconds: 900,
        })),
        "sandbox_infrastructure must decode exactly like output_persistence"
    );
    assert!(
        matches!(
            decode_retry_attempts(accrual_with("some_future_cause_this_binary_does_not_know")),
            Err(CompletionTxError::RetryCorrupt)
        ),
        "an unrecognized cause token must be rejected as corrupt, never silently accepted"
    );
}

/// The write-side construction Sol's review caught hardcoding `OUTPUT_PERSISTENCE_CAUSE`
/// regardless of `failure.cause` — this test would FAIL if that regressed, unlike the
/// decode-only tests above (which would still pass against a hardcoded write side, since decoding
/// never sees what the write side chose not to write). Proves `SandboxInfrastructure` produces
/// `cause == "sandbox_infrastructure"` in the actual persisted record, and that its receipt
/// genuinely differs from `OutputPersistence`'s for the identical claim/usage — i.e. the receipt
/// hash really binds to the cause, not just to fields a hardcoded cause would leave unchanged.
#[test]
fn expected_retry_attempt_record_binds_the_actual_cause_not_a_hardcoded_one() {
    let claim = CompletionClaim {
        tenant: TenantId("acme".into()),
        run: RunId("33333333-3333-3333-3333-333333333333".into()),
        job_id: "job-cause-binding".into(),
        idem_token: "idem-cause-binding".into(),
        lease_owner: "runner-3".into(),
        lease_epoch: 1,
        claim_nonce: "44444444-4444-4444-4444-444444444444".into(),
    };
    let usage = ResourceUsage {
        cpu_seconds: 5,
        mem_byte_seconds: 500,
    };
    let sandbox_infra = expected_retry_attempt_record(
        &claim,
        "fr-par",
        &RetryableAttemptFailure {
            cause: RetryableAttemptCause::SandboxInfrastructure,
            usage,
        },
    );
    assert_eq!(sandbox_infra.cause, "sandbox_infrastructure");

    let output_persistence = expected_retry_attempt_record(
        &claim,
        "fr-par",
        &RetryableAttemptFailure {
            cause: RetryableAttemptCause::OutputPersistence,
            usage,
        },
    );
    assert_eq!(output_persistence.cause, "output_persistence");
    assert_ne!(
        sandbox_infra.receipt, output_persistence.receipt,
        "the receipt must actually bind to the cause — a reverted hardcoded-cause bug would make \
         every cause produce the SAME receipt for identical claim/usage"
    );
}

/// The canonical cause -> storage-token mapping is the ONE place this is defined (Sol's review
/// caught a pre-existing bug where the persist site hardcoded `OUTPUT_PERSISTENCE_CAUSE`
/// regardless of the actual `failure.cause`, silently mislabeling every non-output-persistence
/// row). Round-tripping both known causes through `as_storage_token`/`from_storage_token` proves
/// the mapping is bijective and an unknown token maps to `None`, never a default guess.
#[test]
fn retryable_attempt_cause_storage_token_round_trips_and_rejects_unknown_tokens() {
    for cause in [
        RetryableAttemptCause::OutputPersistence,
        RetryableAttemptCause::SandboxInfrastructure,
    ] {
        let token = cause.as_storage_token();
        assert_eq!(
            RetryableAttemptCause::from_storage_token(token),
            Some(cause),
            "cause {cause:?} must round-trip through its own storage token"
        );
    }
    assert_eq!(
        RetryableAttemptCause::from_storage_token("not_a_real_cause"),
        None,
        "an unrecognized token must map to None, never silently coerce to an existing cause"
    );
}

#[test]
fn terminal_usage_aggregation_refuses_u64_overflow_and_bigint_overflow() {
    assert_eq!(
        checked_add_accounting_usage(
            ResourceUsage {
                cpu_seconds: u64::MAX,
                mem_byte_seconds: 0,
            },
            ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 0,
            },
        ),
        Err(CiUsageAggregationError::Overflow),
    );
    assert_eq!(
        checked_add_accounting_usage(
            ResourceUsage {
                cpu_seconds: i64::MAX as u64,
                mem_byte_seconds: 0,
            },
            ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 0,
            },
        ),
        Err(CiUsageAggregationError::DurableRange),
    );
    assert_eq!(
        checked_accounting_usage(ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: i64::MAX as u64 + 1,
        }),
        Err(CiUsageAggregationError::DurableRange),
    );
}

#[test]
fn preparation_completion_receipts_are_disposition_bound_and_externally_pinned() {
    let tenant = TenantId("receipt-tenant".into());
    let input = PreparationCompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        wf_run_id: "11111111-1111-4111-8111-111111111111",
        ci_run_id: "22222222-2222-4222-8222-222222222222",
        job_id: "33333333-3333-4333-8333-333333333333",
        idem_token: "receipt-idem",
        stage: "build",
        reserve_handle: "ci-reserve:v2:fixed",
        usage: ResourceUsage {
            cpu_seconds: 17,
            mem_byte_seconds: 23,
        },
        lease_owner: "runner-fixed",
        lease_epoch: 7,
        claim_nonce: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        claim_started_at_epoch_secs: 1_785_000_000,
        claim_expires_at_epoch_secs: 1_785_000_300,
    };
    let transport = preparation_completion_receipts(
        input,
        PreparationTerminalDisposition::Failed {
            phase: myelin_ci_sandbox::PreparationPhase::CheckoutTransport,
        },
    );
    let materialization = preparation_completion_receipts(
        input,
        PreparationTerminalDisposition::Failed {
            phase: myelin_ci_sandbox::PreparationPhase::CheckoutMaterialization,
        },
    );
    let timed_out = preparation_completion_receipts(
        input,
        PreparationTerminalDisposition::TimedOut {
            phase: myelin_ci_sandbox::PreparationPhase::CheckoutTransport,
        },
    );
    let exhausted =
        preparation_completion_receipts(input, PreparationTerminalDisposition::AttemptsExhausted);
    assert_eq!(transport.legacy_v3, materialization.legacy_v3);
    assert_eq!(transport.legacy_v3, timed_out.legacy_v3);
    assert_eq!(transport.legacy_v3, exhausted.legacy_v3);
    assert_ne!(transport.current_v4, materialization.current_v4);
    assert_ne!(transport.current_v4, timed_out.current_v4);
    assert_ne!(transport.current_v4, exhausted.current_v4);
    assert_eq!(
        transport.legacy_v3, "v3:3c960f83b2b4ce366ee2cd2c939f91b1c33b12ee581a519aa4f841de32ac85ad",
        "the preparation v3 compatibility digest is externally pinned"
    );
    assert_eq!(
        transport.current_v4, "v4:78b417e2c0371548954f7f0a9b79acebe4143f18a036cea3827394ebd2393295",
        "the preparation disposition-bound receipt is externally pinned"
    );
}

/// **CT-007 slice 5b.3-6d STEP 4: the twelve-field `CiJobTokenRequest` ↔ `PreparationReportClaim`
/// mapping is EXACT (no drop, no reorder).** A canonical request with a DISTINCT value in every field
/// round-trips through the admission-side projection (`preparation_report_claim`) and the reporter-side
/// projection (`token_request_from_preparation_report_claim`) byte-identically — so a claim minted at
/// admission reaches the durable preparation CAS unchanged. Distinct per-field values mean a swapped or
/// dropped field would break equality.
#[test]
fn preparation_report_claim_round_trips_all_twelve_token_request_fields() {
    let request = CiJobTokenRequest {
        tenant_id: "tenant-distinct".into(),
        region: "region-distinct".into(),
        wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
        ci_run_id: "22222222-2222-2222-2222-222222222222".into(),
        job_id: "33333333-3333-3333-3333-333333333333".into(),
        token_authority_handle: "tah-distinct".into(),
        idem_token: "idem-distinct".into(),
        lease_owner: "owner-distinct".into(),
        lease_epoch: 7,
        claim_nonce: "44444444-4444-4444-4444-444444444444".into(),
        claim_started_at_epoch_secs: 101,
        claim_expires_at_epoch_secs: 404,
    };
    // Admission-side projection carries each of the twelve fields UNCHANGED.
    let report_claim = crate::ci_checkout_composition::preparation_report_claim(&request);
    assert_eq!(report_claim.tenant_id, request.tenant_id);
    assert_eq!(report_claim.region, request.region);
    assert_eq!(report_claim.wf_run_id, request.wf_run_id);
    assert_eq!(report_claim.ci_run_id, request.ci_run_id);
    assert_eq!(report_claim.job_id, request.job_id);
    assert_eq!(report_claim.token_authority_handle, request.token_authority_handle);
    assert_eq!(report_claim.idem_token, request.idem_token);
    assert_eq!(report_claim.lease_owner, request.lease_owner);
    assert_eq!(report_claim.lease_epoch, request.lease_epoch);
    assert_eq!(report_claim.claim_nonce, request.claim_nonce);
    assert_eq!(report_claim.claim_started_at_epoch_secs, request.claim_started_at_epoch_secs);
    assert_eq!(report_claim.claim_expires_at_epoch_secs, request.claim_expires_at_epoch_secs);
    // Reporter-side projection reconstructs the byte-identical request (round-trip).
    let back = token_request_from_preparation_report_claim(&report_claim);
    assert_eq!(back, request, "the twelve-field mapping is an exact round-trip");
}
