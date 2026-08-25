#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    decode_encrypted_body, decrypt_body, encode_encrypted_body, encrypt_body, subject_dek_erasure,
    ChatFreeText, DurableChatMessageEraser, PostRestoreChatMessageReEraser,
    CHAT_ERASE_CASCADE_TOKEN,
};
use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, Timestamp, Ulid, UlidMinter};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use myelin_storage::kms::{KekId, KmsEngine};
use myelin_storage::{DurablePostPitLedger, PostPitErasureScope, SubstrateProvider};
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

async fn fresh_store() -> (sqlx::PgPool, PgMessageStore, String, SubstrateProvider) {
    fresh_store_in(region()).await
}

async fn fresh_store_in(
    store_region: Region,
) -> (sqlx::PgPool, PgMessageStore, String, SubstrateProvider) {
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
    PgMessageStore::new(admin.clone(), store_region.as_str(), table.clone())
        .migrate()
        .await
        .expect("create the same tenant-scoped schema used by the production message store");
    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .expect("the durable outbox schema is available");
    let mut config = MyelinConfig::dev();
    config.database_url = app_url();
    config.region = store_region.0.clone();
    let app = SubstrateProvider::connect(config, 4)
        .await
        .expect("connect to dev Postgres as the production application role");
    let store = PgMessageStore::new(app.db_pool().clone(), store_region.as_str(), table.clone());
    (admin, store, table, app)
}

fn encrypted_message(
    kms: &KmsEngine,
    conversation: ConversationId,
    author: &str,
    nonce: &str,
    plaintext: &str,
) -> (NewMessage, EncryptedColumn) {
    let tenant = TenantId::from_token(&conversation.tenant);
    let author = SubjectId::new(author);
    let message_region = Region(conversation.region.clone());
    let body = encrypt_body(
        kms,
        &message_region,
        &tenant,
        &author,
        ChatFreeText::BodyInline,
        plaintext.as_bytes(),
    )
    .expect("seal the message body under its author's Chat key");
    let nodes = encrypt_body(
        kms,
        &message_region,
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

fn legacy_encrypted_message(
    kms: &KmsEngine,
    conversation: ConversationId,
    author: &str,
) -> NewMessage {
    let tenant = TenantId::from_token(&conversation.tenant);
    let author = SubjectId::new(author);
    let message_region = Region(conversation.region.clone());
    kms.ensure_kek(&KekId::new(tenant.clone(), message_region.clone()))
        .expect("arrange the legacy tenant key");
    let cryptor = ColumnCryptor::new(kms, message_region);
    let seal = |plaintext: &[u8]| {
        cryptor
            .encrypt(&tenant, Some(&author), &subject_dek_erasure(), plaintext)
            .expect("seal a legacy unscoped Chat body")
    };
    NewMessage {
        conv: conversation,
        thread_root_id: None,
        author: author.0.clone(),
        author_kind: AuthorKind::Human,
        body_inline: encode_encrypted_body(&seal(b"legacy private body"))
            .expect("encode the legacy body"),
        body_nodes: encode_encrypted_body(&seal(b"[]")).expect("encode the legacy nodes"),
        client_nonce: "legacy-message".into(),
    }
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
    let (admin, store, table, provider) = fresh_store().await;
    let kms = Arc::new(KmsEngine::new());
    let ledger = DurablePostPitLedger::new(provider);
    let eraser = DurableChatMessageEraser::new(store.clone(), Arc::clone(&kms), ledger.clone());
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

    let proof = eraser
        .erase_subject_messages(TENANT, ADA, &event_ids, erasure_attempt(operation))
        .await
        .expect("the durable eraser records, shreds, and tombstones Ada's messages");
    assert_eq!(
        (proof.messages_erased, proof.erasure_events_co_committed,),
        (2, 2),
        "both authored messages and both durable consequences move together",
    );
    assert!(proof.key_destroyed_this_attempt);
    assert!(!proof.already_completed);
    assert_eq!(proof.destroyed_key_epoch, Some(0));
    assert!(proof.key_unrecoverable);

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

    let retry = eraser
        .erase_subject_messages(TENANT, ADA, &event_ids, erasure_attempt(operation))
        .await
        .expect("a delivery retry returns the durable receipt");
    assert_eq!(
        (retry.messages_erased, retry.erasure_events_co_committed,),
        (2, 2),
        "a completed message erasure replays its original proof without another event",
    );
    assert!(
        retry.already_completed
            && !retry.key_destroyed_this_attempt
            && retry.destroyed_key_epoch.is_none(),
        "replay must not mistake a later Chat key for the key destroyed by the operation",
    );
    assert_eq!(
        store
            .prepare_author_erasure(TENANT, &ada, operation)
            .await
            .expect("the durable marker remains readable after response loss"),
        AuthoredMessageErasureState::Completed(
            myelin_chat::store::pg::AuthoredMessageEraseReceipt {
                messages_tombstoned: 2,
                erasure_events_co_committed: 2,
            },
        ),
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

    assert!(
        decrypt_body(&kms, &region(), &first_envelope).is_err()
            && decrypt_body(&kms, &region(), &second_envelope).is_err(),
        "the tombstones and Chat-scoped key together leave no recoverable authored body",
    );
    let post_pit = ledger
        .completed_after(PostPitErasureScope::Chat, 0)
        .await
        .expect("the post-restore ledger remains queryable");
    assert!(
        post_pit.iter().any(|entry| {
            entry.tenant == TenantId::from_token(TENANT) && entry.subject == SubjectId::new(ADA)
        }),
        "restore recovery can discover and reapply this Chat erasure",
    );

    clear_test_rows(&admin, &table, &[planning, incident]).await;
}

#[tokio::test]
async fn an_erasure_event_failure_leaves_a_resumable_storage_transaction() {
    let (admin, store, table, provider) = fresh_store().await;
    let kms = Arc::new(KmsEngine::new());
    let eraser = DurableChatMessageEraser::new(
        store.clone(),
        Arc::clone(&kms),
        DurablePostPitLedger::new(provider),
    );
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
    let error = eraser
        .erase_subject_messages(
            TENANT,
            ADA,
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
    let stranded_body = decode_encrypted_body(&messages[0].body_inline)
        .expect("the rolled-back row still contains its encrypted envelope");
    assert!(
        decrypt_body(&kms, &region(), &stranded_body).is_err(),
        "the irreversible key step stays honest even when the database step must resume",
    );
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

    let resumed = eraser
        .erase_subject_messages(TENANT, ADA, &UlidMinter::new(), erasure_attempt(operation))
        .await
        .expect("a retry can finish storage after the irreversible key step");
    assert_eq!(
        (resumed.messages_erased, resumed.erasure_events_co_committed),
        (2, 2),
    );
    assert!(
        !resumed.already_completed
            && !resumed.key_destroyed_this_attempt
            && resumed.destroyed_key_epoch.is_none(),
        "the retry observes that the old key was already destroyed",
    );
    let finished = store
        .range(&conversation, RangeCursor::Recent, 10)
        .await
        .expect("read the room after resuming the operation");
    assert!(finished.iter().all(|message| {
        message.state == MessageState::Tombstoned
            && message.body_inline.is_empty()
            && message.body_nodes.is_empty()
    }));

    clear_test_rows(&admin, &table, &[conversation]).await;
}

#[tokio::test]
async fn a_restored_database_replays_only_chat_erasures_newer_than_its_restore_point() {
    let restore_region = Region::new(format!("chat-restore-{}", std::process::id()));
    let (admin, store, table, provider) = fresh_store_in(restore_region.clone()).await;
    let kms = Arc::new(KmsEngine::new());
    let ledger = DurablePostPitLedger::new(provider);
    let ids = MonotonicUlidSource::new();
    let tenant = format!("chat-restore-{}", std::process::id());
    let ada = event_actor_pseudonym(&tenant, ADA);
    let bob = event_actor_pseudonym(&tenant, BOB);
    let conversation = ConversationId::new(
        &tenant,
        restore_region.as_str(),
        "01J0CHATP411RESTORED00000",
    );
    let (erased, erased_envelope) = encrypted_message(
        &kms,
        conversation.clone(),
        &ada,
        "restored-ada",
        "Ada's erased message came back with the database",
    );
    let (preserved, preserved_envelope) = encrypted_message(
        &kms,
        conversation.clone(),
        &bob,
        "restored-bob",
        "Bob's message predates the restore boundary and remains",
    );
    store.append_storage_only(&ids, erased).await.unwrap();
    store.append_storage_only(&ids, preserved).await.unwrap();
    ledger
        .record(
            PostPitErasureScope::Chat,
            &TenantId::from_token(&tenant),
            &SubjectId::new(ADA),
            200,
        )
        .await
        .unwrap();
    ledger
        .record(
            PostPitErasureScope::Chat,
            &TenantId::from_token(&tenant),
            &SubjectId::new(BOB),
            50,
        )
        .await
        .unwrap();

    let reeraser = PostRestoreChatMessageReEraser::new(ledger, store.clone(), Arc::clone(&kms));
    let first = reeraser
        .run(100, &UlidMinter::new(), now())
        .await
        .expect("reapply post-restore Chat erasures from the preserved live ledger");
    assert_eq!(first.selected_subjects, 1);
    assert_eq!(first.newly_re_erased_subjects, 1);
    assert_eq!(first.already_erased_subjects, 0);
    assert_eq!(
        (first.messages_erased, first.erasure_events_co_committed),
        (1, 1)
    );
    assert!(decrypt_body(&kms, &restore_region, &erased_envelope).is_err());
    assert_eq!(
        decrypt_body(&kms, &restore_region, &preserved_envelope).unwrap(),
        b"Bob's message predates the restore boundary and remains",
    );

    let replay = reeraser
        .run(100, &UlidMinter::new(), now())
        .await
        .expect("the restore operator is safe to resume after response loss");
    assert_eq!(replay.newly_re_erased_subjects, 0);
    assert_eq!(replay.already_erased_subjects, 1);
    assert_eq!(
        (replay.messages_erased, replay.erasure_events_co_committed),
        (1, 1)
    );
    let messages = store
        .range(&conversation, RangeCursor::Recent, 10)
        .await
        .unwrap();
    assert_eq!(
        messages
            .iter()
            .find(|message| message.author == ada)
            .unwrap()
            .state,
        MessageState::Tombstoned,
    );
    assert_eq!(
        messages
            .iter()
            .find(|message| message.author == bob)
            .unwrap()
            .state,
        MessageState::Active,
    );

    clear_test_rows(&admin, &table, &[conversation]).await;
}

#[tokio::test]
async fn a_legacy_cross_product_key_is_never_misreported_as_safely_shredded() {
    let (admin, store, table, provider) = fresh_store().await;
    let kms = Arc::new(KmsEngine::new());
    let eraser = DurableChatMessageEraser::new(
        store.clone(),
        Arc::clone(&kms),
        DurablePostPitLedger::new(provider),
    );
    let ids = MonotonicUlidSource::new();
    let event_ids = UlidMinter::new();
    let ada = event_actor_pseudonym(TENANT, ADA);
    let conversation = ConversationId::new(TENANT, region().as_str(), "01J0CHATP411LEGACY000000");
    store
        .append_storage_only(
            &ids,
            legacy_encrypted_message(&kms, conversation.clone(), &ada),
        )
        .await
        .expect("arrange one message from before Chat had an independent key scope");
    let operation = "privacy-request:ada-legacy";
    store
        .prepare_author_erasure(TENANT, &ada, operation)
        .await
        .expect("persist the attempted erasure");

    let error = eraser
        .erase_subject_messages(TENANT, ADA, &event_ids, erasure_attempt(operation))
        .await
        .expect_err("a shared legacy key cannot support an honest Chat-only certificate");
    assert!(error
        .to_string()
        .contains("refuses legacy or foreign key scope"));
    let message = store
        .range(&conversation, RangeCursor::Recent, 1)
        .await
        .expect("inspect the refused mutation")
        .pop()
        .expect("the legacy message remains");
    assert_eq!(message.state, MessageState::Active);
    assert!(
        !message.body_inline.is_empty(),
        "a refusal does not destroy data while claiming success",
    );
    let envelope = decode_encrypted_body(&message.body_inline).expect("decode the legacy body");
    assert_eq!(
        decrypt_body(&kms, &region(), &envelope).expect("the shared legacy key remains intact"),
        b"legacy private body",
    );

    clear_test_rows(&admin, &table, &[conversation]).await;
}
