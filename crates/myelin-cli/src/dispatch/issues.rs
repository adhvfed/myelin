use super::{CliError, EdgeCall, HttpMethod, RetryPolicy};
use myelin_issues::api::{
    parse_cli, CliCommand, ImportMode, MAX_ISSUE_IMPORT_JSON_BYTES, MAX_ISSUE_IMPORT_RECORDS,
};
use myelin_issues::{CreateIssue, ImportIssue};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportDocument {
    records: Vec<ImportRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRecord {
    source_id: String,
    type_id: String,
    prefix: String,
    title: String,
}

pub fn issues_dispatch(args: &[&str]) -> Result<EdgeCall, CliError> {
    issues_dispatch_with_context(args, None, &|_| {
        Err(CliError::Usage(
            "issue import input is available through the myelin executable".into(),
        ))
    })
}

pub fn issues_dispatch_with_project(
    args: &[&str],
    default_project: Option<&str>,
) -> Result<EdgeCall, CliError> {
    issues_dispatch_with_context(args, default_project, &|_| {
        Err(CliError::Usage(
            "issue import input is available through the myelin executable".into(),
        ))
    })
}

pub fn issues_dispatch_with_context(
    args: &[&str],
    default_project: Option<&str>,
    read_import: &dyn Fn(&str) -> Result<String, CliError>,
) -> Result<EdgeCall, CliError> {
    let uses_default = args.first().copied() == Some("create") && !args.contains(&"--project");
    let contextual_args;
    let args = if uses_default {
        let project = require_project(default_project, "create")?;
        contextual_args = {
            let mut contextual = Vec::with_capacity(args.len() + 2);
            contextual.extend_from_slice(args);
            contextual.extend(["--project", project]);
            contextual
        };
        contextual_args.as_slice()
    } else {
        args
    };

    let command = parse_cli(args).map_err(|error| CliError::Usage(error.to_string()))?;
    command_to_call(command, default_project, read_import)
}

fn require_project<'a>(project: Option<&'a str>, operation: &str) -> Result<&'a str, CliError> {
    project.ok_or_else(|| {
        CliError::Usage(format!(
            "issue {operation} needs a project; pass --project or run `myelin context use --project <project>`"
        ))
    })
}

fn command_to_call(
    command: CliCommand,
    default_project: Option<&str>,
    read_import: &dyn Fn(&str) -> Result<String, CliError>,
) -> Result<EdgeCall, CliError> {
    Ok(match command {
        CliCommand::List {
            state,
            key,
            limit,
            cursor,
        } => {
            let mut query = format!("state={}&limit={limit}", state.as_str());
            if let Some(key) = key {
                query.push_str("&key=");
                query.push_str(&key);
            }
            if let Some(cursor) = cursor {
                query.push_str("&cursor=");
                query.push_str(&cursor);
            }
            EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/issues".into(),
                query: Some(query),
                payload: None,
                idempotency_key: None,
                retry_policy: RetryPolicy::None,
            }
        }
        CliCommand::Create {
            project_id,
            type_id,
            prefix,
            title,
        } => {
            let mut payload = json!({
                "project_id": project_id,
                "title": title,
            });
            if let Some(type_id) = type_id {
                payload["type_id"] = json!(type_id);
            }
            if let Some(prefix) = prefix {
                payload["prefix"] = json!(prefix);
            }
            EdgeCall::post_json("/v1/issues", payload)
        }
        CliCommand::Import {
            source,
            job_id,
            input,
            mode,
        } => {
            let project = require_project(default_project, "import")?;
            let document = read_document(&input, read_import)?;
            let mut source_ids = BTreeSet::new();
            let mut records = Vec::with_capacity(document.records.len());
            for record in document.records {
                if !source_ids.insert(record.source_id.clone()) {
                    return Err(CliError::Usage(
                        "issue import input contains a duplicate source_id".into(),
                    ));
                }
                let import = ImportIssue {
                    import_job_id: job_id.clone(),
                    source,
                    source_id: record.source_id,
                    issue: CreateIssue {
                        project_id: project.into(),
                        type_id: record.type_id,
                        prefix: record.prefix,
                        title: record.title,
                    },
                };
                import
                    .validate()
                    .map_err(|error| CliError::Usage(error.to_string()))?;
                records.push(json!({
                    "source_id": import.source_id,
                    "project_id": import.issue.project_id,
                    "type_id": import.issue.type_id,
                    "prefix": import.issue.prefix,
                    "title": import.issue.title,
                }));
            }
            let payload = json!({ "source": source.token(), "records": records });
            if payload.to_string().len() > MAX_ISSUE_IMPORT_JSON_BYTES {
                return Err(CliError::Usage(format!(
                    "normalized issue import exceeds the {MAX_ISSUE_IMPORT_JSON_BYTES}-byte request limit"
                )));
            }
            let path = match mode {
                ImportMode::DryRun => format!("/v1/issues/imports/{job_id}/dry-run"),
                ImportMode::Run { .. } => format!("/v1/issues/imports/{job_id}/run"),
            };
            match mode {
                ImportMode::DryRun => EdgeCall::post_read_json(path, payload),
                ImportMode::Run { .. } => EdgeCall::post_json(path, payload),
            }
        }
        CliCommand::View { issue_id } => EdgeCall::get(format!("/v1/issues/{issue_id}")),
        CliCommand::Close { issue_id } => {
            EdgeCall::post_json(format!("/v1/issues/{issue_id}/close"), json!({}))
        }
    })
}

fn read_document(
    input: &str,
    read_import: &dyn Fn(&str) -> Result<String, CliError>,
) -> Result<ImportDocument, CliError> {
    let contents = read_import(input)?;
    if contents.len() > MAX_ISSUE_IMPORT_JSON_BYTES {
        return Err(CliError::Usage(format!(
            "issue import input exceeds the {MAX_ISSUE_IMPORT_JSON_BYTES}-byte request limit"
        )));
    }
    let document: ImportDocument = serde_json::from_str(&contents)
        .map_err(|error| CliError::Usage(format!("invalid issue import JSON: {error}")))?;
    if document.records.is_empty() || document.records.len() > MAX_ISSUE_IMPORT_RECORDS {
        return Err(CliError::Usage(format!(
            "issue import input must contain 1..={MAX_ISSUE_IMPORT_RECORDS} records"
        )));
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "11111111-1111-1111-1111-111111111111";
    const TYPE: &str = "22222222-2222-2222-2222-222222222222";
    const JOB: &str = "33333333-3333-3333-3333-333333333333";

    fn document(_: &str) -> Result<String, CliError> {
        Ok(json!({
            "records": [{
                "source_id": "JIRA-41",
                "type_id": TYPE,
                "prefix": "ENG",
                "title": "Imported from a file",
            }]
        })
        .to_string())
    }

    #[test]
    fn dry_run_is_a_keyless_read_and_run_is_a_retry_keyed_mutation() {
        let dry_run = issues_dispatch_with_context(
            &[
                "import",
                "--from",
                "jira",
                "--job",
                JOB,
                "--input",
                "jira.json",
                "--dry-run",
            ],
            Some(PROJECT),
            &document,
        )
        .unwrap();
        assert_eq!(dry_run.method, HttpMethod::Post);
        assert_eq!(dry_run.path, format!("/v1/issues/imports/{JOB}/dry-run"));
        assert_eq!(dry_run.retry_policy, RetryPolicy::None);

        let run = issues_dispatch_with_context(
            &[
                "import",
                "--from",
                "jira",
                "--job",
                JOB,
                "--input",
                "jira.json",
                "--run",
                "--resume",
            ],
            Some(PROJECT),
            &document,
        )
        .unwrap();
        assert_eq!(run.path, format!("/v1/issues/imports/{JOB}/run"));
        assert_eq!(run.retry_policy, RetryPolicy::CallerKeyRequired);
        let body: serde_json::Value =
            serde_json::from_slice(run.payload.as_ref().unwrap()).unwrap();
        assert_eq!(body["source"], "jira");
        assert_eq!(body["records"][0]["project_id"], PROJECT);
        assert_eq!(body["records"][0]["source_id"], "JIRA-41");
    }

    #[test]
    fn import_files_fail_locally_before_any_edge_call() {
        let args = [
            "import",
            "--from",
            "jira",
            "--job",
            JOB,
            "--input",
            "jira.json",
            "--run",
        ];
        assert!(issues_dispatch_with_context(&args, None, &document)
            .unwrap_err()
            .to_string()
            .contains("context use --project"));
        assert!(issues_dispatch_with_context(&args, Some(PROJECT), &|_| Ok("{}".into())).is_err());
        assert!(issues_dispatch_with_context(&args, Some(PROJECT), &|_| {
            Ok(json!({
                "records": [
                    {"source_id":"same", "type_id":TYPE, "prefix":"ENG", "title":"one"},
                    {"source_id":"same", "type_id":TYPE, "prefix":"ENG", "title":"two"},
                ]
            })
            .to_string())
        })
        .is_err());
    }
}
