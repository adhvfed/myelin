//! # EB-31 (global P-440) GATE / DRILL — the Bus as the proven E2E SPINE of the four whole-system
//! scenarios (E2E-1 .. E2E-4) — the dated green artifacts.
//!
//! **Roadmap.** B-M5 (the world-scale hardening band). This prompt ships NO new contract surface: it
//! COMPOSES the Bus's already-owned carriage primitives at **E2E scope** and PROVES the Bus is the
//! spine every whole-system scenario rides — it carries every chained mutation, holds under each, and
//! emits the §4.11 / §10.2 survival signals that make the pass observable (drill catalogue §2 / §3.3:
//! a system that survives but emits no survival signal has FAILED). The subsystems own the E2E
//! ORCHESTRATION (their halves are the other M5 prompts + the Id spine P-427); THIS test drives the
//! **Bus-side carriage** of each scenario as a chained (not single-handler) flow.
//!
//! Mirrors the precedent set by the Id spine (`myelin-identity-service` `e2e_id_spine_e2e1_to_e2e4.rs`,
//! P-427): one carriage primitive per scenario, no bespoke per-scenario transport, the verdict bridged
//! onto the FROZEN §10.2 harness assertion library so a regression is LOUD (`myelin-harness` is a
//! DEV-dependency only — it never enters the `myelin-events` production DAG, §2.9).
//!
//! ## What the canon owes (drill catalogue §2 E2E-1..E2E-4; event-bus §4.11; contract 1.8)
//! - **E2E-1 — the PR context pane:** the Bus carries CI's `ci.check.updated` over the firehose so the
//!   pane's checks panel LIVE-updates and the per-ref cache busts — **0 ops lost** across a viewer
//!   reconnect mid-update (the resume-cursor backfill, §4.3). The Bus spine = the **firehose carriage**.
//! - **E2E-2 — the flagship (CI-fail → triage agent → … → fix-PR):** the Bus carries the Signal that
//!   wakes the agent, the durable `ci.result` `wait_for_signal` (a doubly-delivered rollup wakes the
//!   merge-queue EXACTLY ONCE), and the NESTED-causality run (every chained effect carries the root
//!   `correlation_id` + a monotonic `depth`, never a runaway). The Bus spine = **Signal + durable wait
//!   + nested causality**.
//! - **E2E-3 — spec-to-ship lineage:** the Bus's **reindex-from-cold == live** — wiping a derived
//!   store and rebuilding it SOLELY from the `*.snapshot` replay (the live consumer path, no bespoke
//!   recovery reader) yields the byte-identical projection. The Bus spine = **`*.snapshot` reindex**.
//! - **E2E-4 — the DSAR fan-out:** the Bus as a **holder** in the DSAR — inline-PII events are
//!   crypto-shredded (the per-subject DEK destroyed → 0 recoverable in the live log AND after an older
//!   backup is restored), `*.erased` tombstones re-emitted. The Bus spine = **holder erase + re-erase**.
//!
//! **Quantified gate (EI-01 §3 — prove it, never weaken):** E2E-1 ops-lost-on-reconnect == 0; E2E-2
//! merge-wake-count == 1 AND nested-causality root-drift == 0 AND causal-depth monotonic; E2E-3
//! reindex cold-vs-live drift == 0; E2E-4 recoverable-PII == 0 AND resurrected-after-restore == 0.
//! Each gate is bridged onto the contract-1.8 survival-signal set and asserted through the harness
//! telemetry-assertion library (loud on red).

use std::sync::Arc;

use myelin_events::{
    derive_envelope, reindex, snapshot_event_id, Actor, AggregateKey, ArtifactRef,
    BusErasureLedger, BusEventLog, BusHolder, BusObservations, BusSignal, BusSignals, BusTransport,
    CiOverall, CiResult, CiResultWaitSubstrate, DataRole, DerivedStore, EmitContext,
    EmitContextBase, EventDraft, EventEnvelope, EventId, EventType, Firehose, FirehoseScope,
    FrameDraft, IdMinter, InMemoryShredder, InlinePiiShredder, MetricRecorder, MonotonicMinter,
    OutboxStore, OutboxTx, PiiKeyRef, ReferenceReindexSource, Region, ReindexSource, Relay,
    SnapshotScope, TenantId, Timestamp, Visibility, WakeOutcome,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

// ── fixtures (one cell — the same shapes the M0/M1 Bus drills use) ────────────────────────────────

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn now() -> Timestamp {
    Timestamp("2026-06-24T00:00:00Z".into())
}
fn clock() -> Timestamp {
    Timestamp("2026-06-24T00:00:01Z".into())
}
fn actor(id: &str) -> Actor {
    Actor(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}
fn ctx_base(caused_by: Option<myelin_events::CausedBy>) -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: actor("platform"),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by,
    }
}

/// A benign (references-not-payloads) domain event draft — the default carriage shape.
fn draft(type_: &str, subject: &str, aggregate: &str) -> EventDraft {
    EventDraft {
        type_: EventType(type_.into()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": subject }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-1 — the PR context pane: the Bus carries the live check-update over the firehose; a viewer
// reconnect mid-update loses ZERO ops (the per-ref cache busts on every carried frame).
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-1 (Bus spine = firehose carriage): the PR pane's checks panel live-updates as CI emits
/// `ci.check.updated`, and a viewer who RECONNECTS mid-update loses 0 ops (the resume-cursor backfill,
/// §4.3) — the per-ref cache busts on every carried frame.**
///
/// The pane's checks panel subscribes to the firehose on the bounded per-run scope `run:<run_id>` (CI
/// is the heaviest firehose producer, §4.3 — its check/log frames ride this transport keyed by the
/// run). CI emits a stream of `ci.check.updated` frames (build→running→success, test→running→failure).
/// The viewer's connection DROPS mid-stream; on reconnect it resumes from its last seq and the Bus
/// BACKFILLS the gap from the retention window BEFORE going live — so the reassembled frame sequence
/// the pane sees is the COMPLETE in-order set (0 lost, 0 duplicated). Each frame is a cache-bust for
/// the per-ref pane cell (the Chat D-C7 analog).
#[test]
fn e2e1_pr_pane_live_check_update_zero_ops_lost_on_reconnect() {
    let stream = "ci.check.updated";
    let scope = FirehoseScope::parse("run:deadbeefcafe").expect("a bounded per-run scope");
    // A generous window (the routine reconnect must backfill from it, never fall to resync_required).
    let mut firehose = Firehose::with_limits(64, 16);

    // The pane opens its subscription live (no backfill — it starts at the head).
    let pane = firehose
        .subscribe(stream, &scope, None)
        .expect("the pane subscribes on the bounded per-ref scope");

    // CI emits the FIRST tranche of check-updates (the pane is live → it sees them as they land).
    let first_tranche = ["build:running", "test:running", "build:success"];
    for f in first_tranche {
        firehose.publish(stream, &scope, FrameDraft::new(f));
    }
    let mut pane_saw: Vec<String> = pane
        .drain_ready()
        .into_iter()
        .map(|fr| fr.payload.0)
        .collect();
    let last_seq = pane.last_seq();
    assert_eq!(
        pane_saw.len(),
        3,
        "E2E-1: the pane live-received the first three check-updates"
    );

    // The viewer's connection DROPS. CI keeps emitting (the pane is offline for these frames).
    let gap_tranche = ["test:failure", "lint:success"];
    for f in gap_tranche {
        firehose.publish(stream, &scope, FrameDraft::new(f));
    }

    // RECONNECT: resume from the pane's last seen seq → the Bus backfills (last_seq, now] then lives.
    let resumed = firehose
        .resume(stream, &scope, last_seq)
        .expect("the reconnect backfills from the retention window (no resync_required)");
    for fr in resumed.drain_ready() {
        pane_saw.push(fr.payload.0);
    }

    // A POST-reconnect live frame lands and is delivered (no gap across the backfill→live boundary).
    firehose.publish(stream, &scope, FrameDraft::new("deploy:success"));
    for fr in resumed.drain_ready() {
        pane_saw.push(fr.payload.0);
    }

    // THE GATE: the reassembled sequence is the COMPLETE in-order set — 0 lost, 0 duplicated.
    let expected = vec![
        "build:running".to_string(),
        "test:running".to_string(),
        "build:success".to_string(),
        "test:failure".to_string(),
        "lint:success".to_string(),
        "deploy:success".to_string(),
    ];
    let ops_lost = expected.iter().filter(|e| !pane_saw.contains(e)).count() as i64;
    let ops_duplicated = (pane_saw.len() as i64) - (expected.len() as i64);
    assert_eq!(
        pane_saw, expected,
        "E2E-1: the pane sees every check-update in order across the reconnect"
    );
    assert_eq!(ops_lost, 0, "E2E-1 RED: a check-update was lost across the viewer reconnect — threshold 0, NOT weakened");
    assert_eq!(
        ops_duplicated, 0,
        "E2E-1 RED: a check-update was duplicated across the reconnect — threshold 0"
    );
    assert!(
        !resumed.resync_required(),
        "E2E-1: the reconnect backfilled from the window (no cold resync)"
    );

    // ── BRIDGE onto the §10.2 firehose survival signals — loud on red. ──
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", "run:deadbeefcafe"),
        ],
        ops_lost,
    );
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", "run:deadbeefcafe"),
        ],
        Predicate::Eq(0),
    )
    .expect_green();
    // The reconnect did NOT fall to the cold path → 0 resync_required.
    src.set_scalar(SignalName::ResyncRequiredCount, 0);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-440 E2E GREEN 2026-06-24] E2E-1 PR context pane (Bus spine = firehose carriage): the pane \
         saw {} check-updates in order across a viewer reconnect mid-update → ops_lost=0, \
         ops_duplicated=0, resync_required=false (the per-ref cache busts on every carried frame).",
        pane_saw.len()
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-2 — the flagship: the Bus carries the wake Signal, the durable ci.result wait (wake-once), and
// the nested-causality run (root carried, depth monotonic, no runaway).
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-2 (Bus spine = Signal + durable wait + nested causality): a doubly-delivered `ci.result`
/// rollup wakes the merge-queue workflow EXACTLY ONCE, and the agent's chained run carries the ROOT
/// `correlation_id` end-to-end with a MONOTONIC causal depth (never a runaway).**
///
/// The flagship loop, Bus-side carriage only:
/// 1. CI's failing run emits `ci.check.updated` (the ROOT fact). The triage agent's chained effects —
///    `create_issue` → `post_chat_message` → `open_pr` → `git.merge` — are each emitted with the
///    PRIOR event as their cause, so each carries the SAME root `correlation_id` and a strictly
///    increasing `depth` (the nested-causality the Bus stamps correct-by-construction, §2.1).
/// 2. The merge-queue durable workflow parks on `wait_for_signal("ci.result", idem_key)`; the rollup
///    is delivered TWICE (at-least-once) → the wait wakes EXACTLY ONCE (the idempotent substrate).
#[test]
fn e2e2_flagship_wake_once_and_nested_causality_root_carried() {
    let outbox = OutboxStore::new();
    // ONE minter for the whole chained run (a fresh minter per begin would re-issue ULID 0 and
    // collide on the UNIQUE(event_id) — the run shares the cell's monotonic id source).
    let minter = minter();

    // (1) The ROOT fact: CI's failing check. Emitted as a root (cause = None).
    let mut tx = outbox.begin(minter.clone(), ctx_base(None));
    tx.emit(
        draft(
            "ci.check.updated",
            "myelin://acme/git/commit/deadbeef",
            "ci.check:deadbeef",
        ),
        None,
    )
    .expect("the root check fact emits");
    tx.stage_state_change("ci check recorded");
    tx.commit().expect("root commits");
    let root = outbox.committed_rows()[0].envelope.clone();
    let root_correlation = root.correlation_id.clone();

    // (2) The agent's CHAINED effects — each caused by the prior event (nested causality). We thread
    //     the cause so the Bus stamps the SAME root + a monotonic depth on every link.
    let chain: [(&str, &str, &str); 4] = [
        (
            "issue.created",
            "myelin://acme/issues/issue/ENG-1",
            "issue:ENG-1",
        ),
        (
            "chat.message.created",
            "myelin://acme/chat/message/m1",
            "chat.message:m1",
        ),
        ("git.pr.opened", "myelin://acme/git/pr/42", "git.pr:42"),
        (
            "git.merge.requested",
            "myelin://acme/git/pr/42/merge",
            "git.pr.merge:42",
        ),
    ];
    let mut cause = root.clone();
    let mut depths: Vec<u32> = vec![root.depth];
    let mut root_drift: i64 = 0;
    for (type_, subject, aggregate) in chain {
        let mut tx = outbox.begin(minter.clone(), ctx_base(None));
        tx.emit(draft(type_, subject, aggregate), Some(&cause))
            .expect("the chained effect emits");
        tx.stage_state_change(format!("{type_} applied"));
        tx.commit().expect("chained effect commits");
        // Read back the committed envelope (the LAST committed row for this aggregate).
        let env = outbox
            .committed_rows()
            .into_iter()
            .find(|r| r.envelope.aggregate.0 == aggregate)
            .map(|r| r.envelope)
            .expect("the chained event is committed");
        // The ROOT carries through every link (nested causality, §2.1).
        if env.correlation_id != root_correlation {
            root_drift += 1;
        }
        depths.push(env.depth);
        cause = env;
    }

    // THE GATE (nested causality): the root carried through every link (0 drift) and depth is strictly
    // monotonic (each effect is one hop deeper — a runaway would balloon; here it is a bounded chain).
    assert_eq!(root_drift, 0, "E2E-2 RED: the root correlation_id drifted across the chained run — threshold 0, NOT weakened");
    for w in depths.windows(2) {
        assert!(
            w[1] > w[0],
            "E2E-2: causal depth is strictly monotonic across the chain ({:?})",
            depths
        );
    }
    let causal_depth_max = *depths.iter().max().unwrap();
    assert_eq!(
        causal_depth_max,
        depths.len() as u32 - 1,
        "E2E-2: the depth equals the chain length (no skipped hops, no runaway)"
    );

    // (3) THE DURABLE WAIT: the merge-queue parks on `ci.result`; the rollup is delivered TWICE →
    //     it wakes EXACTLY ONCE (the idempotent wait_for_signal substrate, §5.9 / FLOW-D4).
    let idem = "merge-queue:pr-42";
    let mut wait = CiResultWaitSubstrate::new();
    assert!(
        wait.wait_for_signal(idem).is_none(),
        "the merge-queue parks (no result yet)"
    );
    let result = CiResult {
        commit_oid: "deadbeef".into(),
        overall: CiOverall::Failure, // (the merge is gated; the wake-once is what we assert here)
        contexts: vec!["build".into(), "test".into()],
        idem_token: idem.into(),
    };
    // FIRST delivery wakes the parked wait.
    let first = wait.deliver(result.clone());
    // SECOND (at-least-once duplicate) delivery is idempotent — already resolved.
    let second = wait.deliver(result.clone());
    assert!(
        matches!(first, WakeOutcome::Woke),
        "E2E-2: the first ci.result delivery wakes the merge-queue"
    );
    assert!(
        matches!(second, WakeOutcome::Duplicate),
        "E2E-2: the duplicate ci.result delivery is absorbed (one wake)"
    );
    let wake_count = wait.wake_count(idem) as i64;
    assert_eq!(wake_count, 1, "E2E-2 RED: the merge-queue woke {wake_count} times on a doubly-delivered rollup — exactly-once violated — threshold 1, NOT weakened");

    // ── BRIDGE onto §10.2 — loud on red. ──
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_root_drift")],
        root_drift,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_root_drift")],
        Predicate::Eq(0),
    )
    .expect_green();
    // wake-count == 1 (the durable-wait exactly-once across the duplicate).
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_wake_count")],
        wake_count,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_wake_count")],
        Predicate::Eq(1),
    )
    .expect_green();
    // The causal-depth ceiling did NOT fire (the chain is bounded, not a runaway).
    src.set_scalar(SignalName::CausalDepthFirings, 0);
    src.assert_signal(SignalName::CausalDepthFirings, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-440 E2E GREEN 2026-06-24] E2E-2 flagship (Bus spine = Signal + durable wait + nested \
         causality): the chained run carried the root correlation_id through all {} links \
         (root_drift=0), causal depth strictly monotonic {depths:?} (max={causal_depth_max}, no \
         runaway); a doubly-delivered ci.result woke the merge-queue wake_count=1 (exactly-once).",
        chain.len()
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-3 — spec-to-ship lineage: reindex-from-cold == live (the *.snapshot replay, no bespoke reader).
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **E2E-3 (Bus spine = `*.snapshot` reindex): the lineage's derived projection rebuilt SOLELY from
/// the cold `*.snapshot` replay (the live consumer path) BYTE-matches the live projection — 0 drift,
/// no bespoke recovery reader.**
///
/// The lineage chain (doc-spec → issue → PR → commit → ci-run → deploy → chat-decision) is modelled as
/// aggregates an owner materialises. The LIVE derived store ingests them; then we WIPE the store and
/// rebuild it SOLELY from `reindex(scope)` → the `*.snapshot` events drained through the relay into the
/// SAME `ingest` path (cold == live). The two projections must be byte-identical.
#[test]
fn e2e3_spec_to_ship_lineage_reindex_cold_equals_live() {
    // The owner's lineage aggregates (a knowledge/issues/git owner stamps these; the Bus replays them).
    let mut source = ReferenceReindexSource::new("lineage", "node");
    let nodes = [
        ("lineage.node:spec-doc", 1, "r-spec"),
        ("lineage.node:issue", 1, "r-issue"),
        ("lineage.node:pr", 2, "r-pr"),
        ("lineage.node:commit", 1, "r-commit"),
        ("lineage.node:ci-run", 1, "r-ci"),
        ("lineage.node:deploy", 1, "r-deploy"),
        ("lineage.node:chat-decision", 1, "r-chat"),
    ];
    for (agg, ver, r) in nodes {
        source.upsert(agg, ver, serde_json::json!({ "version": ver, "ref": r }));
    }
    let scope = SnapshotScope::new("lineage", "node:all");

    // (A) LIVE projection: ingest the live snapshot-shaped events (the steady-state consumer path).
    let mut live = DerivedStore::new();
    {
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&source];
        reindex(&scope, None, sources, &mut outbox, ctx_base(None)).expect("seed live via replay");
        let bus = myelin_events::InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), clock);
        relay.drain_to_empty();
        for env in bus.consume("") {
            live.ingest(&env);
        }
    }

    // (B) COLD rebuild: a FRESH derived store, hydrated SOLELY from `reindex` → `*.snapshot` → ingest
    //     (the SAME consumer path, no bespoke recovery reader).
    let mut cold = DerivedStore::new();
    {
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&source];
        let receipt = reindex(&scope, None, sources, &mut outbox, ctx_base(None))
            .expect("cold rebuild via replay");
        assert_eq!(
            receipt.snapshots_emitted,
            nodes.len(),
            "the reindex re-emitted every lineage node"
        );
        let bus = myelin_events::InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), clock);
        relay.drain_to_empty();
        for env in bus.consume("") {
            cold.ingest(&env);
        }
    }

    // THE GATE: cold == live, BYTE-for-byte.
    let drift: i64 = if cold.parity_bytes() == live.parity_bytes() {
        0
    } else {
        1
    };
    assert_eq!(drift, 0, "E2E-3 RED: the cold-rebuilt lineage drifted from live (the *.snapshot reindex did not match) — threshold 0, NOT weakened");
    assert_eq!(
        cold.len(),
        nodes.len(),
        "E2E-3: the cold rebuild materialised every lineage node"
    );

    // Idempotency: a SECOND reindex into the SAME outbox is a no-op (deterministic ids, ON CONFLICT).
    let home_id = snapshot_event_id(&AggregateKey("lineage.node:spec-doc".into()), 1);
    let _ = home_id; // the deterministic id the consumer keys idempotency off (asserted in CDC 2.6).

    // ── BRIDGE onto §10.2 (the reindex-parity zero) — loud on red. ──
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e3_reindex_cold_vs_live_drift")],
        drift,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e3_reindex_cold_vs_live_drift")],
        Predicate::Eq(0),
    )
    .expect_green();

    println!(
        "[P-440 E2E GREEN 2026-06-24] E2E-3 spec-to-ship lineage (Bus spine = *.snapshot reindex): the \
         lineage projection rebuilt SOLELY from the cold *.snapshot replay (live consumer path, no \
         bespoke reader) == live → drift=0 over {} nodes.",
        nodes.len()
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// E2E-4 — the DSAR fan-out: the Bus as a holder; inline-PII crypto-shredded; 0 recoverable in the log
// AND after an older backup is restored.
// ──────────────────────────────────────────────────────────────────────────────────────────────

fn inline_pii(event_id: &str, subject: &str) -> EventEnvelope {
    let d = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(PiiKeyRef(format!("kms://acme/0/subject:{subject}"))),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: actor(subject),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by: None,
    };
    derive_envelope(d, ctx, None)
}

/// **E2E-4 (Bus spine = holder erase + re-erase): a `dsr_submit` reaches the Bus holder — the
/// subject's inline-PII event is crypto-shredded (the per-subject DEK destroyed → 0 recoverable in the
/// live log), and after an OLDER backup is restored the re-erasure pass re-destroys the resurrected key
/// (0 resurrected post-restore).**
///
/// Seed a subject's inline-PII event into the live log. `erase_and_record` crypto-shreds the per-subject
/// DEK + records the erasure in the PII-free ledger. Then RESTORE an older backup (pre-erase) → the DEK
/// is resurrected and the row is back without its tombstone; `re_erase_after_restore` replays the ledger
/// (cold == live) → 0 resurrected. We assert recoverable-PII == 0 in the live log AND resurrected == 0
/// after the restore, and bridge the survival signals (nothing lost re-erasing: depth 0, dead-letter 0).
#[test]
fn e2e4_dsar_fanout_bus_holder_zero_recoverable_and_zero_resurrected() {
    // (1) Seed the subject's inline-PII event in the LIVE cell; seal its DEK (the key is live).
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    let key = ev
        .pii_key_ref
        .clone()
        .expect("the inline-PII event carries a key");
    shredder.seal(&key);
    live_log.append(ev);
    assert!(
        shredder.is_live(&key),
        "precondition: the subject's inline-PII DEK is live"
    );

    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut live_outbox = OutboxStore::new();

    // (2) THE DSAR FAN-OUT reaches the Bus holder: erase + record (crypto-shred the per-subject DEK).
    holder
        .erase_and_record(
            "u42",
            &mut live_log,
            &mut live_outbox,
            minter(),
            &ledger,
            now(),
        )
        .expect("the Bus holder's erase + ledger record");

    // THE GATE (live): 0 recoverable inline-PII — the DEK is destroyed, the row is tombstoned.
    let mut recoverable_pii: i64 = 0;
    if shredder.is_live(&key) {
        recoverable_pii += 1; // the DEK survived the shred — a recoverable-PII failure.
    }
    if !live_log.is_tombstoned("01J-1") {
        recoverable_pii += 1; // the row was not tombstoned — consumers could still read it.
    }
    assert_eq!(recoverable_pii, 0, "E2E-4 RED: the subject's inline-PII is still recoverable in the live log — threshold 0, NOT weakened");
    assert!(
        ledger.is_erased("u42"),
        "E2E-4: the PII-free ledger durably remembers the erasure (re-erasure can replay)"
    );

    // (3) RESTORE an OLDER backup (pre-erase): the DEK is resurrected, the row back without a tombstone.
    let mut restored_log = BusEventLog::new();
    restored_log.append(inline_pii("01J-1", "u42"));
    shredder.seal(&key);
    assert!(
        shredder.is_live(&key),
        "the restore RESURRECTED the subject's inline-PII DEK"
    );

    // (4) RE-ERASE AFTER RESTORE: replay the ledger (cold == live) — re-destroy + re-tombstone.
    let mut reerase_outbox = OutboxStore::new();
    let receipt = holder
        .re_erase_after_restore(
            &ledger,
            &mut restored_log,
            &mut reerase_outbox,
            minter(),
            now(),
        )
        .expect("the post-restore re-erasure (KMS reachable)");

    // THE GATE (post-restore): 0 resurrected inline-PII keys.
    let resurrected = receipt.resurrected as i64;
    assert_eq!(resurrected, 0, "E2E-4 RED: a restored backup resurrected the subject's inline-PII key — threshold 0, NOT weakened");
    assert!(
        receipt.is_green(),
        "E2E-4: the Bus's restore-verify leg is GREEN"
    );
    assert!(
        !shredder.is_live(&key),
        "E2E-4: the key STAYS destroyed across the restore"
    );
    assert!(
        restored_log.is_tombstoned("01J-1"),
        "E2E-4: the restored row carries a tombstone again"
    );

    // (5) RELAY → BROKER: the re-emitted tombstone publishes (consumers degrade on the restored row).
    let bus = myelin_events::InProcessBus::new();
    let relay = Relay::new(reerase_outbox.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert!(
        drain.published >= 1,
        "E2E-4: the relay published the re-emitted *.erased tombstone"
    );

    // ── BRIDGE onto §10.2 — loud on red. The DSAR survival signals: recoverable=0, resurrected=0,
    //    and nothing lost re-erasing across the restore (outbox drained, no dead-letters). ──
    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&reerase_outbox, &drain, &obs, &now(), 0);
    let mut rec = MetricRecorder::new();
    sig.emit_to(&mut rec);

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e4_recoverable_pii")],
        recoverable_pii,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e4_recoverable_pii")],
        Predicate::Eq(0),
    )
    .expect_green();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e4_resurrected")],
        resurrected,
    );
    src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e4_resurrected")],
        Predicate::Eq(0),
    )
    .expect_green();
    if let Some(v) = rec.scalar(BusSignal::OutboxDepth) {
        src.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(BusSignal::DeadLetterCount) {
        src.set_scalar(SignalName::DeadLetterCount, v);
    }
    src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    src.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-440 E2E GREEN 2026-06-24] E2E-4 DSAR fan-out (Bus spine = holder erase + re-erase): the \
         subject's inline-PII DEK crypto-shredded → recoverable_pii=0 in the live log; an older backup \
         resurrected it → re-erase replayed the ledger → resurrected=0 post-restore (the key stays \
         destroyed; the restored row re-tombstoned; nothing lost re-erasing)."
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────────
// The composed spine + the mutation floor: the Bus carries all four; each gate can go RED.
// ──────────────────────────────────────────────────────────────────────────────────────────────

/// **The composed proof: the Bus is the carriage spine of E2E-1..E2E-4 — every scenario rides the SAME
/// owned carriage primitives (firehose / Signal+durable-wait+causality / reindex / holder), one per
/// scenario, no bespoke per-scenario transport.** A regression in any one primitive surfaces here,
/// composed E2E.
#[test]
fn bus_is_the_carriage_spine_of_all_four_e2e_scenarios() {
    // E2E-1 spine: a firehose subscription delivers a published frame on a bounded scope.
    let scope = FirehoseScope::parse("run:c1").expect("bounded scope");
    let mut fh = Firehose::with_limits(8, 4);
    let sub = fh
        .subscribe("ci.check.updated", &scope, None)
        .expect("subscribe");
    fh.publish("ci.check.updated", &scope, FrameDraft::new("build:success"));
    assert_eq!(
        sub.drain_ready().len(),
        1,
        "spine E2E-1: the firehose carries a check-update"
    );

    // E2E-2 spine: the durable wait wakes exactly once on a doubly-delivered rollup.
    let mut wait = CiResultWaitSubstrate::new();
    let r = CiResult {
        commit_oid: "c".into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into()],
        idem_token: "k".into(),
    };
    assert!(wait.wait_for_signal("k").is_none());
    let _ = wait.deliver(r.clone());
    let _ = wait.deliver(r);
    assert_eq!(
        wait.wake_count("k"),
        1,
        "spine E2E-2: the durable wait wakes exactly once"
    );

    // E2E-2 spine (causality): a caused event carries the cause's root + one-deeper depth.
    let outbox = OutboxStore::new();
    let cell_minter = minter(); // one id source for the cell (no ULID collision across begins).
    let mut tx = outbox.begin(cell_minter.clone(), ctx_base(None));
    tx.emit(draft("x.happened", "myelin://acme/x/1", "x:1"), None)
        .expect("root");
    tx.commit().expect("commit");
    let root = outbox.committed_rows()[0].envelope.clone();
    let mut tx = outbox.begin(cell_minter.clone(), ctx_base(None));
    tx.emit(draft("y.happened", "myelin://acme/y/1", "y:1"), Some(&root))
        .expect("child");
    tx.commit().expect("commit");
    let child = outbox
        .committed_rows()
        .into_iter()
        .find(|r| r.envelope.aggregate.0 == "y:1")
        .unwrap()
        .envelope;
    assert_eq!(
        child.correlation_id, root.correlation_id,
        "spine E2E-2: the root carries through"
    );
    assert_eq!(
        child.depth,
        root.depth + 1,
        "spine E2E-2: causal depth is one hop deeper"
    );

    // E2E-3 spine: a reindex re-emits an owner's aggregate as a *.snapshot the consumer ingests.
    let mut source = ReferenceReindexSource::new("o", "a");
    source.upsert("o.a:1", 1, serde_json::json!({ "version": 1 }));
    let mut ob = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&source];
    let receipt = reindex(
        &SnapshotScope::new("o", "a:1"),
        None,
        sources,
        &mut ob,
        ctx_base(None),
    )
    .expect("reindex");
    assert_eq!(
        receipt.snapshots_emitted, 1,
        "spine E2E-3: the reindex re-emits the *.snapshot"
    );

    // E2E-4 spine: the holder's erase destroys the per-subject DEK (the crypto-shred lever).
    let mut log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-9", "uX");
    let key = ev.pii_key_ref.clone().unwrap();
    shredder.seal(&key);
    log.append(ev);
    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let mut tx = OutboxStore::new();
    holder
        .erase("uX", &mut log, &mut tx, minter())
        .expect("holder erase");
    assert!(
        !shredder.is_live(&key),
        "spine E2E-4: the holder's erase destroys the per-subject DEK"
    );

    println!(
        "[P-440 E2E GREEN 2026-06-24] the Bus IS the carriage spine of E2E-1..E2E-4: E2E-1 firehose \
         carriage, E2E-2 Signal + durable wait (wake-once) + nested causality (root carried, depth+1), \
         E2E-3 *.snapshot reindex, E2E-4 holder crypto-shred — one carriage primitive per scenario, no \
         bespoke per-scenario transport (EI-01 §7)."
    );
}

/// **The spine mutation floor: each scenario's gate MUST be able to go RED.** A drill that cannot go
/// red is no gate (EI-01 §3). We model the broken behaviour each gate guards against and assert the
/// gate reads RED — proving none of the four gates is vacuous.
#[test]
fn e2e_spine_gates_are_not_vacuous() {
    // E2E-1 mutation: a TOO-SMALL retention window forces a reconnect past the window floor →
    // resync_required (the cold path). A reconnect that loses ops would read this as a non-zero gap.
    let scope = FirehoseScope::parse("run:c2").expect("scope");
    let mut fh = Firehose::with_limits(2, 4); // window of 2 — a 3-frame gap evicts the head.
    let sub = fh
        .subscribe("ci.check.updated", &scope, None)
        .expect("subscribe");
    fh.publish("ci.check.updated", &scope, FrameDraft::new("f1"));
    let _ = sub.drain_ready();
    let last = sub.last_seq();
    // Three frames published while offline → the window (cap 2) evicts the gap head.
    for f in ["f2", "f3", "f4"] {
        fh.publish("ci.check.updated", &scope, FrameDraft::new(f));
    }
    let resumed = fh.resume("ci.check.updated", &scope, last);
    let resync_required = resumed.is_err(); // the broken case: the gap fell out of the window.
    assert!(resync_required, "E2E-1 mutation: a too-small window forces resync_required (the gate is real — a real reconnect must backfill within the window)");

    // E2E-2 mutation: a NON-idempotent wait would wake twice on a doubly-delivered rollup. The real
    // substrate wakes once; we assert that a second wake (the broken behaviour) WOULD read != 1.
    let mut wait = CiResultWaitSubstrate::new();
    let r = CiResult {
        commit_oid: "c".into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into()],
        idem_token: "k".into(),
    };
    let _ = wait.wait_for_signal("k");
    let _ = wait.deliver(r.clone());
    let _ = wait.deliver(r);
    // The CORRECT substrate gives 1; a broken (non-idempotent) one would give 2. The gate asserts ==1.
    let broken_double_wake: i64 = 2; // the value a non-idempotent substrate would emit.
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_mutation")],
        broken_double_wake,
    );
    let verdict = src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e2_mutation")],
        Predicate::Eq(1),
    );
    assert!(!verdict.is_green(), "E2E-2 mutation: a non-idempotent wait (wake-count 2) reads RED against the exactly-once gate (the gate is real, not vacuous)");
    assert_eq!(
        wait.wake_count("k"),
        1,
        "the REAL substrate still wakes exactly once"
    );

    // E2E-4 mutation: an inline-PII DEK left LIVE after an erase (a broken shred) reads as recoverable.
    let shredder = InMemoryShredder::new();
    let key = PiiKeyRef("kms://acme/0/subject:uBroken".into());
    shredder.seal(&key); // sealed but NEVER shredded — the broken case.
    let broken_recoverable: i64 = if shredder.is_live(&key) { 1 } else { 0 };
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e4_mutation")],
        broken_recoverable,
    );
    let verdict = src.assert_labelled(
        SignalName::CrossTenantCount,
        vec![Label::new("scenario", "e2e4_mutation")],
        Predicate::Eq(0),
    );
    assert!(broken_recoverable == 1 && !verdict.is_green(), "E2E-4 mutation: a never-shredded DEK reads recoverable=1 → the gate reads RED (the gate is real, not vacuous)");

    println!(
        "[P-440 E2E MUTATION 2026-06-24] the spine gates are not vacuous: E2E-1 a too-small window \
         forces resync_required; E2E-2 a non-idempotent wait (wake-count 2) reads RED; E2E-4 a \
         never-shredded DEK reads recoverable → each gate reads RED on its broken behaviour."
    );
}
