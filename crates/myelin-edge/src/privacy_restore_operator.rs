use std::{str::FromStr, sync::Arc};

use myelin_chat::store::pg::PgMessageStore;
use myelin_chat::store::pg_conversation::MESSAGE_TABLE;
use myelin_chat::{PostRestoreChatMessageReEraser, PostRestoreChatMessageReport};
use myelin_events::clock::system_clock_reading;
use myelin_events::UlidMinter;
use myelin_issues::{PostRestoreIssueTitleReEraser, PostRestoreIssueTitleReport};
use myelin_storage::{
    DurableAgentTraceStore, DurablePostPitLedger, KmsEngine, PostRestoreAgentDataReEraser,
    PostRestoreAgentDataReport, SubstrateProvider,
};
use serde_json::json;
use sqlx::postgres::PgConnectOptions;

const LIVE_LEDGER_DATABASE_URL_ENV: &str = "MYELIN_POST_RESTORE_LEDGER_DATABASE_URL";

#[derive(Debug, PartialEq, Eq)]
struct Command {
    restored_before_unix: u64,
    confirmed_cell: String,
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut restored_before_unix = None;
        let mut confirmed_cell = None;
        let mut services_stopped = None;
        let mut index = 0;
        while index < args.len() {
            let name = args[index].as_str();
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{name} requires a value"))?;
            match name {
                "--restored-before-unix" => set_once(
                    &mut restored_before_unix,
                    value.clone(),
                    "--restored-before-unix",
                )?,
                "--confirm-cell" => set_once(&mut confirmed_cell, value.clone(), "--confirm-cell")?,
                "--confirm-services-stopped" => set_once(
                    &mut services_stopped,
                    value.clone(),
                    "--confirm-services-stopped",
                )?,
                other => return Err(format!("unknown flag `{other}`")),
            }
            index += 2;
        }

        let restored_before_unix = restored_before_unix
            .ok_or_else(|| "--restored-before-unix is required".to_string())
            .and_then(|value| parse_canonical_positive_u64(&value))?;
        let confirmed_cell = confirmed_cell
            .filter(|value| !value.is_empty() && value.trim() == value)
            .ok_or_else(|| "--confirm-cell is required".to_string())?;
        if services_stopped.as_deref() != Some("yes") {
            return Err(
                "--confirm-services-stopped yes is required before modifying a restored cell"
                    .into(),
            );
        }
        Ok(Self {
            restored_before_unix,
            confirmed_cell,
        })
    }
}

pub async fn run(
    restored_provider: SubstrateProvider,
    restored_kms: Arc<KmsEngine>,
    runtime: tokio::runtime::Handle,
    cell_id: &str,
    args: &[String],
) {
    let command = Command::parse(args).unwrap_or_else(|error| refuse(error, 2));
    if command.confirmed_cell != cell_id {
        refuse(
            "--confirm-cell does not match the restored target cell".into(),
            2,
        );
    }
    let live_ledger_url = std::env::var(LIVE_LEDGER_DATABASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| refuse(format!("{LIVE_LEDGER_DATABASE_URL_ENV} is required"), 1));
    let same_database = same_database(&live_ledger_url, &restored_provider.config().database_url)
        .unwrap_or_else(|_| {
            refuse(
                format!("{LIVE_LEDGER_DATABASE_URL_ENV} is not a valid PostgreSQL connection URL"),
                1,
            )
        });
    if same_database {
        refuse(
            format!(
                "{LIVE_LEDGER_DATABASE_URL_ENV} must name the preserved live ledger, not the restored target database"
            ),
            1,
        );
    }

    let mut live_config = restored_provider.config().clone();
    live_config.database_url = live_ledger_url;
    let live_provider = SubstrateProvider::connect(live_config, 2)
        .await
        .unwrap_or_else(|_| refuse("the preserved live erasure ledger is unavailable".into(), 1));
    let live_ledger = DurablePostPitLedger::new(live_provider);
    let restored_holder = DurableAgentTraceStore::with_runtime(
        restored_provider.clone(),
        runtime,
        restored_kms.clone(),
    );
    let agent_data = PostRestoreAgentDataReEraser::new(live_ledger.clone(), restored_holder)
        .run(command.restored_before_unix)
        .await
        .unwrap_or_else(|error| refuse(error.to_string(), 1));
    let observed = system_clock_reading()
        .unwrap_or_else(|error| refuse(format!("the privacy clock is unavailable: {error}"), 1));
    let chat_messages = PostRestoreChatMessageReEraser::new(
        live_ledger.clone(),
        PgMessageStore::new(
            restored_provider.db_pool().clone(),
            restored_provider.config().region.clone(),
            MESSAGE_TABLE,
        ),
        restored_kms.clone(),
    )
    .run(
        command.restored_before_unix,
        &UlidMinter::new(),
        observed.timestamp(),
    )
    .await
    .unwrap_or_else(|error| refuse(error.to_string(), 1));
    let issue_titles =
        PostRestoreIssueTitleReEraser::new(live_ledger, restored_provider, restored_kms)
            .run(command.restored_before_unix, observed)
            .await
            .unwrap_or_else(|error| refuse(error.to_string(), 1));

    println!(
        "{}",
        report_json(&agent_data, &chat_messages, &issue_titles)
    );
}

fn report_json(
    agent_data: &PostRestoreAgentDataReport,
    chat_messages: &PostRestoreChatMessageReport,
    issue_titles: &PostRestoreIssueTitleReport,
) -> serde_json::Value {
    json!({
        "restore_reerase": {
            "restored_before_unix": agent_data.restored_to_offset,
            "scopes": {
                "agent_data": {
                    "selected_subjects": agent_data.selected_subjects,
                    "newly_re_erased_subjects": agent_data.newly_re_erased_subjects,
                    "already_erased_subjects": agent_data.already_erased_subjects,
                    "records_erased": agent_data.records_erased,
                    "new_processing_blocked": true,
                },
                "chat_messages": {
                    "selected_subjects": chat_messages.selected_subjects,
                    "newly_re_erased_subjects": chat_messages.newly_re_erased_subjects,
                    "already_erased_subjects": chat_messages.already_erased_subjects,
                    "messages_erased": chat_messages.messages_erased,
                    "erasure_events_co_committed": chat_messages.erasure_events_co_committed,
                },
                "issue_titles": {
                    "selected_subjects": issue_titles.selected_subjects,
                    "newly_re_erased_subjects": issue_titles.newly_re_erased_subjects,
                    "already_erased_subjects": issue_titles.already_erased_subjects,
                    "titles_erased": issue_titles.titles_erased,
                    "erasure_events_co_committed": issue_titles.erasure_events_co_committed,
                },
            },
            "complete": true,
        }
    })
}

fn set_once(slot: &mut Option<String>, value: String, name: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate flag `{name}`"));
    }
    Ok(())
}

fn parse_canonical_positive_u64(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0 && parsed.to_string() == value)
        .ok_or_else(|| {
            "--restored-before-unix must be a canonical positive Unix timestamp".to_string()
        })
}

fn same_database(left: &str, right: &str) -> Result<bool, ()> {
    let identity = |value: &str| {
        let options = PgConnectOptions::from_str(value).map_err(|_| ())?;
        Ok::<_, ()>((
            options.get_host().to_string(),
            options.get_port(),
            options
                .get_database()
                .unwrap_or_else(|| options.get_username())
                .to_string(),
        ))
    };
    Ok(identity(left)? == identity(right)?)
}

fn refuse(message: String, exit_code: i32) -> ! {
    eprintln!("edge privacy-reerase: {message}");
    std::process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_args() -> Vec<String> {
        vec![
            "--restored-before-unix".into(),
            "1787687000".into(),
            "--confirm-cell".into(),
            "cell-eu-1".into(),
            "--confirm-services-stopped".into(),
            "yes".into(),
        ]
    }

    #[test]
    fn a_restore_command_names_one_exact_point_and_target_cell() {
        assert_eq!(
            Command::parse(&valid_args()).unwrap(),
            Command {
                restored_before_unix: 1_787_687_000,
                confirmed_cell: "cell-eu-1".into(),
            }
        );
    }

    #[test]
    fn ambiguous_or_unconfirmed_restore_commands_are_refused() {
        let mut leading_zero = valid_args();
        leading_zero[1] = "01787687000".into();
        let mut wrong_confirmation = valid_args();
        wrong_confirmation[5] = "true".into();
        let mut duplicate = valid_args();
        duplicate.extend(["--confirm-cell".into(), "cell-eu-2".into()]);

        for args in [
            Vec::new(),
            leading_zero,
            wrong_confirmation,
            duplicate,
            vec!["--unknown".into(), "value".into()],
        ] {
            assert!(Command::parse(&args).is_err(), "ambiguous args: {args:?}");
        }
    }

    #[test]
    fn changing_credentials_does_not_disguise_the_restored_database_as_a_live_ledger() {
        assert_eq!(
            same_database(
                "postgres://ledger_reader:one@db.internal:5432/myelin",
                "postgres://restored_writer:two@db.internal/myelin",
            ),
            Ok(true)
        );
        assert_eq!(
            same_database(
                "postgres://ledger_reader:one@db.internal/myelin_live",
                "postgres://restored_writer:two@db.internal/myelin_restored",
            ),
            Ok(false)
        );
        assert!(same_database("not a database", "postgres://db/myelin").is_err());
    }

    #[test]
    fn completion_reports_every_scope_the_operator_reapplies() {
        let body = report_json(
            &PostRestoreAgentDataReport {
                restored_to_offset: 42,
                selected_subjects: 2,
                newly_re_erased_subjects: 1,
                already_erased_subjects: 1,
                records_erased: 7,
            },
            &PostRestoreChatMessageReport {
                restored_to_offset: 42,
                selected_subjects: 1,
                newly_re_erased_subjects: 1,
                already_erased_subjects: 0,
                messages_erased: 3,
                erasure_events_co_committed: 3,
            },
            &PostRestoreIssueTitleReport {
                restored_to_offset: 42,
                selected_subjects: 2,
                newly_re_erased_subjects: 1,
                already_erased_subjects: 1,
                titles_erased: 4,
                erasure_events_co_committed: 4,
            },
        );
        assert_eq!(body["restore_reerase"]["restored_before_unix"], 42);
        assert_eq!(
            body["restore_reerase"]["scopes"]["agent_data"]["records_erased"],
            7,
        );
        assert_eq!(
            body["restore_reerase"]["scopes"]["chat_messages"]["messages_erased"],
            3,
        );
        assert_eq!(
            body["restore_reerase"]["scopes"]["issue_titles"]["titles_erased"],
            4,
        );
        assert_eq!(body["restore_reerase"]["complete"], true);
    }
}
