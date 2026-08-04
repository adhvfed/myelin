#![cfg(feature = "integration")]

use myelin_events::{ReindexSource, SnapshotScope};
use myelin_refs::{strip_sub, ArtifactRef};
use myelin_refs_service::{
    edge_id, RefsReindexSource, SourceEdge, CREATE_EDGE_INDEXES_DDL, CREATE_EDGE_TABLE_DDL,
    MAKE_EDGE_TENANT_SCOPED_DDL, REFS_OWNER_TOKEN,
};

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

fn src_edge(agg: &str, source: &str, target: &str, rel: &str, actor: &str) -> SourceEdge {
    SourceEdge {
        aggregate: agg.into(),
        version: 1,
        source: ArtifactRef(source.into()),
        target: ArtifactRef(target.into()),
        rel: rel.into(),
        origin_actor: actor.into(),
        zookie: Some("zk-1".into()),
    }
}

#[tokio::test]
async fn reindex_from_source_byte_matches_live_on_real_postgres() {
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
    let tbl = format!("edge_p165_{suffix}");

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

    let mut truth = RefsReindexSource::new();
    truth.record(src_edge(
        "refs.edge:1",
        "myelin://tenantA/chat/message/m1#block-9",
        "myelin://tenantA/knowledge/page/7c2#block-3",
        "embeds",
        "p-opaque-1",
    ));
    truth.record(src_edge(
        "refs.edge:2",
        "myelin://tenantA/chat/message/m2",
        "myelin://tenantA/issue/issue/ENG-1",
        "mentions",
        "p-opaque-2",
    ));
    truth.record(src_edge(
        "refs.edge:3",
        "myelin://tenantA/git/commit/abc",
        "myelin://tenantA/issue/issue/ENG-2",
        "links",
        "p-opaque-1",
    ));

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
         VALUES ('tenantA','fr-par',$1,$2,$3,$4,$5,$6,'reference',$7,$8,now(),'zk-1',false,'kms://tenantA/0/tenant') \
         ON CONFLICT (tenant_id, edge_id) DO NOTHING"
    );

    let parity_sql = format!(
        "SELECT string_agg( \
            edge_id||'|'||source||'|'||source_root||'|'||target||'|'||target_root||'|'||rel||'|'|| \
            rel_class||'|'||origin_actor||'|'||coalesce(zookie,'')||'|'||tombstoned, E'\\n' \
            ORDER BY edge_id) AS img FROM {tbl}"
    );

    let scope = SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all");
    async fn rebuild(
        conn: &mut sqlx::PgConnection,
        upsert_sql: &str,
        truth: &RefsReindexSource,
        scope: &SnapshotScope,
    ) {
        for draft in truth.replay(scope, None) {
            let source = draft
                .payload
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            let target = draft
                .payload
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            let rel = draft
                .payload
                .get("rel")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            let actor = draft
                .payload
                .get("origin_actor")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            let tenant = myelin_tenancy::TenantId("tenantA".into());
            let id = edge_id(&tenant, &source, &target, &rel);
            let source_root = strip_sub(&ArtifactRef(source.clone())).0;
            let target_root = strip_sub(&ArtifactRef(target.clone())).0;
            sqlx::query(upsert_sql)
                .bind(&id)
                .bind(&source)
                .bind(&source_root)
                .bind(&target)
                .bind(&target_root)
                .bind(&rel)
                .bind(format!("evt-{id}"))
                .bind(&actor)
                .execute(&mut *conn)
                .await
                .expect("upsert edge");
        }
    }

    rebuild(&mut conn, &upsert_sql, &truth, &scope).await;
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
    assert_eq!(live_count, 3, "the live table holds the 3 edges");

    sqlx::query(&format!("DELETE FROM {tbl}"))
        .execute(&mut *conn)
        .await
        .expect("wipe partition");
    let after_wipe: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(after_wipe, 0, "the partition is wiped");

    rebuild(&mut conn, &upsert_sql, &truth, &scope).await;
    let rebuilt_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");

    assert_eq!(
        rebuilt_img, live_img,
        "the rebuilt edge index byte-matches the live index (cold == live)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
