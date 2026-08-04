use clap::{Parser, Subcommand};
use myelin_cli::client::execute;
use myelin_cli::config::{self, resolve_edge, resolve_token, store_token};
use myelin_cli::dispatch::{
    ci_dispatch, git_dispatch, issues_dispatch, notif_dispatch, EdgeCall, HttpMethod,
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
            run_call(&cli, &getenv, &read_file, call).await
        }
        Command::Git { args } => {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let call = git_dispatch(&refs)?;
            run_call(&cli, &getenv, &read_file, call).await
        }
        Command::Issues { args } => {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let call = issues_dispatch(&refs)?;
            run_call(&cli, &getenv, &read_file, call).await
        }
        Command::Ci { args } => {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let call = ci_dispatch(&refs)?;
            run_call(&cli, &getenv, &read_file, call).await
        }
        Command::Notif { args } => {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let call = notif_dispatch(&refs)?;
            run_call(&cli, &getenv, &read_file, call).await
        }
    }
}

async fn run_call(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
    call: EdgeCall,
) -> Result<(), CliError> {
    let edge = resolve_edge(cli.edge.as_deref(), cli.scheme.as_deref(), getenv);
    let token = resolve_token(cli.token.as_deref(), getenv, read_file)?;
    let call = match call.method {
        HttpMethod::Post => {
            let key = cli.idempotency_key.as_deref().ok_or_else(|| {
                CliError::Usage(
                    "mutating commands require --idempotency-key <key>; reuse the same key when \
                     retrying after a lost response"
                        .into(),
                )
            })?;
            call.with_idempotency_key(key)?
        }
        HttpMethod::Get => {
            if cli.idempotency_key.is_some() {
                return Err(CliError::Usage(
                    "--idempotency-key applies only to mutating commands".into(),
                ));
            }
            call
        }
    };
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
