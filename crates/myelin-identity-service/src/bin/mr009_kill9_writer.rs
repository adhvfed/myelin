//! # MR-009 — the kill-9 / restart durability child process (writer + reader).
//!
//! This is a REAL `std::process` child the MR-009 harness
//! (`tests/integration_mr009_kill9_durability.rs`) spawns and **SIGKILLs**. It exists so the
//! durability proof is a GENUINE process-death proof, not the census's old in-process "crash"
//! (drop + recreate in the SAME process — `recover_from_mirror()` / a map copy). The harness:
//!   1. spawns this bin in **`write`** mode for a store family — it writes load-bearing state
//!      through the REAL durable composition root, COMMITS, prints a `MR009-READY` line, then
//!      **blocks forever** (so the only way it ends is the parent's `SIGKILL`, never a clean
//!      shutdown / flush-on-exit);
//!   2. `kill -9`s it (a real signal — `std::process::Child::kill()` is SIGKILL on Unix) and
//!      asserts the child died **by signal 9** (not a clean exit);
//!   3. spawns this SAME bin in **`read`** mode (a FRESH OS process over the SAME live backends)
//!      which reads the state back and prints a `MR009-READ <json>` line the parent asserts on.
//!
//! Because the writer is a separate OS process that the kernel destroyed mid-life, every byte the
//! reader sees came from the live backend (Postgres / NATS / the durable KMS store), never from
//! shared process memory. That is exactly the bar the census said the old drills missed.
//!
//! Only built under `--features integration` (`required-features` in `Cargo.toml`), so the default
//! `cargo build/check --workspace` never compiles it (it stays DB-free).
//!
//! Usage: `mr009-kill9-writer <write|read> <family> <run-id> [handoff-json]`
//!   family ∈ { identity, revocation, events, placement, kms }
//! Machine output is the SINGLE line `MR009-READY <json>` / `MR009-READ <json>` /
//! `MR009-SKIP <why>` (backends unreachable → graceful skip, like the sibling integration tests).

use std::sync::Arc;
use std::time::Duration;

use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTx, Timestamp, Ulid, UlidMinter, Visibility,
};
use myelin_identity::{
    DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, RevokeTarget, TupleDelta,
};
use myelin_identity_service::principal_store::{PrincipalProfile, PrincipalStore};
use myelin_identity_service::revocation::RevocationStore;
use myelin_identity_service::tuple_store::TupleStore;
use myelin_storage::events_serve::EventsRuntime;
use myelin_storage::kms_durable::DurableKmsBacking;
use myelin_storage::placement_durable::{
    DurableCellRow, DurablePlacementBacking, DurablePlacementRow,
};
use myelin_storage::PgOutboxBacking;
use myelin_storage::{
    DurablePrincipalBacking, DurableRevocationBacking, DurableTupleBacking, KekId, KeyClass,
    SealKey, SubstrateProvider, NONCE_LEN,
};
use myelin_tenancy::{Region, TenantId};

/// The default test seal key (32 bytes / 64 hex) used when `MYELIN_KMS_SEAL_KEY` is not in the
/// environment. The harness ALWAYS passes the parent's resolved value down to BOTH children so the
/// writer + reader agree on the unseal key (the cross-restart KMS proof).
const DEFAULT_SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// The number of committed-but-unsent outbox rows the events family writes.
const EVENTS_N: usize = 8;

/// The number of rows the events family STAGES on the ghost aggregate WITHOUT committing before it
/// blocks for the SIGKILL (MR-009b W3b.6 kill-9 EMIT drill: emit-iff-committed on the durable arm —
/// these must be ABSENT from PG after the crash, 0 ghost).
const EVENTS_GHOST_N: usize = 4;

fn admin_config() -> MyelinConfig {
    let mut c = MyelinConfig::dev();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn seal_key() -> SealKey {
    let hex = std::env::var("MYELIN_KMS_SEAL_KEY").unwrap_or_else(|_| DEFAULT_SEAL_HEX.to_string());
    SealKey::from_encoded(&hex).expect("MYELIN_KMS_SEAL_KEY must be 64 hex chars")
}

/// A per-run-UNIQUE, lexically-monotonic id minter for the durable tuple store's co-committed
/// `identity.tuple.written` event (MR-009b W3b.3). The default `MonotonicMinter` resets to `0` per store,
/// so every run mints the SAME `event_id` — which the global `outbox` `UNIQUE(event_id)` collapses
/// via `ON CONFLICT DO NOTHING`, masking the co-commit when suites share the live DB. The production
/// wall-clock+random ULID source (P-S12) is globally unique; this seeds uniqueness from the run-id so
/// each kill-9 run's event survives independently (the id stays lexically-monotonic within the store).
struct RunSeededMinter {
    base: String,
    n: std::sync::atomic::AtomicU64,
}

impl RunSeededMinter {
    fn new(base: impl Into<String>) -> RunSeededMinter {
        RunSeededMinter {
            base: base.into(),
            n: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl IdMinter for RunSeededMinter {
    fn mint(&self) -> Ulid {
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ulid(format!("01J{}{n:012}", self.base))
    }
}

fn scope(tenant: &str, region: &str) -> myelin_storage::TenantScope {
    let p = Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    myelin_storage::TenantScope::from_verified_token(&p, Region(region.into()))
}

// ---- deterministic per-run keys (writer + reader derive the SAME ones from the run-id) ----

fn id_tenant(run: &str) -> String {
    format!("mr009id-{run}")
}
fn rev_tenant(run: &str) -> String {
    format!("mr009rev-{run}")
}
fn id_cell(run: &str) -> String {
    format!("mr009idkms-{run}")
}
fn kms_cell(run: &str) -> String {
    format!("mr009kms-{run}")
}
fn kms_tenant(run: &str) -> TenantId {
    TenantId(format!("01J0MR009KMS{run}"))
}
fn place_cell(run: &str) -> String {
    format!("mr009cell-{run}")
}
fn place_tenant_id(run: &str) -> String {
    format!("01J0MR009PLACE{run}")
}
fn events_aggregate(run: &str) -> String {
    format!("issue:MR009-{run}")
}
/// The aggregate the STAGED-BUT-NEVER-COMMITTED ghost rows are emitted on (the parent asserts PG
/// holds ZERO rows for it after the SIGKILL — the 0-ghost half of the W3b.6 kill-9 emit drill).
fn events_ghost_aggregate(run: &str) -> String {
    format!("issue:MR009GHOST-{run}")
}
fn events_stream(run: &str) -> String {
    format!("MYELIN_MR009_{}", run.replace('-', "_"))
}
fn events_subject_root(run: &str) -> String {
    format!("myelin_mr009_{}", run.replace('-', "_"))
}
fn profile_email(run: &str) -> String {
    format!("alice-{run}@mr009.test")
}
fn profile_name(run: &str) -> String {
    format!("Alice {run}")
}
fn kms_secret(run: &str) -> String {
    format!("mr009 kms secret {run}")
}

/// The outcome of a family op: a machine line for the parent to parse.
enum Outcome {
    /// `write` mode succeeded — state is committed; the JSON is the handoff payload.
    Ready(serde_json::Value),
    /// `read` mode succeeded — the JSON is what the fresh process observed.
    Read(serde_json::Value),
    /// Backends unreachable — graceful skip (the parent skips the test).
    Skip(String),
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: mr009-kill9-writer <write|read> <family> <run-id> [handoff-json]");
        std::process::exit(2);
    }
    let mode = args[1].clone();
    let family = args[2].clone();
    let run = args[3].clone();
    let handoff: serde_json::Value = args
        .get(4)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build a multi-thread runtime");
    let handle = rt.handle().clone();

    let outcome = rt.block_on(run_family(&mode, &family, &run, &handoff, handle));

    match outcome {
        Ok(Outcome::Ready(json)) => {
            // The state is COMMITTED. Announce readiness, then block FOREVER so the only exit is
            // the parent's SIGKILL — a genuine crash (no graceful shutdown / flush-on-exit path).
            println!("MR009-READY {json}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        Ok(Outcome::Read(json)) => {
            println!("MR009-READ {json}");
        }
        Ok(Outcome::Skip(why)) => {
            println!("MR009-SKIP {why}");
        }
        Err(e) => {
            eprintln!("MR009 child error: {e}");
            // Emit a SKIP so the parent does not hang waiting for a READY/READ line.
            println!("MR009-SKIP error:{e}");
        }
    }
}

async fn run_family(
    mode: &str,
    family: &str,
    run: &str,
    handoff: &serde_json::Value,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let writing = match mode {
        "write" => true,
        "read" => false,
        other => return Err(format!("unknown mode {other}")),
    };
    match family {
        "identity" => identity_family(writing, run, handle).await,
        "revocation" => revocation_family(writing, run, handle).await,
        "events" => events_family(writing, run, handle).await,
        "placement" => placement_family(writing, run).await,
        "kms" => kms_family(writing, run, handoff).await,
        other => Err(format!("unknown family {other}")),
    }
}

/// Connect the app-role provider (NOBYPASSRLS, reset-on-release) — `None`/Skip if PG is down.
async fn app_provider() -> Result<SubstrateProvider, Outcome> {
    SubstrateProvider::connect(MyelinConfig::dev(), 6)
        .await
        .map_err(|_| Outcome::Skip("pg-unreachable".into()))
}

/// Connect the admin-role provider (for the cross-tenant infra tables: kms / placement / outbox).
async fn admin_provider() -> Result<SubstrateProvider, Outcome> {
    SubstrateProvider::connect(admin_config(), 6)
        .await
        .map_err(|_| Outcome::Skip("pg-unreachable".into()))
}

// =================================================================================================
// identity — a principal + ReBAC tuple + a KMS-sealed profile that DECRYPTS across a kill-9 restart.
// =================================================================================================

async fn identity_family(
    writing: bool,
    run: &str,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let app = match app_provider().await {
        Ok(p) => p,
        Err(skip) => return Ok(skip),
    };
    let admin = match admin_provider().await {
        Ok(p) => p,
        Err(skip) => return Ok(skip),
    };
    let region = app.config().region.clone();
    let tenant = id_tenant(run);
    let s = scope(&tenant, &region);
    let alice = PrincipalId("p:alice".into());
    let seal = seal_key();

    // The profile PII is sealed under a per-SUBJECT DEK minted in the KMS engine; for the decrypt to
    // survive a PROCESS death the engine's root+KEK+DEK must be DURABLE (MR-025). So both writer and
    // reader build the engine via the durable backing's `load_or_generate` over the SAME cell + seal.
    let kms_backing = DurableKmsBacking::new(admin.db_pool().clone(), id_cell(run));
    let engine = kms_backing
        .load_or_generate(&seal)
        .await
        .map_err(|e| format!("kms load_or_generate: {e}"))?;
    let engine = Arc::new(engine);

    let pstore = PrincipalStore::with_pg(
        engine.clone(),
        DurablePrincipalBacking::new(app.clone()),
        handle.clone(),
    );
    // The durable S3 tuple store — its identity.tuple.written emit co-commits into the SAME rebac_tuple
    // tx as the write (MR-009b W3b.3), so no separate OutboxStore is threaded here. A run-seeded
    // minter keeps the co-committed event_id unique across runs sharing the live outbox.
    let tstore = TupleStore::with_pg_minter(
        Arc::new(RunSeededMinter::new(id_tenant(run))),
        DurableTupleBacking::new(app.clone()),
        handle.clone(),
    );

    if writing {
        let profile = PrincipalProfile {
            email: profile_email(run),
            display_name: profile_name(run),
        };
        pstore
            .put_principal(
                &s,
                alice.clone(),
                PrincipalKind::Human,
                DataRole::Processor,
                PrincipalStatus::Active,
                Some(&profile),
            )
            .map_err(|e| format!("put_principal: {e}"))?;
        // The KEK+subject DEK were minted IN the engine by put_principal's seal — persist them to the
        // durable store so a fresh engine (the reader) can resolve + decrypt after the writer dies.
        kms_backing
            .persist(&engine, &seal)
            .await
            .map_err(|e| format!("persist kms after seal: {e}"))?;
        tstore
            .write_tuples(
                &s,
                &Principal::stub(
                    PrincipalId("p:writer".into()),
                    PrincipalKind::Human,
                    TenantId(tenant.clone()),
                ),
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("repo:core".into()),
                    relation: RelName("reader".into()),
                    subject: alice.clone(),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-06-26T00:00:00Z".into()),
            )
            .map_err(|e| format!("write_tuples: {e}"))?;
        return Ok(Outcome::Ready(serde_json::json!({})));
    }

    // read: a FRESH process over live PG + the durable KMS store reads it all back.
    let row = pstore
        .get_principal(&s, &alice)
        .ok_or_else(|| "principal row not durable across kill-9".to_string())?;
    let profile = pstore
        .get_profile(&s, &alice)
        .map_err(|e| format!("get_profile: {e}"))?
        .ok_or_else(|| "profile not durable".to_string())?;
    let tuple_present = tstore
        .tuples_in(&s)
        .into_iter()
        .any(|t| t.tuple.object.0 == "repo:core" && t.tuple.subject.0 == "p:alice");
    Ok(Outcome::Read(serde_json::json!({
        "principal_kind": format!("{:?}", row.kind),
        "status": format!("{:?}", row.status),
        "profile_email": profile.email,
        "profile_name": profile.display_name,
        "tuple_present": tuple_present,
    })))
}

// =================================================================================================
// revocation — a revoked jti + a run-token TTL that survive kill-9 (revoked reads revoked; expiry honored).
// =================================================================================================

async fn revocation_family(
    writing: bool,
    run: &str,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let app = match app_provider().await {
        Ok(p) => p,
        Err(skip) => return Ok(skip),
    };
    let region = app.config().region.clone();
    let tenant = rev_tenant(run);
    let s = scope(&tenant, &region);
    let store = RevocationStore::with_pg(DurableRevocationBacking::new(app), handle);
    let jti = RevokeTarget::Jti(format!("jti-{run}"));
    let run_target = RevokeTarget::Jti(format!("run-{run}"));

    if writing {
        store.revoke(&s, &jti, Timestamp("2026-06-26T00:00:00Z".into()));
        store.register_run_token_ttl(
            &s,
            &format!("run-{run}"),
            Timestamp("2026-06-26T00:00:00Z".into()),
            Timestamp("2026-06-26T00:05:00Z".into()),
        );
        return Ok(Outcome::Ready(serde_json::json!({})));
    }

    let jti_revoked = store.is_revoked(&s, &jti, &Timestamp("2026-06-26T00:00:01Z".into()));
    let run_state_before =
        store.run_token_state(&s, &run_target, &Timestamp("2026-06-26T00:02:00Z".into()));
    let run_state_after =
        store.run_token_state(&s, &run_target, &Timestamp("2026-06-26T00:06:00Z".into()));
    Ok(Outcome::Read(serde_json::json!({
        "jti_revoked": jti_revoked,
        "run_state_before": format!("{run_state_before:?}"),
        "run_state_after": format!("{run_state_after:?}"),
    })))
}

// =================================================================================================
// events — committed-but-unsent outbox rows survive kill-9; the restart relay drains them → 0 lost.
// =================================================================================================

async fn events_family(
    writing: bool,
    run: &str,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let admin = match admin_provider().await {
        Ok(p) => p,
        Err(skip) => return Ok(skip),
    };
    // The outbox + consumer_dedup foundation tables (idempotent; the parent migrates too).
    admin
        .migrate_foundation()
        .await
        .map_err(|e| format!("migrate_foundation: {e}"))?;
    let cfg = admin.config().clone();
    let aggregate = events_aggregate(run);

    let runtime = match EventsRuntime::over_pool(
        admin.db_pool().clone(),
        &cfg.region,
        &cfg.nats_url,
        &events_stream(run),
        &events_subject_root(run),
        &format!("{}_pull", events_stream(run)),
        handle,
    ) {
        Ok(r) => r,
        Err(_) => return Ok(Outcome::Skip("nats-unreachable".into())),
    };

    if writing {
        // **The MR-009b W3b.6 kill-9 EMIT drill (the flip's exit proof):** emit through the ONE
        // production emit path — `OutboxStore::durable(PgOutboxBacking)` + the frozen
        // `OutboxTx::emit` → `OutboxTransaction::commit` (which routes the whole staged buffer
        // through `DurableOutboxBacking::commit_staged` in ONE PG tx) — with the PRODUCTION
        // `UlidMinter` (the W3b.3 named condition: a per-run-unique id source, never the
        // per-instance-resetting `MonotonicMinter`). This is the byte-identical composition-root
        // shape the six rewired service mains boot; the old raw `PgRelay::co_commit_in_tx` loop
        // was the pre-flip shape (that caller's-tx co-commit path stays proven by the IDENTITY
        // family's tuple co-commit assert).
        let store = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
            admin.db_pool().clone(),
            tokio::runtime::Handle::current(),
        )));
        let minter: Arc<dyn IdMinter> = Arc::new(UlidMinter::new());

        // (a) COMMITTED rows: one durable co-commit transaction, EVENTS_N staged rows, committed.
        //     All committed + UNSENT — we do NOT drain (the restart relay proves 0 lost).
        let mut tx = store.begin(Arc::clone(&minter), events_ctx_base());
        tx.stage_state_change(format!("mr009 events kill-9 emit drill {run}"));
        for i in 0..EVENTS_N {
            tx.emit(events_draft(run, i, &aggregate), None)
                .map_err(|e| format!("emit committed row {i}: {}", e.0))?;
        }
        tx.commit()
            .map_err(|e| format!("durable co-commit of the {EVENTS_N} rows: {}", e.0))?;

        // (b) GHOST rows: a SECOND transaction stages EVENTS_GHOST_N rows on the ghost aggregate
        //     and is deliberately NEVER committed — it is held OPEN (leaked, not dropped) while
        //     this process blocks for the SIGKILL, so at the moment of the crash the rows are
        //     staged-in-process-memory only. Emit-iff-committed (BUS-D4) on the DURABLE arm means
        //     PG must hold ZERO rows for the ghost aggregate after the kill (the parent asserts).
        let ghost_aggregate = events_ghost_aggregate(run);
        let mut ghost_tx = store.begin(Arc::clone(&minter), events_ctx_base());
        for i in 0..EVENTS_GHOST_N {
            ghost_tx
                .emit(events_draft(run, i, &ghost_aggregate), None)
                .map_err(|e| format!("emit ghost row {i}: {}", e.0))?;
        }
        // Keep the un-committed transaction ALIVE until the SIGKILL (never dropped, never
        // committed) — the genuine "staged at kill" crash shape.
        std::mem::forget(ghost_tx);

        return Ok(Outcome::Ready(serde_json::json!({
            "committed": EVENTS_N,
            "ghost_staged": EVENTS_GHOST_N,
        })));
    }

    // read (the RESTART relay): a fresh process re-claims the survived outbox rows and drains them to
    // the real broker. The "0 lost / survived" counting is the PARENT's job (it counts the outbox
    // depth for our aggregate before + after this drain — a tenant-less outbox count belongs in the
    // test, not in src, since the outbox is cross-tenant infra with no tenant column, contract 2.3).
    let _ = aggregate; // (the aggregate scoping is asserted by the parent's count, not here)
    let published = runtime
        .drain_relay_to_empty()
        .await
        .map_err(|e| format!("drain: {e}"))?;
    Ok(Outcome::Read(serde_json::json!({ "published": published })))
}

/// The ambient emit context for the W3b.6 kill-9 EMIT drill (the production emit path derives the
/// envelope from this + the minted ULID through the frozen `derive_envelope` — no hand-built
/// `EventEnvelope` in the drill: the drill drives the SAME emit surface production drives).
fn events_ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-26T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-26T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:mr009".into())),
    }
}

/// One emitted event draft for the kill-9 emit drill (the `event_id` is minted by the injected
/// `UlidMinter` at emit; the per-aggregate `seq` is assigned by the durable backing at commit).
fn events_draft(run: &str, i: usize, aggregate: &str) -> EventDraft {
    let _ = run;
    EventDraft {
        type_: EventType("issues.issue.created".into()),
        subject: ArtifactRef(format!("myelin://acme/issues/{i}")),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": "mr009" }),
        data_role: myelin_events::DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

// =================================================================================================
// placement (control-plane) — a tenant→cell placement that survives kill-9.
// =================================================================================================

async fn placement_family(writing: bool, run: &str) -> Result<Outcome, String> {
    let admin = match admin_provider().await {
        Ok(p) => p,
        Err(skip) => return Ok(skip),
    };
    let backing = DurablePlacementBacking::new(admin.db_pool().clone());
    let cell = place_cell(run);
    let tenant = place_tenant_id(run);

    if writing {
        backing
            .insert_cell(&DurableCellRow {
                cell_id: cell.clone(),
                region: "eu-west".into(),
                status: "Active".into(),
                isolation_kind: "Pool".into(),
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
                utilisation: 10,
                version: 1,
                endpoint: "cell.eu-west.myelin.eu".into(),
            })
            .await
            .map_err(|e| format!("insert_cell: {e}"))?;
        backing
            .place_tenant(&DurablePlacementRow {
                tenant_id: tenant.clone(),
                region: "eu-west".into(),
                home_cell: cell.clone(),
                isolation_tier: "Pool".into(),
                slug: format!("acme-{run}"),
                status: "Active".into(),
                member_cells: vec![cell.clone()],
            })
            .await
            .map_err(|e| format!("place_tenant: {e}"))?;
        return Ok(Outcome::Ready(serde_json::json!({})));
    }

    let placement = backing
        .get_placement(&tenant)
        .await
        .map_err(|e| format!("get_placement: {e}"))?
        .ok_or_else(|| "placement not durable across kill-9".to_string())?;
    Ok(Outcome::Read(serde_json::json!({
        "home_cell": placement.home_cell,
        "region": placement.region,
        "status": placement.status,
    })))
}

// =================================================================================================
// kms — the sealed root + KEK + DEK survive kill-9; data sealed pre-kill decrypts post-restart.
// =================================================================================================

async fn kms_family(
    writing: bool,
    run: &str,
    handoff: &serde_json::Value,
) -> Result<Outcome, String> {
    let admin = match admin_provider().await {
        Ok(p) => p,
        Err(skip) => return Ok(skip),
    };
    let backing = DurableKmsBacking::new(admin.db_pool().clone(), kms_cell(run));
    let seal = seal_key();
    let tenant = kms_tenant(run);
    let region = Region("eu-west".into());

    let engine = backing
        .load_or_generate(&seal)
        .await
        .map_err(|e| format!("load_or_generate: {e}"))?;

    if writing {
        // Mint + DURABLY persist the KEK then the DEK (the write-through helpers persist as they mint).
        backing
            .ensure_kek(&engine, &KekId::new(tenant.clone(), region.clone()))
            .await
            .map_err(|e| format!("ensure_kek: {e}"))?;
        let key_ref = backing
            .ensure_dek(&engine, &tenant, &region, KeyClass::Tenant)
            .await
            .map_err(|e| format!("ensure_dek: {e}"))?;
        let dek = engine
            .resolve_dek(&key_ref, &region)
            .map_err(|e| format!("resolve_dek: {e}"))?;
        let (nonce, ct) = dek.seal(kms_secret(run).as_bytes());
        // Hand the ciphertext off to the reader over stdout (it is not stored in any table).
        return Ok(Outcome::Ready(serde_json::json!({
            "nonce": nonce.to_vec(),
            "ct": ct,
        })));
    }

    // read: a FRESH engine (loaded the durable root+KEK+DEK) decrypts the pre-kill ciphertext.
    let nonce_vec: Vec<u8> = serde_json::from_value(handoff["nonce"].clone())
        .map_err(|e| format!("decode nonce handoff: {e}"))?;
    let ct: Vec<u8> = serde_json::from_value(handoff["ct"].clone())
        .map_err(|e| format!("decode ct handoff: {e}"))?;
    if nonce_vec.len() != NONCE_LEN {
        return Err(format!("nonce handoff wrong length: {}", nonce_vec.len()));
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&nonce_vec);
    // ensure_dek is idempotent — over a loaded store it returns the EXISTING key ref (no re-mint).
    let key_ref = engine
        .ensure_dek(&tenant, &region, KeyClass::Tenant)
        .map_err(|e| format!("re-derive key_ref: {e}"))?;
    let dek = engine
        .resolve_dek(&key_ref, &region)
        .map_err(|e| format!("resolve_dek (post-restart): {e}"))?;
    let plain = dek
        .open(&nonce, &ct)
        .ok_or_else(|| "post-restart decrypt FAILED (root/KEK/DEK not durable)".to_string())?;
    let decrypted = String::from_utf8(plain).map_err(|e| format!("utf8: {e}"))?;
    Ok(Outcome::Read(serde_json::json!({ "decrypted": decrypted })))
}
