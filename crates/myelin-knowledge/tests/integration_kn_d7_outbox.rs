#![cfg(feature = "integration")]

use myelin_events::{
    Actor, CausedBy, EmitContextBase, EventId, IdMinter, MonotonicMinter, OutboxStore, Region,
    TenantId, Timestamp, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{emit_change, KnowledgeChange};
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
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-7".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn block_changes() -> Vec<KnowledgeChange> {
    vec![
        KnowledgeChange::BlockCreated {
            page_id: "7c2".into(),
            block_id: "1".into(),
        },
        KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "2".into(),
        },
        KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "3".into(),
        },
    ]
}

fn staged_block_rows() -> Vec<(String, String, String, serde_json::Value)> {
    let store = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>;
    let mut tx = store.begin(minter, ctx_base());
    let mut ids = Vec::new();
    for ch in &block_changes() {
        ids.push(emit_change(&mut tx, &tenant(), ch, None).expect("emit"));
    }
    tx.commit()
        .expect("commit the in-memory derive (the real co-commit is the PG tx below)");
    ids.into_iter()
        .map(|id: EventId| {
            let row = store.row(&id).expect("staged knowledge row");
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
async fn emit_iff_committed_n_blocks_n_rows_and_rollback_zero_on_real_postgres() {
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
    let outbox = format!("outbox_p296_{suffix}");
    let block_tbl = format!("block_p296_{suffix}");

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
        "CREATE TABLE IF NOT EXISTS {block_tbl} (block_id TEXT PRIMARY KEY, page_id TEXT, version BIGINT)"
    ))
    .execute(&admin)
    .await
    .expect("create block table");
    sqlx::query(&format!("GRANT ALL ON {outbox} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant outbox");
    sqlx::query(&format!("GRANT ALL ON {block_tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant block");

    let rows = staged_block_rows();
    assert_eq!(
        rows.len(),
        3,
        "the 3 block changes derive 3 knowledge.* events"
    );

    let insert_outbox = format!(
        "INSERT INTO {outbox} (event_id, aggregate, seq, subject, envelope) VALUES ($1,$2,$3,$4,$5)"
    );

    let mut tx = app
        .begin()
        .await
        .expect("begin the block state transaction");
    for (i, ch) in block_changes().iter().enumerate() {
        let block_id = match ch {
            KnowledgeChange::BlockCreated { block_id, .. }
            | KnowledgeChange::BlockUpdated { block_id, .. } => block_id.clone(),
            _ => unreachable!(),
        };
        sqlx::query(&format!(
            "INSERT INTO {block_tbl} (block_id, page_id, version) VALUES ($1, '7c2', $2)"
        ))
        .bind(&block_id)
        .bind(i as i64)
        .execute(&mut *tx)
        .await
        .expect("write the block state row");
    }
    for (i, (event_id, aggregate, subject, envelope)) in rows.iter().enumerate() {
        sqlx::query(&insert_outbox)
            .bind(event_id)
            .bind(aggregate)
            .bind(i as i64)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx)
            .await
            .expect("emit the knowledge event into the SAME tx");
    }
    tx.commit()
        .await
        .expect("commit the blocks + events together");

    let n: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} \
         WHERE published_at IS NULL AND envelope->>'type_' LIKE 'knowledge.block.%'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n, 3,
        "3 block changes committed → 3 relay-visible knowledge.block.* rows (0 lost)"
    );
    let c: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {block_tbl}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("n");
    assert_eq!(c, 3, "the block state rows co-committed");

    let agg: String = sqlx::query(&format!("SELECT DISTINCT aggregate FROM {outbox}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("aggregate");
    assert_eq!(
        agg, "myelin://tenantA/knowledge/page/7c2",
        "the aggregate is the PAGE (the doc, §4)"
    );

    let mut tx2 = app.begin().await.expect("begin a second state transaction");
    sqlx::query(&format!(
        "INSERT INTO {block_tbl} (block_id, page_id, version) VALUES ('9', '7c2', 99)"
    ))
    .execute(&mut *tx2)
    .await
    .expect("write a second block row");
    let rows2 = staged_block_rows();
    for (i, (event_id, aggregate, subject, envelope)) in rows2.iter().enumerate() {
        sqlx::query(&insert_outbox)
            .bind(format!("{event_id}-tx2"))
            .bind(aggregate)
            .bind(100 + i as i64)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx2)
            .await
            .expect("emit the second event set into the SAME tx");
    }
    tx2.rollback()
        .await
        .expect("ABORT the state transaction (the crash before commit)");

    let n_after: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} WHERE envelope->>'type_' LIKE 'knowledge.block.%'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n_after, 3,
        "aborted state tx wrote 0 events (emit-iff-committed, KN-D7): still only the 3 committed"
    );
    let c_after: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {block_tbl}"))
        .fetch_one(&app)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        c_after, 3,
        "the aborted block row rolled back too (no block without its event)"
    );

    sqlx::query(&format!("DROP TABLE {outbox}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {block_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
