#![cfg(feature = "integration")]

use myelin_chat::store::pg::PgMessageStore;
use myelin_chat::store::{AuthorKind, ConversationId, MonotonicUlidSource, NewMessage};
use myelin_content::InlineNode;
use myelin_events::{Actor, ArtifactRef, IdMinter, Timestamp, Ulid, UlidMinter};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn region() -> &'static str {
    "fr-par"
}

fn actor() -> Actor {
    Actor(Principal::stub(
        PrincipalId("alice".into()),
        PrincipalKind::Human,
        TenantId("acmeP399".into()),
    ))
}

fn now() -> Timestamp {
    Timestamp("2026-06-24T00:00:00Z".into())
}

struct ConstantMinter(Ulid);

impl IdMinter for ConstantMinter {
    fn mint(&self) -> Ulid {
        self.0.clone()
    }
}

fn new_msg(conv: &ConversationId, nonce: &str, author: &str, body: &str) -> NewMessage {
    NewMessage {
        conv: conv.clone(),
        thread_root_id: None,
        author: author.into(),
        author_kind: AuthorKind::Human,
        body_inline: body.as_bytes().to_vec(),
        body_nodes: Vec::new(),
        client_nonce: nonce.into(),
    }
}

async fn fresh_store() -> (sqlx::PgPool, String, PgMessageStore, String) {
    let admin = PgPoolOptions::new()
        .max_connections(6)
        .connect(&admin_url())
        .await
        .expect(
            "connect to dev Postgres as admin (is the stack up? \
             run `fed test:backend`)",
        );
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    );
    let table = format!("message_p399_{suffix}");
    let store = PgMessageStore::new(admin.clone(), region(), table.clone());
    store.migrate().await.expect("apply the message DDL + RLS");
    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .expect("apply the frozen outbox migration");
    (admin, table, store, suffix)
}

async fn drop_store(admin: &sqlx::PgPool, table: &str) {
    sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
        .execute(admin)
        .await
        .expect("drop the isolated message table");
}

async fn delete_outbox_aggregate(admin: &sqlx::PgPool, aggregate: &str) {
    let mut transaction = admin.begin().await.expect("begin outbox cleanup");
    sqlx::query("SELECT event_id FROM outbox WHERE aggregate = $1 FOR UPDATE")
        .bind(aggregate)
        .fetch_all(&mut *transaction)
        .await
        .expect("lock this test's outbox rows");
    sqlx::query("DELETE FROM outbox_quarantine WHERE aggregate = $1")
        .bind(aggregate)
        .execute(&mut *transaction)
        .await
        .expect("delete this test's quarantine rows");
    sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
        .bind(aggregate)
        .execute(&mut *transaction)
        .await
        .expect("delete this test's outbox rows");
    transaction.commit().await.expect("commit outbox cleanup");
}

#[tokio::test]
async fn chat_p5_co_commit_idempotent_send_and_per_conversation_order() {
    let (admin, table, store, suffix) = fresh_store().await;

    let conv = ConversationId::new("acmeP399", region(), format!("01J0CONVP399{suffix}"));
    let src = MonotonicUlidSource::new();
    let event_ids = UlidMinter::new();

    delete_outbox_aggregate(&admin, &conv.conversation_id).await;

    let id0 = store
        .append_co_commit(
            &src,
            new_msg(&conv, "n0", "alice", "hello world"),
            event_ids.mint().into(),
            actor(),
            now(),
            now(),
        )
        .await
        .expect("co-commit append");

    let msg_count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE conversation_id = $1"
    ))
    .bind(&conv.conversation_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        msg_count, 1,
        "CHAT-D13: the message row is present after co-commit"
    );

    let ob_rows = sqlx::query(
        "SELECT event_id, aggregate, seq, subject, envelope FROM outbox WHERE aggregate = $1 ORDER BY seq",
    )
    .bind(&conv.conversation_id)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        ob_rows.len(),
        1,
        "CHAT-D13: exactly one co-committed event, 0 phantom"
    );
    let envelope: serde_json::Value = ob_rows[0].get("envelope");
    assert_eq!(
        envelope["type_"], "chat.message.created",
        "the co-committed event is chat.message.created"
    );
    assert_eq!(
        ob_rows[0].get::<String, _>("aggregate"),
        conv.conversation_id,
        "aggregate = conversation_id (contract 2.3)"
    );
    assert_eq!(
        ob_rows[0].get::<i64, _>("seq"),
        0,
        "first event for the aggregate is seq 0"
    );
    let payload_str = envelope["payload"].to_string();
    assert!(
        !payload_str.contains("hello world"),
        "the body bytes must NEVER ride the bus: {payload_str}"
    );
    assert!(
        ob_rows[0].get::<String, _>("subject").contains("#message-"),
        "subject is the stable message-<id> #sub anchor"
    );

    let id0_retry = store
        .append_co_commit(
            &src,
            new_msg(&conv, "n0", "alice", "hello world (retry)"),
            event_ids.mint().into(),
            actor(),
            now(),
            now(),
        )
        .await
        .expect("co-commit retry");
    assert_eq!(
        id0_retry, id0,
        "CHAT-D14: a retried send dedups to the existing id"
    );

    let msg_count2: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE conversation_id = $1"
    ))
    .bind(&conv.conversation_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(msg_count2, 1, "CHAT-D14: message-count = 1 (no second row)");
    let ob_count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
        .bind(&conv.conversation_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        ob_count, 1,
        "CHAT-D14: exactly one event (the retry emitted none)"
    );

    const N: usize = 24;
    for i in 1..=N {
        store
            .append_co_commit(
                &src,
                new_msg(&conv, &format!("n{i}"), "alice", &format!("m{i}")),
                event_ids.mint().into(),
                actor(),
                now(),
                now(),
            )
            .await
            .expect("burst co-commit");
    }
    let mut seqs: Vec<i64> = sqlx::query_scalar("SELECT seq FROM outbox WHERE aggregate = $1")
        .bind(&conv.conversation_id)
        .fetch_all(&admin)
        .await
        .unwrap();
    seqs.sort_unstable();
    let expected: Vec<i64> = (0..=N as i64).collect();
    assert_eq!(
        seqs, expected,
        "CHAT-D2: per-conversation seqs are contiguous + gap-free + no-dup (0 ordering violations)"
    );

    delete_outbox_aggregate(&admin, &conv.conversation_id).await;
    drop_store(&admin, &table).await;
}

#[tokio::test]
async fn a_structured_reference_is_one_atomic_durable_action() {
    let (admin, table, store, suffix) = fresh_store().await;
    let src = MonotonicUlidSource::new();
    let event_ids = UlidMinter::new();

    let referenced_conv =
        ConversationId::new("acmeP399", region(), format!("01J0REFSP399{suffix}"));
    let target = ArtifactRef("myelin://acmeP399/issue/issue/ENG-41".into());
    let nodes = vec![InlineNode::ArtifactRefNode(target.clone())];
    let source_message_id = store
        .append_structured_co_commit(
            &src,
            NewMessage {
                conv: referenced_conv.clone(),
                thread_root_id: None,
                author: "alice".into(),
                author_kind: AuthorKind::Human,
                body_inline: b"encrypted-inline-envelope".to_vec(),
                body_nodes: b"encrypted-node-envelope".to_vec(),
                client_nonce: "reference-issue-41".into(),
            },
            event_ids.mint().into(),
            &event_ids,
            &nodes,
            actor(),
            now(),
            now(),
        )
        .await
        .expect("co-commit a message and its structured reference");
    let source = myelin_chat::subs::mint_message("acmeP399", source_message_id.as_str()).unwrap();
    let edge_aggregate = myelin_chat::content::edge_aggregate_key(&source, &target);
    let reference_rows = sqlx::query(
        "SELECT aggregate, envelope FROM outbox WHERE aggregate = $1 OR aggregate = $2",
    )
    .bind(&referenced_conv.conversation_id)
    .bind(&edge_aggregate.0)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        reference_rows.len(),
        2,
        "one user action durably records exactly its message event and reference edge"
    );
    let envelopes = reference_rows
        .iter()
        .map(|row| row.get::<serde_json::Value, _>("envelope"))
        .collect::<Vec<_>>();
    let message_event = envelopes
        .iter()
        .find(|envelope| envelope["type_"] == "chat.message.created")
        .expect("the message event is co-committed");
    let edge_event = envelopes
        .iter()
        .find(|envelope| envelope["type_"] == "refs.edge.created")
        .expect("the reference edge is co-committed");
    assert_eq!(edge_event["subject"], source.0);
    assert_eq!(edge_event["payload"]["source"], source.0);
    assert_eq!(edge_event["payload"]["target"], target.0);
    assert_eq!(edge_event["payload"]["rel"], "links");
    assert_eq!(
        edge_event["causation_id"], message_event["event_id"],
        "the edge is causally downstream of the message that introduced it"
    );
    assert_eq!(
        edge_event["correlation_id"], message_event["correlation_id"],
        "the message and edge remain one trace"
    );
    assert_eq!(edge_event["depth"], 1);

    let stored_nodes: Vec<u8> = sqlx::query_scalar(&format!(
        "SELECT body_nodes FROM {table} WHERE conversation_id = $1 AND message_id = $2"
    ))
    .bind(&referenced_conv.conversation_id)
    .bind(source_message_id.as_str())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        stored_nodes, b"encrypted-node-envelope",
        "the store persists only the caller's encrypted node envelope"
    );

    let rollback_conv = ConversationId::new("acmeP399", region(), format!("01J0ROLLP399{suffix}"));
    let colliding_id = event_ids.mint();
    let error = store
        .append_structured_co_commit(
            &src,
            NewMessage {
                conv: rollback_conv.clone(),
                thread_root_id: None,
                author: "alice".into(),
                author_kind: AuthorKind::Human,
                body_inline: b"encrypted-inline-envelope".to_vec(),
                body_nodes: b"encrypted-node-envelope".to_vec(),
                client_nonce: "must-roll-back".into(),
            },
            colliding_id.clone().into(),
            &ConstantMinter(colliding_id.clone()),
            &nodes,
            actor(),
            now(),
            now(),
        )
        .await
        .expect_err("a divergent event-id collision must abort the entire append");
    assert!(error.to_string().contains("divergent"));
    let rolled_back_messages: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE conversation_id = $1"
    ))
    .bind(&rollback_conv.conversation_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    let rolled_back_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE event_id = $1")
            .bind(&colliding_id.0)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        (rolled_back_messages, rolled_back_events),
        (0, 0),
        "a failed reference edge leaves neither a visible message nor an orphan event"
    );

    delete_outbox_aggregate(&admin, &referenced_conv.conversation_id).await;
    delete_outbox_aggregate(&admin, &edge_aggregate.0).await;
    drop_store(&admin, &table).await;
}
