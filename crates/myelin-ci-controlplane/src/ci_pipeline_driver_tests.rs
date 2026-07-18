//! DB-free unit tests for the CT-004d.2 culmination driver (chunks 2/3/5). The live-PG + real-`runsc`
//! end-to-end is `tests/integration_ci_ct004d2_culmination.rs`; these prove the PURE, security-critical
//! halves without a pool: the trust-tier/region forwarding, the deterministic job-id, and the
//! verdict-vocabulary bridge reporter.

use super::*;
use myelin_flow::{FlowExecutor, JobKind, JobSpec as FlowJobSpec, RunId, StartSpec};
use myelin_tenancy::{Region, TenantId};

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
    let widening_builder: StageSpecBuilder =
        fixed_command_spec_builder("registry/x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", vec!["true".into()], 60)
            .expect("pinned image");
    let (enq, spec) = build_dispatch_parts(
        &terms(TrustTier::UntrustedFork, "de-fra"),
        &widening_builder,
        &flow_spec("pipeline://acme/ci#build", "run/ci.pipeline:0/job"),
    )
    .expect("build the dispatch");

    // trust_tier forwarded UNCHANGED from the run's terms (never the builder's Trusted).
    assert_eq!(enq.trust_tier, TrustTier::UntrustedFork, "enqueue tier = the run's stamped tier");
    assert_eq!(
        spec.trust_tier,
        TrustTier::UntrustedFork,
        "the spec tier is STAMPED to the run's tier — the builder's widened Trusted is overwritten"
    );
    // co_persist_dispatch's enq.trust_tier == spec.trust_tier holds BY CONSTRUCTION.
    assert_eq!(enq.trust_tier, spec.trust_tier, "the claim-gating tier == the executing spec's tier");
    // region forwarded UNCHANGED from the run's terms.
    assert_eq!(enq.region, "de-fra", "region = the run's residency pin, forwarded unchanged");
    // the echo idem_token is stamped on the spec (the runner echoes it on job.done).
    assert_eq!(spec.idem_token, IdemToken("run/ci.pipeline:0/job".into()));
    assert_eq!(enq.idem_token, "run/ci.pipeline:0/job", "the jq_idem key = the dispatch token");
}

/// A `Trusted` run forwards `Trusted` (the control that the gate is exact, not a blanket narrow).
#[test]
fn build_dispatch_forwards_a_trusted_run_as_trusted() {
    let builder =
        fixed_command_spec_builder("registry/x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", vec!["true".into()], 60)
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
    assert_eq!(a, a2, "deterministic on the idem_token (a re-dispatch re-derives the SAME job_id)");
    assert_ne!(a, b, "distinct stages get distinct job_ids");
    assert!(sqlx::types::Uuid::parse_str(&a).is_ok(), "the job_id is a real uuid");
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
    assert_eq!(spec.limits.timeout_secs, MAX_JOB_TIMEOUT_SECS, "clamped to the ceiling");
}

/// The verdict bridge round-trips a dispatch's stage name (the reporter reads it back by idem_token).
#[test]
fn stage_verdict_bridge_round_trips() {
    let bridge = StageVerdictBridge::new();
    assert_eq!(bridge.stage_for("tok"), None, "empty before record");
    bridge.record("tok", "build");
    assert_eq!(bridge.stage_for("tok"), Some("build".to_string()));
    assert_eq!(bridge.stage_for("other"), None, "only the recorded token maps");
}

fn executor_with_run(run_id: &str) -> (FlowExecutor, RunId) {
    let ex = FlowExecutor::new(
        std::sync::Arc::new(FixedMinter(run_id.to_string())),
        TenantId("acme".into()),
        Region("fr-par".into()),
    );
    ex.register_definition(CI_PIPELINE_WF_TYPE);
    let run = ex
        .start(StartSpec {
            wf_type: CI_PIPELINE_WF_TYPE.into(),
            input: vec![],
            budget: None,
            idem_key: format!("ci:{run_id}"),
        })
        .expect("start");
    (ex, run)
}

struct FixedMinter(String);
impl myelin_events::IdMinter for FixedMinter {
    fn mint(&self) -> myelin_events::Ulid {
        myelin_events::Ulid(self.0.clone())
    }
}

/// **The reporter re-encodes the runner's `passed` into the stage-verdict marker the pipeline body
/// decodes (when the bridge has the stage).** A `passed=true` job.done for the recorded `build` stage
/// buffers `ci.stage.verdict:pass:build` — exactly what `WfCtx::run_ci_pipeline` reads.
#[test]
fn reporter_encodes_the_stage_verdict_when_the_bridge_has_the_stage() {
    let (ex, run) = executor_with_run("run-a");
    let bridge = StageVerdictBridge::new();
    bridge.record("tok-build", "build");
    let reporter = CiPipelineReporter::new(ex.clone(), TenantId("acme".into()), bridge);

    let out = reporter
        .report_done(
            &run,
            "tok-build",
            &myelin_ci_sandbox::TerminalReport {
                passed: true,
                result_refs: vec![ArtifactRef("myelin://acme/ci/log.available".into())],
            },
        )
        .expect("report");
    assert_eq!(out, myelin_flow::SignalOutcome::Buffered);

    let row = ex
        .signals()
        .get(&TenantId("acme".into()), &run.0, JOB_DONE_SIGNAL, "tok-build")
        .expect("buffered job.done");
    assert_eq!(
        row.payload[0],
        stage_verdict_marker("build", true),
        "the derived pass is re-encoded as the stage-verdict marker the body decodes"
    );
    assert!(
        row.payload.iter().any(|r| r.0.contains("log.available")),
        "the result refs still travel (references-not-payloads)"
    );
}

/// **Without a bridge mapping, the reporter falls back to the raw `passed` marker (never a fabricated
/// pass) — behaviourally identical to `EngineTerminalReporter`.** The body then rejects it LOUDLY.
#[test]
fn reporter_falls_back_to_the_passed_marker_without_a_mapping() {
    let (ex, run) = executor_with_run("run-b");
    let reporter =
        CiPipelineReporter::new(ex.clone(), TenantId("acme".into()), StageVerdictBridge::new());
    reporter
        .report_done(
            &run,
            "tok-unknown",
            &myelin_ci_sandbox::TerminalReport {
                passed: false,
                result_refs: vec![],
            },
        )
        .expect("report");
    let row = ex
        .signals()
        .get(&TenantId("acme".into()), &run.0, JOB_DONE_SIGNAL, "tok-unknown")
        .expect("buffered");
    assert_eq!(
        row.payload[0],
        ArtifactRef("myelin://job-done/passed-false".into()),
        "no stage mapping → the raw passed marker (the body rejects it loudly, never a silent pass)"
    );
}
