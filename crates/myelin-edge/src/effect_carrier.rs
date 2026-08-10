use serde_json::Value;

pub(crate) fn parse_proposed(value: &str) -> Option<(String, Value)> {
    let rest = value.strip_prefix("tool:")?;
    let (tool, arguments) = rest.split_once("|args:")?;
    if tool.is_empty() {
        return None;
    }
    serde_json::from_str(arguments)
        .ok()
        .map(|arguments| (tool.to_string(), arguments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_effect_carriers_require_one_named_tool_and_valid_json() {
        let (tool, arguments) =
            parse_proposed(r#"tool:issues.create|args:{"title":"CI is red"}"#).unwrap();
        assert_eq!(tool, "issues.create");
        assert_eq!(arguments["title"], "CI is red");
        for malformed in [
            "garbage",
            "tool:|args:{}",
            "tool:issues.create|args:not-json",
        ] {
            assert!(
                parse_proposed(malformed).is_none(),
                "accepted `{malformed}`"
            );
        }
    }
}
