use serde_json::json;

use super::{CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};

const DEFAULT_LIMIT: u16 = 50;

pub fn knowledge_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    let (noun, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no knowledge command (try: page list | page get <id> | page create | page link <id> <ref>)".into(),
        )
    })?;
    if *noun != "page" {
        return Err(CliError::Usage(format!(
            "unknown knowledge noun `{noun}` (try: page)"
        )));
    }
    let (verb, rest) = rest.split_first().ok_or_else(|| {
        CliError::Usage(
            "no knowledge page command (try: list | get <id> | create | link <id> <ref>)".into(),
        )
    })?;
    match *verb {
        "list" => list_pages(rest),
        "get" | "show" => get_page(rest),
        "create" => create_page(rest),
        "link" => link_page(rest),
        other => Err(CliError::Usage(format!(
            "unknown knowledge page command `{other}` (try: list | get | create | link)"
        ))),
    }
}

fn list_pages(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("`kb page list {flag}` needs a value")))?;
        match flag {
            "--limit" if limit.is_none() => limit = Some(parse_limit(value)?),
            "--cursor" if cursor.is_none() => {
                canonical_ulid("Knowledge cursor", value)?;
                cursor = Some(*value);
            }
            "--limit" | "--cursor" => {
                return Err(CliError::Usage(format!(
                    "duplicate knowledge page list flag `{flag}`"
                )))
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown knowledge page list flag `{other}`"
                )))
            }
        }
        index += 2;
    }
    let mut query = FormQuery::default();
    query.push("limit", &limit.unwrap_or(DEFAULT_LIMIT).to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/knowledge/pages".into(),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn get_page(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [page] = args else {
        return Err(CliError::Usage(
            "`kb page get` needs exactly one <page_id>".into(),
        ));
    };
    canonical_ulid("Knowledge page id", page)?;
    Ok(EdgeCall::get(format!("/v1/knowledge/pages/{page}")))
}

fn create_page(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut title = None;
    let mut template = None;
    let mut visibility = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("`kb page create {flag}` needs a value")))?;
        match flag {
            "--title" if title.is_none() => title = Some(*value),
            "--template" if template.is_none() => template = Some(*value),
            "--visibility" if visibility.is_none() => visibility = Some(*value),
            "--title" | "--template" | "--visibility" => {
                return Err(CliError::Usage(format!(
                    "duplicate knowledge page create flag `{flag}`"
                )))
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown knowledge page create flag `{other}`"
                )))
            }
        }
        index += 2;
    }
    let title =
        title.ok_or_else(|| CliError::Usage("`kb page create` needs --title <title>".into()))?;
    clean_title(title)?;
    let template = template.unwrap_or("blank");
    if !matches!(template, "blank" | "product-spec" | "runbook") {
        return Err(CliError::Usage(
            "knowledge template must be blank, product-spec, or runbook".into(),
        ));
    }
    let visibility = visibility.unwrap_or("team");
    if !matches!(visibility, "private" | "team") {
        return Err(CliError::Usage(
            "knowledge visibility must be private or team".into(),
        ));
    }
    Ok(EdgeCall::post_json(
        "/v1/knowledge/pages",
        json!({
            "title": title,
            "template": template,
            "visibility": visibility,
        }),
    ))
}

fn link_page(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [page, reference, rest @ ..] = args else {
        return Err(CliError::Usage(
            "`myelin doc page link` needs <page_id> <myelin-ref> and optional --note <text>".into(),
        ));
    };
    canonical_ulid("Knowledge page id", page)?;
    let parsed = myelin_refs::parse_scoped(reference)
        .map_err(|error| CliError::Usage(format!("invalid Knowledge link reference: {error}")))?;
    if parsed.artifact_ref.0 != *reference || reference.len() > 1_024 {
        return Err(CliError::Usage(
            "Knowledge links require one canonical myelin:// reference up to 1024 bytes".into(),
        ));
    }

    let note = match rest {
        [] => None,
        ["--note", note] => {
            validate_link_note(note)?;
            Some(*note)
        }
        ["--note"] => {
            return Err(CliError::Usage(
                "`myelin doc page link --note` needs a value".into(),
            ))
        }
        _ => {
            return Err(CliError::Usage(
                "`myelin doc page link` accepts only one optional --note <text>".into(),
            ))
        }
    };
    let mut payload = json!({ "reference": reference });
    if let Some(note) = note {
        payload["note"] = json!(note);
    }
    Ok(EdgeCall::post_json(
        format!("/v1/knowledge/pages/{page}/links"),
        payload,
    ))
}

fn validate_link_note(value: &str) -> Result<(), CliError> {
    if !value.is_empty()
        && value.trim() == value
        && value.len() <= 4 * 1_024
        && !value.chars().any(char::is_control)
        && !value.contains('\u{fffc}')
    {
        Ok(())
    } else {
        Err(CliError::Usage(
            "Knowledge link note must be 1-4096 clean UTF-8 bytes without surrounding whitespace"
                .into(),
        ))
    }
}

fn parse_limit(value: &str) -> Result<u16, CliError> {
    let parsed = value.parse::<u16>().map_err(|_| {
        CliError::Usage("knowledge --limit must be an integer between 1 and 100".into())
    })?;
    if !(1..=100).contains(&parsed) || parsed.to_string() != value {
        return Err(CliError::Usage(
            "knowledge --limit must be an integer between 1 and 100".into(),
        ));
    }
    Ok(parsed)
}

fn canonical_ulid(label: &str, value: &str) -> Result<(), CliError> {
    if value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
    {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{label} must be a canonical uppercase ULID"
        )))
    }
}

fn clean_title(value: &str) -> Result<(), CliError> {
    if value.trim() == value
        && !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(CliError::Usage(
            "knowledge title must be 1-512 clean UTF-8 bytes without surrounding whitespace".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "01J00000000000000000000000";

    #[test]
    fn page_list_and_get_map_to_the_public_read_surface() {
        let list =
            knowledge_dispatch(&["page", "list", "--limit", "25", "--cursor", PAGE]).unwrap();
        assert_eq!(list.path, "/v1/knowledge/pages");
        assert_eq!(
            list.query.as_deref(),
            Some("limit=25&cursor=01J00000000000000000000000")
        );
        let get = knowledge_dispatch(&["page", "get", PAGE]).unwrap();
        assert_eq!(get.method, HttpMethod::Get);
        assert_eq!(get.path, format!("/v1/knowledge/pages/{PAGE}"));
    }

    #[test]
    fn page_create_has_safe_product_defaults_and_no_second_retry_token() {
        let call = knowledge_dispatch(&[
            "page",
            "create",
            "--title",
            "Deployment runbook",
            "--template",
            "runbook",
        ])
        .unwrap();
        assert_eq!(call.method, HttpMethod::Post);
        assert_eq!(call.path, "/v1/knowledge/pages");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(call.payload.as_deref().unwrap()).unwrap(),
            json!({
                "title": "Deployment runbook",
                "template": "runbook",
                "visibility": "team"
            })
        );
    }

    #[test]
    fn page_link_is_one_retry_safe_structured_context_write() {
        let reference = "myelin://acme/issue/issue/ENG-41";
        let call = knowledge_dispatch(&[
            "page",
            "link",
            PAGE,
            reference,
            "--note",
            "Delivery is tracked by",
        ])
        .unwrap();
        assert_eq!(call.method, HttpMethod::Post);
        assert_eq!(call.path, format!("/v1/knowledge/pages/{PAGE}/links"));
        assert_eq!(call.retry_policy, RetryPolicy::CallerKeyRequired);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(call.payload.as_deref().unwrap()).unwrap(),
            json!({
                "reference": reference,
                "note": "Delivery is tracked by",
            })
        );
    }

    #[test]
    fn malformed_knowledge_commands_fail_before_transport() {
        for args in [
            vec!["page", "list", "--limit", "0"],
            vec!["page", "get", "not-an-id"],
            vec!["page", "create"],
            vec!["page", "create", "--title", " bad"],
            vec!["page", "create", "--title", "x", "--template", "unknown"],
            vec!["page", "link", PAGE, "not-a-reference"],
            vec![
                "page",
                "link",
                PAGE,
                "myelin://acme/issue/issue/ENG-41",
                "--note",
                " bad",
            ],
            vec!["database", "list"],
        ] {
            assert_eq!(knowledge_dispatch(&args).unwrap_err().code(), 2, "{args:?}");
        }
    }
}
