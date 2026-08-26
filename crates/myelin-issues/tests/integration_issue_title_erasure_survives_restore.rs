#![cfg(feature = "integration")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_events::clock::{clock_reading_from_unix, ClockReading};
use myelin_events::Actor;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_issues::{
    decrypt_free_text, issues_hot_tables, issues_migrations, CreateIssue, DurableIssueTitleEraser,
    IssueAuthorizer, IssuePermission, IssueTitleErasureAttempt, PgIssueStore,
    PostRestoreIssueTitleReEraser, VisibleIssues,
};
use myelin_storage::{
    all_durable_migrations, DurableKmsBacking, DurablePostPitLedger, EncryptedColumn, KmsEngine,
    PiiKeyRef, SealKey, SubstrateProvider, NONCE_LEN,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Row, ValueRef};

#[derive(Clone)]
struct AllowAuthorizer;

impl IssueAuthorizer for AllowAuthorizer {
    fn may_create(&self, _principal: &Principal, _project_id: &str) -> bool {
        true
    }

    fn may_access(
        &self,
        _principal: &Principal,
        _issue_id: &str,
        _permission: IssuePermission,
    ) -> bool {
        true
    }

    fn visible_issues(&self, _principal: &Principal) -> Result<VisibleIssues, String> {
        Ok(VisibleIssues::All)
    }
}

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
    unique("issue_title_restore").replace('-', "_")
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
        "the backup containing the live Issue title succeeds: {}",
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
            "the Issue restore failed beyond harmless client/server setting skew: {stderr}",
        );
    }
    restored_url
}

fn person(tenant: &str, principal: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new("fr-par"),
        PrincipalId(principal.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn issue(prefix: &str, title: &str) -> CreateIssue {
    CreateIssue {
        project_id: "11111111-1111-1111-1111-111111111111".into(),
        type_id: "22222222-2222-2222-2222-222222222222".into(),
        prefix: prefix.into(),
        title: title.into(),
    }
}

fn observed(unix_seconds: u64) -> ClockReading {
    clock_reading_from_unix(i64::try_from(unix_seconds).expect("test time fits i64"))
        .expect("test time has an RFC 3339 rendering")
}

fn attempt(operation: &str, actor: &Principal, at: ClockReading) -> IssueTitleErasureAttempt {
    IssueTitleErasureAttempt::new(operation, Actor(actor.clone()), at)
        .expect("the restore drill uses a bounded operation id")
}

async fn encrypted_title(pool: &PgPool, tenant: &str, issue_id: &str) -> EncryptedColumn {
    let row = sqlx::query(
        "SELECT title_nonce, title_ciphertext, pii_key_ref \
           FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
    )
    .bind(tenant)
    .bind(issue_id)
    .fetch_one(pool)
    .await
    .expect("the Issue title has durable encrypted material");
    let nonce: Vec<u8> = row.get("title_nonce");
    EncryptedColumn {
        key_ref: PiiKeyRef::parse(&row.get::<String, _>("pii_key_ref"))
            .expect("the stored title key reference is canonical"),
        nonce: nonce
            .try_into()
            .unwrap_or_else(|_| panic!("the stored title nonce is {NONCE_LEN} bytes")),
        ciphertext: row.get("title_ciphertext"),
    }
}

fn open_title(kms: &KmsEngine, region: &Region, title: &EncryptedColumn) -> String {
    String::from_utf8(
        decrypt_free_text(kms, region, title).expect("the title key is available at this point"),
    )
    .expect("Issue titles are UTF-8")
}

async fn assert_title_is_erased(pool: &PgPool, tenant: &str, issue_id: &str) {
    let row = sqlx::query(
        "SELECT title, title_erased, title_nonce, title_ciphertext, pii_key_ref, \
                title_subject, created_by_principal \
           FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
    )
    .bind(tenant)
    .bind(issue_id)
    .fetch_one(pool)
    .await
    .expect("the shared Issue coordinate remains after title erasure");
    assert_eq!(row.get::<String, _>("title"), "[erased issue title]");
    assert!(row.get::<bool, _>("title_erased"));
    for column in [
        "title_nonce",
        "title_ciphertext",
        "pii_key_ref",
        "title_subject",
        "created_by_principal",
    ] {
        assert!(
            row.try_get_raw(column)
                .expect("known Issue column")
                .is_null(),
            "{column} is cleared when the title is erased",
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_issue_title_erasure_survives_restoring_a_real_database_backup() {
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
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .unwrap();
    let live = SubstrateProvider::connect(app_config(), 4)
        .await
        .expect("connect as the production application role");

    let tenant = unique("issue-title-restore-tenant");
    let cell = unique("issue-title-restore-cell");
    let region = Region(live.config().region.clone());
    let ada = person(&tenant, &format!("human:ada:{}", unique("subject")));
    let grace = person(&tenant, &format!("human:grace:{}", unique("subject")));
    let seal_key = SealKey::from_encoded(&"91".repeat(32)).expect("a 32-byte test seal key");
    let live_kms = Arc::new(
        DurableKmsBacking::new(live.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("load the durable KMS used by live Issues"),
    );
    let live_store = PgIssueStore::new(live.clone(), live_kms.clone(), AllowAuthorizer);
    let ledger = DurablePostPitLedger::new(live.clone());
    let live_eraser =
        DurableIssueTitleEraser::new(live_store.clone(), live_kms.clone(), ledger.clone());

    let private_issue = live_store
        .create(
            &ada,
            issue(
                "ADA",
                "Ada's private launch concern must not return after a restore",
            ),
        )
        .await
        .expect("Ada has authored an Issue title at the backup point");
    let neighbour_issue = live_store
        .create(
            &grace,
            issue(
                "GRC",
                "Grace's neighbouring context must survive Ada's request",
            ),
        )
        .await
        .expect("Grace has authored a neighbouring Issue title at the backup point");
    let private_title = encrypted_title(admin.db_pool(), &tenant, &private_issue.id).await;
    let neighbour_title = encrypted_title(admin.db_pool(), &tenant, &neighbour_issue.id).await;

    let scratch_database = scratch_database_name();
    let migration_url = admin_config().database_url;
    let dump_path = dump_database(&migration_url, &scratch_database);
    let restored_to = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock follows the Unix epoch")
        .as_secs();

    let erased = live_eraser
        .erase_subject_titles(
            &tenant,
            &ada.principal_id.0,
            attempt(
                "privacy-request:issues-after-backup",
                &ada,
                observed(restored_to.saturating_add(1)),
            ),
        )
        .await
        .expect("the live privacy request erases Ada's authored Issue title");
    assert_eq!(
        (erased.titles_erased, erased.erasure_events_co_committed),
        (1, 1),
    );
    assert!(
        decrypt_free_text(&live_kms, &region, &private_title).is_err(),
        "the live database no longer resolves Ada's old title",
    );
    assert_eq!(
        open_title(&live_kms, &region, &neighbour_title),
        "Grace's neighbouring context must survive Ada's request",
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
    let restored = SubstrateProvider::connect(restored_config, 4)
        .await
        .expect("connect to the restored database as the application role");
    let restored_kms = Arc::new(
        DurableKmsBacking::new(restored.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("load the resurrected Issue-title key hierarchy"),
    );
    let restored_store = PgIssueStore::new(restored.clone(), restored_kms.clone(), AllowAuthorizer);

    assert_eq!(
        open_title(&restored_kms, &region, &private_title),
        "Ada's private launch concern must not return after a restore",
        "the real restore has teeth: it resurrects the exact title that was later erased",
    );
    assert_eq!(
        open_title(&restored_kms, &region, &neighbour_title),
        "Grace's neighbouring context must survive Ada's request",
    );

    let re_erased =
        PostRestoreIssueTitleReEraser::new(ledger.clone(), restored.clone(), restored_kms.clone())
            .run(restored_to, observed(restored_to.saturating_add(2)))
            .await
            .expect("replay the live Issue erasure ledger into the restored database");
    assert_eq!(re_erased.selected_subjects, 1);
    assert_eq!(re_erased.newly_re_erased_subjects, 1);
    assert_eq!(re_erased.already_erased_subjects, 0);
    assert_eq!(
        (
            re_erased.titles_erased,
            re_erased.erasure_events_co_committed,
        ),
        (1, 1),
    );
    assert!(
        decrypt_free_text(&restored_kms, &region, &private_title).is_err(),
        "re-erasure destroys the Issue-title key resurrected by the backup",
    );
    assert_eq!(
        open_title(&restored_kms, &region, &neighbour_title),
        "Grace's neighbouring context must survive Ada's request",
    );
    assert_title_is_erased(&restored_admin, &tenant, &private_issue.id).await;

    let fresh_issue = restored_store
        .create(
            &ada,
            issue(
                "ADA",
                "Ada may write again after her narrow Issue-title erasure",
            ),
        )
        .await
        .expect("a narrow title request does not erase Ada's right to create new work");
    let fresh_title = encrypted_title(&restored_admin, &tenant, &fresh_issue.id).await;
    let resumed =
        PostRestoreIssueTitleReEraser::new(ledger, restored.clone(), restored_kms.clone())
            .run(restored_to, observed(restored_to.saturating_add(3)))
            .await
            .expect("a response-lost operator invocation resumes safely");
    assert_eq!(resumed.newly_re_erased_subjects, 0);
    assert_eq!(resumed.already_erased_subjects, 1);
    assert_eq!(
        open_title(&restored_kms, &region, &fresh_title),
        "Ada may write again after her narrow Issue-title erasure",
        "replaying the restore receipt never consumes newly authored work",
    );

    restored.db_pool().close().await;
    restored_admin.close().await;
    sqlx::raw_sql(&format!("DROP DATABASE {scratch_database} WITH (FORCE)"))
        .execute(admin.db_pool())
        .await
        .expect("remove the isolated restored database");
    let _ = std::fs::remove_file(&dump_path);
    sqlx::query("DELETE FROM issue_authz_binding WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM issue_title_erasure_operation WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM issue WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM prefix_counter WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
        .bind(&tenant)
        .execute(admin.db_pool())
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM post_pit_erasure_ledger \
          WHERE tenant_id = $1 AND region = $2 AND scope = 'issue_titles'",
    )
    .bind(&tenant)
    .bind(region.as_str())
    .execute(admin.db_pool())
    .await
    .expect("remove the isolated live restore obligation");
    for kms_table in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
        sqlx::query(&format!("DELETE FROM {kms_table} WHERE cell_id = $1"))
            .bind(&cell)
            .execute(admin.db_pool())
            .await
            .expect("remove the isolated live key hierarchy");
    }
}
