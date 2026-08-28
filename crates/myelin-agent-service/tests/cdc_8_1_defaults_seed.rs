use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    assert_no_silent_loosening, requires_approval_default, requires_approval_for_landing,
    seed_requires_approval, WrittenDeviation,
};

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

#[test]
fn cdc_8_1_register_seeds_the_frozen_6_3_default_for_every_subsystem() {
    let mut cat = Catalogue { defs: vec![] };

    let raws = [
        ("ci", "deploy", true),
        ("ci", "run_pipeline", true),
        ("git", "merge", false),
        ("git", "open_pr", true),
        ("issues", "create", true),
        ("issues", "close", false),
        ("knowledge", "link_work", true),
        ("chat", "post_message", true),
    ];
    for (sub, name, wrong) in raws {
        cat.register_tool(seed_requires_approval(raw(sub, name, wrong)));
    }

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
    expect("create", false);
    expect("close", true);
    expect("link_work", false);
    expect("post_message", false);
}

#[test]
fn cdc_8_1_cross_subsystem_effect_inherits_the_landing_default() {
    assert!(requires_approval_for_landing("chat", "git", "merge"));
    assert!(!requires_approval_for_landing(
        "chat",
        "knowledge",
        "link_work"
    ));
    assert_eq!(
        requires_approval_for_landing("chat", "ci", "deploy"),
        requires_approval_default("ci", "deploy")
    );
}

#[test]
fn cdc_8_1_no_silent_loosening_gate() {
    let loosened = raw("git", "merge", false);
    assert!(
        assert_no_silent_loosening(&loosened, &[]).is_err(),
        "a silent yes→no loosening of a consequential action is rejected (VISION §3)"
    );

    let dev = WrittenDeviation::new("git", "merge", "audited auto-merge bot per tenant policy");
    assert!(assert_no_silent_loosening(&loosened, std::slice::from_ref(&dev)).is_ok());

    let tightened = raw("git", "open_pr", true);
    assert!(assert_no_silent_loosening(&tightened, &[]).is_ok());

    let seeded = raw("ci", "deploy", true);
    assert!(assert_no_silent_loosening(&seeded, &[]).is_ok());
}
