//! Issues-owned command grammar for the product CLI.
//!
//! The top-level `myelin` binary owns authentication and transport only. Issues owns these verbs and
//! validates every identifier before a request can leave the machine. Tenant and region are absent
//! by construction: both always come from the verified capability token at the Edge.

/// The default and maximum page sizes exposed by the founder CLI floor.
pub const DEFAULT_CLI_PAGE_LIMIT: u32 = 50;
pub const MAX_CLI_PAGE_LIMIT: u32 = 100;
const MAX_TITLE_BYTES: usize = 512;

/// A fully parsed Issues command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    List {
        limit: u32,
        cursor: Option<String>,
    },
    Create {
        project_id: String,
        type_id: String,
        prefix: String,
        title: String,
    },
    View {
        issue_id: String,
    },
    Close {
        issue_id: String,
    },
}

/// Total parse failure. No variant contains tenant, region, or any server-derived detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliParseError {
    Empty,
    Unknown { token: String },
    MissingValue { flag: &'static str },
    DuplicateFlag { flag: &'static str },
    BadValue { field: &'static str, value: String },
    BadTitle,
}

impl core::fmt::Display for CliParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(
                f,
                "no Issues command given (try: list | create | view | close)"
            ),
            Self::Unknown { token } => write!(f, "unknown Issues command token `{token}`"),
            Self::MissingValue { flag } => write!(f, "missing value for `{flag}`"),
            Self::DuplicateFlag { flag } => write!(f, "duplicate flag `{flag}`"),
            Self::BadValue { field, value } => {
                write!(f, "malformed {field}: `{value}`")
            }
            Self::BadTitle => write!(f, "malformed title (expected 1..=512 bytes)"),
        }
    }
}

impl std::error::Error for CliParseError {}

/// Parse `myelin issues ...` arguments. Unknown, duplicate, missing, and surplus tokens are always
/// rejected; no parser branch silently ignores input.
pub fn parse_cli(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, rest) = args.split_first().ok_or(CliParseError::Empty)?;
    match *verb {
        "list" => parse_list(rest),
        "create" => parse_create(rest),
        "view" => parse_issue_id(rest, |issue_id| CliCommand::View { issue_id }),
        "close" => parse_issue_id(rest, |issue_id| CliCommand::Close { issue_id }),
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_list(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--limit" => {
                if limit.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--limit" });
                }
                let value = flag_value(args, &mut index, "--limit")?;
                let parsed = value.parse::<u32>().ok().filter(|n| (1..=100).contains(n));
                limit = Some(parsed.ok_or_else(|| CliParseError::BadValue {
                    field: "limit (expected 1..=100)",
                    value: value.to_string(),
                })?);
            }
            "--cursor" => {
                if cursor.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--cursor" });
                }
                let value = flag_value(args, &mut index, "--cursor")?;
                require_uuid("cursor UUID", value)?;
                cursor = Some(value.to_string());
            }
            other => {
                return Err(CliParseError::Unknown {
                    token: other.to_string(),
                });
            }
        }
        index += 1;
    }
    Ok(CliCommand::List {
        limit: limit.unwrap_or(DEFAULT_CLI_PAGE_LIMIT),
        cursor,
    })
}

fn parse_create(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let mut project_id = None;
    let mut type_id = None;
    let mut prefix = None;
    let mut title = None;
    let mut index = 0;
    while index < args.len() {
        let (slot, flag): (&mut Option<String>, &'static str) = match args[index] {
            "--project" => (&mut project_id, "--project"),
            "--type" => (&mut type_id, "--type"),
            "--prefix" => (&mut prefix, "--prefix"),
            "--title" => (&mut title, "--title"),
            other => {
                return Err(CliParseError::Unknown {
                    token: other.to_string(),
                });
            }
        };
        if slot.is_some() {
            return Err(CliParseError::DuplicateFlag { flag });
        }
        *slot = Some(flag_value(args, &mut index, flag)?.to_string());
        index += 1;
    }

    let project_id = required(project_id, "--project")?;
    let type_id = required(type_id, "--type")?;
    let prefix = required(prefix, "--prefix")?;
    let title = required(title, "--title")?;
    require_uuid("project UUID", &project_id)?;
    require_uuid("type UUID", &type_id)?;
    if prefix.len() < 2
        || prefix.len() > 10
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(CliParseError::BadValue {
            field: "prefix (expected 2..=10 uppercase ASCII letters/digits)",
            value: prefix,
        });
    }
    if title.is_empty() || title.len() > MAX_TITLE_BYTES {
        return Err(CliParseError::BadTitle);
    }
    Ok(CliCommand::Create {
        project_id,
        type_id,
        prefix,
        title,
    })
}

fn parse_issue_id(
    args: &[&str],
    command: impl FnOnce(String) -> CliCommand,
) -> Result<CliCommand, CliParseError> {
    let [value] = args else {
        if let Some(token) = args.get(1) {
            return Err(CliParseError::Unknown {
                token: (*token).to_string(),
            });
        }
        return Err(CliParseError::MissingValue { flag: "issue UUID" });
    };
    require_uuid("issue UUID", value)?;
    Ok(command((*value).to_string()))
}

fn flag_value<'a>(
    args: &'a [&str],
    index: &mut usize,
    flag: &'static str,
) -> Result<&'a str, CliParseError> {
    *index += 1;
    args.get(*index)
        .copied()
        .filter(|value| !value.starts_with("--"))
        .ok_or(CliParseError::MissingValue { flag })
}

fn required(value: Option<String>, flag: &'static str) -> Result<String, CliParseError> {
    value.ok_or(CliParseError::MissingValue { flag })
}

fn require_uuid(field: &'static str, value: &str) -> Result<(), CliParseError> {
    if is_canonical_uuid(value) {
        Ok(())
    } else {
        Err(CliParseError::BadValue {
            field,
            value: value.to_string(),
        })
    }
}

/// Canonical lowercase hyphenated UUID shape used by CLI and operator-bootstrap validation.
pub fn is_canonical_uuid(value: &str) -> bool {
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
    const TYPE: &str = "22222222-2222-2222-2222-222222222222";

    #[test]
    fn parses_every_founder_command_without_scope_selectors() {
        assert_eq!(
            parse_cli(&["list"]).unwrap(),
            CliCommand::List {
                limit: 50,
                cursor: None
            }
        );
        assert_eq!(
            parse_cli(&["list", "--limit", "1", "--cursor", PROJECT]).unwrap(),
            CliCommand::List {
                limit: 1,
                cursor: Some(PROJECT.into())
            }
        );
        assert_eq!(
            parse_cli(&[
                "create",
                "--project",
                PROJECT,
                "--type",
                TYPE,
                "--prefix",
                "ENG2",
                "--title",
                "Founder issue"
            ])
            .unwrap(),
            CliCommand::Create {
                project_id: PROJECT.into(),
                type_id: TYPE.into(),
                prefix: "ENG2".into(),
                title: "Founder issue".into()
            }
        );
        assert_eq!(
            parse_cli(&["view", PROJECT]).unwrap(),
            CliCommand::View {
                issue_id: PROJECT.into()
            }
        );
        assert_eq!(
            parse_cli(&["close", PROJECT]).unwrap(),
            CliCommand::Close {
                issue_id: PROJECT.into()
            }
        );
    }

    #[test]
    fn rejects_unknown_duplicate_missing_and_surplus_tokens() {
        assert!(matches!(parse_cli(&[]), Err(CliParseError::Empty)));
        assert!(matches!(
            parse_cli(&["list", "--all"]),
            Err(CliParseError::Unknown { .. })
        ));
        assert!(matches!(
            parse_cli(&["list", "--limit", "5", "--limit", "6"]),
            Err(CliParseError::DuplicateFlag { .. })
        ));
        assert!(matches!(
            parse_cli(&["create", "--project", PROJECT]),
            Err(CliParseError::MissingValue { .. })
        ));
        assert!(matches!(
            parse_cli(&["view", PROJECT, "extra"]),
            Err(CliParseError::Unknown { .. })
        ));
        assert!(matches!(
            parse_cli(&["close"]),
            Err(CliParseError::MissingValue { .. })
        ));
    }

    #[test]
    fn rejects_malformed_bounds_ids_and_prefixes_locally() {
        for limit in ["0", "101", "nope"] {
            assert!(matches!(
                parse_cli(&["list", "--limit", limit]),
                Err(CliParseError::BadValue { .. })
            ));
        }
        assert!(parse_cli(&["list", "--cursor", "not-a-uuid"]).is_err());
        assert!(parse_cli(&["view", "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"]).is_err());
        for prefix in ["E", "engineering", "ENG_TOO_LONG", "ENG-"] {
            assert!(parse_cli(&[
                "create",
                "--project",
                PROJECT,
                "--type",
                TYPE,
                "--prefix",
                prefix,
                "--title",
                "x"
            ])
            .is_err());
        }
    }

    #[test]
    fn invalid_free_text_is_never_reflected_by_the_parse_error() {
        let sensitive = format!("customer-secret-{}", "x".repeat(512));
        let error = parse_cli(&[
            "create",
            "--project",
            PROJECT,
            "--type",
            TYPE,
            "--prefix",
            "ENG",
            "--title",
            &sensitive,
        ])
        .unwrap_err();
        assert_eq!(error, CliParseError::BadTitle);
        assert!(!error.to_string().contains("customer-secret"));
        assert!(!format!("{error:?}").contains("customer-secret"));
    }
}
