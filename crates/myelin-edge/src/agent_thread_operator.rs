use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_agent_service::workspace::LocalDevelopmentWorkspaceProvisioner;
use myelin_storage::{DurableAgentThreadBacking, SubstrateProvider};

use myelin_edge::AgentThreadReconciler;

struct Command {
    tenant: String,
    observed_at: DateTime<Utc>,
    limit: u32,
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut tenant = None;
        let mut observed_at = None;
        let mut limit = None;
        let mut index = 0;
        while index < args.len() {
            let name = args[index].as_str();
            let Some(value) = args.get(index + 1) else {
                return Err(format!("{name} requires a value"));
            };
            if value.starts_with("--") {
                return Err(format!("{name} requires a value"));
            }
            let slot = match name {
                "--tenant" => &mut tenant,
                "--now" => &mut observed_at,
                "--limit" => &mut limit,
                other => return Err(format!("unknown flag `{other}`")),
            };
            if slot.replace(value.clone()).is_some() {
                return Err(format!("duplicate flag `{name}`"));
            }
            index += 2;
        }
        let tenant = tenant
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "--tenant is required".to_string())?;
        let observed_at = observed_at
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|_| "--now must be an RFC 3339 timestamp".to_string())
            })
            .transpose()?
            .unwrap_or_else(Utc::now);
        let limit = limit
            .map(|value| {
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|value| (1..=100).contains(value))
                    .ok_or_else(|| "--limit must be an integer between 1 and 100".to_string())
            })
            .transpose()?
            .unwrap_or(100);
        Ok(Self {
            tenant,
            observed_at,
            limit,
        })
    }
}

pub async fn run(provider: SubstrateProvider, args: &[String]) {
    let command = Command::parse(args).unwrap_or_else(|error| {
        eprintln!("edge agent-thread-reconcile: {error}");
        std::process::exit(2);
    });
    let workspace_root = super::validated_persistent_directory(
        "MYELIN_AGENT_WORKSPACE_ROOT",
        std::env::var("MYELIN_AGENT_WORKSPACE_ROOT"),
    )
    .unwrap_or_else(|error| {
        eprintln!("edge agent-thread-reconcile: {error}");
        std::process::exit(1);
    });
    let workspaces =
        LocalDevelopmentWorkspaceProvisioner::open(workspace_root).unwrap_or_else(|error| {
            eprintln!("edge agent-thread-reconcile: workspace storage unavailable: {error}");
            std::process::exit(1);
        });
    let reconciler = AgentThreadReconciler::new(
        DurableAgentThreadBacking::new(provider),
        Arc::new(workspaces),
    );
    let report = reconciler
        .reconcile_tenant(&command.tenant, command.observed_at, command.limit)
        .await
        .unwrap_or_else(|error| {
            eprintln!("edge agent-thread-reconcile: {error}");
            std::process::exit(1);
        });
    eprintln!(
        "edge agent-thread-reconcile: tenant={} made_inaccessible={} cleanup_candidates={} deleted={} cleanup_failures={} changed_before_completion={}",
        command.tenant,
        report.made_inaccessible,
        report.cleanup_candidates,
        report.deleted,
        report.cleanup_failures,
        report.changed_before_completion,
    );
    if report.cleanup_failures != 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_explicit_clock_is_accepted_and_ambiguous_flags_are_refused() {
        let command = Command::parse(&[
            "--tenant".into(),
            "acme".into(),
            "--now".into(),
            "2026-08-25T00:00:00Z".into(),
            "--limit".into(),
            "25".into(),
        ])
        .unwrap();
        assert_eq!(command.tenant, "acme");
        assert_eq!(
            command.observed_at.to_rfc3339(),
            "2026-08-25T00:00:00+00:00"
        );
        assert_eq!(command.limit, 25);

        for args in [
            vec![
                "--tenant".into(),
                "acme".into(),
                "--tenant".into(),
                "other".into(),
            ],
            vec![
                "--tenant".into(),
                "acme".into(),
                "--limit".into(),
                "0".into(),
            ],
            vec![
                "--tenant".into(),
                "acme".into(),
                "--surprise".into(),
                "x".into(),
            ],
        ] {
            assert!(Command::parse(&args).is_err());
        }
    }
}
