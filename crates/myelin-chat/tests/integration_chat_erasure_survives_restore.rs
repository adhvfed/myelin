#![cfg(feature = "integration")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use myelin_chat::events::{event_actor_pseudonym, pseudonymized_event_principal};
use myelin_chat::store::pg::{MessageAttribution, MessageErasureAttempt, PgMessageStore};
use myelin_chat::store::{
    AuthorKind, ConversationId, MessageState, MonotonicUlidSource, NewMessage, RangeCursor,
};
use myelin_chat::{
    decrypt_body, encode_encrypted_body, encrypt_body, ChatFreeText, DurableChatMessageEraser,
    PostRestoreChatMessageReEraser,
};
use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, Timestamp, UlidMinter};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{
    all_durable_migrations, DurableKmsBacking, DurablePostPitLedger, HotTables, KmsEngine, SealKey,
    SubstrateProvider,
};
use myelin_storage::{EncryptedColumn, SubjectId};
use myelin_tenancy::{Region, TenantId};

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
    unique("chat_restore").replace('-', "_")
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
        "the backup containing the live Chat body succeeds: {}",
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
            "the Chat restore failed beyond harmless client/server setting skew: {stderr}",
        );
    }
    restored_url
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock follows the Unix epoch")
        .as_secs()
}

fn now() -> Timestamp {
    Timestamp("2026-08-26T00:00:00Z".into())
}

fn actor(tenant: &str, principal: &str) -> Actor {
    let principal = Principal::stub(
        PrincipalId(principal.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    Actor(pseudonymized_event_principal(tenant, &principal))
}

fn encrypted_message(
    kms: &KmsEngine,
    conversation: ConversationId,
    subject: &str,
    nonce: &str,
    plaintext: &str,
) -> (NewMessage, EncryptedColumn) {
    let tenant = TenantId::from_token(&conversation.tenant);
    let author = event_actor_pseudonym(&conversation.tenant, subject);
    let subject = SubjectId::new(author.clone());
    let region = Region(conversation.region.clone());
    let body = encrypt_body(
        kms,
        &region,
        &tenant,
        &subject,
        ChatFreeText::BodyInline,
        plaintext.as_bytes(),
    )
    .expect("encrypt the authored Chat body under its durable scoped key");
    let nodes = encrypt_body(
        kms,
        &region,
        &tenant,
        &subject,
        ChatFreeText::BodyNodes,
        b"[]",
    )
    .expect("encrypt the structured body under the same scoped key");
    (
        NewMessage {
            conv: conversation,
            thread_root_id: None,
            author,
            author_kind: AuthorKind::Human,
            body_inline: encode_encrypted_body(&body).expect("encode the encrypted body"),
            body_nodes: encode_encrypted_body(&nodes).expect("encode the encrypted nodes"),
            client_nonce: nonce.into(),
        },
        body,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_chat_erasure_survives_restoring_a_real_database_backup() {
    let admin = SubstrateProvider::connect(admin_config(), 2)
        .await
        .expect("connect as the migration owner");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    let live = SubstrateProvider::connect(app_config(), 2)
        .await
        .expect("connect as the production application role");

    let tenant = unique("chat-restore-tenant");
    let region = Region(live.config().region.clone());
    let cell = unique("chat-restore-cell");
    let table = unique("chat_message_restore").replace('-', "_");
    PgMessageStore::new(admin.db_pool().clone(), region.as_str(), table.clone())
        .migrate()
        .await
        .expect("create the production Chat message schema");
    let messages = PgMessageStore::new(live.db_pool().clone(), region.as_str(), table.clone());
    let seal_key = SealKey::from_encoded(&"83".repeat(32)).expect("a 32-byte test seal key");
    let live_kms = Arc::new(
        DurableKmsBacking::new(live.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("load the durable KMS used by live Chat"),
    );
    let ledger = DurablePostPitLedger::new(live.clone());
    let eraser =
        DurableChatMessageEraser::new(messages.clone(), Arc::clone(&live_kms), ledger.clone());
    let conversation = ConversationId::new(&tenant, region.as_str(), unique("conversation"));
    let message_ids = MonotonicUlidSource::new();
    let event_ids = UlidMinter::new();

    let (private_message, private_envelope) = encrypted_message(
        &live_kms,
        conversation.clone(),
        "ada",
        "ada-before-backup",
        "Ada's private launch concern must not return after a restore",
    );
    let (neighbour_message, neighbour_envelope) = encrypted_message(
        &live_kms,
        conversation.clone(),
        "bob",
        "bob-before-backup",
        "Bob's neighbouring context must survive Ada's request",
    );
    messages
        .append_storage_only(&message_ids, private_message)
        .await
        .expect("Ada has authored Chat history at the backup point");
    messages
        .append_storage_only(&message_ids, neighbour_message)
        .await
        .expect("Bob has neighbouring Chat history at the backup point");

    let scratch_database = scratch_database_name();
    let migration_url = admin_config().database_url;
    let dump_path = dump_database(&migration_url, &scratch_database);
    let restored_to = now_seconds().saturating_sub(1);

    let erased = eraser
        .erase_subject_messages(
            &tenant,
            "ada",
            &event_ids,
            MessageErasureAttempt::new(
                "privacy-request:chat-after-backup",
                actor(&tenant, "ada"),
                now(),
                now(),
            ),
        )
        .await
        .expect("the live privacy request erases Ada's authored Chat history");
    assert_eq!(
        (erased.messages_erased, erased.erasure_events_co_committed),
        (1, 1)
    );
    assert!(
        decrypt_body(&live_kms, &region, &private_envelope).is_err(),
        "the live database no longer resolves Ada's old body",
    );
    assert_eq!(
        decrypt_body(&live_kms, &region, &neighbour_envelope).unwrap(),
        b"Bob's neighbouring context must survive Ada's request",
    );

    let restored_admin_url =
        restore_database(&admin, &migration_url, &scratch_database, &dump_path).await;
    let mut restored_config = app_config();
    restored_config.database_url =
        database_url_for(&restored_config.database_url, &scratch_database);
    restored_config.database_migration_url = restored_admin_url;
    let restored = SubstrateProvider::connect(restored_config, 2)
        .await
        .expect("connect to the restored database as the application role");
    let restored_kms = Arc::new(
        DurableKmsBacking::new(restored.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("load the resurrected Chat key hierarchy"),
    );
    let restored_messages =
        PgMessageStore::new(restored.db_pool().clone(), region.as_str(), table.clone());

    assert_eq!(
        decrypt_body(&restored_kms, &region, &private_envelope).unwrap(),
        b"Ada's private launch concern must not return after a restore",
        "the real restore has teeth: it resurrects the exact body that was later erased",
    );
    assert_eq!(
        restored_messages
            .range(&conversation, RangeCursor::Recent, 10)
            .await
            .expect("read the restored conversation")
            .len(),
        2,
    );

    let re_erased = PostRestoreChatMessageReEraser::new(
        ledger.clone(),
        restored_messages.clone(),
        Arc::clone(&restored_kms),
    )
    .run(restored_to, &event_ids, now())
    .await
    .expect("replay the live Chat erasure ledger into the restored database");
    assert_eq!(re_erased.selected_subjects, 1);
    assert_eq!(re_erased.newly_re_erased_subjects, 1);
    assert_eq!(re_erased.already_erased_subjects, 0);
    assert_eq!(
        (
            re_erased.messages_erased,
            re_erased.erasure_events_co_committed
        ),
        (1, 1),
    );
    assert!(
        decrypt_body(&restored_kms, &region, &private_envelope).is_err(),
        "re-erasure destroys the Chat key resurrected by the backup",
    );
    assert_eq!(
        decrypt_body(&restored_kms, &region, &neighbour_envelope).unwrap(),
        b"Bob's neighbouring context must survive Ada's request",
    );
    let restored_history = restored_messages
        .range(&conversation, RangeCursor::Recent, 10)
        .await
        .expect("read the safely re-erased conversation");
    assert_eq!(
        restored_history
            .iter()
            .find(|message| message.author == event_actor_pseudonym(&tenant, "ada"))
            .expect("Ada's stable message coordinate remains")
            .state,
        MessageState::Tombstoned,
    );
    assert_eq!(
        restored_history
            .iter()
            .find(|message| message.author == event_actor_pseudonym(&tenant, "bob"))
            .expect("Bob's neighbouring message remains")
            .state,
        MessageState::Active,
    );

    let (new_message, new_envelope) = encrypted_message(
        &restored_kms,
        conversation.clone(),
        "ada",
        "ada-after-recovery",
        "Ada may choose to speak again after her narrow Chat-history erasure",
    );
    restored_messages
        .append_co_commit(
            &message_ids,
            new_message,
            event_ids.mint().into(),
            MessageAttribution::new(actor(&tenant, "ada"), PrincipalId("ada".into())),
            now(),
            now(),
        )
        .await
        .expect("a narrow Chat-history request does not erase Ada's right to speak again");
    let resumed = PostRestoreChatMessageReEraser::new(
        ledger,
        restored_messages.clone(),
        Arc::clone(&restored_kms),
    )
    .run(restored_to, &UlidMinter::new(), now())
    .await
    .expect("a response-lost operator invocation resumes safely");
    assert_eq!(resumed.newly_re_erased_subjects, 0);
    assert_eq!(resumed.already_erased_subjects, 1);
    assert_eq!(
        decrypt_body(&restored_kms, &region, &new_envelope).unwrap(),
        b"Ada may choose to speak again after her narrow Chat-history erasure",
        "replaying the restore receipt never consumes newly authored history",
    );

    restored.db_pool().close().await;
    sqlx::raw_sql(&format!("DROP DATABASE {scratch_database} WITH (FORCE)"))
        .execute(admin.db_pool())
        .await
        .expect("remove the isolated restored database");
    let _ = std::fs::remove_file(&dump_path);
    sqlx::raw_sql(&format!(
        "DROP TABLE IF EXISTS {table}_thread_participant; \
         DROP TABLE IF EXISTS {table}_erasure_operation; \
         DROP TABLE IF EXISTS {table}"
    ))
    .execute(admin.db_pool())
    .await
    .expect("remove the isolated live Chat tables");
    sqlx::query(
        "DELETE FROM post_pit_erasure_ledger WHERE tenant_id = $1 AND region = $2 AND scope = 'chat'",
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
