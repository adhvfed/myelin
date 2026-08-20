#![cfg(feature = "integration")]

mod common;

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use myelin_storage::identity_durable_migrations;
use myelin_storage::kms_durable::kms_durable_migrations;
use myelin_storage::migration::HotTables;
use myelin_storage::placement_durable::placement_durable_migrations;

const DEFAULT_SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

const WRITER_BIN: &str = env!("CARGO_BIN_EXE_mr009_kill9_writer");

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

fn ensure_migrated() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = common::admin_provider(4).await;
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
    })
}

fn child_cmd(mode: &str, family: &str, run: &str, handoff: &str) -> Command {
    let mut cmd = Command::new(WRITER_BIN);
    cmd.args([mode, family, run, handoff])
        .env("MYELIN_KMS_SEAL_KEY", seal_hex())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    cmd
}

enum Line {
    Ready(String),
    Read(String),
    Error(String),
}

fn parse_line(line: &str) -> Option<Line> {
    let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
    match tag {
        "MR009-READY" => Some(Line::Ready(rest.to_string())),
        "MR009-READ" => Some(Line::Read(rest.to_string())),
        "MR009-ERROR" => Some(Line::Error(rest.to_string())),
        _ => None,
    }
}

fn spawn_writer_until_ready(family: &str, run: &str) -> (Child, String) {
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
        Ok(Some(Line::Ready(json))) => (child, json),
        Ok(Some(Line::Error(why))) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("[{family}] the writer failed before becoming ready: {why}");
        }
        Ok(Some(Line::Read(_))) | Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("[{family}] the writer produced no READY line within 120 seconds");
        }
    }
}

fn sigkill_and_assert_crash(family: &str, mut child: Child) {
    child.kill().expect("send SIGKILL to the writer child");
    let status = child.wait().expect("reap the SIGKILLed child");
    assert_eq!(
        status.signal(),
        Some(9),
        "[{family}] the writer MUST have died by SIGKILL (signal 9), not a clean exit - \
         got status {status:?} (code={:?}, signal={:?})",
        status.code(),
        status.signal()
    );
    assert!(
        status.code().is_none(),
        "[{family}] a SIGKILLed process has NO clean exit code (it never ran a shutdown path)"
    );
    println!(
        "[MR-009] {family}: writer child SIGKILLed - confirmed died by signal 9 (no clean exit)."
    );
}

fn read_back(family: &str, run: &str, handoff: &str) -> serde_json::Value {
    let out = child_cmd("read", family, run, handoff)
        .output()
        .expect("run read child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        match parse_line(line) {
            Some(Line::Read(json)) => {
                return serde_json::from_str(&json).expect("parse READ json");
            }
            Some(Line::Error(why)) => {
                panic!("[{family}] the reader failed: {why}");
            }
            _ => {}
        }
    }
    panic!("[{family}] the read child produced no MR009-READ line; stdout=\n{stdout}");
}

#[test]
fn kill9_identity_principal_tuple_and_profile_decrypt_across_restart() {
    ensure_migrated();
    let run = run_id();
    let (child, _ready) = spawn_writer_until_ready("identity", &run);
    sigkill_and_assert_crash("identity", child);

    let read = read_back("identity", &run, "{}");
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
    assert_eq!(
        count_outbox_for_identity_tuple(&run),
        1,
        "the identity.tuple.written event co-committed with the tuple + survived kill-9 \
         (both exist - 0 ghost / 0 lost, the kill-9 shape)"
    );
    cleanup_identity(&run);
    println!(
        "[MR-009] PASS  family=IDENTITY  principal+tuple durable across kill-9; \
         identity.tuple.written co-committed + survived (0 ghost / 0 lost); \
         profile DECRYPTS post-restart under the durable KMS root (MR-025)."
    );
}

#[test]
fn kill9_revocation_jti_and_run_token_ttl_survive() {
    ensure_migrated();
    let run = run_id();
    let (child, _ready) = spawn_writer_until_ready("revocation", &run);
    sigkill_and_assert_crash("revocation", child);

    let read = read_back("revocation", &run, "{}");
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
    ensure_migrated();
    let run = run_id();
    let (child, ready) = spawn_writer_until_ready("events", &run);
    let ready: serde_json::Value = serde_json::from_str(&ready).expect("parse READY json");
    assert_eq!(ready["committed"], 8, "writer committed 8 rows: {ready}");
    assert_eq!(
        ready["ghost_staged"], 4,
        "writer staged 4 uncommitted ghost rows before blocking: {ready}"
    );
    sigkill_and_assert_crash("events", child);

    let survived = count_unsent_for_aggregate(&run);
    assert_eq!(
        survived, 8,
        "all 8 committed-but-unsent outbox rows SURVIVED kill-9 (0 lost on the crash)"
    );

    let ghosts = count_rows_for_ghost_aggregate(&run);
    assert_eq!(
        ghosts, 0,
        "rows staged-but-uncommitted at the kill must be ABSENT after the crash (0 ghost)"
    );

    let read = read_back("events", &run, "{}");
    assert!(
        read["published"].is_u64(),
        "the restarted relay completed a typed drain pass: {read}"
    );

    let remaining = count_unsent_for_aggregate(&run);
    assert_eq!(
        remaining, 0,
        "the restarted relay and any concurrently elected publisher drained every survived row \
         (0 lost across the kill-9 + restart): {read}"
    );
    assert_eq!(
        count_rows_for_ghost_aggregate(&run),
        0,
        "the restart relay drained only COMMITTED rows - the ghost aggregate stays empty"
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
    ensure_migrated();
    let run = run_id();
    let (child, _ready) = spawn_writer_until_ready("placement", &run);
    sigkill_and_assert_crash("placement", child);

    let read = read_back("placement", &run, "{}");
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
    ensure_migrated();
    let run = run_id();
    let (child, ready) = spawn_writer_until_ready("kms", &run);
    sigkill_and_assert_crash("kms", child);

    let read = read_back("kms", &run, &ready);
    assert_eq!(
        read["decrypted"],
        format!("mr009 kms secret {run}"),
        "data sealed PRE-kill DECRYPTS post-restart - the sealed root + KEK + DEK survived kill-9: {read}"
    );
    cleanup_kms(&run);
    println!(
        "[MR-009] PASS  family=KMS  sealed root + KEK + DEK survived kill-9; \
         pre-kill ciphertext decrypts post-restart."
    );
}

#[test]
fn three_instance_consistency_no_split_brain() {
    ensure_migrated();
    let run = run_id();
    let (child, _ready) = spawn_writer_until_ready("identity", &run);
    sigkill_and_assert_crash("identity", child);

    let mut views: Vec<serde_json::Value> = Vec::new();
    for _ in 0..3 {
        views.push(read_back("identity", &run, "{}"));
    }
    assert_eq!(views.len(), 3, "spawned 3 reader instances");
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

fn with_admin_pool<F>(f: F)
where
    F: for<'a> FnOnce(
        &'a sqlx::PgPool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>,
{
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = common::admin_provider(2).await;
        f(admin.db_pool()).await;
    });
}

fn cleanup_identity(run: &str) {
    let tenant = format!("mr009id-{run}");
    let cell = format!("mr009idkms-{run}");
    let tuple_aggregate = format!("identity:tuple:mr009id-{run}:repo:core");
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

fn count_outbox_for_identity_tuple(run: &str) -> i64 {
    let aggregate = format!("identity:tuple:mr009id-{run}:repo:core");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = common::admin_provider(2).await;
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate = $1")
            .bind(&aggregate)
            .fetch_one(admin.db_pool())
            .await
            .expect("count identity tuple outbox rows")
    })
}

fn count_unsent_for_aggregate(run: &str) -> i64 {
    let aggregate = format!("issue:MR009-{run}");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = common::admin_provider(2).await;
        sqlx::query_scalar(
            "SELECT count(*) FROM outbox WHERE aggregate = $1 AND published_at IS NULL",
        )
        .bind(&aggregate)
        .fetch_one(admin.db_pool())
        .await
        .expect("count unsent outbox rows")
    })
}

fn count_rows_for_ghost_aggregate(run: &str) -> i64 {
    let aggregate = format!("issue:MR009GHOST-{run}");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let admin = common::admin_provider(2).await;
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
