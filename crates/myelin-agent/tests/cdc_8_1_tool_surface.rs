//! # The CDC pair for contract 8.1 — `ToolSurface::register_tool(ToolDef)` + `resolve`
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.1
//! (`ToolSurface::register_tool(ToolDef{name, input_schema, required_caps, effect_kind,
//! side_effecting, requires_approval, exposed_over_mcp})` + `resolve` — one permissioned catalogue,
//! MCP-exposable; per-subsystem `requires_approval` defaults frozen — but SEEDED in AG-P8 → P-220).
//! Owning architecture: `agent-fabric.md` §4.2 / §1.3. AG-P1 / P-130 ships the SIGNATURE half.
//!
//! ## What this pair pins (the signature half of 8.1)
//! - the **PROVIDER** is the agent fabric: it owns the [`ToolSurface`] trait + the [`ToolDef`]
//!   value type with its frozen field list. A `register_tool` followed by `resolve` round-trips
//!   every field, and an unknown name resolves to `None` (the catalogue contract).
//! - the **CONSUMER** is every subsystem (Issues/Git/CI/Knowledge/Chat) that contributes its
//!   actions: here a representative subsystem builds a `ToolDef` (the full field list) and registers
//!   it; the field shape is exactly what its migrations + the MCP projection consume.
//!
//! The persisted catalogue + the per-subsystem `requires_approval` defaults SEED are AG-P8
//! (→ P-220); the data-model migration is AG-P2 (→ P-131). This is the trait-signature pair AG-P1's
//! TESTS field names.

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};

/// **PROVIDER side of 8.1 (agent fabric).** The platform-owned catalogue: register then resolve.
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

/// **CONSUMER side of 8.1 (a subsystem).** A subsystem contributes a `ToolDef` to the one
/// catalogue; it builds the full frozen field list (the shape its migrations + the MCP projection
/// read).
fn consumer_tool() -> ToolDef {
    ToolDef {
        name: ToolName("issue.transition".into()),
        subsystem: "issues".into(),
        version: 1,
        input_schema: "{\"type\":\"object\"}".into(),
        required_caps: vec!["issue.transition".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // The COLUMN exists; the per-subsystem DEFAULT is seeded in AG-P8 (→ P-220).
        requires_approval: false,
        exposed_over_mcp: false,
    }
}

#[test]
fn cdc_8_1_register_then_resolve_round_trips_the_tool_def_field_list() {
    let mut catalogue = ProviderCatalogue { tools: vec![] };
    let consumer = consumer_tool();

    catalogue.register_tool(consumer.clone());

    // The consumer's registered def resolves back byte-for-byte (every field of the frozen list).
    let resolved = catalogue
        .resolve(&ToolName("issue.transition".into()))
        .expect("the registered tool resolves");
    assert_eq!(resolved, &consumer);

    // An unknown name resolves to None (the catalogue does not invent tools).
    assert!(catalogue.resolve(&ToolName("does.not.exist".into())).is_none());
}
