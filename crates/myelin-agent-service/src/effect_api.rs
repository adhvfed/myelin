use myelin_agent::{ToolCall, ToolDef, ToolSurface};

pub fn validate_schema(input_schema: &str, input_json: &str) -> Result<(), String> {
    let input: serde_json::Value = serde_json::from_str(input_json)
        .map_err(|error| format!("input is not valid JSON: {error}"))?;
    let schema: serde_json::Value = serde_json::from_str(input_schema)
        .map_err(|error| format!("tool input_schema is not valid JSON: {error}"))?;

    let validator = jsonschema::draft202012::options()
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .build(&schema)
        .map_err(|error| format!("tool input_schema is not valid JSON Schema: {error}"))?;

    validator.validate(&input).map_err(|error| {
        let location = error.instance_path();
        if location.as_str().is_empty() {
            format!("tool arguments do not match input schema: {error}")
        } else {
            format!("tool arguments do not match input schema at `{location}`: {error}")
        }
    })
}

pub fn validate_tool_arguments(def: &ToolDef, arguments: &serde_json::Value) -> Result<(), String> {
    let input_json = serde_json::to_string(arguments)
        .map_err(|error| format!("tool arguments are not serialisable JSON: {error}"))?;
    validate_schema(&def.input_schema, &input_json)
}

pub fn validate_call<S: ToolSurface + ?Sized>(
    catalogue: &S,
    call: &ToolCall,
) -> Result<(), String> {
    let def = catalogue
        .resolve(&call.name)
        .ok_or_else(|| format!("tool `{}` is not registered in the catalogue", call.name.0))?;
    validate_tool_arguments(def, &call.arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolCallId, ToolName};
    use serde_json::json;

    const CLOSED_SCHEMA: &str = r#"{
        "type": "object",
        "required": ["name", "count", "state"],
        "properties": {
            "name": {"type": "string", "minLength": 2, "maxLength": 5, "pattern": "^[A-Z]+$"},
            "count": {"type": "integer", "minimum": 1, "maximum": 3},
            "state": {"type": "string", "enum": ["open", "closed"]}
        },
        "additionalProperties": false
    }"#;

    fn definition() -> ToolDef {
        ToolDef {
            name: ToolName("issues.example".into()),
            version: 1,
            subsystem: "issues".into(),
            input_schema: CLOSED_SCHEMA.into(),
            required_caps: Vec::new(),
            effect_kind: EffectKind::Mutate,
            requires_approval: false,
            side_effecting: true,
            exposed_over_mcp: true,
        }
    }

    #[test]
    fn the_advertised_schema_is_the_executed_schema() {
        assert!(
            validate_schema(CLOSED_SCHEMA, r#"{"name":"TEAM","count":2,"state":"open"}"#).is_ok()
        );

        for (input, rejected_constraint) in [
            (r#"{"count":2,"state":"open"}"#, "required"),
            (
                r#"{"name":"TEAM","count":2,"state":"open","surprise":true}"#,
                "additionalProperties",
            ),
            (r#"{"name":"t","count":2,"state":"open"}"#, "minLength"),
            (
                r#"{"name":"TOOLONG","count":2,"state":"open"}"#,
                "maxLength",
            ),
            (r#"{"name":"Team","count":2,"state":"open"}"#, "pattern"),
            (r#"{"name":"TEAM","count":0,"state":"open"}"#, "minimum"),
            (r#"{"name":"TEAM","count":4,"state":"open"}"#, "maximum"),
            (r#"{"name":"TEAM","count":2,"state":"other"}"#, "enum"),
        ] {
            let error = match validate_schema(CLOSED_SCHEMA, input) {
                Ok(()) => panic!("{rejected_constraint} must reject {input}"),
                Err(error) => error,
            };
            assert!(
                error.contains("do not match input schema"),
                "{rejected_constraint} returned an opaque error: {error}"
            );
        }
    }

    #[test]
    fn malformed_inputs_and_schemas_fail_closed() {
        assert!(validate_schema("{}", "not JSON")
            .unwrap_err()
            .contains("input is not valid JSON"));
        assert!(validate_schema("not JSON", "{}")
            .unwrap_err()
            .contains("input_schema is not valid JSON"));
        assert!(validate_schema(r#"{"type":"not-a-type"}"#, "{}")
            .unwrap_err()
            .contains("input_schema is not valid JSON Schema"));
        assert!(validate_schema("{}", r#"["any", "valid", "JSON"]"#).is_ok());
    }

    #[test]
    fn a_tool_call_is_resolved_then_validated() {
        struct Catalogue(ToolDef);

        impl ToolSurface for Catalogue {
            fn register_tool(&mut self, definition: ToolDef) {
                self.0 = definition;
            }

            fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
                (self.0.name == *name).then_some(&self.0)
            }
        }

        let catalogue = Catalogue(definition());
        let valid = ToolCall {
            id: ToolCallId("call-1".into()),
            name: ToolName("issues.example".into()),
            arguments: json!({"name": "TEAM", "count": 2, "state": "open"}),
        };
        assert!(validate_call(&catalogue, &valid).is_ok());

        let mut unknown = valid.clone();
        unknown.name = ToolName("issues.unknown".into());
        assert!(validate_call(&catalogue, &unknown)
            .unwrap_err()
            .contains("is not registered"));

        let mut invalid = valid;
        invalid.arguments = json!({"name": "TEAM", "count": 2, "state": "open", "extra": 1});
        assert!(validate_call(&catalogue, &invalid).is_err());
    }
}
