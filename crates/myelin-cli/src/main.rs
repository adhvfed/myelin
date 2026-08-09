use clap::{Parser, Subcommand};
use myelin_cli::client::execute;
use myelin_cli::config::{self, resolve_edge, resolve_token, store_token};
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
    Login,
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
        Command::Login => {
            if cli.idempotency_key.is_some() {
                return Err(CliError::Usage(
                    "--idempotency-key applies only to Edge mutation commands".into(),
                ));
            }
            let token = cli
                .token
                .clone()
                .or_else(|| getenv(config::env::TOKEN).filter(|s| !s.is_empty()))
                .ok_or_else(|| {
                    CliError::NotAuthenticated(
                        "login needs a token via --token or $MYELIN_TOKEN (the human-login IdP mint \
                         is MR-012-deferred)".into(),
                    )
                })?;
            let path = store_token(&token, &getenv)?;
            println!("stored capability token at {}", path.display());
            Ok(())
        }
        Command::Whoami => {
            let call = EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/whoami".into(),
                query: None,
                payload: None,
                idempotency_key: None,
            };
            run_call(&cli, &getenv, &read_file, call, None).await
        }
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
    let edge = resolve_edge(cli.edge.as_deref(), cli.scheme.as_deref(), getenv);
    let token = resolve_token(cli.token.as_deref(), getenv, read_file)?;
    if call.path.starts_with("/v1/ci/runs/") && call.path.ends_with("/log/live") {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        return myelin_cli::ci_watch::execute_ci_watch(&edge, &token, &call, cli.json, &mut output)
            .await;
    }
    let value = execute(&edge, &token, &call).await?;
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
