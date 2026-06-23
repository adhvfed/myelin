//! **CI-P18 / P-361 — the `check_attempt` monotonic counter bump, PROVEN against the live dev-stack
//! Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p18_check_attempt_bump -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (CI-P18 touches the `check_attempt`
//! OLTP table, arch 01 §3.2): the production [`BUMP_CHECK_ATTEMPT_SQL`] UPSERT, run against real
//! Postgres, is MONOTONIC — the FIRST dispatch of `(commit_oid, context)` returns `run_attempt = 1`,
//! each re-dispatch bumps strictly (2, 3, …), and the returned attempt is the value CI stamps into
//! `CheckStatus.run_attempt`. The supersession key is the counter, never wall-clock (X-1). The drill
//! is registered red-until-proven and flips green ONLY here, against the live stack — never mocked.
//!
//! The test applies the REAL `check_attempt` DDL onto a uniquely-suffixed throwaway table so
//! concurrent runs don't collide; the bump SQL SHAPE under test is byte-for-byte the production
//! constant (only the table identifier is suffixed for isolation + cleanup).
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
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let tbl = format!("check_attempt_p361_{suffix}");

    // ── 1. Apply the REAL forward-only check_attempt CREATE TABLE (arch 01 §3.2 shape), suffixed. ──
    let create =
        CREATE_CHECK_ATTEMPT_DDL.replace("EXISTS check_attempt (", &format!("EXISTS {tbl} ("));
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the check_attempt CREATE TABLE forward-only");

    // The production bump SQL, retargeted onto the suffixed table (the SHAPE is byte-identical —
    // both the INSERT target AND the ON-CONFLICT `check_attempt.next_attempt` self-reference).
    let bump = BUMP_CHECK_ATTEMPT_SQL
        .replace("INTO check_attempt (", &format!("INTO {tbl} ("))
        .replace("check_attempt.next_attempt", &format!("{tbl}.next_attempt"));

    let run_a = "11111111-1111-1111-1111-111111111111";
    let run_b = "22222222-2222-2222-2222-222222222222";

    let do_bump = |run: &'static str| {
        let bump = bump.clone();
        let admin = admin.clone();
        async move {
            let row = sqlx::query(&bump)
                .bind("acme") // tenant_id
                .bind("fr-par") // region
                .bind("myelin://acme/git/repo/core") // repo_ref
                .bind("deadbeef") // commit_oid
                .bind("ci:build") // context
                .bind(sqlx::types::Uuid::parse_str(run).unwrap()) // current_run
                .fetch_one(&admin)
                .await
                .expect("the check_attempt bump returns the stamped run_attempt");
            row.get::<i32, _>("run_attempt")
        }
    };

    // ── 2. The FIRST dispatch of (commit_oid, context) → run_attempt = 1. ───────────────────────
    assert_eq!(
        do_bump(run_a).await,
        1,
        "first dispatch stamps run_attempt 1"
    );
    // ── 3. Each RE-dispatch bumps strictly (2, 3, 4) — monotonic, never wall-clock. ─────────────
    assert_eq!(do_bump(run_b).await, 2, "a re-run bumps to 2");
    assert_eq!(do_bump(run_a).await, 3, "and again to 3");
    assert_eq!(do_bump(run_b).await, 4, "strictly increasing");

    // ── 4. A DIFFERENT context has its OWN monotonic sequence (the key is (commit_oid, context)). ─
    let bump_test = bump.replace("'ci:build'", "'ci:test'");
    let _ = bump_test; // (the bind carries the context; assert the independent sequence via a bind)
    let row = sqlx::query(&bump)
        .bind("acme")
        .bind("fr-par")
        .bind("myelin://acme/git/repo/core")
        .bind("deadbeef")
        .bind("ci:test") // a DIFFERENT context
        .bind(sqlx::types::Uuid::parse_str(run_a).unwrap())
        .fetch_one(&admin)
        .await
        .expect("bump the test context");
    assert_eq!(
        row.get::<i32, _>("run_attempt"),
        1,
        "a different context starts its OWN sequence at 1 (per (commit_oid, context))"
    );

    // ── 5. The stored next_attempt reflects the highest issued + the current_run provenance. ────
    let stored = sqlx::query(&format!(
        "SELECT next_attempt, current_run FROM {tbl} WHERE context = 'ci:build'"
    ))
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        stored.get::<i32, _>("next_attempt"),
        5,
        "after 4 build dispatches (returning 1,2,3,4) next_attempt = 5 (the NEXT bump returns 4)"
    );
    // The current_run is the run that most recently produced this context's status — the LAST build
    // bump was run_b (do_bump order: run_a, run_b, run_a, run_b → the 4th, returning attempt 4).
    assert_eq!(
        stored.get::<sqlx::types::Uuid, _>("current_run"),
        sqlx::types::Uuid::parse_str(run_b).unwrap(),
        "current_run is the run that most recently produced the status (supersession provenance)"
    );

    // ── 6. Forward-only: the production DDL carries NO DROP. ────────────────────────────────────
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
