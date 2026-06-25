//! AG-P25 → global P-481: the seam-floor **gap-report** + the `no-llm-in-platform` re-confirm.
//!
//! This prompt is the NAMING of the three designed-not-built Fabric floors (`LlmAgentRuntime`, the
//! external MCP endpoint, agent long-term memory/RAG) and the three `[OPEN -> LEGAL]` items (L-3
//! implicit auto-dispatch, L-4 reasoning-capture, build-data-as-training foreclosed). The gate has
//! two halves (architecture §3.3/§6.2/§12; roadmap §2 M5 / §3):
//!
//! 1. **0 invisible gaps** — every named floor + legal item is recorded with a NON-EMPTY trigger +
//!    follow-on + band/owner (`crate::seam::SeamFloor::is_fully_recorded`). A floor named without a
//!    trigger or without a follow-on is an invisible gap and fails the report.
//! 2. **The `no-llm-in-platform` lint (contract 1.6) stays green over the seam doc** — the seam
//!    module `src/seam.rs` adds NO model/SDK/prompt/model-name fingerprint. The live workspace gate
//!    (`myelin-lints` `workspace_clean.rs`) already scans every `crates/*/src/*.rs`; this test
//!    additionally re-runs the lint over THIS crate's `seam.rs` source directly, so the proof lives
//!    with the deliverable.
//!
//! No new core module: the seam doc itself is the deliverable; these are its assertions.

use std::path::{Path, PathBuf};

use myelin_agent::seam::{all_seam_items, FollowOnBand, SeamKind, NAMED_FLOORS, OPEN_LEGAL_ITEMS};
use myelin_lints::lints::no_llm_in_platform;

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The three named floors are exactly: the real runtime, the external MCP endpoint, long-term
/// memory. The three legal items are exactly: L-3, L-4, build-data-as-training. None may go
/// missing (a missing floor is the silent-skip failure VISION §3 forbids).
#[test]
fn the_six_seam_items_are_all_present_and_correctly_kinded() {
    assert_eq!(
        NAMED_FLOORS.len(),
        3,
        "exactly the THREE named floors must be recorded (runtime / external MCP / memory)"
    );
    assert_eq!(
        OPEN_LEGAL_ITEMS.len(),
        3,
        "exactly the THREE [OPEN -> LEGAL] items must be recorded (L-3 / L-4 / training)"
    );

    let floor_ids: Vec<&str> = NAMED_FLOORS.iter().map(|f| f.id).collect();
    assert_eq!(
        floor_ids,
        vec![
            "llm-agent-runtime",
            "external-mcp-endpoint",
            "long-term-memory-rag"
        ],
        "the three named floors must be the runtime, external MCP, and long-term memory"
    );

    let legal_ids: Vec<&str> = OPEN_LEGAL_ITEMS.iter().map(|f| f.id).collect();
    assert_eq!(
        legal_ids,
        vec![
            "l3-implicit-auto-dispatch",
            "l4-reasoning-capture",
            "build-data-as-training"
        ],
        "the three legal items must be L-3 auto-dispatch, L-4 reasoning-capture, training-basis"
    );

    for f in NAMED_FLOORS {
        assert_eq!(
            f.kind,
            SeamKind::NamedFloor,
            "floor `{}` must be kinded NamedFloor",
            f.id
        );
    }
    for f in OPEN_LEGAL_ITEMS {
        assert_eq!(
            f.kind,
            SeamKind::OpenLegal,
            "item `{}` must be kinded OpenLegal",
            f.id
        );
    }
}

/// THE GAP-REPORT GATE: 0 invisible gaps. Every seam item (floor + legal) carries a non-empty
/// trigger AND a non-empty follow-on AND a band/owner — none is named-without-a-follow-on.
#[test]
fn the_gap_report_has_zero_invisible_gaps() {
    let mut invisible: Vec<&str> = Vec::new();
    for item in all_seam_items() {
        if !item.is_fully_recorded() {
            invisible.push(item.id);
        }
        // The two load-bearing fields the prompt names explicitly: a trigger and a follow-on.
        assert!(
            !item.trigger.is_empty(),
            "seam item `{}` is named WITHOUT a trigger — an invisible gap",
            item.id
        );
        assert!(
            !item.follow_on.is_empty(),
            "seam item `{}` is named WITHOUT a follow-on — an invisible gap",
            item.id
        );
    }
    assert!(
        invisible.is_empty(),
        "0 invisible gaps required; these seam items are under-recorded: {invisible:?}"
    );
}

/// The named build-floors land in a post-M5 band (the swap trigger is the safety drills green —
/// proven reachable by AG-P24's E2E-2). The runtime is specifically the post-M5/execution slice.
#[test]
fn the_runtime_floor_is_banded_post_m5_execution_and_triggered_by_the_safety_drills() {
    let runtime = NAMED_FLOORS
        .iter()
        .find(|f| f.id == "llm-agent-runtime")
        .expect("the LlmAgentRuntime floor must be recorded");

    assert!(
        runtime.band_or_owner.contains("post-M5") && runtime.band_or_owner.contains("execution"),
        "the runtime floor lands post-M5/execution, got `{}`",
        runtime.band_or_owner
    );
    assert!(
        runtime.trigger.contains("safety drills"),
        "the runtime swap trigger is the safety drills green, got `{}`",
        runtime.trigger
    );
    assert!(
        runtime.follow_on.contains("NOT a rewrite") || runtime.follow_on.contains("config/impl"),
        "the runtime swap is a config/impl swap, NOT a rewrite (VISION §3), got `{}`",
        runtime.follow_on
    );

    // The post-M5 bands exist as distinct variants so the report is machine-readable.
    let _ = FollowOnBand::PostM5Execution;
    let _ = FollowOnBand::PostM5;
    let _ = FollowOnBand::PostM5OtherSystem;
}

/// THE `no-llm-in-platform` RE-CONFIRM (contract 1.6): the lint is green over the seam module's
/// source — the seam doc adds NO model/SDK/prompt/model-name fingerprint. A regression that lands
/// a forbidden literal in `seam.rs` turns this red.
#[test]
fn the_no_llm_lint_is_green_over_the_seam_module() {
    let seam_src = std::fs::read_to_string(crate_src().join("seam.rs"))
        .expect("the seam module src/seam.rs must exist");
    let violations = no_llm_in_platform().run(&seam_src);
    assert!(
        violations.is_empty(),
        "the seam doc must add NO model/SDK/prompt fingerprint (no-llm-in-platform 1.6), \
         but found: {violations:?}"
    );
}

/// Defence in depth: the lint is also green over the WHOLE crate src (lib.rs + seam.rs), the same
/// posture the live workspace gate holds — naming the floors does not introduce a model string.
#[test]
fn the_no_llm_lint_is_green_over_the_whole_agent_crate_src() {
    let src = crate_src();
    let mut all_violations = Vec::new();
    for entry in std::fs::read_dir(&src).expect("crate src dir must exist") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let body = std::fs::read_to_string(&path).expect("read rs file");
            let v = no_llm_in_platform().run(&body);
            if !v.is_empty() {
                all_violations.push((path, v));
            }
        }
    }
    assert!(
        all_violations.is_empty(),
        "no-llm-in-platform must be green over the whole agent crate src, found: {all_violations:?}"
    );
}
