//! CT-004d.1 — DB-free unit gates for the durable launch-template store: the jsonb round-trip is FAITHFUL
//! (every field), a corrupt/missing spec is a fail-closed resolve error (never a fabricated default),
//! the SECURITY trust-tier + lease-TTL dispatch invariants fail closed, and the SQL constants carry
//! the exact bind arity the store binds. The live PG co-persist → claim → resolve → runsc exec end to
//! end is `tests/integration_ci_ct004d1_dispatch_resolve.rs`.

use super::*;
use myelin_ci_sandbox::{
    EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind, MeterTarget, ResourceLimits,
    RunTokenCredential, SecretRef, TrustTier, WorkspaceSpec,
};

/// A fully-populated durable launch template — EVERY field set to a non-default, distinguishable value, so the
/// round-trip test proves no field is dropped/defaulted by the jsonb serialization.
fn full_spec(trust: TrustTier, timeout_secs: u32) -> DurableCiJobLaunchTemplate {
    let resolved = myelin_ci_sandbox::JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned(
            "registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        vec!["/bin/build".into(), "--release".into(), "--jobs=4".into()],
        vec![
            EnvVar { name: "CI".into(), value: "true".into() },
            EnvVar { name: "LANG".into(), value: "en_US.UTF-8".into() },
        ],
        vec![SecretRef {
            name: "NPM_TOKEN".into(),
            handle: "broker://tenantA/npm#read".into(),
        }],
        EgressPolicy {
            allow: vec!["registry.example".into(), "10.0.0.0/8".into()],
        },
        ResourceLimits {
            cpu_millis: 2500,
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 4 << 30,
            pids_max: 256,
            timeout_secs,
        },
        WorkspaceSpec {
            repo_ref: Some("myelin://repo/tenantA/web".into()),
            commit: Some("deadbeefcafe".into()),
        },
        trust,
        RunTokenCredential::new("test-bearer", "run-token-jti-xyz", 300).unwrap(),
        MeterTarget { reserve_id: "reserve-9910".into() },
        IdemToken("idem-ci.pipeline:stage-3".into()),
    )
    .unwrap();
    let (spec, _token) = resolved.into_template();
    DurableCiJobLaunchTemplate {
        spec,
        ci_run_id: "22222222-2222-2222-2222-222222222222".into(),
        token_authority_handle: "identity-authority:job-1".into(),
    }
}

/// **The whole non-secret launch template round-trips through `spec jsonb` FAITHFULLY.** Fidelity is
/// load-bearing: serialize → jsonb → decode yields an equal template, while the claim-bound bearer
/// and JTI are structurally absent and can only be attached after a live lease is issued.
#[test]
fn a_full_launch_template_round_trips_without_a_token() {
    let spec = full_spec(TrustTier::UntrustedFork, 1800);
    // serialize exactly as `co_persist_dispatch` does (serde_json::to_value → the jsonb column).
    let json = serde_json::to_value(&spec).expect("launch template serializes to jsonb");
    // decode exactly as `get_spec` does.
    let back = decode_launch_template(&uid_job(), json)
        .expect("the stored jsonb decodes back to a launch template");
    assert_eq!(
        back, spec,
        "the decoded spec equals the original — no field lost/defaulted"
    );

    // Spot-check the load-bearing fields explicitly (a defence against a PartialEq that ever loosened).
    assert_eq!(
        back.spec.image.reference, spec.spec.image.reference,
        "the digest-pinned image survives"
    );
    assert_eq!(
        back.spec.command, spec.spec.command,
        "the command line survives"
    );
    assert_eq!(back.spec.env, spec.spec.env, "the env vars survive");
    assert_eq!(
        back.spec.secret_refs, spec.spec.secret_refs,
        "the secret NAME refs survive"
    );
    assert_eq!(
        back.spec.egress.allow, spec.spec.egress.allow,
        "the egress allowlist survives"
    );
    assert_eq!(
        back.spec.limits, spec.spec.limits,
        "all resource limits survive (incl. timeout, pids_max)"
    );
    assert_eq!(
        back.spec.workspace, spec.spec.workspace,
        "the workspace repo+commit survives"
    );
    assert_eq!(
        back.spec.trust_tier, spec.spec.trust_tier,
        "the trust tier survives"
    );
    assert_eq!(
        back.spec.meter_to, spec.spec.meter_to,
        "the meter target survives"
    );
    assert_eq!(
        back.spec.idem_token, spec.spec.idem_token,
        "the idem token survives"
    );
    assert_eq!(back.token_authority_handle, spec.token_authority_handle);
    assert!(!serde_json::to_string(&back).unwrap().contains("run_token"));
}

/// **A corrupt stored spec is a fail-closed [`CiJobSpecStoreError::CorruptSpec`], NEVER a default.**
/// The stored spec is what executes; an un-decodable jsonb MUST fail the resolve closed (the runner
/// then does not launch), never coerce to a fabricated default spec.
#[test]
fn a_corrupt_spec_jsonb_is_a_fail_closed_resolve_error() {
    // A syntactically-fine json that is NOT a launch-template shape.
    let corrupt = serde_json::json!({ "not": "a jobspec", "kind": "Nonsense" });
    let e = decode_launch_template("11111111-1111-1111-1111-111111111111", corrupt)
        .expect_err("a non-template jsonb fails the resolve closed");
    assert!(
        matches!(e, CiJobSpecStoreError::CorruptSpec { .. }),
        "an un-decodable spec is CorruptSpec (fail-closed), got: {e:?}"
    );
    // A partial spec (missing required fields) also fails closed — no field is defaulted in.
    let partial = serde_json::json!({ "kind": "Ci" });
    assert!(matches!(
        decode_launch_template("job", partial).unwrap_err(),
        CiJobSpecStoreError::CorruptSpec { .. }
    ));
}

/// **The SECURITY invariant fails closed: the enqueue's `trust_tier` MUST equal the spec's.** A
/// dispatch that would enqueue an `untrusted_fork` spec behind a widened `trusted` gate (or any
/// mismatch) is refused BEFORE any row is written — the claim-gating tier can never diverge from the
/// tier of the spec that executes.
#[test]
fn a_trust_tier_mismatch_is_refused_before_any_write() {
    let fork_spec = full_spec(TrustTier::UntrustedFork, 60);
    // The classic widening attempt: gate the row as `trusted` while the spec is `untrusted_fork`.
    let e = validate_dispatch(TrustTier::Trusted, &fork_spec)
        .expect_err("a widened gate tier is refused");
    match e {
        CiJobSpecStoreError::TrustTierMismatch { enqueue, spec } => {
            assert_eq!(enqueue, "trusted");
            assert_eq!(spec, "untrusted_fork");
        }
        other => panic!("expected TrustTierMismatch, got {other:?}"),
    }
    // The matching case (the honest dispatch) passes.
    assert!(validate_dispatch(TrustTier::UntrustedFork, &fork_spec).is_ok());
    // Every tier agreeing with itself is admitted.
    for t in [
        TrustTier::Trusted,
        TrustTier::UntrustedFork,
        TrustTier::SelfHosted,
    ] {
        assert!(validate_dispatch(t, &full_spec(t, 60)).is_ok());
    }
}

/// **The lease-TTL floor fails closed: a spec timeout above [`MAX_JOB_TIMEOUT_SECS`] is refused.** So a
/// leased job can never outlive the runner's lease (the CT-004c.2 double-run guard, closed at
/// dispatch). At-the-ceiling is admitted; one second over is refused.
#[test]
fn a_timeout_over_the_ceiling_is_refused() {
    let at = full_spec(TrustTier::Trusted, MAX_JOB_TIMEOUT_SECS);
    assert!(
        validate_dispatch(TrustTier::Trusted, &at).is_ok(),
        "at the ceiling is admitted"
    );

    let over = full_spec(TrustTier::Trusted, MAX_JOB_TIMEOUT_SECS + 1);
    let e = validate_dispatch(TrustTier::Trusted, &over).expect_err("over the ceiling is refused");
    match e {
        CiJobSpecStoreError::TimeoutTooLong { requested, ceiling } => {
            assert_eq!(requested, MAX_JOB_TIMEOUT_SECS + 1);
            assert_eq!(ceiling, MAX_JOB_TIMEOUT_SECS);
        }
        other => panic!("expected TimeoutTooLong, got {other:?}"),
    }
}

/// **The runner's wired lease TTL is strictly ABOVE the max job timeout — the invariant that makes a
/// lease-outliving double-run impossible.** This is the numeric proof of the CT-004c.2 verifier fix:
/// `CI_RUNNER_LEASE_TTL_SECS > MAX_JOB_TIMEOUT_SECS`, so a job capped at the ceiling still finishes
/// (and heartbeats) before its lease could lapse.
#[test]
fn the_wired_lease_ttl_exceeds_the_max_job_timeout() {
    assert!(
        crate::runner_bind::CI_RUNNER_LEASE_TTL_SECS > MAX_JOB_TIMEOUT_SECS as i64,
        "the runner lease TTL ({}) must exceed the max job timeout ({}) so a job never outlives its lease",
        crate::runner_bind::CI_RUNNER_LEASE_TTL_SECS,
        MAX_JOB_TIMEOUT_SECS
    );
}

/// **A non-uuid job/run id is a loud refusal (never coerced) — the durable columns are `uuid`.**
#[test]
fn a_non_uuid_id_is_a_loud_refusal() {
    let e = parse_id_local("job_id", "not-a-uuid").unwrap_err();
    assert!(matches!(
        e,
        CiJobSpecStoreError::BadId {
            field: "job_id",
            ..
        }
    ));
    assert!(parse_id_local("run_id", "00000000-0000-0000-0000-000000000001").is_ok());
}

/// **The store's SQL constants carry the exact bind arity the store binds (DB-free drift guard).** A
/// renamed column / changed bind order is loud here, before the live integration test.
#[test]
fn the_bound_sql_matches_the_store_binds() {
    // INSERT ci_job_spec: seven binds ($1..$7, the $7 durable stage), idempotent on the (tenant,
    // job_id) PK, RETURNING job_id.
    assert!(INSERT_JOB_SPEC_QUERY.contains("$7") && !INSERT_JOB_SPEC_QUERY.contains("$8"));
    assert!(INSERT_JOB_SPEC_QUERY.contains("stage"));
    assert!(INSERT_JOB_SPEC_QUERY.contains("ON CONFLICT (tenant_id, job_id) DO NOTHING"));
    assert!(INSERT_JOB_SPEC_QUERY.contains("RETURNING job_id"));
    assert!(INSERT_JOB_SPEC_QUERY.contains("ci_job_spec"));
    // SELECT spec: two binds ($1 tenant, $2 job_id), reads the spec column.
    assert!(SELECT_JOB_SPEC_QUERY.contains("$2") && !SELECT_JOB_SPEC_QUERY.contains("$3"));
    assert!(SELECT_JOB_SPEC_QUERY.contains("SELECT spec FROM ci_job_spec"));
    assert!(
        SELECT_JOB_SPEC_QUERY.contains("tenant_id = $1")
            && SELECT_JOB_SPEC_QUERY.contains("job_id = $2")
    );
    // SELECT dispatch identity: two binds, reads (run_id, idem_token, stage) for the reporter's verify.
    assert!(
        SELECT_JOB_SPEC_IDENTITY_QUERY.contains("$2")
            && !SELECT_JOB_SPEC_IDENTITY_QUERY.contains("$3")
    );
    assert!(
        SELECT_JOB_SPEC_IDENTITY_QUERY.contains("run_id")
            && SELECT_JOB_SPEC_IDENTITY_QUERY.contains("idem_token")
            && SELECT_JOB_SPEC_IDENTITY_QUERY.contains("stage")
    );
    assert!(
        SELECT_JOB_SPEC_IDENTITY_QUERY.contains("tenant_id = $1")
            && SELECT_JOB_SPEC_IDENTITY_QUERY.contains("job_id = $2")
    );
    // The null-stage activation guard: region-scoped, counts NON-terminal jobs whose stage IS NULL,
    // joined job_queue↔ci_job_spec on (tenant_id, job_id).
    assert!(NON_TERMINAL_NULL_STAGE_JOBS_QUERY.contains("count(*)"));
    assert!(NON_TERMINAL_NULL_STAGE_JOBS_QUERY.contains("q.region = $1"));
    assert!(NON_TERMINAL_NULL_STAGE_JOBS_QUERY.contains("q.state <> 'terminal'"));
    assert!(NON_TERMINAL_NULL_STAGE_JOBS_QUERY.contains("q.stage IS NULL"));
    assert!(!NON_TERMINAL_NULL_STAGE_JOBS_QUERY.contains("ci_job_spec"));
}

/// A stable uuid string for the round-trip test's job id argument (only used for the error label).
fn uid_job() -> String {
    "22222222-2222-2222-2222-222222222222".to_string()
}
