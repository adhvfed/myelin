#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_chat::events::{
    channel_aggregate, event_actor_pseudonym, pseudonymized_event_principal,
};
use myelin_chat::store::pg::{
    AuthoredMessageErasureState, MessageAttribution, MessageErasureAttempt, PgMessageStore,
};
use myelin_chat::store::{
    AuthorKind, ConversationId, MessageState, MonotonicUlidSource, NewMessage, RangeCursor,
};
use myelin_chat::{
    chat_subject_key_class, decode_encrypted_body, decrypt_body, encode_encrypted_body,
    encrypt_body, ChatFreeText, CHAT_ERASE_CASCADE_TOKEN,
};
use myelin_events::{Actor, IdMinter, Timestamp, Ulid, UlidMinter};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{DekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

static NEXT_STORE: AtomicU64 = AtomicU64::new(0);
const TENANT: &str = "acmeP411";
const ADA: &str = "ada";
const BOB: &str = "bob";

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn region() -> Region {
    Region::new("fr-par")
}

fn now() -> Timestamp {
    Timestamp("2026-06-21T00:00:00Z".into())
}

fn erasure_actor(raw_principal: &str) -> Actor {
    let principal = Principal::stub(
        PrincipalId(raw_principal.into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    Actor(pseudonymized_event_principal(TENANT, &principal))
}

fn erasure_attempt(operation_id: &str) -> MessageErasureAttempt {
    MessageErasureAttempt::new(operation_id, erasure_actor(ADA), now(), now())
}

struct ConstantMinter(Ulid);

impl IdMinter for ConstantMinter {
    fn mint(&self) -> Ulid {
        self.0.clone()
    }
}

async fn fresh_store() -> (sqlx::PgPool, PgMessageStore, String) {
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed),
    );
    let table = format!("chat_message_erase_{suffix}");
    PgMessageStore::new(admin.clone(), region().as_str(), table.clone())
        .migrate()
        .await
        .expect("create the same tenant-scoped schema used by the production message store");
    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .expect("the durable outbox schema is available");
    let app = PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the production application role");
    let store = PgMessageStore::new(app, region().as_str(), table.clone());
    (admin, store, table)
}

fn encrypted_message(
    kms: &KmsEngine,
    conversation: ConversationId,
    author: &str,
    nonce: &str,
    plaintext: &str,
) -> (NewMessage, EncryptedColumn) {
    let tenant = TenantId::from_token(TENANT);
    let author = SubjectId::new(author);
    let body = encrypt_body(
        kms,
        &region(),
        &tenant,
        &author,
        ChatFreeText::BodyInline,
        plaintext.as_bytes(),
    )
    .expect("seal the message body under its author's Chat key");
    let nodes = encrypt_body(
        kms,
        &region(),
        &tenant,
        &author,
        ChatFreeText::BodyNodes,
        b"[]",
    )
    .expect("seal the structured body under the same Chat key");
    (
        NewMessage {
            conv: conversation,
            thread_root_id: None,
            author: author.0,
            author_kind: AuthorKind::Human,
            body_inline: encode_encrypted_body(&body).expect("encode the encrypted body"),
            body_nodes: encode_encrypted_body(&nodes).expect("encode the encrypted nodes"),
            client_nonce: nonce.into(),
        },
        body,
    )
}

async fn clear_test_rows(admin: &sqlx::PgPool, table: &str, conversations: &[ConversationId]) {
    let mut transaction = admin.begin().await.expect("begin isolated cleanup");
    for conversation in conversations {
        let aggregate = channel_aggregate(&conversation.conversation_id).0;
        sqlx::query("DELETE FROM outbox_quarantine WHERE aggregate = $1")
            .bind(&aggregate)
            .execute(&mut *transaction)
            .await
            .expect("clear this story's quarantined events");
        sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
            .bind(&aggregate)
            .execute(&mut *transaction)
            .await
            .expect("clear this story's outbox events");
    }
    sqlx::query(&format!("DROP TABLE IF EXISTS {table}_thread_participant"))
        .execute(&mut *transaction)
        .await
        .expect("drop the isolated thread participant table");
    sqlx::query(&format!("DROP TABLE IF EXISTS {table}_erasure_operation"))
        .execute(&mut *transaction)
        .await
        .expect("drop the isolated erasure operation table");
    sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
        .execute(&mut *transaction)
        .await
        .expect("drop the isolated message table");
    transaction.commit().await.expect("commit isolated cleanup");
}

#[tokio::test]
async fn erasing_one_person_tombstones_only_their_real_postgres_messages_and_events() {
    let (admin, store, table) = fresh_store().await;
    let kms = KmsEngine::new();
    let ids = MonotonicUlidSource::new();
    let event_ids = UlidMinter::new();
    let ada = event_actor_pseudonym(TENANT, ADA);
    let bob = event_actor_pseudonym(TENANT, BOB);
    let planning = ConversationId::new(TENANT, region().as_str(), "01J0CHATP411PLANNING00000");
    let incident = ConversationId::new(TENANT, region().as_str(), "01J0CHATP411INCIDENT00000");

    let (first, first_envelope) = encrypted_message(
        &kms,
        planning.clone(),
        &ada,
        "ada-planning",
        "My private launch concern belongs to Ada",
    );
    let (second, second_envelope) = encrypted_message(
        &kms,
        incident.clone(),
        &ada,
        "ada-incident",
        "My private incident note also belongs to Ada",
    );
    let (neighbour, _) = encrypted_message(
        &kms,
        planning.clone(),
        &bob,
        "bob-planning",
        "Bob's nearby message must survive",
    );
    store
        .append_storage_only(&ids, first)
        .await
        .expect("Ada can write in the planning room");
    store
        .append_storage_only(&ids, second)
        .await
        .expect("Ada can write in the incident room");
    store
        .append_storage_only(&ids, neighbour)
        .await
        .expect("Bob can write beside Ada");

    let operation = "privacy-request:ada-2026-06-21";
    assert_eq!(
        store
            .prepare_author_erasure(TENANT, &ada, operation)
            .await
            .expect("persist the operation before irreversible work"),
        AuthoredMessageErasureState::Pending,
    );
    let (late_message, _) = encrypted_message(
        &kms,
        planning.clone(),
        &ada,
        "late-arrival",
        "This must not cross an in-progress erasure",
    );
    let refused = store
        .append_co_commit(
            &ids,
            late_message,
            event_ids.mint().into(),
            MessageAttribution::new(erasure_actor(ADA), PrincipalId(ADA.into())),
            now(),
            now(),
        )
        .await
        .expect_err("a fresh write cannot overtake Ada's in-progress erasure");
    assert!(refused.to_string().contains("erasure is in progress"));

    let receipt = store
        .tombstone_author_co_commit(TENANT, &ada, &event_ids, erasure_attempt(operation))
        .await
        .expect("the production message store completes Ada's erasure atomically");
    assert_eq!(
        (
            receipt.messages_tombstoned,
            receipt.erasure_events_co_committed,
        ),
        (2, 2),
        "both authored messages and both durable consequences move together",
    );

    let planning_messages = store
        .range(&planning, RangeCursor::Recent, 10)
        .await
        .expect("read the planning room after erasure");
    let ada_after = planning_messages
        .iter()
        .find(|message| message.author == ada)
        .expect("Ada's immutable message coordinate remains as a tombstone");
    assert_eq!(ada_after.state, MessageState::Tombstoned);
    assert!(ada_after.body_inline.is_empty() && ada_after.body_nodes.is_empty());
    let bob_after = planning_messages
        .iter()
        .find(|message| message.author == bob)
        .expect("Bob's neighbouring message remains");
    assert_eq!(bob_after.state, MessageState::Active);
    let bob_body = decode_encrypted_body(&bob_after.body_inline).expect("decode Bob's envelope");
    assert_eq!(
        decrypt_body(&kms, &region(), &bob_body).expect("Bob's key remains usable"),
        b"Bob's nearby message must survive",
    );

    let erased_events = sqlx::query(
        "SELECT envelope FROM outbox \
          WHERE aggregate = $1 OR aggregate = $2 ORDER BY aggregate, seq",
    )
    .bind(channel_aggregate(&planning.conversation_id).0)
    .bind(channel_aggregate(&incident.conversation_id).0)
    .fetch_all(&admin)
    .await
    .expect("inspect the durable erasure events");
    assert_eq!(erased_events.len(), 2);
    assert!(erased_events.iter().all(|row| {
        row.get::<serde_json::Value, _>("envelope")["type_"] == CHAT_ERASE_CASCADE_TOKEN
    }));

    let retry = store
        .tombstone_author_co_commit(TENANT, &ada, &event_ids, erasure_attempt(operation))
        .await
        .expect("a delivery retry returns the durable receipt");
    assert_eq!(
        (retry.messages_tombstoned, retry.erasure_events_co_committed,),
        (2, 2),
        "a completed message erasure replays its original proof without another event",
    );
    assert_eq!(
        store
            .prepare_author_erasure(TENANT, &ada, operation)
            .await
            .expect("the durable marker remains readable after response loss"),
        AuthoredMessageErasureState::Completed(receipt.clone()),
    );
    let event_count_after_retry: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1 OR aggregate = $2")
            .bind(channel_aggregate(&planning.conversation_id).0)
            .bind(channel_aggregate(&incident.conversation_id).0)
            .fetch_one(&admin)
            .await
            .expect("count events after replaying the durable receipt");
    assert_eq!(
        event_count_after_retry, 2,
        "receipt replay never republishes a completed erasure",
    );

    kms.destroy_dek(&DekId::new(
        TenantId::from_token(TENANT),
        chat_subject_key_class(&ada),
    ))
    .expect("shred Ada's independent Chat key");
    assert!(
        decrypt_body(&kms, &region(), &first_envelope).is_err()
            && decrypt_body(&kms, &region(), &second_envelope).is_err(),
        "the tombstones and Chat-scoped key together leave no recoverable authored body",
    );

    clear_test_rows(&admin, &table, &[planning, incident]).await;
}

#[tokio::test]
async fn an_erasure_event_failure_restores_every_message_body() {
    let (admin, store, table) = fresh_store().await;
    let kms = KmsEngine::new();
    let ids = MonotonicUlidSource::new();
    let ada = event_actor_pseudonym(TENANT, ADA);
    let conversation = ConversationId::new(TENANT, region().as_str(), "01J0CHATP411ROLLBACK00000");
    for (nonce, body) in [
        ("first", "first body must roll back"),
        ("second", "second body must roll back"),
    ] {
        let (message, _) = encrypted_message(&kms, conversation.clone(), &ada, nonce, body);
        store
            .append_storage_only(&ids, message)
            .await
            .expect("arrange an authored message");
    }

    let operation = "privacy-request:ada-rollback";
    assert_eq!(
        store
            .prepare_author_erasure(TENANT, &ada, operation)
            .await
            .expect("persist the operation before attempting the mutation"),
        AuthoredMessageErasureState::Pending,
    );
    let one_id_for_two_different_events = UlidMinter::new().mint();
    let error = store
        .tombstone_author_co_commit(
            TENANT,
            &ada,
            &ConstantMinter(one_id_for_two_different_events),
            erasure_attempt(operation),
        )
        .await
        .expect_err("a divergent event identity must abort the whole erasure transaction");
    assert!(error.to_string().contains("co-commit Chat message erasure"));

    let messages = store
        .range(&conversation, RangeCursor::Recent, 10)
        .await
        .expect("read the room after the refused transaction");
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|message| {
        message.state == MessageState::Active
            && !message.body_inline.is_empty()
            && !message.body_nodes.is_empty()
    }));
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
        .bind(channel_aggregate(&conversation.conversation_id).0)
        .fetch_one(&admin)
        .await
        .expect("count rolled-back events");
    assert_eq!(
        event_count, 0,
        "neither message mutation nor event may escape a failed co-commit",
    );
    assert_eq!(
        store
            .prepare_author_erasure(TENANT, &ada, operation)
            .await
            .expect("the failed operation remains resumable"),
        AuthoredMessageErasureState::Pending,
    );

    clear_test_rows(&admin, &table, &[conversation]).await;
}
