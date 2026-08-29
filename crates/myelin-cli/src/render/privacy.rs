use serde_json::Value;

pub(super) fn render_response(value: &Value) -> Option<String> {
    if let Some(request) = value.get("request") {
        return render_request(request);
    }
    if let Some(certificate) = value.get("certificate") {
        return render_certificate(certificate);
    }
    if let Some(agent_data) = value.get("agent_data") {
        return render_status(agent_data);
    }
    value.get("erasure").and_then(render_erasure)
}

fn render_request(request: &Value) -> Option<String> {
    let id = canonical_request_id(request.get("id")?.as_str()?)?;
    if request.get("kind")?.as_str()? != "erasure" {
        return None;
    }
    let scope = scope_label(request.get("scope")?.as_str()?)?;
    let state = match request.get("state")?.as_str()? {
        "pending" => "pending",
        "processing" => "processing",
        "completed" => "completed",
        _ => return None,
    };
    let attempts = request.get("attempt_count")?.as_u64()?;
    let certificate_available = request.get("certificate_available")?.as_bool()?;
    let next = if certificate_available {
        format!("myelin privacy request certificate {id}")
    } else {
        format!("myelin privacy request status {id}")
    };
    Some(format!(
        "Privacy erasure request {id}: {state}.\nScope: {scope}. Attempts: {attempts}.\nNext: `{next}`.\n"
    ))
}

fn render_certificate(certificate: &Value) -> Option<String> {
    let request_id = canonical_request_id(certificate.get("request_id")?.as_str()?)?;
    if certificate.get("kind")?.as_str()? != "erasure" {
        return None;
    }
    let scope = scope_label(certificate.get("scope")?.as_str()?)?;
    let content_hash = certificate.get("content_hash")?.as_str()?;
    if !is_blake3_hash(content_hash) {
        return None;
    }
    let holders = certificate.get("holders")?.as_array()?;
    if holders.is_empty() || holders.len() > 64 {
        return None;
    }
    let mut rendered = format!("Privacy erasure certificate {request_id}.\nScope: {scope}.\n");
    for holder in holders {
        let name = holder.get("holder")?.as_str()?;
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return None;
        }
        if holder.get("operation")?.as_str()? != "erasure"
            || !holder.get("key_unrecoverable")?.as_bool()?
        {
            return None;
        }
        let records = holder.get("records_erased")?.as_u64()?;
        rendered.push_str(&format!(
            "- {}: {}; key unrecoverable.\n",
            super::terminal_safe_single_line(name),
            labelled_records(records, "record erased", "records erased")
        ));
    }
    rendered.push_str(&format!("Certificate hash: {content_hash}\n"));
    Some(rendered)
}

fn canonical_request_id(value: &str) -> Option<&str> {
    crate::dispatch::is_canonical_uuid(value).then_some(value)
}

fn scope_label(scope: &str) -> Option<&'static str> {
    match scope {
        "agent_data" => Some("agent traces and replay journals"),
        "chat_messages" => Some("messages you authored in Chat"),
        "git_pull_request_text" => Some("pull-request titles and bodies you authored in Git"),
        "issue_titles" => Some("Issue titles you authored"),
        _ => None,
    }
}

fn is_blake3_hash(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
        ("erasing", false) => format!(
            "Agent data erasure is incomplete: {}; new agent processing is blocked. \
             Retry `myelin privacy agent-data erase` to finish crypto-shredding.\n",
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
    fn interrupted_erasure_names_the_safe_recovery_command() {
        let rendered = render_response(&json!({
            "agent_data": {
                "scope": "agent_data",
                "state": "erasing",
                "recoverable_records": 2,
                "new_processing_allowed": false
            }
        }))
        .unwrap();
        assert!(rendered.contains("erasure is incomplete"));
        assert!(rendered.contains("myelin privacy agent-data erase"));
        assert!(rendered.contains("new agent processing is blocked"));
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

    #[test]
    fn durable_request_output_points_to_its_owned_certificate() {
        let rendered = render_response(&json!({
            "request": {
                "id": "01234567-89ab-cdef-0123-456789abcdef",
                "kind": "erasure",
                "scope": "chat_messages",
                "state": "completed",
                "attempt_count": 1,
                "certificate_available": true
            }
        }))
        .unwrap();
        assert!(rendered.contains("messages you authored in Chat"));
        assert!(rendered.contains("completed"));
        assert!(rendered
            .contains("myelin privacy request certificate 01234567-89ab-cdef-0123-456789abcdef"));
    }

    #[test]
    fn certificate_output_names_each_verified_holder_without_overclaiming() {
        let rendered = render_response(&json!({
            "certificate": {
                "request_id": "01234567-89ab-cdef-0123-456789abcdef",
                "kind": "erasure",
                "scope": "agent_data",
                "holders": [
                    {
                        "holder": "agent_traces",
                        "operation": "erasure",
                        "records_erased": 2,
                        "key_unrecoverable": true
                    },
                    {
                        "holder": "tool_effects",
                        "operation": "erasure",
                        "records_erased": 1,
                        "key_unrecoverable": true
                    }
                ],
                "content_hash": format!("blake3:{}", "a".repeat(64))
            }
        }))
        .unwrap();
        assert!(rendered.contains("agent_traces: 2 records erased"));
        assert!(rendered.contains("tool_effects: 1 record erased"));
        assert!(rendered.contains("key unrecoverable"));
        assert!(!rendered.contains("full account"));
    }
}
