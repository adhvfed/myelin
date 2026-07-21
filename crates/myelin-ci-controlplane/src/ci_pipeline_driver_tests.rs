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
        definition_snapshot: "blake3:abcd".into(),
        trigger_kind: "push".into(),
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
        &job_id,
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
        &job_id,
        Some(durable_identity("R", idem, "build")),
    )
    .expect_err("a cross-tenant completion is refused");
    assert!(matches!(err, ClaimRefusal::TenantMismatch { .. }));
}

/// **A completion whose claimed `job_id` is not the deterministic dispatch id for its `idem_token` is
/// refused** — the predictable idem token is no longer a free pass; it must agree with the claimed row.
#[test]
fn verify_refuses_a_job_id_that_is_not_the_dispatch_id_for_the_idem_token() {
    let idem = "R/ci.pipeline:0/job";
    let expected = DurableJobRunner::stage_job_id(idem);
    let forged_job_id = DurableJobRunner::stage_job_id("R/ci.pipeline:9/job");
    let err = verify_claimed_identity(
        &TenantId("acme".into()),
        &TenantId("acme".into()),
        "R",
        &forged_job_id,
        idem,
        &expected,
        Some(durable_identity("R", idem, "build")),
    )
    .expect_err("a job_id that is not stage_job_id(idem_token) is refused");
    assert!(matches!(err, ClaimRefusal::JobIdMismatch { .. }));
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
        &job_id,
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
        &job_id,
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
        &job_id,
        Some(durable_identity("R", "R/ci.pipeline:7/job", "build")),
    )
    .expect_err("a divergent durable idem_token is refused");
    assert!(matches!(idem_err, ClaimRefusal::IdemMismatch { .. }));
}

#[test]
fn completion_receipt_binds_claim_authority_stage_and_ordered_refs() {
    let tenant = TenantId("acme".into());
    let run = RunId("11111111-1111-1111-1111-111111111111".into());
    let nonce = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let refs = vec![
        ArtifactRef("myelin://acme/ci/artifact/first".into()),
        ArtifactRef("myelin://acme/ci/artifact/second".into()),
    ];
    let receipt = completion_receipt(CompletionReceiptInput {
        tenant: &tenant,
        region: "fr-par",
        run: &run,
        job_id: "22222222-2222-2222-2222-222222222222",
        idem_token: "idem-1",
        stage: "build",
        passed: true,
        result_refs: &refs,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: nonce,
    });
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
        result_refs: &reversed,
        lease_owner: "worker-1",
        lease_epoch: 7,
        claim_nonce: nonce,
    });
    assert!(receipt.starts_with("v2:"));
    assert_ne!(receipt, changed, "result-ref order is receipt authority");
}
