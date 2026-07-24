//! # `myelin` — the CLI binary (E0.9 / MR-020)
//!
//! The thin clap shell: parse `myelin <subsystem> <command> [args]`, resolve the capability token,
//! present it as `Authorization: Bearer`, call the edge, and render the view-model. ALL the policy
//! (grammar reuse, the edge client, the envelope parsing, the rendering, the exit codes) lives in the
//! library ([`myelin_cli`]); this binary wires clap to it and maps the typed [`CliError`] to a clean
//! stderr message + a non-zero exit (never a panic).

use clap::{Parser, Subcommand};
use myelin_cli::client::execute;
use myelin_cli::config::{self, resolve_edge, resolve_token, store_token};
use myelin_cli::dispatch::{
    ci_dispatch, git_dispatch, issues_dispatch, notif_dispatch, EdgeCall, HttpMethod,
};
use myelin_cli::error::CliError;

/// The `myelin` CLI — drive Git / notifications / … through the product edge with a capability token.
#[derive(Parser, Debug)]
#[command(name = "myelin", version, about, long_about = None)]
struct Cli {
    /// Emit machine-readable JSON (for scripting / agents) instead of the human form. Place BEFORE
    /// the subsystem (e.g. `myelin --json git repo list`).
    #[arg(long, global = true)]
    json: bool,
    /// The edge base URL. Remote edges require verified HTTPS; the default loopback HTTP URL is for
    /// local development (`http://127.0.0.1:8080`, or `$MYELIN_EDGE`).
    #[arg(long, global = true)]
    edge: Option<String>,
    /// The capability token (overrides `$MYELIN_TOKEN` / the stored token). Prefer `myelin login`.
    #[arg(long, global = true)]
    token: Option<String>,
    /// The token scheme presented to the edge (default `agent`, or `$MYELIN_TOKEN_SCHEME`).
    #[arg(long, global = true)]
    scheme: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Store a capability token (from `--token` or `$MYELIN_TOKEN`) for later commands. NOTE: the
    /// full human-login mint (IdP → token) is MR-012-deferred; this stores a token you already hold.
    Login,
    /// Show the authenticated principal (GET /v1/whoami) — proves the Bearer presentation + edge auth.
    Whoami,
    /// Git commands — REUSES myelin-git's grammar: `repo list [--limit 1..100] [--cursor <rl1_…>]`
    /// | `pr view <repo> <n>` | `search code <q> [--repo <slug>]` | …
    Git {
        /// The git subcommand + args, parsed by git's own `parse_cli`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Issue commands: `list` | `create --project … --type … --prefix … --title …` | `view` | `close`.
    Issues {
        /// The Issues subcommand + args, parsed by the subsystem's own total grammar.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Durable CI reads: `list [--status …]` | `view <run>` |
    /// `logs <run> --job <job> [--start <byte>] [--limit <bytes>]`.
    Ci {
        /// The CI subcommand + args, parsed by CI's own total grammar.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Notification/inbox commands: `list [--view <v>] [--limit <n>] [--cursor <c>]` | `show <id>`.
    #[command(alias = "inbox")]
    Notif {
        /// The notif subcommand + args, parsed by notif's own grammar.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let exit = match run().await {
        Ok(()) => 0,
        Err(e) => {
            // A clean message + a stable non-zero exit — never a panic/backtrace. The token is never
            // part of a CliError, so it cannot leak here.
            eprintln!("myelin: {e}");
            e.code()
        }
    };
    std::process::exit(exit);
}

async fn run() -> Result<(), CliError> {
    // clap handles `--help`/usage errors itself (exit 2). Past `parse`, every error is a typed CliError.
    let cli = Cli::parse();
    let getenv = |k: &str| std::env::var(k).ok();
    let read_file = |p: &std::path::Path| std::fs::read_to_string(p).ok();

    match &cli.command {
        Command::Login => {
            // Acquire the token from --token or $MYELIN_TOKEN (the IdP mint is the MR-012 seam).
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
            // Print the PATH only — never the token (the never-log-the-token floor).
            println!("stored capability token at {}", path.display());
            Ok(())
        }
        Command::Whoami => {
            let call = EdgeCall {
                method: HttpMethod::Get,
                path: "/v1/whoami".into(),
                query: None,
                payload: None,
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

/// Resolve the token + edge config, run the call, and print the rendered view-model.
async fn run_call(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
    call: EdgeCall,
) -> Result<(), CliError> {
    let edge = resolve_edge(cli.edge.as_deref(), cli.scheme.as_deref(), getenv);
    let token = resolve_token(cli.token.as_deref(), getenv, read_file)?;
    let value = execute(&edge, &token, &call).await?;
    print!(
        "{}",
        myelin_cli::render::render_for_call(&value, cli.json, &call)
    );
    Ok(())
}
