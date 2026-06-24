//! Unit tests for the REF-P27 whole-system E2E wedge (E2E-1 / E2E-3 / E2E-4 — the Refs side). Each
//! test drives a chained-mutation scenario END-TO-END (the whole flow, not a single handler) and
//! asserts its named green artifact + the F1 leak invariant at E2E scale. The deeper chained / drill
//! coverage lives in `tests/e2e_wedge_ref_p27.rs`.

use super::*;
use myelin_events::{Actor, Timestamp};

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: e2e_tenant(),
        region: e2e_region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            e2e_tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:00Z".into()),
        caused_by: None,
    }
}

// ── E2E-1 — the PR context pane (Refs as the spine). ──

/// **E2E-1 green: the PR pane resolves every connected artifact per-viewer, the mid-flight
/// ci.check.updated live-updates, and the second (denied) viewer's confidential issue tombstones with
/// 0 title/count/backlink leak.** The whole flow is driven end-to-end.
#[test]
fn e2e_1_pr_pane_is_green_zero_leak() {
    let art = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert!(art.is_green(), "E2E-1 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 title/count/backlink leak: {}",
        art.evidence
    );
}

/// **The confidential issue tombstones for the outsider — NEVER a projection (the load-bearing leak
/// invariant at E2E scale).** A regression that leaked the title would flip `leaks > 0`.
#[test]
fn e2e_1_outsider_never_sees_confidential_title() {
    let art = run_e2e_1_pr_pane();
    assert!(
        art.evidence.contains("tombstone(denied)=true"),
        "the outsider's confidential issue must tombstone (denied): {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

// ── E2E-3 — spec-to-ship traceability (the full lineage traverse + reindex parity). ──

/// **E2E-3 green: the full lineage walks depth-16 cycle-safe per-viewer, the per-viewer prune drops the
/// unreadable node AND its branch (0 leak), and the wiped index reindexes to byte-match live.**
#[test]
fn e2e_3_spec_to_ship_is_green_zero_leak() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert_eq!(art.scenario, "E2E-3");
    assert!(art.is_green(), "E2E-3 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 leak through the traverse: {}",
        art.evidence
    );
}

/// **The cold-reindex byte-matches live (F4 / REF-D4 at scale) — the cold==live mutation leg.**
#[test]
fn e2e_3_cold_reindex_byte_matches_live() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert!(
        art.evidence.contains("cold-reindex==live=true"),
        "the wiped index must reindex to byte-match live: {}",
        art.evidence
    );
}

/// **The per-viewer prune is not a side-channel: a node reachable ONLY through an unreadable node is
/// ALSO pruned (the branch-prune leak invariant at E2E scale).**
#[test]
fn e2e_3_prune_drops_the_whole_unreadable_branch() {
    let art = run_e2e_3_spec_to_ship(ctx_base());
    assert!(
        art.evidence.contains("deploy_pruned=true") && art.evidence.contains("chat_pruned=true"),
        "the unreadable node AND its branch must be pruned: {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

// ── E2E-4 — the DSAR fan-out (Refs' edges + cache return 0 recoverable PII). ──

/// **E2E-4 green: a DSAR erase crypto-shreds the subject's cached titles + tombstones the edges, a
/// restored pre-erase backup is re-erased from the ledger, and 0 recoverable PII survives (incl.
/// backups) — the holder-coverage receipt includes Refs.**
#[test]
fn e2e_4_dsar_fanout_is_green_zero_recoverable_pii() {
    let art = run_e2e_4_dsar_fanout();
    assert_eq!(art.scenario, "E2E-4");
    assert!(art.is_green(), "E2E-4 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 recoverable PII (incl. backups): {}",
        art.evidence
    );
}

/// **The restore actually resurrected the PII (the non-vacuous proof) — the 0-recoverable is earned
/// AFTER a backup brought the name-bearing titles + DEKs back.**
#[test]
fn e2e_4_restore_is_non_vacuous() {
    let art = run_e2e_4_dsar_fanout();
    assert!(
        art.evidence.contains("restore resurrected"),
        "the restore must resurrect the PII before the re-erase re-shreds it: {}",
        art.evidence
    );
}

// ── The whole wedge — all three Refs-side scenarios green (completes R-M5). ──

/// **The whole Refs-side E2E wedge: E2E-1 + E2E-3 + E2E-4 each emit a green artifact (completes
/// R-M5).** The master M5 exit gate cites E2E-1..E2E-4 green; a red E2E-1 must NOT let M6 start.
#[test]
fn whole_wedge_all_three_scenarios_green() {
    let artifacts = run_refs_e2e_wedge(ctx_base());
    assert_eq!(artifacts.len(), 3, "the wedge runs E2E-1/E2E-3/E2E-4");
    for (art, expected) in artifacts.iter().zip(E2E_SCENARIOS.iter()) {
        assert_eq!(&art.scenario, expected);
        assert!(
            art.is_green(),
            "{} must be green to complete R-M5: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(art.leaks, 0, "{} 0 leak", art.scenario);
    }
}

/// **The E2E scenario list names exactly the three Refs crosses (-1/-3/-4) — the master M5 exit gate
/// reads this list.**
#[test]
fn e2e_scenarios_named() {
    assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3", "E2E-4"]);
}
