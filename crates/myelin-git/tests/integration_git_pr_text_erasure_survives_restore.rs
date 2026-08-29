#![cfg(feature = "integration")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_events::clock::{clock_reading_from_unix, ClockReading};
use myelin_events::Actor;
use myelin_git::durable_erase::DurablePrTextEraser;
use myelin_git::lifecycle::PullRequest;
use myelin_git::pg_pr_store::{
    git_pr_hot_tables, git_pr_migrations, PgPrStore, PrOperationId, PrTextErasureAttempt,
};
use myelin_git::post_restore::PostRestorePrTextReEraser;
use myelin_git::pr_store::PrRecord;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    all_durable_migrations, ColumnCryptor, DurableKmsBacking, DurablePostPitLedger,
    EncryptedColumn, PgBootstrap, PiiKeyRef, SealKey, SubstrateProvider, TenantScope, NONCE_LEN,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;

fn app_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url.clone();
            config.database_migration_url =
                database_url.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
        }
    }
    config
}

fn admin_config() -> MyelinConfig {
    let mut config = app_config();
    config.database_url = config.database_migration_url.clone();
    config
}

fn unique(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the test clock follows the Unix epoch")
            .as_nanos(),
    )
}

fn scratch_database_name() -> String {
    unique("git_pr_text_restore").replace('-', "_")
}

fn database_url_for(database_url: &str, database: &str) -> String {
    let (server, _) = database_url
        .rsplit_once('/')
        .expect("a PostgreSQL URL names a database");
    format!("{server}/{database}")
}

fn dump_database(database_url: &str, scratch_database: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("{scratch_database}.dump"));
    let dump = Command::new("pg_dump")
        .args(["--format=custom", "--file"])
        .arg(&path)
        .arg(database_url)
        .output()
        .expect("the PostgreSQL backup client is installed");
    assert!(
        dump.status.success(),
        "the backup containing the live pull-request text succeeds: {}",
        String::from_utf8_lossy(&dump.stderr),
    );
    path
}

async fn restore_database(
    admin: &SubstrateProvider,
    admin_url: &str,
    database: &str,
    dump: &Path,
) -> String {
    sqlx::raw_sql(&format!("CREATE DATABASE {database}"))
        .execute(admin.db_pool())
        .await
        .expect("create an empty database for the restored point in time");
    let restored_url = database_url_for(admin_url, database);
    let restore = Command::new("pg_restore")
        .args(["--no-owner", "--dbname"])
        .arg(&restored_url)
        .arg(dump)
        .output()
        .expect("the PostgreSQL restore client is installed");
    if !restore.status.success() {
        let stderr = String::from_utf8_lossy(&restore.stderr);
        let only_client_server_setting_skew = stderr
            .lines()
            .filter(|line| line.contains("error:"))
            .all(|line| line.contains("unrecognized configuration parameter"));
        assert!(
            only_client_server_setting_skew,
            "the Git restore failed beyond harmless client/server setting skew: {stderr}",
        );
    }
    restored_url
}

fn person(tenant: &str, region: &Region, principal: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        region.clone(),
        PrincipalId(principal.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn pull_request(title: &str, body: &str, head_repo: &str, head_oid: char) -> PrRecord {
    let lifecycle = PullRequest::open(
        0,
        "refs/heads/main",
        "refs/heads/private-change",
        "author@tenant.noreply",
        false,
    );
    let mut record = PrRecord::open(&lifecycle, head_oid.to_string().repeat(40));
    record.head_repo_slug = head_repo.into();
    record.title = title.into();
    record.body_md = Some(body.into());
    record
}

fn observed(unix_seconds: u64) -> ClockReading {
    clock_reading_from_unix(i64::try_from(unix_seconds).expect("test time fits i64"))
        .expect("test time has an RFC 3339 rendering")
}

fn attempt(operation: &str, actor: &Principal, at: ClockReading) -> PrTextErasureAttempt {
    PrTextErasureAttempt::new(operation, Actor(actor.clone()), at)
        .expect("the restore drill uses a bounded operation id")
}

async fn encrypted_title(pool: &PgPool, tenant: &str, repo: &str, number: u64) -> EncryptedColumn {
    let number = i64::try_from(number).expect("the PR number fits PostgreSQL bigint");
    let row = sqlx::query(
        "SELECT title_nonce, title_ciphertext, title_pii_key_ref \
           FROM git_pr WHERE tenant_id=$1 AND repo_slug=$2 AND number=$3",
    )
    .bind(tenant)
    .bind(repo)
    .bind(number)
    .fetch_one(pool)
    .await
    .expect("the pull-request title has durable encrypted material");
    let nonce: Vec<u8> = row.get("title_nonce");
    EncryptedColumn {
        key_ref: PiiKeyRef::parse(&row.get::<String, _>("title_pii_key_ref"))
            .expect("the stored title key reference is canonical"),
        nonce: nonce
            .try_into()
            .unwrap_or_else(|_| panic!("the stored title nonce is {NONCE_LEN} bytes")),
        ciphertext: row.get("title_ciphertext"),
    }
}

fn open_text(kms: &myelin_storage::KmsEngine, region: &Region, text: &EncryptedColumn) -> String {
    String::from_utf8(
        ColumnCryptor::new(kms, region.clone())
            .decrypt(text)
            .expect("the Git subject key is available at this point"),
    )
    .expect("pull-request text is UTF-8")
}

async fn assert_pr_text_is_erased(pool: &PgPool, tenant: &str, repo: &str, number: u64) {
    let number = i64::try_from(number).expect("the PR number fits PostgreSQL bigint");
    let row = sqlx::query(
        "SELECT free_text_erased, title_nonce, title_ciphertext, title_pii_key_ref, \
                body_nonce, body_ciphertext, body_pii_key_ref \
           FROM git_pr WHERE tenant_id=$1 AND repo_slug=$2 AND number=$3",
    )
    .bind(tenant)
    .bind(repo)
    .bind(number)
    .fetch_one(pool)
    .await
    .expect("the shared pull-request coordinate remains after text erasure");
    assert!(row.get::<bool, _>("free_text_erased"));
    assert_eq!(row.get::<Vec<u8>, _>("title_nonce"), vec![0; NONCE_LEN]);
    assert_eq!(row.get::<Vec<u8>, _>("title_ciphertext"), vec![0]);
    assert_eq!(row.get::<String, _>("title_pii_key_ref"), "erased");
    assert!(row.get::<Option<Vec<u8>>, _>("body_nonce").is_none());
    assert!(row.get::<Option<Vec<u8>>, _>("body_ciphertext").is_none());
    assert!(row.get::<Option<String>, _>("body_pii_key_ref").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_pull_request_text_erasure_survives_restoring_a_real_database_backup() {
    let admin = SubstrateProvider::connect(admin_config(), 2)
        .await
        .expect("connect as the migration owner");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(
            &all_durable_migrations(),
            &myelin_storage::HotTables::none(),
        )
        .await
        .unwrap();
    admin
        .migrate(&git_pr_migrations(), &git_pr_hot_tables())
        .await
        .unwrap();
    let live = PgBootstrap::connect(app_config(), 4)
        .await
        .expect("validate the split production database roles")
        .into_runtime()
        .await
        .expect("handoff the validated production runtime");

    let tenant = unique("git-pr-text-restore-tenant");
    let cell = unique("git-pr-text-restore-cell");
    let repo = unique("private-repo");
    let region = Region(live.config().region.clone());
    let ada = person(
        &tenant,
        &region,
        &format!("human:ada:{}", unique("subject")),
    );
    let grace = person(
        &tenant,
        &region,
        &format!("human:grace:{}", unique("subject")),
    );
    let scope = TenantScope::from_verified_token(&ada, region.clone());
    let seal_key = SealKey::from_encoded(&"92".repeat(32)).expect("a 32-byte test seal key");
    let live_kms = Arc::new(
        DurableKmsBacking::new(live.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("load the durable KMS used by live Git"),
    );
    let live_store = PgPrStore::new(
        live.clone(),
        live_kms.clone(),
        tokio::runtime::Handle::current(),
    )
    .expect("open the production Git PR store");
    let ledger = DurablePostPitLedger::new(live.clone());
    let live_eraser =
        DurablePrTextEraser::new(live_store.clone(), live_kms.clone(), ledger.clone());

    let private_pr = live_store
        .open(
            &scope,
            &repo,
            pull_request(
                "Ada's private launch plan must not return after a restore",
                "The launch body also belongs to Ada and must be erased.",
                &repo,
                'a',
            ),
            &PrOperationId::parse("restore-drill-open-ada").unwrap(),
            &ada,
        )
        .expect("Ada has authored pull-request text at the backup point");
    let neighbour_pr = live_store
        .open(
            &scope,
            &repo,
            pull_request(
                "Grace's neighboring context must survive Ada's request",
                "This neighboring body remains readable.",
                &repo,
                'b',
            ),
            &PrOperationId::parse("restore-drill-open-grace").unwrap(),
            &grace,
        )
        .expect("Grace has authored neighboring pull-request text at the backup point");
    let private_title = encrypted_title(admin.db_pool(), &tenant, &repo, private_pr.number).await;
    let neighbour_title =
        encrypted_title(admin.db_pool(), &tenant, &repo, neighbour_pr.number).await;

    let scratch_database = scratch_database_name();
    let migration_url = admin_config().database_url;
    let dump_path = dump_database(&migration_url, &scratch_database);
    let restored_to = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock follows the Unix epoch")
        .as_secs();

    let erased = live_eraser
        .erase_subject_pr_text(
            &tenant,
            &ada.principal_id.0,
            attempt(
                "privacy-request:git-after-backup",
                &ada,
                observed(restored_to.saturating_add(1)),
            ),
        )
        .await
        .expect("the live privacy request erases Ada's authored pull-request text");
    assert_eq!(
        (
            erased.pull_requests_erased,
            erased.erasure_events_co_committed
        ),
        (1, 1),
    );
    assert!(
        ColumnCryptor::new(&live_kms, region.clone())
            .decrypt(&private_title)
            .is_err(),
        "the live database no longer resolves Ada's old title",
    );
    assert_eq!(
        open_text(&live_kms, &region, &neighbour_title),
        "Grace's neighboring context must survive Ada's request",
    );

    let restored_admin_url =
        restore_database(&admin, &migration_url, &scratch_database, &dump_path).await;
    let restored_admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&restored_admin_url)
        .await
        .expect("connect to the restored database for forensic assertions");
    let mut restored_config = app_config();
    restored_config.database_url =
        database_url_for(&restored_config.database_url, &scratch_database);
    restored_config.database_migration_url = restored_admin_url;
    let restored = PgBootstrap::connect(restored_config, 4)
        .await
        .expect("validate the restored split database roles")
        .into_runtime()
        .await
        .expect("handoff the validated restored runtime");
    let restored_kms = Arc::new(
        DurableKmsBacking::new(restored.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("load the resurrected Git subject-key hierarchy"),
    );
    let restored_store = PgPrStore::new(
        restored.clone(),
        restored_kms.clone(),
        tokio::runtime::Handle::current(),
    )
    .expect("open the restored Git PR store");

    assert_eq!(
        open_text(&restored_kms, &region, &private_title),
        "Ada's private launch plan must not return after a restore",
        "the real restore has teeth: it resurrects the exact title erased after the backup",
    );
    assert_eq!(
        open_text(&restored_kms, &region, &neighbour_title),
        "Grace's neighboring context must survive Ada's request",
    );

    let re_eraser = PostRestorePrTextReEraser::new(
        ledger.clone(),
        restored.clone(),
        restored_kms.clone(),
        tokio::runtime::Handle::current(),
    )
    .expect("construct the restored Git holder");
    let re_erased = re_eraser
        .run(restored_to, observed(restored_to.saturating_add(2)))
        .await
        .expect("replay the live Git erasure ledger into the restored database");
    assert_eq!(re_erased.selected_subjects, 1);
    assert_eq!(re_erased.newly_re_erased_subjects, 1);
    assert_eq!(re_erased.already_erased_subjects, 0);
    assert_eq!(
        (
            re_erased.pull_requests_erased,
            re_erased.erasure_events_co_committed,
        ),
        (1, 1),
    );
    assert!(
        ColumnCryptor::new(&restored_kms, region.clone())
            .decrypt(&private_title)
            .is_err(),
        "re-erasure destroys the Git key resurrected by the backup",
    );
    assert_eq!(
        open_text(&restored_kms, &region, &neighbour_title),
        "Grace's neighboring context must survive Ada's request",
    );
    assert_pr_text_is_erased(&restored_admin, &tenant, &repo, private_pr.number).await;
    let erased_record = restored_store
        .get(&scope, &repo, private_pr.number)
        .expect("read the retained pull-request coordinate")
        .expect("the erased pull request remains addressable");
    assert_eq!(erased_record.title, "[erased pull request title]");
    assert!(erased_record.body_md.is_none());

    let fresh_pr = restored_store
        .open(
            &scope,
            &repo,
            pull_request(
                "Ada may write again after her narrow Git-text erasure",
                "Fresh work is outside the completed request.",
                &repo,
                'c',
            ),
            &PrOperationId::parse("restore-drill-open-fresh").unwrap(),
            &ada,
        )
        .expect("a narrow request does not erase Ada's right to create new work");
    let fresh_title = encrypted_title(&restored_admin, &tenant, &repo, fresh_pr.number).await;
    let resumed = re_eraser
        .run(restored_to, observed(restored_to.saturating_add(3)))
        .await
        .expect("a response-lost operator invocation resumes safely");
    assert_eq!(resumed.newly_re_erased_subjects, 0);
    assert_eq!(resumed.already_erased_subjects, 1);
    assert_eq!(
        open_text(&restored_kms, &region, &fresh_title),
        "Ada may write again after her narrow Git-text erasure",
        "replaying the restore receipt never consumes newly authored work",
    );

    restored.db_pool().close().await;
    restored_admin.close().await;
    sqlx::raw_sql(&format!("DROP DATABASE {scratch_database} WITH (FORCE)"))
        .execute(admin.db_pool())
        .await
        .expect("remove the isolated restored database");
    let _ = std::fs::remove_file(&dump_path);
    sqlx::query("DELETE FROM git_pr_command WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM git_pr_text_erasure_operation WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM git_pr WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM git_pr_counter WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant'=$1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM post_pit_erasure_ledger \
          WHERE tenant_id=$1 AND region=$2 AND scope='git_pr_text'",
    )
    .bind(&tenant)
    .bind(region.as_str())
    .execute(admin.db_pool())
    .await
    .expect("remove the isolated live restore obligation");
    for kms_table in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
        sqlx::query(&format!("DELETE FROM {kms_table} WHERE cell_id=$1"))
            .bind(&cell)
            .execute(admin.db_pool())
            .await
            .expect("remove the isolated live key hierarchy");
    }
}
