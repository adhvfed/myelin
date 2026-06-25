//! Unit tests for the KN-P32 all-hands-doc surge controls + the concurrent-same-gap LexoRank storm.
//!
//! Written to the mandatory-core cargo-mutants floor: every threshold boundary (the graded shed order,
//! the per-doc op cap, the read-fanout bound), the per-tenant isolation, and the 0-reorder LexoRank
//! predicate has a KILLING assertion — an off-by-one that sheds an editor before a viewer, leaks one
//! tenant's budget, lets a doc's op fan-out grow unbounded, or collides two same-gap keys fails here.

use super::*;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
use myelin_tenancy::Region;

fn tenant(s: &str) -> TenantId {
    TenantId(s.to_string())
}

fn human(tenant_slug: &str) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("h-{tenant_slug}")),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn agent(tenant_slug: &str) -> Principal {
    Principal::new(
        tenant(tenant_slug),
        Region("fr-par".into()),
        PrincipalId(format!("a-{tenant_slug}")),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        },
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

/// cap 6, reserve 2 → non-human budget 4; step = max(4/8,1)=1 → speculative ceiling 2, batch 3, agent
/// 4. A small deterministic budget so the graded thresholds are easy to reach. Per-doc cap + fanout
/// are large here so the OP-STREAM lane drives the shed (separate tests drive the per-doc/fanout caps).
fn small_lane_budget() -> SurfaceBudget {
    SurfaceBudget {
        per_tenant_in_flight_cap: 6,
        human_lane_reservation: 2,
        retry_after_secs: 3,
    }
}

// ───────────────────────── the shed budget is read from the file ─────────────────────────

/// **The Knowledge collab shed budget is read from the thresholds file** (the prompt's explicit
/// requirement). The gate opens against the canonical `thresholds.toml` `[[shed_budgets]]` row for
/// `CollabOpStream` — not a hardcoded number. A missing row would have been a loud error.
#[test]
fn the_collab_shed_budget_is_read_from_the_thresholds_file() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let gate =
        CollabSurgeGate::from_thresholds(&thresholds).expect("CollabOpStream budget present");
    assert_eq!(gate.surface(), ShedSurface::CollabOpStream);

    let b = thresholds
        .shed_budget(ShedSurface::CollabOpStream)
        .expect("present");
    assert!(
        b.per_tenant_in_flight_cap > 0,
        "CollabOpStream bounded (§7.1)"
    );
    assert!(
        b.human_lane_reservation > 0,
        "CollabOpStream reserves an active-editor (human) lane"
    );
    // the surge multiplier in the file matches the documented default-to-beat (never hardcoded).
    assert_eq!(thresholds.surge.multiplier, COLLAB_SURGE_MULTIPLIER);
}

// ───────────────────────── the active-editor lane reservation (the shed order) ─────────────────────

/// **The shed order serves the active human editor while the agent edit lane sheds (KN-D8):** the human
/// active edit is SERVED while the agent edit lane SHEDS (`429 + Retry-After`).
#[test]
fn shed_order_serves_the_human_editor_while_the_agent_lane_sheds() {
    let mut gate = CollabSurgeGate::with_budget(small_lane_budget());
    let a = agent("acme");
    let h = human("acme");

    // an agent edit storm fills the non-human budget (cap-reserved = 4) then sheds.
    for _ in 0..4 {
        assert!(
            gate.admit_for(&a, "doc1", None).is_ok(),
            "agent edit admitted under budget"
        );
    }
    let shed = gate
        .admit_for(&a, "doc1", None)
        .expect_err("the agent edit storm sheds");
    assert_eq!(shed.lane, RunClass::Agent);
    assert_eq!(shed.reason, CollabShedReason::OpStreamLane);
    assert_eq!(shed.retry_after_secs, 3, "the shed carries a Retry-After");

    // THE GATE: the active HUMAN editor's op is STILL SERVED (shed last).
    assert_eq!(
        gate.admit_for(&h, "doc1", None)
            .expect("the human editor is served while the agent sheds"),
        RunClass::Human
    );
    assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
    assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
}

/// **Viewers shed before editors (the prompt's shed order):** a passive viewer (a speculative
/// down-class) sheds at a TIGHTER graded ceiling than an active editor (agent), which sheds before the
/// human. The full priority: viewer(speculative) → batch/CI → agent(editor) → human(active editor).
#[test]
fn viewers_shed_before_editors_agents_before_humans() {
    let mut gate = CollabSurgeGate::with_budget(small_lane_budget());
    let t = tenant("acme");
    // fill non_human to 2 with agent edits (under every ceiling: speculative 2, batch 3, agent 4).
    for _ in 0..2 {
        gate.admit_doc_op(&t, "doc1", RunClass::Agent)
            .expect("agent edit admitted");
    }
    // a passive VIEWER (speculative) sheds FIRST (ceiling 2, not < 2).
    assert!(
        gate.admit_doc_op(&t, "doc1", RunClass::Speculative)
            .is_err(),
        "a passive viewer sheds before an editor"
    );
    // batch/CI still admitted (ceiling 3).
    gate.admit_doc_op(&t, "doc1", RunClass::BatchCi)
        .expect("batch admitted"); // non_human → 3
    assert!(
        gate.admit_doc_op(&t, "doc1", RunClass::BatchCi).is_err(),
        "batch/CI sheds next"
    );
    // an agent EDITOR still admitted (ceiling 4).
    gate.admit_doc_op(&t, "doc1", RunClass::Agent)
        .expect("agent editor admitted"); // non_human → 4
    assert!(
        gate.admit_doc_op(&t, "doc1", RunClass::Agent).is_err(),
        "the agent editor sheds before the human"
    );
    // the active HUMAN editor is served — shed last.
    gate.admit_doc_op(&t, "doc1", RunClass::Human)
        .expect("the active human editor is served — shed last");

    assert_eq!(
        gate.shed_count(RunClass::Speculative),
        1,
        "viewer shed first"
    );
    assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
    assert_eq!(gate.shed_count(RunClass::Agent), 1);
    assert_eq!(gate.shed_count(RunClass::Human), 0, "human editor: 0 shed");
}

/// **Per-tenant: one tenant's edit storm NEVER sheds another tenant's human editor (blast-radius).**
#[test]
fn one_tenants_storm_never_sheds_anothers_human_editor() {
    let mut gate = CollabSurgeGate::with_budget(small_lane_budget());
    let noisy = agent("noisy");
    let quiet_human = human("quiet");

    for _ in 0..4 {
        gate.admit_for(&noisy, "doc1", None)
            .expect("noisy agent admitted");
    }
    assert!(
        gate.admit_for(&noisy, "doc1", None).is_err(),
        "noisy agent edit lane sheds"
    );
    assert_eq!(gate.in_flight(&tenant("noisy")), 4, "noisy has 4 in-flight");
    assert_eq!(
        gate.in_flight(&tenant("quiet")),
        0,
        "the quiet tenant's budget is independent"
    );
    assert_eq!(
        gate.admit_for(&quiet_human, "doc2", None)
            .expect("the quiet human editor is served"),
        RunClass::Human,
        "the noisy storm must NEVER shed another tenant's human editor"
    );
}

/// **A machine principal can NEVER up-class to the active-human-editor lane** (structurally
/// unspoofable). A human-issued prefetch read MAY down-class itself to a passive viewer.
#[test]
fn a_machine_principal_cannot_spoof_the_human_editor_lane() {
    let a = agent("acme");
    // an agent has no human header at all → it can never become a human editor.
    assert_eq!(
        CollabSurgeGate::derive_class(&a, None),
        RunClass::Agent,
        "an agent edit is the agent lane, never the protected human lane"
    );
    let h = human("acme");
    // a human-issued read down-classes itself to a passive viewer (speculative — sheds first).
    assert_eq!(
        CollabSurgeGate::derive_class(&h, Some(RunClassHeader::Speculative)),
        RunClass::Speculative,
        "a human-issued passive read may down-class itself to a viewer"
    );
    // a human active edit (no header) holds the protected lane.
    assert_eq!(CollabSurgeGate::derive_class(&h, None), RunClass::Human);
}

// ───────────────────────── the per-doc op in-flight cap (bounded everything) ───────────────────────

/// **The per-doc op cap bounds one hot doc's op fan-out (Little's Law, §7.1).** With a generous lane
/// budget but a TIGHT per-doc cap, the hot doc's ops fast-fail at the cap — the op-stream lane is NOT
/// the thing that sheds, the per-doc cap is. A DIFFERENT doc is unaffected (the cap is per-doc).
#[test]
fn the_per_doc_op_cap_bounds_one_hot_docs_fan_out() {
    // generous lane (cap 100, reserve 25) so the lane never sheds; per-doc op cap 3, fanout generous.
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 100,
        human_lane_reservation: 25,
        retry_after_secs: 4,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 3, 100);
    let t = tenant("acme");

    for _ in 0..3 {
        gate.admit_doc_op(&t, "hot", RunClass::Agent)
            .expect("under the per-doc cap");
    }
    assert_eq!(gate.doc_in_flight("hot"), 3, "the hot doc is at its cap");
    let shed = gate
        .admit_doc_op(&t, "hot", RunClass::Agent)
        .expect_err("the hot doc's op fan-out sheds at the per-doc cap");
    assert_eq!(shed.reason, CollabShedReason::PerDocOpCap);
    assert_eq!(shed.retry_after_secs, 4);
    assert_eq!(
        gate.doc_op_shed_count("hot"),
        1,
        "the per-doc shed is counted"
    );
    // in-flight never exceeds the bound (Little's Law).
    assert_eq!(
        gate.doc_in_flight("hot"),
        3,
        "in-flight never grows past the cap"
    );

    // a DIFFERENT doc is unaffected — the cap is per-doc.
    gate.admit_doc_op(&t, "cool", RunClass::Agent)
        .expect("a different doc has its own cap");
    assert_eq!(gate.doc_in_flight("cool"), 1);
}

/// **A per-doc shed does NOT leak an op-stream lane slot (no double-charge).** When the lane admits but
/// the per-doc cap sheds, the lane slot taken is released — otherwise a per-doc storm would silently
/// starve the lane. Drives the hot doc to its per-doc cap and asserts the lane in-flight recovered.
#[test]
fn a_per_doc_shed_releases_the_lane_slot_no_double_charge() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 100,
        human_lane_reservation: 25,
        retry_after_secs: 4,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 2, 100);
    let t = tenant("acme");
    gate.admit_doc_op(&t, "hot", RunClass::Agent).expect("1");
    gate.admit_doc_op(&t, "hot", RunClass::Agent).expect("2");
    assert_eq!(gate.in_flight(&t), 2, "2 lane slots taken");
    // the per-doc cap sheds — the lane slot must be released, so lane in-flight stays at 2 not 3.
    assert!(gate.admit_doc_op(&t, "hot", RunClass::Agent).is_err());
    assert_eq!(
        gate.in_flight(&t),
        2,
        "a per-doc shed did not leak a lane slot (no double-charge)"
    );
}

// ───────────────────────── the read-fanout bound (bounded everything) ──────────────────────────────

/// **The read-fanout bound caps one edit's broadcast under a viewer storm.** The fan-out fast-fails at
/// its bound rather than turning one edit into an unbounded broadcast. Per-doc + released-recovers.
#[test]
fn the_read_fanout_bound_caps_one_edits_broadcast() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 100,
        human_lane_reservation: 25,
        retry_after_secs: 6,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 100, 2);
    gate.admit_read_fanout("hot").expect("1");
    gate.admit_read_fanout("hot").expect("2");
    let shed = gate
        .admit_read_fanout("hot")
        .expect_err("the read fan-out sheds at its bound");
    assert_eq!(shed.reason, CollabShedReason::ReadFanout);
    assert_eq!(
        shed.lane,
        RunClass::Speculative,
        "a viewer fan-out is speculative"
    );
    assert_eq!(gate.read_fanout_shed_count("hot"), 1);
    // a released slot is reusable (the fan-out recovers after the surge passes).
    gate.release_read_fanout("hot");
    gate.admit_read_fanout("hot")
        .expect("a released fan-out slot is reusable");
}

// ───────────────────────── release recovers the op lane + per-doc cap ──────────────────────────────

#[test]
fn release_op_frees_both_the_lane_and_the_per_doc_slot() {
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 4,
        human_lane_reservation: 1,
        retry_after_secs: 1,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 2, 100);
    let t = tenant("acme");
    gate.admit_doc_op(&t, "hot", RunClass::Agent).expect("1");
    gate.admit_doc_op(&t, "hot", RunClass::Agent)
        .expect("2 — at per-doc cap");
    assert!(
        gate.admit_doc_op(&t, "hot", RunClass::Agent).is_err(),
        "per-doc cap reached"
    );
    gate.release_op(&t, "hot", RunClass::Agent);
    assert_eq!(gate.doc_in_flight("hot"), 1, "per-doc slot freed");
    gate.admit_doc_op(&t, "hot", RunClass::Agent)
        .expect("a released slot is reusable");
}

// ───────────────────────── the KN-D8 + F6 surge report ─────────────────────────────────────────────

/// **The KN-D8 + F6 surge report is GREEN under a real storm** (the surge properties).
#[test]
fn run_collab_surge_is_green() {
    // lane cap 12, reserve 4 → non-reserved budget 8 (> the per-doc cap 4, so the per-doc cap is the
    // binding bound for ONE doc while the lane saturates on the SPREAD). per-doc cap 4 + fanout 4 so all
    // three controls genuinely shed.
    let budget = SurfaceBudget {
        per_tenant_in_flight_cap: 12,
        human_lane_reservation: 4,
        retry_after_secs: 2,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(budget, 4, 4);
    let surging = tenant("noisy");
    let quiet = tenant("quiet");
    // a storm well past every bound so the machine lanes + the per-doc/fanout caps MUST shed.
    let report = run_collab_surge(
        &mut gate,
        &surging,
        &quiet,
        "all-hands",
        100,
        100,
        COLLAB_SURGE_MULTIPLIER,
    );
    assert!(report.is_green(), "{}", report.summary());
    assert!(report.surging_agent_shed_count > 0, "agent edit lane shed");
    assert!(report.surging_viewer_shed_count > 0, "viewer lane shed");
    assert_eq!(report.surging_human_shed_count, 0, "human editor lane held");
    assert!(
        report.surging_human_admitted,
        "surging tenant's human editor held"
    );
    assert!(
        report.quiet_human_admitted,
        "quiet co-tenant's human editor held"
    );
    assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    assert!(report.hot_doc_op_cap_shed_count > 0, "hot doc op cap held");
    assert!(
        report.hot_doc_read_fanout_shed_count > 0,
        "read fanout held"
    );
}

/// **The surge gate is NOT vacuous — an UNBOUNDED lane + UNBOUNDED caps (no shed) read RED.**
#[test]
fn an_unbounded_gate_reads_red() {
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 2,
    };
    let mut gate = CollabSurgeGate::with_budget_and_bounds(huge, 1_000_000, 1_000_000);
    let report = run_collab_surge(
        &mut gate,
        &tenant("noisy"),
        &tenant("quiet"),
        "all-hands",
        100,
        100,
        COLLAB_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm"
    );
    assert!(!report.is_green(), "an unbounded gate MUST read RED");
}

// ───────────────────────── the concurrent-same-gap LexoRank insert storm ───────────────────────────

/// **The concurrent-same-gap LexoRank insert storm (§3.5): 0 key-collision reorder + bounded
/// rebalance.** Thousands of concurrent inserts into the SAME sibling gap each produce a DISTINCT
/// order key (the frozen 2-char jitter), all sort strictly within the gap (no reorder), and none trip
/// the 48-char rebalance trigger (bounded rebalance cost).
#[test]
fn the_lexorank_storm_has_zero_reorder_and_bounded_rebalance() {
    let lo = OrderKey::parse("U00").expect("lo");
    let hi = OrderKey::parse("V00").expect("hi");
    // a genuine same-gap storm: 2000 concurrent inserts into the one gap (lo, hi).
    let report = run_lexorank_storm(Some(&lo), Some(&hi), 2000);
    assert!(report.is_green(), "{}", report.summary());
    assert_eq!(report.inserts, 2000);
    assert_eq!(
        report.distinct_keys, 2000,
        "every concurrent insert produced a DISTINCT key — 0 key-collision reorder"
    );
    assert!(
        report.all_within_gap,
        "every key sorts strictly within the gap — no reorder relative to the rest of the list"
    );
    assert_eq!(
        report.rebalance_triggers, 0,
        "the single-gap storm forced 0 rebalance — bounded rebalance cost (§3.5)"
    );
}

/// **The storm is order-preserving even with NO gap bounds (insert-at-end storm).** With `lo`/`hi` =
/// `None` (a start/end insert), the keys are still all distinct — no collision under the jitter.
#[test]
fn the_lexorank_storm_is_distinct_even_unbounded_gap() {
    let report = run_lexorank_storm(None, None, 500);
    assert_eq!(
        report.distinct_keys, 500,
        "an unbounded-gap storm still produces distinct keys (the jitter)"
    );
    assert!(
        report.all_within_gap,
        "no bounds → trivially within the (open) gap"
    );
    assert!(report.is_green(), "{}", report.summary());
}

/// **The LexoRank-storm report is NOT vacuous — colliding inputs would read RED.** A sanity check that
/// the distinct-key predicate actually distinguishes: a degenerate 1-insert storm has 1 distinct key
/// (green), but the predicate `distinct_keys == inserts` is the thing that catches a collision.
#[test]
fn the_lexorank_report_predicate_is_not_vacuous() {
    // A hand-built report with a collision (distinct < inserts) reads RED — the predicate has teeth.
    let collided = LexoStormReport {
        inserts: 10,
        distinct_keys: 9, // one collision
        all_within_gap: true,
        rebalance_triggers: 0,
    };
    assert!(
        !collided.is_green(),
        "a key-collision reorder MUST read RED (the predicate is not vacuous)"
    );
    // and a runaway rebalance reads RED.
    let runaway = LexoStormReport {
        inserts: 10,
        distinct_keys: 10,
        all_within_gap: true,
        rebalance_triggers: 1, // a key ran past the 48-char trigger
    };
    assert!(!runaway.is_green(), "an unbounded rebalance MUST read RED");
    // an escaped-gap key reads RED.
    let escaped = LexoStormReport {
        inserts: 10,
        distinct_keys: 10,
        all_within_gap: false,
        rebalance_triggers: 0,
    };
    assert!(!escaped.is_green(), "a reorder (escaped gap) MUST read RED");
}

// ───────────────────────── the floors are named ────────────────────────────────────────────────────

#[test]
fn the_floors_are_named() {
    assert_eq!(COLLAB_SURGE_MULTIPLIER, 30);
    assert!(
        FLEET_HARDWARE_FLOOR.contains("fleet"),
        "the world-scale fleet-hardware load is the named remaining floor"
    );
}
