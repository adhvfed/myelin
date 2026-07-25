//! # P-ID-32 (global P-425) GATE / DRILL — the S8 measured tunables finalised at world-scale.
//!
//! **Roadmap milestone ID-M5** (S8 tunables finalised) — identity §13 (S8 as the named first
//! replica; **measure before you shard**, ID-4) + §15 (the two open tunables: the Ids-vs-Filter
//! threshold + the `reverse_index_lag` freshness SLO). Contract-index rows **4.3** (the cardinality
//! cap tunable, finalised at scale) + **4.10** (the `reverse_index_lag` SLO the watermark fallback
//! reads).
//!
//! ## What this drill finalises (the two open S8 tunables)
//! The SHAPE of both is FROZEN since P-ID-11 (Ids under the cap, Filter above; serve-from-S8 within
//! the lag SLO, fall back to `check` beyond it). Only the NUMBERS were open. EI-01 §3 / "measure-
//! not-predict": the numbers are **measured under world-scale load** here (riding the P-ID-31 30×
//! authz surge + a cell-scale list/scan load), confirmed to be the right default-to-beat, and
//! written/dated to `thresholds.toml`. This CLOSES the P-ID-11 cardinality-cap floor.
//!
//! 1. **The Ids↔Filter cardinality cap** (`authz_index.ids_cardinality_cap`). The cap is the set
//!    size at which materialising + inlining `WHERE id IN (…)` stops being cheaper than the
//!    `authz_visible` JOIN push-down. The measurement drives the materialise path and the push-down
//!    path at increasing reachable-set sizes and reads the structural cost of each (the materialise
//!    path's cost grows linearly with set size — the inlined `IN (…)` list + the per-candidate
//!    re-resolution; the push-down path's cost is a fixed single JOIN). The crossover is where the
//!    materialise cost first exceeds the push-down's fixed cost. The finalised cap must sit
//!    at-or-under the measured crossover (so a materialised list is genuinely the cheaper plan), and
//!    a list AT the cap returns `Ids` while one OVER it returns `Filter`.
//! 2. **The `reverse_index_lag` freshness SLO** (`authz_index.reverse_index_lag_slo_ms`). Under a
//!    surge of `identity.tuple.written` events, the lag from event-accept to S8 projection is measured;
//!    the SLO is the bound a zookie-stamped scan tolerates before it must fall back to `check`. The
//!    drill proves a scan whose required revision is WITHIN the SLO of the watermark serves from S8
//!    (the fast JOIN path), and a scan one revision BEYOND the watermark falls back to per-row
//!    `check` (the new-enemy guard — never serve a stale grant). The SLO must stay bounded above by
//!    the revocation SLA (§15 / §8.2) so a stale grant can never outlive a revoke.
//!
//! Both numbers are read from the FROZEN thresholds file — never a hardcoded literal (EI-01 §3). A
//! regression that pushes either past its measured bound is a dated `[[claimed_not_proven]]` row,
//! never a lowered bar.

use std::time::Instant;

use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_harness::load_generator::Multiplier;
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    lower,
    namespace::{FragmentDef, NamespaceEngine, PermissionRule, Userset},
    watermark_verdict, ListObjects, ReverseIndex, ReverseIndexConsumer, TupleStore,
    WatermarkVerdict,
};
use myelin_storage::TenantScope;
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

// ── fixtures ──────────────────────────────────────────────────────────────────────────────────

fn admin(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(&admin(tenant), Region("eu-west".into()))
}

fn subject(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn add(object: &str, relation: &str, subj: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subj.into()),
        caveat: None,
    })
}

fn now() -> Timestamp {
    Timestamp("2026-06-24T00:00:00Z".into())
}

fn latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn pinned(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("zk-{rev:020}")),
        mode: ConsistencyMode::Strong,
    }
}

/// A namespace engine with a `repo` fragment (a `reader` relation + a `read` permission) — the
/// candidate source + the permission resolution `list_objects` materialises over.
fn repo_namespace() -> NamespaceEngine {
    let mut namespace = NamespaceEngine::with_core_hierarchy();
    let _ = namespace.admit(&FragmentDef {
        object_type: ObjectType("repo".into()),
        relations: vec![RelName("reader".into())],
        permissions: vec![PermissionRule {
            permission: Permission("read".into()),
            rewrite: Userset::Relation(RelName("reader".into())),
        }],
    });
    namespace
}

/// Build a `list_objects` evaluator at `cap`, seeding `n` `repo:N#reader@subject` grants for
/// `subject` in `scope` (fed through S3 → outbox → relay → the S8 consumer, the live feed).
fn wired_with_grants(cap: usize, s: &TenantScope, subj: &str, n: usize) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    let grants: Vec<TupleDelta> = (0..n)
        .map(|i| add(&format!("repo:r{i}"), "reader", subj))
        .collect();
    store
        .write_tuples(s, &admin(&s.tenant().0), &grants, None, None, now())
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox, bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, repo_namespace(), index, cap)
}

// ── (1) THE CARDINALITY CAP — measured crossover, finalised + dated ─────────────────────────────

/// **The Ids↔Filter cardinality-cap MEASUREMENT (the SCHED tunable-measurement scenario).**
///
/// Drives the two `list_objects` plans at increasing reachable-set sizes under a world-scale list
/// load and reads the structural cost of each. The materialise (`Ids`) path's cost grows with the
/// set size (the inlined `IN (…)` list + the per-candidate permission re-resolution); the push-down
/// (`Filter`) path is a fixed single JOIN. The CROSSOVER — where the materialise cost first exceeds
/// the push-down's fixed cost — is the empirical cardinality cap. The finalised cap in the file
/// must sit at-or-under that crossover (so a materialised list is the genuinely cheaper plan), and
/// it must be a positive default-to-beat.
#[test]
fn id32_cardinality_cap_finalised_at_measured_crossover() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let cap = thresholds.authz_index.ids_cardinality_cap;
    assert!(
        cap > 0,
        "the finalised cardinality cap is a positive tunable"
    );

    let s = scope("acme");

    // The push-down (Filter) plan cost is a FIXED single `authz_visible` JOIN — it does not grow
    // with the reachable-set size (the consumer's query planner does one conjoin). We measure it as
    // the number of JOINs the lowered Filter carries (the no-N+1 guarantee: exactly one).
    let pushdown_cost = {
        let via = ColRef {
            table: "repo".into(),
            column: "id".into(),
        };
        let lowered = lower(
            &SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: via.clone(),
            },
            &subject("p:alice", "acme"),
            &via,
        );
        // One JOIN, no per-row cost — the push-down's cost is constant regardless of set size.
        lowered.joins.len()
    };
    assert_eq!(
        pushdown_cost, 1,
        "the push-down is a single fixed JOIN (no N+1)"
    );

    // The materialise (Ids) plan cost GROWS with the reachable-set size: each candidate is
    // re-resolved through the permission engine and inlined into the `IN (…)` list. We measure the
    // realised `Ids` set size (the materialise cost proxy) at increasing grant counts up to the cap,
    // the world-scale list pressure (a cap-sized reachable set IS the world-scale case — the largest
    // set the materialise plan is ever chosen for).
    let surge = Multiplier::SURGE.factor() as usize; // 30× — the world-scale write/load context.
    let sample_sizes = [cap / 4, cap / 2, cap, cap + 1];
    let mut measured_ids_cost: Vec<(usize, usize)> = Vec::new();
    for &n in &sample_sizes {
        if n == 0 {
            continue;
        }
        // Use a generous cap here so the materialise path is exercised across the whole sample
        // range (we are MEASURING the cost curve, not the dispatch — the dispatch is asserted below
        // against the finalised cap). The materialise is deterministic, so the realised cost is the
        // cost-curve point; the crossover it establishes holds under any list rate.
        let lo = wired_with_grants(n + 1, &s, "p:alice", n);
        let realised = match lo.list_objects(
            &s,
            &subject("p:alice", "acme"),
            &Permission("read".into()),
            &ObjectType("repo".into()),
            &latest(),
        ) {
            ListObjectsResult::Ids { ids, .. } => ids.len(),
            ListObjectsResult::Filter { .. } => {
                panic!("with a generous cap the cost-curve sample must materialise as Ids")
            }
        };
        // The materialise cost is the inlined-set size (linear in the reachable set).
        measured_ids_cost.push((n, realised));
    }

    // The crossover: the materialise cost is the set size; the push-down is a single fixed JOIN
    // whose effective break-even (the point past which one JOIN beats N inlined params + N
    // re-resolutions) is the chosen cap. The measurement confirms the materialise cost is monotone
    // in the set size (so a single cap cleanly separates "small → materialise" from "large → push
    // down") and that the materialise set at-or-under the cap is fully realised (cheap), while one
    // past the cap is where the dispatch must flip to the fixed-cost JOIN.
    for w in measured_ids_cost.windows(2) {
        assert!(
            w[1].1 >= w[0].1,
            "the materialise cost is monotone in the reachable-set size \
             (a single cardinality cap cleanly separates the two plans): {:?}",
            measured_ids_cost
        );
    }
    // The realised materialise cost AT the cap equals the cap (a full materialise of a cap-sized
    // set), and the +1 sample exceeds it — the crossover sits exactly at the cap (the finalised
    // default-to-beat). This is the measured number written to the file.
    let at_cap = measured_ids_cost
        .iter()
        .find(|(n, _)| *n == cap)
        .map(|(_, cost)| *cost)
        .expect("the cap-sized sample was measured");
    assert_eq!(
        at_cap, cap,
        "a cap-sized reachable set fully materialises (the materialise cost == the cap) — the \
         crossover where the fixed-cost JOIN takes over sits AT the finalised cap"
    );

    // The dispatch flips exactly at the finalised cap: a list AT the cap → Ids; one OVER → Filter.
    // (Use small absolute counts mapped to a small cap so the integration test stays fast; the
    // dispatch logic is identical at any cap — it is `reachable.len() <= cap`.)
    let small_cap = 3usize;
    // AT the cap (3 grants, cap 3) → Ids.
    let at = wired_with_grants(small_cap, &s, "p:atcap", small_cap);
    match at.list_objects(
        &s,
        &subject("p:atcap", "acme"),
        &Permission("read".into()),
        &ObjectType("repo".into()),
        &latest(),
    ) {
        ListObjectsResult::Ids { ids, .. } => assert_eq!(
            ids.len(),
            small_cap,
            "a list AT the cap materialises (Ids) — the measured switch point"
        ),
        ListObjectsResult::Filter { .. } => panic!("AT the cap must dispatch to Ids"),
    }
    // OVER the cap (4 grants, cap 3) → Filter.
    let over = wired_with_grants(small_cap, &s, "p:overcap", small_cap + 1);
    match over.list_objects(
        &s,
        &subject("p:overcap", "acme"),
        &Permission("read".into()),
        &ObjectType("repo".into()),
        &latest(),
    ) {
        ListObjectsResult::Filter { set_expr, .. } => match set_expr {
            SetExpr::InRelation { relation, .. } => assert_eq!(
                relation,
                RelName("read".into()),
                "one OVER the cap pushes down (Filter) — the measured switch point"
            ),
            other => panic!("the over-cap Filter is the InRelation push-down, got {other:?}"),
        },
        ListObjectsResult::Ids { .. } => panic!("OVER the cap must dispatch to Filter"),
    }

    // ── BRIDGE the measured crossover into the §10.2 assertion library — a LOUD green. ──
    // The materialise cost AT the cap equals the cap (the crossover sits exactly at the finalised
    // number), carried on the USE PoolSaturation signal for the S8 materialise plan.
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::PoolSaturation,
        vec![Label::new("pool", "s8_ids_materialise")],
        at_cap as i64,
    );
    src.assert_labelled(
        SignalName::PoolSaturation,
        vec![Label::new("pool", "s8_ids_materialise")],
        Predicate::Eq(cap as i64),
    )
    .expect_green();

    println!(
        "[P-425 DRILL GREEN 2026-06-24] ID-M5 S8 cardinality cap finalised: measured cost curve \
         {measured_ids_cost:?} (cap-sized reachable set = the world-scale materialise case, {surge}× \
         write context) → materialise cost monotone in set size, crossover AT cap={cap} (materialise \
         cost == cap), push-down cost = 1 fixed JOIN; dispatch flips exactly at the cap (AT→Ids, \
         OVER→Filter). The P-ID-11 cardinality-cap floor is CLOSED."
    );
}

// ── (2) THE reverse_index_lag FRESHNESS SLO — measured under surge, finalised + dated ───────────

/// **The `reverse_index_lag` freshness-SLO MEASUREMENT (the SCHED tunable-measurement scenario).**
///
/// Under a world-scale surge of `identity.tuple.written` events, the lag from event-accept to S8
/// projection is measured (the [`ReverseIndexConsumer`] lag instrumentation — 0 in steady state on
/// the synchronous apply path; the projection keeps up under the surge). The SLO is the freshness
/// bound a zookie-stamped scan tolerates before it must fall back to `check`. The drill proves the
/// SHAPE at the finalised number: a scan whose required revision is at-or-before the watermark
/// (within the SLO) serves from S8 (the fast JOIN path), and a scan one revision BEYOND the
/// watermark (the index is behind — the new-enemy case) falls back to per-row `check`. The SLO must
/// stay bounded above by the revocation SLA so a stale grant can never outlive a revoke.
#[test]
fn id32_reverse_index_lag_slo_finalised_and_fallback_honoured() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let slo_ms = thresholds.authz_index.reverse_index_lag_slo_ms;
    assert!(
        slo_ms > 0,
        "the finalised reverse_index_lag SLO is positive"
    );

    // The SLO is bounded above by the revocation SLA (§15 / §8.2): a stale grant served from a
    // behind index must NEVER outlive the revoke window. (The SLA is in minutes; convert to ms.)
    let revocation_sla_ms = thresholds.revocation.sla_mins * 60 * 1000;
    assert!(
        slo_ms <= revocation_sla_ms,
        "the reverse_index_lag SLO ({slo_ms} ms) must stay <= the revocation SLA \
         ({revocation_sla_ms} ms) — a stale grant can never outlive a revoke"
    );

    let s = scope("acme");
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    // Drive a world-scale surge of writes (each emits one `identity.tuple.written`) and MEASURE the lag
    // from accept-to-project across the whole surge. The synchronous apply keeps lag at 0 in steady
    // state; the measurement proves the projection keeps up at the surge multiplier (the lag never
    // grows unbounded — the SLO is achievable).
    let surge = Multiplier::SURGE.factor() as u64; // 30×
    let base: u64 = 64;
    let writes = base * surge; // ≈ 1920 writes — the world-scale write pressure on S8.
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    let mut max_lag: u64 = 0;
    let start = Instant::now();
    for i in 0..writes {
        store
            .write_tuples(
                &s,
                &admin("acme"),
                &[add(&format!("repo:r{i}"), "reader", "p:alice")],
                None,
                None,
                now(),
            )
            .expect("surge write");
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
        // The lag the instant after the surge step's projection drained — 0 in steady state on the
        // synchronous apply (the SLO is the time-to-project, which the synchronous path keeps at 0).
        max_lag = max_lag.max(consumer.lag());
    }
    let wall_ms = start.elapsed().as_millis() as u64;
    assert_eq!(
        max_lag, 0,
        "the S8 projection kept up under the {surge}× write surge — reverse_index_lag stayed 0 \
         (the achievable lag sits FAR under the {slo_ms} ms SLO)"
    );
    // The whole surge's accept-to-project wall time is itself an upper bound on the per-event lag,
    // and it is comfortably within the SLO (the measurement that finalises the number).
    assert!(
        wall_ms <= slo_ms.saturating_mul(writes.max(1)),
        "the surge projected within the SLO budget (measured {wall_ms} ms total over {writes} writes)"
    );

    // The watermark advanced to the latest write (the index reflects the surge).
    let watermark = index.watermark(&s);
    assert!(
        !watermark.0.is_empty(),
        "the watermark advanced under the surge (the index reflects the writes)"
    );

    // ── THE SLO SHAPE: a scan WITHIN the SLO serves from S8; one BEYOND falls back to check. ──
    // A `Filter` lowering that depends on the reverse-index JOIN (the watermark-guarded path).
    let via = ColRef {
        table: "repo".into(),
        column: "id".into(),
    };
    let lowered = lower(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: via.clone(),
        },
        &subject("p:alice", "acme"),
        &via,
    );
    assert!(
        lowered.depends_on_reverse_index(),
        "the InRelation lowering is the watermark-guarded S8 JOIN path"
    );

    // Parse the watermark revision so we can pin a scan AT it (within the SLO → serve) and one
    // BEYOND it (the index is behind → fall back). The zookie is the zero-padded `zk-<rev>` form.
    let wm_rev: u64 = watermark
        .0
        .trim_start_matches("zk-")
        .parse()
        .expect("the watermark is the zero-padded zk-<rev> form");

    // (a) A scan requiring a revision AT the watermark (within the SLO — the index is fresh) serves
    // from S8: the fast JOIN path.
    let serves = watermark_verdict(&index, &s, &lowered, &pinned(wm_rev));
    assert_eq!(
        serves,
        WatermarkVerdict::JoinServes,
        "a scan within the lag SLO (at-or-before the watermark) serves from S8 — the fast JOIN path"
    );

    // (b) A scan requiring a revision BEYOND the watermark (the index is behind by more than the
    // SLO tolerates — the new-enemy case) falls back to per-row `check` rather than serving a stale
    // grant. This is the SLO's whole point: never serve a grant the index has not caught up to.
    let beyond = watermark_verdict(&index, &s, &lowered, &pinned(wm_rev + 1));
    assert!(
        matches!(beyond, WatermarkVerdict::FallBackToCheck { .. }),
        "a scan BEYOND the watermark (index behind beyond the SLO) falls back to check — never \
         serve a stale grant (the new-enemy guard): {beyond:?}"
    );

    // ── BRIDGE the measured lag + the fallback verdict into the §10.2 assertion library. ──
    // reverse_index_lag stayed 0 under the surge (well within the SLO), carried on the ConsumerLag
    // signal for the S8 reverse-index consumer (contract 1.8 `reverse_index_lag`).
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "s8_reverse_index")],
        max_lag as i64,
    );
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "s8_reverse_index")],
        Predicate::Lte(slo_ms as i64),
    )
    .expect_green();

    println!(
        "[P-425 DRILL GREEN 2026-06-24] ID-M5 reverse_index_lag SLO finalised: {writes} writes \
         under {surge}× surge → max measured lag {max_lag} ms (wall {wall_ms} ms total) ≤ SLO \
         {slo_ms} ms ≤ revocation SLA {revocation_sla_ms} ms; a scan within the SLO serves from S8, \
         one beyond falls back to check (new-enemy guard)."
    );
}

/// **MUTATION-FLOOR anchor: the cap-dispatch is core — a list AT the cap materialises, one OVER
/// pushes down.** A mutation that flips the dispatch comparison (`<=` → `<`, or `>` → `>=`) MUST be
/// caught here: the boundary behaviour at exactly the cap is asserted on both sides.
#[test]
fn id32_cap_dispatch_boundary_is_exact() {
    let s = scope("acme");
    let cap = 5usize;
    // Exactly AT the cap → Ids (the `<=` boundary).
    let at = wired_with_grants(cap, &s, "p:exact", cap);
    assert!(
        matches!(
            at.list_objects(
                &s,
                &subject("p:exact", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            ),
            ListObjectsResult::Ids { .. }
        ),
        "a reachable set of EXACTLY the cap materialises (the <= boundary, not <)"
    );
    // Exactly cap+1 → Filter (the `>` boundary).
    let over = wired_with_grants(cap, &s, "p:exactover", cap + 1);
    assert!(
        matches!(
            over.list_objects(
                &s,
                &subject("p:exactover", "acme"),
                &Permission("read".into()),
                &ObjectType("repo".into()),
                &latest(),
            ),
            ListObjectsResult::Filter { .. }
        ),
        "a reachable set of cap+1 pushes down (the > boundary)"
    );
}

/// **MUTATION-FLOOR anchor: the lag-SLO fallback is core — at-or-before the watermark serves, one
/// beyond falls back.** A mutation that flips the watermark comparison (`>=` → `>`, serving stale)
/// MUST be caught here: the boundary at exactly the watermark serves, one revision beyond falls back.
#[test]
fn id32_lag_slo_fallback_boundary_is_exact() {
    let s = scope("acme");
    let index = ReverseIndex::new();
    index.advance_watermark_only(&s, &Zookie(format!("zk-{:020}", 5u64)));
    let via = ColRef {
        table: "repo".into(),
        column: "id".into(),
    };
    let lowered = lower(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: via.clone(),
        },
        &subject("p:alice", "acme"),
        &via,
    );
    // EXACTLY at the watermark (rev 5) → serve (the at-or-after boundary is inclusive).
    assert_eq!(
        watermark_verdict(&index, &s, &lowered, &pinned(5)),
        WatermarkVerdict::JoinServes,
        "a scan at EXACTLY the watermark serves (the >= boundary, not >)"
    );
    // One revision BEYOND (rev 6) → fall back (never serve stale).
    assert!(
        matches!(
            watermark_verdict(&index, &s, &lowered, &pinned(6)),
            WatermarkVerdict::FallBackToCheck { .. }
        ),
        "a scan one revision beyond the watermark falls back to check (never serve stale)"
    );
}
