#![cfg(feature = "integration")]

use myelin_ci_controlplane::{BUMP_CHECK_ATTEMPT_SQL, CREATE_CHECK_ATTEMPT_DDL};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn check_attempt_bump_is_monotonic_on_live_postgres() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let tbl = format!("check_attempt_p361_{suffix}");

    let create =
        CREATE_CHECK_ATTEMPT_DDL.replace("EXISTS check_attempt (", &format!("EXISTS {tbl} ("));
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the check_attempt CREATE TABLE forward-only");

    let bump = BUMP_CHECK_ATTEMPT_SQL
        .replace("INTO check_attempt (", &format!("INTO {tbl} ("))
        .replace("check_attempt.next_attempt", &format!("{tbl}.next_attempt"))
        .replace("check_attempt.current_run", &format!("{tbl}.current_run"));

    let run_a = "11111111-1111-1111-1111-111111111111";
    let run_b = "22222222-2222-2222-2222-222222222222";

    let do_bump = |run: &'static str| {
        let bump = bump.clone();
        let admin = admin.clone();
        async move {
            let row = sqlx::query(&bump)
                .bind("acme")
                .bind("fr-par")
                .bind("myelin://acme/git/repo/core")
                .bind("deadbeef")
                .bind("ci:build")
                .bind(sqlx::types::Uuid::parse_str(run).unwrap())
                .fetch_one(&admin)
                .await
                .expect("the check_attempt bump returns the stamped run_attempt");
            row.get::<i32, _>("run_attempt")
        }
    };

    assert_eq!(
        do_bump(run_a).await,
        1,
        "first dispatch stamps run_attempt 1"
    );
    assert_eq!(do_bump(run_a).await, 1, "same-run retry reuses attempt 1");
    assert_eq!(do_bump(run_b).await, 2, "a re-run bumps to 2");
    assert_eq!(do_bump(run_b).await, 2, "same-run retry reuses attempt 2");
    assert_eq!(do_bump(run_a).await, 3, "and again to 3");
    assert_eq!(do_bump(run_b).await, 4, "strictly increasing");

    let bump_test = bump.replace("'ci:build'", "'ci:test'");
    let _ = bump_test;
    let row = sqlx::query(&bump)
        .bind("acme")
        .bind("fr-par")
        .bind("myelin://acme/git/repo/core")
        .bind("deadbeef")
        .bind("ci:test")
        .bind(sqlx::types::Uuid::parse_str(run_a).unwrap())
        .fetch_one(&admin)
        .await
        .expect("bump the test context");
    assert_eq!(
        row.get::<i32, _>("run_attempt"),
        1,
        "a different context starts its OWN sequence at 1 (per (commit_oid, context))"
    );

    let stored = sqlx::query(&format!(
        "SELECT next_attempt, current_run FROM {tbl} WHERE context = 'ci:build'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        stored.get::<i32, _>("next_attempt"),
        5,
        "after four distinct build runs (returning 1,2,3,4) next_attempt = 5; exact retries consume no attempts"
    );
    assert_eq!(
        stored.get::<sqlx::types::Uuid, _>("current_run"),
        sqlx::types::Uuid::parse_str(run_b).unwrap(),
        "current_run is the run that most recently produced the status (supersession provenance)"
    );

    assert!(
        !CREATE_CHECK_ATTEMPT_DDL
            .to_ascii_uppercase()
            .contains("DROP"),
        "the check_attempt schema is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
