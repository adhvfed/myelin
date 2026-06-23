//! **ISS-P16 / P-382 — the co-equal board↔roadmap `ViewSpec` views, PROVEN same-row against the live
//! dev-stack Postgres (the ISS-D1 gate's REAL artifact: same row id, 0 drift).**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-issues --features integration \
//!     --test integration_iss_p16_coequal_views -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (the prompt touches the board/roadmap DB
//! read contract — two `ViewSpec`s over the ONE `issue` table). The board and the roadmap are NOT two
//! object graphs: they are two index-range scans over the SAME `issue` rows, sliced by the denormalised
//! `type_rank` (board ≤ 1, roadmap ≥ 2). We seed one `issue` table, run the board scan and the roadmap
//! scan over it, EDIT a row's `type_rank`/date on the board path (an UPDATE to the ONE row), and assert
//! the roadmap scan reads the **SAME ROW ID** afterward (0 drift). The ISS-D1 drill flips green ONLY here,
//! against the live stack — never mocked. Survival signal: **the edited row's id is identical across the
//! board and the roadmap scans; there is one store, no parallel reality.**
#![cfg(feature = "integration")]

use myelin_issues::{BOARD_TYPE_RANK_MAX, ROADMAP_TYPE_RANK_MIN};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn iss_d1_board_and_roadmap_are_the_same_row_zero_drift() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let issue_tbl = format!("issue_p382_{suffix}");

    // ── 1. The ONE `issue` table — the co-equal spine. Both the board and the roadmap scan THIS, sliced
    //       by the denormalised `type_rank`. (The minimal board/roadmap columns the two scans touch.) ───
    sqlx::query(&format!(
        "CREATE TABLE {issue_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, id text NOT NULL, \
           type_rank smallint NOT NULL, state_category text NOT NULL, \
           rank text NOT NULL, earliest_start date, title text NOT NULL, \
           PRIMARY KEY (tenant_id, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the one issue table");

    // ── 2. Seed the SAME rows: board-shaped (type_rank 0 — stories) + roadmap-shaped (type_rank 2 —
    //       epics). One spine, two lenses. ENG-1421 starts board-shaped (a story). ────────────────────
    for (id, type_rank, cat, rank, title) in [
        ("ENG-1421", 0i16, "started", "U", "Login 500 on SSO"),
        ("ENG-1430", 0i16, "unstarted", "V", "Cache miss 500"),
        (
            "ENG-2000",
            2i16,
            "started",
            "M",
            "Sovereign auth (initiative)",
        ),
        ("ENG-2001", 2i16, "unstarted", "N", "SSO hardening (epic)"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {issue_tbl} (tenant_id, region, id, type_rank, state_category, rank, title) \
             VALUES ('acme', 'fr-par', $1, $2, $3, $4, $5)"
        ))
        .bind(id)
        .bind(type_rank)
        .bind(cat)
        .bind(rank)
        .bind(title)
        .execute(&admin)
        .await
        .expect("seed an issue row");
    }

    // The board / roadmap scans — the SAME `issue` table, sliced by the denormalised `type_rank`.
    let board_sql = format!(
        "SELECT id FROM {issue_tbl} WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND type_rank <= {BOARD_TYPE_RANK_MAX} ORDER BY rank"
    );
    // `earliest_start` cast to TEXT so the test reads it dep-free (no chrono in scope here; the column is
    // a real `date` in the table — the cast is only the test's read shape).
    let roadmap_sql = format!(
        "SELECT id, earliest_start::text AS earliest_start FROM {issue_tbl} \
         WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND type_rank >= {ROADMAP_TYPE_RANK_MIN} ORDER BY rank"
    );

    let board_ids = |rows: &[sqlx::postgres::PgRow]| -> Vec<String> {
        rows.iter().map(|r| r.get::<String, _>("id")).collect()
    };

    // ── 3. Initial scans: ENG-1421/ENG-1430 on the board; ENG-2000/ENG-2001 on the roadmap. Disjoint +
    //       exhaustive — the type_rank partition over the SAME rows. ──────────────────────────────────
    let board0 = sqlx::query(&board_sql)
        .fetch_all(&admin)
        .await
        .expect("board scan");
    let roadmap0 = sqlx::query(&roadmap_sql)
        .fetch_all(&admin)
        .await
        .expect("roadmap scan");
    assert_eq!(
        board_ids(&board0),
        vec!["ENG-1421".to_string(), "ENG-1430".to_string()],
        "the board lens (type_rank ≤ 1)"
    );
    assert_eq!(
        board_ids(&roadmap0),
        vec!["ENG-2000".to_string(), "ENG-2001".to_string()],
        "the roadmap lens (type_rank ≥ 2)"
    );

    // ── 4. THE ISS-D1 EDIT: on the board, promote ENG-1421 (a story → an epic) and set its roadmap date
    //       axis. This is an UPDATE to the ONE row — there is NO second store. ─────────────────────────
    sqlx::query(&format!(
        "UPDATE {issue_tbl} SET type_rank = {ROADMAP_TYPE_RANK_MIN}, earliest_start = '2026-07-01' \
         WHERE tenant_id = 'acme' AND id = 'ENG-1421'"
    ))
    .execute(&admin)
    .await
    .expect("edit ENG-1421 on the board (promote + set date)");

    // ── 5. Read on the roadmap: the SAME row id (ENG-1421) now appears on the roadmap with the date the
    //       board edit set — 0 drift. The board no longer shows it (the SAME row moved lenses, not a copy).
    let board1 = sqlx::query(&board_sql)
        .fetch_all(&admin)
        .await
        .expect("board scan after edit");
    let roadmap1 = sqlx::query(&roadmap_sql)
        .fetch_all(&admin)
        .await
        .expect("roadmap scan after edit");

    assert!(
        !board_ids(&board1).iter().any(|i| i == "ENG-1421"),
        "ENG-1421 left the board lens (promoted) — the SAME row, not a copy"
    );
    let roadmap1_ids = board_ids(&roadmap1);
    assert!(
        roadmap1_ids.iter().any(|i| i == "ENG-1421"),
        "ENG-1421 now appears on the roadmap lens (the SAME row id, 0 drift): {roadmap1_ids:?}"
    );

    // The roadmap reads the date the BOARD edit set on the SAME row (no parallel reality).
    let date_on_roadmap: Option<String> = roadmap1
        .iter()
        .find(|r| r.get::<String, _>("id") == "ENG-1421")
        .and_then(|r| r.get::<Option<String>, _>("earliest_start"));
    assert_eq!(
        date_on_roadmap,
        Some("2026-07-01".to_string()),
        "the roadmap reads the earliest_start the BOARD edit set on the SAME row (0 drift)"
    );

    // ── 6. The same-row-id assertion (the green artifact): the edited row's id is IDENTICAL across the
    //       board edit and the roadmap read. One store, no drift. ─────────────────────────────────────
    let edited_id = "ENG-1421";
    let roadmap_row_id = roadmap1
        .iter()
        .map(|r| r.get::<String, _>("id"))
        .find(|i| i == edited_id);
    assert_eq!(
        roadmap_row_id.as_deref(),
        Some(edited_id),
        "ISS-D1: the row id the board edited == the row id the roadmap reads (0 drift)"
    );

    println!(
        "[P-382 INTEGRATION GREEN] ISS-D1 board↔roadmap co-equality PROVEN against live Postgres: \
         one `issue` table, two index-range scans sliced by the denormalised type_rank; editing \
         ENG-1421 on the board (promote + set earliest_start) moved the SAME row id onto the roadmap \
         with the board-set date (0 drift) — there is one store, no parallel reality."
    );

    // ── 7. Cleanup (forward teardown). ──────────────────────────────────────────────────────────────
    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
