use super::*;
use myelin_ci_sandbox::{
    EgressPolicy, EnvVar, IdemToken, ImageRef, JobKind, MeterTarget, ResourceLimits,
    RunTokenCredential, SecretRef, TrustTier, WorkspaceSpec,
};

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
            tmpfs_bytes: 4 << 30,
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
        project_id: "55555555-5555-4555-8555-555555555555".into(),
        ci_run_id: "22222222-2222-2222-2222-222222222222".into(),
        token_authority_handle: "identity-authority:job-1".into(),
    }
}

#[test]
fn a_full_launch_template_round_trips_without_a_token() {
    let spec = full_spec(TrustTier::UntrustedFork, 1800);
    let json = serde_json::to_value(&spec).expect("launch template serializes to jsonb");
    let back = decode_launch_template(&uid_job(), json)
        .expect("the stored jsonb decodes back to a launch template");
    assert_eq!(
        back, spec,
        "the decoded spec equals the original - no field lost/defaulted"
    );

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

#[test]
fn a_corrupt_spec_jsonb_is_a_fail_closed_resolve_error() {
    let corrupt = serde_json::json!({ "not": "a jobspec", "kind": "Nonsense" });
    let e = decode_launch_template("11111111-1111-1111-1111-111111111111", corrupt)
        .expect_err("a non-template jsonb fails the resolve closed");
    assert!(
        matches!(e, CiJobSpecStoreError::CorruptSpec { .. }),
        "an un-decodable spec is CorruptSpec (fail-closed), got: {e:?}"
    );
    let partial = serde_json::json!({ "kind": "Ci" });
    assert!(matches!(
        decode_launch_template("job", partial).unwrap_err(),
        CiJobSpecStoreError::CorruptSpec { .. }
    ));
}

#[test]
fn a_trust_tier_mismatch_is_refused_before_any_write() {
    let fork_spec = full_spec(TrustTier::UntrustedFork, 60);
    let e = validate_dispatch(TrustTier::Trusted, None, &fork_spec)
        .expect_err("a widened gate tier is refused");
    match e {
        CiJobSpecStoreError::TrustTierMismatch { enqueue, spec } => {
            assert_eq!(enqueue, "trusted");
            assert_eq!(spec, "untrusted_fork");
        }
        other => panic!("expected TrustTierMismatch, got {other:?}"),
    }
    assert!(validate_dispatch(TrustTier::UntrustedFork, None, &fork_spec).is_ok());
    for t in [
        TrustTier::Trusted,
        TrustTier::UntrustedFork,
        TrustTier::SelfHosted,
    ] {
        assert!(validate_dispatch(t, None, &full_spec(t, 60)).is_ok());
    }
}

#[test]
fn a_timeout_over_the_ceiling_is_refused() {
    let at = full_spec(TrustTier::Trusted, MAX_JOB_TIMEOUT_SECS);
    assert!(
        validate_dispatch(TrustTier::Trusted, None, &at).is_ok(),
        "at the ceiling is admitted"
    );

    let over = full_spec(TrustTier::Trusted, MAX_JOB_TIMEOUT_SECS + 1);
    let e = validate_dispatch(TrustTier::Trusted, None, &over)
        .expect_err("over the ceiling is refused");
    match e {
        CiJobSpecStoreError::TimeoutTooLong { requested, ceiling } => {
            assert_eq!(requested, MAX_JOB_TIMEOUT_SECS + 1);
            assert_eq!(ceiling, MAX_JOB_TIMEOUT_SECS);
        }
        other => panic!("expected TimeoutTooLong, got {other:?}"),
    }
}

#[test]
fn the_wired_lease_ttl_exceeds_the_max_job_timeout() {
    assert!(
        crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS > MAX_JOB_TIMEOUT_SECS as i64,
        "the runner lease TTL ({}) must exceed the max job timeout ({}) so a job never outlives its lease",
        crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
        MAX_JOB_TIMEOUT_SECS
    );
}

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

fn uid_job() -> String {
    "22222222-2222-2222-2222-222222222222".to_string()
}
