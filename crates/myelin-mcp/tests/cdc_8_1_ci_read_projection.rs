use myelin_agent::EffectKind;
use myelin_ci_controlplane::ci_tool_def;
#[path = "support/registry.rs"]
mod registry_support;

use registry_support::registry_for_subsystems;
use serde_json::Value;

#[test]
fn cdc_8_1_ci_provider_and_mcp_consumer_are_byte_aligned() {
    let registry = registry_for_subsystems(&["git", "ci"]);
    for name in ["read_run", "read_log"] {
        let provider = ci_tool_def(name);
        let consumer = registry
            .resolve(&format!("ci.{name}"))
            .expect("exposed CI definition");
        assert!(provider.exposed_over_mcp);
        assert_eq!(consumer.effect_kind(), EffectKind::Read);
        assert!(!consumer.side_effecting());
        assert!(!consumer.requires_approval());
        assert_eq!(
            consumer.required_caps(),
            provider
                .required_caps
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            consumer.to_mcp_json()["inputSchema"],
            serde_json::from_str::<Value>(&provider.input_schema).unwrap()
        );
    }

    for internal_only in ["run", "cancel_run", "validate", "plan", "deploy"] {
        assert!(
            registry.resolve(&format!("ci.{internal_only}")).is_none(),
            "MCP must not promise an unimplemented CI adapter"
        );
    }
}
