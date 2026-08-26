#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::{Mode, MyelinConfig};
use myelin_events::clock::clock_reading_from_unix;
use myelin_events::Actor;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_issues::{
    issue_title_holder_receipts, issues_hot_tables, issues_migrations, CreateIssue,
    DurableIssueTitleEraser, IssueAuthorizer, IssuePermission, IssueTitleErasureAttempt,
    PgIssueStore, VisibleIssues,
};
use myelin_storage::{
    all_durable_migrations, DekId, DurablePostPitLedger, KmsEngine, PgBootstrap, SubstrateProvider,
};
use myelin_substrate::HotTables;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use myelin_issues::dek::issue_subject_key_class;

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

fn person(tenant: &str, principal_id: &str) -> Principal {
    Principal::new(
        TenantId::from_token(tenant),
        Region::new("fr-par"),
        PrincipalId(principal_id.into()),
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

fn attempt(operation: &str, actor: &Principal, unix_seconds: i64) -> IssueTitleErasureAttempt {
    IssueTitleErasureAttempt::new(
        operation,
        Actor(actor.clone()),
        clock_reading_from_unix(unix_seconds).unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_erases_only_their_titles_and_later_work_gets_a_fresh_key() {
    let mut config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    config.region = "fr-par".into();
    let bootstrap = PgBootstrap::connect(config, 8)
        .await
        .expect("validate split database roles (have you run `fed test:backend`?)");
    bootstrap.migrate_foundation().await.unwrap();
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
        .unwrap();
    let provider = bootstrap.into_runtime().await.unwrap();

    let unique = format!("{}_{}", std::process::id(), now_nanos());
    let tenant = format!("issue_title_privacy_{unique}");
    let ada = person(&tenant, &format!("human:ada:{unique}"));
    let grace = person(&tenant, &format!("human:grace:{unique}"));
    let kms = Arc::new(KmsEngine::new());
    let store = PgIssueStore::new(provider.clone(), kms.clone(), AllowAuthorizer);
    let eraser = DurableIssueTitleEraser::new(
        store.clone(),
        kms.clone(),
        DurablePostPitLedger::new(provider.clone()),
    );

    let first = store
        .create(&ada, issue("ADA", "A private launch concern"))
        .await
        .unwrap();
    let second = store
        .create(&ada, issue("ADA", "A second authored thought"))
        .await
        .unwrap();
    let colleague = store
        .create(&grace, issue("GRC", "A colleague's intact title"))
        .await
        .unwrap();

    let proof = eraser
        .erase_subject_titles(
            &tenant,
            &ada.principal_id.0,
            attempt("privacy-request:first", &ada, 1_787_578_400),
        )
        .await
        .expect("the scoped key and its two titles erase together");
    assert_eq!(proof.titles_erased, 2);
    assert_eq!(proof.erasure_events_co_committed, 2);
    assert!(proof.key_unrecoverable);
    assert_eq!(
        issue_title_holder_receipts(&proof).unwrap()[0].records_erased,
        2
    );

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .unwrap();
    for id in [&first.id, &second.id] {
        let row = sqlx::query(
            "SELECT title, title_erased, title_nonce, title_ciphertext, pii_key_ref, \
                    title_subject, created_by_principal \
               FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
        )
        .bind(&tenant)
        .bind(id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("title"), "[erased issue title]");
        assert!(row.get::<bool, _>("title_erased"));
        assert!(row.get::<Option<Vec<u8>>, _>("title_nonce").is_none());
        assert!(row.get::<Option<Vec<u8>>, _>("title_ciphertext").is_none());
        assert!(row.get::<Option<String>, _>("pii_key_ref").is_none());
        assert!(row.get::<Option<String>, _>("title_subject").is_none());
        assert!(row
            .get::<Option<String>, _>("created_by_principal")
            .is_none());
    }
    let colleague_row = sqlx::query(
        "SELECT title_erased, title_ciphertext IS NOT NULL AS title_present \
           FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
    )
    .bind(&tenant)
    .bind(&colleague.id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!colleague_row.get::<bool, _>("title_erased"));
    assert!(colleague_row.get::<bool, _>("title_present"));

    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox \
         WHERE envelope->>'tenant' = $1 AND envelope->>'type_' = 'issue.issue.updated' \
           AND envelope #>> '{payload,change_kind}' = 'title_erased'",
    )
    .bind(&tenant)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        event_count, 2,
        "each erased title has one durable consequence"
    );

    let fresh = store
        .create(&ada, issue("ADA", "Useful work after the first erasure"))
        .await
        .expect("completed erasure permits a fresh key epoch");
    let old_replay = eraser
        .erase_subject_titles(
            &tenant,
            &ada.principal_id.0,
            attempt("privacy-request:first", &ada, 1_787_578_401),
        )
        .await
        .unwrap();
    assert!(old_replay.already_completed);
    assert_eq!(old_replay.titles_erased, 2);
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT title_erased FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
    )
    .bind(&tenant)
    .bind(&fresh.id)
    .fetch_one(&admin)
    .await
    .unwrap());

    let fresh_key = DekId::new(
        TenantId::from_token(&tenant),
        issue_subject_key_class(&ada.principal_id.0),
    );
    assert!(kms.export_dek(&fresh_key).unwrap().is_some());
    let second_proof = eraser
        .erase_subject_titles(
            &tenant,
            &ada.principal_id.0,
            attempt("privacy-request:second", &ada, 1_787_578_402),
        )
        .await
        .unwrap();
    assert_eq!(second_proof.titles_erased, 1);
    assert!(kms.export_dek(&fresh_key).unwrap().is_none());

    legacy_shared_keys_are_refused_before_any_key_is_destroyed(provider).await;
    admin.close().await;
}

async fn legacy_shared_keys_are_refused_before_any_key_is_destroyed(provider: SubstrateProvider) {
    let unique = format!("{}_{}", std::process::id(), now_nanos());
    let tenant = format!("issue_title_legacy_{unique}");
    let ada = person(&tenant, &format!("human:ada:{unique}"));
    let kms = Arc::new(KmsEngine::new());
    let store = PgIssueStore::new(provider.clone(), kms.clone(), AllowAuthorizer);
    let created = store
        .create(&ada, issue("LEG", "A legacy-keyed title"))
        .await
        .unwrap();
    let key_id = DekId::new(
        TenantId::from_token(&tenant),
        issue_subject_key_class(&ada.principal_id.0),
    );
    assert!(kms.export_dek(&key_id).unwrap().is_some());

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .unwrap();
    let key_ref: String =
        sqlx::query_scalar("SELECT pii_key_ref FROM issue WHERE tenant_id = $1 AND id = $2::uuid")
            .bind(&tenant)
            .bind(&created.id)
            .fetch_one(&admin)
            .await
            .unwrap();
    let legacy = key_ref.replace("scoped-subject:issues:", "subject:");
    sqlx::query("UPDATE issue SET pii_key_ref = $3 WHERE tenant_id = $1 AND id = $2::uuid")
        .bind(&tenant)
        .bind(&created.id)
        .bind(legacy)
        .execute(&admin)
        .await
        .unwrap();

    let eraser =
        DurableIssueTitleEraser::new(store, kms.clone(), DurablePostPitLedger::new(provider));
    let error = eraser
        .erase_subject_titles(
            &tenant,
            &ada.principal_id.0,
            attempt("privacy-request:legacy", &ada, 1_787_578_500),
        )
        .await
        .expect_err("a generic cross-product key cannot be shredded as an Issues-only key");
    assert!(error.to_string().contains("legacy or foreign key scope"));
    assert!(kms.export_dek(&key_id).unwrap().is_some());
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT title_erased FROM issue WHERE tenant_id = $1 AND id = $2::uuid",
    )
    .bind(&tenant)
    .bind(&created.id)
    .fetch_one(&admin)
    .await
    .unwrap());
    admin.close().await;
}

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL")
        .unwrap_or_else(|_| "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin".into())
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
