#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_storage::hitl_gate_durable::{
    hitl_gate_durable_migrations, DurableHitlGateBacking, GateDecideError, GateRecord, GateState,
    HitlVerdictStore,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};

fn app_config() -> MyelinConfig {
    let mut c = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            c.database_url = database_url;
        }
    }
    c
}

fn admin_config() -> MyelinConfig {
    let mut c = app_config();
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
        opened_at_unix: 100,
        decided_at_unix: None,
        expires_at_unix: i64::MAX,
        approval_consumed_at_unix: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_verdicts_survive_across_store_instances_with_distinct_approver_enforced() {
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

    let app = SubstrateProvider::connect(app_config(), 4)
        .await
        .expect("open the app-role provider");
    let region = app.config().region.clone();
    let suffix = uniq();
    let tenant = format!("r24-hitl-{suffix}");
    let scope = scope_for(&tenant, &region);

    let gate_id = format!("gate:{suffix}");
    let effect = "gate:git.merge:myelin://acme/git/pr/40";

    let mut store1 = HitlVerdictStore::with_pg(app.clone());
    store1
        .open(&scope, waiting(&suffix, &gate_id, effect))
        .expect("the pending gate row INSERTs");

    let app2 = SubstrateProvider::connect(app_config(), 4)
        .await
        .expect("open a second app-role provider (simulated second process)");
    let mut store2 = HitlVerdictStore::with_pg(app2);
    let rec = store2
        .fetch(&scope, &gate_id)
        .expect("the gate row is visible across store instances/pools");
    assert_eq!(rec.state, GateState::Waiting);
    assert_eq!(rec.requested_by, "agent:claude");
    assert!(
        !rec.authorizes(effect, &rec.run_id, "agent:claude"),
        "a waiting gate authorizes nothing"
    );

    assert_eq!(
        store2.approve(&scope, &gate_id, "agent:claude", PrincipalKind::Human),
        Err(GateDecideError::SelfApproval)
    );
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

    store2
        .approve(&scope, &gate_id, "psn:lead", PrincipalKind::Human)
        .expect("a distinct eligible human approves");
    let rec = store1.fetch(&scope, &gate_id).unwrap();
    assert_eq!(rec.state, GateState::Approved);
    assert_eq!(rec.decided_by.as_deref(), Some("psn:lead"));
    assert!(rec.authorizes(effect, &rec.run_id, "agent:claude"));
    assert!(
        !rec.authorizes(
            "gate:git.merge:myelin://acme/git/pr/41",
            &rec.run_id,
            "agent:claude"
        ),
        "the approval is bound to ITS effect (never the tool name)"
    );
    assert_eq!(
        store1.approve(&scope, &gate_id, "psn:maintainer", PrincipalKind::Human),
        Err(GateDecideError::AlreadyDecided(GateState::Approved))
    );

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
        !rec2.authorizes(effect2, &rec2.run_id, "agent:claude"),
        "a rejected gate never authorizes"
    );

    let gate3 = format!("gate:{suffix}-async");
    store1
        .open(&scope, waiting(&suffix, &gate3, effect2))
        .expect("third gate opens for the async HTTP-facing seam");
    let durable = DurableHitlGateBacking::new(app.clone());
    let first = durable
        .decide(
            &scope,
            &gate3,
            GateState::Approved,
            "psn:lead",
            PrincipalKind::Human,
            200,
        )
        .await
        .expect("the tenant transaction succeeds")
        .expect("the eligible human decides the exact gate");
    assert!(first.changed);
    assert_eq!(first.record.state, GateState::Approved);

    let replay = durable
        .decide(
            &scope,
            &gate3,
            GateState::Approved,
            "psn:lead",
            PrincipalKind::Human,
            201,
        )
        .await
        .expect("the replay transaction succeeds")
        .expect("an exact decision replay is idempotent");
    assert!(!replay.changed);
    assert_eq!(replay.record.decided_at_unix, Some(200));
    assert_eq!(
        durable
            .decide(
                &scope,
                &gate3,
                GateState::Rejected,
                "psn:lead",
                PrincipalKind::Human,
                202,
            )
            .await
            .expect("the conflicting transaction itself succeeds"),
        Err(GateDecideError::AlreadyDecided(GateState::Approved)),
        "idempotency never turns a conflicting retry into a second verdict"
    );

    let gate4 = format!("gate:{suffix}-expiry");
    let mut expiring = waiting(&suffix, &gate4, effect2);
    expiring.expires_at_unix = 250;
    store1.open(&scope, expiring).expect("expiring gate opens");
    let expired = durable
        .expire_if_due(&scope, &gate4, 250)
        .await
        .expect("the expiry transaction succeeds")
        .expect("the exact due gate expires");
    assert!(expired.changed);
    assert_eq!(expired.record.state, GateState::Expired);
    let expiry_replay = durable
        .expire_if_due(&scope, &gate4, 251)
        .await
        .expect("the expiry replay transaction succeeds")
        .expect("expiry is retry-idempotent");
    assert!(!expiry_replay.changed);

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
