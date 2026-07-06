//! **CI-P11 (P-354, M4) LIVE integration: the definition CAS snapshot against the REAL object store
//! (RustFS).**
//!
//! The binding data-layer policy: a contract that touches the object store (11.2 `BlobStore` — the
//! CAS definition snapshot, arch 02 §1.4) ships a REAL integration test green against the live dev
//! stack, NOT a mock. The resolver writes the resolved, digest-pinned, matrix-expanded DAG through
//! `myelin_storage::s3blob::S3BlobStore` (the dev<->prod backing SWAP — `resolve_snapshot` takes
//! `&dyn BlobStore`, so the object-store backing is a CONFIG swap, never a code change) and:
//!  1. the snapshot is content-addressed into the REAL bucket (put through the trait);
//!  2. the SAME bytes are read back at the SAME content address (the run's reproducible definition);
//!  3. the address equals the BLAKE3 of the canonical bytes (reproducible — `myelin ci plan` parity);
//!  4. a FLOATING TAG is still rejected fail-closed BEFORE any byte hits the bucket (0 un-digested
//!     references reach a snapshot — the supply-chain control holds with the real store underneath).
//!
//! Bring the stack up with `docker compose -f docker-compose.dev.yml up -d --wait`, then run
//! `cargo test -p myelin-ci-dispatch --features integration`.

#![cfg(feature = "integration")]

use myelin_ci_dispatch::resolve::{resolve_snapshot, CiDefinition, JobDef, ResolveError};
use myelin_ci_dispatch::OnTrigger;
use myelin_config::MyelinConfig;
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;

const PINNED_BUILD: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
const PINNED_TEST: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

/// **The CAS definition snapshot round-trips through the REAL object store + a floating tag is
/// refused before any byte lands.**
#[tokio::test]
async fn cas_snapshot_round_trips_through_real_object_store_and_floating_tag_is_refused() {
    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-ci-p11-{}", std::process::id()));

    // A matrix definition: 2 os × 2 rust over `test`, plus a `build` it needs — 5 resolved instances.
    let def = CiDefinition {
        on: OnTrigger::PullRequest,
        jobs: vec![
            JobDef::normal("build", PINNED_BUILD),
            JobDef::normal("test", PINNED_TEST)
                .with_needs(["build"])
                .with_matrix("os", vec!["linux".into(), "macos".into()])
                .with_matrix("rust", vec!["stable".into(), "beta".into()]),
        ],
    };

    // The sync BlobStore trait drives the async S3 SDK via block_in_place; run the whole flow on a
    // blocking thread (the same posture as the storage integration tests).
    let (snapshot_bytes, address, got) = {
        let handle = handle.clone();
        let tenant = tenant.clone();
        let def = def.clone();
        let cfg = cfg.clone();
        tokio::task::spawn_blocking(move || {
            let store = S3BlobStore::connect(&cfg.s3, handle.clone());

            // (1) resolve + content-address into the REAL bucket.
            let (snap, address) = resolve_snapshot(&def, &store, &tenant)
                .expect("a digest-pinned def resolves + writes the CAS snapshot to RustFS");
            assert_eq!(
                snap.jobs.len(),
                5,
                "build + (2 os × 2 rust) = 5 resolved instances"
            );

            // (2) read the SAME bytes back at the SAME content address.
            let got = store
                .get(&tenant, &address)
                .expect("the CAS snapshot blob is present in the real bucket");

            (snap.canonical_bytes(), address, got)
        })
        .await
        .expect("blocking CAS-snapshot task")
    };

    assert_eq!(
        got, snapshot_bytes,
        "the snapshot read back from the REAL object store is byte-identical to what was written"
    );
    // (3) reproducible: the address IS the BLAKE3 of the canonical bytes (`myelin ci plan` parity).
    assert_eq!(
        address,
        ContentHash::blake3(&snapshot_bytes),
        "the snapshot address is the BLAKE3 content address (reproducible)"
    );

    // (4) a FLOATING TAG is refused fail-closed — and BEFORE any byte hits the bucket (the
    // digest-pin check runs in resolve_snapshot prior to the `put`). 0 un-digested references reach a
    // snapshot, even with the real store underneath.
    let floating = CiDefinition {
        on: OnTrigger::Push,
        jobs: vec![JobDef::normal("build", "alpine:3")],
    };
    let err = {
        let handle = handle.clone();
        let tenant = tenant.clone();
        let cfg = cfg.clone();
        tokio::task::spawn_blocking(move || {
            let store = S3BlobStore::connect(&cfg.s3, handle.clone());
            resolve_snapshot(&floating, &store, &tenant).expect_err(
                "a floating tag must be rejected fail-closed against the real store too",
            )
        })
        .await
        .expect("blocking floating-tag task")
    };
    assert!(
        matches!(&err, ResolveError::FloatingTag { reference, .. } if reference == "alpine:3"),
        "the floating tag is refused fail-closed (0 un-digested references reach the bucket): {err:?}"
    );

    // Clean up the snapshot probe object via a raw client.
    let creds = aws_sdk_s3::config::Credentials::new(
        &cfg.s3.access_key,
        &cfg.s3.secret_key,
        None,
        None,
        "myelin-dev",
    );
    let conf = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(cfg.s3.region.clone()))
        .endpoint_url(&cfg.s3.endpoint)
        .force_path_style(cfg.s3.force_path_style)
        .credentials_provider(creds)
        .build();
    let raw = aws_sdk_s3::Client::from_conf(conf);
    let digest = &address.digest_hex;
    let (fan, rest) = digest.split_at(2);
    let key = format!("{}/{}/{}/{}", tenant.0, address.algo.tag(), fan, rest);
    let _ = raw
        .delete_object()
        .bucket(&cfg.s3.bucket)
        .key(&key)
        .send()
        .await;
}
