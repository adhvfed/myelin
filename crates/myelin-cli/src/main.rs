use clap::{Args, Parser, Subcommand};
use myelin_cli::client::execute;
use myelin_cli::config::{
    self, remove_stored_credentials, resolve_credential, resolve_edge, store_credential,
    SESSION_SCHEME,
};
use myelin_cli::device_auth::{
    begin_authorization, new_authorization_request, wait_for_authorization,
};
use myelin_cli::dispatch::{
    chat_dispatch, ci_dispatch, git_dispatch, issues_dispatch, knowledge_dispatch, notif_dispatch,
    EdgeCall, HttpMethod,
};
use myelin_cli::error::CliError;

#[derive(Parser, Debug)]
#[command(name = "myelin", version, about, long_about = None)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    edge: Option<String>,
    #[arg(long, global = true)]
    token: Option<String>,
    #[arg(long, global = true)]
    scheme: Option<String>,
    #[arg(long, global = true)]
    idempotency_key: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Login(LoginArgs),
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Whoami,
    Git {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Issues {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Ci {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Chat {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(name = "kb", visible_aliases = ["doc", "knowledge"])]
    Knowledge {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(alias = "inbox")]
    Notif {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Args, Debug, Clone, Copy)]
struct LoginArgs {
    /// Explicitly use browser/device authorization (already the default without --token).
    #[arg(long)]
    device: bool,
    /// Print the approval URL without trying to launch a browser.
    #[arg(long)]
    no_browser: bool,
}

#[derive(Subcommand, Debug)]
enum AuthCommand {
    Login(LoginArgs),
    Status,
    Logout,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let exit = match run().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("myelin: {e}");
            e.code()
        }
    };
    std::process::exit(exit);
}

async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let getenv = |k: &str| std::env::var(k).ok();
    let read_file = |p: &std::path::Path| std::fs::read_to_string(p).ok();

    match &cli.command {
        Command::Login(args) => login(&cli, *args, &getenv).await,
        Command::Auth { command } => match command {
            AuthCommand::Login(args) => login(&cli, *args, &getenv).await,
            AuthCommand::Status => run_call(&cli, &getenv, &read_file, whoami_call(), None).await,
            AuthCommand::Logout => logout(&cli, &getenv),
        },
        Command::Whoami => run_call(&cli, &getenv, &read_file, whoami_call(), None).await,
        Command::Git { args } => {
            let (call, command_key) = dispatch_command(args, git_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Issues { args } => {
            let (call, command_key) = dispatch_command(args, issues_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Ci { args } => {
            let (call, command_key) = dispatch_command(args, ci_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Chat { args } => {
            let (call, command_key) = dispatch_command(args, chat_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Knowledge { args } => {
            let (call, command_key) = dispatch_command(args, knowledge_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Notif { args } => {
            let (call, command_key) = dispatch_command(args, notif_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
    }
}

async fn login(
    cli: &Cli,
    args: LoginArgs,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<(), CliError> {
    reject_auth_mutation_flags(cli)?;
    if cli.json {
        return Err(CliError::Usage(
            "interactive login does not support --json; use `myelin auth status --json` after login"
                .into(),
        ));
    }

    let supplied_token = cli
        .token
        .clone()
        .or_else(|| getenv(config::env::TOKEN).filter(|value| !value.is_empty()));
    if let Some(token) = supplied_token {
        if args.device {
            return Err(CliError::Usage(
                "--device cannot be combined with --token or $MYELIN_TOKEN".into(),
            ));
        }
        let scheme = cli
            .scheme
            .clone()
            .or_else(|| getenv(config::env::SCHEME).filter(|value| !value.is_empty()))
            .unwrap_or_else(|| config::DEFAULT_SCHEME.into());
        let path = store_credential(&token, &scheme, getenv)?;
        println!(
            "Stored the {scheme} credential at {}. Run `myelin auth status` to verify it.",
            path.display()
        );
        return Ok(());
    }
    if cli.scheme.is_some() || getenv(config::env::SCHEME).is_some() {
        return Err(CliError::Usage(
            "--scheme and $MYELIN_TOKEN_SCHEME apply only to a supplied token".into(),
        ));
    }

    let edge = resolve_edge(cli.edge.as_deref(), Some(SESSION_SCHEME), getenv);
    let request = new_authorization_request();
    let authorization = begin_authorization(&edge, &request).await?;

    println!(
        "Confirm this code in your browser: {}",
        authorization.user_code
    );
    println!("{}", authorization.verification_uri_complete);
    if !args.no_browser {
        if open_browser(&authorization.verification_uri_complete) {
            println!("Opened the approval page in your browser.");
        } else {
            println!("Your browser could not be opened automatically; use the URL above.");
        }
    }
    println!("Waiting for approval…");
    flush_stdout()?;

    let authorized = wait_for_authorization(&edge, &request, &authorization).await?;
    let path = store_credential(
        &authorized.credential.token,
        &authorized.credential.scheme,
        getenv,
    )?;
    let expires_at = chrono::DateTime::from_timestamp(authorized.expires_at_unix, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| authorized.expires_at_unix.to_string());
    println!("Approved. Your CLI session is ready until {expires_at}.");
    println!("Credentials are stored owner-only at {}.", path.display());
    Ok(())
}

fn logout(cli: &Cli, getenv: &dyn Fn(&str) -> Option<String>) -> Result<(), CliError> {
    reject_auth_mutation_flags(cli)?;
    let removed = remove_stored_credentials(getenv)?;
    let environment_active = cli.token.as_deref().is_some_and(|value| !value.is_empty())
        || getenv(config::env::TOKEN).is_some_and(|value| !value.is_empty());
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "logged_out": true,
                "stored_credential_removed": removed,
                "environment_credential_active": environment_active,
            }))
            .expect("logout JSON is serializable")
        );
    } else if environment_active {
        println!(
            "Removed stored credentials. A --token or $MYELIN_TOKEN credential is still active for this process."
        );
    } else if removed {
        println!("Removed stored credentials from this device.");
    } else {
        println!("No stored credentials were present on this device.");
    }
    Ok(())
}

fn reject_auth_mutation_flags(cli: &Cli) -> Result<(), CliError> {
    if cli.idempotency_key.is_some() {
        return Err(CliError::Usage(
            "--idempotency-key applies only to Edge mutation commands".into(),
        ));
    }
    Ok(())
}

fn whoami_call() -> EdgeCall {
    EdgeCall {
        method: HttpMethod::Get,
        path: "/v1/whoami".into(),
        query: None,
        payload: None,
        idempotency_key: None,
    }
}

fn flush_stdout() -> Result<(), CliError> {
    use std::io::Write as _;
    std::io::stdout().flush().map_err(|error| {
        CliError::Transport(format!("could not write login instructions: {error}"))
    })
}

fn open_browser(url: &str) -> bool {
    use std::process::{Command as ProcessCommand, Stdio};

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = ProcessCommand::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = ProcessCommand::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = ProcessCommand::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(not(any(unix, target_os = "windows")))]
    return false;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn dispatch_command(
    args: &[String],
    dispatch: impl FnOnce(&[&str]) -> Result<EdgeCall, CliError>,
) -> Result<(EdgeCall, Option<&str>), CliError> {
    let mut command_args = Vec::with_capacity(args.len());
    let mut idempotency_key = None;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        if argument == "--idempotency-key" {
            let value = args.get(index + 1).ok_or_else(|| {
                CliError::Usage("--idempotency-key needs a retry-stable value".into())
            })?;
            if idempotency_key.replace(value.as_str()).is_some() {
                return Err(CliError::Usage("duplicate --idempotency-key".into()));
            }
            index += 2;
        } else if let Some(value) = argument.strip_prefix("--idempotency-key=") {
            if idempotency_key.replace(value).is_some() {
                return Err(CliError::Usage("duplicate --idempotency-key".into()));
            }
            index += 1;
        } else {
            command_args.push(argument);
            index += 1;
        }
    }
    Ok((dispatch(&command_args)?, idempotency_key))
}

async fn run_call(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
    call: EdgeCall,
    command_key: Option<&str>,
) -> Result<(), CliError> {
    let idempotency_key = match (cli.idempotency_key.as_deref(), command_key) {
        (Some(_), Some(_)) => return Err(CliError::Usage("duplicate --idempotency-key".into())),
        (top_level, trailing) => top_level.or(trailing),
    };
    let call = match call.method {
        HttpMethod::Post => {
            let key = idempotency_key.ok_or_else(|| {
                CliError::Usage(
                    "mutating commands require --idempotency-key <key>; reuse the same key when \
                     retrying after a lost response"
                        .into(),
                )
            })?;
            call.with_idempotency_key(key)?
        }
        HttpMethod::Get => {
            if idempotency_key.is_some() {
                return Err(CliError::Usage(
                    "--idempotency-key applies only to mutating commands".into(),
                ));
            }
            call
        }
    };
    let credential = resolve_credential(
        cli.token.as_deref(),
        cli.scheme.as_deref(),
        getenv,
        read_file,
    )?;
    let edge = resolve_edge(cli.edge.as_deref(), Some(&credential.scheme), getenv);
    if call.path.starts_with("/v1/ci/runs/") && call.path.ends_with("/log/live") {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        return myelin_cli::ci_watch::execute_ci_watch(
            &edge,
            &credential.token,
            &call,
            cli.json,
            &mut output,
        )
        .await;
    }
    let value = execute(&edge, &credential.token, &call).await?;
    print!(
        "{}",
        myelin_cli::render::render_for_call(&value, cli.json, &call)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_grammars_accept_a_retry_key_in_the_natural_trailing_position() {
        let args = [
            "create".into(),
            "engineering".into(),
            "--topic".into(),
            "Release".into(),
            "--idempotency-key".into(),
            "release-room".into(),
        ];
        let (call, key) = dispatch_command(&args, chat_dispatch).unwrap();
        assert_eq!(call.path, "/v1/chat/conversations");
        assert_eq!(key, Some("release-room"));
    }

    #[test]
    fn delegated_grammars_reject_missing_and_duplicate_retry_keys() {
        for args in [
            vec!["list".into(), "--idempotency-key".into()],
            vec![
                "list".into(),
                "--idempotency-key=one".into(),
                "--idempotency-key".into(),
                "two".into(),
            ],
        ] {
            assert_eq!(
                dispatch_command(&args, chat_dispatch).unwrap_err().code(),
                2
            );
        }
    }
}
