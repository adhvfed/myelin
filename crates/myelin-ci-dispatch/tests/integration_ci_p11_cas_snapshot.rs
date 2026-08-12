#![cfg(feature = "integration")]

use myelin_ci_dispatch::resolve::{resolve_snapshot, CiDefinition, JobDef, ResolveError};
use myelin_ci_dispatch::OnTrigger;
use myelin_config::MyelinConfig;
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;

const PINNED_BUILD: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
const PINNED_TEST: &str =
    "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

#[tokio::test]
async fn cas_snapshot_round_trips_through_real_object_store_and_floating_tag_is_refused() {
    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-ci-p11-{}", std::process::id()));

    let def = CiDefinition {
        on: OnTrigger::PullRequest,
        jobs: vec![
            JobDef::normal("build", PINNED_BUILD, ["build"]),
            JobDef::normal("test", PINNED_TEST, ["test"])
                .with_needs(["build"])
                .with_matrix("os", vec!["linux".into(), "macos".into()])
                .with_matrix("rust", vec!["stable".into(), "beta".into()]),
        ],
    };

    let (snapshot_bytes, address, got) = {
        let handle = handle.clone();
        let tenant = tenant.clone();
        let def = def.clone();
        let cfg = cfg.clone();
        tokio::task::spawn_blocking(move || {
            let store = S3BlobStore::connect(&cfg.s3, handle.clone());

            let (snap, address) = resolve_snapshot(&def, &store, &tenant)
                .expect("a digest-pinned def resolves + writes the CAS snapshot to RustFS");
            assert_eq!(
                snap.jobs.len(),
                5,
                "build + (2 os × 2 rust) = 5 resolved instances"
            );

            let got = store
                .get(&tenant, &address)
                .expect("the CAS snapshot blob is present in the real bucket");

            (snap.canonical_bytes().unwrap(), address, got)
        })
        .await
        .expect("blocking CAS-snapshot task")
    };

    assert_eq!(
        got, snapshot_bytes,
        "the snapshot read back from the REAL object store is byte-identical to what was written"
    );
    assert_eq!(
        address,
        ContentHash::blake3(&snapshot_bytes),
        "the snapshot address is the BLAKE3 content address (reproducible)"
    );

    let floating = CiDefinition {
        on: OnTrigger::Push,
        jobs: vec![JobDef::normal("build", "alpine:3", ["build"])],
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
