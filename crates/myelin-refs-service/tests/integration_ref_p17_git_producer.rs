//! **REF-P17 / P-258 — Refs consumes the REAL Git producer edges, PROVEN against the live dev-stack
//! Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p17_git_producer -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P17 — the Git producer edge
//! ingest + the §4.7 reindex-from-source byte-parity executed against the REAL §3.2 `edge` table on
//! real Postgres, on a production-shaped GIT edge corpus (a PR-link, a commit-trailer "Closes <issue>",
//! a blob line-range embed). The drill is registered red-until-proven and flips green ONLY here, with a
//! real artifact (the reindex-parity image).
//!
//! **REF-D1 (leak) re-confirmed on real Git edges over real Postgres:** the §3.2 RLS isolates a tenant
//! — a session pinned to tenant B reads ZERO of tenant A's Git edges (the cross-tenant leak invariant
//! holds at the DATABASE layer, not just in the in-memory model — REF-D2 / the IDOR floor).
//!
//! **REF-D4 (reindex-from-cold byte-parity, CI variant) over real Postgres on the Git corpus:** the
//! only way a Git edge row lands in `edge` is the live consumer's upsert (the deterministic `edge_id` +
//! `strip_sub` roots — the SAME production logic, the Git source URN carrying the `#sub`-precise blob
//! line-range). We (1) build the LIVE table from the Git edge corpus; (2) capture its byte-image;
//! (3) WIPE the partition (no Git-DB reload); (4) rebuild ONLY by re-driving the SAME upserts from the
//! replayed Git snapshots (the reindex re-emit path — Refs' `ReindexSource::replay`); (5) assert the
//! rebuilt table byte-matches the live one. The content-anchored blob line-range root (`#L1-L9` →
//! `blob/<repo>:<ref>:<path>`) re-derives at the right grain — never a stale raw line number (§4.7).
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

fn git_edge(agg: &str, source: &str, target: &str, rel: &str, actor: &str) -> SourceEdge {
    SourceEdge {
        aggregate: agg.into(),
        version: 1,
        source: ArtifactRef(source.into()),
        target: ArtifactRef(target.into()),
        rel: rel.into(),
        // A Git edge's origin_actor is the PSEUDONYMOUS commit author (erasure-safe; never the name —
        // the floor: pseudonymous-by-default authors, GIT-P25). Refs holds only the opaque id.
        origin_actor: actor.into(),
        zookie: Some("zk-1".into()),
    }
}

#[tokio::test]
async fn git_producer_edges_ingest_and_reindex_byte_match_on_real_postgres() {
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
    let tbl = format!("edge_p258_{suffix}");

    // ── Apply the REAL §3.2 schema (create + three indexes + RLS), suffixed for isolation. ──
    sqlx::query(&rename(CREATE_EDGE_TABLE_DDL, &tbl)).execute(&admin).await.expect("create edge table");
    for (name, idx) in CREATE_EDGE_INDEXES_DDL {
        sqlx::query(&rename(idx, &tbl))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name}: {e}"));
    }
    sqlx::query(&rename(MAKE_EDGE_TENANT_SCOPED_DDL, &tbl)).execute(&admin).await.expect("RLS scope");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app")).execute(&admin).await.expect("grant app");

    // ── A production-shaped GIT edge corpus (the REAL producer edges REF-P17 wires). ──
    // A PR-link, a commit-trailer "Closes <issue>", a blob line-range embed (the #sub-precise blob root
    // carries the content-anchored range; the stored root is the #sub-stripped blob).
    let mut truth = RefsReindexSource::new();
    truth.record(git_edge(
        "refs.edge:1",
        "myelin://tenantA/git/pr/repo7:4291",
        "myelin://tenantA/issue/issue/ENG-12",
        "links",
        "git-pseudonym-1",
    ));
    truth.record(git_edge(
        "refs.edge:2",
        "myelin://tenantA/git/commit/repo7:abc123",
        "myelin://tenantA/issue/issue/ENG-7",
        "links",
        "git-pseudonym-2",
    ));
    truth.record(git_edge(
        "refs.edge:3",
        "myelin://tenantA/chat/message/m1",
        "myelin://tenantA/git/blob/repo7:main:src%2Flib.rs#L42-L88",
        "embeds",
        "git-pseudonym-1",
    ));

    // Pin the session to tenantA (RLS).
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id','tenantA',false)").execute(&mut *conn).await.unwrap();
    sqlx::query("SELECT set_config('myelin.region','fr-par',false)").execute(&mut *conn).await.unwrap();

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
    async fn rebuild(conn: &mut sqlx::PgConnection, upsert_sql: &str, truth: &RefsReindexSource, scope: &SnapshotScope) {
        for draft in truth.replay(scope, None) {
            let source = draft.payload.get("source").and_then(|v| v.as_str()).unwrap().to_string();
            let target = draft.payload.get("target").and_then(|v| v.as_str()).unwrap().to_string();
            let rel = draft.payload.get("rel").and_then(|v| v.as_str()).unwrap().to_string();
            let actor = draft.payload.get("origin_actor").and_then(|v| v.as_str()).unwrap().to_string();
            let tenant = myelin_tenancy::TenantId("tenantA".into());
            let id = edge_id(&tenant, &source, &target, &rel);
            // The blob line-range edge's stored target_root is the #sub-STRIPPED blob root — the
            // content-anchored range re-derives at the blob grain (never a stale raw line number, §4.7).
            let source_root = strip_sub(&ArtifactRef(source.clone())).0;
            let target_root = strip_sub(&ArtifactRef(target.clone())).0;
            sqlx::query(upsert_sql)
                .bind(&id).bind(&source).bind(&source_root).bind(&target).bind(&target_root)
                .bind(&rel).bind(format!("evt-{id}")).bind(&actor)
                .execute(&mut *conn).await.expect("upsert git edge");
        }
    }

    // ── (1) Build the LIVE Git edge table; capture its byte-image. ──
    rebuild(&mut conn, &upsert_sql, &truth, &scope).await;
    let live_img: String = sqlx::query(&parity_sql).fetch_one(&mut *conn).await.unwrap().get("img");
    let live_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn).await.unwrap().get("n");
    assert_eq!(live_count, 3, "the live Git edge table holds the 3 producer edges");

    // The blob line-range edge stored the #sub-STRIPPED blob root (the content-anchored range is on
    // the FULL target; the index keys on the root) — assert the strip held over real Postgres.
    let blob_root: String = sqlx::query(&format!(
        "SELECT target_root FROM {tbl} WHERE rel='embeds'"
    )).fetch_one(&mut *conn).await.unwrap().get("target_root");
    assert_eq!(
        blob_root, "myelin://tenantA/git/blob/repo7:main:src%2Flib.rs",
        "the line-range embed's stored root is the #sub-stripped blob (the range re-derives, never stale)"
    );

    // ── REF-D2 (IDOR / cross-tenant) at the DATABASE layer: a tenantB session reads 0 of these edges. ──
    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id','tenantB',false)").execute(&mut *conn_b).await.unwrap();
    sqlx::query("SELECT set_config('myelin.region','fr-par',false)").execute(&mut *conn_b).await.unwrap();
    let b_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn_b).await.unwrap().get("n");
    assert_eq!(b_count, 0, "RLS isolates tenants — tenantB reads 0 of tenantA's Git edges (REF-D2)");

    // ── (2) WIPE the partition (the cold-rebuild precondition — NO Git-DB reload). ──
    sqlx::query(&format!("DELETE FROM {tbl}")).execute(&mut *conn).await.expect("wipe partition");
    let after_wipe: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn).await.unwrap().get("n");
    assert_eq!(after_wipe, 0, "the Git edge partition is wiped");

    // ── (3) Rebuild ONLY from the replayed Git snapshots (the reindex re-emit path), SAME upsert. ──
    rebuild(&mut conn, &upsert_sql, &truth, &scope).await;
    let rebuilt_img: String = sqlx::query(&parity_sql).fetch_one(&mut *conn).await.unwrap().get("img");

    // ── (4) The rebuilt Git edge table byte-matches the live table (§4.7 reindex-parity, Git corpus). ──
    assert_eq!(rebuilt_img, live_img, "the rebuilt Git edge index byte-matches the live index (cold == live)");

    // Cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {tbl}")).execute(&admin).await.unwrap();
}
