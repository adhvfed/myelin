use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};

struct ProviderCatalogue {
    tools: Vec<ToolDef>,
}

impl ToolSurface for ProviderCatalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.tools.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.tools.iter().find(|d| &d.name == name)
    }
}

fn consumer_tool() -> ToolDef {
    ToolDef {
        name: ToolName("issue.transition".into()),
        subsystem: "issues".into(),
        version: 1,
        input_schema: "{\"type\":\"object\"}".into(),
        required_caps: vec!["issue.transition".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

#[test]
fn cdc_8_1_register_then_resolve_round_trips_the_tool_def_field_list() {
    let mut catalogue = ProviderCatalogue { tools: vec![] };
    let consumer = consumer_tool();

    catalogue.register_tool(consumer.clone());

    let resolved = catalogue
        .resolve(&ToolName("issue.transition".into()))
        .expect("the registered tool resolves");
    assert_eq!(resolved, &consumer);

    assert!(catalogue
        .resolve(&ToolName("does.not.exist".into()))
        .is_none());
}
