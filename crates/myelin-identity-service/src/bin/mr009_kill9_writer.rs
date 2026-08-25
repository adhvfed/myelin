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

const DEFAULT_SEAL_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

const EVENTS_N: usize = 8;

const EVENTS_GHOST_N: usize = 4;

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

fn seal_key() -> SealKey {
    let hex = std::env::var("MYELIN_KMS_SEAL_KEY").unwrap_or_else(|_| DEFAULT_SEAL_HEX.to_string());
    SealKey::from_encoded(&hex).expect("MYELIN_KMS_SEAL_KEY must be 64 hex chars")
}

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

enum Outcome {
    Ready(serde_json::Value),
    Read(serde_json::Value),
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
        Err(e) => {
            eprintln!("MR009 child error: {e}");
            println!("MR009-ERROR {e}");
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

async fn app_provider() -> Result<SubstrateProvider, String> {
    SubstrateProvider::connect(app_config(), 6)
        .await
        .map_err(|error| format!("connect to the required Postgres backend: {error}"))
}

async fn admin_provider() -> Result<SubstrateProvider, String> {
    SubstrateProvider::connect(admin_config(), 6)
        .await
        .map_err(|error| format!("connect to the required admin Postgres backend: {error}"))
}

async fn identity_family(
    writing: bool,
    run: &str,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let app = app_provider().await?;
    let admin = admin_provider().await?;
    let region = app.config().region.clone();
    let tenant = id_tenant(run);
    let s = scope(&tenant, &region);
    let alice = PrincipalId("p:alice".into());
    let seal = seal_key();

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

    let row = pstore
        .get_principal(&s, &alice)
        .map_err(|e| format!("get_principal: {e}"))?
        .ok_or_else(|| "principal row not durable across kill-9".to_string())?;
    let profile = pstore
        .get_profile(&s, &alice)
        .map_err(|e| format!("get_profile: {e}"))?
        .ok_or_else(|| "profile not durable".to_string())?;
    let tuple_present = tstore
        .tuples_in(&s)
        .map_err(|e| format!("read tuples: {e}"))?
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

async fn revocation_family(
    writing: bool,
    run: &str,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let app = app_provider().await?;
    let region = app.config().region.clone();
    let tenant = rev_tenant(run);
    let s = scope(&tenant, &region);
    let store = RevocationStore::with_pg(DurableRevocationBacking::new(app), handle);
    let jti = RevokeTarget::Jti(format!("jti-{run}"));
    let run_target = RevokeTarget::Jti(format!("run-{run}"));

    if writing {
        store
            .revoke(&s, &jti, Timestamp("2026-06-26T00:00:00Z".into()))
            .map_err(|error| format!("record revocation: {error}"))?;
        store
            .register_run_token_ttl(
                &s,
                &format!("run-{run}"),
                Timestamp("2026-06-26T00:00:00Z".into()),
                Timestamp("2026-06-26T00:05:00Z".into()),
            )
            .map_err(|error| format!("record run-token lifetime: {error}"))?;
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

async fn events_family(
    writing: bool,
    run: &str,
    handle: tokio::runtime::Handle,
) -> Result<Outcome, String> {
    let admin = admin_provider().await?;
    admin
        .migrate_foundation()
        .await
        .map_err(|e| format!("migrate_foundation: {e}"))?;
    let cfg = admin.config().clone();
    let aggregate = events_aggregate(run);

    let runtime = EventsRuntime::over_pool(
        admin.db_pool().clone(),
        &cfg.region,
        &cfg.nats_url,
        &events_stream(run),
        &events_subject_root(run),
        &format!("{}_pull", events_stream(run)),
        handle,
    )
    .map_err(|error| format!("connect to the required NATS backend: {error}"))?;

    if writing {
        let store = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
            admin.db_pool().clone(),
            tokio::runtime::Handle::current(),
        )));
        let minter: Arc<dyn IdMinter> = Arc::new(UlidMinter::new());

        let mut tx = store.begin(Arc::clone(&minter), events_ctx_base());
        tx.stage_state_change(format!("mr009 events kill-9 emit drill {run}"));
        for i in 0..EVENTS_N {
            tx.emit(events_draft(run, i, &aggregate), None)
                .map_err(|e| format!("emit committed row {i}: {}", e.0))?;
        }
        tx.commit()
            .map_err(|e| format!("durable co-commit of the {EVENTS_N} rows: {}", e.0))?;

        let ghost_aggregate = events_ghost_aggregate(run);
        let mut ghost_tx = store.begin(Arc::clone(&minter), events_ctx_base());
        for i in 0..EVENTS_GHOST_N {
            ghost_tx
                .emit(events_draft(run, i, &ghost_aggregate), None)
                .map_err(|e| format!("emit ghost row {i}: {}", e.0))?;
        }
        std::mem::forget(ghost_tx);

        return Ok(Outcome::Ready(serde_json::json!({
            "committed": EVENTS_N,
            "ghost_staged": EVENTS_GHOST_N,
        })));
    }

    let _ = aggregate;
    let published = runtime
        .drain_relay_to_empty()
        .await
        .map_err(|e| format!("drain: {e}"))?;
    Ok(Outcome::Read(serde_json::json!({ "published": published })))
}

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

fn events_draft(run: &str, i: usize, aggregate: &str) -> EventDraft {
    EventDraft {
        type_: EventType("issue.issue.created".into()),
        subject: ArtifactRef(format!("myelin://acme/issue/issue/MR009-{run}-{i}")),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": "mr009" }),
        data_role: myelin_events::DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

async fn placement_family(writing: bool, run: &str) -> Result<Outcome, String> {
    let admin = admin_provider().await?;
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

async fn kms_family(
    writing: bool,
    run: &str,
    handoff: &serde_json::Value,
) -> Result<Outcome, String> {
    let admin = admin_provider().await?;
    let backing = DurableKmsBacking::new(admin.db_pool().clone(), kms_cell(run));
    let seal = seal_key();
    let tenant = kms_tenant(run);
    let region = Region("eu-west".into());

    let engine = backing
        .load_or_generate(&seal)
        .await
        .map_err(|e| format!("load_or_generate: {e}"))?;

    if writing {
        engine
            .ensure_kek(&KekId::new(tenant.clone(), region.clone()))
            .map_err(|e| format!("ensure_kek: {e}"))?;
        let key_ref = engine
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .map_err(|e| format!("ensure_dek: {e}"))?;
        let dek = engine
            .resolve_dek(&key_ref, &region)
            .map_err(|e| format!("resolve_dek: {e}"))?;
        let (nonce, ct) = dek.seal(kms_secret(run).as_bytes());
        return Ok(Outcome::Ready(serde_json::json!({
            "nonce": nonce.to_vec(),
            "ct": ct,
        })));
    }

    let nonce_vec: Vec<u8> = serde_json::from_value(handoff["nonce"].clone())
        .map_err(|e| format!("decode nonce handoff: {e}"))?;
    let ct: Vec<u8> = serde_json::from_value(handoff["ct"].clone())
        .map_err(|e| format!("decode ct handoff: {e}"))?;
    if nonce_vec.len() != NONCE_LEN {
        return Err(format!("nonce handoff wrong length: {}", nonce_vec.len()));
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&nonce_vec);
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
