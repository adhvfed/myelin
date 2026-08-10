use std::fmt::Write as _;

use serde_json::Value;

use crate::dispatch::EdgeCall;

use super::terminal_safe_single_line;

pub fn render_response(value: &Value, call: Option<&EdgeCall>) -> Option<String> {
    let root = value.get("root_ref")?.as_str()?;
    let items = value.get("items")?.as_array()?;
    let mut output = format!("Backlinks to {}\n", terminal_safe_single_line(root));
    if items.is_empty() {
        output.push_str("(no visible backlinks)\n");
    }
    for item in items {
        let relation = terminal_safe_single_line(item.get("relation")?.as_str()?);
        let source = terminal_safe_single_line(item.get("ref")?.as_str()?);
        let actor = terminal_safe_single_line(item.get("origin_actor")?.as_str()?);
        let _ = writeln!(output, "{relation:<10} {source}  by {actor}");
    }
    if let Some(cursor) = value
        .get("page")?
        .get("next_cursor")
        .and_then(Value::as_str)
    {
        let reference = call?
            .query
            .as_deref()?
            .split('&')
            .find_map(|pair| pair.strip_prefix("ref="))?;
        let decoded = percent_encoding::percent_decode_str(reference)
            .decode_utf8()
            .ok()?;
        let _ = writeln!(
            output,
            "… (more - run: myelin ref backlinks {} --cursor {})",
            terminal_safe_single_line(&decoded),
            terminal_safe_single_line(cursor)
        );
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_chain_reads_like_a_small_piece_of_work() {
        let rendered = render_response(
            &serde_json::json!({
                "root_ref": "myelin://acme/issue/issue/ENG-41",
                "items": [{
                    "ref": "myelin://acme/chat/message/M1#message-M1",
                    "relation": "links",
                    "origin_actor": "agent:release-helper"
                }],
                "page": { "next_cursor": null, "limit": 50 }
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            rendered,
            "Backlinks to myelin://acme/issue/issue/ENG-41\nlinks      myelin://acme/chat/message/M1#message-M1  by agent:release-helper\n"
        );
    }
}
