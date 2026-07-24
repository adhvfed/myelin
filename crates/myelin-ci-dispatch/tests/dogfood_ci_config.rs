//! The checked-in founder pipeline must remain an armed, resolvable V2 request.
//!
//! This reads the real repository file rather than a copied fixture so a malformed, floating-image,
//! renamed-check, or marker-losing edit cannot silently leave the live acceptance run untriggerable.

use myelin_ci_dispatch::{
    parse_versioned_ci_config, plan_dispatch, CiPlanContract, DispatchOutcome, GitConfigReader,
    GitReadError, OnTrigger,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_tenancy::{Region, TenantId};
use std::fs;
use std::path::{Path, PathBuf};

const MARKER: &str = "MYELIN-CI-a005e32fc1bb0c2b64e7d40ac1a01236";
const ROOTFS_IMAGE: &str = "myelin.local/linux-small-v1-rootfs@sha256:f9bd3926a7b47e1dd4729e5788d40dc6daf4ce159a91db169ef5bb803e73ec1f";
const OID: &str = "dddddddddddddddddddddddddddddddddddddddd";
const COMMAND: [&str; 3] = [
    "/bin/sh",
    "-c",
    "printf '%s\\n' 'MYELIN-CI-a005e32fc1bb0c2b64e7d40ac1a01236'",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("dispatch is under workspace/crates")
        .to_path_buf()
}

struct CheckedInConfig(Vec<u8>);

impl GitConfigReader for CheckedInConfig {
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
        Ok((path == ".myelin/ci.toml").then(|| self.0.clone()))
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
fn checked_in_founder_pipeline_is_armed_resolvable_and_emits_the_receipt_marker_once() {
    let bytes = fs::read(workspace_root().join(".myelin/ci.toml"))
        .expect("the founder pipeline must remain checked in");
    let definition =
        parse_versioned_ci_config(&bytes, ".myelin/ci.toml").expect("founder config parses");
    match &definition.contract {
        CiPlanContract::V2(execution) => assert_eq!(
            execution.profile,
            myelin_ci_controlplane::CiExecutionProfileV1::LinuxSmallV1
        ),
        CiPlanContract::V1 => panic!("founder pipeline must remain a V2 execution request"),
    }
    assert_eq!(definition.on, OnTrigger::Push);
    assert_eq!(definition.jobs.len(), 1);
    let job = &definition.jobs[0];
    assert_eq!(job.name, "build", "the required check context stays build");
    assert_eq!(job.image, ROOTFS_IMAGE, "the staged rootfs pin is explicit");
    assert_eq!(job.command, COMMAND);

    let blobs = FsBlobStore::new();
    let armed = match plan_dispatch(&push_envelope(), &CheckedInConfig(bytes), &blobs) {
        DispatchOutcome::Arm(armed) => armed,
        DispatchOutcome::Skip(reason) => panic!("founder push did not arm: {reason:?}"),
    };
    assert_eq!(armed.handoff.run_write.trigger_kind, "push");
    assert_eq!(armed.handoff.queued_checks.len(), 1);
    assert_eq!(
        armed.handoff.queued_checks[0].payload["context"],
        serde_json::json!({"provider": "ci", "name": "build"})
    );

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
    assert_eq!(resolved.jobs.len(), 1);
    assert_eq!(resolved.jobs[0].stage, "build");
    assert_eq!(resolved.jobs[0].image, ROOTFS_IMAGE);
    assert_eq!(resolved.jobs[0].command, COMMAND);
    assert_eq!(
        resolved.jobs[0].command.join("\0").matches(MARKER).count(),
        1,
        "the exact acceptance marker survives the production dispatch/CAS path once"
    );
}
