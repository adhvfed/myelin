use crate::import::SourceSystem;
use crate::pg_issue_store::{is_valid_issue_title, MAX_TITLE_BYTES};
use crate::refs_glue::IssueLifecycleRel;

pub const DEFAULT_CLI_PAGE_LIMIT: u32 = 50;
pub const MAX_CLI_PAGE_LIMIT: u32 = 100;
pub const MAX_ISSUE_KEY_PREFIX_BYTES: usize = 32;
pub const MAX_ISSUE_CURSOR_BYTES: usize = 192;
pub const MAX_ISSUE_IMPORT_JSON_BYTES: usize = 512 * 1024;
pub const MAX_ISSUE_IMPORT_RECORDS: usize = 256;
const CURSOR_VERSION: u8 = 1;
const CURSOR_PREFIX: &str = "ic_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueListState {
    Open,
    Closed,
    All,
}

impl IssueListState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }

    fn wire(self) -> u8 {
        match self {
            Self::Open => 0,
            Self::Closed => 1,
            Self::All => 2,
        }
    }

    fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Open),
            1 => Some(Self::Closed),
            2 => Some(Self::All),
            _ => None,
        }
    }
}

pub fn normalize_issue_key_prefix(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_ISSUE_KEY_PREFIX_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(value.to_ascii_uppercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuePageCursor {
    pub updated_at_micros: i64,
    pub issue_id: String,
}

pub fn encode_issue_page_cursor(
    state: IssueListState,
    key: Option<&str>,
    updated_at_micros: i64,
    issue_id: &str,
) -> Result<String, &'static str> {
    if updated_at_micros <= 0 || !is_canonical_uuid(issue_id) {
        return Err("invalid cursor position");
    }
    let key = match key {
        Some(value) => Some(normalize_issue_key_prefix(value).ok_or("invalid key prefix")?),
        None => None,
    };
    let key = key.as_deref().unwrap_or("");
    let mut payload = Vec::with_capacity(3 + key.len() + 8 + issue_id.len());
    payload.push(CURSOR_VERSION);
    payload.push(state.wire());
    payload.push(key.len() as u8);
    payload.extend_from_slice(key.as_bytes());
    payload.extend_from_slice(&updated_at_micros.to_be_bytes());
    payload.extend_from_slice(issue_id.as_bytes());
    let mut encoded = String::with_capacity(CURSOR_PREFIX.len() + payload.len() * 2);
    encoded.push_str(CURSOR_PREFIX);
    for byte in payload {
        encoded.push(hex_digit(byte >> 4));
        encoded.push(hex_digit(byte & 0x0f));
    }
    Ok(encoded)
}

pub fn decode_issue_page_cursor(
    cursor: &str,
    expected_state: IssueListState,
    expected_key: Option<&str>,
) -> Result<IssuePageCursor, &'static str> {
    if cursor.len() > MAX_ISSUE_CURSOR_BYTES || !cursor.starts_with(CURSOR_PREFIX) {
        return Err("malformed or unsupported cursor");
    }
    let hex = &cursor[CURSOR_PREFIX.len()..];
    if !hex.len().is_multiple_of(2) {
        return Err("malformed or unsupported cursor");
    }
    let mut payload = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        payload.push((decode_hex(pair[0])? << 4) | decode_hex(pair[1])?);
    }
    if payload.len() < 3 + 8 + 36 || payload[0] != CURSOR_VERSION {
        return Err("malformed or unsupported cursor");
    }
    let state = IssueListState::from_wire(payload[1]).ok_or("malformed or unsupported cursor")?;
    let key_len = payload[2] as usize;
    if key_len > MAX_ISSUE_KEY_PREFIX_BYTES || payload.len() != 3 + key_len + 8 + 36 {
        return Err("malformed or unsupported cursor");
    }
    let key_bytes = &payload[3..3 + key_len];
    let key = core::str::from_utf8(key_bytes).map_err(|_| "malformed or unsupported cursor")?;
    let decoded_key = if key.is_empty() {
        None
    } else {
        Some(normalize_issue_key_prefix(key).ok_or("malformed or unsupported cursor")?)
    };
    let expected_key = match expected_key {
        Some(value) => Some(normalize_issue_key_prefix(value).ok_or("invalid key prefix")?),
        None => None,
    };
    if state != expected_state || decoded_key != expected_key {
        return Err("cursor does not match list filters");
    }
    let timestamp_start = 3 + key_len;
    let updated_at_micros = i64::from_be_bytes(
        payload[timestamp_start..timestamp_start + 8]
            .try_into()
            .map_err(|_| "malformed or unsupported cursor")?,
    );
    let issue_id = core::str::from_utf8(&payload[timestamp_start + 8..])
        .map_err(|_| "malformed or unsupported cursor")?
        .to_string();
    if updated_at_micros <= 0 || !is_canonical_uuid(&issue_id) {
        return Err("malformed or unsupported cursor");
    }
    Ok(IssuePageCursor {
        updated_at_micros,
        issue_id,
    })
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + value - 10) as char,
    }
}

fn decode_hex(value: u8) -> Result<u8, &'static str> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err("malformed or unsupported cursor"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportMode {
    DryRun,
    Run,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    List {
        state: IssueListState,
        key: Option<String>,
        limit: u32,
        cursor: Option<String>,
    },
    Create {
        project_id: String,
        type_id: Option<String>,
        prefix: Option<String>,
        title: String,
    },
    Import {
        source: SourceSystem,
        job_id: String,
        input: String,
        mode: ImportMode,
    },
    View {
        issue_id: String,
    },
    Close {
        issue_id: String,
    },
    ListRelations {
        issue_id: String,
    },
    CreateRelation {
        issue_id: String,
        relation: IssueLifecycleRel,
        target_ref: String,
    },
    RemoveRelation {
        issue_id: String,
        relation_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliParseError {
    Empty,
    Unknown { token: String },
    MissingValue { flag: &'static str },
    DuplicateFlag { flag: &'static str },
    BadValue { field: &'static str, value: String },
    BadTitle,
    InvalidCombination(&'static str),
}

impl core::fmt::Display for CliParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(
                f,
                "no Issues command given (try: list | create | import | view | close | relation)"
            ),
            Self::Unknown { token } => write!(f, "unknown Issues command token `{token}`"),
            Self::MissingValue { flag } => write!(f, "missing value for `{flag}`"),
            Self::DuplicateFlag { flag } => write!(f, "duplicate flag `{flag}`"),
            Self::BadValue { field, value } => {
                write!(f, "malformed {field}: `{value}`")
            }
            Self::BadTitle => write!(
                f,
                "malformed title (expected 1..={MAX_TITLE_BYTES} bytes without surrounding whitespace or control characters)"
            ),
            Self::InvalidCombination(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CliParseError {}

pub fn parse_cli(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (verb, rest) = args.split_first().ok_or(CliParseError::Empty)?;
    match *verb {
        "list" => parse_list(rest),
        "create" => parse_create(rest),
        "import" => parse_import(rest),
        "view" => parse_issue_id(rest, |issue_id| CliCommand::View { issue_id }),
        "close" => parse_issue_id(rest, |issue_id| CliCommand::Close { issue_id }),
        "relation" => parse_relation(rest),
        other => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_relation(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let (action, operands) = args.split_first().ok_or(CliParseError::MissingValue {
        flag: "relation action (list|add|remove)",
    })?;
    match (*action, operands) {
        ("list", [issue_id]) => {
            require_uuid("issue UUID", issue_id)?;
            Ok(CliCommand::ListRelations {
                issue_id: (*issue_id).to_string(),
            })
        }
        ("add", [issue_id, relation, target_ref]) => {
            require_uuid("issue UUID", issue_id)?;
            let relation = IssueLifecycleRel::from_token(relation).ok_or_else(|| {
                CliParseError::BadValue {
                    field: "relation (expected parent|blocks|blocked_by|closes|depends_on|relates)",
                    value: (*relation).to_string(),
                }
            })?;
            let parsed = myelin_refs::parse_scoped(target_ref).map_err(|_| {
                CliParseError::BadValue {
                    field: "target (expected a canonical issue reference)",
                    value: (*target_ref).to_string(),
                }
            })?;
            if parsed.artifact_ref.0 != *target_ref
                || parsed.subsystem != "issue"
                || parsed.type_ != "issue"
                || parsed.sub.is_some()
            {
                return Err(CliParseError::BadValue {
                    field: "target (expected a canonical issue reference)",
                    value: (*target_ref).to_string(),
                });
            }
            Ok(CliCommand::CreateRelation {
                issue_id: (*issue_id).to_string(),
                relation,
                target_ref: (*target_ref).to_string(),
            })
        }
        ("remove", [issue_id, relation_id]) => {
            require_uuid("issue UUID", issue_id)?;
            require_uuid("relation UUID", relation_id)?;
            Ok(CliCommand::RemoveRelation {
                issue_id: (*issue_id).to_string(),
                relation_id: (*relation_id).to_string(),
            })
        }
        ("list" | "add" | "remove", _) => Err(CliParseError::InvalidCombination(
            "issue relation expects `list <issue-id>`, `add <issue-id> <relation> <target-ref>`, or `remove <issue-id> <relation-id>`",
        )),
        (other, _) => Err(CliParseError::Unknown {
            token: other.to_string(),
        }),
    }
}

fn parse_import(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let mut source = None;
    let mut job_id = None;
    let mut input = None;
    let mut mode = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--from" => {
                if source.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--from" });
                }
                let value = flag_value(args, &mut index, "--from")?;
                source =
                    Some(
                        SourceSystem::parse(value).ok_or_else(|| CliParseError::BadValue {
                            field: "import source (expected jira|linear|github|csv)",
                            value: value.to_string(),
                        })?,
                    );
            }
            "--job" => {
                if job_id.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--job" });
                }
                let value = flag_value(args, &mut index, "--job")?.to_string();
                require_uuid("import job UUID", &value)?;
                job_id = Some(value);
            }
            "--input" => {
                if input.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--input" });
                }
                let value = flag_value(args, &mut index, "--input")?;
                if value.is_empty() || value.len() > 4_096 {
                    return Err(CliParseError::BadValue {
                        field: "import input path (expected 1..=4096 bytes)",
                        value: value.to_string(),
                    });
                }
                input = Some(value.to_string());
            }
            "--dry-run" => {
                if mode.replace(ImportMode::DryRun).is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--dry-run" });
                }
            }
            "--run" => {
                if mode.replace(ImportMode::Run).is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--run" });
                }
            }
            other => {
                return Err(CliParseError::Unknown {
                    token: other.to_string(),
                });
            }
        }
        index += 1;
    }

    let source = source.ok_or(CliParseError::MissingValue { flag: "--from" })?;
    let job_id = required(job_id, "--job")?;
    let input = required(input, "--input")?;
    let mode = mode.ok_or(CliParseError::InvalidCombination(
        "issue import requires exactly one of --dry-run or --run",
    ))?;
    Ok(CliCommand::Import {
        source,
        job_id,
        input,
        mode,
    })
}

fn parse_list(args: &[&str]) -> Result<CliCommand, CliParseError> {
    let mut state = None;
    let mut key = None;
    let mut limit = None;
    let mut cursor = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--state" => {
                if state.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--state" });
                }
                let value = flag_value(args, &mut index, "--state")?;
                state =
                    Some(
                        IssueListState::parse(value).ok_or_else(|| CliParseError::BadValue {
                            field: "state (expected open|closed|all)",
                            value: value.to_string(),
                        })?,
                    );
            }
            "--key" => {
                if key.is_some() {
                    return Err(CliParseError::DuplicateFlag { flag: "--key" });
                }
                let value = flag_value(args, &mut index, "--key")?;
                key = Some(normalize_issue_key_prefix(value).ok_or_else(|| {
                    CliParseError::BadValue {
                        field: "key prefix (expected 1..=32 ASCII letters/digits/hyphen)",
                        value: value.to_string(),
                    }
                })?);
            }
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
    let state = state.unwrap_or(IssueListState::Open);
    if let Some(value) = cursor.as_deref() {
        decode_issue_page_cursor(value, state, key.as_deref()).map_err(|_| {
            CliParseError::BadValue {
                field: "opaque cursor",
                value: value.to_string(),
            }
        })?;
    }
    Ok(CliCommand::List {
        state,
        key,
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
            token if !token.starts_with('-') && title.is_none() => {
                title = Some(token.to_string());
                index += 1;
                continue;
            }
            token if !token.starts_with('-') => {
                return Err(CliParseError::InvalidCombination(
                    "issue create accepts exactly one title",
                ));
            }
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
    let title = required(title, "issue title")?;
    require_uuid("project UUID", &project_id)?;
    if let Some(type_id) = type_id.as_deref() {
        require_uuid("type UUID", type_id)?;
    }
    if let Some(prefix) = prefix.as_deref() {
        if prefix.len() < 2
            || prefix.len() > 10
            || !prefix
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(CliParseError::BadValue {
                field: "prefix (expected 2..=10 uppercase ASCII letters/digits)",
                value: prefix.to_string(),
            });
        }
    }
    if !is_valid_issue_title(&title) {
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
    const RELATION: &str = "33333333-3333-3333-3333-333333333333";
    const TARGET_REF: &str = "myelin://acme/issue/issue/ENG-2";

    #[test]
    fn parses_every_founder_command_without_scope_selectors() {
        assert_eq!(
            parse_cli(&["list"]).unwrap(),
            CliCommand::List {
                state: IssueListState::Open,
                key: None,
                limit: 50,
                cursor: None
            }
        );
        let cursor = encode_issue_page_cursor(
            IssueListState::Closed,
            Some("eng-"),
            1_700_000_000_123_456,
            PROJECT,
        )
        .unwrap();
        assert_eq!(
            parse_cli(&[
                "list", "--state", "closed", "--key", "eng-", "--limit", "1", "--cursor", &cursor,
            ])
            .unwrap(),
            CliCommand::List {
                state: IssueListState::Closed,
                key: Some("ENG-".into()),
                limit: 1,
                cursor: Some(cursor)
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
                type_id: Some(TYPE.into()),
                prefix: Some("ENG2".into()),
                title: "Founder issue".into()
            }
        );
        assert_eq!(
            parse_cli(&["create", "A concise issue", "--project", PROJECT]).unwrap(),
            CliCommand::Create {
                project_id: PROJECT.into(),
                type_id: None,
                prefix: None,
                title: "A concise issue".into(),
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
        assert_eq!(
            parse_cli(&["relation", "list", PROJECT]).unwrap(),
            CliCommand::ListRelations {
                issue_id: PROJECT.into()
            }
        );
        assert_eq!(
            parse_cli(&["relation", "add", PROJECT, "blocks", TARGET_REF]).unwrap(),
            CliCommand::CreateRelation {
                issue_id: PROJECT.into(),
                relation: IssueLifecycleRel::Blocks,
                target_ref: TARGET_REF.into(),
            }
        );
        assert_eq!(
            parse_cli(&["relation", "remove", PROJECT, RELATION]).unwrap(),
            CliCommand::RemoveRelation {
                issue_id: PROJECT.into(),
                relation_id: RELATION.into(),
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
        for invalid in [
            vec!["relation"],
            vec!["relation", "list"],
            vec!["relation", "add", PROJECT, "blocks"],
            vec!["relation", "remove", PROJECT],
            vec!["relation", "archive", PROJECT],
        ] {
            assert!(parse_cli(&invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn import_grammar_makes_preview_and_resumable_run_explicit() {
        assert_eq!(
            parse_cli(&[
                "import",
                "--from",
                "jira",
                "--job",
                PROJECT,
                "--input",
                "jira-export.json",
                "--dry-run",
            ])
            .unwrap(),
            CliCommand::Import {
                source: SourceSystem::Jira,
                job_id: PROJECT.into(),
                input: "jira-export.json".into(),
                mode: ImportMode::DryRun,
            }
        );
        assert_eq!(
            parse_cli(&["import", "--from", "github", "--job", PROJECT, "--input", "-", "--run",])
                .unwrap(),
            CliCommand::Import {
                source: SourceSystem::GitHub,
                job_id: PROJECT.into(),
                input: "-".into(),
                mode: ImportMode::Run,
            }
        );

        for invalid in [
            vec!["import", "--from", "jira", "--job", PROJECT, "--input", "x"],
            vec![
                "import", "--from", "jira", "--job", PROJECT, "--input", "x", "--run", "--resume",
            ],
            vec![
                "import", "--from", "Jira", "--job", PROJECT, "--input", "x", "--run",
            ],
        ] {
            assert!(parse_cli(&invalid).is_err(), "accepted {invalid:?}");
        }
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
        assert!(parse_cli(&["list", "--state", "OPEN"]).is_err());
        assert!(parse_cli(&["list", "--key", "title search"]).is_err());
        assert!(parse_cli(&["view", "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"]).is_err());
        assert!(parse_cli(&["relation", "add", PROJECT, "follows", TARGET_REF]).is_err());
        assert!(parse_cli(&[
            "relation",
            "add",
            PROJECT,
            "blocks",
            "myelin://acme/knowledge/page/01J0PAGE"
        ])
        .is_err());
        assert!(parse_cli(&["relation", "remove", PROJECT, "not-a-uuid"]).is_err());
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
        for title in [" padded ", "line\nbreak", "hidden\u{85}control"] {
            assert_eq!(
                parse_cli(&[
                    "create",
                    "--project",
                    PROJECT,
                    "--type",
                    TYPE,
                    "--prefix",
                    "ENG",
                    "--title",
                    title,
                ]),
                Err(CliParseError::BadTitle),
            );
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

    #[test]
    fn cursor_roundtrips_compound_position_and_is_filter_bound() {
        let cursor = encode_issue_page_cursor(
            IssueListState::Open,
            Some("eng-1"),
            1_700_000_000_123_456,
            PROJECT,
        )
        .unwrap();
        assert!(cursor.len() <= MAX_ISSUE_CURSOR_BYTES);
        assert!(cursor
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'));
        assert!(!cursor.contains("acme"));
        assert!(!cursor.contains("founder"));
        assert!(!cursor.contains("secret title"));
        assert_eq!(
            decode_issue_page_cursor(&cursor, IssueListState::Open, Some("ENG-1")).unwrap(),
            IssuePageCursor {
                updated_at_micros: 1_700_000_000_123_456,
                issue_id: PROJECT.into(),
            }
        );
        assert!(decode_issue_page_cursor(&cursor, IssueListState::Closed, Some("ENG-1")).is_err());
        assert!(decode_issue_page_cursor(&cursor, IssueListState::Open, Some("OPS")).is_err());
    }

    #[test]
    fn cursor_rejects_malformed_foreign_version_and_oversize_inputs() {
        let cursor =
            encode_issue_page_cursor(IssueListState::All, None, 1_700_000_000_123_456, PROJECT)
                .unwrap();
        for malformed in ["", "ic_0", "ic_zz", "IC_0100"] {
            assert!(decode_issue_page_cursor(malformed, IssueListState::All, None).is_err());
        }
        let mut foreign = cursor.clone();
        foreign.replace_range(3..5, "02");
        assert!(decode_issue_page_cursor(&foreign, IssueListState::All, None).is_err());
        assert!(decode_issue_page_cursor(
            &format!("ic_{}", "00".repeat(MAX_ISSUE_CURSOR_BYTES)),
            IssueListState::All,
            None,
        )
        .is_err());
    }
}
