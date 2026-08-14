use crate::client::EdgeHttpClient;
use crate::config::{load_profile_credential, selected_saved_profile, EdgeConfig, SESSION_SCHEME};
use crate::context::{decode_identity, identity_call};
use crate::dispatch::{ci_dispatch, project_dispatch, repo_dispatch, tool_dispatch};
use crate::error::CliError;
use crate::git_credential::{self, CredentialScope, GitConfiguration, GitConfigurationStatus};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

const MAX_DIAGNOSTIC_CHARS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ready,
    Attention,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_command: Option<String>,
}

impl DoctorCheck {
    fn ready(name: &str, summary: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Ready,
            summary: summary.into(),
            next_command: None,
        }
    }

    fn attention(name: &str, summary: impl Into<String>, next_command: &str) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Attention,
            summary: summary.into(),
            next_command: Some(next_command.into()),
        }
    }

    fn failed(name: &str, error: &CliError) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Failed,
            summary: bounded_diagnostic(&error.to_string()),
            next_command: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    pub ready: bool,
    pub profile: String,
    pub edge_url: String,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn new(profile: String, edge_url: String, checks: Vec<DoctorCheck>) -> Self {
        let ready = checks
            .iter()
            .all(|check| check.status == CheckStatus::Ready);
        Self {
            ready,
            profile,
            edge_url,
            checks,
        }
    }

    pub fn render(&self, json_mode: bool) -> String {
        if json_mode {
            return format!(
                "{}\n",
                serde_json::to_string_pretty(self).expect("doctor report is serializable")
            );
        }

        let mut rendered = format!(
            "Myelin doctor: {}\nProfile: {}\nEdge: {}\n",
            if self.ready {
                "ready"
            } else {
                "needs attention"
            },
            self.profile,
            self.edge_url
        );
        for check in &self.checks {
            let status = match check.status {
                CheckStatus::Ready => "ok",
                CheckStatus::Attention => "attention",
                CheckStatus::Failed => "failed",
            };
            rendered.push_str(&format!("[{status}] {} — {}\n", check.name, check.summary));
            if let Some(command) = &check.next_command {
                rendered.push_str(&format!("  next: {command}\n"));
            }
        }
        rendered
    }
}

pub async fn diagnose(
    profile_name: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&Path) -> Option<String>,
) -> Result<DoctorReport, CliError> {
    let selected = load_profile_credential(profile_name, getenv, read_file)?.ok_or_else(|| {
        CliError::NotAuthenticated(
            "doctor needs a saved browser session; run `myelin auth login` first".into(),
        )
    })?;
    selected.credential.ensure_not_expired()?;
    if selected.credential.scheme != SESSION_SCHEME {
        return Err(CliError::NotAuthenticated(
            "doctor checks the browser-approved development context; run `myelin auth login` first"
                .into(),
        ));
    }
    let edge_url = selected.credential.edge_url.clone().ok_or_else(|| {
        CliError::Config(
            "the selected profile predates Edge-aware login; run `myelin auth login` again".into(),
        )
    })?;
    let saved_profile =
        selected_saved_profile(profile_name, getenv, read_file)?.ok_or_else(|| {
            CliError::Config("the selected credential has no saved profile metadata".into())
        })?;
    if saved_profile.name != selected.profile_name || saved_profile.edge_url != edge_url {
        return Err(CliError::Config(
            "the selected credential and profile metadata disagree; sign in to this profile again"
                .into(),
        ));
    }

    let edge = EdgeConfig {
        url: edge_url.clone(),
        scheme: selected.credential.scheme.clone(),
    };
    let project_call = match saved_profile.project.as_deref() {
        Some(project) => project_dispatch(&["show", project], Some(project))?,
        None => project_dispatch(&["list", "--limit", "1"], None)?,
    };
    let repository_call = repo_dispatch(&["list", "--limit", "1"])?;
    let ci_call = ci_dispatch(&["list", "--limit", "1"])?;
    let tool_call = tool_dispatch(&["list", "--limit", "1"])?;
    let identity_call = identity_call();

    let client = EdgeHttpClient::new()?;
    let token = &selected.credential.token;
    let (identity, project, repositories, ci_runs, tools) = tokio::join!(
        client.execute(&edge, token, &identity_call),
        client.execute(&edge, token, &project_call),
        client.execute(&edge, token, &repository_call),
        client.execute(&edge, token, &ci_call),
        client.execute(&edge, token, &tool_call),
    );

    let executable = std::env::current_exe().map_err(|error| {
        CliError::Config(format!("cannot locate the Myelin executable: {error}"))
    })?;
    let git_scope = CredentialScope::from_edge_url(&edge_url)?;
    let git_configuration = GitConfiguration::new(
        &git_scope,
        &executable,
        &selected.credential.scheme,
        &selected.profile_name,
    )?;

    let checks = vec![
        identity_check(identity),
        project_check(project, saved_profile.project.as_deref()),
        catalogue_check(
            "repositories",
            repositories,
            "No visible repositories yet.",
            "myelin repo create <name> --idempotency-key <key>",
        ),
        reachable_catalogue_check("ci", ci_runs, "CI run catalogue is reachable."),
        nonempty_catalogue_check(
            "agent tools",
            tools,
            "Governed tool catalogue is available.",
        ),
        git_check(git_credential::inspect(&git_configuration)),
    ];

    Ok(DoctorReport::new(selected.profile_name, edge_url, checks))
}

fn identity_check(result: Result<Value, CliError>) -> DoctorCheck {
    match result.and_then(|value| decode_identity(&value)) {
        Ok(identity) => DoctorCheck::ready(
            "identity",
            format!(
                "Authenticated as {} in {}/{}.",
                identity.principal_id, identity.tenant, identity.region
            ),
        ),
        Err(error) => DoctorCheck::failed("identity", &error),
    }
}

fn project_check(result: Result<Value, CliError>, active_project: Option<&str>) -> DoctorCheck {
    match (result, active_project) {
        (Ok(value), Some(expected)) => {
            let actual = value
                .get("project")
                .and_then(|project| project.get("id"))
                .and_then(Value::as_str);
            if actual == Some(expected) {
                DoctorCheck::ready("project", format!("Active project {expected} is visible."))
            } else {
                DoctorCheck::failed(
                    "project",
                    &malformed_response("project lookup did not return the selected project"),
                )
            }
        }
        (Ok(value), None) => match catalogue_len(&value) {
            Ok(0) => DoctorCheck::attention(
                "project",
                "No visible projects yet.",
                "myelin project create <name> --prefix <key> --idempotency-key <key>",
            ),
            Ok(_) => DoctorCheck::attention(
                "project",
                "Projects are available, but this profile has no active project.",
                "myelin context use --project <project-id>",
            ),
            Err(error) => DoctorCheck::failed("project", &error),
        },
        (Err(error), _) => DoctorCheck::failed("project", &error),
    }
}

fn catalogue_check(
    name: &str,
    result: Result<Value, CliError>,
    empty_summary: &str,
    next_command: &str,
) -> DoctorCheck {
    match result.and_then(|value| catalogue_len(&value)) {
        Ok(0) => DoctorCheck::attention(name, empty_summary, next_command),
        Ok(count) => {
            DoctorCheck::ready(name, format!("Visible catalogue is reachable ({count}+)."))
        }
        Err(error) => DoctorCheck::failed(name, &error),
    }
}

fn reachable_catalogue_check(
    name: &str,
    result: Result<Value, CliError>,
    summary: &str,
) -> DoctorCheck {
    match result.and_then(|value| catalogue_len(&value)) {
        Ok(_) => DoctorCheck::ready(name, summary),
        Err(error) => DoctorCheck::failed(name, &error),
    }
}

fn nonempty_catalogue_check(
    name: &str,
    result: Result<Value, CliError>,
    summary: &str,
) -> DoctorCheck {
    match result.and_then(|value| catalogue_len(&value)) {
        Ok(0) => DoctorCheck::failed(
            name,
            &malformed_response("Edge returned an empty governed tool catalogue"),
        ),
        Ok(_) => DoctorCheck::ready(name, summary),
        Err(error) => DoctorCheck::failed(name, &error),
    }
}

fn git_check(result: Result<GitConfigurationStatus, CliError>) -> DoctorCheck {
    match result {
        Ok(GitConfigurationStatus::Configured) => DoctorCheck::ready(
            "git authentication",
            "Git uses this profile for the selected Edge.",
        ),
        Ok(GitConfigurationStatus::DifferentProfile) => DoctorCheck::attention(
            "git authentication",
            "Git uses another Myelin profile for the selected Edge.",
            "myelin auth configure-git",
        ),
        Ok(GitConfigurationStatus::Missing) => DoctorCheck::attention(
            "git authentication",
            "Git is not configured to use this browser-approved session.",
            "myelin auth configure-git",
        ),
        Err(error) => DoctorCheck::failed("git authentication", &error),
    }
}

fn catalogue_len(value: &Value) -> Result<usize, CliError> {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| malformed_response("catalogue response has no items array"))
}

fn malformed_response(reason: &str) -> CliError {
    CliError::Transport(format!(
        "Edge returned a malformed doctor response: {reason}"
    ))
}

fn bounded_diagnostic(value: &str) -> String {
    let mut bounded: String = value.chars().take(MAX_DIAGNOSTIC_CHARS).collect();
    if value.chars().count() > MAX_DIAGNOSTIC_CHARS {
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{HttpMethod, RetryPolicy};
    use serde_json::json;

    #[test]
    fn report_is_ready_only_when_every_check_is_ready() {
        let ready = DoctorCheck::ready("identity", "ready");
        assert!(DoctorReport::new("work".into(), "https://edge".into(), vec![ready.clone()]).ready);
        assert!(
            !DoctorReport::new(
                "work".into(),
                "https://edge".into(),
                vec![
                    ready,
                    DoctorCheck::attention("project", "choose one", "myelin context use")
                ],
            )
            .ready
        );
    }

    #[test]
    fn catalogue_decoders_refuse_optimistic_or_malformed_results() {
        assert_eq!(catalogue_len(&json!({"items": []})).unwrap(), 0);
        assert!(catalogue_len(&json!({"items": "many"})).is_err());
        assert_eq!(
            nonempty_catalogue_check("tools", Ok(json!({"items": []})), "available",).status,
            CheckStatus::Failed
        );
    }

    #[test]
    fn diagnostics_are_bounded_before_entering_json_or_a_terminal() {
        let error = CliError::Transport("x".repeat(MAX_DIAGNOSTIC_CHARS + 100));
        let check = DoctorCheck::failed("edge", &error);
        assert!(check.summary.chars().count() <= MAX_DIAGNOSTIC_CHARS + 1);
        assert!(check.summary.ends_with('…'));
    }

    #[test]
    fn human_and_json_reports_expose_actions_without_secrets() {
        let report = DoctorReport::new(
            "work".into(),
            "https://edge.example".into(),
            vec![DoctorCheck::attention(
                "project",
                "Choose one.",
                "myelin context use --project <project-id>",
            )],
        );
        let human = report.render(false);
        assert!(human.contains("Myelin doctor: needs attention"));
        assert!(human.contains("next: myelin context use"));
        let json: Value = serde_json::from_str(&report.render(true)).unwrap();
        assert_eq!(json["ready"], false);
        assert_eq!(json["checks"][0]["status"], "attention");
    }

    #[test]
    fn doctor_calls_are_queries_only() {
        for call in [
            identity_call(),
            project_dispatch(&["list", "--limit", "1"], None).unwrap(),
            repo_dispatch(&["list", "--limit", "1"]).unwrap(),
            ci_dispatch(&["list", "--limit", "1"]).unwrap(),
            tool_dispatch(&["list", "--limit", "1"]).unwrap(),
        ] {
            assert_eq!(call.method, HttpMethod::Get);
            assert_eq!(call.retry_policy, RetryPolicy::None);
            assert!(call.idempotency_key.is_none());
            assert!(call.payload.is_none());
        }
    }
}
