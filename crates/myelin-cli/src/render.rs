//! # Rendering the edge view-model — human-readable by default, `--json` for scripts/agents.
//!
//! The edge serves the subsystem ViewModel DATA as JSON (the SAME vocabulary the UI renders — never a
//! parallel one). The CLI renders it two ways:
//! - **`--json`** — the raw JSON, pretty-printed, for scripting / an agent consumer (machine-readable);
//! - **default** — a compact human-readable form: the uniform `{items,page}` list as one line per
//!   item, the `whoami` principal as a single line, and an unknown shape falls back to terminal-safe
//!   JSON (total — it never panics on an unexpected shape).

use myelin_git::web::RepoListCursor;
use serde_json::Value;
use std::fmt::Write as _;

/// Make an untrusted server string safe to place on one terminal line. Printable Unicode is kept
/// intact; line separators and terminal-control bytes become visible ASCII escape sequences.
fn terminal_safe_single_line(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => safe.push_str("\\n"),
            '\r' => safe.push_str("\\r"),
            '\t' => safe.push_str("\\t"),
            '\u{2028}' => safe.push_str("\\u{2028}"),
            '\u{2029}' => safe.push_str("\\u{2029}"),
            character if character.is_control() => {
                let codepoint = character as u32;
                if codepoint <= 0xff {
                    let _ = write!(safe, "\\x{codepoint:02x}");
                } else {
                    let _ = write!(safe, "\\u{{{codepoint:x}}}");
                }
            }
            character => safe.push(character),
        }
    }
    safe
}

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
        if let Some(cursor) = value
            .get("page")
            .and_then(|p| p.get("next_cursor"))
            .and_then(Value::as_str)
        {
            if RepoListCursor::parse(cursor).is_ok() {
                let cursor = terminal_safe_single_line(cursor);
                out.push_str(&format!(
                    "… (more — run: myelin git repo list --cursor {cursor})\n"
                ));
            } else {
                out.push_str("… (more — pass --cursor to page)\n");
            }
        }
        return out;
    }
    // The whoami principal view-model.
    if let Some(pid) = value.get("principal_id").and_then(Value::as_str) {
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("?");
        let tenant = value.get("tenant").and_then(Value::as_str).unwrap_or("?");
        let region = value.get("region").and_then(Value::as_str).unwrap_or("?");
        let pid = terminal_safe_single_line(pid);
        let kind = terminal_safe_single_line(kind);
        let tenant = terminal_safe_single_line(tenant);
        let region = terminal_safe_single_line(region);
        return format!("{pid} ({kind})  tenant={tenant}  region={region}\n");
    }
    // Issues create is asynchronous. Render the durable receipt honestly and pair it with the exact
    // follow-up read; never claim the pending row is visible before the authorization reconciler.
    if let (Some(issue), Some(authorization)) = (value.get("issue"), value.get("authorization")) {
        if let (Some(id), Some(key)) = (
            issue.get("id").and_then(Value::as_str),
            issue.get("key").and_then(Value::as_str),
        ) {
            let status = authorization
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let id = terminal_safe_single_line(id);
            let key = terminal_safe_single_line(key);
            let status = terminal_safe_single_line(status);
            return format!(
                "{key} staged ({id}); authorization={status}\nnot visible yet; after reconciliation: myelin issues view {id}\n"
            );
        }
    }
    // An Issue view/close response.
    if is_issue(value) {
        return format!("{}\n", render_issue(value));
    }
    // An unknown shape: terminal-safe JSON (total — never a panic). Sanitize the serialized form too:
    // JSON permits DEL/C1 bytes unescaped, and those are still terminal controls in human mode.
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    format!("{}\n", terminal_safe_single_line(&serialized))
}

/// Render one list item to a line. Known shapes: a RepoHome (`slug`/`state`) and a code-search hit
/// (`repo`/`path`/`line`/`excerpt`); an unknown item falls back to compact JSON.
fn render_item(item: &Value) -> String {
    if is_issue(item) {
        return render_issue(item);
    }
    if let Some(slug) = item.get("slug").and_then(Value::as_str) {
        let state = item.get("state").and_then(Value::as_str).unwrap_or("?");
        let slug = terminal_safe_single_line(slug);
        let state = terminal_safe_single_line(state);
        return format!("{slug} [{state}]");
    }
    if let (Some(repo), Some(path)) = (
        item.get("repo").and_then(Value::as_str),
        item.get("path").and_then(Value::as_str),
    ) {
        let line = item.get("line").and_then(Value::as_i64).unwrap_or(0);
        let excerpt = item.get("excerpt").and_then(Value::as_str).unwrap_or("");
        let repo = terminal_safe_single_line(repo);
        let path = terminal_safe_single_line(path);
        let excerpt = terminal_safe_single_line(excerpt);
        return format!("{repo}:{path}:{line}  {excerpt}");
    }
    terminal_safe_single_line(&item.to_string())
}

fn is_issue(value: &Value) -> bool {
    value.get("id").and_then(Value::as_str).is_some()
        && value.get("key").and_then(Value::as_str).is_some()
        && value.get("title").and_then(Value::as_str).is_some()
}

fn render_issue(value: &Value) -> String {
    let key = terminal_safe_single_line(value.get("key").and_then(Value::as_str).unwrap_or("?"));
    let title = terminal_safe_single_line(value.get("title").and_then(Value::as_str).unwrap_or(""));
    let state =
        terminal_safe_single_line(value.get("state").and_then(Value::as_str).unwrap_or("?"));
    let id = terminal_safe_single_line(value.get("id").and_then(Value::as_str).unwrap_or("?"));
    format!("{key}  [{state}]  {title}  ({id})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_mode_is_pretty_raw() {
        let v = json!({
            "items": [
                {
                    "state": "populated",
                    "slug": "acme/alpha",
                    "clone_url": "/acme/eu-west/alpha.git"
                },
                {"state": "empty", "slug": "acme/empty"}
            ],
            "page": {"next_cursor": null, "limit": 50}
        });
        let out = render(&v, true);
        assert!(out.contains("\"slug\": \"acme/alpha\""));
        assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), v);
        assert!(!out.contains("readme_excerpt") && !out.contains("entries"));
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
        assert!(out.contains("acme/alpha [populated]"));
        assert!(out.contains("acme/beta [populated]"));
        assert!(!out.contains("more"), "no cursor → no more hint");
    }

    #[test]
    fn human_list_shows_more_hint_when_cursor_present() {
        let v = json!({"items":[{"slug":"a","state":"populated"}],"page":{"next_cursor":"2","limit":2}});
        assert!(render(&v, false).contains("more"));
    }

    #[test]
    fn repository_next_cursor_hint_round_trips_through_git_parser_and_dispatch() {
        let cursor = RepoListCursor::new([8; 32], "alpha").unwrap().encode();
        let rendered = render(
            &json!({
                "items": [{"slug": "acme/alpha", "state": "populated"}],
                "page": {"next_cursor": cursor, "limit": 1}
            }),
            false,
        );
        let command = rendered
            .lines()
            .find_map(|line| {
                line.strip_prefix("… (more — run: ")
                    .and_then(|line| line.strip_suffix(')'))
            })
            .expect("actionable next-page command");
        let words = command.split_whitespace().collect::<Vec<_>>();
        assert_eq!(&words[..4], &["myelin", "git", "repo", "list"]);
        let call = crate::dispatch::git_dispatch(&words[2..]).expect("hint parses and dispatches");
        assert_eq!(call.path, "/v1/git/repos");
        assert_eq!(
            call.query.as_deref(),
            Some(format!("view=summary&cursor={cursor}").as_str())
        );
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

    #[test]
    fn pending_issue_receipt_never_claims_immediate_visibility() {
        let id = "33333333-3333-3333-3333-333333333333";
        let out = render(
            &json!({
                "issue": {"id": id, "key": "ENG-1", "project_id": "p"},
                "authorization": {"status": "pending", "request_event_id": "evt"}
            }),
            false,
        );
        assert!(out.contains("authorization=pending"));
        assert!(out.contains("not visible yet"));
        assert!(out.contains(&format!("myelin issues view {id}")));
        assert!(!out.contains("created successfully"));
    }

    #[test]
    fn issue_list_and_view_have_a_human_row() {
        let issue = json!({
            "id":"33333333-3333-3333-3333-333333333333",
            "key":"ENG-1",
            "title":"Founder issue",
            "state":"open"
        });
        let row = render(&issue, false);
        assert!(row.contains("ENG-1  [open]  Founder issue"));
        let list = render(&json!({"items":[issue],"page":{"next_cursor":null}}), false);
        assert!(list.contains("ENG-1  [open]  Founder issue"));
    }

    #[test]
    fn human_issue_fields_escape_terminal_controls_but_preserve_printable_unicode() {
        let issue = json!({
            "id":"id\u{7f}tail",
            "key":"ENG\t1",
            "title":"Grüße 🚀\nsecond row\u{1b}[31mred\u{85}next",
            "state":"open\rclosed"
        });

        let out = render(&issue, false);
        assert_eq!(
            out.lines().count(),
            1,
            "untrusted fields cannot inject rows"
        );
        assert!(!out.contains('\u{1b}'));
        assert!(!out.contains('\r'));
        assert!(!out.contains('\t'));
        assert!(!out.contains('\u{7f}'));
        assert!(!out.contains('\u{85}'));
        assert!(out.contains("Grüße 🚀"), "printable Unicode survives");
        assert!(out.contains("\\nsecond row\\x1b[31mred\\x85next"));
        assert!(out.contains("ENG\\t1  [open\\rclosed]"));
        assert!(out.contains("id\\x7ftail"));

        let json = render(&issue, true);
        assert!(json.contains("Grüße 🚀"));
        assert!(json.contains("\\nsecond row"), "JSON keeps JSON escaping");
    }

    #[test]
    fn all_other_human_server_fields_are_terminal_safe() {
        let receipt = json!({
            "issue": {"id": "id\n2", "key": "ENG\u{1b}[2J"},
            "authorization": {"status": "pending\tunsafe"}
        });
        let receipt_out = render(&receipt, false);
        assert!(!receipt_out.contains('\u{1b}'));
        assert!(receipt_out.contains("id\\n2"));
        assert!(receipt_out.contains("ENG\\x1b[2J"));
        assert!(receipt_out.contains("pending\\tunsafe"));

        let page = json!({
            "items": [{"repo": "repo\nrow", "path": "p\u{1b}[A", "line": 1,
                       "excerpt": "Grüße\tthere"}],
            "page": {"next_cursor": null}
        });
        let page_out = render(&page, false);
        assert_eq!(page_out.lines().count(), 1);
        assert!(!page_out.contains('\u{1b}'));
        assert!(page_out.contains("repo\\nrow:p\\x1b[A:1  Grüße\\tthere"));

        let fallback_out = render(&json!({"unknown": "safe 🚀\u{7f}\u{85}"}), false);
        assert!(fallback_out.contains("safe 🚀\\x7f\\x85"));
        assert!(!fallback_out.contains('\u{7f}'));
        assert!(!fallback_out.contains('\u{85}'));
    }
}
