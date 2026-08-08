#![cfg(feature = "integration")]

use myelin_chat::store::pg::PgMessageStore;
use myelin_chat::store::{AuthorKind, ConversationId, MonotonicUlidSource, NewMessage};
use myelin_events::relay::InProcessBus;
use myelin_events::{Actor, EventId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::pgrelay::PgRelay;
use myelin_tenancy::TenantId;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

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

fn event_id(n: usize) -> EventId {
    EventId(format!("01JCHATP399{n:020}"))
}

#[tokio::test]
async fn chat_p5_co_commit_idempotent_send_and_per_conversation_order() {
    let admin = PgPoolOptions::new()
        .max_connections(6)
        .connect(&admin_url())
        .await
        .expect(
            "connect to dev Postgres as admin (is the stack up? \
             run `fed test:backend`)",
        );

    let suffix = std::process::id();
    let table = format!("message_p399_{suffix}");

    let store = PgMessageStore::new(admin.clone(), region(), table.clone());
    store.migrate().await.expect("apply the message DDL + RLS");

    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .expect("apply the frozen outbox migration");

    let conv = ConversationId::new("acmeP399", region(), format!("01J0CONVP399{suffix}"));
    let src = MonotonicUlidSource::new();

    sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
        .bind(&conv.conversation_id)
        .execute(&admin)
        .await
        .ok();

    let id0 = store
        .append_co_commit(
            &src,
            new_msg(&conv, "n0", "alice", "hello world"),
            event_id(0),
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

    let relay = PgRelay::new(admin.clone());
    let bus = InProcessBus::new();
    let drained = relay.relay_once(&bus, 64).await.expect("relay drain");
    assert_eq!(
        drained, 1,
        "CHAT-D13: the co-committed event is delivered exactly once"
    );
    assert_eq!(
        bus.delivered_count(),
        1,
        "0 phantom: exactly one distinct event on the bus"
    );
    assert_eq!(
        relay.outbox_depth().await.unwrap(),
        0,
        "the outbox drains to depth 0 (0 orphan)"
    );

    let id0_retry = store
        .append_co_commit(
            &src,
            new_msg(&conv, "n0", "alice", "hello world (retry)"),
            event_id(999),
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
                event_id(i),
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

    sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
        .bind(&conv.conversation_id)
        .execute(&admin)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
        .execute(&admin)
        .await
        .ok();
}
