#![cfg(feature = "integration")]

use std::process::Command;

use myelin_config::MyelinConfig;
use myelin_storage::pg::PgStore;

use myelin_events::nats::NatsJetStreamBus;
use myelin_events::relay::BusTransport;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

use myelin_harness::load_generator::{LoadGenerator, Multiplier, PrincipalMix, Sink, StormProfile};

mod common;

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn admin_pool(cfg: &MyelinConfig) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&admin_url(cfg))
        .await
        .expect("connect admin pool (is the stack up?)")
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn envelope(id: &str, tenant: &TenantId, aggregate: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(format!("myelin://{}/issues/{id}", tenant.0)),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: Some(CausedBy("session:load".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "x" }),
    }
}

fn hardened_probe(extra_args: &[&str], sh: &str) -> (bool, String) {
    let mut args: Vec<String> = vec!["run".into(), "--rm".into()];
    for a in extra_args {
        args.push((*a).into());
    }
    args.push("alpine:3".into());
    args.push("sh".into());
    args.push("-c".into());
    args.push(sh.into());
    let out = Command::new("docker")
        .args(&args)
        .output()
        .expect("spawn docker (is the daemon reachable?)");
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), combined)
}

#[test]
fn sandbox_escape_containerized_smoke() {
    let pull = Command::new("docker")
        .args(["pull", "alpine:3"])
        .output()
        .expect("spawn docker pull");
    assert!(
        pull.status.success(),
        "could not pull alpine:3 for the hardened-container smoke"
    );

    let (egress_ok, egress_out) = hardened_probe(
        &["--network=none"],
        "wget -T 2 -q -O /dev/null http://1.1.1.1/ 2>&1; echo EXIT=$?",
    );
    assert!(
        egress_ok,
        "the hardened container itself must run; output: {egress_out}"
    );
    assert!(
        egress_out.contains("EXIT=") && !egress_out.contains("EXIT=0"),
        "EGRESS-DENY breached: a request escaped a --network=none container; output: {egress_out}"
    );

    let (roroot_ok, roroot_out) = hardened_probe(
        &["--read-only", "--network=none"],
        "touch /should-not-write 2>&1; echo EXIT=$?",
    );
    assert!(
        roroot_ok,
        "the hardened container itself must run; output: {roroot_out}"
    );
    assert!(
        roroot_out.contains("EXIT=") && !roroot_out.contains("EXIT=0"),
        "READ-ONLY-ROOT breached: a write to / succeeded in a --read-only container; output: {roroot_out}"
    );

    let (caps_ok, caps_out) = hardened_probe(
        &[
            "--cap-drop=ALL",
            "--network=none",
            "--user=1000:1000",
            "--read-only",
            "--tmpfs=/tmp",
        ],
        "touch /tmp/f 2>&1; chown 0:0 /tmp/f 2>&1; echo EXIT=$?",
    );
    assert!(
        caps_ok,
        "the hardened container itself must run; output: {caps_out}"
    );
    assert!(
        caps_out.contains("EXIT=") && !caps_out.contains("EXIT=0"),
        "DROPPED-CAPS breached: a cap-gated chown succeeded with --cap-drop=ALL; output: {caps_out}"
    );

    println!(
        "[2026-06-19] PASS  smoke=SANDBOX-ESCAPE-CONTAINERIZED  egress-deny=ok read-only-root=ok \
         dropped-caps=ok  (hardened-container POSTURE asserted)  FLOOR-STILL-OPEN: the full \
         real-kernel SANDBOX-ESCAPE gate needs gVisor / Firecracker microVM, not Docker"
    );
}

#[derive(Default)]
struct CollectingSink {
    issued: Vec<(TenantId, u64)>,
}

impl Sink for CollectingSink {
    fn handle(&mut self, request: &myelin_harness::load_generator::Request) {
        self.issued.push((request.tenant.clone(), request.seq));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_10x_containerized_smoke() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 8)
        .await
        .expect("connect Postgres (is the stack up?)");
    store.migrate().await.expect("run migrations");
    let pool = admin_pool(&cfg).await;

    let tag = format!("{}-{}", std::process::id(), 0);
    let state_table = format!("load10x_state_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {state_table} (id text PRIMARY KEY, event_id text NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create load state table");
    let agg = format!("issue:LOAD10X-{tag}");

    let stream = format!("MYELIN_LOAD10X_{}", tag.replace('-', "_"));
    let subject_root = format!("myelin_load10x_{}", tag.replace('-', "_"));
    let consumer = format!("{stream}_pull");
    let bus = NatsJetStreamBus::connect(
        &cfg.nats_url,
        &stream,
        &subject_root,
        &consumer,
        tokio::runtime::Handle::current(),
    )
    .expect("connect NATS JetStream bus (is the stack up with -js?)");
    tokio::task::block_in_place(|| bus.purge());

    common::with_cleanup(
        || async {
            let tenants = vec![
                TenantId(format!("acme-load-{tag}")),
                TenantId(format!("globex-load-{tag}")),
            ];
            let gen = LoadGenerator::new(
                20,
                Multiplier::STRESS,
                PrincipalMix::balanced(),
                StormProfile::collab_op_stream(),
                tenants.clone(),
            )
            .expect("non-empty tenant list");

            let mut sink = CollectingSink::default();
            gen.drive(&mut sink);
            let total = gen.total_requests() as usize;
            assert_eq!(
                sink.issued.len(),
                total,
                "the generator issued exactly base*10 requests"
            );
            assert_eq!(
                total, 200,
                "10× of base=20 is 200 requests (the containerized burst)"
            );

            let relay = store.relay();
            let mut committed: std::collections::HashSet<EventId> = std::collections::HashSet::new();
            for (i, (tenant, _req_seq)) in sink.issued.iter().enumerate() {
                let id = format!("load10x-evt-{tag}-{i}");
                relay
                    .enqueue_with_state(
                        &state_table,
                        &format!("state-{i}"),
                        &agg,
                        i as i64,
                        &envelope(&id, tenant, &agg),
                    )
                    .await
                    .expect("co-commit state + outbox row under load");
                committed.insert(EventId(id));
            }
            assert_eq!(
                relay.outbox_depth().await.expect("depth") as usize,
                total,
                "all {total} load events durably committed + unsent before the drain"
            );

            let mut published_total = 0usize;
            for _ in 0..16 {
                let n = relay
                    .relay_once(&bus, 64)
                    .await
                    .expect("relay drain pass under load");
                published_total += n as usize;
                if relay.outbox_depth().await.expect("depth") == 0 {
                    break;
                }
            }
            assert_eq!(
                relay.outbox_depth().await.expect("final depth"),
                0,
                "the outbox drained to 0 under the 10× containerized load (no loss)"
            );
            assert!(
                published_total >= total,
                "every committed event was published (>= {total}); got {published_total}"
            );

            let mut delivered: std::collections::HashMap<EventId, usize> =
                std::collections::HashMap::new();
            for _ in 0..16 {
                let batch = tokio::task::block_in_place(|| bus.consume(&subject_root));
                if batch.is_empty() {
                    break;
                }
                for env in &batch {
                    *delivered.entry(env.event_id.clone()).or_insert(0) += 1;
                    tokio::task::block_in_place(|| bus.ack(&consumer, &env.event_id));
                }
            }
            let delivered_ids: std::collections::HashSet<EventId> =
                delivered.keys().cloned().collect();
            assert_eq!(
                delivered_ids, committed,
                "0 lost under 10× load: the delivered set equals exactly the committed set"
            );
            for (id, count) in &delivered {
                assert_eq!(
                    *count, 1,
                    "0 ghost under 10× load: {id:?} delivered exactly once"
                );
            }

            println!(
                "[2026-06-19] PASS  smoke=LOAD-10X-CONTAINERIZED  multiplier=10x base=20 issued={total} \
                 committed={total} delivered={total} lost=0 ghost=0 outbox_depth=0  \
                 backend=real-PG+real-NATS-JetStream  tenants={}  FLOOR-STILL-OPEN: the WORLD-SCALE 30x \
                 LOAD drill needs real hardware (multi-node cluster), not a single dev box",
                tenants.len()
            );
        },
        || async {
            common::delete_outbox_for_aggregate(&pool, &agg).await;
            sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {state_table}"))
                .execute(&pool)
                .await
                .ok();
            tokio::task::block_in_place(|| bus.purge());
        },
    )
    .await;
}
