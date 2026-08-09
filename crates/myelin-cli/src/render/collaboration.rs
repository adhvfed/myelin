use std::fmt::Write as _;

use serde_json::Value;

use crate::dispatch::EdgeCall;

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value) -> Option<String> {
    if value.get("items").is_some() {
        return None;
    }
    if let Some(conversation) = value.get("conversation") {
        return render_conversation(conversation).map(|row| format!("{row}\n"));
    }
    if let Some(page) = value.get("page") {
        return render_page_document(page);
    }
    value
        .get("message_id")
        .and_then(Value::as_str)
        .map(|id| format!("sent ({})\n", terminal_safe_single_line(id)))
}

pub(super) fn render_collection_header(value: &Value) -> Option<String> {
    let conversation = value.get("conversation")?;
    render_conversation(conversation).map(|row| format!("{row}\n"))
}

pub(super) fn render_item(value: &Value) -> Option<String> {
    render_conversation(value)
        .or_else(|| render_message(value))
        .or_else(|| render_page_summary(value))
}

pub(super) fn page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    if !canonical_ulid(cursor) {
        return None;
    }
    let limit = call
        .query
        .as_deref()
        .and_then(|query| query_field(query, "limit"))?;
    let parsed_limit = limit.parse::<u16>().ok()?;
    if parsed_limit.to_string() != limit || !(1..=100).contains(&parsed_limit) {
        return None;
    }
    match call.path.as_str() {
        "/v1/chat/conversations" => Some(format!(
            "myelin chat list --limit {limit} --cursor {cursor}"
        )),
        "/v1/knowledge/pages" => Some(format!(
            "myelin kb page list --limit {limit} --cursor {cursor}"
        )),
        path => {
            let conversation = path
                .strip_prefix("/v1/chat/conversations/")?
                .strip_suffix("/messages")?;
            canonical_ulid(conversation).then(|| {
                format!("myelin chat history {conversation} --limit {limit} --before {cursor}")
            })
        }
    }
}

fn render_conversation(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let channel = value.get("channel")?.as_str()?;
    let topic = value.get("topic")?.as_str()?;
    Some(format!(
        "#{}  {}  ({})",
        terminal_safe_single_line(channel),
        terminal_safe_single_line(topic),
        terminal_safe_single_line(id),
    ))
}

fn render_message(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let content = value.get("content")?.as_str()?;
    let state = value.get("state")?.as_str()?;
    let author = if value.get("is_you").and_then(Value::as_bool) == Some(true) {
        "you"
    } else {
        value.get("author")?.as_str()?
    };
    let edited = if value.get("edited").and_then(Value::as_bool) == Some(true) {
        " edited"
    } else {
        ""
    };
    Some(format!(
        "{}  {}  [{}{}]  ({})",
        terminal_safe_single_line(author),
        terminal_safe_single_line(content),
        terminal_safe_single_line(state),
        edited,
        terminal_safe_single_line(id),
    ))
}

fn render_page_summary(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let title = value.get("title")?.as_str()?;
    let visibility = value.get("visibility")?.as_str()?;
    let version = value.get("version")?.as_u64()?;
    Some(format!(
        "{}  [{}]  v{}  ({})",
        terminal_safe_single_line(title),
        terminal_safe_single_line(visibility),
        version,
        terminal_safe_single_line(id),
    ))
}

fn render_page_document(value: &Value) -> Option<String> {
    let mut output = format!("{}\n", render_page_summary(value)?);
    let blocks = value.get("blocks")?.as_array()?;
    if blocks.is_empty() {
        output.push_str("  (empty page)\n");
    }
    for block in blocks {
        let kind = block.get("type")?.as_str()?;
        let markdown = block.get("markdown")?.as_str()?;
        let _ = writeln!(
            output,
            "  {}: {}",
            terminal_safe_single_line(kind),
            terminal_safe_single_line(markdown),
        );
    }
    Some(output)
}

fn canonical_ulid(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "01J00000000000000000000000";

    #[test]
    fn collaboration_rows_are_compact_and_terminal_safe() {
        assert_eq!(
            render_item(&json!({"id": ID, "channel": "eng\nops", "topic": "Release"})),
            Some(format!("#eng\\nops  Release  ({ID})"))
        );
        assert_eq!(
            render_item(&json!({
                "id": ID,
                "author": "opaque-author",
                "is_you": true,
                "content": "ready\nnext",
                "state": "active",
                "edited": false
            })),
            Some(format!("you  ready\\nnext  [active]  ({ID})"))
        );
        assert_eq!(
            render_item(&json!({
                "id": ID,
                "title": "Deploy safely",
                "visibility": "team",
                "version": 3
            })),
            Some(format!("Deploy safely  [team]  v3  ({ID})"))
        );
    }

    #[test]
    fn knowledge_documents_show_their_blocks_without_control_injection() {
        let output = render_response(&json!({
            "page": {
                "id": ID,
                "title": "Runbook",
                "visibility": "team",
                "version": 1,
                "blocks": [{"type": "heading", "markdown": "Recovery\nsteps"}]
            }
        }))
        .unwrap();
        assert_eq!(
            output,
            format!("Runbook  [team]  v1  ({ID})\n  heading: Recovery\\nsteps\n")
        );
    }

    #[test]
    fn every_collaboration_cursor_hint_round_trips_through_dispatch() {
        let chat = crate::dispatch::chat_dispatch(&["list", "--limit", "2"]).unwrap();
        let hint = page_command(&chat, ID).unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            crate::dispatch::chat_dispatch(&words[2..])
                .unwrap()
                .query
                .as_deref(),
            Some("limit=2&cursor=01J00000000000000000000000")
        );

        let history = crate::dispatch::chat_dispatch(&["history", ID, "--limit", "2"]).unwrap();
        let hint = page_command(&history, ID).unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            crate::dispatch::chat_dispatch(&words[2..])
                .unwrap()
                .query
                .as_deref(),
            Some("limit=2&before=01J00000000000000000000000")
        );

        let knowledge =
            crate::dispatch::knowledge_dispatch(&["page", "list", "--limit", "2"]).unwrap();
        let hint = page_command(&knowledge, ID).unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            crate::dispatch::knowledge_dispatch(&words[2..])
                .unwrap()
                .query
                .as_deref(),
            Some("limit=2&cursor=01J00000000000000000000000")
        );
    }
}
