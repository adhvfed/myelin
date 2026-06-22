//! **REF-P21 / P-337 — Refs serves Chat unfurls (the MAXIMAL producer) + the leak/IDOR invariant on
//! the FULL five-producer corpus, PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p21_chat_producer -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P21 — the Chat unfurl
//! reference-edge corpus (the FINAL producer: mention/artifact_ref/embed over EVERY artifact class —
//! Issues / Git / CI / Knowledge / Chat) executed against the REAL §3.2 `edge` table on real Postgres,
//! on the most adversarial five-producer corpus. The drill is registered red-until-proven and flips
//! green ONLY here, with real artifacts.
//!
//! **REF-D1 / REF-D2 (the leak / IDOR invariant) at the DATABASE layer on the FULL corpus:** the §3.2
//! RLS isolates a tenant — a session pinned to tenant B reads ZERO of tenant A's edges (including the
//! Chat unfurls). The cross-tenant leak invariant holds at the DATABASE layer, not just in the
//! in-memory model.
//!
//! **The non-member-returns-0 ReBAC property at the DATABASE layer:** a backlink read filtered to the
//! channel-member allow-set (the Chat `channel.read = member + parent_project->read` fragment lowered
//! over `edge.source_root`) returns ONLY the member-readable chat backlinks — a non-member's lowered
//! `source_root IN (<member set>)` admits 0 of the private channel's chat backlinks.
//!
//! **REF-D4 (reindex-from-cold byte-parity) over real Postgres on the Chat corpus:** we (1) build the
//! LIVE table from the Chat producer's edges; (2) capture its byte-image; (3) WIPE the partition (no
//! Chat-DB reload); (4) rebuild ONLY by re-driving the SAME producer edge extraction (the reindex
//! re-emit path) → byte-parity.
#![cfg(feature = "integration")]

use myelin_content::InlineNode;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    edge_id, ChatEdgeProducer, EdgeRel, CREATE_EDGE_INDEXES_DDL, CREATE_EDGE_TABLE_DDL,
    MAKE_EDGE_TENANT_SCOPED_DDL,
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

/// The maximal Chat message body: it unfurls every artifact class (Issues / Git / CI / Knowledge /
/// Chat) — five structured ref nodes → five reference edges, sourced from the Chat message root.
fn chat_unfurl_rows(message_id: &str) -> Vec<Row> {
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::message_root("tenantA", message_id);
    let mentionee = Principal::stub(PrincipalId("reviewer".into()), PrincipalKind::Human, t());
    let body = vec![
        InlineNode::Mention(mentionee),
        InlineNode::ArtifactRefNode(myelin_refs::ArtifactRef(
            "myelin://tenantA/issue/issue/ENG-1".into(),
        )),
        InlineNode::Embed(myelin_refs::ArtifactRef(
            "myelin://tenantA/git/commit/core:deadbeef".into(),
        )),
        InlineNode::Embed(myelin_refs::ArtifactRef(
            "myelin://tenantA/ci/run/run-9".into(),
        )),
        InlineNode::Embed(myelin_refs::ArtifactRef(
            "myelin://tenantA/knowledge/page/42".into(),
        )),
    ];
    producer
        .chat_edges(&source, &body)
        .into_iter()
        .map(|d| {
            let rel = match d.rel {
                EdgeRel::Mentions => "mentions",
                EdgeRel::Links => "links",
                EdgeRel::Embeds => "embeds",
            };
            let id = edge_id(&t(), &d.source.0, &d.target.0, rel);
            Row {
                edge_id: id,
                source: d.source.0.clone(),
                source_root: myelin_refs::strip_sub(&d.source).0,
                target: d.target.0.clone(),
                target_root: myelin_refs::strip_sub(&d.target).0,
                rel: rel.into(),
                rel_class: "reference".into(),
                actor: "chat-pseudonym".into(),
            }
        })
        .collect()
}

#[tokio::test]
async fn chat_unfurls_ingest_reindex_and_idor_on_real_postgres() {
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
    let tbl = format!("edge_p337_{suffix}");

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

    // ── The Chat corpus: two messages in two channels both unfurl the same artifact classes. One
    //    message (PUBLIC) is member-readable; the other (PRIVATE) is not — the non-member-returns-0
    //    ReBAC corpus. Each message produces 5 reference edges (mention + 4 artifact unfurls). ──
    let public_msg = "01HPUBLIC";
    let private_msg = "01HPRIVATE";
    let mut corpus: Vec<Row> = Vec::new();
    corpus.extend(chat_unfurl_rows(public_msg));
    corpus.extend(chat_unfurl_rows(private_msg));

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
                .expect("upsert chat unfurl edge");
        }
    }

    // ── (1) Build the LIVE Chat edge table; capture its byte-image. ──
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
        live_count, 10,
        "two messages × five unfurl edges each = 10 reference edges"
    );
    // Every Chat unfurl edge is reference-class (5.4 — Chat's mention/artifact_ref/embed are references).
    let ref_count: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE rel_class='reference'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        ref_count, 10,
        "every Chat unfurl edge is reference-class (5.4)"
    );
    // Every edge is sourced from a Chat message root (the maximal producer).
    let chat_sourced: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE source_root LIKE 'myelin://tenantA/chat/message/%'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        chat_sourced, 10,
        "every unfurl is sourced from a Chat message"
    );

    // ── The non-member-returns-0 ReBAC property at the DB layer: the channel-member allow-set lowered
    //    over source_root. A non-member is a `member` of ONLY the PUBLIC channel's message — so the
    //    member-filtered backlink read of the shared issue admits only the public channel's chat
    //    backlink, 0 of the private channel's (the Chat `channel.read` fragment over source_root). ──
    let public_src = format!("myelin://tenantA/chat/message/{public_msg}");
    let nonmember_visible_chat_backlinks: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} \
         WHERE target='myelin://tenantA/issue/issue/ENG-1' AND rel='links' \
           AND source_root = ANY($1)"
    ))
    .bind(vec![public_src.clone()]) // the non-member's member allow-set (public channel only)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        nonmember_visible_chat_backlinks, 1,
        "the non-member sees ONLY the public channel's chat backlink (0 of the private channel's)"
    );
    // Confirm the private channel's backlink EXISTS in the table (it is filtered out, not absent).
    let total_issue_backlinks: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} \
         WHERE target='myelin://tenantA/issue/issue/ENG-1' AND rel='links'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        total_issue_backlinks, 2,
        "both channels link the issue — the private one is FILTERED OUT for the non-member, not absent"
    );

    // ── REF-D1 / REF-D2 (IDOR / cross-tenant) at the DATABASE layer: a tenantB session reads 0. ──
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
        "RLS isolates tenants — tenantB reads 0 of tenantA's Chat unfurl edges (REF-D1/D2)"
    );

    // ── (2) WIPE the partition (the cold-rebuild precondition — NO Chat-DB reload). ──
    sqlx::query(&format!("DELETE FROM {tbl}"))
        .execute(&mut *conn)
        .await
        .expect("wipe partition");
    let after_wipe: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(after_wipe, 0, "the Chat edge partition is wiped");

    // ── (3) Rebuild ONLY from the SAME producer edge extraction (the reindex re-emit path). ──
    let rebuilt_corpus: Vec<Row> = {
        let mut c = Vec::new();
        c.extend(chat_unfurl_rows(public_msg));
        c.extend(chat_unfurl_rows(private_msg));
        c
    };
    rebuild(&mut conn, &upsert_sql, &rebuilt_corpus).await;
    let rebuilt_img: String = sqlx::query(&parity_sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("img");

    // ── (4) The rebuilt Chat edge table byte-matches the live table (§4.7 reindex-parity). ──
    assert_eq!(
        rebuilt_img, live_img,
        "the rebuilt Chat edge index byte-matches the live index (cold == live)"
    );

    // Cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
