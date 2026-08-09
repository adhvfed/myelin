use serde_json::Value;

use crate::dispatch::{is_canonical_project_id, EdgeCall};

use super::{query_field, terminal_safe_single_line};

pub(super) fn render_response(value: &Value) -> Option<String> {
    let project = value.get("project")?;
    let summary = render_project(project)?;
    let default_type = terminal_safe_single_line(project.get("default_issue_type_id")?.as_str()?);
    let disposition = match value.get("created").and_then(Value::as_bool) {
        Some(true) => "Created project",
        Some(false) => "Project already existed",
        None => "Project",
    };
    Some(format!(
        "{disposition}: {summary}\n  default issue type: {default_type}\n"
    ))
}

pub(super) fn render_item(value: &Value) -> Option<String> {
    render_project(value)
}

pub(super) fn page_command(call: &EdgeCall, cursor: &str) -> Option<String> {
    if call.path != "/v1/projects" || !is_canonical_project_id(cursor) {
        return None;
    }
    let limit = call
        .query
        .as_deref()
        .and_then(|query| query_field(query, "limit"))?;
    let parsed = limit.parse::<u32>().ok()?;
    if parsed.to_string() != limit || !(1..=100).contains(&parsed) {
        return None;
    }
    Some(format!(
        "myelin project list --limit {limit} --cursor {cursor}"
    ))
}

fn render_project(value: &Value) -> Option<String> {
    let id = value.get("id")?.as_str()?;
    let reference = value.get("ref")?.as_str()?;
    let name = value.get("name")?.as_str()?;
    let prefix = value.get("issue_prefix")?.as_str()?;
    if !is_canonical_project_id(id) || !reference.contains("/identity/project/") {
        return None;
    }
    Some(format!(
        "{}  [{}]  {}",
        terminal_safe_single_line(name),
        terminal_safe_single_line(prefix),
        terminal_safe_single_line(reference),
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::dispatch::project_dispatch;

    const ID: &str = "11111111-1111-1111-1111-111111111111";

    fn project() -> Value {
        json!({
            "id": ID,
            "ref": format!("myelin://acme/identity/project/{ID}"),
            "name": "Developer\nexperience",
            "issue_prefix": "DX",
            "default_issue_type_id": "22222222-2222-2222-2222-222222222222",
        })
    }

    #[test]
    fn project_responses_are_addressable_and_terminal_safe() {
        assert_eq!(
            render_response(&json!({"project": project(), "created": true})).unwrap(),
            format!(
                "Created project: Developer\\nexperience  [DX]  myelin://acme/identity/project/{ID}\n  default issue type: 22222222-2222-2222-2222-222222222222\n"
            )
        );
        assert!(render_item(&project())
            .unwrap()
            .contains("myelin://acme/identity/project/"));
    }

    #[test]
    fn project_pagination_hints_round_trip_through_dispatch() {
        let call = project_dispatch(&["list", "--limit", "7"], None).unwrap();
        let hint = page_command(&call, ID).unwrap();
        let words = hint.split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            project_dispatch(&words[2..], None)
                .unwrap()
                .query
                .as_deref(),
            Some("limit=7&cursor=11111111-1111-1111-1111-111111111111")
        );
    }
}
