//! The checked-in founder pipeline must remain an armed, resolvable V2 request.
//!
//! This reads the real repository file rather than a copied fixture so a malformed, floating-image,
//! renamed-check, or edge-losing edit cannot silently leave the live acceptance run untriggerable.
//!
//! CT-007 gate-4 cutover: the founder pipeline is now the 3-job `build` → {`test`, `clippy`} DAG on
//! the `linux-build-v1` profile, each job a structured `cargo` build recipe (no raw shell command)
//! against the staged `linux-rust-v1` rootfs digest. These assertions pin that exact structure —
//! job names, DAG edges (`needs`), the executed cargo argv, the execution profile, and the image
//! digest — through the real parse → plan_dispatch → CAS-snapshot path.

use myelin_ci_dispatch::{
    parse_versioned_ci_config, plan_dispatch, CiPlanContract, DispatchOutcome, GitConfigReader,
    GitReadError, OnTrigger, StructuredBuildToolV1, StructuredBuildV1,
};
use myelin_events::{
    Actor, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_tenancy::{Region, TenantId};
use std::fs;
use std::path::{Path, PathBuf};

const ROOTFS_IMAGE: &str = "myelin.local/linux-rust-v1-rootfs@sha256:e6684d70e026a1433a7e32e2d29c100468d08579ef532834fdd27d4808c35a60";
const OID: &str = "dddddddddddddddddddddddddddddddddddddddd";

/// The exact tenant-authored Cargo recipe for each job (the platform prepends `cargo` and injects
/// the vendor `--config` pairs at `platform_argv()` time — asserted separately below).
fn recipe(args: &[&str]) -> StructuredBuildV1 {
    StructuredBuildV1 {
        tool: StructuredBuildToolV1::Cargo,
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("dispatch is under workspace/crates")
        .to_path_buf()
}

/// Serves the checked-in repository files the planner reads at the pushed ref: the pipeline config
/// AND the root `Cargo.lock` (structured Cargo build jobs key their server-trusted vendor on the
/// root lock's SHA-256, so `plan_dispatch` reads it during resolution). Both are read live from the
/// real workspace so a lock/vendor drift surfaces here rather than in the live acceptance run.
struct CheckedInRepo;

impl GitConfigReader for CheckedInRepo {
    fn read_repo_file(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        assert_eq!(
            (tenant, region, repo, oid),
            ("myelin", "fr-par", "myelin", OID)
        );
        match path {
            ".myelin/ci.toml" | "Cargo.lock" => Ok(Some(
                fs::read(workspace_root().join(path)).expect("checked-in repo file must be readable"),
            )),
            _ => Ok(None),
        }
    }
}

fn push_envelope() -> EventEnvelope {
    let tenant = TenantId("myelin".into());
    let ref_key = myelin_git::receive_pack::GitRefEventKey::new(
        "myelin",
        &myelin_git::receive_pack::RefName::new("refs/heads/main"),
    )
    .unwrap();
    EventEnvelope {
        event_id: EventId("founder-push".into()),
        type_: EventType(myelin_git::events::GIT_REF_UPDATED.into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("founder".into()),
            PrincipalKind::Human,
            tenant,
        )),
        subject: ref_key.subject("myelin").unwrap(),
        aggregate: ref_key.aggregate(),
        causation_id: None,
        correlation_id: CorrelationId("founder-acceptance".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-24T00:00:00Z".into()),
        payload: serde_json::json!({
            "repo": "myelin",
            "ref": "refs/heads/main",
            "new_oid": OID,
            "old_oid": "0000000000000000000000000000000000000000",
            "forced": false
        }),
    }
}

#[test]
fn checked_in_founder_pipeline_is_the_armed_resolvable_build_test_clippy_dag() {
    let bytes = fs::read(workspace_root().join(".myelin/ci.toml"))
        .expect("the founder pipeline must remain checked in");
    let definition =
        parse_versioned_ci_config(&bytes, ".myelin/ci.toml").expect("founder config parses");

    // ---- contract / trigger / execution profile ----
    match &definition.contract {
        CiPlanContract::V2(execution) => assert_eq!(
            execution.profile,
            myelin_ci_controlplane::CiExecutionProfileV1::LinuxBuildV1,
            "the cutover pipeline runs on the Rust-capable linux-build-v1 profile"
        ),
        CiPlanContract::V1 => panic!("founder pipeline must remain a V2 execution request"),
    }
    assert_eq!(definition.on, OnTrigger::Push);

    // ---- the authored 3-job build → {test, clippy} DAG ----
    assert_eq!(definition.jobs.len(), 3, "build + test + clippy");
    let authored: Vec<String> = definition.jobs.iter().map(|j| j.name.clone()).collect();
    assert_eq!(authored, ["build", "test", "clippy"].map(String::from));

    for job in &definition.jobs {
        assert_eq!(
            job.image, ROOTFS_IMAGE,
            "every job pins the staged linux-rust-v1 rootfs digest"
        );
        assert!(
            job.command.is_empty(),
            "structured `build` jobs carry no raw command argv"
        );
    }
    let authored_job = |n: &str| definition.jobs.iter().find(|j| j.name == n).unwrap();
    assert_eq!(
        authored_job("build").build.as_ref().unwrap(),
        &recipe(&["build", "--locked"])
    );
    assert!(authored_job("build").needs.is_empty(), "build is the DAG root");
    assert_eq!(
        authored_job("test").build.as_ref().unwrap(),
        &recipe(&["test", "--locked", "--lib"])
    );
    assert_eq!(authored_job("test").needs, ["build"].map(String::from));
    assert_eq!(
        authored_job("clippy").build.as_ref().unwrap(),
        &recipe(&["clippy", "--locked", "--all-targets", "--", "-D", "warnings"])
    );
    assert_eq!(authored_job("clippy").needs, ["build"].map(String::from));

    // ---- plan_dispatch arms the run through the real CAS path ----
    let blobs = FsBlobStore::new();
    let armed = match plan_dispatch(&push_envelope(), &CheckedInRepo, &blobs) {
        DispatchOutcome::Arm(armed) => armed,
        DispatchOutcome::Skip(reason) => panic!("founder push did not arm: {reason:?}"),
    };
    assert_eq!(armed.handoff.run_write.trigger_kind, "push");

    // One queued `ci.check.updated{state: queued}` per top-level job, in authored order.
    let queued_contexts: Vec<serde_json::Value> = armed
        .handoff
        .queued_checks
        .iter()
        .map(|d| d.payload["context"].clone())
        .collect();
    assert_eq!(
        queued_contexts,
        vec![
            serde_json::json!({"provider": "ci", "name": "build"}),
            serde_json::json!({"provider": "ci", "name": "test"}),
            serde_json::json!({"provider": "ci", "name": "clippy"}),
        ],
        "exactly the three build/test/clippy check contexts are queued"
    );

    // ---- the resolved snapshot the runner consumes (persisted via CAS) ----
    let snapshot_ref = &armed.handoff.run_write.definition_snapshot.0;
    let snapshot_hash = ContentHash::parse(
        snapshot_ref
            .rsplit('/')
            .next()
            .expect("snapshot ref has a hash segment"),
    )
    .expect("snapshot ref carries a content hash");
    let snapshot_bytes = blobs
        .get(&TenantId("myelin".into()), &snapshot_hash)
        .expect("planner persisted the resolved snapshot");
    let snapshot = myelin_ci_controlplane::decode_resolved_run_plan(&snapshot_bytes)
        .expect("persisted snapshot decodes");
    let resolved = snapshot.as_v2().expect("the snapshot remains V2");
    assert_eq!(
        resolved.execution.profile,
        myelin_ci_controlplane::CiExecutionProfileV1::LinuxBuildV1
    );

    // Resolved jobs are sorted strictly by name: build, clippy, test.
    let resolved_names: Vec<String> = resolved.jobs.iter().map(|j| j.name.clone()).collect();
    assert_eq!(resolved_names, ["build", "clippy", "test"].map(String::from));

    let resolved_job = |n: &str| resolved.jobs.iter().find(|j| j.name == n).unwrap();
    for n in ["build", "test", "clippy"] {
        let j = resolved_job(n);
        assert_eq!(j.stage, n, "the authored stage name survives resolution");
        assert_eq!(j.image, ROOTFS_IMAGE);
        assert!(
            j.command.is_empty(),
            "the structured recipe lives in `build`, not a raw argv"
        );
    }
    assert_eq!(
        resolved_job("build").build.as_ref().unwrap(),
        &recipe(&["build", "--locked"])
    );
    assert_eq!(
        resolved_job("test").build.as_ref().unwrap(),
        &recipe(&["test", "--locked", "--lib"])
    );
    assert_eq!(
        resolved_job("clippy").build.as_ref().unwrap(),
        &recipe(&["clippy", "--locked", "--all-targets", "--", "-D", "warnings"])
    );

    // The DAG edges survive resolution (no matrix, so needs are the authored names).
    assert!(resolved_job("build").needs.is_empty());
    assert_eq!(resolved_job("test").needs, ["build"].map(String::from));
    assert_eq!(resolved_job("clippy").needs, ["build"].map(String::from));

    // Every Cargo build job is stamped with the server-trusted vendor selected from THIS repo's root
    // Cargo.lock SHA-256 (a fail-closed match against the registered workspace vendor) — the runner
    // mounts that read-only tree for the `--locked`, offline build.
    let workspace_vendor = myelin_ci_sandbox::cargo_vendor_workspace_reference();
    for n in ["build", "test", "clippy"] {
        assert_eq!(
            resolved_job(n).selected_cargo_vendor.as_deref(),
            Some(workspace_vendor.as_str()),
            "job `{n}` binds the registered workspace cargo vendor"
        );
    }

    // The exact cargo argv the sandbox executes: `cargo` is prepended and the two platform vendor
    // `--config` pairs are injected before any `--` driver separator.
    let build_argv = resolved_job("build").build.as_ref().unwrap().platform_argv();
    assert!(build_argv.starts_with(&["cargo", "build", "--locked"].map(String::from)));
    assert_eq!(
        build_argv.iter().filter(|a| a.as_str() == "--config").count(),
        2,
        "the platform injects its two vendor --config pairs"
    );

    let clippy_argv = resolved_job("clippy")
        .build
        .as_ref()
        .unwrap()
        .platform_argv();
    assert!(clippy_argv.starts_with(&["cargo", "clippy", "--locked", "--all-targets"].map(String::from)));
    assert!(
        clippy_argv.ends_with(&["--", "-D", "warnings"].map(String::from)),
        "the `-- -D warnings` compiler-driver tail stays after the injected --config pairs"
    );
}
