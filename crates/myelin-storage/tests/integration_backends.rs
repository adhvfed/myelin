//! Live-backend integration tests (Stage 1 / infra).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. These run ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test integration_backends -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the
//! same tests run against Scaleway by exporting the prod env vars.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;

/// Proves the Postgres OLTP backend is reachable through DATABASE_URL and that the RLS
/// conventions from the init script are in place: the app role is NOT superuser / NOT
/// bypassrls, and the myelin_make_tenant_scoped helper exists.
#[tokio::test]
async fn postgres_oltp_reachable_and_rls_ready() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres (is the stack up?)");

    // RUNTIME query (sqlx::query — NOT the compile-time query! macro), so the build is DB-free.
    let row = sqlx::query(
        "SELECT current_database() AS db, \
                (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) AS super, \
                (SELECT rolbypassrls FROM pg_roles WHERE rolname = current_user) AS bypass",
    )
    .fetch_one(&pool)
    .await
    .expect("query Postgres");

    let db: String = row.get("db");
    let is_super: bool = row.get("super");
    let bypass: bool = row.get("bypass");
    assert_eq!(db, "myelin");
    // The app role must NOT silently bypass RLS — the load-bearing isolation property.
    assert!(!is_super, "app role must not be superuser");
    assert!(!bypass, "app role must not have BYPASSRLS");

    // The RLS convention helper installed by the init script is callable.
    let helper: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_proc WHERE proname = 'myelin_make_tenant_scoped'",
    )
    .fetch_one(&pool)
    .await
    .expect("query for RLS helper");
    assert_eq!(helper, 1, "myelin_make_tenant_scoped RLS helper must exist");
}

/// Proves a (tenant, region) RLS policy actually isolates rows end-to-end against real
/// Postgres: a session set to tenant A sees ONLY tenant A's row. Exercises the
/// myelin_make_tenant_scoped convention on a throwaway table.
#[tokio::test]
async fn postgres_rls_isolates_tenants() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres");

    // A unique table name so concurrent runs don't collide.
    let tbl = format!("rls_probe_{}", std::process::id());
    // Owner-side DDL: the app role was granted default privileges, but creating the table as
    // the admin owner is what production migrations do. Here the app role creates it (it has
    // CREATE on public by default in PG16? no — so use admin via a separate connection).
    // We run DDL with the same app pool; PG16 revokes public CREATE, so grant it for the test.
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin");

    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (id int, tenant_id text, region text, body text)"
    ))
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();
    // Seed two tenants' rows (as admin, who is FORCEd under RLS too — so set the GUCs).
    for (id, t) in [(1, "tenantA"), (2, "tenantB")] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(t)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} (id, tenant_id, region, body) VALUES ($1, $2, 'fr-par', 'x')"
        ))
        .bind(id)
        .bind(t)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    // As the app role with tenant_id=tenantA, only tenantA's row is visible.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let rows = sqlx::query(&format!("SELECT tenant_id FROM {tbl}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "RLS must hide the other tenant's row");
    let seen: String = rows[0].get("tenant_id");
    assert_eq!(seen, "tenantA");

    // Cleanup.
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

/// Proves the S3-compatible object store (RustFS) is reachable through the S3_* config with a
/// custom endpoint + path-style addressing: put an object, get it back, head it, delete it.
#[tokio::test]
async fn rustfs_s3_blob_roundtrip() {
    use aws_sdk_s3::primitives::ByteStream;

    let cfg = MyelinConfig::dev();
    let s3 = &cfg.s3;

    let creds = aws_sdk_s3::config::Credentials::new(
        &s3.access_key,
        &s3.secret_key,
        None,
        None,
        "myelin-dev",
    );
    let conf = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(s3.region.clone()))
        .endpoint_url(&s3.endpoint)
        .force_path_style(s3.force_path_style)
        .credentials_provider(creds)
        .build();
    let client = aws_sdk_s3::Client::from_conf(conf);

    let key = format!("integration/probe-{}.bin", std::process::id());
    let body = b"myelin-stage1-blob".to_vec();

    client
        .put_object()
        .bucket(&s3.bucket)
        .key(&key)
        .body(ByteStream::from(body.clone()))
        .send()
        .await
        .expect("put_object to RustFS (is the stack up + bucket created?)");

    let got = client
        .get_object()
        .bucket(&s3.bucket)
        .key(&key)
        .send()
        .await
        .expect("get_object");
    let bytes = got.body.collect().await.expect("collect body").into_bytes();
    assert_eq!(bytes.as_ref(), body.as_slice());

    let head = client
        .head_object()
        .bucket(&s3.bucket)
        .key(&key)
        .send()
        .await
        .expect("head_object");
    assert_eq!(head.content_length(), Some(body.len() as i64));

    client
        .delete_object()
        .bucket(&s3.bucket)
        .key(&key)
        .send()
        .await
        .expect("delete_object");
}

/// **P-ST-22 (P-252) LIVE integration: the git pack tier against the REAL object store (RustFS).**
///
/// The local-disk-floor git pack tier rides the `BlobStore` trait (§3.5), so the object-store
/// backing (`S3BlobStore`) is a backing SWAP — `GitPackTier<S3BlobStore>` is the same tier with the
/// object store underneath, NO code change (the relocatability §3.5 decides now). This proves the
/// seam against the live RustFS dev stack (the binding policy: a DB/object-store contract ships a
/// real integration test green against the live stack — NOT a mock):
///  1. a git loose object is put + got THROUGH the trait against the real bucket (content round-trip);
///  2. the object's git SHA address is content-derived, not a node path (relocation-stable);
///  3. STOR-D7 on packs: the real object store re-hashes on read and REFUSES a corrupt object
///     (0 silent serve) — proven by overwriting the underlying S3 object with wrong bytes.
#[tokio::test]
async fn git_pack_tier_over_real_object_store_roundtrips_and_detects_corruption() {
    use myelin_storage::s3blob::S3BlobStore;
    use myelin_storage::{
        BlobError, GitObjectKind, GitPackError, GitPackTier, RepoGitPlacement, RepoId,
        RepoPlacementStatus, StorageGroup,
    };
    use myelin_tenancy::{Region, TenantId};

    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let bucket = cfg.s3.bucket.clone();
    // A raw client for the corruption step (overwrite the stored object's bytes out-of-band).
    let raw = {
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
        aws_sdk_s3::Client::from_conf(conf)
    };

    // The git pack tier + the corruption probe both run on a blocking thread (the sync BlobStore
    // trait drives the async SDK via block_in_place; the corruption overwrite uses the raw client).
    let tenant = TenantId(format!("itest-git-{}", std::process::id()));
    let repo = RepoId::from_token("web");
    let content = b"fn main() { println!(\"git pack over real object store\"); }\n".to_vec();

    // The whole flow runs on ONE blocking thread so the tier (and its in-memory git-SHA → native
    // index) stays alive across put → clean get → out-of-band corrupt → refused get. The raw S3
    // overwrite (the corruption) is driven on the runtime handle via `block_on`.
    let (refused, native_key) = {
        let handle = handle.clone();
        let tenant = tenant.clone();
        let repo = repo.clone();
        let content = content.clone();
        let bucket = bucket.clone();
        let raw = raw.clone();
        tokio::task::spawn_blocking(move || {
            let tier = GitPackTier::new(
                tenant.clone(),
                S3BlobStore::connect(&cfg.s3, handle.clone()),
            );
            tier.place_repo(
                repo.clone(),
                RepoGitPlacement {
                    group: StorageGroup::from_token("pack-0"),
                    region: Region::new(&cfg.s3.region),
                    status: RepoPlacementStatus::Active,
                },
            );
            // (1) put + (2) get THROUGH the trait against the REAL bucket.
            let address = tier
                .put_object(&repo, GitObjectKind::Blob, &content)
                .expect("put object");
            let got = tier.get_object(&repo, &address).expect("get object");
            assert_eq!(
                got, content,
                "git object round-trips through the real object store"
            );

            // The native (BLAKE3) key the framed object is stored under; reconstruct its S3 key.
            let native = tier
                .native_addr_for_test(&repo, &address)
                .expect("native addr");
            let dh = &native.digest_hex;
            let (fan, rest) = dh.split_at(2);
            let native_key = format!("{}/{}/{}/{}", tenant.0, native.algo.tag(), fan, rest);

            // (3) STOR-D7 on packs over the REAL store: overwrite the stored object with WRONG bytes
            // OUT-OF-BAND (the raw client, on the runtime), then prove the SAME tier's
            // re-hash-on-read REFUSES it (0 silent serve).
            handle.block_on(async {
                raw.put_object()
                    .bucket(&bucket)
                    .key(&native_key)
                    .body(aws_sdk_s3::primitives::ByteStream::from(
                        b"corrupted bytes".to_vec(),
                    ))
                    .send()
                    .await
                    .expect("overwrite the stored object with corrupt bytes");
            });

            let r = tier.get_object(&repo, &address);
            let refused = matches!(r, Err(GitPackError::Blob(BlobError::IntegrityFail { .. })));
            (refused, native_key)
        })
        .await
        .expect("blocking git pack tier task")
    };
    assert!(
        refused,
        "STOR-D7 on packs over the REAL object store: a corrupt git object MUST be refused \
         (0 silent serve), never served silently"
    );

    // Clean up the probe object.
    let _ = raw
        .delete_object()
        .bucket(&bucket)
        .key(&native_key)
        .send()
        .await;
}

/// **P-ST-30 (P-441) LIVE integration: the object-store BlobStore replica recovery against the
/// REAL object store (RustFS).**
///
/// The fs→object swap is a BACKING change: [`ReplicatedBlobStore`] fronts a PRIMARY
/// [`S3BlobStore`] with a REPLICA [`S3BlobStore`] (a second RustFS bucket), all behind the
/// UNCHANGED `BlobStore` trait. This proves the STOR-D7 "recover from a replica" property on the
/// REAL object store (the binding policy: an object-store contract ships a real integration test
/// green against the live stack — NOT a mock):
///  1. a put writes the SAME content-addressed bytes to the primary AND replica buckets;
///  2. a clean get round-trips through the trait against the real buckets;
///  3. the PRIMARY object is corrupted OUT-OF-BAND (overwritten with wrong bytes via the raw
///     client) → the get re-hashes, detects the mismatch, RECOVERS the correct bytes from the
///     REPLICA bucket (0 silent serve), heals the primary, and `blob_recovered_from_replica`
///     fires;
///  4. a second get serves cleanly from the HEALED primary (no further recovery).
#[tokio::test]
async fn replicated_object_store_recovers_corrupt_primary_from_replica() {
    use myelin_storage::s3blob::S3BlobStore;
    use myelin_storage::{BlobStore, ReplicatedBlobStore};
    use myelin_tenancy::TenantId;

    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();

    // The replica lives in a SECOND bucket on the same RustFS (a distinct backing). Create it if
    // absent (idempotent). The primary bucket is the dev default (already created by the stack).
    let primary_bucket = cfg.s3.bucket.clone();
    let replica_bucket = format!("{}-replica", cfg.s3.bucket);

    let raw = {
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
        aws_sdk_s3::Client::from_conf(conf)
    };
    // Idempotent replica-bucket create (ignore AlreadyOwned/AlreadyExists).
    let _ = raw.create_bucket().bucket(&replica_bucket).send().await;

    let tenant = TenantId(format!("itest-repl-{}", std::process::id()));
    let content = b"replicated-object-store-trustworthy-bytes".to_vec();

    let primary_s3 = cfg.s3.clone();
    let mut replica_s3 = cfg.s3.clone();
    replica_s3.bucket = replica_bucket.clone();

    let (recovered_ok, healed_ok, native_key) = {
        let handle = handle.clone();
        let tenant = tenant.clone();
        let content = content.clone();
        let primary_bucket = primary_bucket.clone();
        let raw = raw.clone();
        tokio::task::spawn_blocking(move || {
            // Primary + replica S3 backings behind the UNCHANGED trait, fronted by the replicated
            // store — the fs→object swap is the inner backing, the recovery code is unchanged.
            let store = ReplicatedBlobStore::new(
                S3BlobStore::connect(&primary_s3, handle.clone()),
                vec![S3BlobStore::connect(&replica_s3, handle.clone())],
            );

            // (1)+(2) put writes both buckets; a clean get round-trips through the trait.
            let h = store.put(&tenant, &content).expect("replicated put");
            assert_eq!(store.get(&tenant, &h).expect("clean get"), content);

            // The S3 key the bytes are stored under (same key in both buckets — content-addressed).
            let dh = &h.digest_hex;
            let (fan, rest) = dh.split_at(2);
            let native_key = format!("{}/{}/{}/{}", tenant.0, h.algo.tag(), fan, rest);

            // (3) Corrupt ONLY the PRIMARY bucket's object out-of-band (the raw client).
            handle.block_on(async {
                raw.put_object()
                    .bucket(&primary_bucket)
                    .key(&native_key)
                    .body(aws_sdk_s3::primitives::ByteStream::from(
                        b"CORRUPTED-PRIMARY-BYTES".to_vec(),
                    ))
                    .send()
                    .await
                    .expect("overwrite the primary object with corrupt bytes");
            });

            // The get RECOVERS the correct bytes from the replica bucket (0 silent serve).
            let recovered = store.get(&tenant, &h).expect("recovered from replica");
            let recovered_ok =
                recovered == content && store.telemetry().blob_recovered_from_replica() == 1;

            // (4) The primary was HEALED: a second get serves cleanly, no further recovery.
            let healed = store.get(&tenant, &h).expect("healed primary read");
            let healed_ok =
                healed == content && store.telemetry().blob_recovered_from_replica() == 1;

            (recovered_ok, healed_ok, native_key)
        })
        .await
        .expect("blocking replicated S3 task")
    };

    assert!(
        recovered_ok,
        "STOR-D7 on the REAL object store: a corrupt primary MUST be recovered from the replica \
         bucket (0 silent serve), with blob_recovered_from_replica == 1"
    );
    assert!(
        healed_ok,
        "the primary MUST be healed from the replica (a second read serves without re-recovery)"
    );

    // Clean up the probe objects in both buckets.
    let _ = raw
        .delete_object()
        .bucket(&primary_bucket)
        .key(&native_key)
        .send()
        .await;
    let _ = raw
        .delete_object()
        .bucket(&replica_bucket)
        .key(&native_key)
        .send()
        .await;
}

/// **P-ST-31 (P-442) LIVE integration: object-backed git packs against the REAL object store
/// (RustFS) — the local-disk-packs follow-on, the explicit sequenced transition (EI-04 §3).**
///
/// Authoritative git bytes move from node-local disk onto the OBJECT tier: a
/// `GitPackTier<ReplicatedBlobStore<S3BlobStore>>` puts/serves git objects through the UNCHANGED
/// `BlobStore` trait, with a PRIMARY object bucket + a REPLICA object bucket underneath — a backing
/// SWAP, the consumer (`GitPackTier`) untouched. This proves STOR-D7 stays green on the
/// object-backed packs against the live RustFS dev stack (the binding policy: an object-store
/// contract ships a real integration test green against the live stack — NOT a mock):
///  1. a git object is put + got THROUGH the trait against the real object backing (content round-trip);
///  2. the PRIMARY object bucket's copy is corrupted OUT-OF-BAND (overwritten via the raw client) →
///     the git-object read re-hashes, detects the mismatch, RECOVERS the correct bytes from the
///     REPLICA object bucket (0 silent serve), and `blob_recovered_from_replica` fires.
#[tokio::test]
async fn object_backed_git_packs_over_real_object_store_recover_corrupt_primary() {
    use myelin_storage::s3blob::S3BlobStore;
    use myelin_storage::{
        object_backed_pack_tier, place_repo_object_backed, GitObjectKind, RepoGitPlacement, RepoId,
        RepoPlacementStatus, StorageGroup,
    };
    use myelin_tenancy::{Region, TenantId};

    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();

    let primary_bucket = cfg.s3.bucket.clone();
    let replica_bucket = format!("{}-replica", cfg.s3.bucket);

    let raw = {
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
        aws_sdk_s3::Client::from_conf(conf)
    };
    // Idempotent replica-bucket create (ignore AlreadyOwned/AlreadyExists).
    let _ = raw.create_bucket().bucket(&replica_bucket).send().await;

    let tenant = TenantId(format!("itest-objpack-{}", std::process::id()));
    let repo = RepoId::from_token("monorepo");
    let content =
        b"fn main() { println!(\"object-backed git packs over real object store\"); }\n".to_vec();

    let primary_s3 = cfg.s3.clone();
    let mut replica_s3 = cfg.s3.clone();
    replica_s3.bucket = replica_bucket.clone();

    let (recovered_ok, native_key) = {
        let handle = handle.clone();
        let tenant = tenant.clone();
        let repo = repo.clone();
        let content = content.clone();
        let primary_bucket = primary_bucket.clone();
        let raw = raw.clone();
        tokio::task::spawn_blocking(move || {
            // The OBJECT-BACKED pack tier: GitPackTier over a ReplicatedBlobStore fronting primary +
            // replica S3 object buckets, all behind the UNCHANGED trait — the fs→object swap is the
            // inner backing, the consumer code is unchanged.
            let tier = object_backed_pack_tier(
                tenant.clone(),
                S3BlobStore::connect(&primary_s3, handle.clone()),
                vec![S3BlobStore::connect(&replica_s3, handle.clone())],
            );
            place_repo_object_backed(
                &tier,
                repo.clone(),
                RepoGitPlacement {
                    group: StorageGroup::from_token("pack-0"),
                    region: Region::new(&primary_s3.region),
                    status: RepoPlacementStatus::Active,
                },
            );

            // (1) put + get THROUGH the trait against the REAL object backing (content round-trip).
            let address = tier
                .put_object(&repo, GitObjectKind::Blob, &content)
                .expect("put object through the object tier");
            assert_eq!(
                tier.get_object(&repo, &address).expect("clean get"),
                content,
                "git object round-trips through the real object-backed tier"
            );

            // The native (BLAKE3) key the framed object is stored under (same key in both buckets —
            // content-addressed); reconstruct its S3 key to corrupt it out-of-band.
            let native = tier
                .native_addr_for_test(&repo, &address)
                .expect("native addr");
            let dh = &native.digest_hex;
            let (fan, rest) = dh.split_at(2);
            let native_key = format!("{}/{}/{}/{}", tenant.0, native.algo.tag(), fan, rest);

            // (2) Corrupt ONLY the PRIMARY object bucket's copy out-of-band (the raw client).
            handle.block_on(async {
                raw.put_object()
                    .bucket(&primary_bucket)
                    .key(&native_key)
                    .body(aws_sdk_s3::primitives::ByteStream::from(
                        b"CORRUPTED-PRIMARY-PACK-BYTES".to_vec(),
                    ))
                    .send()
                    .await
                    .expect("overwrite the primary object with corrupt bytes");
            });

            // The git-object read RECOVERS the correct bytes from the replica bucket (0 silent serve).
            let recovered = tier
                .get_object(&repo, &address)
                .expect("recovered from the replica object bucket");
            let recovered_ok =
                recovered == content && tier.blobs().telemetry().blob_recovered_from_replica() == 1;

            (recovered_ok, native_key)
        })
        .await
        .expect("blocking object-backed git pack task")
    };

    assert!(
        recovered_ok,
        "STOR-D7 on object-backed packs over the REAL object store: a corrupt primary object MUST be \
         recovered from the replica bucket (0 silent serve), with blob_recovered_from_replica == 1"
    );

    // Clean up the probe objects in both buckets.
    let _ = raw
        .delete_object()
        .bucket(&primary_bucket)
        .key(&native_key)
        .send()
        .await;
    let _ = raw
        .delete_object()
        .bucket(&replica_bucket)
        .key(&native_key)
        .send()
        .await;
}

/// **P-ST-28 (P-330) LIVE integration: the C4 trust-scoped CI cache namespaces against the REAL
/// object store (RustFS).**
///
/// The C4 namespace (`CiCacheNamespace`) rides the `BlobStore` trait, so the dev<->prod backing is a
/// SWAP — the SAME write-scope refusal runs with the real `S3BlobStore` underneath, NO code change.
/// This proves the poisoned-cache defence against the live RustFS dev stack (the binding policy: an
/// object-store-touching contract ships a real integration test green against the live stack — NOT a
/// mock):
///  1. a trusted run writes a build-cache entry into the `trusted` scope — its bytes land in the real
///     bucket (a content-addressed blob);
///  2. an `untrusted_fork` run may READ that trusted entry (a cache hit is fine) — proven against the
///     real store;
///  3. the fork's WRITE to the `trusted` scope is REFUSED by the blob client BEFORE any byte reaches
///     the real bucket (0 cross-scope landings on the real store; `cache_scope_violation` fires);
///  4. the fork's write to its OWN `fork:<pr_id>` scope round-trips against the real bucket and is
///     INVISIBLE to a trusted read of the same name (the confinement holds end-to-end).
#[tokio::test]
async fn c4_trust_scoped_cache_namespaces_over_real_object_store() {
    use myelin_storage::s3blob::S3BlobStore;
    use myelin_storage::{CacheScope, CacheScopeError, CiCacheNamespace, TrustTier};
    use myelin_tenancy::TenantId;

    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-c4-{}", std::process::id()));

    // The whole flow runs on ONE blocking thread (the sync BlobStore trait drives the async SDK via
    // block_in_place); the C4 namespace's in-memory scope index stays alive across the run.
    tokio::task::spawn_blocking(move || {
        let store = S3BlobStore::connect(&cfg.s3, handle.clone());
        let cache = CiCacheNamespace::over(tenant.clone(), &store);

        // (1) A trusted run populates the trusted build cache — lands in the REAL bucket.
        cache
            .put(
                TrustTier::Trusted,
                "main",
                &CacheScope::Trusted,
                "deps",
                b"resolved-deps-over-real-object-store",
            )
            .expect("a trusted run writes the trusted scope (real bucket)");

        // (2) A fork run READs the trusted scope — a cache hit is fine (read against the real store).
        assert_eq!(
            cache
                .get(&CacheScope::Trusted, "deps")
                .expect("a fork may read the trusted scope (real bucket)"),
            b"resolved-deps-over-real-object-store"
        );

        // (3) The fork's WRITE to the trusted scope is REFUSED before any byte hits the bucket.
        let poison = cache.put(
            TrustTier::UntrustedFork,
            "1337",
            &CacheScope::Trusted,
            "deps",
            b"MALICIOUS-PAYLOAD",
        );
        assert!(
            matches!(poison, Err(CacheScopeError::ForkWriteToTrusted { .. })),
            "the fork write to the trusted scope MUST be refused, got {poison:?}"
        );
        assert_eq!(cache.telemetry().cache_scope_violation(), 1);
        // 0 cross-scope landing: the trusted "deps" is STILL the trusted run's bytes on the real store.
        assert_eq!(
            cache.get(&CacheScope::Trusted, "deps").unwrap(),
            b"resolved-deps-over-real-object-store"
        );

        // (4) The fork writes its OWN scope (real bucket) — confined; invisible as trusted.
        let fork = CacheScope::Fork {
            pr_id: "1337".to_string(),
        };
        cache
            .put(
                TrustTier::UntrustedFork,
                "1337",
                &fork,
                "deps",
                b"fork-deps",
            )
            .expect("a fork writes its own fork:<pr_id> scope (real bucket)");
        assert_eq!(cache.get(&fork, "deps").unwrap(), b"fork-deps");
        assert!(matches!(
            cache.get(&CacheScope::Trusted, "deps"),
            Ok(ref b) if b.as_slice() == b"resolved-deps-over-real-object-store"
        ));
    })
    .await
    .expect("blocking C4 cache task");
}
