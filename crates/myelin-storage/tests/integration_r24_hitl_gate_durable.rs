//! # R2.4 — the durable HITL verdict store, proven against LIVE Postgres (`agent_hitl_gate`, 0054).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test -p myelin-storage` stays
//! DB-free (the verdict-lookup CORE is unit-tested DB-free over the in-memory arm in
//! `src/hitl_gate_durable.rs`). Runs ONLY against the docker-compose dev stack:
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-storage --features integration \
//!       --test integration_r24_hitl_gate_durable -- --nocapture
//!
//! Proves on the REAL table what R2.4 requires of the server-side verdict authority:
//!   1. a gated effect's `waiting` row INSERTs and is **lookup-able by its opaque gate_id from a
//!      SECOND store instance over a fresh pool** (across requests/processes — the property the old
//!      never-stored `hitl:{jti}:{tool}` display string could not have);
//!   2. approve/reject UPDATE the state durably, with the distinct-approver rule enforced in the
//!      durable arm (self-approval + out-of-filter refused; the row stays `waiting`);
//!   3. `GateRecord::authorizes` admits ONLY the approved exact effect for the requesting agent —
//!      read back from the second instance.
//!
//! Skips gracefully if the DB is unreachable (the sibling integration-test convention).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::hitl_gate_durable::{
    hitl_gate_durable_migrations, GateDecideError, GateRecord, GateState, HitlVerdictStore,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};

fn admin_config() -> MyelinConfig {
    let mut c = MyelinConfig::dev();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn scope_for(tenant: &str, region: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("psn:human-x".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region(region.into()))
}

fn waiting(tenant_tag: &str, gate_id: &str, effect: &str) -> GateRecord {
    GateRecord {
        gate_id: gate_id.into(),
        run_id: format!("run-{tenant_tag}"),
        effect_id: effect.into(),
        risk_summary: b"{\"template_key\":\"agent.hitl.merge_pr\"}".to_vec(),
        cost_estimate: 50,
        approver_filter: vec!["psn:lead".into(), "psn:maintainer".into()],
        state: GateState::Waiting,
        card_ref: Some("card:R1:0".into()),
        requested_by: "agent:claude".into(),
        decided_by: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_verdicts_survive_across_store_instances_with_distinct_approver_enforced() {
    // Boot: apply the 0054 migration as the admin/owner role (idempotent on the shared schema).
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate(&hitl_gate_durable_migrations(), &HotTables::none())
        .await
        .expect("migration 0054 applies (idempotent)");

    // The stores run through the app role (NOBYPASSRLS) — the production path.
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("open the app-role provider");
    let region = app.config().region.clone();
    let suffix = uniq();
    let tenant = format!("r24-hitl-{suffix}");
    let scope = scope_for(&tenant, &region);

    let gate_id = format!("gate:{suffix}");
    let effect = "gate:git.merge:myelin://acme/git/pr/40";

    // (1) INSERT the waiting gate through instance ONE.
    let mut store1 = HitlVerdictStore::with_pg(app.clone());
    store1
        .open(&scope, waiting(&suffix, &gate_id, effect))
        .expect("the pending gate row INSERTs");

    // ... and it is lookup-able by gate_id from a SECOND instance over a FRESH pool (the
    // across-processes property).
    let app2 = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("open a second app-role provider (simulated second process)");
    let mut store2 = HitlVerdictStore::with_pg(app2);
    let rec = store2
        .fetch(&scope, &gate_id)
        .expect("the gate row is visible across store instances/pools");
    assert_eq!(rec.state, GateState::Waiting);
    assert_eq!(rec.requested_by, "agent:claude");
    assert!(
        !rec.authorizes(effect, "agent:claude"),
        "a waiting gate authorizes nothing"
    );

    // (2) The distinct-HUMAN-approver rule holds in the DURABLE arm: self-approval, a distinct
    //     MACHINE (R2.4b), and an out-of-filter principal are all refused and the row STAYS waiting.
    assert_eq!(
        store2.approve(&scope, &gate_id, "agent:claude", PrincipalKind::Human),
        Err(GateDecideError::SelfApproval)
    );
    // R2.4b — a distinct principal that IS eligible but is a MACHINE is refused on LIVE PG.
    assert_eq!(
        store2.approve(
            &scope,
            &gate_id,
            "psn:lead",
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None
            }
        ),
        Err(GateDecideError::MachineApproverRefused),
        "a distinct, in-filter MACHINE approver is refused durably (distinct-HUMAN, R2.4b)"
    );
    assert_eq!(
        store2.approve(&scope, &gate_id, "psn:stranger", PrincipalKind::Human),
        Err(GateDecideError::NotEligible)
    );
    assert_eq!(
        store1.fetch(&scope, &gate_id).unwrap().state,
        GateState::Waiting
    );

    // A distinct eligible HUMAN approves through instance TWO ...
    store2
        .approve(&scope, &gate_id, "psn:lead", PrincipalKind::Human)
        .expect("a distinct eligible human approves");
    // ... and instance ONE reads the durable verdict back: approved, by psn:lead, authorizing
    // exactly the bound effect for the requesting agent — and nothing else.
    let rec = store1.fetch(&scope, &gate_id).unwrap();
    assert_eq!(rec.state, GateState::Approved);
    assert_eq!(rec.decided_by.as_deref(), Some("psn:lead"));
    assert!(rec.authorizes(effect, "agent:claude"));
    assert!(
        !rec.authorizes("gate:git.merge:myelin://acme/git/pr/41", "agent:claude"),
        "the approval is bound to ITS effect (never the tool name)"
    );
    // Terminal: a re-decide refuses durably.
    assert_eq!(
        store1.approve(&scope, &gate_id, "psn:maintainer", PrincipalKind::Human),
        Err(GateDecideError::AlreadyDecided(GateState::Approved))
    );

    // (3) A sibling gate rejected through ONE reads rejected from TWO (withheld forever).
    let gate2 = format!("gate:{suffix}-2");
    let effect2 = "gate:git.merge:myelin://acme/git/pr/41";
    store1
        .open(&scope, waiting(&suffix, &gate2, effect2))
        .expect("second gate opens");
    store1
        .reject(&scope, &gate2, "psn:lead", PrincipalKind::Human)
        .expect("rejects");
    let rec2 = store2.fetch(&scope, &gate2).unwrap();
    assert_eq!(rec2.state, GateState::Rejected);
    assert!(
        !rec2.authorizes(effect2, "agent:claude"),
        "a rejected gate never authorizes"
    );

    // Cleanup (admin role — RLS-bypassing owner).
    let _ = sqlx::query("DELETE FROM agent_hitl_gate WHERE tenant_id = $1 AND region = $2")
        .bind(&tenant)
        .bind(&region)
        .execute(admin.db_pool())
        .await;
    println!(
        "OK: agent_hitl_gate (0054) holds the server-side verdicts across store instances; \
         distinct-approver + per-effect binding enforced on LIVE PG."
    );
}
