#![cfg(feature = "integration")]

// The erasure-survives-restore drill, against a REAL pg_dump/pg_restore:
//
//   1. a subject's data exists under a per-subject DEK (durable KMS, real pg)
//   2. a backup is taken (pg_dump, custom format)
//   3. the subject is erased through the production path - which now records
//      the erasure in the post-PIT ledger BEFORE destroying the key
//   4. the backup is restored into a scratch database - and the drill proves
//      the wrapped subject DEK is RESURRECTED there (the drill has teeth)
//   5. the re-erase pass replays the live ledger against the restored
//      database and the subject is unreadable again
//
// This replaces the deleted "restore-verify permanent gate", which simulated
// all of this over hand-built in-memory vectors and proved nothing.

use std::process::Command;
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::reerase::ReErasePass;
use myelin_storage::reerase_durable::DurablePostPitLedger;
use myelin_storage::restore::{ReindexFromSource, RestoreReport, SourceLog};
use myelin_storage::{
    all_durable_migrations, AgentTraceWrite, AgentTraceWriter, DurableAgentTraceStore,
    DurableKmsBacking, EraseError, EraseHolders, HotTables, KeyClass, KmsError, PiiKeyRef,
    SealKey, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};

fn app_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
}

fn admin_config() -> MyelinConfig {
    let mut config = app_config();
    config.database_url = config
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    config
}

fn unique(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    )
}

fn scratch_db_name() -> String {
    format!(
        "erasure_drill_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos()
    )
}

fn swap_database(url: &str, db: &str) -> String {
    let (base, _) = url
        .rsplit_once('/')
        .expect("a postgres URL carries a database segment");
    format!("{base}/{db}")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock follows the epoch")
        .as_secs()
}

fn trace(run_id: &str) -> AgentTraceWrite {
    let answer = "A result that must stay erased across restores.";
    AgentTraceWrite {
        run_id: run_id.into(),
        agent_principal: "agent:privacy-test".into(),
        requested_by: "founder".into(),
        answer: answer.into(),
        trace_body: serde_json::json!({
            "schema": "myelin.agent_trace.v1",
            "run_id": run_id,
            "actor": "agent:privacy-test",
            "requested_by": "founder",
            "answer": answer,
            "charged_micro": 42,
            "blocks": [{
                "type": "paragraph",
                "inline": {
                    "spans": [{"Text": {"text": answer, "marks": [], "link": null}}],
                    "nodes": []
                }
            }]
        }),
        charged_micro: 42,
    }
}

// The drill's scope is the KMS tier: the per-subject DEK must stay destroyed
// across a restore. The holder fan-out (search purge, refs tombstones, bus
// PII) has its own drills; here each holder is a recorded no-op.
struct NoopHolder;

impl myelin_storage::PseudonymShred for NoopHolder {
    fn shred_pseudonym(
        &self,
        _s: &myelin_storage::SubjectId,
        _t: &TenantId,
    ) -> Result<(), EraseError> {
        Ok(())
    }
}
impl myelin_storage::SearchPurge for NoopHolder {
    fn purge_and_reindex(
        &self,
        _s: &myelin_storage::SubjectId,
        _t: &TenantId,
    ) -> Result<(), EraseError> {
        Ok(())
    }
}
impl myelin_storage::RefsTombstone for NoopHolder {
    fn tombstone(
        &self,
        _s: &myelin_storage::SubjectId,
        _t: &TenantId,
    ) -> Result<(), EraseError> {
        Ok(())
    }
}
impl myelin_storage::BusErase for NoopHolder {
    fn erase_inline_pii(
        &self,
        _s: &myelin_storage::SubjectId,
        _t: &TenantId,
    ) -> Result<(), EraseError> {
        Ok(())
    }
}
impl myelin_storage::ErasureLedgerSink for NoopHolder {
    fn record_erasure(&self, _s: &myelin_storage::SubjectId, _t: &TenantId, _at: u64) {}
    fn is_erased(&self, _s: &myelin_storage::SubjectId, _t: &TenantId) -> bool {
        false
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subject_erased_after_a_backup_stays_erased_when_that_backup_is_restored() {
    let admin = SubstrateProvider::connect(admin_config(), 2)
        .await
        .expect("the drill requires the configured Postgres backend");
    admin.migrate_foundation().await.unwrap();
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap();
    let provider = SubstrateProvider::connect(app_config(), 2)
        .await
        .expect("open the app provider");

    let tenant = TenantId(unique("restore-drill"));
    let region = Region(provider.config().region.clone());
    let cell = unique("restore-drill-cell");
    let seal_key = SealKey::from_encoded(&"77".repeat(32)).expect("a 32-byte test seal key");
    let kms = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), cell.clone())
            .load_or_generate(&seal_key)
            .await
            .expect("the durable drill KMS starts"),
    );
    let store = DurableAgentTraceStore::with_runtime(
        provider.clone(),
        tokio::runtime::Handle::current(),
        kms.clone(),
    );

    // 1. the subject's data exists under its per-subject DEK.
    store
        .write(&tenant, trace("55555555-5555-4555-8555-555555555555"))
        .expect("one subject-owned trace exists before the backup");
    let key_ref = PiiKeyRef::new(tenant.clone(), 0, KeyClass::Subject("founder".to_string()));
    kms.resolve_dek(&key_ref, &region)
        .expect("the subject DEK resolves before the backup (the data is live)");

    // 2. a real backup, taken while the subject is still live.
    let admin_url = admin_config().database_url;
    let dump_path = std::env::temp_dir().join(format!("{}.dump", scratch_db_name()));
    let dump = Command::new("pg_dump")
        .args(["--format=custom", "--file"])
        .arg(&dump_path)
        .arg(&admin_url)
        .output()
        .expect("pg_dump runs");
    assert!(
        dump.status.success(),
        "pg_dump captured the live database: {}",
        String::from_utf8_lossy(&dump.stderr)
    );
    // the restore point sits strictly before the erasure that follows.
    let pit_secs = now_secs().saturating_sub(1);

    // 3. the subject is erased through the production path (which records the
    //    post-PIT ledger row before destroying the key).
    let receipt = store
        .erase_for_subject(&tenant.0, "founder")
        .await
        .expect("the production erase path completes");
    assert!(!receipt.already_erased, "a fresh erasure, not a replay");
    match kms.resolve_dek(&key_ref, &region) {
        Err(KmsError::DekUnavailable(_)) => {}
        other => panic!("the live subject DEK must be destroyed after erasure, got {other:?}"),
    }

    // 4. restore the pre-erasure backup into a scratch database.
    let scratch = scratch_db_name();
    let scratch_url = swap_database(&admin_url, &scratch);
    sqlx::raw_sql(&format!("CREATE DATABASE {scratch}"))
        .execute(admin.db_pool())
        .await
        .expect("create the scratch restore target");
    let restore = Command::new("pg_restore")
        .args(["--no-owner", "--dbname"])
        .arg(&scratch_url)
        .arg(&dump_path)
        .output()
        .expect("pg_restore runs");
    // a newer client dumping an older server emits session SETs the server
    // does not know (e.g. transaction_timeout). those are the ONLY errors a
    // green drill may ignore; any data-level restore error still fails.
    if !restore.status.success() {
        let stderr = String::from_utf8_lossy(&restore.stderr);
        let benign_version_skew = stderr
            .lines()
            .filter(|line| line.contains("error:"))
            .all(|line| line.contains("unrecognized configuration parameter"));
        assert!(
            benign_version_skew,
            "pg_restore failed beyond session-setting version skew: {stderr}"
        );
    }

    let mut scratch_config = admin_config();
    scratch_config.database_url = scratch_url.clone();
    let scratch_provider = SubstrateProvider::connect(scratch_config, 2)
        .await
        .expect("connect to the restored scratch database");
    let scratch_kms = DurableKmsBacking::new(scratch_provider.db_pool().clone(), cell.clone())
        .load_or_generate(&seal_key)
        .await
        .expect("the restored KMS state loads under the same seal key");

    // THE TEETH: the restore genuinely resurrects the erased subject. If this
    // assertion ever starts failing, the drill has gone vacuous - fix the
    // drill, do not celebrate.
    scratch_kms.resolve_dek(&key_ref, &region).expect(
        "the restored backup resurrects the erased subject's DEK - this is the exact \
         exposure the re-erase pass exists to close",
    );

    // 5. replay the live post-PIT erasure ledger against the restored
    //    database; the subject must be unreadable again.
    let ledger = DurablePostPitLedger::new(provider.clone());
    let noop = NoopHolder;
    let holders = EraseHolders {
        pseudonym: &noop,
        search: &noop,
        refs: &noop,
        bus: &noop,
        ledger: &noop,
        git_reach: None,
    };
    let report = RestoreReport {
        restored_to_offset: pit_secs,
        oltp_rows: vec![],
        derived: ReindexFromSource::reindex(&SourceLog::new(), 0),
        restored_keys: vec![],
        dangling_ref_count: 0,
    };
    let pass = ReErasePass::new(&scratch_kms, region.clone());
    let re_erase = tokio::task::block_in_place(|| {
        pass.run(&report, &ledger, &holders, now_secs() * 1000)
    })
    .expect("the re-erase pass completes against the restored database");

    assert!(
        re_erase.re_erased_subject(&myelin_storage::SubjectId::new("founder"), &tenant),
        "the ledger replay re-erased the drill subject: {re_erase:?}"
    );
    assert!(
        re_erase.is_green(),
        "no subject stays resurrected after the pass: {re_erase:?}"
    );
    match scratch_kms.resolve_dek(&key_ref, &region) {
        Err(KmsError::DekUnavailable(_)) => {}
        other => panic!(
            "the restored subject DEK must be destroyed after the re-erase pass, got {other:?}"
        ),
    }
    let (leftover,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM kms_wrapped_dek WHERE tenant_id = $1 AND cell_id = $2",
    )
    .bind(&tenant.0)
    .bind(&cell)
    .fetch_one(scratch_provider.db_pool())
    .await
    .expect("count restored DEK rows");
    assert_eq!(
        leftover, 0,
        "no wrapped DEK row for the erased subject survives in the restored database"
    );

    // cleanup: the scratch database and the dump are drill artifacts.
    drop(scratch_kms);
    scratch_provider.db_pool().close().await;
    sqlx::raw_sql(&format!("DROP DATABASE {scratch} WITH (FORCE)"))
        .execute(admin.db_pool())
        .await
        .expect("drop the scratch restore target");
    let _ = std::fs::remove_file(&dump_path);
}
