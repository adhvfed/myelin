//! **REF-P20 / P-336 — Refs projects the SECOND real TE-7 mirror (Issues `issue_relation`) +
//! resolves Issues' `field-`/`row-` sub-anchors, PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p20_issue_producer -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P20 — the Issues
//! `issue_relation` lifecycle mirror (the inverse-paired `rel_class=lifecycle` edges across the WHOLE
//! lifecycle vocabulary) + the §3.3 / §4.7 reindex-from-source byte-parity AND the TE-7 SECOND-mirror
//! reconvergence (the typed table WINS), executed against the REAL §3.2 `edge` table on real Postgres,
//! on a production-shaped Issues corpus. The drill is registered red-until-proven and flips green ONLY
//! here, with real artifacts.
//!
//! **REF-D9 (the leak / IDOR invariant) at the DATABASE layer:** the §3.2 RLS isolates a tenant — a
//! session pinned to tenant B reads ZERO of tenant A's Issues lifecycle edges (the cross-tenant leak
//! invariant holds at the DATABASE layer, not just in the in-memory model).
//!
//! **REF-D4 (reindex-from-cold byte-parity) over real Postgres on the Issues corpus:** the corpus
//! carries the `issue_relation` mirror's inverse-paired lifecycle pairs across the vocabulary. We
//! (1) build the LIVE table; (2) capture its byte-image; (3) WIPE the partition (no Issues-DB reload);
//! (4) rebuild ONLY by re-driving the SAME mirror upserts (the reindex re-emit path) → byte-parity.
//!
//! **REF-D4 TE-7 half — the SECOND real reconvergence (the typed table WINS):** an out-of-band
//! `issue_relation` edit drifts the projection; the authoritative snapshot re-relates the issue; a
//! scoped reindex reconverges — the stale relation edge is TOMBSTONED, the typed truth becomes live,
//! proven against the real `edge` table (supports ISS-D6).
#![cfg(feature = "integration")]

use myelin_issues::events::RELATION_CREATED;
use myelin_refs_service::{
    edge_id, mirror_issue_relation, IssueEdgeProducer, IssueRelationEvent, CREATE_EDGE_INDEXES_DDL,
    CREATE_EDGE_TABLE_DDL, MAKE_EDGE_TENANT_SCOPED_DDL,
};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn rename(ddl: &str, tbl: &str) -> String {
    ddl.replace("EXISTS edge (", &format!("EXISTS {tbl} ("))
        .replace("ON edge (", &format!("ON {tbl} ("))
        .replace("ON edge ", &format!("ON {tbl} "))
        .replace("('edge')", &format!("('{tbl}')"))
        .replace("edge_inbound", &format!("{tbl}_inbound"))
        .replace("edge_outbound", &format!("{tbl}_outbound"))
        .replace("edge_by_rel", &format!("{tbl}_by_rel"))
}

fn t() -> TenantId {
    TenantId("tenantA".into())
}

/// One edge row tuple for the upsert.
struct Row {
    edge_id: String,
    source: String,
    source_root: String,
    target: String,
    target_root: String,
    rel: String,
    rel_class: String,
    actor: String,
}

/// Build the inverse-paired `issue_relation` mirror rows for `(source-key, target-key, rel)`.
fn relation_rows(src: &str, tgt: &str, rel: &str) -> Vec<Row> {
    let ev = IssueRelationEvent {
        source: IssueEdgeProducer::issue_root("tenantA", src),
        target: IssueEdgeProducer::issue_root("tenantA", tgt),
        rel: rel.into(),
        origin_event_id: format!("evt-{src}-{tgt}-{rel}"),
        origin_event_type: RELATION_CREATED.into(),
        origin_actor: "issue-pseudonym".into(),
        zookie: Some("zk-1".into()),
    };
    mirror_issue_relation(&t(), &ev)
        .expect("recognised trigger + known rel")
        .into_iter()
        .map(|e| Row {
            edge_id: e.edge_id,
            source: e.source.0.clone(),
            source_root: e.source_root.0.clone(),
            target: e.target.0.clone(),
            target_root: e.target_root.0.clone(),
            rel: e.rel,
            rel_class: "lifecycle".into(),
            actor: e.origin_actor,
        })
        .collect()
}

#[tokio::test]
async fn issue_relation_mirror_ingest_reindex_and_reconverge_on_real_postgres() {
    use sqlx::Row as _;

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
    let tbl = format!("edge_p336_{suffix}");

    // ── Apply the REAL §3.2 schema (create + three indexes + RLS), suffixed for isolation. ──
    sqlx::query(&rename(CREATE_EDGE_TABLE_DDL, &tbl))
        .execute(&admin)
        .await
        .expect("create edge table");
    for (name, idx) in CREATE_EDGE_INDEXES_DDL {
        sqlx::query(&rename(idx, &tbl))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name}: {e}"));
    }
    sqlx::query(&rename(MAKE_EDGE_TENANT_SCOPED_DDL, &tbl))
        .execute(&admin)
        .await
        .expect("RLS scope");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant app");

    // ── A production-shaped Issues corpus: the issue_relation mirror across the WHOLE vocabulary. ──
    // blocks (paired), parent (paired), relates (symmetric), closes (None-inverse, forward only).
    let mut corpus: Vec<Row> = Vec::new();
    corpus.extend(relation_rows("ENG-1", "ENG-2", "blocks"));
    corpus.extend(relation_rows("PLAT-9", "ENG-1", "parent"));
    corpus.extend(relation_rows("ENG-3", "ENG-4", "relates"));
    corpus.extend(relation_rows("ENG-5", "ENG-6", "closes"));

    // Pin the session to tenantA (RLS).
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id','tenantA',false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region','fr-par',false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let upsert_sql = format!(
        "INSERT INTO {tbl} \
           (tenant_id, region, edge_id, source, source_root, target, target_root, rel, rel_class, \
            origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref) \
         VALUES ('tenantA','fr-par',$1,$2,$3,$4,$5,$6,$7,$8,$9,now(),'zk-1',false,'kms://tenantA/0/tenant') \
         ON CONFLICT (tenant_id, edge_id) DO NOTHING"
    );
    let parity_sql = format!(
        "SELECT string_agg( \
            edge_id||'|'||source||'|'||source_root||'|'||target||'|'||target_root||'|'||rel||'|'|| \
            rel_class||'|'||origin_actor||'|'||coalesce(zookie,'')||'|'||tombstoned, E'\\n' \
            ORDER BY edge_id) AS img FROM {tbl}"
    );

    async fn rebuild(conn: &mut sqlx::PgConnection, upsert_sql: &str, rows: &[Row]) {
        for row in rows {
            sqlx::query(upsert_sql)
                .bind(&row.edge_id)
                .bind(&row.source)
                .bind(&row.source_root)
                .bind(&row.target)
                .bind(&row.target_root)
                .bind(&row.rel)
                .bind(&row.rel_class)
                .bind(format!("evt-{}", row.edge_id))
                .bind(&row.actor)
                .execute(&mut *conn)
                .await
                .expect("upsert issue_relation edge");
        }
    }

    // ── (1) Build the LIVE Issues edge table; capture its byte-image. ──
    rebuild(&mut conn, &upsert_sql, &corpus).await;
    let live_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");
    // blocks→2, parent→2, relates→2, closes→1 = 7 edges, all lifecycle-class.
    let live_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        live_count, 7,
        "blocks(2)+parent(2)+relates(2)+closes(1) inverse-paired lifecycle edges"
    );
    let lifecycle_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel_class='lifecycle'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        lifecycle_count, 7,
        "every issue_relation mirror edge is lifecycle-class (5.5)"
    );

    // The forward `blocks` edge: ENG-1 → ENG-2; the frozen inverse `blocked_by`: ENG-2 → ENG-1.
    let blocks_target: String =
        sqlx::query(&format!("SELECT target FROM {tbl} WHERE rel='blocks'"))
            .fetch_one(&mut *conn)
            .await
            .unwrap()
            .get("target");
    assert_eq!(blocks_target, "myelin://tenantA/issue/issue/ENG-2");
    let blocked_by_target: String =
        sqlx::query(&format!("SELECT target FROM {tbl} WHERE rel='blocked_by'"))
            .fetch_one(&mut *conn)
            .await
            .unwrap()
            .get("target");
    assert_eq!(blocked_by_target, "myelin://tenantA/issue/issue/ENG-1");
    // The symmetric `relates` mirror stored BOTH directions (same rel, endpoints swapped).
    let relates_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel='relates'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        relates_count, 2,
        "relates is symmetric — both directions stored"
    );
    // The directional `closes` (None-inverse) stored ONLY the forward edge (the inverse is the floor).
    let closes_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel='closes'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        closes_count, 1,
        "closes has no frozen inverse — forward only"
    );

    // ── REF-D9 (IDOR / cross-tenant) at the DATABASE layer: a tenantB session reads 0 of these edges. ──
    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id','tenantB',false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region','fr-par',false)")
        .execute(&mut *conn_b)
        .await
        .unwrap();
    let b_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn_b)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        b_count, 0,
        "RLS isolates tenants — tenantB reads 0 of tenantA's Issues lifecycle edges (REF-D9)"
    );

    // ── (2) WIPE the partition (the cold-rebuild precondition — NO Issues-DB reload). ──
    sqlx::query(&format!("DELETE FROM {tbl}"))
        .execute(&mut *conn)
        .await
        .expect("wipe partition");
    let after_wipe: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(after_wipe, 0, "the Issues edge partition is wiped");

    // ── (3) Rebuild ONLY from the SAME mirror upserts (the reindex re-emit path). ──
    rebuild(&mut conn, &upsert_sql, &corpus).await;
    let rebuilt_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");

    // ── (4) The rebuilt Issues edge table byte-matches the live table (§4.7 reindex-parity). ──
    assert_eq!(
        rebuilt_img, live_img,
        "the rebuilt Issues edge index byte-matches the live index (cold == live)"
    );

    // ── REF-D4 TE-7 half — the SECOND real reconvergence (the typed table WINS; supports ISS-D6). ──
    // Drift: ENG-1 is ALSO (mistakenly, out of band) recorded as `blocks` ENG-STALE in the projection.
    let drift = relation_rows("ENG-1", "ENG-STALE", "blocks");
    rebuild(&mut conn, &upsert_sql, &drift).await;
    // The authoritative typed snapshot says ENG-1 `blocks` ENG-2 (the live truth). Reconverge:
    // tombstone any forward `blocks` edge sourced from ENG-1 whose target the typed snapshot does NOT
    // back. (The SAME mirror::reconverge typed-wins set arithmetic, here over real Postgres.)
    let eng1_root = "myelin://tenantA/issue/issue/ENG-1";
    let truth_blocks_edge_id = edge_id(
        &t(),
        eng1_root,
        "myelin://tenantA/issue/issue/ENG-2",
        "blocks",
    );
    let tombstoned = sqlx::query(&format!(
        "UPDATE {tbl} SET tombstoned=true \
         WHERE rel_class='lifecycle' AND source=$1 AND rel='blocks' AND edge_id<>$2 AND tombstoned=false"
    ))
    .bind(eng1_root)
    .bind(&truth_blocks_edge_id)
    .execute(&mut *conn)
    .await
    .expect("reconverge: tombstone drift")
    .rows_affected();
    assert_eq!(
        tombstoned, 1,
        "the drifted ENG-1→ENG-STALE blocks edge is tombstoned (typed table wins)"
    );

    // After reconvergence the LIVE forward `blocks` target of ENG-1 is EXACTLY ENG-2 (typed table won).
    let live_blocks: Vec<(String,)> = sqlx::query_as(&format!(
        "SELECT target FROM {tbl} WHERE rel='blocks' AND source=$1 AND tombstoned=false"
    ))
    .bind(eng1_root)
    .fetch_all(&mut *conn)
    .await
    .expect("live blocks of ENG-1");
    assert_eq!(
        live_blocks.len(),
        1,
        "exactly one live blocks edge after reconvergence"
    );
    assert_eq!(
        live_blocks[0].0, "myelin://tenantA/issue/issue/ENG-2",
        "the typed table wins — ENG-1 blocks ENG-2, the drifted ENG-STALE is tombstoned"
    );

    // Cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
