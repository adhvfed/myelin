//! # MR-009 — the kill-9 / restart durability VERIFY (the capstone of the persistence wave).
//!
//! The master-plan done-bar: **load-bearing state survives `kill -9` + restart, and is consistent
//! across instances.** This is the STRONGER property the per-store integration tests
//! (MR-007/008/023/024/025) could not prove: each of those simulated a restart with a fresh STORE
//! INSTANCE in the SAME process (the census's exact complaint about the old drills —
//! `recover_from_mirror()` / a same-process map copy). Here the writer is a REAL `std::process`
//! child that the kernel **SIGKILLs** mid-life, and a FRESH OS process reads the state back from the
//! live backends. No graceful shutdown, no flush-on-exit, no shared memory — genuine crash durability.
//!
//! ## How the proof is constructed (per store family)
//!   1. The parent runs the migrations (admin role) so the tables exist.
//!   2. The parent spawns `mr009_kill9_writer write <family> <run-id>` (a REAL child, located via
//!      `CARGO_BIN_EXE_mr009_kill9_writer`). The child writes load-bearing state through the REAL
//!      durable composition root, COMMITS, prints `MR009-READY`, then **blocks forever**.
//!   3. The parent **`kill -9`s** the child (`Child::kill()` = SIGKILL on Unix) and asserts the
//!      child died **by signal 9** (`ExitStatus::signal() == Some(9)`) — proving it was a genuine
//!      crash, NOT a clean exit (a clean exit would be `code()==Some(0)`, `signal()==None`).
//!   4. The parent spawns `mr009_kill9_writer read <family> <run-id>` — a FRESH OS process over the
//!      SAME live backends — and asserts the state read back is intact + correct.
//!
//! The 3-instance consistency check spawns THREE independent read processes over the same backends
//! and asserts they see the identical written state (no split-brain on the durable stores).
//!
//! Run against the dev stack (the make-it-real env):
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   MYELIN_KMS_SEAL_KEY=00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff \
//!     cargo test -p myelin-identity-service --features integration \
//!       --test integration_mr009_kill9_durability -- --nocapture --test-threads=1
//!
//! Skips gracefully if the backends are unreachable (the child prints `MR009-SKIP`).
#![cfg(feature = "integration")]

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use myelin_config::MyelinConfig;
use myelin_storage::kms_durable::kms_durable_migrations;
use myelin_storage::migration::HotTables;
use myelin_storage::placement_durable::placement_durable_migrations;
use myelin_storage::{identity_durable_migrations, SubstrateProvider};

/// The same default seal key the child falls back to (the parent always passes the resolved value
/// down to both children so the writer + reader agree — the cross-restart KMS proof).
const DEFAULT_SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

const WRITER_BIN: &str = env!("CARGO_BIN_EXE_mr009_kill9_writer");

fn admin_config() -> MyelinConfig {
    let mut c = MyelinConfig::dev();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn seal_hex() -> String {
    std::env::var("MYELIN_KMS_SEAL_KEY").unwrap_or_else(|_| DEFAULT_SEAL_HEX.to_string())
}

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Apply every durable migration (admin role). Returns `false` (SKIP) if PG is unreachable.
fn ensure_migrated() -> bool {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = match SubstrateProvider::connect(admin_config(), 4).await {
            Ok(p) => p,
            Err(_) => {
                eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
                return false;
            }
        };
        admin
            .migrate(&identity_durable_migrations(), &HotTables::none())
            .await
            .expect("identity+revocation durable migrations");
        admin
            .migrate_foundation()
            .await
            .expect("outbox + consumer_dedup foundation migrations");
        admin
            .migrate(&placement_durable_migrations(), &HotTables::none())
            .await
            .expect("placement durable migrations");
        admin
            .migrate(&kms_durable_migrations(), &HotTables::none())
            .await
            .expect("kms durable migrations");
        true
    })
}

/// Build a child `Command` for the writer bin in the given mode, passing the resolved seal key down.
fn child_cmd(mode: &str, family: &str, run: &str, handoff: &str) -> Command {
    let mut cmd = Command::new(WRITER_BIN);
    cmd.args([mode, family, run, handoff])
        .env("MYELIN_KMS_SEAL_KEY", seal_hex())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd
}

/// The single machine line a child emits: the `MR009-<TAG> <rest>` payload.
enum Line {
    Ready(String),
    Read(String),
    Skip(String),
}

fn parse_line(line: &str) -> Option<Line> {
    let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
    match tag {
        "MR009-READY" => Some(Line::Ready(rest.to_string())),
        "MR009-READ" => Some(Line::Read(rest.to_string())),
        "MR009-SKIP" => Some(Line::Skip(rest.to_string())),
        _ => None,
    }
}

/// Spawn a WRITE child, read its first `MR009-*` line (with a timeout), and KEEP the child alive
/// (it is blocking, waiting to be SIGKILLed). Returns `(child, ready_json)` or `None` to SKIP.
fn spawn_writer_until_ready(family: &str, run: &str) -> Option<(Child, String)> {
    let mut child = child_cmd("write", family, run, "{}")
        .spawn()
        .expect("spawn writer child");
    let stdout = child.stdout.take().expect("piped stdout");

    let (tx, rx) = mpsc::channel::<Option<Line>>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(parsed) = parse_line(&line) {
                let _ = tx.send(Some(parsed));
                return;
            }
        }
        let _ = tx.send(None);
    });

    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Some(Line::Ready(json))) => Some((child, json)),
        Ok(Some(Line::Skip(why))) => {
            eprintln!("SKIP [{family}]: writer skipped: {why}");
            let _ = child.kill();
            let _ = child.wait();
            None
        }
        Ok(Some(Line::Read(_))) | Ok(None) | Err(_) => {
            eprintln!("SKIP [{family}]: writer produced no READY line in time");
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// SIGKILL a blocked writer child and ASSERT it died by signal 9 (a genuine crash, not a clean exit).
fn sigkill_and_assert_crash(family: &str, mut child: Child) {
    child.kill().expect("send SIGKILL to the writer child");
    let status = child.wait().expect("reap the SIGKILLed child");
    assert_eq!(
        status.signal(),
        Some(9),
        "[{family}] the writer MUST have died by SIGKILL (signal 9), not a clean exit — \
         got status {status:?} (code={:?}, signal={:?})",
        status.code(),
        status.signal()
    );
    assert!(
        status.code().is_none(),
        "[{family}] a SIGKILLed process has NO clean exit code (it never ran a shutdown path)"
    );
    println!(
        "[MR-009] {family}: writer child SIGKILLed — confirmed died by signal 9 (no clean exit)."
    );
}

/// Run a READ child (a FRESH OS process), returning its `MR009-READ` JSON. `None` to SKIP.
fn read_back(family: &str, run: &str, handoff: &str) -> Option<serde_json::Value> {
    let out = child_cmd("read", family, run, handoff)
        .output()
        .expect("run read child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        match parse_line(line) {
            Some(Line::Read(json)) => {
                return Some(serde_json::from_str(&json).expect("parse READ json"));
            }
            Some(Line::Skip(why)) => {
                eprintln!("SKIP [{family}]: reader skipped: {why}");
                return None;
            }
            _ => {}
        }
    }
    panic!("[{family}] the read child produced no MR009-READ line; stdout=\n{stdout}");
}

// =================================================================================================
// The five store families — each: write in a child → SIGKILL → fresh process reads it back intact.
// =================================================================================================

#[test]
fn kill9_identity_principal_tuple_and_profile_decrypt_across_restart() {
    if !ensure_migrated() {
        return;
    }
    let run = run_id();
    let Some((child, _ready)) = spawn_writer_until_ready("identity", &run) else {
        return;
    };
    sigkill_and_assert_crash("identity", child);

    let Some(read) = read_back("identity", &run, "{}") else {
        return;
    };
    assert_eq!(
        read["principal_kind"], "Human",
        "principal kind durable: {read}"
    );
    assert_eq!(read["status"], "Active", "principal status durable: {read}");
    assert_eq!(
        read["profile_email"],
        format!("alice-{run}@mr009.test"),
        "the KMS-sealed profile email DECRYPTED after a kill-9 restart (durable root+KEK+DEK): {read}"
    );
    assert_eq!(read["profile_name"], format!("Alice {run}"));
    assert_eq!(
        read["tuple_present"], true,
        "the ReBAC tuple survived kill-9: {read}"
    );
    // BUS-2 exact (MR-009b W3b.3): the iam.tuple_written event co-committed into the SAME
    // rebac_tuple transaction as the tuple write, so it SURVIVED the kill-9 alongside the tuple —
    // both the tuple AND its event are present (0 lost). Because they co-commit atomically, there
    // can be no event without its committed tuple (0 ghost): the crash left either both or neither.
    assert_eq!(
        count_outbox_for_identity_tuple(&run),
        1,
        "the iam.tuple_written event co-committed with the tuple + survived kill-9 \
         (both exist — 0 ghost / 0 lost, the kill-9 shape)"
    );
    cleanup_identity(&run);
    println!(
        "[MR-009] PASS  family=IDENTITY  principal+tuple durable across kill-9; \
         iam.tuple_written co-committed + survived (0 ghost / 0 lost); \
         profile DECRYPTS post-restart under the durable KMS root (MR-025)."
    );
}

#[test]
fn kill9_revocation_jti_and_run_token_ttl_survive() {
    if !ensure_migrated() {
        return;
    }
    let run = run_id();
    let Some((child, _ready)) = spawn_writer_until_ready("revocation", &run) else {
        return;
    };
    sigkill_and_assert_crash("revocation", child);

    let Some(read) = read_back("revocation", &run, "{}") else {
        return;
    };
    assert_eq!(
        read["jti_revoked"], true,
        "the revoked jti reads revoked after kill-9: {read}"
    );
    assert_eq!(
        read["run_state_before"], "LiveWithinRunLife",
        "the run-token TTL survived kill-9 (Live within run-life): {read}"
    );
    assert_eq!(
        read["run_state_after"], "Expired",
        "expiry honored across kill-9 (Expired past its TTL): {read}"
    );
    cleanup_revocation(&run);
    println!(
        "[MR-009] PASS  family=REVOCATION  revoked jti + run-token TTL/expiry survived kill-9."
    );
}

#[test]
fn kill9_events_outbox_rows_survive_and_restart_relay_drains_zero_lost() {
    if !ensure_migrated() {
        return;
    }
    let run = run_id();
    let Some((child, ready)) = spawn_writer_until_ready("events", &run) else {
        return;
    };
    // The W3b.6 kill-9 EMIT drill shape: the writer emitted through the PRODUCTION path
    // (`OutboxStore::durable(PgOutboxBacking)` + `OutboxTx::emit` → `commit`, UlidMinter ids) —
    // 8 rows COMMITTED on the live aggregate, and 4 more STAGED on the ghost aggregate in a
    // transaction that is deliberately NEVER committed (held open until the SIGKILL).
    let ready: serde_json::Value = serde_json::from_str(&ready).expect("parse READY json");
    assert_eq!(ready["committed"], 8, "writer committed 8 rows: {ready}");
    assert_eq!(
        ready["ghost_staged"], 4,
        "writer staged 4 uncommitted ghost rows before blocking: {ready}"
    );
    sigkill_and_assert_crash("events", child);

    // SURVIVED: the committed-but-unsent outbox rows are still there (the writer was SIGKILLed
    // mid-life; the rows committed BEFORE the crash). The outbox is cross-tenant infra with no
    // tenant column (contract 2.3), so this aggregate-scoped count lives in the test (the same
    // posture as the mr023 test), never in production src.
    let survived = count_unsent_for_aggregate(&run);
    assert_eq!(
        survived, 8,
        "all 8 committed-but-unsent outbox rows SURVIVED kill-9 (0 lost on the crash)"
    );

    // 0 GHOST (MR-009b W3b.6, emit-iff-committed on the DURABLE arm): the rows that were STAGED
    // in the open, never-committed transaction at the moment of the SIGKILL must be ABSENT from
    // PG — an emit becomes durable IFF its transaction committed; a crash between staging and
    // commit writes NOTHING (BUS-D4, the structural half, now proven across a real process death).
    let ghosts = count_rows_for_ghost_aggregate(&run);
    assert_eq!(
        ghosts, 0,
        "rows staged-but-uncommitted at the kill must be ABSENT after the crash (0 ghost)"
    );

    // The RESTART relay (a FRESH process) re-claims + drains the survived rows to the real broker.
    let Some(read) = read_back("events", &run, "{}") else {
        return;
    };
    assert!(
        read["published"].as_i64().unwrap_or(0) >= 8,
        "the restart relay re-published the survived rows: {read}"
    );

    // 0 LOST: after the drain none of our committed rows remain unsent.
    let remaining = count_unsent_for_aggregate(&run);
    assert_eq!(
        remaining, 0,
        "the restart relay drained every survived row (0 lost across the kill-9 + restart)"
    );
    // Belt-and-braces: the drain cannot conjure the ghost rows either (still 0 post-restart).
    assert_eq!(
        count_rows_for_ghost_aggregate(&run),
        0,
        "the restart relay drained only COMMITTED rows — the ghost aggregate stays empty"
    );
    cleanup_events(&run);
    println!(
        "[MR-009] PASS  family=EVENTS  8 committed outbox rows (emitted through the production \
         OutboxStore::durable path) survived kill-9; restart relay drained them → 0 lost; 4 \
         staged-uncommitted rows absent → 0 ghost (MR-009b W3b.6 emit drill)."
    );
}

#[test]
fn kill9_control_plane_placement_survives() {
    if !ensure_migrated() {
        return;
    }
    let run = run_id();
    let Some((child, _ready)) = spawn_writer_until_ready("placement", &run) else {
        return;
    };
    sigkill_and_assert_crash("placement", child);

    let Some(read) = read_back("placement", &run, "{}") else {
        return;
    };
    assert_eq!(
        read["home_cell"],
        format!("mr009cell-{run}"),
        "the tenant→cell placement survived kill-9: {read}"
    );
    assert_eq!(read["region"], "eu-west");
    assert_eq!(read["status"], "Active");
    cleanup_placement(&run);
    println!("[MR-009] PASS  family=CONTROL-PLANE  tenant→cell placement survived kill-9.");
}

#[test]
fn kill9_kms_root_kek_dek_survive_and_data_decrypts_post_restart() {
    if !ensure_migrated() {
        return;
    }
    let run = run_id();
    let Some((child, ready)) = spawn_writer_until_ready("kms", &run) else {
        return;
    };
    // `ready` is the handoff JSON: the (nonce, ciphertext) the writer sealed pre-kill.
    sigkill_and_assert_crash("kms", child);

    let Some(read) = read_back("kms", &run, &ready) else {
        return;
    };
    assert_eq!(
        read["decrypted"],
        format!("mr009 kms secret {run}"),
        "data sealed PRE-kill DECRYPTS post-restart — the sealed root + KEK + DEK survived kill-9: {read}"
    );
    cleanup_kms(&run);
    println!(
        "[MR-009] PASS  family=KMS  sealed root + KEK + DEK survived kill-9; \
         pre-kill ciphertext decrypts post-restart."
    );
}

// =================================================================================================
// 3-instance consistency — three independent OS processes over the same backends agree (no split-brain).
// =================================================================================================

#[test]
fn three_instance_consistency_no_split_brain() {
    if !ensure_migrated() {
        return;
    }
    let run = run_id();
    // Write the identity state once, then kill the writer (so every reader sees PERSISTED state).
    let Some((child, _ready)) = spawn_writer_until_ready("identity", &run) else {
        return;
    };
    sigkill_and_assert_crash("identity", child);

    // THREE independent read PROCESSES (genuine instances) over the same live backends.
    let mut views: Vec<serde_json::Value> = Vec::new();
    for i in 0..3 {
        match read_back("identity", &run, "{}") {
            Some(v) => views.push(v),
            None => {
                eprintln!("SKIP: instance {i} could not read");
                cleanup_identity(&run);
                return;
            }
        }
    }
    assert_eq!(views.len(), 3, "spawned 3 reader instances");
    // All three instances see the IDENTICAL written state (no split-brain).
    assert_eq!(views[0], views[1], "instance 0 and 1 disagree: {views:?}");
    assert_eq!(views[1], views[2], "instance 1 and 2 disagree: {views:?}");
    assert_eq!(
        views[0]["profile_email"],
        format!("alice-{run}@mr009.test"),
        "all instances decrypt the same profile under the shared durable KMS root: {views:?}"
    );
    assert_eq!(views[0]["tuple_present"], true);
    cleanup_identity(&run);
    println!(
        "[MR-009] PASS  3-INSTANCE CONSISTENCY  three independent OS processes over the same \
         backends saw the identical written state (no split-brain on the durable stores)."
    );
}

// =================================================================================================
// Cleanup (admin role — best-effort; idempotent IF NOT EXISTS migrations make re-runs safe anyway).
// =================================================================================================

fn with_admin_pool<F>(f: F)
where
    F: for<'a> FnOnce(
        &'a sqlx::PgPool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>,
{
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let Ok(admin) = SubstrateProvider::connect(admin_config(), 2).await else {
            return;
        };
        f(admin.db_pool()).await;
    });
}

fn cleanup_identity(run: &str) {
    let tenant = format!("mr009id-{run}");
    let cell = format!("mr009idkms-{run}");
    // The iam.tuple_written row the durable write co-committed (aggregate-scoped; the outbox has no
    // tenant column, contract 2.3).
    let tuple_aggregate = format!("iam:tuple:mr009id-{run}:repo:core");
    with_admin_pool(|pool| {
        Box::pin(async move {
            for sql in [
                "DELETE FROM rebac_tuple WHERE tenant_id = $1",
                "DELETE FROM principal WHERE tenant_id = $1",
                "DELETE FROM credential_link WHERE tenant_id = $1",
            ] {
                let _ = sqlx::query(sql).bind(&tenant).execute(pool).await;
            }
            let _ = sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
                .bind(&tuple_aggregate)
                .execute(pool)
                .await;
            for table in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
                let _ = sqlx::query(&format!("DELETE FROM {table} WHERE cell_id = $1"))
                    .bind(&cell)
                    .execute(pool)
                    .await;
            }
        })
    });
}

fn cleanup_revocation(run: &str) {
    let tenant = format!("mr009rev-{run}");
    with_admin_pool(|pool| {
        Box::pin(async move {
            for sql in [
                "DELETE FROM revocation WHERE tenant_id = $1",
                "DELETE FROM run_token_teardown WHERE tenant_id = $1",
            ] {
                let _ = sqlx::query(sql).bind(&tenant).execute(pool).await;
            }
        })
    });
}

/// Count this run's iam.tuple_written outbox rows for the identity tuple aggregate — the event the
/// durable S3 write co-commits into the SAME rebac_tuple tx (MR-009b W3b.3). Aggregate-scoped, lives
/// in the test (the outbox is cross-tenant infra with no tenant column, contract 2.3).
fn count_outbox_for_identity_tuple(run: &str) -> i64 {
    let aggregate = format!("iam:tuple:mr009id-{run}:repo:core");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = SubstrateProvider::connect(admin_config(), 2)
            .await
            .expect("admin pool");
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
            .bind(&aggregate)
            .fetch_one(admin.db_pool())
            .await
            .expect("count identity tuple outbox rows")
    })
}

/// Count this run's committed-but-unsent outbox rows (raw, aggregate-scoped — lives in the test, not
/// src: the outbox is cross-tenant infra with no tenant column, contract 2.3).
fn count_unsent_for_aggregate(run: &str) -> i64 {
    let aggregate = format!("issue:MR009-{run}");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = SubstrateProvider::connect(admin_config(), 2)
            .await
            .expect("admin pool");
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE aggregate = $1 AND published_at IS NULL",
        )
        .bind(&aggregate)
        .fetch_one(admin.db_pool())
        .await
        .expect("count unsent outbox rows")
    })
}

/// Count ALL rows (sent or unsent) for the run's GHOST aggregate — the 0-ghost assertion input
/// (rows staged in a never-committed transaction at the SIGKILL must not exist in PG at all).
fn count_rows_for_ghost_aggregate(run: &str) -> i64 {
    let aggregate = format!("issue:MR009GHOST-{run}");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = SubstrateProvider::connect(admin_config(), 2)
            .await
            .expect("admin pool");
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
            .bind(&aggregate)
            .fetch_one(admin.db_pool())
            .await
            .expect("count ghost outbox rows")
    })
}

fn cleanup_events(run: &str) {
    let aggregate = format!("issue:MR009-{run}");
    let ghost = format!("issue:MR009GHOST-{run}");
    with_admin_pool(|pool| {
        Box::pin(async move {
            for agg in [aggregate, ghost] {
                let _ = sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
                    .bind(&agg)
                    .execute(pool)
                    .await;
            }
        })
    });
}

fn cleanup_placement(run: &str) {
    let tenant = format!("01J0MR009PLACE{run}");
    let cell = format!("mr009cell-{run}");
    with_admin_pool(|pool| {
        Box::pin(async move {
            let _ = sqlx::query("DELETE FROM tenant_placement WHERE tenant_id = $1")
                .bind(&tenant)
                .execute(pool)
                .await;
            let _ = sqlx::query("DELETE FROM cell WHERE cell_id = $1")
                .bind(&cell)
                .execute(pool)
                .await;
        })
    });
}

fn cleanup_kms(run: &str) {
    let cell = format!("mr009kms-{run}");
    with_admin_pool(|pool| {
        Box::pin(async move {
            for table in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
                let _ = sqlx::query(&format!("DELETE FROM {table} WHERE cell_id = $1"))
                    .bind(&cell)
                    .execute(pool)
                    .await;
            }
        })
    });
}
