//! # Rendering the edge view-model — human-readable by default, `--json` for scripts/agents.
//!
//! The edge serves the subsystem ViewModel DATA as JSON (the SAME vocabulary the UI renders — never a
//! parallel one). The CLI renders it two ways:
//! - **`--json`** — the raw JSON, pretty-printed, for scripting / an agent consumer (machine-readable);
//! - **default** — a compact human-readable form: the uniform `{items,page}` list as one line per
//!   item, the `whoami` principal as a single line, and an unknown shape falls back to pretty JSON
//!   (total — it never panics on an unexpected shape).

use serde_json::Value;

/// Render an edge response value. In `json_mode` the raw JSON is pretty-printed; otherwise a compact
/// human form is produced (the `{items,page}` list, the whoami line, or a JSON fallback).
pub fn render(value: &Value, json_mode: bool) -> String {
    if json_mode {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    }
    // The uniform list envelope: one line per item + an optional "more" hint.
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        let mut out = String::new();
        if items.is_empty() {
            out.push_str("(no items)\n");
        }
        for item in items {
            out.push_str(&render_item(item));
            out.push('\n');
        }
        if value
            .get("page")
            .and_then(|p| p.get("next_cursor"))
            .map(|c| !c.is_null())
            .unwrap_or(false)
        {
            out.push_str("… (more — pass --cursor to page)\n");
        }
        return out;
    }
    // The whoami principal view-model.
    if let Some(pid) = value.get("principal_id").and_then(Value::as_str) {
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("?");
        let tenant = value.get("tenant").and_then(Value::as_str).unwrap_or("?");
        let region = value.get("region").and_then(Value::as_str).unwrap_or("?");
        return format!("{pid} ({kind})  tenant={tenant}  region={region}\n");
    }
    // An unknown shape: pretty JSON (total — never a panic).
    format!("{}\n", serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
}

/// Render one list item to a line. Known shapes: a RepoHome (`slug`/`state`) and a code-search hit
/// (`repo`/`path`/`line`/`excerpt`); an unknown item falls back to compact JSON.
fn render_item(item: &Value) -> String {
    if let Some(slug) = item.get("slug").and_then(Value::as_str) {
        let state = item.get("state").and_then(Value::as_str).unwrap_or("?");
        return format!("{slug}  [{state}]");
    }
    if let (Some(repo), Some(path)) = (
        item.get("repo").and_then(Value::as_str),
        item.get("path").and_then(Value::as_str),
    ) {
        let line = item.get("line").and_then(Value::as_i64).unwrap_or(0);
        let excerpt = item.get("excerpt").and_then(Value::as_str).unwrap_or("");
        return format!("{repo}:{path}:{line}  {excerpt}");
    }
    item.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_mode_is_pretty_raw() {
        let v = json!({"items":[{"slug":"acme/alpha","state":"populated"}],"page":{"next_cursor":null,"limit":50}});
        let out = render(&v, true);
        assert!(out.contains("\"slug\": \"acme/alpha\""));
    }

    #[test]
    fn human_repo_list_is_one_line_per_repo() {
        let v = json!({
            "items":[
                {"slug":"acme/alpha","state":"populated"},
                {"slug":"acme/beta","state":"populated"}
            ],
            "page":{"next_cursor":null,"limit":50}
        });
        let out = render(&v, false);
        assert!(out.contains("acme/alpha  [populated]"));
        assert!(out.contains("acme/beta  [populated]"));
        assert!(!out.contains("more"), "no cursor → no more hint");
    }

    #[test]
    fn human_list_shows_more_hint_when_cursor_present() {
        let v = json!({"items":[{"slug":"a","state":"populated"}],"page":{"next_cursor":"2","limit":2}});
        assert!(render(&v, false).contains("more"));
    }

    #[test]
    fn human_whoami_is_a_single_line() {
        let v = json!({"principal_id":"svc:agent","kind":"service","tenant":"acme","region":"eu-west"});
        let out = render(&v, false);
        assert!(out.contains("svc:agent (service)"));
        assert!(out.contains("tenant=acme"));
    }

    #[test]
    fn human_search_hit_renders_repo_path_line() {
        let v = json!({"items":[{"repo":"myelin","path":"src/x.rs","line":3,"excerpt":"fn x"}],"page":{"next_cursor":null}});
        assert!(render(&v, false).contains("myelin:src/x.rs:3  fn x"));
    }

    #[test]
    fn unknown_shape_falls_back_to_json_no_panic() {
        let out = render(&json!({"weird":[1,2,3]}), false);
        assert!(out.contains("weird"));
        // empty list renders the explicit marker.
        assert!(render(&json!({"items":[]}), false).contains("(no items)"));
    }
}
