#![cfg(feature = "integration")]

use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::emit_edges;

use std::sync::Arc;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn tenant() -> TenantId {
    TenantId("tenantA".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-7".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn source_doc() -> ArtifactRef {
    ArtifactRef("myelin://tenantA/chat/message/m1".into())
}

fn content_event() -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-content".into()),
        type_: EventType("chat.message.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: source_doc(),
        aggregate: AggregateKey("chat:message:m1".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-root-corr".into()),
        caused_by: Some(CausedBy("session:abc".into())),
        depth: 3,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "body_ref": "r1" }),
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn three_node_doc() -> Vec<InlineNode> {
    let target = ArtifactRef("myelin://tenantA/knowledge/page/7c2".into());
    vec![
        InlineNode::Mention(principal()),
        InlineNode::ArtifactRefNode(target.clone()),
        InlineNode::Embed(target),
    ]
}

fn staged_edge_rows() -> Vec<(String, String, String, serde_json::Value)> {
    let store = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>;
    let content = content_event();
    let mut tx = store.begin(minter, ctx_base());
    let ids = emit_edges(&mut tx, &source_doc(), &three_node_doc(), &content).expect("emit ok");
    tx.commit()
        .expect("commit ok (the in-memory derive - the real co-commit is the PG tx below)");
    ids.into_iter()
        .map(|id: EventId| {
            let row = store.row(&id).expect("staged edge row");
            (
                row.event_id.0.clone(),
                row.aggregate.0.clone(),
                row.subject.0.clone(),
                serde_json::to_value(&row.envelope).expect("envelope → jsonb"),
            )
        })
        .collect()
}

#[tokio::test]
async fn emit_iff_committed_n_nodes_n_rows_and_rollback_zero_on_real_postgres() {
    use sqlx::Row;

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
    let outbox = format!("outbox_p157_{suffix}");
    let content_tbl = format!("content_p157_{suffix}");

    let outbox_ddl = OUTBOX_MIGRATION
        .replace("EXISTS outbox (", &format!("EXISTS {outbox} ("))
        .replace("ON outbox (", &format!("ON {outbox} ("))
        .replace(
            "outbox_event_id_unique",
            &format!("{outbox}_event_id_unique"),
        )
        .replace(
            "outbox_aggregate_seq_unique",
            &format!("{outbox}_aggregate_seq_unique"),
        )
        .replace("outbox_unsent_idx", &format!("{outbox}_unsent_idx"));
    for stmt in outbox_ddl
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        sqlx::query(stmt)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("outbox ddl `{stmt}`: {e}"));
    }
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {content_tbl} (id TEXT PRIMARY KEY, body_ref TEXT)"
    ))
    .execute(&admin)
    .await
    .expect("create content table");
    sqlx::query(&format!("GRANT ALL ON {outbox} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant outbox");
    sqlx::query(&format!("GRANT ALL ON {content_tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant content");

    let rows = staged_edge_rows();
    assert_eq!(rows.len(), 3, "the 3-node document derives 3 edge events");

    let insert_outbox = format!(
        "INSERT INTO {outbox} (event_id, aggregate, seq, subject, envelope) VALUES ($1,$2,$3,$4,$5)"
    );

    let mut tx = app.begin().await.expect("begin the content transaction");
    sqlx::query(&format!(
        "INSERT INTO {content_tbl} (id, body_ref) VALUES ('m1','r1')"
    ))
    .execute(&mut *tx)
    .await
    .expect("write the content row");
    for (i, (event_id, aggregate, subject, envelope)) in rows.iter().enumerate() {
        sqlx::query(&insert_outbox)
            .bind(event_id)
            .bind(aggregate)
            .bind(i as i64)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx)
            .await
            .expect("emit the edge row into the SAME tx");
    }
    tx.commit()
        .await
        .expect("commit the content + edges together");

    let n: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} \
         WHERE published_at IS NULL AND envelope->>'type_' = 'refs.edge.created'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n, 3,
        "3 structured nodes committed → 3 relay-visible refs.edge.created rows (0 lost)"
    );
    let c: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {content_tbl}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("n");
    assert_eq!(c, 1, "the content row co-committed");

    let mut tx2 = app
        .begin()
        .await
        .expect("begin a second content transaction");
    sqlx::query(&format!(
        "INSERT INTO {content_tbl} (id, body_ref) VALUES ('m2','r2')"
    ))
    .execute(&mut *tx2)
    .await
    .expect("write the second content row");
    let rows2 = staged_edge_rows();
    for (i, (event_id, aggregate, subject, envelope)) in rows2.iter().enumerate() {
        sqlx::query(&insert_outbox)
            .bind(format!("{event_id}-tx2"))
            .bind(aggregate)
            .bind(100 + i as i64)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx2)
            .await
            .expect("emit the second edge set into the SAME tx");
    }
    tx2.rollback()
        .await
        .expect("ABORT the content transaction (no commit)");

    let n_after: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} WHERE envelope->>'type_' = 'refs.edge.created'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n_after, 3,
        "aborted content tx wrote 0 edges (emit-iff-committed): still only the 3 committed"
    );
    let c_after: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {content_tbl}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        c_after, 1,
        "the aborted content row rolled back too (no content without its edges)"
    );

    sqlx::query(&format!("DROP TABLE {outbox}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {content_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
