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

    let mut corpus: Vec<Row> = Vec::new();
    corpus.extend(relation_rows("ENG-1", "ENG-2", "blocks"));
    corpus.extend(relation_rows("PLAT-9", "ENG-1", "parent"));
    corpus.extend(relation_rows("ENG-3", "ENG-4", "relates"));
    corpus.extend(relation_rows("ENG-5", "ENG-6", "closes"));

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
    let relates_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel='relates'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        relates_count, 2,
        "relates is symmetric - both directions stored"
    );
    let closes_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel='closes'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        closes_count, 1,
        "closes has no frozen inverse - forward only"
    );

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
        "RLS isolates tenants - tenantB reads 0 of tenantA's Issues lifecycle edges (REF-D9)"
    );

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

    rebuild(&mut conn, &upsert_sql, &corpus).await;
    let rebuilt_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");

    assert_eq!(
        rebuilt_img, live_img,
        "the rebuilt Issues edge index byte-matches the live index (cold == live)"
    );

    let drift = relation_rows("ENG-1", "ENG-STALE", "blocks");
    rebuild(&mut conn, &upsert_sql, &drift).await;
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
        "the typed table wins - ENG-1 blocks ENG-2, the drifted ENG-STALE is tombstoned"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
