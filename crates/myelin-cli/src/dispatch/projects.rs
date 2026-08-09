use super::{CliError, EdgeCall, FormQuery, HttpMethod, RetryPolicy};
use serde_json::json;

const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;
const MAX_NAME_BYTES: usize = 100;
const MAX_PREFIX_BYTES: usize = 10;

pub fn project_dispatch(
    args: &[&str],
    default_project: Option<&str>,
) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no project command given (try: create <name> --prefix <key> | list | show [id])"
                .into(),
        )
    })?;
    match *verb {
        "create" => create_call(rest),
        "list" => list_call(rest),
        "show" => show_call(rest, default_project),
        other => Err(CliError::Usage(format!(
            "unknown project command token `{other}`"
        ))),
    }
}

fn create_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut name = None;
    let mut prefix = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--prefix" => {
                if prefix.is_some() {
                    return Err(CliError::Usage(
                        "duplicate project create flag `--prefix`".into(),
                    ));
                }
                prefix = Some(flag_value(args, &mut index, "--prefix")?);
            }
            token if !token.starts_with('-') && name.is_none() => name = Some(token),
            token if !token.starts_with('-') => {
                return Err(CliError::Usage(format!(
                    "unexpected project create argument `{token}`"
                )))
            }
            flag => {
                return Err(CliError::Usage(format!(
                    "unknown project create flag `{flag}`"
                )))
            }
        }
        index += 1;
    }

    let name = name.ok_or_else(|| CliError::Usage("project create needs a name".into()))?;
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(CliError::Usage(format!(
            "project name must contain 1..={MAX_NAME_BYTES} bytes, without surrounding whitespace or control characters"
        )));
    }
    let prefix = prefix.ok_or_else(|| {
        CliError::Usage("project create needs --prefix <2..10 uppercase letters/digits>".into())
    })?;
    if prefix.len() < 2
        || prefix.len() > MAX_PREFIX_BYTES
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(CliError::Usage(
            "project prefix must be 2..=10 uppercase ASCII letters/digits".into(),
        ));
    }
    Ok(EdgeCall::post_json(
        "/v1/projects",
        json!({ "name": name, "issue_prefix": prefix }),
    ))
}

fn list_call(args: &[&str]) -> Result<EdgeCall, CliError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        let (slot, flag): (&mut Option<&str>, &str) = match args[index] {
            "--limit" => (&mut limit, "--limit"),
            "--cursor" => (&mut cursor, "--cursor"),
            token => {
                return Err(CliError::Usage(format!(
                    "unknown project list flag `{token}`"
                )))
            }
        };
        if slot.is_some() {
            return Err(CliError::Usage(format!(
                "duplicate project list flag `{flag}`"
            )));
        }
        *slot = Some(flag_value(args, &mut index, flag)?);
        index += 1;
    }

    let limit = match limit {
        Some(value) => {
            let parsed = value.parse::<u32>().map_err(|_| {
                CliError::Usage("project list limit must be an integer between 1 and 100".into())
            })?;
            if !(1..=MAX_PAGE_LIMIT).contains(&parsed) || parsed.to_string() != value {
                return Err(CliError::Usage(
                    "project list limit must be an integer between 1 and 100".into(),
                ));
            }
            parsed
        }
        None => DEFAULT_PAGE_LIMIT,
    };
    if let Some(cursor) = cursor {
        require_uuid("project cursor", cursor)?;
    }

    let mut query = FormQuery::default();
    query.push("limit", &limit.to_string());
    if let Some(cursor) = cursor {
        query.push("cursor", cursor);
    }
    Ok(EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/projects".into(),
        query: Some(query.finish()),
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    })
}

fn show_call(args: &[&str], default_project: Option<&str>) -> Result<EdgeCall, CliError> {
    let project = match args {
        [] => default_project.ok_or_else(|| {
            CliError::Usage(
                "project show needs an id or an active project; run `myelin context use --project <id>`"
                    .into(),
            )
        })?,
        [project] => project,
        [_, extra, ..] => {
            return Err(CliError::Usage(format!(
                "unexpected project show argument `{extra}`"
            )))
        }
    };
    require_uuid("project id", project)?;
    Ok(EdgeCall::get(format!("/v1/projects/{project}")))
}

fn flag_value<'a>(args: &'a [&str], index: &mut usize, flag: &str) -> Result<&'a str, CliError> {
    *index += 1;
    args.get(*index)
        .copied()
        .ok_or_else(|| CliError::Usage(format!("`{flag}` needs a value")))
}

fn require_uuid(label: &str, value: &str) -> Result<(), CliError> {
    if is_canonical_project_id(value) {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "{label} must be a canonical lowercase UUID"
        )))
    }
}

pub fn is_canonical_project_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "11111111-1111-1111-1111-111111111111";

    #[test]
    fn project_commands_are_natural_strict_and_context_aware() {
        let create =
            project_dispatch(&["create", "Developer experience", "--prefix", "DX"], None).unwrap();
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.path, "/v1/projects");
        assert_eq!(create.retry_policy, RetryPolicy::CallerKeyRequired);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(create.payload.as_ref().unwrap()).unwrap(),
            json!({"name": "Developer experience", "issue_prefix": "DX"})
        );

        let list = project_dispatch(&["list", "--limit", "7", "--cursor", PROJECT], None).unwrap();
        assert_eq!(
            list.query.as_deref(),
            Some("limit=7&cursor=11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(
            project_dispatch(&["show"], Some(PROJECT)).unwrap().path,
            format!("/v1/projects/{PROJECT}")
        );
    }

    #[test]
    fn malformed_project_commands_fail_before_transport() {
        for args in [
            vec!["create", "DX"],
            vec!["create", "DX", "--prefix", "lower"],
            vec!["create", "one", "two", "--prefix", "DX"],
            vec!["list", "--limit", "0"],
            vec!["list", "--limit", "101"],
            vec!["list", "--cursor", "not-an-id"],
            vec!["show"],
            vec!["show", "not-an-id"],
        ] {
            assert!(project_dispatch(&args, None).is_err(), "accepted {args:?}");
        }
    }
}
