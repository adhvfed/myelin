//! # The provider CDC for contract 8.1 (the FROZEN §6.3 `requires_approval` defaults seed) +
//! the VISION §3 no-silent-loosening guard (AG-P8 → P-220)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.1
//! (`ToolSurface::register_tool` — the `requires_approval` column SEEDED from the FROZEN §6.3 table).
//! Owning architecture: `agent-fabric.md` §6.3 (the frozen defaults table) + §5.2 step 6 (the HITL
//! gate reads the §6.3 default). AG-P1 (→ P-130) froze the COLUMN; THIS pair pins the SEED VALUES +
//! the loosen-guard AG-P8 owns.
//!
//! The PROVIDER is the §6.3 seed ([`requires_approval_default`] / [`seed_requires_approval`]); the
//! CONSUMER is a subsystem `register_tool` path that stamps the frozen default onto its `ToolDef` and
//! is rejected if it silently loosens a consequential `yes → no` (VISION §3).

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    assert_no_silent_loosening, requires_approval_default, requires_approval_for_landing,
    seed_requires_approval, WrittenDeviation,
};

/// A simple in-memory `ToolSurface` (the §4.2 registry — the consumer of the seed).
struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

fn raw(subsystem: &str, name: &str, requires_approval: bool) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: subsystem.into(),
        version: 1,
        input_schema: "{}".into(),
        required_caps: vec![],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval,
        exposed_over_mcp: false,
    }
}

/// **PROVIDER+CONSUMER CDC for 8.1 — every subsystem's tool is SEEDED from the frozen §6.3 table at
/// registration; resolving it reads the frozen default.** The CONSUMER is a subsystem
/// `register_tool` path; the PROVIDER is the §6.3 seed. The defaults table is reproduced verbatim
/// across all five subsystems.
#[test]
fn cdc_8_1_register_seeds_the_frozen_6_3_default_for_every_subsystem() {
    let mut cat = Catalogue { defs: vec![] };

    // each subsystem registers a tool with an ARBITRARY (possibly wrong) requires_approval value;
    // the seed seam corrects it to the FROZEN §6.3 default before it enters the catalogue.
    let raws = [
        ("ci", "deploy", true),
        ("ci", "run_pipeline", true), // registered wrong (gated) → seeded NOT gated.
        ("git", "merge", false),      // registered wrong (un-gated) → seeded gated.
        ("git", "open_pr", true),     // registered wrong (gated) → seeded NOT gated.
        ("issues", "forecast", true),
        ("issues", "transition", false),
        ("knowledge", "publish", false),
        ("knowledge", "draft", true),
        ("chat", "post_message", true),
    ];
    for (sub, name, wrong) in raws {
        cat.register_tool(seed_requires_approval(raw(sub, name, wrong)));
    }

    // resolving each reads the FROZEN default — regardless of the value it was registered with.
    let expect = |name: &str, gated: bool| {
        let d = cat.resolve(&ToolName(name.into())).expect("registered");
        assert_eq!(
            d.requires_approval, gated,
            "{name} requires_approval seeded to the frozen §6.3 default ({gated})"
        );
    };
    expect("deploy", true);
    expect("run_pipeline", false);
    expect("merge", true);
    expect("open_pr", false);
    expect("forecast", false);
    expect("transition", true);
    expect("publish", true);
    expect("draft", false);
    expect("post_message", false);
}

/// **CONSUMER CDC — the cross-subsystem rule: a Chat-invoked effect that mutates another subsystem
/// inherits THAT subsystem's default ("governed where it lands", §6.3 last row).**
#[test]
fn cdc_8_1_cross_subsystem_effect_inherits_the_landing_default() {
    // a chat-invoked git.merge is governed where it LANDS (git → gated), not where invoked (chat).
    assert!(requires_approval_for_landing("chat", "git", "merge"));
    // a chat-invoked knowledge.draft lands in knowledge → reversible (NOT gated).
    assert!(!requires_approval_for_landing("chat", "knowledge", "draft"));
    // it equals the landing subsystem's plain default.
    assert_eq!(
        requires_approval_for_landing("chat", "ci", "deploy"),
        requires_approval_default("ci", "deploy")
    );
}

/// **The GATE fixture (8.1 / VISION §3) — a registration that silently LOOSENS a frozen consequential
/// `yes → no` is REJECTED; the same registration WITH a written deviation is admitted; a TIGHTENING
/// is always admitted.** This is the no-silent-loosening guard the gate requires.
#[test]
fn cdc_8_1_no_silent_loosening_gate() {
    // silent loosening of git.merge (frozen yes) to no → REJECTED.
    let loosened = raw("git", "merge", /* requires_approval */ false);
    assert!(
        assert_no_silent_loosening(&loosened, &[]).is_err(),
        "a silent yes→no loosening of a consequential action is rejected (VISION §3)"
    );

    // with a written deviation → admitted.
    let dev = WrittenDeviation::new("git", "merge", "audited auto-merge bot per tenant policy");
    assert!(assert_no_silent_loosening(&loosened, std::slice::from_ref(&dev)).is_ok());

    // tightening open_pr (frozen no) to gated → always admitted (no deviation needed).
    let tightened = raw("git", "open_pr", true);
    assert!(assert_no_silent_loosening(&tightened, &[]).is_ok());

    // a CI deploy (frozen yes) registered AT its frozen value → admitted (the seeded path).
    let seeded = raw("ci", "deploy", true);
    assert!(assert_no_silent_loosening(&seeded, &[]).is_ok());
}
