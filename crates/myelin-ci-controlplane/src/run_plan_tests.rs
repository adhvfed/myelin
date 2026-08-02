use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_storage::{BlobError, BlobMeta, BlobStore, ContentHash};
use myelin_tenancy::TenantId;

use super::*;

const PINNED_IMAGE: &str =
    "registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Debug)]
struct CountingBlobStore {
    bytes: Vec<u8>,
    hash: ContentHash,
    advertised_len: usize,
    calls: Mutex<Vec<(&'static str, String, ContentHash)>>,
}

impl CountingBlobStore {
    fn new(bytes: Vec<u8>) -> Self {
        let hash = ContentHash::blake3(&bytes);
        Self {
            advertised_len: bytes.len(),
            bytes,
            hash,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn with_advertised_len(mut self, advertised_len: usize) -> Self {
        self.advertised_len = advertised_len;
        self
    }

    fn calls(&self) -> Vec<(&'static str, String, ContentHash)> {
        self.calls.lock().expect("calls mutex").clone()
    }

    fn record(&self, operation: &'static str, tenant: &TenantId, hash: &ContentHash) {
        self.calls.lock().expect("calls mutex").push((
            operation,
            tenant.as_str().to_string(),
            hash.clone(),
        ));
    }
}

impl BlobStore for CountingBlobStore {
    fn put(&self, tenant: &TenantId, bytes: &[u8]) -> Result<ContentHash, BlobError> {
        let hash = ContentHash::blake3(bytes);
        self.record("put", tenant, &hash);
        Ok(hash)
    }

    fn get(&self, tenant: &TenantId, hash: &ContentHash) -> Result<Vec<u8>, BlobError> {
        self.record("get", tenant, hash);
        if hash != &self.hash {
            return Err(BlobError::NotFound {
                tenant: tenant.clone(),
                hash: hash.clone(),
            });
        }
        Ok(self.bytes.clone())
    }

    fn head(&self, tenant: &TenantId, hash: &ContentHash) -> Result<BlobMeta, BlobError> {
        self.record("head", tenant, hash);
        if hash != &self.hash {
            return Err(BlobError::NotFound {
                tenant: tenant.clone(),
                hash: hash.clone(),
            });
        }
        Ok(BlobMeta {
            hash: self.hash.clone(),
            stored_len: self.advertised_len,
        })
    }

    fn delete(&self, tenant: &TenantId, hash: &ContentHash) -> Result<(), BlobError> {
        self.record("delete", tenant, hash);
        Ok(())
    }
}

fn job(name: &str) -> ResolvedJobV1 {
    ResolvedJobV1 {
        name: name.into(),
        image: PINNED_IMAGE.into(),
        command: vec!["/bin/test".into(), "--locked".into()],
        needs: Vec::new(),
        is_generator: false,
        matrix_key: BTreeMap::new(),
    }
}

fn valid_plan() -> ResolvedRunPlanV1 {
    let build = job("build");
    let mut test = job("test");
    test.needs.push("build".into());
    ResolvedRunPlanV1 {
        schema_version: RUN_PLAN_SCHEMA_V1,
        jobs: vec![build, test],
    }
}

fn valid_plan_v2() -> ResolvedRunPlanV2 {
    let convert = |stage: &str, job: ResolvedJobV1| ResolvedJobV2 {
        stage: stage.into(),
        name: job.name,
        image: job.image,
        command: job.command,
        build: None,
        needs: job.needs,
        is_generator: job.is_generator,
        matrix_key: job.matrix_key,
    };
    let build = convert("build", job("build"));
    let mut test = job("test--os-linux");
    test.needs.push("build".into());
    test.matrix_key.insert("os".into(), "linux".into());
    let mut test = convert("test", test);
    test.name = derive_concrete_job_name(&test.stage, &test.matrix_key);
    ResolvedRunPlanV2 {
        schema_version: RUN_PLAN_SCHEMA_V2,
        execution: CiExecutionRequestV1 {
            schema_version: EXECUTION_REQUEST_SCHEMA_V1,
            profile: CiExecutionProfileV1::LinuxSmallV1,
        },
        jobs: vec![build, test],
    }
}

fn structured_cargo_plan_v2() -> ResolvedRunPlanV2 {
    let mut plan = valid_plan_v2();
    plan.jobs[0].command.clear();
    plan.jobs[0].build = Some(StructuredBuildV1 {
        tool: StructuredBuildToolV1::Cargo,
        args: vec!["build".into(), "--locked".into()],
    });
    plan
}

fn run_for(hash: &ContentHash) -> CiRunRecord {
    CiRunRecord {
        tenant_id: "tenant_01".into(),
        run_id: "00000000-0000-0000-0000-000000000001".into(),
        region: "eu-west".into(),
        project_id: "00000000-0000-0000-0000-000000000002".into(),
        pipeline_id: "00000000-0000-0000-0000-000000000003".into(),
        wf_run_id: "00000000-0000-0000-0000-000000000004".into(),
        repo_ref: Some("repo_01".into()),
        commit_oid: Some("abc123".into()),
        cause_event_id: Some("event_01".into()),
        cause_depth: 0,
        caused_by: None,
        definition_snapshot: format!(
            "myelin://tenant_01/ci/snapshot/{}",
            hash.to_multihash_string()
        ),
        trigger_kind: "push".into(),
        concurrency_group: None,
        pr_head_generation: None,
        trust_tier: "trusted".into(),
        state: "queued".into(),
        correlation_id: "correlation_01".into(),
    }
}

fn store_and_run(bytes: Vec<u8>) -> (CountingBlobStore, CiRunRecord) {
    let store = CountingBlobStore::new(bytes);
    let run = run_for(&store.hash);
    (store, run)
}

fn assert_invalid(plan: ResolvedRunPlanV1, expected: &str) {
    let bytes = serde_json::to_vec(&plan).expect("serialize adversarial plan");
    let (store, run) = store_and_run(bytes);
    let error = load_resolved_run_plan(&store, &run).expect_err("plan must be refused");
    match error {
        RunPlanError::InvalidPlan { detail } => assert!(
            detail.contains(expected),
            "expected `{expected}` in `{detail}`"
        ),
        other => panic!("expected invalid plan, got {other:?}"),
    }
}

#[test]
fn loads_exact_tenant_scoped_canonical_plan_after_head() {
    let plan = valid_plan();
    let bytes = plan.canonical_bytes().expect("canonical plan");
    let (store, run) = store_and_run(bytes.clone());

    let prepared = load_resolved_run_plan(&store, &run).expect("prepare plan");

    assert_eq!(prepared.tenant().as_str(), "tenant_01");
    assert_eq!(prepared.content_hash(), &ContentHash::blake3(&bytes));
    assert_eq!(prepared.plan(), &plan);
    assert_eq!(
        store
            .calls()
            .iter()
            .map(|(operation, _, _)| *operation)
            .collect::<Vec<_>>(),
        vec!["head", "get"]
    );
    assert!(store
        .calls()
        .iter()
        .all(|(_, tenant, hash)| tenant == "tenant_01" && hash == prepared.content_hash()));
}

#[test]
fn canonical_bytes_are_stable_and_matrix_identity_is_collision_safe() {
    let mut left = job("left");
    left.matrix_key.insert("a".into(), "bc".into());
    let mut right = job("right");
    right.matrix_key.insert("ab".into(), "c".into());
    assert_ne!(left.matrix_identity(), right.matrix_identity());

    let plan = valid_plan();
    let bytes = plan.canonical_bytes().expect("first");
    assert_eq!(bytes, plan.canonical_bytes().expect("second"));
    assert_eq!(
        bytes,
        concat!(
            "{\"schema_version\":1,\"jobs\":[",
            "{\"name\":\"build\",\"image\":\"registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"command\":[\"/bin/test\",\"--locked\"],\"needs\":[],\"is_generator\":false,\"matrix_key\":{}},",
            "{\"name\":\"test\",\"image\":\"registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"command\":[\"/bin/test\",\"--locked\"],\"needs\":[\"build\"],\"is_generator\":false,\"matrix_key\":{}}]}"
        )
        .as_bytes(),
        "the V1 wire stays byte-identical"
    );
}

#[test]
fn version_two_canonical_wire_and_launch_request_digest_are_pinned() {
    let plan = valid_plan_v2();
    let bytes = plan.canonical_bytes().expect("canonical V2 plan");
    let expected = concat!(
        "{\"schema_version\":2,\"execution\":{\"schema_version\":1,\"profile\":\"linux-small-v1\"},\"jobs\":[",
        "{\"stage\":\"build\",\"name\":\"build\",\"image\":\"registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"command\":[\"/bin/test\",\"--locked\"],\"needs\":[],\"is_generator\":false,\"matrix_key\":{}},",
        "{\"stage\":\"test\",\"name\":\"test--f1a421a6c2c1159fe7bb9c489237a217bfe1d6d2a45f35b5c67d21730c69b358\",\"image\":\"registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"command\":[\"/bin/test\",\"--locked\"],\"needs\":[\"build\"],\"is_generator\":false,\"matrix_key\":{\"os\":\"linux\"}}]}"
    );
    assert_eq!(bytes, expected.as_bytes());
    assert_eq!(
        decode_resolved_run_plan(&bytes).expect("decode canonical V2"),
        VersionedResolvedRunPlan::V2(plan.clone())
    );
    assert_eq!(
        plan.launch_request_digest_v1().expect("request digest"),
        "blake3:e41fa8f911b554840fd4b3abe85833295869529566da294839c52bd21171610e"
    );
}

#[test]
fn structured_cargo_recipe_is_canonical_and_constructs_direct_argv() {
    let plan = structured_cargo_plan_v2();
    let bytes = plan
        .canonical_bytes()
        .expect("valid structured Cargo recipe");
    let decoded = decode_resolved_run_plan(&bytes).expect("decode structured recipe");
    assert_eq!(decoded, VersionedResolvedRunPlan::V2(plan.clone()));

    let build = plan.jobs[0].build.as_ref().unwrap();
    assert_eq!(
        build.platform_argv(),
        [
            "cargo",
            "build",
            "--locked",
            "--config",
            "source.crates-io.replace-with=\"vendored\"",
            "--config",
            "source.vendored.directory=\"/opt/myelin/cargo-vendor\"",
        ]
    );
    assert!(!build
        .platform_argv()
        .iter()
        .any(|arg| arg == "/bin/sh" || arg == "-c"));
}

#[test]
fn structured_build_validation_rejects_shell_unknown_tool_and_oversized_args() {
    let mut shell = structured_cargo_plan_v2();
    shell.jobs[0].build.as_mut().unwrap().args =
        vec!["build".into(), "--locked;touch-pwned".into()];
    assert!(matches!(
        shell.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("shell metacharacters")
    ));

    let mut oversized = structured_cargo_plan_v2();
    oversized.jobs[0].build.as_mut().unwrap().args = vec!["x".repeat(257)];
    assert!(matches!(
        oversized.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("exceeds 256 bytes")
    ));

    let mut tenant_config = structured_cargo_plan_v2();
    tenant_config.jobs[0].build.as_mut().unwrap().args = vec![
        "build".into(),
        "--locked".into(),
        "--config".into(),
        "source.crates-io.replace-with=tenant".into(),
    ];
    assert!(matches!(
        tenant_config.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("exactly `build --locked`")
    ));

    let valid = structured_cargo_plan_v2().canonical_bytes().unwrap();
    let mut unknown: serde_json::Value = serde_json::from_slice(&valid).unwrap();
    unknown["jobs"][0]["build"]["tool"] = serde_json::json!("make");
    assert!(matches!(
        decode_resolved_run_plan(&serde_json::to_vec(&unknown).unwrap()),
        Err(RunPlanError::WireMalformed { detail }) if detail.contains("unknown variant `make`")
    ));
}

#[test]
fn version_two_job_execution_is_exactly_one_of_command_or_build() {
    let mut both = structured_cargo_plan_v2();
    both.jobs[0].command = vec!["/bin/sh".into(), "-c".into(), "cargo build".into()];
    assert!(matches!(
        both.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("never both")
    ));

    let mut neither = valid_plan_v2();
    neither.jobs[0].command.clear();
    assert!(matches!(
        neither.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("either command or build")
    ));
}

#[test]
fn concrete_job_name_derivation_is_pinned_and_binds_stage_and_matrix() {
    assert_eq!(derive_concrete_job_name("build", &BTreeMap::new()), "build");
    let matrix = BTreeMap::from([("os".into(), "linux".into())]);
    assert_eq!(
        derive_concrete_job_name("test", &matrix),
        "test--f1a421a6c2c1159fe7bb9c489237a217bfe1d6d2a45f35b5c67d21730c69b358"
    );
    assert_ne!(
        derive_concrete_job_name("test", &matrix),
        derive_concrete_job_name("tests", &matrix)
    );
    assert_ne!(
        derive_concrete_job_name("test", &matrix),
        derive_concrete_job_name("test", &BTreeMap::from([("os".into(), "macos".into())]))
    );
}

#[test]
fn version_two_stage_and_static_dag_rules_are_fail_closed() {
    let mut bad_stage = valid_plan_v2();
    bad_stage.jobs[0].stage = "bad stage".into();
    assert!(matches!(
        bad_stage.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("stage")
    ));

    let mut generator = valid_plan_v2();
    generator.jobs[0].is_generator = true;
    assert!(matches!(
        generator.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("fragment ingestion")
    ));

    let mut forged_name = valid_plan_v2();
    forged_name.jobs.last_mut().unwrap().name = "test".into();
    let mut forged_stage = valid_plan_v2();
    forged_stage.jobs.last_mut().unwrap().stage = "tests".into();
    let mut forged_matrix = valid_plan_v2();
    forged_matrix
        .jobs
        .last_mut()
        .unwrap()
        .matrix_key
        .insert("os".into(), "macos".into());
    for mismatch in [forged_name, forged_stage, forged_matrix] {
        assert!(matches!(
            mismatch.canonical_bytes(),
            Err(RunPlanError::InvalidPlan { detail }) if detail.contains("does not match stage")
        ));
    }

    let mut empty_matrix_mismatch = valid_plan_v2();
    empty_matrix_mismatch.jobs[0].name = "build--forged".into();
    assert!(matches!(
        empty_matrix_mismatch.canonical_bytes(),
        Err(RunPlanError::InvalidPlan { detail }) if detail.contains("does not match stage")
    ));

    let unknown = valid_plan_v2().canonical_bytes().unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&unknown).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("trust_tier".into(), serde_json::json!("trusted"));
    assert!(matches!(
        decode_resolved_run_plan(&serde_json::to_vec(&value).unwrap()),
        Err(RunPlanError::WireMalformed { .. })
    ));
}

#[test]
fn every_reference_provenance_refusal_happens_before_blob_access() {
    let bytes = valid_plan().canonical_bytes().expect("canonical plan");
    let (store, base) = store_and_run(bytes);
    let cases = [
        ("", "empty tenant"),
        ("blake3:abcd", "legacy bare hash"),
        (
            "myelin://tenant_01/ci/snap/blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "wrong path",
        ),
        (
            "myelin://tenant_01/ci/snapshot/blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "short hash",
        ),
        (
            "myelin://tenant_01/ci/snapshot/blake3:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "uppercase hash",
        ),
        (
            "myelin://tenant_01/ci/snapshot/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "wrong hash algorithm",
        ),
        (
            "myelin://tenant_01/ci/snapshot/blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/extra",
            "extra path",
        ),
    ];

    for (reference, label) in cases {
        let mut run = base.clone();
        if label == "empty tenant" {
            run.tenant_id.clear();
        } else {
            run.definition_snapshot = reference.into();
        }
        assert!(
            load_resolved_run_plan(&store, &run).is_err(),
            "{label} must fail"
        );
        assert!(store.calls().is_empty(), "{label} touched the blob store");
    }
}

#[test]
fn cross_tenant_reference_is_refused_before_blob_access() {
    let bytes = valid_plan().canonical_bytes().expect("canonical plan");
    let (store, mut run) = store_and_run(bytes);
    run.definition_snapshot = run
        .definition_snapshot
        .replacen("tenant_01", "tenant_02", 1);

    assert!(matches!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::TenantMismatch { .. })
    ));
    assert!(store.calls().is_empty());
}

#[test]
fn valid_but_absent_hash_is_a_loud_head_failure_and_never_gets() {
    let bytes = valid_plan().canonical_bytes().expect("canonical plan");
    let (store, mut run) = store_and_run(bytes);
    let absent = ContentHash::blake3(b"not the snapshot");
    run.definition_snapshot = format!(
        "myelin://tenant_01/ci/snapshot/{}",
        absent.to_multihash_string()
    );

    assert!(matches!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::Blob(BlobError::NotFound { .. }))
    ));
    assert_eq!(
        store
            .calls()
            .iter()
            .map(|(operation, _, _)| *operation)
            .collect::<Vec<_>>(),
        vec!["head"]
    );
}

#[test]
fn missing_repository_or_commit_provenance_is_refused_before_blob_access() {
    let bytes = valid_plan().canonical_bytes().expect("canonical plan");
    let (store, base) = store_and_run(bytes);
    let mut cases = Vec::new();
    let mut missing_repo = base.clone();
    missing_repo.repo_ref = None;
    cases.push(missing_repo);
    let mut empty_repo = base.clone();
    empty_repo.repo_ref = Some(" ".into());
    cases.push(empty_repo);
    let mut missing_commit = base.clone();
    missing_commit.commit_oid = None;
    cases.push(missing_commit);
    let mut empty_commit = base;
    empty_commit.commit_oid = Some(String::new());
    cases.push(empty_commit);

    for run in cases {
        assert!(matches!(
            load_resolved_run_plan(&store, &run),
            Err(RunPlanError::ProvenanceRefused { .. })
        ));
        assert!(
            store.calls().is_empty(),
            "provenance refusal must precede head and get"
        );
    }
}

#[test]
fn loader_accepts_a_dyn_blob_store_at_the_composition_boundary() {
    let plan = valid_plan();
    let bytes = plan.canonical_bytes().expect("canonical plan");
    let (store, run) = store_and_run(bytes);
    let erased: &dyn BlobStore = &store;
    assert_eq!(
        load_resolved_run_plan(erased, &run)
            .expect("dyn store")
            .plan(),
        &plan
    );
}

#[test]
fn head_size_limit_refuses_get() {
    let bytes = valid_plan().canonical_bytes().expect("canonical plan");
    let store = CountingBlobStore::new(bytes).with_advertised_len(MAX_RUN_PLAN_BYTES + 1);
    let run = run_for(&store.hash);

    assert!(matches!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::SnapshotTooLarge { .. })
    ));
    assert_eq!(
        store
            .calls()
            .iter()
            .map(|(operation, _, _)| *operation)
            .collect::<Vec<_>>(),
        vec!["head"]
    );
}

#[test]
fn plaintext_size_is_rechecked_after_get() {
    let bytes = vec![b' '; MAX_RUN_PLAN_BYTES + 1];
    let store = CountingBlobStore::new(bytes).with_advertised_len(1);
    let run = run_for(&store.hash);

    assert!(matches!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::SnapshotTooLarge { .. })
    ));
    assert_eq!(store.calls().len(), 2);
}

#[test]
fn legacy_and_unknown_versions_loudly_require_redispatch() {
    for (bytes, reason) in [
        (
            br#"{"jobs":[]}"#.to_vec(),
            RedispatchReason::LegacyUnversioned,
        ),
        (
            br#"{"schema_version":3,"jobs":[]}"#.to_vec(),
            RedispatchReason::UnsupportedVersion(3),
        ),
    ] {
        let (store, run) = store_and_run(bytes);
        let error = load_resolved_run_plan(&store, &run).expect_err("version must fail");
        assert_eq!(error, RunPlanError::RedispatchRequired(reason));
        assert!(error.requires_redispatch());
    }
}

#[test]
fn current_v1_starter_loader_explicitly_refuses_v2_without_launch_authority() {
    let bytes = valid_plan_v2().canonical_bytes().unwrap();
    let (store, run) = store_and_run(bytes);
    assert_eq!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::LaunchAuthorityRequired {
            version: RUN_PLAN_SCHEMA_V2,
        })
    );
}

#[test]
fn manifest_launch_loader_accepts_only_canonical_v2() {
    let v2 = valid_plan_v2();
    let bytes = v2.canonical_bytes().unwrap();
    let (store, run) = store_and_run(bytes.clone());
    let prepared = load_launch_run_plan_v2(&store, &run).expect("prepare canonical V2 launch");
    assert_eq!(prepared.plan(), &v2);
    assert_eq!(prepared.content_hash(), &ContentHash::blake3(&bytes));
    assert_eq!(prepared.tenant().as_str(), "tenant_01");

    let bytes = valid_plan().canonical_bytes().unwrap();
    let (store, run) = store_and_run(bytes);
    assert!(matches!(
        load_launch_run_plan_v2(&store, &run),
        Err(RunPlanError::InvalidPlan { detail })
            if detail.contains("requires run-plan schema V2; received V1")
    ));
}

#[test]
fn public_versioned_decoder_refuses_oversized_input_before_parsing() {
    let bytes = vec![b' '; MAX_RUN_PLAN_BYTES + 1];
    assert_eq!(
        decode_resolved_run_plan(&bytes),
        Err(RunPlanError::SnapshotTooLarge {
            actual: MAX_RUN_PLAN_BYTES + 1,
            maximum: MAX_RUN_PLAN_BYTES,
        })
    );
}

#[test]
fn unknown_fields_and_authority_fields_are_rejected() {
    for field in [
        "unknown",
        "secrets",
        "egress",
        "mounts",
        "run_token",
        "trust_tier",
    ] {
        let bytes = format!("{{\"schema_version\":1,\"jobs\":[],\"{field}\":true}}").into_bytes();
        let (store, run) = store_and_run(bytes);
        assert!(matches!(
            load_resolved_run_plan(&store, &run),
            Err(RunPlanError::WireMalformed { .. })
        ));
    }

    let bytes = format!(
        "{{\"schema_version\":1,\"jobs\":[{{\"name\":\"build\",\"image\":\"{PINNED_IMAGE}\",\"command\":[\"test\"],\"needs\":[],\"is_generator\":false,\"matrix_key\":{{}},\"secret_refs\":[]}}]}}"
    )
    .into_bytes();
    let (store, run) = store_and_run(bytes);
    assert!(matches!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::WireMalformed { .. })
    ));
}

#[test]
fn noncanonical_json_is_rejected() {
    let canonical = valid_plan().canonical_bytes().expect("canonical plan");
    let mut whitespace = Vec::with_capacity(canonical.len() + 1);
    whitespace.push(b' ');
    whitespace.extend(canonical);
    let (store, run) = store_and_run(whitespace);
    assert!(matches!(
        load_resolved_run_plan(&store, &run),
        Err(RunPlanError::WireMalformed { .. })
    ));
}

#[test]
fn generators_are_refused_until_fragment_ingestion_exists() {
    let mut plan = valid_plan();
    plan.jobs[0].is_generator = true;
    assert_invalid(plan, "fragment ingestion is not implemented");
}

#[test]
fn names_and_job_count_are_bounded_and_deterministic() {
    let mut plan = valid_plan();
    plan.jobs[0].name = "not a token".into();
    assert_invalid(plan, "machine token");

    let mut plan = valid_plan();
    plan.jobs[0].name = format!("a{}", "b".repeat(MAX_JOB_NAME_BYTES));
    assert_invalid(plan, "bounded machine token");

    let jobs = (0..=MAX_RUN_PLAN_JOBS)
        .map(|index| job(&format!("job_{index:04}")))
        .collect();
    assert_invalid(
        ResolvedRunPlanV1 {
            schema_version: 1,
            jobs,
        },
        "above the 1024-job limit",
    );

    let mut plan = valid_plan();
    plan.jobs.swap(0, 1);
    assert_invalid(plan, "sorted strictly by name");
}

#[test]
fn dependency_graph_rejects_unknown_duplicate_self_and_cycles() {
    let mut plan = valid_plan();
    plan.jobs[1].needs = vec!["missing".into()];
    assert_invalid(plan, "unknown job");

    let mut plan = valid_plan();
    plan.jobs[1].needs = vec!["build".into(), "build".into()];
    assert_invalid(plan, "repeats need");

    let mut plan = valid_plan();
    plan.jobs[0].needs = vec!["build".into()];
    assert_invalid(plan, "depends on itself");

    let mut plan = valid_plan();
    plan.jobs[0].needs = vec!["test".into()];
    assert_invalid(plan, "contains a cycle");
}

#[test]
fn image_must_be_bounded_and_strictly_digest_pinned() {
    for image in [
        "alpine:latest".to_string(),
        "alpine@sha256:abcd".to_string(),
        format!("{}@sha256:{}", "r".repeat(MAX_IMAGE_BYTES), "a".repeat(64)),
    ] {
        let mut plan = valid_plan();
        plan.jobs[0].image = image;
        assert_invalid(plan, "image");
    }
}

#[test]
fn command_is_explicit_bounded_and_nul_free() {
    let cases = [
        (Vec::new(), "1..=64"),
        (vec![String::new()], "executable is empty"),
        (vec!["x\0y".into()], "contains NUL"),
        (vec!["x".into(); MAX_COMMAND_ARGS + 1], "1..=64"),
        (vec!["x".repeat(MAX_COMMAND_BYTES + 1)], "above 32768"),
    ];
    for (command, expected) in cases {
        let mut plan = valid_plan();
        plan.jobs[0].command = command;
        assert_invalid(plan, expected);
    }
}

#[test]
fn matrix_axes_and_values_are_bounded_machine_tokens() {
    let mut plan = valid_plan();
    plan.jobs[0]
        .matrix_key
        .insert("bad key".into(), "linux".into());
    assert_invalid(plan, "matrix key");

    let mut plan = valid_plan();
    plan.jobs[0]
        .matrix_key
        .insert("os".into(), "bad/value".into());
    assert_invalid(plan, "matrix value");

    let mut plan = valid_plan();
    plan.jobs[0].matrix_key.insert(
        format!("a{}", "b".repeat(MAX_MATRIX_KEY_BYTES)),
        "linux".into(),
    );
    assert_invalid(plan, "matrix key");

    let mut plan = valid_plan();
    plan.jobs[0].matrix_key.insert(
        "os".into(),
        format!("a{}", "b".repeat(MAX_MATRIX_VALUE_BYTES)),
    );
    assert_invalid(plan, "matrix value");

    let mut plan = valid_plan();
    plan.jobs[0].matrix_key = (0..=MAX_MATRIX_AXES)
        .map(|index| (format!("axis_{index:02}"), "value".into()))
        .collect();
    assert_invalid(plan, "more than 16 matrix axes");
}
