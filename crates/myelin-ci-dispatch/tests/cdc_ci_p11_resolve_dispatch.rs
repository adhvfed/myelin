use std::sync::Arc;

use myelin_ci_dispatch::resolve::{
    reserve_and_start, resolve_snapshot, snapshot_ref, CheckContext, CiDefinition, JobDef,
    ResolvedJob, ResolvedSnapshot, RunFacts, CI_PIPELINE_WF_TYPE,
};
use myelin_ci_dispatch::{stamp_trust, OnTrigger, RunProvenance};
use myelin_events::{EventId, IdMinter, MonotonicMinter};
use myelin_flow::{DurableExecutor, FlowExecutor, StartSpec};
use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
use myelin_tenancy::{Region, TenantId};

const PINNED_BUILD: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
const PINNED_TEST: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn definition() -> CiDefinition {
    CiDefinition {
        on: OnTrigger::PullRequest,
        jobs: vec![
            JobDef::normal("build", PINNED_BUILD, ["build"]),
            JobDef::normal("test", PINNED_TEST, ["test"]).with_needs(["build"]),
        ],
    }
}

fn facts(snapshot_run_id: &str) -> RunFacts {
    RunFacts {
        run_id: snapshot_run_id.into(),
        tenant_id: "acme".into(),
        repo_ref: "myelin://acme/git/repo/web".into(),
        source_ref: None,
        commit_oid: "deadbeef".into(),
        contexts: vec![CheckContext::ci("build"), CheckContext::ci("test/unit")],
        cause_event_id: EventId("ev-pr-1".into()),
        started_at: "2026-07-23T00:00:00Z".into(),
    }
}

#[test]
fn cdc_11_2_consumer_snapshot_is_content_addressed_through_blobstore() {
    let store = FsBlobStore::new();
    let def = definition();

    let (snap, addr): (ResolvedSnapshot, ContentHash) =
        resolve_snapshot(&def, &store, &tenant()).expect("a digest-pinned def resolves");
    let _: &ResolvedJob = &snap.jobs[0];

    let bytes = store
        .get(&tenant(), &addr)
        .expect("the CAS blob is present");
    assert_eq!(bytes, snap.canonical_bytes().unwrap(), "get returns the put bytes");
    assert_eq!(addr, ContentHash::blake3(&snap.canonical_bytes().unwrap()));

    let (_snap2, addr2) =
        resolve_snapshot(&def, &FsBlobStore::new(), &tenant()).expect("re-resolves");
    assert_eq!(
        addr, addr2,
        "same definition → same content address (reproducible)"
    );

    let r = snapshot_ref(&tenant(), &addr);
    assert!(
        r.0.contains(&addr.to_multihash_string()),
        "the snapshot ref carries the content address"
    );
}

#[test]
fn cdc_9_1_consumer_reserve_start_is_idempotent_through_durable_executor() {
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let executor = FlowExecutor::new(minter, tenant(), Region("fr-par".into()));
    executor.register_definition(CI_PIPELINE_WF_TYPE);

    let store = FsBlobStore::new();
    let (_snap, addr) = resolve_snapshot(&definition(), &store, &tenant()).expect("resolves");
    let snap_ref = snapshot_ref(&tenant(), &addr);

    let stamp = stamp_trust(&RunProvenance {
        is_fork: true,
        targets_self_hosted: false,
        read_excludes_fork: false,
    });
    let handoff = reserve_and_start(
        &snap_ref,
        &stamp,
        &OnTrigger::PullRequest,
        &facts("run-9001"),
    );

    let spec: StartSpec = handoff.start_spec.clone();
    assert_eq!(spec.wf_type, CI_PIPELINE_WF_TYPE);
    assert_eq!(spec.input, vec![snap_ref.clone()]);

    let run = executor.start(spec.clone()).expect("ci.pipeline starts");

    let run_again = executor
        .start(spec)
        .expect("re-start under the same idem_key");
    assert_eq!(
        run, run_again,
        "a re-delivered trigger is ONE run (9.1 idempotency)"
    );

    let input = executor.run_input(&run).expect("the started run's input");
    assert_eq!(
        input,
        vec![snap_ref],
        "started on the snapshot ref (references-not-payloads)"
    );

    assert!(handoff.is_atomic_bundle());
}
