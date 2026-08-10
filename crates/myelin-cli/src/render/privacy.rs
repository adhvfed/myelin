use serde_json::Value;

pub(super) fn render_response(value: &Value) -> Option<String> {
    if let Some(agent_data) = value.get("agent_data") {
        return render_status(agent_data);
    }
    value.get("erasure").and_then(render_erasure)
}

fn render_status(agent_data: &Value) -> Option<String> {
    if agent_data.get("scope")?.as_str()? != "agent_data" {
        return None;
    }
    let state = agent_data.get("state")?.as_str()?;
    let count = agent_data.get("recoverable_records")?.as_u64()?;
    let processing_allowed = agent_data.get("new_processing_allowed")?.as_bool()?;
    let summary = match (state, processing_allowed) {
        ("active", true) => format!(
            "Agent data: {}; new agent processing is allowed.\n",
            recoverable_records(count)
        ),
        ("restricted", false) => format!(
            "Agent data restricted: {}; new agent processing is blocked.\n",
            recoverable_records(count)
        ),
        ("erased", false) => format!(
            "Agent data erased: {}; new agent processing is permanently blocked.\n",
            recoverable_records(count)
        ),
        _ => return None,
    };
    Some(format!(
        "{summary}Scope: agent traces and replay journals, not full account erasure. Erasure is irreversible.\n"
    ))
}

fn render_erasure(erasure: &Value) -> Option<String> {
    if erasure.get("scope")?.as_str()? != "agent_data" || !erasure.get("erased")?.as_bool()? {
        return None;
    }
    let already_erased = erasure.get("already_erased")?.as_bool()?;
    let total = erasure.get("records_erased")?.as_u64()?;
    let traces = erasure.get("traces_erased")?.as_u64()?;
    let model_steps = erasure.get("model_steps_erased")?.as_u64()?;
    let tool_effects = erasure.get("tool_effects_erased")?.as_u64()?;
    let key_unrecoverable = erasure.get("key_unrecoverable")?.as_bool()?;

    let first_line = if already_erased {
        "Agent data was already erased; no recoverable records remained.\n".to_string()
    } else {
        format!(
            "Erased {} ({}; {}; {}).\n",
            records(total),
            labelled_records(traces, "trace", "traces"),
            labelled_records(model_steps, "model replay", "model replays"),
            labelled_records(tool_effects, "tool effect", "tool effects"),
        )
    };
    let key_line = if key_unrecoverable {
        "The subject key is unrecoverable."
    } else {
        "Warning: the subject key could not be confirmed unrecoverable."
    };
    Some(format!(
        "{first_line}{key_line} New agent processing is permanently blocked.\nScope: agent traces and replay journals, not full account erasure.\n"
    ))
}

fn records(count: u64) -> String {
    labelled_records(count, "record", "records")
}

fn recoverable_records(count: u64) -> String {
    format!(
        "{count} recoverable {}",
        if count == 1 { "record" } else { "records" }
    )
}

fn labelled_records(count: u64, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn status_names_the_narrow_scope_and_irreversible_consequence() {
        let rendered = render_response(&json!({
            "agent_data": {
                "scope": "agent_data",
                "state": "active",
                "recoverable_records": 3,
                "new_processing_allowed": true
            }
        }))
        .unwrap();
        assert!(rendered.contains("3 recoverable records"));
        assert!(rendered.contains("not full account erasure"));
        assert!(rendered.contains("irreversible"));
    }

    #[test]
    fn receipt_is_specific_and_does_not_overclaim_account_erasure() {
        let rendered = render_response(&json!({
            "erasure": {
                "scope": "agent_data",
                "erased": true,
                "already_erased": false,
                "records_erased": 4,
                "traces_erased": 2,
                "model_steps_erased": 1,
                "tool_effects_erased": 1,
                "key_unrecoverable": true
            }
        }))
        .unwrap();
        assert!(rendered.contains("2 traces; 1 model replay; 1 tool effect"));
        assert!(rendered.contains("subject key is unrecoverable"));
        assert!(rendered.contains("permanently blocked"));
        assert!(rendered.contains("not full account erasure"));
    }
}
