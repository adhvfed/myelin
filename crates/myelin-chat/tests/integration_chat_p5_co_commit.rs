#![cfg(feature = "integration")]

use myelin_chat::store::pg::{MessageAttribution, PgMessageStore};
use myelin_chat::store::{AuthorKind, ConversationId, NewMessage, StoreError, SystemUlidSource};
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

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn region() -> &'static str {
    "fr-par"
}

fn attribution() -> MessageAttribution {
    attribution_for("alice")
}

fn attribution_for(principal_id: &str) -> MessageAttribution {
    let principal = Principal::stub(
        PrincipalId(principal_id.into()),
        PrincipalKind::Human,
        TenantId("acmeP399".into()),
    );
    MessageAttribution::new(Actor(principal), PrincipalId(principal_id.into()))
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
    PgMessageStore::new(admin.clone(), region(), table.clone())
        .migrate()
        .await
        .expect("apply the message DDL + RLS");
    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .expect("apply the frozen outbox migration");
    let app = PgPoolOptions::new()
        .max_connections(6)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the production application role");
    let store = PgMessageStore::new(app, region(), table.clone());
    (admin, table, store, suffix)
}

async fn drop_store(admin: &sqlx::PgPool, table: &str) {
    sqlx::query(&format!("DROP TABLE IF EXISTS {table}_thread_participant"))
        .execute(admin)
        .await
        .expect("drop the isolated thread participant table");
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

async fn delete_message_visibility(admin: &sqlx::PgPool, message_id: &str) {
    let object = format!("message:{message_id}");
    delete_outbox_aggregate(admin, &format!("identity:tuple:acmeP399:{object}")).await;
    sqlx::query(
        "DELETE FROM rebac_tuple \
         WHERE tenant_id = 'acmeP399' AND region = $1 AND object_id = $2",
    )
    .bind(region())
    .bind(&object)
    .execute(admin)
    .await
    .expect("delete this test's message visibility relationship");
    sqlx::query(
        "DELETE FROM rebac_object_revision \
         WHERE tenant_id = 'acmeP399' AND region = $1 AND object_id = $2",
    )
    .bind(region())
    .bind(object)
    .execute(admin)
    .await
    .expect("delete this test's message visibility revision");
}

#[tokio::test]
async fn chat_p5_co_commit_idempotent_send_and_per_conversation_order() {
    let (admin, table, store, suffix) = fresh_store().await;

    let conv = ConversationId::new("acmeP399", region(), format!("01J0CONVP399{suffix}"));
    let channel_aggregate = myelin_chat::events::channel_aggregate(&conv.conversation_id).0;
    let src = SystemUlidSource::new();
    let event_ids = UlidMinter::new();

    delete_outbox_aggregate(&admin, &channel_aggregate).await;

    let first_message = new_msg(&conv, "n0", "alice", "hello world");
    let (left, right) = tokio::join!(
        store.append_co_commit(
            &src,
            first_message.clone(),
            event_ids.mint().into(),
            attribution(),
            now(),
            now(),
        ),
        store.append_co_commit(
            &src,
            first_message,
            event_ids.mint().into(),
            attribution(),
            now(),
            now(),
        ),
    );
    let id0 = left.expect("first concurrent co-commit append");
    assert_eq!(
        right.expect("second concurrent co-commit append"),
        id0,
        "concurrent sends with one client nonce agree on the authoritative message id",
    );

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

    let visibility = sqlx::query(
        "SELECT relation, subject FROM rebac_tuple \
         WHERE tenant_id = $1 AND region = $2 AND object_id = $3",
    )
    .bind(&conv.tenant)
    .bind(&conv.region)
    .bind(format!("message:{}", id0.as_str()))
    .fetch_one(&admin)
    .await
    .expect("the message visibility relationship co-committed");
    assert_eq!(
        (
            visibility.get::<String, _>("relation"),
            visibility.get::<String, _>("subject"),
        ),
        (
            "parent_channel".into(),
            format!("channel:{}#read", conv.conversation_id),
        ),
        "message.view follows the exact channel.read relationship used by authorization",
    );
    let identity_aggregate = format!("identity:tuple:{}:message:{}", conv.tenant, id0.as_str());
    let identity_envelope: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM outbox WHERE aggregate = $1")
            .bind(&identity_aggregate)
            .fetch_one(&admin)
            .await
            .expect("the relationship projection event co-committed");
    assert_eq!(identity_envelope["type_"], "identity.tuple.written");
    assert_eq!(
        identity_envelope["payload"]["deltas"],
        serde_json::json!([{
            "op": "add",
            "object": format!("message:{}", id0.as_str()),
            "relation": "parent_channel",
            "subject": format!("channel:{}#read", conv.conversation_id),
        }]),
        "the projection event describes the same authoritative relationship as the row",
    );

    let ob_rows = sqlx::query(
        "SELECT event_id, aggregate, seq, subject, envelope FROM outbox WHERE aggregate = $1 ORDER BY seq",
    )
    .bind(&channel_aggregate)
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
        channel_aggregate,
        "messages share the canonical channel ordering partition (contract 2.3)"
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
            attribution(),
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
        .bind(&channel_aggregate)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        ob_count, 1,
        "CHAT-D14: exactly one event (the retry emitted none)"
    );
    let identity_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
            .bind(&identity_aggregate)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        identity_count, 1,
        "an idempotent retry emits no second relationship event",
    );

    const N: usize = 24;
    let mut message_ids = vec![id0];
    for i in 1..=N {
        message_ids.push(
            store
                .append_co_commit(
                    &src,
                    new_msg(&conv, &format!("n{i}"), "alice", &format!("m{i}")),
                    event_ids.mint().into(),
                    attribution(),
                    now(),
                    now(),
                )
                .await
                .expect("burst co-commit"),
        );
    }
    let mut seqs: Vec<i64> = sqlx::query_scalar("SELECT seq FROM outbox WHERE aggregate = $1")
        .bind(&channel_aggregate)
        .fetch_all(&admin)
        .await
        .unwrap();
    seqs.sort_unstable();
    let expected: Vec<i64> = (0..=N as i64).collect();
    assert_eq!(
        seqs, expected,
        "CHAT-D2: per-conversation seqs are contiguous + gap-free + no-dup (0 ordering violations)"
    );

    delete_outbox_aggregate(&admin, &channel_aggregate).await;
    for message_id in message_ids {
        delete_message_visibility(&admin, message_id.as_str()).await;
    }
    drop_store(&admin, &table).await;
}

#[tokio::test]
async fn a_structured_reference_is_one_atomic_durable_action() {
    let (admin, table, store, suffix) = fresh_store().await;
    let src = SystemUlidSource::new();
    let event_ids = UlidMinter::new();

    let referenced_conv =
        ConversationId::new("acmeP399", region(), format!("01J0REFSP399{suffix}"));
    let channel_aggregate =
        myelin_chat::events::channel_aggregate(&referenced_conv.conversation_id).0;
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
            attribution(),
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
    .bind(&channel_aggregate)
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
            attribution(),
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
    let rolled_back_visibility: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rebac_tuple \
         WHERE tenant_id = $1 AND region = $2 AND relation = 'parent_channel' AND subject = $3",
    )
    .bind(&rollback_conv.tenant)
    .bind(&rollback_conv.region)
    .bind(format!("channel:{}#read", rollback_conv.conversation_id))
    .fetch_one(&admin)
    .await
    .unwrap();
    let rolled_back_identity_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox \
         WHERE envelope -> 'payload' -> 'deltas' @> $1::jsonb",
    )
    .bind(serde_json::json!([{
        "subject": format!("channel:{}#read", rollback_conv.conversation_id),
    }]))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        (rolled_back_visibility, rolled_back_identity_events),
        (0, 0),
        "the failed append also leaves no authorization relationship or projection event",
    );

    delete_outbox_aggregate(&admin, &channel_aggregate).await;
    delete_outbox_aggregate(&admin, &edge_aggregate.0).await;
    delete_message_visibility(&admin, source_message_id.as_str()).await;
    drop_store(&admin, &table).await;
}

#[tokio::test]
async fn a_reply_co_commits_one_addressed_notification_and_refuses_false_roots() {
    let (admin, table, store, suffix) = fresh_store().await;
    let source = SystemUlidSource::new();
    let event_ids = UlidMinter::new();
    let conversation = ConversationId::new("acmeP399", region(), format!("01J0THREADP399{suffix}"));
    let channel_aggregate = myelin_chat::events::channel_aggregate(&conversation.conversation_id).0;
    delete_outbox_aggregate(&admin, &channel_aggregate).await;

    let root = store
        .append_structured_co_commit(
            &source,
            new_msg(&conversation, "root", "root-author", "encrypted-root"),
            event_ids.mint().into(),
            &event_ids,
            &[],
            attribution_for("alice"),
            now(),
            now(),
        )
        .await
        .expect("append a root with its notification recipient");
    let stored_recipient: String = sqlx::query_scalar(&format!(
        "SELECT principal_id FROM {table}_thread_participant \
         WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
           AND thread_root_id = $4 AND role = 0"
    ))
    .bind(&conversation.tenant)
    .bind(&conversation.region)
    .bind(&conversation.conversation_id)
    .bind(root.as_str())
    .fetch_one(&admin)
    .await
    .expect("the root author is durable beside the root");
    assert_eq!(stored_recipient, "alice");

    let mut reply = new_msg(&conversation, "reply", "reply-author", "encrypted-reply");
    reply.thread_root_id = Some(root.clone());
    let reply_id = store
        .append_structured_co_commit(
            &source,
            reply.clone(),
            event_ids.mint().into(),
            &event_ids,
            &[],
            attribution_for("bob"),
            now(),
            now(),
        )
        .await
        .expect("append the reply and its notification");
    let replayed = store
        .append_structured_co_commit(
            &source,
            reply,
            event_ids.mint().into(),
            &event_ids,
            &[],
            attribution_for("bob"),
            now(),
            now(),
        )
        .await
        .expect("retry the exact reply");
    assert_eq!(replayed, reply_id);

    let channel_events: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT envelope FROM outbox WHERE aggregate = $1 ORDER BY seq")
            .bind(&channel_aggregate)
            .fetch_all(&admin)
            .await
            .unwrap();
    assert_eq!(
        channel_events
            .iter()
            .filter(|event| event["type_"] == myelin_chat::events::CHAT_THREAD_REPLIED)
            .count(),
        1,
        "the retry emits no second thread-domain event",
    );
    let thread_ref = format!(
        "myelin://acmeP399/chat/thread/{}#thread-{}",
        root.as_str(),
        root.as_str(),
    );
    let signal_rows = sqlx::query(
        "SELECT aggregate, envelope FROM outbox \
         WHERE envelope ->> 'type_' = 'signal.opened' \
           AND envelope -> 'payload' ->> 'subject' = $1",
    )
    .bind(&thread_ref)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(signal_rows.len(), 1, "one retry-safe addressed signal");
    let signal = signal_rows[0].get::<serde_json::Value, _>("envelope");
    assert_eq!(signal["payload"]["notification_reason"], "replied");
    assert!(signal["payload"]["mentions"].to_string().contains("alice"));
    assert!(!signal["payload"].to_string().contains("encrypted-reply"));
    let follower: String = sqlx::query_scalar(&format!(
        "SELECT principal_id FROM {table}_thread_participant \
         WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
           AND thread_root_id = $4 AND role = 1"
    ))
    .bind(&conversation.tenant)
    .bind(&conversation.region)
    .bind(&conversation.conversation_id)
    .bind(root.as_str())
    .fetch_one(&admin)
    .await
    .expect("replying starts following the thread durably");
    assert_eq!(follower, "bob");

    let mut follow_up = new_msg(
        &conversation,
        "root-author-follows-up",
        "root-author",
        "encrypted-follow-up",
    );
    follow_up.thread_root_id = Some(root.clone());
    let follow_up_id = store
        .append_structured_co_commit(
            &source,
            follow_up,
            event_ids.mint().into(),
            &event_ids,
            &[],
            attribution_for("alice"),
            now(),
            now(),
        )
        .await
        .expect("the root author can answer the participant following the thread");
    let watched_signals = sqlx::query(
        "SELECT aggregate, envelope FROM outbox \
         WHERE envelope ->> 'type_' = 'signal.opened' \
           AND envelope -> 'payload' ->> 'subject' = $1 \
           AND envelope -> 'payload' ->> 'notification_reason' = 'thread_watched'",
    )
    .bind(&thread_ref)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(watched_signals.len(), 1, "one watched-thread signal");
    let watched = watched_signals[0].get::<serde_json::Value, _>("envelope");
    assert!(watched["payload"]["mentions"].to_string().contains("bob"));
    assert!(!watched["payload"]["mentions"].to_string().contains("alice"));
    assert!(!watched["payload"]
        .to_string()
        .contains("encrypted-follow-up"));
    let replied_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox \
         WHERE aggregate = $1 AND envelope ->> 'type_' = 'chat.thread.replied'",
    )
    .bind(&channel_aggregate)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(replied_events, 2, "each real reply has one domain event");

    let mut nested = new_msg(&conversation, "nested", "bob", "nested");
    nested.thread_root_id = Some(reply_id.clone());
    assert!(matches!(
        store
            .append_structured_co_commit(
                &source,
                nested,
                event_ids.mint().into(),
                &event_ids,
                &[],
                attribution_for("bob"),
                now(),
                now(),
            )
            .await,
        Err(StoreError::NotFound(id)) if id == reply_id
    ));

    let mut foreign = new_msg(
        &ConversationId::new("acmeP399", region(), format!("01J0OTHERP399{suffix}")),
        "foreign",
        "bob",
        "foreign",
    );
    foreign.thread_root_id = Some(root.clone());
    assert!(matches!(
        store
            .append_structured_co_commit(
                &source,
                foreign,
                event_ids.mint().into(),
                &event_ids,
                &[],
                attribution_for("bob"),
                now(),
                now(),
            )
            .await,
        Err(StoreError::NotFound(id)) if id == root
    ));

    sqlx::query(&format!(
        "UPDATE {table} SET state = 3 \
         WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 AND message_id = $4"
    ))
    .bind(&conversation.tenant)
    .bind(&conversation.region)
    .bind(&conversation.conversation_id)
    .bind(root.as_str())
    .execute(&admin)
    .await
    .unwrap();
    let mut after_removal = new_msg(&conversation, "after-removal", "bob", "too late");
    after_removal.thread_root_id = Some(root.clone());
    assert!(matches!(
        store
            .append_structured_co_commit(
                &source,
                after_removal,
                event_ids.mint().into(),
                &event_ids,
                &[],
                attribution_for("bob"),
                now(),
                now(),
            )
            .await,
        Err(StoreError::CasConflict { message_id, .. }) if message_id == root
    ));

    let signal_aggregate = signal_rows[0].get::<String, _>("aggregate");
    let watched_signal_aggregate = watched_signals[0].get::<String, _>("aggregate");
    delete_outbox_aggregate(&admin, &channel_aggregate).await;
    delete_outbox_aggregate(&admin, &signal_aggregate).await;
    delete_outbox_aggregate(&admin, &watched_signal_aggregate).await;
    delete_message_visibility(&admin, root.as_str()).await;
    delete_message_visibility(&admin, reply_id.as_str()).await;
    delete_message_visibility(&admin, follow_up_id.as_str()).await;
    drop_store(&admin, &table).await;
}
