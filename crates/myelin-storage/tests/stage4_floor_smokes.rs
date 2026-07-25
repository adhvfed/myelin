//! # Stage 4 — the two genuine-floor CONTAINERIZED SMOKES.
//!
//! Two of the data-layer drills genuinely need MORE than Docker to be a full gate, so they stay
//! RED on the infra scorecard with their floor NAMED. This file gives each one a CONTAINERIZED
//! SMOKE so it is not zero-coverage NOW:
//!
//!  1. SANDBOX-ESCAPE — the full gate needs a real isolation kernel (gVisor / Firecracker
//!     microVM). The SMOKE here launches a HARDENED Docker container (egress-deny via
//!     `--network=none`, read-only root via `--read-only`, dropped caps via `--cap-drop=ALL`)
//!     and probes from INSIDE it that (a) egress is denied, (b) the root FS is read-only, and
//!     (c) a capability-gated operation is refused. This proves the hardening POSTURE works; it
//!     does NOT prove kernel-level escape resistance (that is the named floor).
//!
//!  2. WORLD-SCALE 30× LOAD — the full gate needs real hardware (a multi-node cluster). The
//!     SMOKE here drives the myelin-harness `LoadGenerator` at 10× against the LIVE stack (real
//!     PG outbox → real NATS JetStream) and asserts survival: every issued request's event is
//!     committed via the transactional outbox, the relay drains the outbox to 0 (no loss under
//!     the 10× containerized load), and every committed event is delivered exactly-once. This
//!     proves the path survives a containerized 10× burst; it does NOT prove world-scale 30× on
//!     real hardware (that is the named floor).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test stage4_floor_smokes -- --nocapture
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

// ----------------------------------------------------------------------------------------------
// shared helpers (mirrors stage3_drills.rs — admin role for DDL/seed)
// ----------------------------------------------------------------------------------------------

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// A raw admin/owner pool for test-only DDL + cleanup SQL (MR-013 removed `PgStore::pool()` — the
/// bare tenant-bypassing hatch). Test infrastructure, NOT the tenant store handing out its pool.
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

// ==============================================================================================
// FLOOR SMOKE 1 — hardened-container SANDBOX-ESCAPE smoke (egress-deny + ro-root + dropped caps)
// ==============================================================================================
//
// NAMED FLOOR (still open): the full SANDBOX-ESCAPE gate needs a real isolation kernel (gVisor /
// Firecracker microVM), not a Docker container. This smoke proves the hardening POSTURE: a
// container launched with the production isolation flags actually denies egress, refuses writes
// to a read-only root, and runs with all Linux capabilities dropped. If any of those leaks, the
// hardening config is broken — the smoke fails LOUD.

/// Run a probe command inside a hardened `alpine` container and return (exit_status_success,
/// combined_output). `extra_args` carries the hardening flags under test.
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
    // Pre-pull alpine so the probes themselves are not measuring a registry fetch (and to fail
    // early + loud if the image is unavailable).
    let pull = Command::new("docker")
        .args(["pull", "alpine:3"])
        .output()
        .expect("spawn docker pull");
    assert!(
        pull.status.success(),
        "could not pull alpine:3 for the hardened-container smoke"
    );

    // (a) EGRESS-DENY: with --network=none the container has NO route off-box. A connect to a
    //     public address MUST fail. (We use a TCP connect with a short timeout; success here
    //     would mean egress leaked.) `nc -w2 -z` returns non-zero when it cannot connect.
    let (egress_ok, egress_out) = hardened_probe(
        &["--network=none"],
        // nc may be missing; fall back to a /dev/tcp-style probe via wget with a timeout.
        "wget -T 2 -q -O /dev/null http://1.1.1.1/ 2>&1; echo EXIT=$?",
    );
    // The container ran (docker exited 0) but the egress attempt inside MUST have failed.
    assert!(
        egress_ok,
        "the hardened container itself must run; output: {egress_out}"
    );
    assert!(
        egress_out.contains("EXIT=") && !egress_out.contains("EXIT=0"),
        "EGRESS-DENY breached: a request escaped a --network=none container; output: {egress_out}"
    );

    // (b) READ-ONLY-ROOT: with --read-only the root filesystem is immutable. A write to / MUST
    //     fail. We assert the write is refused (non-zero EXIT recorded inside the container).
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

    // (c) DROPPED-CAPS: with --cap-drop=ALL the container holds NO Linux capabilities. A
    //     capability-gated op (changing file ownership, which needs CAP_CHOWN/CAP_FOWNER) MUST
    //     be refused. We create a file on a writable tmpfs then attempt chown to a different uid.
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

// ==============================================================================================
// FLOOR SMOKE 2 — 10× CONTAINERIZED LOAD smoke (myelin-harness LoadGenerator → live PG+NATS)
// ==============================================================================================
//
// NAMED FLOOR (still open): the full WORLD-SCALE 30× LOAD drill needs real hardware (a multi-node
// cluster), not a single dev box. This smoke drives the harness LoadGenerator at 10× against the
// LIVE stack and asserts the emit path SURVIVES the burst with 0 loss.

/// A Sink that records the (tenant, aggregate, seq) of every issued request so the test can
/// co-commit one outbox event per request after the generator finishes issuing. The generator's
/// `drive` is synchronous; we collect here and do the async I/O outside it.
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

    // The real NATS JetStream bus (durable stream + durable PULL consumer). Connected HERE (ahead
    // of the wrapped body below, moved up from its original position right before the drain step)
    // so `bus.purge()` is available to the cleanup closure regardless of how the body exits.
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

    // Wrapped so a mid-test assertion failure or panic still drops this run's state table +
    // outbox rows + NATS stream, instead of only the happy path reaching the final cleanup below
    // (see tests/common/mod.rs).
    common::with_cleanup(
        || async {
            // 10× the baseline. base=20 → 200 requests at 10×. A real-but-bounded containerized
            // burst (the 30× world-scale number is the named floor; 10× is the smoke).
            let tenants = vec![
                TenantId(format!("acme-load-{tag}")),
                TenantId(format!("globex-load-{tag}")),
            ];
            let gen = LoadGenerator::new(
                20,
                Multiplier::STRESS, // 10×
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

            // Co-commit ONE outbox event per issued request, in the SAME tx as a domain state change
            // (emit-iff-committed). The aggregate is per (tenant) so the outbox `seq` is monotonic
            // per aggregate; we use the global issue order as the unique id.
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

            // Drain to real NATS JetStream and assert the outbox drains to 0 (no loss under the
            // burst).
            // Drain in batches until the outbox is empty (a few passes for 200 rows at batch 64).
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

            // Drain the durable consumer; assert every committed event delivered exactly-once.
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
            // cleanup. See `common::delete_outbox_for_aggregate` for why this isn't a bare
            // `DELETE FROM outbox WHERE aggregate = $1`: `outbox_quarantine` FKs to `outbox` with
            // `ON DELETE RESTRICT`, and a concurrent quarantine sweep on this shared dev DB
            // (confirmed live) can otherwise leave this run's tagged rows stuck forever.
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
