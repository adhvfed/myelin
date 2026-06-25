//! Unit tests for the ISS-P36 whole-system E2E-3 wedge (the Issues side — spec-to-ship traceability).
//! Each test drives the chained lineage scenario END-TO-END (the whole spec→issue→PR→CI walk + the
//! mid-flight reindex, not a single handler) and asserts the named green artifact + the F1 leak
//! invariant at E2E scale. The audit-tamper leg + the CDC re-asserts live in `tests/e2e_lineage_iss_p36.rs`.

use super::*;

/// **E2E-3 green: the complete lineage resolves per-viewer (insider walks the whole chain), the
/// outsider's confidential hop tombstones with 0 leak, and the cold-reindex byte-matches live.** The
/// whole flow is driven end-to-end; a regression in any leg flips `is_green()` false.
#[test]
fn e2e_3_lineage_is_green_zero_leak() {
    let art = run_e2e_3_lineage();
    assert_eq!(art.scenario, "E2E-3");
    assert!(art.is_green(), "E2E-3 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 title/count/backlink leak: {}",
        art.evidence
    );
}

/// **The lineage is COMPLETE per-viewer (the insider walks the spec→initiative→issue→PR→CI chain).** The
/// bounded cycle-safe traverse reaches every node within the depth bound (5.3).
#[test]
fn e2e_3_lineage_is_complete_per_viewer() {
    let art = run_e2e_3_lineage();
    assert!(
        art.evidence.contains("lineage_complete=true"),
        "the lineage must be complete (every hop reached): {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("insider_walks_full_lineage=true"),
        "the insider must resolve every Issues hop: {}",
        art.evidence
    );
    assert!(
        art.evidence
            .contains(&format!("depth≤{LINEAGE_DEPTH_BOUND}=true")),
        "the walk must stay within the depth bound: {}",
        art.evidence
    );
}

/// **The confidential mid-chain issue tombstones for the outsider — NEVER a projection (the load-bearing
/// leak invariant at E2E scale); the lineage still degrades gracefully (the downstream hops resolve).**
#[test]
fn e2e_3_outsider_never_sees_confidential_title_lineage_degrades() {
    let art = run_e2e_3_lineage();
    assert!(
        art.evidence.contains("tombstone(denied)=true"),
        "the outsider's confidential hop must tombstone (denied): {}",
        art.evidence
    );
    assert!(
        !art.evidence.contains("SECRET") && !art.evidence.contains("weights"),
        "the secret title must NEVER appear in the artifact body: {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("lineage_degrades_gracefully=true"),
        "the lineage must still reach the downstream PR/CI hops + the sibling: {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

/// **Cold-reindex == live (contract 2.6): the cold-rebuilt issue/relation set byte-matches the live
/// truth at 0 drift.** A regression that lost a `closes` edge / an issue aggregate on the cold rebuild
/// would flip `drift > 0`. This re-confirms the 2.6 reindex-from-source floor holds under the E2E load.
#[test]
fn e2e_3_cold_reindex_equals_live_zero_drift() {
    let (matches, drift) = cold_reindex_matches_live();
    assert!(matches, "cold-reindex must byte-match live (2.6)");
    assert_eq!(drift, 0, "0 drift between cold and live");
    let art = run_e2e_3_lineage();
    assert!(
        art.evidence
            .contains("cold-reindex==live (2.6)=true (drift=0)"),
        "the artifact records the cold==live parity at 0 drift: {}",
        art.evidence
    );
}

/// **The whole-wedge driver returns exactly the Issues-side E2E-3 leg, green.** The master M5 exit gate
/// cites E2E-3; this is the single Issues-side scenario.
#[test]
fn issues_e2e_3_runs_e2e_3_green() {
    let arts = run_issues_e2e_3();
    assert_eq!(arts.len(), 1, "Issues crosses exactly E2E-3");
    assert_eq!(arts[0].scenario, "E2E-3");
    assert!(arts[0].is_green(), "E2E-3: {}", arts[0].evidence);
}

/// **The depth bound is the named frozen threshold (5.3), not a stray literal (it is asserted, never
/// weakened).** A regression that widened the bound to mask a runaway walk would be caught.
#[test]
fn e2e_3_depth_bound_is_the_named_threshold() {
    assert_eq!(
        LINEAGE_DEPTH_BOUND,
        crate::refs_glue::TRAVERSE_MAX_DEPTH,
        "the lineage depth bound is the frozen 5.3 traverse bound (depth 16)"
    );
    assert_eq!(LINEAGE_DEPTH_BOUND, 16);
}

/// **The lineage audit anchor is the PII-free initiative ref (the GA-D3 tamper-leg subject).** The audit
/// chain records WHAT shipped (the ref), never a title/body — a regression that leaked a title into the
/// anchor would be caught (the anchor is a URN, no title field).
#[test]
fn e2e_3_audit_anchor_is_pii_free_initiative_ref() {
    let anchor = lineage_audit_anchor();
    assert!(
        anchor.0.contains(initiative_key()),
        "the anchor carries the initiative ref"
    );
    assert!(
        !anchor.0.contains("SECRET") && !anchor.0.contains("weights"),
        "the anchor must carry NO title/body (PII-free)"
    );
}
