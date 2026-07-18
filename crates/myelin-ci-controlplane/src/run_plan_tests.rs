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
        definition_snapshot: format!(
            "myelin://tenant_01/ci/snapshot/{}",
            hash.to_multihash_string()
        ),
        trigger_kind: "push".into(),
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
    assert_eq!(
        plan.canonical_bytes().expect("first"),
        plan.canonical_bytes().expect("second")
    );
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
            br#"{"schema_version":2,"jobs":[]}"#.to_vec(),
            RedispatchReason::UnsupportedVersion(2),
        ),
    ] {
        let (store, run) = store_and_run(bytes);
        let error = load_resolved_run_plan(&store, &run).expect_err("version must fail");
        assert_eq!(error, RunPlanError::RedispatchRequired(reason));
        assert!(error.requires_redispatch());
    }
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
