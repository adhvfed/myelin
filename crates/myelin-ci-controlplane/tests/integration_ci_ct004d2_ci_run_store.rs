//! **CT-004d.2 chunk 4 — the durable `ci_run` writer (`CiRunStore`) round-trip + idempotency + GENUINE
//! RLS isolation, PROVEN on live PG under the APP role (non-BYPASSRLS).**
//!
//! The co-commit atomicity (the `ci_run` ROW ⇄ dedup mark in one tx) is proven in ci-dispatch's
//! `tests/integration_ci_ct004b_trigger_consumer.rs` (`chunk4_*`). THIS proves the store's own verbs
//! against live Postgres:
//!   1. **Round-trip:** `insert_ci_run` writes every column; `get_ci_run` reads them ALL back faithfully.
//!   2. **Exact replay:** a second `insert_ci_run` of the SAME immutable row returns `false` only after
//!      the mandatory second locking statement verifies it; a divergence is a typed collision.
//!   3. **RLS (GENUINE — the `myelin_app` role, RLS ENFORCED, not BYPASSRLS):** a `get_ci_run` scoped
//!      to tenant B CANNOT read tenant A's row even with A's `run_id` in hand — the `(tenant, region)`
//!      RLS policy hides it (`with_tenant_tx` sets the GUC transaction-scoped; the app role has no
//!      BYPASSRLS). This is the no-cross-tenant-query-path floor, live.
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004d2_ci_run_store -- --nocapture
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    ci_run_store_factory, CiRunInsert, CiRunStoreError, ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL,
    ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL, ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL,
    CREATE_CI_RUN_DDL, ERASED_PSEUDONYM,
};
use myelin_events::{HandlerTx, CONSUMER_DEDUP_MIGRATION};
use sqlx::{Executor, PgPool};
use std::time::Duration;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("ci_ct004d2_{}", std::process::id())
}

/// A pool whose connections pin `search_path` to `schema` (so the store's unqualified `ci_run`
/// resolves to the per-test schema's table). `url` selects the role (admin = owner/BYPASSRLS to build;
/// app = RLS-ENFORCED to exercise).
async fn pool_on(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to dev Postgres (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

fn row(tenant: &str, run_id: &str) -> CiRunInsert {
    CiRunInsert {
        tenant_id: tenant.into(),
        region: "fr-par".into(),
        run_id: run_id.into(),
        project_id: "22222222-2222-2222-2222-222222222222".into(),
        pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
        wf_run_id: "44444444-4444-4444-4444-444444444444".into(),
        definition_snapshot: "blake3:snap-abcd".into(),
        trigger_kind: "push".into(),
        concurrency_group: None,
        pr_head_generation: None,
        trust_tier: "trusted".into(),
        state: "queued".into(),
        correlation_id: "corr-1".into(),
        cause_event_id: Some("ev-push-1".into()),
        cause_depth: 0,
        caused_by: None,
        repo_ref: Some("web".into()),
        commit_oid: Some("deadbeefcafe".into()),
        triggered_by: Some("psn:actor-8a2f".into()),
    }
}

fn run_id(n: u64) -> String {
    format!("10000000-0000-0000-0000-{n:012}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chunk4_ci_run_store_verifies_exact_replays_and_rejects_collisions() {
    let schema = schema_name();
    let admin = pool_on(&admin_url(), &schema).await;

    // ── Build the schema + the FORCE-RLS ci_run table (the ONE platform tenant-scoping helper), grant
    //    the app role USAGE + table privileges (mirrors integration_ci_p6). ──
    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop prior");
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    admin
        .execute(CREATE_CI_RUN_DDL)
        .await
        .expect("create ci_run");
    admin
        .execute(ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL)
        .await
        .expect("add ci_run causal provenance");
    admin
        .execute(ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL)
        .await
        .expect("add ci_run concurrency identity");
    admin
        .execute(ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL)
        .await
        .expect("add ci_run PR ordering authority");
    admin
        .execute(CONSUMER_DEDUP_MIGRATION)
        .await
        .expect("create consumer_dedup");
    admin
        .execute("SELECT myelin_make_tenant_scoped('ci_run')")
        .await
        .expect("make ci_run tenant-scoped (FORCE RLS)");
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .expect("grant schema usage");
    admin
        .execute("GRANT ALL ON ci_run TO myelin_app")
        .await
        .expect("grant table privileges");
    admin
        .execute("GRANT ALL ON consumer_dedup TO myelin_app")
        .await
        .expect("grant dedup privileges");

    // ── The store runs on the APP pool (RLS ENFORCED — no BYPASSRLS). ──
    let app = pool_on(&app_url(), &schema).await;
    let store = ci_run_store_factory(app.clone());

    let run_a = "11111111-1111-1111-1111-111111111111";
    let a = row("tenantA", run_a);

    // (1) Round-trip: fresh insert → true; every column reads back faithfully.
    assert!(
        store.insert_ci_run(&a).await.expect("insert tenantA"),
        "a fresh row inserts (true)"
    );
    let got = store
        .get_ci_run("tenantA", "fr-par", run_a)
        .await
        .expect("get tenantA")
        .expect("the row is present");
    assert_eq!(
        got.tenant_id, "tenantA",
        "authoritative tenant partition round-trips"
    );
    assert_eq!(got.run_id, run_a, "run_id round-trips");
    assert_eq!(got.region, "fr-par");
    assert_eq!(got.project_id, "22222222-2222-2222-2222-222222222222");
    assert_eq!(got.pipeline_id, "33333333-3333-3333-3333-333333333333");
    assert_eq!(got.wf_run_id, "44444444-4444-4444-4444-444444444444");
    assert_eq!(got.repo_ref.as_deref(), Some("web"));
    assert_eq!(got.commit_oid.as_deref(), Some("deadbeefcafe"));
    assert_eq!(got.cause_event_id.as_deref(), Some("ev-push-1"));
    assert_eq!(got.definition_snapshot, "blake3:snap-abcd");
    assert_eq!(got.trigger_kind, "push");
    assert!(got.concurrency_group.is_none());
    assert!(got.pr_head_generation.is_none());
    assert_eq!(got.trust_tier, "trusted");
    assert_eq!(got.state, "queued");
    assert_eq!(got.correlation_id, "corr-1");

    // (2) Idempotent only after exact immutable replay verification: false, one row.
    assert!(
        !store.insert_ci_run(&a).await.expect("re-insert tenantA"),
        "an exact redelivery is verified before returning false"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run WHERE run_id = $1::uuid")
        .bind(run_a)
        .fetch_one(&admin) // count via admin (BYPASSRLS) — the ground truth is exactly one row.
        .await
        .unwrap();
    assert_eq!(
        n, 1,
        "exactly one durable ci_run row after the idempotent re-insert"
    );

    let mut pr = row("tenantA", &run_id(3));
    pr.trigger_kind = "pull_request".into();
    pr.concurrency_group = Some("pr:team/web:42".into());
    pr.pr_head_generation = Some(7);
    assert!(store.insert_ci_run(&pr).await.unwrap());
    let stored_pr = store
        .get_ci_run("tenantA", "fr-par", &pr.run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_pr.concurrency_group.as_deref(),
        Some("pr:team/web:42"),
        "event-derived PR concurrency identity round-trips durably"
    );
    assert_eq!(
        stored_pr.pr_head_generation,
        Some(7),
        "producer-authored PR ordering authority round-trips durably"
    );

    // Producer insertion takes the same exact group lock as starter classification/cancellation.
    // Hold that lock independently and prove the real store cannot publish a newer generation
    // behind a starter transaction that is still deciding the group's durable high-water mark.
    let mut held_group = admin.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("myelin.ci.pr-run-supersession.v1:tenantA:fr-par:pr:team/web:43")
        .execute(&mut *held_group)
        .await
        .unwrap();
    let mut later_pr = row("tenantA", &run_id(4));
    later_pr.trigger_kind = "pull_request".into();
    later_pr.concurrency_group = Some("pr:team/web:43".into());
    later_pr.pr_head_generation = Some(8);
    let lock_store = store.clone();
    let mut blocked_insert = tokio::spawn(async move { lock_store.insert_ci_run(&later_pr).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut blocked_insert)
            .await
            .is_err(),
        "producer insertion must wait for the shared PR-group transaction lock"
    );
    held_group.commit().await.unwrap();
    assert!(
        blocked_insert.await.unwrap().unwrap(),
        "producer insertion proceeds after the starter-equivalent lock commits"
    );
    assert!(
        sqlx::query(
            "UPDATE ci_run SET concurrency_group = 'pr:web:42' \
             WHERE tenant_id = 'tenantA' AND run_id = $1::uuid"
        )
        .bind(run_a)
        .execute(&admin)
        .await
        .is_err(),
        "the database refuses PR scheduler authority on a non-PR run"
    );
    assert!(
        sqlx::query(
            "UPDATE ci_run SET pr_head_generation = 7 \
             WHERE tenant_id = 'tenantA' AND run_id = $1::uuid"
        )
        .bind(run_a)
        .execute(&admin)
        .await
        .is_err(),
        "the database refuses PR ordering authority on a non-PR run"
    );
    assert!(
        sqlx::query(
            "UPDATE ci_run SET pr_head_generation = 0 \
             WHERE tenant_id = 'tenantA' AND run_id = $1::uuid"
        )
        .bind(&pr.run_id)
        .execute(&admin)
        .await
        .is_err(),
        "the database refuses a non-positive PR ordering generation"
    );
    assert!(
        sqlx::query(
            "UPDATE ci_run SET concurrency_group = 'pr:web:0' \
             WHERE tenant_id = 'tenantA' AND run_id = $1::uuid"
        )
        .bind(&pr.run_id)
        .execute(&admin)
        .await
        .is_err(),
        "the database refuses malformed PR scheduler authority"
    );

    let canonical_run = run_id(1);
    let mut canonical = row("tenantA", &canonical_run);
    canonical.project_id = "abcdefab-cdef-abcd-efab-cdefabcdefab".into();
    assert!(store.insert_ci_run(&canonical).await.unwrap());
    let mut noncanonical_replay = canonical.clone();
    noncanonical_replay.project_id = "ABCDEFAB-CDEF-ABCD-EFAB-CDEFABCDEFAB".into();
    assert!(
        !store
            .insert_ci_run(&noncanonical_replay)
            .await
            .expect("UUID-semantic replay"),
        "PostgreSQL-equivalent UUID spelling/case is an exact replay, not a string collision"
    );

    // (3) GENUINE RLS: a tenantB-scoped read CANNOT see tenantA's row even knowing its run_id (the app
    //     role is RLS-enforced; with_tenant_tx sets the (tenant, region) GUC → the policy hides it).
    let cross = store
        .get_ci_run("tenantB", "fr-par", run_a)
        .await
        .expect("get under tenantB scope");
    assert!(
        cross.is_none(),
        "RLS: tenantB cannot read tenantA's ci_run (no cross-tenant query path)"
    );

    // tenantB can write + read its OWN row (RLS admits the in-tenant path), including the same run
    // UUID because the durable key is tenant-qualified.
    let run_b = "55555555-5555-5555-5555-555555555555";
    assert!(
        store
            .insert_ci_run(&row("tenantB", run_b))
            .await
            .expect("insert tenantB"),
        "tenantB writes its own row"
    );
    assert!(
        store
            .get_ci_run("tenantB", "fr-par", run_b)
            .await
            .expect("get tenantB own")
            .is_some(),
        "tenantB reads its OWN row"
    );

    let shared_run = run_id(2);
    assert!(store
        .insert_ci_run(&row("tenantA", &shared_run))
        .await
        .unwrap());
    assert!(
        store
            .insert_ci_run(&row("tenantB", &shared_run))
            .await
            .unwrap(),
        "the same run UUID in another tenant is a distinct fresh row"
    );

    // (4) Every immutable field is collision checked. Region is special: the explicit region
    // predicate + FORCE RLS make the conflicting row invisible, so it fails closed without region
    // disclosure. All visible immutable differences report field names only.
    let immutable_fields = [
        "region",
        "project_id",
        "pipeline_id",
        "wf_run_id",
        "repo_ref",
        "commit_oid",
        "cause_event_id",
        "definition_snapshot",
        "trigger_kind",
        "concurrency_group",
        "pr_head_generation",
        "trust_tier",
        "correlation_id",
        "triggered_by",
    ];
    for (index, field) in immutable_fields.into_iter().enumerate() {
        let collision_run = run_id(10 + index as u64);
        let mut original = row("tenantA", &collision_run);
        if matches!(field, "concurrency_group" | "pr_head_generation") {
            original.trigger_kind = "pull_request".into();
            original.concurrency_group = Some("pr:web:42".into());
            original.pr_head_generation = Some(7);
        }
        assert!(
            store.insert_ci_run(&original).await.unwrap(),
            "seed {field}"
        );
        let mut replay = original.clone();
        match field {
            "region" => replay.region = "de-fra".into(),
            "project_id" => replay.project_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into(),
            "pipeline_id" => replay.pipeline_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into(),
            "wf_run_id" => replay.wf_run_id = "cccccccc-cccc-cccc-cccc-cccccccccccc".into(),
            "repo_ref" => replay.repo_ref = None,
            "commit_oid" => replay.commit_oid = None,
            "cause_event_id" => replay.cause_event_id = None,
            "definition_snapshot" => replay.definition_snapshot = "blake3:other".into(),
            "trigger_kind" => replay.trigger_kind = "manual".into(),
            "concurrency_group" => replay.concurrency_group = Some("pr:web:43".into()),
            "pr_head_generation" => replay.pr_head_generation = Some(8),
            "trust_tier" => replay.trust_tier = "self_hosted".into(),
            "correlation_id" => replay.correlation_id = "corr-other".into(),
            "triggered_by" => replay.triggered_by = None,
            _ => unreachable!(),
        }
        let error = store
            .insert_ci_run(&replay)
            .await
            .expect_err("immutable collision");
        if field == "region" {
            assert_eq!(error, CiRunStoreError::ConflictNotVisible);
        } else {
            assert_eq!(
                error,
                CiRunStoreError::ReplayCollision {
                    differing_fields: vec![field]
                },
                "typed collision identifies only the differing field"
            );
            let rendered = error.to_string();
            assert!(
                !rendered.contains("other"),
                "collision error carries no submitted values"
            );
        }
    }

    // Lifecycle advancement does not invalidate an exact reserve replay: mutable state, settlement,
    // and timestamps are intentionally outside the immutable identity comparison.
    let advanced_run = run_id(30);
    let advanced = row("tenantA", &advanced_run);
    assert!(store.insert_ci_run(&advanced).await.unwrap());
    sqlx::query(
        "UPDATE ci_run SET state='succeeded', cost_settled=true, finished_at=now(), \
         created_at=created_at - interval '1 minute' WHERE tenant_id=$1 AND run_id=$2::uuid",
    )
    .bind(&advanced.tenant_id)
    .bind(&advanced.run_id)
    .execute(&admin)
    .await
    .expect("advance lifecycle");
    assert!(
        !store
            .insert_ci_run(&advanced)
            .await
            .expect("advanced exact replay"),
        "an exact immutable replay remains exact after lifecycle advancement"
    );

    // Pseudonym erasure is the one sanctioned monotone change to an immutable identity edge.
    let erased_run = run_id(31);
    let erased = row("tenantA", &erased_run);
    assert!(store.insert_ci_run(&erased).await.unwrap());
    sqlx::query("UPDATE ci_run SET triggered_by=$1 WHERE tenant_id=$2 AND run_id=$3::uuid")
        .bind(ERASED_PSEUDONYM)
        .bind(&erased.tenant_id)
        .bind(&erased.run_id)
        .execute(&admin)
        .await
        .expect("pseudonym-shred actor edge");
    assert!(
        !store
            .insert_ci_run(&erased)
            .await
            .expect("erased actor replay"),
        "stored erased pseudonym accepts the pre-erasure exact replay"
    );

    // Invalid reserve state is rejected before a transaction begins. Use a malformed UUID too: if
    // SQL were touched first this would be Db, not the typed initial-state error.
    let mut invalid = row("tenant-invalid", "not-a-uuid");
    invalid.state = "running".into();
    assert_eq!(
        store.insert_ci_run(&invalid).await,
        Err(CiRunStoreError::InvalidInitialState)
    );
    let invalid_rows: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run WHERE tenant_id=$1")
            .bind("tenant-invalid")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(invalid_rows, 0, "invalid initial state writes no row");

    let malformed = row("tenant-malformed", "still-not-a-uuid");
    assert!(
        matches!(
            store.insert_ci_run(&malformed).await,
            Err(CiRunStoreError::Db(_))
        ),
        "malformed UUID remains a loud database error"
    );

    // Concurrent identical contenders: the loser waits for the winner's commit, then the mandatory
    // second READ COMMITTED statement verifies it exactly.
    let identical = row("tenantA", &run_id(40));
    let identical_peer = identical.clone();
    let (identical_a, identical_b) = tokio::join!(
        store.insert_ci_run(&identical),
        store.insert_ci_run(&identical_peer)
    );
    assert!(
        matches!(
            (identical_a, identical_b),
            (Ok(true), Ok(false)) | (Ok(false), Ok(true))
        ),
        "concurrent exact contenders produce one fresh insert and one verified replay"
    );

    // Concurrent divergent contenders: exactly one wins; the other receives the typed collision
    // after observing the committed winner in its second statement.
    let divergent_a = row("tenantA", &run_id(41));
    let mut divergent_b = divergent_a.clone();
    divergent_b.definition_snapshot = "blake3:concurrent-other".into();
    let (result_a, result_b) = tokio::join!(
        store.insert_ci_run(&divergent_a),
        store.insert_ci_run(&divergent_b)
    );
    let mut fresh = 0;
    let mut collisions = 0;
    for result in [result_a, result_b] {
        match result {
            Ok(true) => fresh += 1,
            Err(CiRunStoreError::ReplayCollision { differing_fields }) => {
                assert_eq!(differing_fields, vec!["definition_snapshot"]);
                collisions += 1;
            }
            other => panic!("unexpected concurrent divergent result: {other:?}"),
        }
    }
    assert_eq!((fresh, collisions), (1, 1));

    // The shared HandlerTx path has identical semantics. An exact replay can co-commit a dedup mark;
    // a divergent replay propagates its typed error so the caller can roll that mark back atomically.
    let co_run = run_id(50);
    let co_original = row("tenantA", &co_run);
    assert!(store.insert_ci_run(&co_original).await.unwrap());
    let rt = tokio::runtime::Handle::current();

    let mut exact_tx = app.begin().await.expect("begin exact co-commit");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id',$1,true), set_config('myelin.region',$2,true)",
    )
    .bind(&co_original.tenant_id)
    .bind(&co_original.region)
    .execute(&mut *exact_tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO consumer_dedup (consumer,event_id) VALUES ('ci-test','exact')")
        .execute(&mut *exact_tx)
        .await
        .unwrap();
    {
        let mut handler_tx = HandlerTx::with_connection(&mut *exact_tx);
        assert!(
            !store
                .co_commit_insert(&mut handler_tx, &co_original, &rt)
                .unwrap(),
            "HandlerTx exact replay is false"
        );
    }
    exact_tx.commit().await.unwrap();

    let mut co_divergent = co_original.clone();
    co_divergent.repo_ref = Some("collision-repo".into());
    let mut divergent_tx = app.begin().await.expect("begin divergent co-commit");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id',$1,true), set_config('myelin.region',$2,true)",
    )
    .bind(&co_original.tenant_id)
    .bind(&co_original.region)
    .execute(&mut *divergent_tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO consumer_dedup (consumer,event_id) VALUES ('ci-test','divergent')")
        .execute(&mut *divergent_tx)
        .await
        .unwrap();
    let co_error = {
        let mut handler_tx = HandlerTx::with_connection(&mut *divergent_tx);
        store
            .co_commit_insert(&mut handler_tx, &co_divergent, &rt)
            .expect_err("HandlerTx collision propagates")
    };
    assert_eq!(
        co_error,
        CiRunStoreError::ReplayCollision {
            differing_fields: vec!["repo_ref"]
        }
    );
    divergent_tx.rollback().await.unwrap();
    let dedup_rows: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM consumer_dedup WHERE consumer='ci-test' AND event_id='divergent'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        dedup_rows, 0,
        "collision rollback removes the co-commit dedup mark"
    );

    admin
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .ok();
    println!("[chunk4/store] PASS ci_run store: exact replay verification; typed immutable collisions; concurrent winner visibility; RLS conflict hiding; lifecycle/erasure monotonicity; HandlerTx dedup rollback.");
}
