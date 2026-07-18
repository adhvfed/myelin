//! **CDC — the CI Trigger & Dispatch CONSUMER side of the resolve/start seam (CI-P11 / P-354, M4).**
//!
//! The contract-coverage scanner requires every covered row to name a CDC file carrying BOTH a
//! provider-side and a consumer-side marker. This file is the **CONSUMER** half for the two rows
//! CI-P11's definition-resolution → CAS-snapshot + reserve/start handoff CONSUMES:
//!
//!   - **11.2** — the content-addressed `BlobStore{put, get, head, delete}`. PROVIDER:
//!     `myelin-storage` (the frozen BLAKE3 content-addressed trait + its `FsBlobStore` floor).
//!     CONSUMER: CI dispatch — [`resolve_snapshot`] serialises the resolved DAG to canonical JSON and
//!     `put`s it as a T2 CAS blob; the returned address IS the run's reproducible definition ref. The
//!     provider's content address round-trips (the consumer reads the SAME bytes back) and is
//!     reproducible (the same definition → the same address).
//!
//!   - **9.1** — the `DurableExecutor{start, ..}` + `StartSpec`. PROVIDER: `myelin-flow` (the frozen
//!     engine-agnostic durable-execution control surface + its `FlowExecutor`). CONSUMER: CI dispatch
//!     — [`reserve_and_start`] builds the `StartSpec{ wf_type: "ci.pipeline", input: [snapshot_ref],
//!     idem_key }`; the provider `start`s the `ci.pipeline` workflow on the snapshot ref, and a
//!     re-delivered trigger (the SAME idem_key) is ONE run, not two (the 9.1 idempotency).
//!
//! These drive the REAL providers (`FsBlobStore`, `FlowExecutor`) through the CI-P11 consumer — no
//! DB, no network (the live-stack CAS-snapshot object-store round-trip is the named integration
//! drill). The provider-side CDCs (`cdc_st_*` / `cdc_9_1_executor`) prove the other half.

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
        repo_ref: "myelin://acme/git/repo/web".into(),
        commit_oid: "deadbeef".into(),
        contexts: vec![CheckContext::ci("build"), CheckContext::ci("test/unit")],
        cause_event_id: EventId("ev-pr-1".into()),
    }
}

/// **11.2 CONSUMER — the resolved DAG is written as a content-addressed CAS blob through the frozen
/// `BlobStore`, and the address round-trips + is reproducible.** The PROVIDER (`myelin-storage`) owns
/// the BLAKE3 content-addressed `put`/`get`; the CONSUMER (CI dispatch) writes the snapshot and reads
/// the SAME bytes back at the SAME address (the run's reproducible, auditable definition).
#[test]
fn cdc_11_2_consumer_snapshot_is_content_addressed_through_blobstore() {
    let store = FsBlobStore::new();
    let def = definition();

    // The consumer resolves + content-addresses through the provider trait.
    let (snap, addr): (ResolvedSnapshot, ContentHash) =
        resolve_snapshot(&def, &store, &tenant()).expect("a digest-pinned def resolves");
    let _: &ResolvedJob = &snap.jobs[0];

    // PROVIDER round-trip: the bytes at the address ARE the snapshot's canonical bytes.
    let bytes = store
        .get(&tenant(), &addr)
        .expect("the CAS blob is present");
    assert_eq!(bytes, snap.canonical_bytes().unwrap(), "get returns the put bytes");
    // The address IS the BLAKE3 content address of those bytes (content-addressed by construction).
    assert_eq!(addr, ContentHash::blake3(&snap.canonical_bytes().unwrap()));

    // REPRODUCIBLE: re-resolving the SAME definition into a FRESH store yields the SAME address.
    let (_snap2, addr2) =
        resolve_snapshot(&def, &FsBlobStore::new(), &tenant()).expect("re-resolves");
    assert_eq!(
        addr, addr2,
        "same definition → same content address (reproducible)"
    );

    // The snapshot ref the run carries is the Refs-rooted handle over that address.
    let r = snapshot_ref(&tenant(), &addr);
    assert!(
        r.0.contains(&addr.to_multihash_string()),
        "the snapshot ref carries the content address"
    );
}

/// **9.1 CONSUMER — the reserve/start handoff starts the `ci.pipeline` workflow on the snapshot ref
/// through the frozen `DurableExecutor`, and a re-delivered trigger is ONE run (idempotent on
/// idem_key).** The PROVIDER (`myelin-flow`'s `FlowExecutor`) owns `start`; the CONSUMER (CI dispatch)
/// builds the `StartSpec` and the provider returns the SAME `RunId` on a re-`start`.
#[test]
fn cdc_9_1_consumer_reserve_start_is_idempotent_through_durable_executor() {
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let executor = FlowExecutor::new(minter, tenant(), Region("fr-par".into()));
    // The ci.pipeline workflow body is CI-P15 (myelin_flow::ci_pipeline); the executor registers the
    // type so `start` resolves it.
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

    // The CONSUMER's StartSpec names the ci.pipeline workflow + starts on the snapshot ref.
    let spec: StartSpec = handoff.start_spec.clone();
    assert_eq!(spec.wf_type, CI_PIPELINE_WF_TYPE);
    assert_eq!(spec.input, vec![snap_ref.clone()]);

    // PROVIDER start: the durable executor starts the run on the snapshot ref.
    let run = executor.start(spec.clone()).expect("ci.pipeline starts");

    // IDEMPOTENT: a re-delivered trigger (the SAME idem_key) is the SAME run, not a second one.
    let run_again = executor
        .start(spec)
        .expect("re-start under the same idem_key");
    assert_eq!(
        run, run_again,
        "a re-delivered trigger is ONE run (9.1 idempotency)"
    );

    // The run was started on the references-not-payloads snapshot ref (never a body).
    let input = executor.run_input(&run).expect("the started run's input");
    assert_eq!(
        input,
        vec![snap_ref],
        "started on the snapshot ref (references-not-payloads)"
    );

    // The atomic bundle the consumer commits alongside the start is all-or-nothing (no partial run).
    assert!(handoff.is_atomic_bundle());
}
