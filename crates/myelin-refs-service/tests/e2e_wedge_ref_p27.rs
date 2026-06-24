//! # REF-P27 / P-458 — the whole-system E2E wedge Refs crosses (E2E-1 / E2E-3 / E2E-4)
//!
//! **The completion of R-M5.** This is the **Refs side of the three whole-system chained-mutation E2E
//! scenarios** — each driving the WHOLE flow end-to-end (not a single handler) over the
//! production-hardened Refs engine, and asserting the scenario's named green artifact + the F1 leak
//! invariant at E2E scale:
//!
//! - **E2E-1 — the PR context pane (Refs is the spine):** every connected artifact resolves per-viewer;
//!   the mid-flight `ci.check.updated` live-updates; the second (denied) viewer's confidential issue
//!   degrades to a TOMBSTONE carrying the root — **0 title/count/backlink leak** (REF-D1 resolve half).
//! - **E2E-3 — spec-to-ship traceability:** `traverse(spec_doc, viewer)` walks the ENTIRE lineage
//!   depth-16 cycle-safe per-viewer; the per-viewer prune drops the unreadable node AND its branch (0
//!   leak through the traverse); the wiped Refs edge index reindexes to BYTE-MATCH live (F4 / REF-D4 at
//!   scale).
//! - **E2E-4 — the DSAR fan-out:** Refs' edges + cache return 0 recoverable PII (incl. backups);
//!   unfurls degrade to tombstones; the holder-coverage receipt includes Refs.
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §2 (E2E-1/E2E-3/
//! E2E-4 — the chained-mutation scenarios). **Architecture:** reference-graph.md §1 (the moat thesis),
//! §4.5 (lineage traverse), §4.7 (reindex parity). **Contract-index rows 5.2/5.3/10.1.** **Doctrine:**
//! EI-01 §3/§4 (drive the WHOLE thing; prove it; never claim a green you did not earn). **VISION §1, §3.**
//!
//! ## What this proves (the master M5 exit gate's E2E-1..E2E-4 green, Refs side)
//! The wedge drives the SAME production-hardened engine the M5 prompts built (no second resolver/
//! traverse/eraser) — the green is the engine's own behaviour observed across the whole chained flow.
//! The leak-invariant mutation floors (resolve.rs / traverse.rs / backlinks.rs / holder.rs) are
//! UNCHANGED and STILL HOLD at E2E scale; this drill adds NO new leak-decision logic.
//!
//! ## Floor named (the ONE legitimate remaining floor)
//! None new. E2E-3's reindex leg inherits the world-scale fleet-hardware 30× load floor
//! ([`myelin_refs_service::WORLD_SCALE_FLEET_LOAD_FLOOR`], REF-P24) and E2E-4's erase leg the
//! backup-fleet floor ([`myelin_refs_service::WORLD_SCALE_BACKUP_FLEET_FLOOR`], REF-P25) — both already
//! named. This wedge is the E2E run over the production-hardened engine; it does not introduce a new
//! floor.
//!
//! Permanent-gate posture: re-run on every resolve/traverse/reindex/erase-touching change; this is the
//! master M5→M6 boundary (a red E2E-1 must NOT let M6 start).

use myelin_events::{Actor, EmitContextBase, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout, run_refs_e2e_wedge,
    E2E_SCENARIOS, WORLD_SCALE_BACKUP_FLEET_FLOOR, WORLD_SCALE_FLEET_LOAD_FLOOR,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

/// **E2E-1 — the PR context pane, end-to-end (Refs as the spine).** The whole chained flow: resolve
/// every connected artifact per-viewer → mid-flight `ci.check.updated` live-update → second denied
/// viewer's confidential issue tombstones with 0 leak. The named green artifact is emitted.
#[test]
fn e2e_1_pr_pane_green_refs_is_the_spine() {
    let art = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert!(
        art.is_green(),
        "E2E-1 (the PR pane — Refs is the spine) must be green: {}",
        art.evidence
    );
    // The F1 leak spine: 0 title/count/backlink leak to the unauthorized viewer.
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 title/count/backlink leak — {}",
        art.evidence
    );
    // The load-bearing chained-mutation assertions are in the evidence (the dated artifact's body).
    assert!(art.evidence.contains("tombstone(denied)=true"));
    assert!(art.evidence.contains("live-updated=true"));
}

/// **E2E-3 — spec-to-ship traceability, end-to-end.** The whole chained flow: the full lineage traverse
/// depth-16 per-viewer → the per-viewer prune (0 leak) → wipe → reindex → byte-match live. The named
/// green artifact is emitted.
#[test]
fn e2e_3_spec_to_ship_green_lineage_and_reindex_parity() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert_eq!(art.scenario, "E2E-3");
    assert!(
        art.is_green(),
        "E2E-3 (the lineage traverse + reindex parity) must be green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "E2E-3: 0 leak through the traverse — {}",
        art.evidence
    );
    // The cold-reindex == live mutation leg (F4 / REF-D4 at scale).
    assert!(art.evidence.contains("cold-reindex==live=true"));
    // The cycle was surfaced as a diagnostic (never a hang — REF-D8 at E2E scale).
    assert!(art.evidence.contains("cycle_surfaced=true"));
}

/// **E2E-4 — the DSAR fan-out, end-to-end (Refs side).** The whole chained flow: seed → erase
/// (crypto-shred + tombstone) → restore a pre-erase backup → re-erase from the ledger → 0 recoverable
/// PII. The named green artifact is emitted; the holder-coverage receipt includes Refs.
#[test]
fn e2e_4_dsar_fanout_green_zero_recoverable_pii_incl_backups() {
    let art = run_e2e_4_dsar_fanout();
    assert_eq!(art.scenario, "E2E-4");
    assert!(
        art.is_green(),
        "E2E-4 (the DSAR fan-out — Refs edges + cache return 0 recoverable PII) must be green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "E2E-4: 0 recoverable PII incl. backups — {}",
        art.evidence
    );
    // Non-vacuous: the restore actually resurrected the PII before the re-erase re-shredded it.
    assert!(art.evidence.contains("restore resurrected"));
}

/// **THE master M5 exit gate (Refs side): E2E-1 + E2E-3 + E2E-4 each green (completes R-M5).** A red
/// E2E-1 must NOT let M6 start. This is the whole-wedge artifact set the gate cites.
#[test]
fn whole_refs_e2e_wedge_completes_r_m5() {
    let artifacts = run_refs_e2e_wedge(ctx_base());
    assert_eq!(artifacts.len(), 3, "the wedge runs E2E-1/E2E-3/E2E-4");
    for (art, expected) in artifacts.iter().zip(E2E_SCENARIOS.iter()) {
        assert_eq!(&art.scenario, expected, "scenario order is -1/-3/-4");
        assert!(
            art.is_green(),
            "{} must be green to complete R-M5: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(art.leaks, 0, "{}: 0 leak at E2E scale", art.scenario);
    }
}

/// **No new floor — the inherited world-scale floors are named (the prompt's DoD: none new).** E2E-3's
/// reindex leg inherits the fleet-load floor (REF-P24); E2E-4's erase leg the backup-fleet floor
/// (REF-P25). This asserts the named constants are present (the floor is named, never silently dropped).
#[test]
fn inherited_world_scale_floors_are_named_no_new_floor() {
    assert!(
        WORLD_SCALE_FLEET_LOAD_FLOOR.contains("30x")
            || WORLD_SCALE_FLEET_LOAD_FLOOR.contains("30×"),
        "E2E-3's reindex leg inherits the named fleet-load floor (REF-P24)"
    );
    assert!(
        WORLD_SCALE_BACKUP_FLEET_FLOOR.contains("30x")
            || WORLD_SCALE_BACKUP_FLEET_FLOOR.contains("30×")
            || WORLD_SCALE_BACKUP_FLEET_FLOOR.contains("backup"),
        "E2E-4's erase leg inherits the named backup-fleet floor (REF-P25)"
    );
}
