#![cfg(feature = "integration")]

use myelin_chat::{
    decrypt_body, encrypt_body, ChatErasureCascade, ChatFreeText, ConversationId, MemDraftStore,
    MemHotTier, MessageId, ReadStateRecord, UnfurlCache, CHAT_ERASE_CASCADE_TOKEN,
};
use myelin_events::{Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp};
use myelin_gdpr::{EraseScope, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::SubjectId;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;
use std::sync::Arc;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

const SUBJECT: &str = "8a2f@acme.noreply";

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acmeP411".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acmeP411".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

#[tokio::test]
async fn chat_erase_cascade_zero_recoverable_pii_at_rest_in_real_postgres() {
    let engine = myelin_storage::kms::KmsEngine::new();
    let tenant = TenantId::from_token("acmeP411");
    let region = Region::new("fr-par");
    let author = SubjectId::new(SUBJECT);

    let plaintext =
        b"my private chat: my email is ada@example.com and my health is private".to_vec();
    let col = encrypt_body(
        &engine,
        &region,
        &tenant,
        &author,
        ChatFreeText::BodyInline,
        &plaintext,
    )
    .expect("seal body_inline under the per-subject DEK");
    assert!(col.key_ref.class.as_token().starts_with("subject:"));

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let tbl = format!("chat_msg_p411_{suffix}");
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (\
           message_id TEXT PRIMARY KEY, \
           author_pseudonym TEXT NOT NULL, \
           body_inline BYTEA NOT NULL, \
           body_inline_pii_key_ref TEXT NOT NULL, \
           body_inline_nonce BYTEA NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create message table");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant message table");

    sqlx::query(&format!(
        "INSERT INTO {tbl} (message_id, author_pseudonym, body_inline, body_inline_pii_key_ref, body_inline_nonce) \
         VALUES ('01J0MSG411', $1, $2, $3, $4)"
    ))
    .bind(SUBJECT)
    .bind(&col.ciphertext)
    .bind(col.key_ref.to_uri())
    .bind(col.nonce.as_slice())
    .execute(&app)
    .await
    .expect("write the at-rest message row");

    let row = sqlx::query(&format!(
        "SELECT body_inline FROM {tbl} WHERE message_id = '01J0MSG411'"
    ))
    .fetch_one(&app)
    .await
    .expect("read the at-rest row back");
    let at_rest: Vec<u8> = row.get("body_inline");
    let mut from_db = col.clone();
    from_db.ciphertext = at_rest.clone();
    assert_eq!(
        decrypt_body(&engine, &region, &from_db).expect("decrypt while the key lives"),
        plaintext,
        "the at-rest body decrypts before the erase"
    );

    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();
    let cascade = ChatErasureCascade::new(
        &engine,
        region.clone(),
        &store,
        &read_state,
        &drafts,
        &cache,
    );

    let conv = ConversationId::new("acmeP411", "fr-par", "c-1");
    let subject = SubjectRef::new(Principal::stub(
        PrincipalId(SUBJECT.into()),
        PrincipalKind::Human,
        GdprTenantId::from_token("acmeP411"),
    ));
    let mut tx = outbox.begin(minter.clone(), ctx_base());
    let report = cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject,
            tenant: GdprTenantId::from_token("acmeP411"),
        },
        &[(conv, MessageId("01J0MSG411".into()))],
    );
    tx.commit().expect("commit the cascade transaction");

    let row_after = sqlx::query(&format!(
        "SELECT body_inline FROM {tbl} WHERE message_id = '01J0MSG411'"
    ))
    .fetch_one(&app)
    .await
    .expect("read the at-rest row back after the cascade");
    let at_rest_after: Vec<u8> = row_after.get("body_inline");
    assert_eq!(
        at_rest_after, at_rest,
        "the ciphertext is unchanged at rest (the immutable log is not rewritten - only the key is shredded)"
    );
    let mut from_db_after = col.clone();
    from_db_after.ciphertext = at_rest_after;
    assert!(
        decrypt_body(&engine, &region, &from_db_after).is_err(),
        "0 recoverable PII: after the cascade's crypto-shred the at-rest body is unrecoverable - never plaintext"
    );

    assert!(
        report.receipts_complete(),
        "the holder-receipt set is complete (0 holders missed)"
    );
    assert!(
        report.destroyed_key_epoch.is_some(),
        "the destroyed-key epoch is recorded (the post-restore re-erase audit trail, 10.8)"
    );
    let erased = outbox
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
        .count();
    assert_eq!(
        erased, 1,
        "the chat.message.erased tombstone is published (the DSR cascade - the bus is the only path)"
    );

    let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await;
}
