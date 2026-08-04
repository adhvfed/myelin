#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::WF_SIGNAL_DDL;
use sqlx::Row;

async fn admin_pool() -> sqlx::PgPool {
    let cfg = MyelinConfig::dev();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin to dev Postgres (is the stack up?)")
}

struct CatchUnwind<F> {
    inner: std::pin::Pin<Box<F>>,
}

impl<F: std::future::Future> std::future::Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.inner.as_mut().poll(cx)))
        {
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(payload) => std::task::Poll::Ready(Err(payload)),
        }
    }
}

async fn with_tables_cleanup<Fut>(pool: &sqlx::PgPool, tables: &[&str], body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = CatchUnwind {
        inner: Box::pin(body()),
    }
    .await;
    for table in tables {
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
            .execute(pool)
            .await;
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

fn job_queue_ddl(tbl: &str) -> String {
    format!(
        "CREATE TABLE {tbl} (\
           tenant_id text NOT NULL, \
           region text NOT NULL, \
           run_id text NOT NULL, \
           job_id text NOT NULL, \
           labels text[] NOT NULL, \
           trust_tier text NOT NULL, \
           idem_token text NOT NULL, \
           lease_owner text, \
           lease_expires timestamptz, \
           PRIMARY KEY (tenant_id, job_id))"
    )
}

#[tokio::test]
async fn runner_lease_heartbeat_and_exactly_once_terminal_in_real_postgres() {
    let admin = admin_pool().await;
    let pid = std::process::id();
    let jq = format!("job_queue_lease_{pid}");
    let sig = format!("wf_signal_runner_{pid}");

    sqlx::query(&format!("DROP TABLE IF EXISTS {jq}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE IF EXISTS {sig}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&job_queue_ddl(&jq))
        .execute(&admin)
        .await
        .expect("the job_queue lease DDL applies");
    sqlx::query(&WF_SIGNAL_DDL.replacen("wf_signal", &sig, 1))
        .execute(&admin)
        .await
        .expect("the wf_signal DDL applies");

    let tables = [jq.as_str(), sig.as_str()];
    with_tables_cleanup(&admin, &tables, || async {
    sqlx::query(&format!(
        "INSERT INTO {jq} (tenant_id, region, run_id, job_id, labels, trust_tier, idem_token) \
         VALUES ('acme','fr-par','R-1','job-1', ARRAY['linux'], 'trusted', 'tok-1')"
    ))
    .execute(&admin)
    .await
    .unwrap();

    let mut tx1 = admin.begin().await.unwrap();
    let claimed = sqlx::query(&format!(
        "WITH eligible AS ( \
           SELECT job_id FROM {jq} \
           WHERE region = 'fr-par' \
             AND labels <@ ARRAY['linux'] \
             AND trust_tier = ANY(ARRAY['trusted']) \
             AND (lease_expires IS NULL OR lease_expires <= now()) \
           ORDER BY job_id \
           FOR UPDATE SKIP LOCKED LIMIT 1 \
         ) \
         UPDATE {jq} q SET lease_owner = 'worker-1', lease_expires = now() + interval '30 seconds' \
         FROM eligible e WHERE q.job_id = e.job_id \
         RETURNING q.job_id, q.idem_token, q.run_id"
    ))
    .fetch_optional(&mut *tx1)
    .await
    .expect("the FOR UPDATE SKIP LOCKED claim applies");
    let claimed = claimed.expect("worker-1 claims the eligible job");
    let job_id: String = claimed.get("job_id");
    let idem_token: String = claimed.get("idem_token");
    let run_id: String = claimed.get("run_id");
    assert_eq!(job_id, "job-1");
    assert_eq!(
        idem_token, "tok-1",
        "the spec's idem_token the runner will echo on job.done"
    );
    tx1.commit().await.unwrap();

    let none = sqlx::query(&format!(
        "WITH eligible AS ( \
           SELECT job_id FROM {jq} \
           WHERE region = 'fr-par' AND labels <@ ARRAY['linux'] \
             AND (lease_expires IS NULL OR lease_expires <= now()) \
           ORDER BY job_id FOR UPDATE SKIP LOCKED LIMIT 1 ) \
         UPDATE {jq} q SET lease_owner='worker-2', lease_expires = now() + interval '30 seconds' \
         FROM eligible e WHERE q.job_id = e.job_id RETURNING q.job_id"
    ))
    .fetch_optional(&admin)
    .await
    .unwrap();
    assert!(
        none.is_none(),
        "a live-leased job is not double-claimed (skip-locked / no double-run)"
    );

    let renewed = sqlx::query(&format!(
        "UPDATE {jq} SET lease_expires = now() + interval '30 seconds' \
         WHERE job_id = 'job-1' AND lease_owner = 'worker-1'"
    ))
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        renewed.rows_affected(),
        1,
        "the owner's heartbeat renews the lease"
    );

    let foreign = sqlx::query(&format!(
        "UPDATE {jq} SET lease_expires = now() + interval '30 seconds' \
         WHERE job_id = 'job-1' AND lease_owner = 'worker-2'"
    ))
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        foreign.rows_affected(),
        0,
        "a non-owner cannot heartbeat the lease"
    );

    sqlx::query(&format!(
        "UPDATE {jq} SET lease_expires = now() - interval '1 second' WHERE job_id = 'job-1'"
    ))
    .execute(&admin)
    .await
    .unwrap();
    let reclaimed = sqlx::query(&format!(
        "WITH eligible AS ( \
           SELECT job_id FROM {jq} \
           WHERE region='fr-par' AND labels <@ ARRAY['linux'] \
             AND (lease_expires IS NULL OR lease_expires <= now()) \
           ORDER BY job_id FOR UPDATE SKIP LOCKED LIMIT 1 ) \
         UPDATE {jq} q SET lease_owner='worker-2', lease_expires = now() + interval '30 seconds' \
         FROM eligible e WHERE q.job_id = e.job_id RETURNING q.job_id, q.lease_owner"
    ))
    .fetch_optional(&admin)
    .await
    .unwrap()
    .expect("worker-2 reclaims the EXPIRED lease");
    let owner: String = reclaimed.get("lease_owner");
    assert_eq!(
        owner, "worker-2",
        "an expired lease is reclaimable (the reaper / reclaim seam)"
    );

    let deliver = |marker: &str| {
        let tbl = sig.clone();
        let pool = admin.clone();
        let rid = run_id.clone();
        let tok = idem_token.clone();
        let m = marker.to_string();
        async move {
            sqlx::query(&format!(
                "INSERT INTO {tbl} (tenant_id, region, run_id, signal_name, idem_key, payload, payload_key_ref, consumed_seq) \
                 VALUES ('acme','fr-par',$1,'job.done',$2, jsonb_build_array('myelin://acme/ci/run/'||$1||'/{m}'), NULL, NULL) \
                 ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING RETURNING run_id"
            ))
            .bind(&rid)
            .bind(&tok)
            .fetch_optional(&pool)
            .await
            .expect("the job.done ON CONFLICT DO NOTHING delivery applies")
        }
    };

    let first = deliver("first").await;
    assert!(
        first.is_some(),
        "the FIRST job.done delivery buffered (the workflow wakes)"
    );
    let second = deliver("second").await;
    assert!(
        second.is_none(),
        "the SECOND job.done is a no-op (ON CONFLICT DO NOTHING - wake once)"
    );

    let count: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS c FROM {sig} WHERE tenant_id='acme' AND run_id=$1 AND signal_name='job.done'"
    ))
    .bind(&run_id)
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("c");
    assert_eq!(
        count, 1,
        "double-effect = 0: a doubly-delivered job.done buffers EXACTLY ONCE"
    );

    let payload: serde_json::Value = sqlx::query(&format!(
        "SELECT payload FROM {sig} WHERE tenant_id='acme' AND run_id=$1 AND signal_name='job.done' AND idem_key=$2"
    ))
    .bind(&run_id)
    .bind(&idem_token)
    .fetch_one(&admin)
    .await
    .unwrap()
    .get("payload");
    assert_eq!(
        payload,
        serde_json::json!(["myelin://acme/ci/run/R-1/first"]),
        "the buffered job.done payload is references-not-payloads (the FIRST delivery's; DO NOTHING never overwrote)"
    );

    println!(
        "[2026-06-21] PASS  drill=CI-P3(live-PG)  claim(FOR UPDATE SKIP LOCKED)=worker-1 second-claim=skipped  \
         heartbeat-renew=owner-only(1) foreign=0  expired->reclaim=worker-2  \
         terminal job.done double-deliver->buffer once rows=1 redelivery=no-op (double-effect=0)  \
         (real Postgres job_queue lease + engine wf_signal ON CONFLICT DO NOTHING - runner reuses the one signal path)"
    );
    })
    .await;
}
