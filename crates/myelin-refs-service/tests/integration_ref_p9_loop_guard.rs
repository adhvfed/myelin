//! **REF-P9 / P-158 — the loop-guard causal-depth stamp on every `refs.edge.*`, PROVEN against the
//! live dev-stack Postgres outbox.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p9_loop_guard -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P9: the loop-guard stamp
//! rides the SAME `outbox` table co-commit contract REF-P8's emit seam already proves. The guard's
//! [`RefsLoopGuard::guarded_emit_edges`] derives the `refs.edge.created` envelopes through the REAL
//! [`myelin_events::OutboxTransaction`] (so the `depth = content.depth + 1` stamp is correct-by-
//! construction via `derive_envelope`), and those envelopes are inserted into the REAL frozen §2.3
//! `outbox` table (the SAME shape the relay drains). We then read the stamp back FROM Postgres and
//! prove:
//!
//! - **The `+1` depth stamp lands in the real outbox envelope:** a content cause at `depth = 3`
//!   commits 3 `refs.edge.created` rows whose `envelope->>'depth'` is `4` in Postgres (read back from
//!   the durable jsonb — not just the in-memory derive).
//! - **The depth-ceiling tripwire parks BEFORE runaway:** a content cause AT the ceiling (12) writes
//!   ZERO `refs.edge.created` rows (the chain halts ≤ ceiling) and fires the tripwire — observable,
//!   never a silent unbounded fork.
//!
//! The drill is registered red-until-proven and flips green ONLY here, against the live stack.
#![cfg(feature = "integration")]

use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, OUTBOX_MIGRATION,
    Region, TenantId, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{GuardDecision, RefsLoopGuard, CAUSAL_DEPTH_CEILING};

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
    Principal::stub(PrincipalId("p-opaque-7".into()), PrincipalKind::Human, tenant())
}
fn source_doc() -> ArtifactRef {
    ArtifactRef("myelin://tenantA/chat/message/m1".into())
}

fn content_event(depth: u32) -> EventEnvelope {
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
        depth,
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

/// Drive the loop guard over a content cause at `depth` and return the staged
/// `(event_id, aggregate, subject, envelope-json)` rows ready to insert into the real outbox, plus
/// the guard's decision (Emitted vs CeilingParked) so the integration test asserts the SAME
/// structural outcome that lands in Postgres.
fn guarded_rows(
    guard: &RefsLoopGuard,
    depth: u32,
) -> (GuardDecision, Vec<(String, String, String, serde_json::Value)>) {
    let store = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>;
    let content = content_event(depth);
    let mut tx = store.begin(minter, ctx_base());
    let decision = guard
        .guarded_emit_edges(&mut tx, &source_doc(), &three_node_doc(), &content)
        .expect("guard ok");
    tx.commit()
        .expect("commit ok (the in-memory derive — the real co-commit is the PG tx below)");
    let ids: Vec<EventId> = match &decision {
        GuardDecision::Emitted { ids, .. } => ids.clone(),
        GuardDecision::CeilingParked { .. } => Vec::new(),
    };
    let rows = ids
        .into_iter()
        .map(|id| {
            let row = store.row(&id).expect("staged edge row");
            (
                row.event_id.0.clone(),
                row.aggregate.0.clone(),
                row.subject.0.clone(),
                serde_json::to_value(&row.envelope).expect("envelope → jsonb"),
            )
        })
        .collect();
    (decision, rows)
}

#[tokio::test]
async fn depth_stamp_lands_in_real_outbox_and_ceiling_parks_zero_on_real_postgres() {
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
    let outbox = format!("outbox_p158_{suffix}");
    let content_tbl = format!("content_p158_{suffix}");

    // ── Apply the REAL frozen §2.3 outbox table (suffixed for isolation) + a content table. ──
    let outbox_ddl = OUTBOX_MIGRATION
        .replace("EXISTS outbox (", &format!("EXISTS {outbox} ("))
        .replace("ON outbox (", &format!("ON {outbox} ("))
        .replace("outbox_event_id_unique", &format!("{outbox}_event_id_unique"))
        .replace("outbox_aggregate_seq_unique", &format!("{outbox}_aggregate_seq_unique"))
        .replace("outbox_unsent_idx", &format!("{outbox}_unsent_idx"));
    for stmt in outbox_ddl.split(';').map(str::trim).filter(|s| !s.is_empty()) {
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

    let insert_outbox = format!(
        "INSERT INTO {outbox} (event_id, aggregate, seq, subject, envelope) VALUES ($1,$2,$3,$4,$5)"
    );

    // ── (1) THE +1 STAMP LANDS IN THE REAL OUTBOX: content depth 3 → 3 edges at depth 4. ──
    let guard = RefsLoopGuard::new();
    let (decision, rows) = guarded_rows(&guard, 3);
    assert!(
        matches!(decision, GuardDecision::Emitted { stamped_depth: 4, .. }),
        "below the ceiling → emitted, stamped at content.depth + 1 = 4"
    );
    assert_eq!(rows.len(), 3, "the 3-node document derives 3 edge events");

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
            .expect("emit the stamped edge row into the SAME tx");
    }
    tx.commit().await.expect("commit the content + stamped edges together");

    // Read the stamp BACK from the durable Postgres jsonb: every refs.edge.created carries depth 4.
    let n_at_depth_4: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} \
         WHERE envelope->>'type_' = 'refs.edge.created' AND (envelope->>'depth')::int = 4"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n_at_depth_4, 3,
        "all 3 committed refs.edge.created rows carry the +1 depth stamp (4) in the real outbox"
    );
    // No edge escaped the stamp: there is NO refs.edge.created at any other depth.
    let n_off_stamp: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} \
         WHERE envelope->>'type_' = 'refs.edge.created' AND (envelope->>'depth')::int <> 4"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(n_off_stamp, 0, "no refs.edge.created escaped the +1 stamp");

    // ── (2) THE CEILING TRIPWIRE PARKS BEFORE RUNAWAY: content AT the ceiling → 0 edge rows. ──
    let (parked_decision, parked_rows) = guarded_rows(&guard, CAUSAL_DEPTH_CEILING);
    assert!(
        matches!(
            parked_decision,
            GuardDecision::CeilingParked { would_be_depth } if would_be_depth == CAUSAL_DEPTH_CEILING + 1
        ),
        "a content cause at the ceiling parks (the would-be edge is over the bound)"
    );
    assert!(parked_rows.is_empty(), "a parked emit stages 0 edge rows");
    assert_eq!(guard.ceiling_tripwire_firings(), 1, "the tripwire fired before runaway");

    // Nothing new lands in the real outbox for the parked hop (the chain halted ≤ ceiling).
    let mut tx2 = app.begin().await.expect("begin a parked-hop transaction");
    sqlx::query(&format!(
        "INSERT INTO {content_tbl} (id, body_ref) VALUES ('m-deep','r-deep')"
    ))
    .execute(&mut *tx2)
    .await
    .expect("write the deep content row");
    for (i, (event_id, aggregate, subject, envelope)) in parked_rows.iter().enumerate() {
        sqlx::query(&insert_outbox)
            .bind(format!("{event_id}-deep"))
            .bind(aggregate)
            .bind(200 + i as i64)
            .bind(subject)
            .bind(envelope)
            .execute(&mut *tx2)
            .await
            .expect("insert (there are none — parked)");
    }
    tx2.commit().await.expect("commit the deep content (0 edges)");

    // Still exactly the 3 stamped edges from step 1 — the parked hop added none (runaway bounded).
    let n_total: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {outbox} WHERE envelope->>'type_' = 'refs.edge.created'"
    ))
    .fetch_one(&app)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        n_total, 3,
        "the ceiling park wrote 0 new edges — the reactive chain halted ≤ ceiling (no runaway)"
    );

    // Cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {outbox}")).execute(&admin).await.unwrap();
    sqlx::query(&format!("DROP TABLE {content_tbl}")).execute(&admin).await.unwrap();
}
