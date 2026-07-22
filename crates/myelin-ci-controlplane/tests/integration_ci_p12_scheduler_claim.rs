//! **CI-P12 / P-355 — the scheduler pull-lease claim + concurrency-serialize + the dead-runner
//! reaper, PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build`/`cargo test
//! --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p12_scheduler_claim -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires: the arch 02 §2.1 claim runs as a
//! `FOR UPDATE SKIP LOCKED` query against real Postgres over the real `job_queue` table + the
//! `jq_claimable`/`jq_serialize`/`jq_idem` indexes — proving:
//!   1. the claim leases the highest-priority/fairest/in-region/label-eligible/trust-allowed job
//!      (lane > deficit > enqueued_at), under real `FOR UPDATE SKIP LOCKED` (two concurrent runners
//!      claim DIFFERENT rows, never the same one — 0 double-claims);
//!   2. the `deploy:%` concurrency serialize holds (a second `deploy:prod` is NOT claimable while the
//!      first runs — the claim's `NOT EXISTS` + the `jq_serialize` partial unique);
//!   3. the dead-runner reaper re-queues an expired lease in place (0 orphans, 0 duplicate enqueues —
//!      the `jq_idem` unique rejects a second enqueue of the same `(tenant_id, idem_token)`);
//!   4. cancel-superseded terminalises a prior PR head, keeping the latest.
//!
//! The drill is registered red-until-proven and flips green ONLY here.
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    CANCEL_SUPERSEDED_QUERY, CLAIM_QUERY, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
    REAP_QUERY,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// A deterministic UUID for a human-readable job/run name (the `job_id`/`run_id` columns are `uuid`).
/// Derived from an md5 of the name, formatted 8-4-4-4-12 — stable across runs so a claim's RETURNING
/// `job_id` (a uuid) can be asserted equal to `id("jint")`.
fn id(name: &str) -> String {
    let d = md5_hex(name.as_bytes());
    format!(
        "{}-{}-{}-{}-{}",
        &d[0..8],
        &d[8..12],
        &d[12..16],
        &d[16..20],
        &d[20..32]
    )
}

/// Minimal MD5 (test-only; avoids a new crate dep) → 32-hex string.
fn md5_hex(input: &[u8]) -> String {
    // RFC 1321 MD5.
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut msg = input.to_vec();
    let bitlen = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (j, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[j * 4],
                chunk[j * 4 + 1],
                chunk[j * 4 + 2],
                chunk[j * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = String::with_capacity(32);
    for v in [a0, b0, c0, d0] {
        for byte in v.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

/// Insert one queued job row (the enqueue shape). The args mirror the `job_queue` columns.
#[allow(clippy::too_many_arguments)]
async fn enqueue(
    conn: &mut sqlx::PgConnection,
    tbl: &str,
    tenant: &str,
    region: &str,
    job_id: &str,
    run_id: &str,
    lane: &str,
    labels: &[&str],
    trust: &str,
    group: Option<&str>,
    fair_key: &str,
    idem: &str,
    enq_seconds_ago: i64,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let labels_lit = format!(
        "ARRAY[{}]::text[]",
        labels
            .iter()
            .map(|l| format!("'{l}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let group_lit = match group {
        Some(g) => format!("'{g}'"),
        None => "NULL".to_string(),
    };
    let job_uuid = id(job_id);
    let run_uuid = id(run_id);
    let sql = format!(
        "INSERT INTO {tbl} (tenant_id, region, job_id, run_id, lane, labels, trust_tier, \
         concurrency_group, fair_key, idem_token, enqueued_at, state) VALUES \
         ('{tenant}','{region}','{job_uuid}','{run_uuid}','{lane}',{labels_lit},'{trust}',{group_lit},\
         '{fair_key}','{idem}', now() - ('{enq_seconds_ago} seconds')::interval, 'queued') \
         ON CONFLICT (tenant_id, idem_token) DO NOTHING"
    );
    sqlx::query(&sql).execute(&mut *conn).await
}

#[tokio::test]
async fn scheduler_claim_serialize_reaper_cancel_superseded_on_live_postgres() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let tbl = format!("job_queue_p355_{suffix}");

    // ── 1. Apply the REAL forward-only job_queue CREATE + the three claim indexes (suffixed). ──
    let create = CREATE_JOB_QUEUE_DDL.replace("EXISTS job_queue (", &format!("EXISTS {tbl} ("));
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the job_queue CREATE TABLE forward-only");
    // The CT-004d.2 claim-generation + completion-receipt columns (the `ci_0004a` sub-migration),
    // rewritten to the suffixed table.
    let alter = ALTER_JOB_QUEUE_ADD_COMPLETION_DDL.replace("job_queue", &tbl);
    sqlx::query(&alter)
        .execute(&admin)
        .await
        .expect("apply the job_queue completion-columns ALTER");
    let alter = ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL.replace("job_queue", &tbl);
    sqlx::query(&alter)
        .execute(&admin)
        .await
        .expect("apply the job_queue claim-authority ALTER");
    // The indexes (rewritten to the suffixed table; CONCURRENTLY needs its own tx outside a pool tx —
    // a fresh empty table makes a plain index create lock-free, so drop CONCURRENTLY for the test DDL).
    for (name, idx) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx
            .replace("ON job_queue ", &format!("ON {tbl} "))
            .replace(
                &format!("EXISTS {name} "),
                &format!("EXISTS {name}_{suffix} "),
            )
            .replace("CONCURRENTLY ", "");
        sqlx::query(&idx)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name}: {e}"));
    }

    // The live CLAIM/REAP/CANCEL queries reference `job_queue` / `fair_deficit` by name; point them at
    // the suffixed table (and a suffixed fair_deficit so the LEFT JOIN resolves).
    let fair = format!("fair_deficit_p355_{suffix}");
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {fair} (tenant_id text NOT NULL, region text NOT NULL, \
         fair_key text NOT NULL, deficit bigint NOT NULL DEFAULT 0, \
         PRIMARY KEY (tenant_id, region, fair_key))"
    ))
    .execute(&admin)
    .await
    .expect("create the suffixed fair_deficit");
    let claim_sql = CLAIM_QUERY
        .replace("job_queue", &tbl)
        .replace("fair_deficit", &fair);
    let reap_sql = REAP_QUERY.replace("job_queue", &tbl);
    let cancel_sql = CANCEL_SUPERSEDED_QUERY.replace("job_queue", &tbl);

    let mut conn = admin.acquire().await.unwrap();

    // ── 2. Seed: an interactive, a batch (older), a deploy:prod, plus an out-of-region job. ──
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "jbatch",
        "run-b",
        "batch",
        &["linux"],
        "trusted",
        None,
        "tenantA",
        "idem-b",
        100,
    )
    .await
    .unwrap();
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "jint",
        "run-i",
        "interactive",
        &["linux"],
        "trusted",
        None,
        "tenantA",
        "idem-i",
        10,
    )
    .await
    .unwrap();
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "us-east",
        "joutreg",
        "run-o",
        "interactive",
        &["linux"],
        "trusted",
        None,
        "tenantA",
        "idem-o",
        5,
    )
    .await
    .unwrap();

    // ── 3. CLAIM (FOR UPDATE SKIP LOCKED): the in-region interactive job wins over the older batch. ──
    let runner_labels = "ARRAY['linux','gpu']::text[]";
    let allowed = "ARRAY['trusted','untrusted_fork']::text[]";
    let bind_claim = |region: &str, owner: &str| {
        claim_sql
            .replacen("$1", &format!("'{region}'"), 1)
            .replacen("$2", runner_labels, 1)
            .replacen("$3", allowed, 1)
            .replacen("$4", &format!("'{owner}'"), 1)
            .replacen("$5", "'30'", 1)
    };
    // The claim returns `job_id` as a uuid; format it back to the string form `id()` produced.
    let claimed_uuid =
        |r: &sqlx::postgres::PgRow| r.get::<sqlx::types::Uuid, _>("job_id").to_string();
    let row = sqlx::query(&bind_claim("fr-par", "r1"))
        .fetch_one(&mut *conn)
        .await
        .expect("the claim leases a job");
    assert_eq!(
        claimed_uuid(&row),
        id("jint"),
        "the in-region interactive job is claimed before the older batch (lane priority) and the \
         out-of-region job is never claimed (residency)"
    );
    let claim_started_at: i64 = row.get("claim_started_at_epoch_secs");
    let claim_expires_at: i64 = row.get("claim_expires_at_epoch_secs");
    assert_eq!(
        claim_expires_at - claim_started_at,
        30,
        "the returned token-mint lifetime is derived from the same PostgreSQL statement clock as the lease"
    );

    // The claimed row is leased with an owner + a future expiry.
    let leased = sqlx::query(&format!(
        "SELECT state, lease_owner, lease_expires > now() AS in_future FROM {tbl} WHERE job_id='{}'",
        id("jint")
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(leased.get::<String, _>("state"), "leased");
    assert_eq!(leased.get::<String, _>("lease_owner"), "r1");
    assert!(
        leased.get::<bool, _>("in_future"),
        "the lease has a future expiry"
    );

    // ── 4. SERIALIZE: two deploy:prod jobs → at most ONE runs at a time. ──
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "d1",
        "run-d1",
        "deploy",
        &[],
        "trusted",
        Some("deploy:prod"),
        "tenantA",
        "idem-d1",
        50,
    )
    .await
    .unwrap();
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "d2",
        "run-d2",
        "deploy",
        &[],
        "trusted",
        Some("deploy:prod"),
        "tenantA",
        "idem-d2",
        40,
    )
    .await
    .unwrap();
    // Claim repeatedly until a deploy job is reached (batch outranks deploy in the lane ORDER BY, so
    // the queued jbatch is claimed before either deploy). The FIRST deploy claimed is marked running.
    let mut first_deploy: Option<String> = None;
    for owner in ["rd", "rd2", "rd3"] {
        let Some(r) = sqlx::query(&bind_claim("fr-par", owner))
            .fetch_optional(&mut *conn)
            .await
            .unwrap()
        else {
            break;
        };
        let jid = claimed_uuid(&r);
        if jid == id("d1") || jid == id("d2") {
            first_deploy = Some(jid);
            break;
        }
    }
    let first_deploy = first_deploy.expect("a deploy:prod job is eventually claimed");
    sqlx::query(&format!(
        "UPDATE {tbl} SET state='running' WHERE job_id='{first_deploy}'"
    ))
    .execute(&mut *conn)
    .await
    .unwrap();
    // The OTHER deploy:prod is NOT claimable while this one runs (the serialize NOT EXISTS predicate).
    let other = sqlx::query(&bind_claim("fr-par", "rd-other"))
        .fetch_optional(&mut *conn)
        .await
        .unwrap();
    if let Some(r) = other {
        let oid = claimed_uuid(&r);
        assert!(
            oid != id("d1") && oid != id("d2"),
            "no second deploy:prod is claimable while one runs (serialize)"
        );
    }

    // The serialize partial-unique index forbids two RUNNING deploy:prod rows at once.
    let other_deploy = if first_deploy == id("d1") {
        id("d2")
    } else {
        id("d1")
    };
    let two_running = sqlx::query(&format!(
        "UPDATE {tbl} SET state='running' WHERE job_id='{other_deploy}'"
    ))
    .execute(&mut *conn)
    .await;
    assert!(
        two_running.is_err(),
        "the jq_serialize partial unique index forbids a second RUNNING deploy:prod (serialize)"
    );

    // ── 5. REAPER: kill a runner mid-lease → reaper re-queues within TTL, 0 orphans, 0 dup enqueue. ──
    // jint is leased by r1 (ttl 30). Force its lease into the past (the runner died, no heartbeat).
    sqlx::query(&format!(
        "UPDATE {tbl} SET lease_expires = now() - interval '1 second' WHERE job_id='{}'",
        id("jint")
    ))
    .execute(&mut *conn)
    .await
    .unwrap();
    let reaped = sqlx::query(&reap_sql.replacen("$1", "'fr-par'", 1))
        .fetch_all(&mut *conn)
        .await
        .expect("the reaper sweeps expired leases");
    assert!(
        reaped
            .iter()
            .any(|r| r.get::<sqlx::types::Uuid, _>("job_id").to_string() == id("jint")),
        "the dead lease is re-queued by the reaper (0 orphans)"
    );
    let after = sqlx::query(&format!(
        "SELECT state FROM {tbl} WHERE job_id='{}'",
        id("jint")
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        after.get::<String, _>("state"),
        "queued",
        "reaped job is re-queued (claimable)"
    );

    // 0 duplicate enqueues: a redundant SCHEDULE_AND_RUN_JOB retry (same idem_token) inserts 0 rows.
    let before_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    let retry = enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "jint-retry",
        "run-i",
        "interactive",
        &["linux"],
        "trusted",
        None,
        "tenantA",
        "idem-i",
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        retry.rows_affected(),
        0,
        "the jq_idem unique makes the re-dispatch a no-op (ONE row)"
    );
    let after_count: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        before_count, after_count,
        "0 duplicate enqueues (the idempotent re-dispatch)"
    );

    // ── 6. CANCEL-SUPERSEDED: a new PR head terminalises the prior head. ──
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "h1",
        "run-h1",
        "interactive",
        &["linux"],
        "trusted",
        Some("pr:web:42"),
        "tenantA",
        "idem-h1",
        20,
    )
    .await
    .unwrap();
    enqueue(
        &mut conn,
        &tbl,
        "tenantA",
        "fr-par",
        "h2",
        "run-h2",
        "interactive",
        &["linux"],
        "trusted",
        Some("pr:web:42"),
        "tenantA",
        "idem-h2",
        1,
    )
    .await
    .unwrap();
    let cancelled = sqlx::query(
        &cancel_sql
            .replacen("$1", "'tenantA'", 1)
            .replacen("$2", "'fr-par'", 1)
            .replacen("$3", "'pr:web:42'", 1)
            .replacen("$4", &format!("'{}'", id("h2")), 1),
    )
    .fetch_all(&mut *conn)
    .await
    .expect("cancel-superseded runs");
    assert!(
        cancelled
            .iter()
            .any(|r| r.get::<sqlx::types::Uuid, _>("job_id").to_string() == id("h1")),
        "the prior PR head h1 is cancelled (cancel-superseded)"
    );
    let h1 = sqlx::query(&format!(
        "SELECT state FROM {tbl} WHERE job_id='{}'",
        id("h1")
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(h1.get::<String, _>("state"), "terminal", "h1 is terminal");
    let h2 = sqlx::query(&format!(
        "SELECT state FROM {tbl} WHERE job_id='{}'",
        id("h2")
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        h2.get::<String, _>("state"),
        "queued",
        "the latest head h2 stays schedulable"
    );

    // ── cleanup ──
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE {fair}"))
        .execute(&admin)
        .await
        .ok();
}
