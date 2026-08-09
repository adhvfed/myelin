use crate::client::execute;
use crate::config::{
    load_profile_credential, saved_profiles, use_profile_context, EdgeConfig, ProfileContext,
};
use crate::dispatch::{EdgeCall, HttpMethod, RetryPolicy};
use crate::error::CliError;
use serde_json::{json, Value};
use std::path::Path;

pub async fn inspect(edge: &EdgeConfig, token: &str) -> Result<ProfileContext, CliError> {
    let identity = execute(edge, token, &identity_call()).await?;
    Ok(ProfileContext {
        tenant: identity_field(&identity, "tenant")?.into(),
        region: identity_field(&identity, "region")?.into(),
        project: None,
    })
}

pub fn list(
    json_mode: bool,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&Path) -> Option<String>,
) -> Result<(), CliError> {
    let profiles = saved_profiles(getenv, read_file)?;
    if json_mode {
        let items: Vec<_> = profiles
            .iter()
            .map(|profile| {
                json!({
                    "name": profile.name,
                    "active": profile.active,
                    "edge_url": profile.edge_url,
                    "scheme": profile.scheme,
                    "expires_at_unix": profile.expires_at_unix,
                    "tenant": profile.tenant,
                    "region": profile.region,
                    "project": profile.project,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "profiles": items }))
                .expect("profile JSON is serializable")
        );
        return Ok(());
    }
    if profiles.is_empty() {
        println!("No saved CLI contexts. Run `myelin auth login` to create one.");
        return Ok(());
    }
    println!("Saved CLI contexts:");
    for profile in profiles {
        let marker = if profile.active { '*' } else { ' ' };
        let tenant = profile.tenant.as_deref().unwrap_or("?");
        let region = profile.region.as_deref().unwrap_or("?");
        let project = profile
            .project
            .as_deref()
            .map(|project| format!("  project={project}"))
            .unwrap_or_default();
        println!(
            "{marker} {}  tenant={tenant}  region={region}{project}  edge={}",
            profile.name, profile.edge_url
        );
    }
    Ok(())
}

pub async fn current(
    json_mode: bool,
    profile_name: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&Path) -> Option<String>,
) -> Result<(), CliError> {
    let selected = load_profile_credential(profile_name, getenv, read_file)?.ok_or_else(|| {
        CliError::NotAuthenticated("no saved CLI context; run `myelin auth login` first".into())
    })?;
    selected.credential.ensure_not_expired()?;
    let edge = EdgeConfig {
        url: selected.credential.edge_url.clone().ok_or_else(|| {
            CliError::Config("the selected profile does not record its issuing Edge".into())
        })?,
        scheme: selected.credential.scheme.clone(),
    };
    let identity = execute(&edge, &selected.credential.token, &identity_call()).await?;
    let project = saved_profiles(getenv, read_file)?
        .into_iter()
        .find(|profile| profile.name == selected.profile_name)
        .and_then(|profile| profile.project);
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "profile": selected.profile_name,
                "edge_url": edge.url,
                "project": project,
                "identity": identity,
            }))
            .expect("current context JSON is serializable")
        );
    } else {
        println!("Profile: {}", selected.profile_name);
        println!("Edge: {}", edge.url);
        if let Some(project) = project {
            println!("Project: {project}");
        }
        print!("{}", crate::render::render(&identity, false));
    }
    Ok(())
}

pub fn select(
    json_mode: bool,
    profile_name: Option<&str>,
    project: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<(), CliError> {
    let selected = use_profile_context(profile_name, project, getenv)?;
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "active_profile": selected.name,
                "project": selected.project,
            }))
            .expect("context selection JSON is serializable")
        );
    } else {
        println!("Using CLI context `{}`.", selected.name);
        if let Some(project) = selected.project {
            println!("Default project: {project}");
        }
    }
    Ok(())
}

fn identity_call() -> EdgeCall {
    EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/whoami".into(),
        query: None,
        payload: None,
        idempotency_key: None,
        retry_policy: RetryPolicy::None,
    }
}

fn identity_field<'a>(identity: &'a Value, field: &str) -> Result<&'a str, CliError> {
    identity
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value.as_bytes().iter().all(u8::is_ascii_graphic)
        })
        .ok_or_else(|| {
            CliError::Transport(format!(
                "Edge returned a malformed identity response: {field} is missing or invalid"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_identity_fields_are_bounded_and_terminal_safe() {
        let identity = json!({ "tenant": "acme", "region": "eu-north" });
        assert_eq!(identity_field(&identity, "tenant").unwrap(), "acme");
        for identity in [
            json!({ "tenant": "two words" }),
            json!({ "tenant": "line\nbreak" }),
            json!({ "tenant": "x".repeat(257) }),
        ] {
            assert!(identity_field(&identity, "tenant").is_err());
        }
    }
}
