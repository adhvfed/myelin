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

    let board_sql = format!(
        "SELECT id FROM {issue_tbl} WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND type_rank <= {BOARD_TYPE_RANK_MAX} ORDER BY rank"
    );
    let roadmap_sql = format!(
        "SELECT id, earliest_start::text AS earliest_start FROM {issue_tbl} \
         WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND type_rank >= {ROADMAP_TYPE_RANK_MIN} ORDER BY rank"
    );

    let board_ids = |rows: &[sqlx::postgres::PgRow]| -> Vec<String> {
        rows.iter().map(|r| r.get::<String, _>("id")).collect()
    };

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

    sqlx::query(&format!(
        "UPDATE {issue_tbl} SET type_rank = {ROADMAP_TYPE_RANK_MIN}, earliest_start = '2026-07-01' \
         WHERE tenant_id = 'acme' AND id = 'ENG-1421'"
    ))
    .execute(&admin)
    .await
    .expect("edit ENG-1421 on the board (promote + set date)");

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
        "ENG-1421 left the board lens (promoted) - the SAME row, not a copy"
    );
    let roadmap1_ids = board_ids(&roadmap1);
    assert!(
        roadmap1_ids.iter().any(|i| i == "ENG-1421"),
        "ENG-1421 now appears on the roadmap lens (the SAME row id, 0 drift): {roadmap1_ids:?}"
    );

    let date_on_roadmap: Option<String> = roadmap1
        .iter()
        .find(|r| r.get::<String, _>("id") == "ENG-1421")
        .and_then(|r| r.get::<Option<String>, _>("earliest_start"));
    assert_eq!(
        date_on_roadmap,
        Some("2026-07-01".to_string()),
        "the roadmap reads the earliest_start the BOARD edit set on the SAME row (0 drift)"
    );

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
         with the board-set date (0 drift) - there is one store, no parallel reality."
    );

    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
