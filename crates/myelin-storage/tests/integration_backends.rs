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
