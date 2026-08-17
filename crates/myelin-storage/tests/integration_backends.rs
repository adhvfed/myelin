#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;

#[tokio::test]
async fn postgres_oltp_reachable_and_rls_ready() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres (is the stack up?)");

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
    assert!(!is_super, "app role must not be superuser");
    assert!(!bypass, "app role must not have BYPASSRLS");

    let helper: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_proc WHERE proname = 'myelin_make_tenant_scoped'",
    )
    .fetch_one(&pool)
    .await
    .expect("query for RLS helper");
    assert_eq!(helper, 1, "myelin_make_tenant_scoped RLS helper must exist");
}

#[tokio::test]
async fn postgres_rls_isolates_tenants() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres");

    let tbl = format!("rls_probe_{}", std::process::id());
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

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

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

    let tenant = TenantId(format!("itest-git-{}", std::process::id()));
    let repo = RepoId::from_token("web");
    let content = b"fn main() { println!(\"git pack over real object store\"); }\n".to_vec();

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
            )
            .expect("place S3-backed test repository");
            let address = tier
                .put_object(&repo, GitObjectKind::Blob, &content)
                .expect("put object");
            let got = tier.get_object(&repo, &address).expect("get object");
            assert_eq!(
                got, content,
                "git object round-trips through the real object store"
            );

            let native = tier
                .native_addr_for_test(&repo, &address)
                .expect("object index state")
                .expect("native addr");
            let dh = native.digest_hex();
            let (fan, rest) = dh.split_at(2);
            let native_key = format!("{}/{}/{}/{}", tenant.0, native.algorithm().tag(), fan, rest);

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

    let _ = raw
        .delete_object()
        .bucket(&bucket)
        .key(&native_key)
        .send()
        .await;
}

#[tokio::test]
async fn replicated_object_store_recovers_corrupt_primary_from_replica() {
    use myelin_storage::s3blob::S3BlobStore;
    use myelin_storage::{BlobStore, ReplicatedBlobStore};
    use myelin_tenancy::TenantId;

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
            let store = ReplicatedBlobStore::new(
                S3BlobStore::connect(&primary_s3, handle.clone()),
                vec![S3BlobStore::connect(&replica_s3, handle.clone())],
            );

            let h = store.put(&tenant, &content).expect("replicated put");
            assert_eq!(store.get(&tenant, &h).expect("clean get"), content);

            let dh = h.digest_hex();
            let (fan, rest) = dh.split_at(2);
            let native_key = format!("{}/{}/{}/{}", tenant.0, h.algorithm().tag(), fan, rest);

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

            let recovered = store.get(&tenant, &h).expect("recovered from replica");
            let recovered_ok =
                recovered == content && store.telemetry().blob_recovered_from_replica() == 1;

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
            )
            .expect("place replicated S3 test repository");

            let address = tier
                .put_object(&repo, GitObjectKind::Blob, &content)
                .expect("put object through the object tier");
            assert_eq!(
                tier.get_object(&repo, &address).expect("clean get"),
                content,
                "git object round-trips through the real object-backed tier"
            );

            let native = tier
                .native_addr_for_test(&repo, &address)
                .expect("object index state")
                .expect("native addr");
            let dh = native.digest_hex();
            let (fan, rest) = dh.split_at(2);
            let native_key = format!("{}/{}/{}/{}", tenant.0, native.algorithm().tag(), fan, rest);

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

#[tokio::test]
async fn c4_trust_scoped_cache_namespaces_over_real_object_store() {
    use myelin_storage::s3blob::S3BlobStore;
    use myelin_storage::{CacheScope, CacheScopeError, CiCacheNamespace, TrustTier};
    use myelin_tenancy::TenantId;

    let cfg = MyelinConfig::dev();
    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-c4-{}", std::process::id()));

    tokio::task::spawn_blocking(move || {
        let store = S3BlobStore::connect(&cfg.s3, handle.clone());
        let cache = CiCacheNamespace::over(tenant.clone(), &store);

        cache
            .put(
                TrustTier::Trusted,
                "main",
                &CacheScope::Trusted,
                "deps",
                b"resolved-deps-over-real-object-store",
            )
            .expect("a trusted run writes the trusted scope (real bucket)");

        assert_eq!(
            cache
                .get(&CacheScope::Trusted, "deps")
                .expect("a fork may read the trusted scope (real bucket)"),
            b"resolved-deps-over-real-object-store"
        );

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
        assert_eq!(
            cache.get(&CacheScope::Trusted, "deps").unwrap(),
            b"resolved-deps-over-real-object-store"
        );

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w7_3_blob_flip_survives_a_fresh_s3_store_reconstruction() {
    use myelin_storage::backend::{blob_store, Backend};
    use myelin_storage::blob::ContentHash;
    use myelin_tenancy::TenantId;

    let handle = tokio::runtime::Handle::current();
    let tenant = TenantId(format!("itest-w73-{}", std::process::id()));
    let payload = b"a re-pointed knowledge/chat blob that MUST survive a process restart".to_vec();
    let endpoint = MyelinConfig::dev().s3.endpoint.clone();

    let hash = {
        let tenant = tenant.clone();
        let payload = payload.clone();
        let handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            let store = blob_store(Backend::Real, &MyelinConfig::dev(), handle)
                .expect("the real object-store backend is always available");
            store
                .put(&tenant, &payload)
                .expect("put through the durable Backend::Real seam")
        })
        .await
        .expect("blocking put task")
    };

    let got = {
        let tenant = tenant.clone();
        let hash = hash.clone();
        let handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            let fresh = blob_store(Backend::Real, &MyelinConfig::dev(), handle)
                .expect("the real object-store backend is always available");
            fresh
                .get(&tenant, &hash)
                .expect("the bytes survived a FRESH store reconstruction (byte-durable, kill-9)")
        })
        .await
        .expect("blocking get task")
    };

    assert_eq!(
        got, payload,
        "W7.3: bytes PUT through the durable Backend::Real seam SURVIVE a fresh store reconstruction \
         (kill-9) - the property the in-memory FsBlobStore floor could NOT provide"
    );
    assert_eq!(
        ContentHash::blake3(&got),
        hash,
        "the reconstructed durable backing serves address-verified bytes (re-hash-on-read holds)"
    );

    let _ = tokio::task::spawn_blocking(move || {
        let s = blob_store(Backend::Real, &MyelinConfig::dev(), handle)
            .expect("the real object-store backend is always available");
        let _ = s.delete(&tenant, &hash);
    })
    .await;

    println!(
        "[W7.3 INTEGRATION GREEN] FsBlobStore→S3 flip: a blob PUT through Backend::Real (the seam \
         provider.blob_store() + the knowledge/chat re-points select) SURVIVES a FRESH S3BlobStore \
         reconstruction on the live dev stack ({endpoint}) - byte-durable, unlike the in-memory fs \
         floor. Integrity-refusal on the S3 arm: git_pack_tier_over_real_object_store_roundtrips_and_detects_corruption."
    );
}
