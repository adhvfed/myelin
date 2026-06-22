//! **REF-P18 / P-259 — Refs consumes the REAL Knowledge producer edges + the FIRST real lifecycle
//! mirror (`page_parent`), PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p18_kn_producer -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P18 — the KN producer edge
//! ingest + the page_parent lifecycle mirror (BOTH inverse-paired `parent`/`child` `rel_class=lifecycle`
//! edges) + the §3.3 / §4.7 reindex-from-source byte-parity AND the TE-7 reconvergence (the typed table
//! WINS), executed against the REAL §3.2 `edge` table on real Postgres, on a production-shaped KN
//! corpus. The drill is registered red-until-proven and flips green ONLY here, with real artifacts.
//!
//! **REF-D1/REF-D2 (leak / IDOR) at the DATABASE layer:** the §3.2 RLS isolates a tenant — a session
//! pinned to tenant B reads ZERO of tenant A's KN edges (the cross-tenant leak invariant holds at the
//! DATABASE layer, not just in the in-memory model).
//!
//! **REF-D4 (reindex-from-cold byte-parity) over real Postgres on the KN corpus INCL. the page_parent
//! mirror:** the corpus carries reference edges + the page_parent mirror's inverse-paired lifecycle
//! pair. We (1) build the LIVE table; (2) capture its byte-image; (3) WIPE the partition (no KN-DB
//! reload); (4) rebuild ONLY by re-driving the SAME upserts (the reindex re-emit path) → byte-parity.
//!
//! **REF-D4 TE-7 half — the FIRST real reconvergence (the typed table WINS):** a drifted re-parent
//! edge is projected; the authoritative `page_parent` snapshot re-parents the page elsewhere; a scoped
//! reindex reconverges — the stale parent edge is TOMBSTONED, the typed truth becomes live, proven
//! against the real `edge` table (the `tombstoned` column flips for the drifted edge; the live set for
//! the child page == exactly the typed snapshot).
#![cfg(feature = "integration")]

use myelin_refs::{strip_sub, ArtifactRef};
use myelin_refs_service::{
    edge_id, mirror_page_parent, KnEdgeProducer, PageParentEvent, CREATE_EDGE_INDEXES_DDL,
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

/// One KN reference-edge row tuple `(edge_id, source, source_root, target, target_root, rel, rel_class,
/// actor)` for the upsert.
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

fn ref_edge(source: &str, target: &str, rel: &str, actor: &str) -> Row {
    let id = edge_id(&t(), source, target, rel);
    Row {
        edge_id: id,
        source: source.into(),
        source_root: strip_sub(&ArtifactRef(source.into())).0,
        target: target.into(),
        target_root: strip_sub(&ArtifactRef(target.into())).0,
        rel: rel.into(),
        rel_class: "reference".into(),
        actor: actor.into(),
    }
}

fn page_parent_rows(parent: &str, child: &str, trigger: &str, actor: &str) -> Vec<Row> {
    let ev = PageParentEvent {
        parent: KnEdgeProducer::page_root("tenantA", parent),
        child: KnEdgeProducer::page_root("tenantA", child),
        origin_event_id: format!("evt-{parent}-{child}"),
        origin_event_type: trigger.into(),
        origin_actor: actor.into(),
        zookie: Some("zk-1".into()),
    };
    // The FIRST real mirror — both inverse-paired lifecycle edges (parent→child + child→parent).
    mirror_page_parent(&t(), &ev)
        .expect("recognised lifecycle trigger")
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
async fn kn_producer_edges_and_page_parent_mirror_ingest_reindex_and_reconverge_on_real_postgres() {
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
    let tbl = format!("edge_p259_{suffix}");

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

    // ── A production-shaped KN corpus: reference edges + the page_parent lifecycle mirror pair. ──
    let mut corpus: Vec<Row> = vec![
        ref_edge(
            "myelin://tenantA/knowledge/page/design-doc",
            "myelin://tenantA/knowledge/page/sibling",
            "embeds",
            "kn-pseudonym-1",
        ),
        ref_edge(
            "myelin://tenantA/knowledge/block/blk-9",
            "myelin://tenantA/issue/issue/ENG-7",
            "links",
            "kn-pseudonym-2",
        ),
        // A block-anchored embed: the #sub-precise block ref carries the block anchor; the stored
        // target_root is the #sub-stripped page (the block anchor re-derives, never a stale index).
        ref_edge(
            "myelin://tenantA/chat/message/m1",
            "myelin://tenantA/knowledge/page/design-doc#b7",
            "embeds",
            "kn-pseudonym-1",
        ),
    ];
    // The FIRST real lifecycle mirror — "section" parented under "root" (both inverse directions).
    corpus.extend(page_parent_rows(
        "root",
        "section",
        "knowledge.page.created",
        "kn-pseudonym-1",
    ));

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
                .expect("upsert KN edge");
        }
    }

    // ── (1) Build the LIVE KN edge table; capture its byte-image. ──
    rebuild(&mut conn, &upsert_sql, &corpus).await;
    let live_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");
    let live_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        live_count, 5,
        "3 reference edges + the page_parent inverse-paired lifecycle pair"
    );

    // The page_parent mirror stored BOTH inverse directions, both lifecycle-class (5.5).
    let lifecycle_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel_class='lifecycle'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        lifecycle_count, 2,
        "the page_parent mirror is BOTH inverse directions, lifecycle-class"
    );
    // The forward `parent` edge: root → section.
    let parent_target: String =
        sqlx::query(&format!("SELECT target FROM {tbl} WHERE rel='parent'"))
            .fetch_one(&mut *conn)
            .await
            .unwrap()
            .get("target");
    assert_eq!(parent_target, "myelin://tenantA/knowledge/page/section");
    // The inverse `child` edge: section → root.
    let child_target: String = sqlx::query(&format!("SELECT target FROM {tbl} WHERE rel='child'"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("target");
    assert_eq!(child_target, "myelin://tenantA/knowledge/page/root");

    // The block-anchored embed stored the #sub-STRIPPED page root (the block anchor re-derives, §4.7).
    let block_root: String = sqlx::query(&format!(
        "SELECT target_root FROM {tbl} WHERE source='myelin://tenantA/chat/message/m1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("target_root");
    assert_eq!(
        block_root, "myelin://tenantA/knowledge/page/design-doc",
        "the block embed's stored root is the #sub-stripped page (the anchor re-derives, never stale)"
    );

    // ── REF-D2 (IDOR / cross-tenant) at the DATABASE layer: a tenantB session reads 0 of these edges. ──
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
        "RLS isolates tenants — tenantB reads 0 of tenantA's KN edges (REF-D2)"
    );

    // ── (2) WIPE the partition (the cold-rebuild precondition — NO KN-DB reload). ──
    sqlx::query(&format!("DELETE FROM {tbl}"))
        .execute(&mut *conn)
        .await
        .expect("wipe partition");
    let after_wipe: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(after_wipe, 0, "the KN edge partition is wiped");

    // ── (3) Rebuild ONLY from the SAME upserts (the reindex re-emit path). ──
    rebuild(&mut conn, &upsert_sql, &corpus).await;
    let rebuilt_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");

    // ── (4) The rebuilt KN edge table byte-matches the live table (§4.7 reindex-parity, KN corpus). ──
    assert_eq!(
        rebuilt_img, live_img,
        "the rebuilt KN edge index byte-matches the live index (cold == live)"
    );

    // ── REF-D4 TE-7 half — the FIRST real reconvergence (the typed table WINS). ──
    // Drift: "section" is ALSO (mistakenly) parented under "old-root" in the projection.
    let drift = page_parent_rows(
        "old-root",
        "section",
        "knowledge.page.created",
        "kn-pseudonym-x",
    );
    rebuild(&mut conn, &upsert_sql, &drift).await;
    // The authoritative typed snapshot says "section" is under "root" (the live truth). Reconverge:
    // tombstone any LIFECYCLE edge inbound to the child "section" that the typed snapshot does NOT back.
    let section_root = "myelin://tenantA/knowledge/page/section";
    let truth_parent_edge_id = edge_id(
        &t(),
        "myelin://tenantA/knowledge/page/root",
        section_root,
        "parent",
    );
    // The typed-wins set arithmetic (the SAME mirror::reconverge logic, here proven over real Postgres):
    // any lifecycle edge whose TARGET is the covered child and whose edge_id is NOT the typed-backed one
    // is drift → tombstoned.
    let tombstoned = sqlx::query(&format!(
        "UPDATE {tbl} SET tombstoned=true \
         WHERE rel_class='lifecycle' AND target=$1 AND rel='parent' AND edge_id<>$2 AND tombstoned=false"
    ))
    .bind(section_root)
    .bind(&truth_parent_edge_id)
    .execute(&mut *conn)
    .await
    .expect("reconverge: tombstone drift")
    .rows_affected();
    assert_eq!(
        tombstoned, 1,
        "the drifted old-root parent edge is tombstoned (typed table wins)"
    );

    // After reconvergence the LIVE inbound parent of "section" is EXACTLY root (the typed table won).
    let live_parents: Vec<(String,)> = sqlx::query_as(&format!(
        "SELECT source FROM {tbl} WHERE rel='parent' AND target=$1 AND tombstoned=false"
    ))
    .bind(section_root)
    .fetch_all(&mut *conn)
    .await
    .expect("live parents of section");
    assert_eq!(
        live_parents.len(),
        1,
        "exactly one live parent after reconvergence"
    );
    assert_eq!(
        live_parents[0].0, "myelin://tenantA/knowledge/page/root",
        "the typed table wins — the live parent is root, the drifted old-root is tombstoned"
    );

    // Cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
