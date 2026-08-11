use serde_json::json;

use super::{CliError, EdgeCall, FormQuery, RetryPolicy};

const DEFAULT_LIMIT: u16 = 50;

pub fn chat_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    chat_dispatch_with_project(args, None)
}

pub fn chat_dispatch_with_project(
    args: &[&str],
    default_project: Option<&str>,
) -> Result<EdgeCall, CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| {
        CliError::Usage(
            "no chat command (try: list | create <channel> --topic <topic> | history <id> | send <id> <message> | ref <id> <ArtifactRef>)"
                .into(),
        )
    })?;
    match *verb {
        "list" => {
            let page = PageArgs::parse(rest, "--cursor")?;
            Ok(page.call("/v1/chat/conversations", "cursor"))
        }
        "create" => create_conversation(rest, default_project),
        "history" => {
            let (conversation, flags) = target_and_flags("history", rest)?;
            canonical_ulid("conversation id", conversation)?;
            let page = PageArgs::parse(flags, "--before")?;
            Ok(page.call(
                &format!("/v1/chat/conversations/{conversation}/messages"),
                "before",
            ))
        }
        "send" | "post" => send_message(rest),
        "ref" => reference_message(rest),
        other => Err(CliError::Usage(format!(
            "unknown chat command `{other}` (try: list | create | history | send | ref)"
        ))),
    }
}

fn create_conversation(args: &[&str], default_project: Option<&str>) -> Result<EdgeCall, CliError> {
    let Some((channel, flags)) = args.split_first() else {
        return Err(CliError::Usage(
            "`chat create` needs a <channel> and --topic <topic>".into(),
        ));
    };
    clean_text("channel", channel, 255)?;
    let mut topic = None;
    let mut project = None;
    let mut index = 0;
    while index < flags.len() {
        let flag = flags[index];
        let value = flags
            .get(index + 1)
            .ok_or_else(|| CliError::Usage(format!("`chat create {flag}` needs a value")))?;
        match flag {
            "--topic" if topic.is_none() => topic = Some(*value),
            "--topic" => return Err(CliError::Usage("duplicate chat flag `--topic`".into())),
            "--project" if project.is_none() => project = Some(*value),
            "--project" => return Err(CliError::Usage("duplicate chat flag `--project`".into())),
            other => {
                return Err(CliError::Usage(format!(
                    "unknown chat create flag `{other}`"
                )))
            }
        }
        index += 2;
    }
    let topic =
        topic.ok_or_else(|| CliError::Usage("`chat create` needs --topic <topic>".into()))?;
    clean_text("topic", topic, 255)?;
    let project = project.or(default_project).ok_or_else(|| {
        CliError::Usage(
            "chat create needs a project; pass --project or run `myelin context use --project <project>`"
                .into(),
        )
    })?;
    if !super::is_canonical_project_id(project) {
        return Err(CliError::Usage(
            "chat project must be a canonical lowercase UUID".into(),
        ));
    }
    Ok(EdgeCall::post_json(
        "/v1/chat/conversations",
        json!({ "project_id": project, "channel": channel, "topic": topic }),
    ))
}

fn send_message(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [conversation, content] = args else {
        return Err(CliError::Usage(
            "`chat send` needs exactly one <conversation_id> and one quoted <message>".into(),
        ));
    };
    canonical_ulid("conversation id", conversation)?;
    if content.trim().is_empty()
        || content.len() > 32 * 1024
        || content.chars().any(|character| {
            character == '\0' || (character.is_control() && character != '\n' && character != '\t')
        })
    {
        return Err(CliError::Usage(
            "chat message must contain 1-32768 UTF-8 bytes and no unsupported controls".into(),
        ));
    }
    Ok(EdgeCall::post_json(
        format!("/v1/chat/conversations/{conversation}/messages"),
        json!({ "content": content }),
    ))
}

fn reference_message(args: &[&str]) -> Result<EdgeCall, CliError> {
    let [conversation, reference] = args else {
        return Err(CliError::Usage(
            "`chat ref` needs exactly one <conversation_id> and one <ArtifactRef>".into(),
        ));
    };
    canonical_ulid("conversation id", conversation)?;
    myelin_refs::parse_scoped(reference)
        .map_err(|error| CliError::Usage(format!("invalid ArtifactRef: {error}")))?;
    Ok(EdgeCall::post_json(
        format!("/v1/chat/conversations/{conversation}/messages"),
        json!({ "content": "\u{FFFC}", "references": [reference] }),
    ))
}

fn target_and_flags<'a>(
    verb: &str,
    args: &'a [&str],
) -> Result<(&'a str, &'a [&'a str]), CliError> {
    args.split_first()
        .map(|(target, flags)| (*target, flags))
        .ok_or_else(|| CliError::Usage(format!("`chat {verb}` needs a <conversation_id>")))
}

struct PageArgs<'a> {
    limit: u16,
    cursor: Option<&'a str>,
}

impl<'a> PageArgs<'a> {
    fn parse(args: &'a [&str], cursor_flag: &str) -> Result<PageArgs<'a>, CliError> {
        let mut limit = None;
        let mut cursor = None;
        let mut index = 0;
        while index < args.len() {
            let flag = args[index];
            let value = args
                .get(index + 1)
                .ok_or_else(|| CliError::Usage(format!("`chat {flag}` needs a value")))?;
            if flag == "--limit" && limit.is_none() {
                limit = Some(parse_limit(value)?);
            } else if flag == cursor_flag && cursor.is_none() {
                canonical_ulid("chat cursor", value)?;
                cursor = Some(*value);
            } else if flag == "--limit" || flag == cursor_flag {
                return Err(CliError::Usage(format!("duplicate chat flag `{flag}`")));
            } else {
                return Err(CliError::Usage(format!("unknown chat flag `{flag}`")));
            }
            index += 2;
        }
        Ok(PageArgs {
            limit: limit.unwrap_or(DEFAULT_LIMIT),
            cursor,
        })
    }

    fn call(self, path: &str, cursor_name: &str) -> EdgeCall {
        let mut query = FormQuery::default();
        query.push("limit", &self.limit.to_string());
        if let Some(cursor) = self.cursor {
            query.push(cursor_name, cursor);
        }
        EdgeCall {
            method: super::HttpMethod::Get,
            path: path.into(),
            query: Some(query.finish()),
            payload: None,
            idempotency_key: None,
            retry_policy: RetryPolicy::None,
        }
    }
}

fn parse_limit(value: &str) -> Result<u16, CliError> {
    let parsed = value
        .parse::<u16>()
        .map_err(|_| CliError::Usage("chat --limit must be an integer between 1 and 100".into()))?;
    if !(1..=100).contains(&parsed) || parsed.to_string() != value {
        return Err(CliError::Usage(
            "chat --limit must be an integer between 1 and 100".into(),
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

fn clean_text(label: &str, value: &str, maximum: usize) -> Result<(), CliError> {
    if value.trim() == value
        && !value.is_empty()
        && value.len() <= maximum
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "chat {label} must be 1-{maximum} clean UTF-8 bytes without surrounding whitespace"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::HttpMethod;

    const CONVERSATION: &str = "01J00000000000000000000000";
    const PROJECT: &str = "11111111-1111-1111-1111-111111111111";
    const OTHER_PROJECT: &str = "22222222-2222-2222-2222-222222222222";

    #[test]
    fn list_and_history_map_to_bounded_cursor_routes() {
        let list = chat_dispatch(&["list", "--limit", "25", "--cursor", CONVERSATION]).unwrap();
        assert_eq!(list.method, HttpMethod::Get);
        assert_eq!(list.path, "/v1/chat/conversations");
        assert_eq!(
            list.query.as_deref(),
            Some("limit=25&cursor=01J00000000000000000000000")
        );

        let history = chat_dispatch(&[
            "history",
            CONVERSATION,
            "--before",
            CONVERSATION,
            "--limit",
            "2",
        ])
        .unwrap();
        assert_eq!(
            history.path,
            format!("/v1/chat/conversations/{CONVERSATION}/messages")
        );
        assert_eq!(
            history.query.as_deref(),
            Some("limit=2&before=01J00000000000000000000000")
        );
    }

    #[test]
    fn create_send_and_ref_map_to_the_shared_public_mutations() {
        let create = chat_dispatch_with_project(
            &["create", "engineering", "--topic", "Release coordination"],
            Some(PROJECT),
        )
        .unwrap();
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.path, "/v1/chat/conversations");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(create.payload.as_deref().unwrap())
                .unwrap(),
            json!({
                "project_id": PROJECT,
                "channel": "engineering",
                "topic": "Release coordination"
            })
        );
        let explicit_project = chat_dispatch_with_project(
            &[
                "create",
                "engineering",
                "--topic",
                "Release coordination",
                "--project",
                OTHER_PROJECT,
            ],
            Some(PROJECT),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                explicit_project.payload.as_deref().unwrap()
            )
            .unwrap()["project_id"],
            OTHER_PROJECT,
        );

        let send = chat_dispatch(&["send", CONVERSATION, "Ready for review."]).unwrap();
        assert_eq!(send.method, HttpMethod::Post);
        assert_eq!(
            send.path,
            format!("/v1/chat/conversations/{CONVERSATION}/messages")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(send.payload.as_deref().unwrap()).unwrap(),
            json!({"content": "Ready for review."})
        );

        let reference = "myelin://acme/issue/issue/ENG-41";
        let reference_message = chat_dispatch(&["ref", CONVERSATION, reference]).unwrap();
        assert_eq!(reference_message.method, HttpMethod::Post);
        assert_eq!(
            reference_message.path,
            format!("/v1/chat/conversations/{CONVERSATION}/messages")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                reference_message.payload.as_deref().unwrap()
            )
            .unwrap(),
            json!({"content": "\u{FFFC}", "references": [reference]})
        );
    }

    #[test]
    fn malformed_chat_commands_fail_before_transport() {
        for args in [
            vec!["list", "--limit", "01"],
            vec!["history", "not-an-id"],
            vec!["send", CONVERSATION, ""],
            vec!["send", CONVERSATION],
            vec!["ref", CONVERSATION, "not-a-reference"],
            vec!["ref", CONVERSATION],
            vec!["create", "engineering"],
            vec!["create", "engineering", "--topic", "x"],
            vec!["create", " engineering", "--topic", "x"],
        ] {
            assert_eq!(chat_dispatch(&args).unwrap_err().code(), 2, "{args:?}");
        }
    }
}
