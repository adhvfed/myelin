use clap::{Args, Parser, Subcommand};
use myelin_cli::client::execute;
use myelin_cli::config::{
    self, load_profile_credential, remove_profile_credential, resolve_edge,
    resolve_profile_credential, resolve_project, selected_saved_profile, store_profile_credential,
    EdgeConfig, SESSION_SCHEME,
};
use myelin_cli::context as cli_context;
use myelin_cli::device_auth::{
    begin_authorization, new_authorization_request, wait_for_authorization,
};
use myelin_cli::dispatch::{
    chat_dispatch, ci_dispatch, git_dispatch, issues_dispatch_with_project, knowledge_dispatch,
    notif_dispatch, repo_dispatch, EdgeCall, HttpMethod,
};
use myelin_cli::error::CliError;
use myelin_cli::git_credential::{
    self, CredentialScope, GitConfiguration, Operation as GitCredentialOperation,
};

#[derive(Parser, Debug)]
#[command(name = "myelin", version, about, long_about = None)]
struct Cli {
    /// Render the stable machine-readable response instead of human output.
    #[arg(long, global = true)]
    json: bool,
    /// Override the Edge endpoint for this command.
    #[arg(long, global = true)]
    edge: Option<String>,
    /// Use an explicit short-lived token instead of a saved credential.
    #[arg(long, global = true)]
    token: Option<String>,
    /// Declare the explicit token's authentication scheme.
    #[arg(long, global = true)]
    scheme: Option<String>,
    /// Select a saved endpoint, context, and credential bundle.
    #[arg(long, global = true)]
    profile: Option<String>,
    /// Override the active project for this command.
    #[arg(long, global = true)]
    project: Option<String>,
    /// Give a mutation a retry-stable identity.
    #[arg(long, global = true)]
    idempotency_key: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(hide = true)]
    Login(LoginArgs),
    /// Sign in, inspect the current identity, or configure native Git.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// List, inspect, or switch saved endpoint and tenant contexts.
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    /// Show the principal authenticated by the selected credential.
    Whoami,
    #[command(hide = true)]
    Git {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Work with repositories, pull requests, and code search.
    Repo {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List, create, inspect, and close issues.
    #[command(name = "issue", visible_alias = "issues")]
    Issue {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Inspect CI runs and logs.
    Ci {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Work with conversations and messages.
    Chat {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Work with knowledge pages.
    #[command(name = "doc", visible_aliases = ["kb", "knowledge"])]
    Knowledge {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read and complete personal notifications.
    #[command(name = "inbox", visible_alias = "notif")]
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
    /// Let Git use the saved Myelin session for this Edge.
    ConfigureGit,
    /// Remove this Myelin session helper from global Git configuration.
    UnconfigureGit,
    #[command(hide = true)]
    GitCredential {
        operation: String,
    },
}

#[derive(Subcommand, Debug)]
enum ContextCommand {
    /// List the contexts saved on this device.
    List,
    /// Show the active context and verify its identity with Edge.
    Current,
    /// Make a saved profile the default for subsequent commands.
    Use { name: Option<String> },
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
            AuthCommand::ConfigureGit => configure_git(&cli, &getenv, &read_file, false),
            AuthCommand::UnconfigureGit => configure_git(&cli, &getenv, &read_file, true),
            AuthCommand::GitCredential { operation } => {
                serve_git_credential(&cli, operation, &getenv, &read_file)
            }
        },
        Command::Context { command } => match command {
            ContextCommand::List => {
                reject_context_flags(&cli, true)?;
                cli_context::list(cli.json, &getenv, &read_file)
            }
            ContextCommand::Current => {
                reject_context_flags(&cli, true)?;
                cli_context::current(cli.json, cli.profile.as_deref(), &getenv, &read_file).await
            }
            ContextCommand::Use { name } => {
                reject_context_use_flags(&cli)?;
                cli_context::select(cli.json, name.as_deref(), cli.project.as_deref(), &getenv)
            }
        },
        Command::Whoami => run_call(&cli, &getenv, &read_file, whoami_call(), None).await,
        Command::Git { args } => {
            let (call, command_key) = dispatch_command(args, git_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Repo { args } => {
            let (call, command_key) = dispatch_command(args, repo_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Issue { args } => {
            let project = resolve_project(
                cli.project.as_deref(),
                cli.profile.as_deref(),
                &getenv,
                &read_file,
            )?;
            let (call, command_key) = dispatch_command(args, |args| {
                issues_dispatch_with_project(args, project.as_deref())
            })?;
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
        let edge = login_edge(cli, &scheme, getenv)?;
        let mut context = cli_context::inspect(&edge, &token).await?;
        context.project = resolve_login_project(cli, getenv)?;
        let path = store_profile_credential(
            cli.profile.as_deref(),
            &token,
            &scheme,
            Some(&edge.url),
            None,
            Some(&context),
            getenv,
        )?;
        println!(
            "Stored the {scheme} credential in the OS credential store; metadata is at {}. Run `myelin auth status` to verify it.",
            path.display()
        );
        return Ok(());
    }
    if cli.scheme.is_some() || getenv(config::env::SCHEME).is_some() {
        return Err(CliError::Usage(
            "--scheme and $MYELIN_TOKEN_SCHEME apply only to a supplied token".into(),
        ));
    }

    let edge = login_edge(cli, SESSION_SCHEME, getenv)?;
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
    let mut context = cli_context::inspect(&edge, &authorized.credential.token).await?;
    context.project = resolve_login_project(cli, getenv)?;
    let path = store_profile_credential(
        cli.profile.as_deref(),
        &authorized.credential.token,
        &authorized.credential.scheme,
        Some(&edge.url),
        Some(authorized.expires_at_unix()),
        Some(&context),
        getenv,
    )?;
    let expires_at = chrono::DateTime::from_timestamp(authorized.expires_at_unix(), 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| authorized.expires_at_unix().to_string());
    println!("Approved. Your CLI session is ready until {expires_at}.");
    println!(
        "Credential metadata is stored at {}; the secret is in the OS credential store.",
        path.display()
    );
    Ok(())
}

fn login_edge(
    cli: &Cli,
    scheme: &str,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<EdgeConfig, CliError> {
    let read = |path: &std::path::Path| std::fs::read_to_string(path).ok();
    let saved = selected_saved_profile(cli.profile.as_deref(), getenv, &read)?;
    Ok(resolve_edge(
        cli.edge.as_deref(),
        Some(scheme),
        saved.as_ref().map(|profile| profile.edge_url.as_str()),
        getenv,
    ))
}

fn resolve_login_project(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<String>, CliError> {
    let read = |path: &std::path::Path| std::fs::read_to_string(path).ok();
    resolve_project(
        cli.project.as_deref(),
        cli.profile.as_deref(),
        getenv,
        &read,
    )
}

fn logout(cli: &Cli, getenv: &dyn Fn(&str) -> Option<String>) -> Result<(), CliError> {
    reject_auth_mutation_flags(cli)?;
    let removed = remove_profile_credential(cli.profile.as_deref(), getenv)?;
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
            "Removed the selected profile's stored credential. A --token or $MYELIN_TOKEN credential is still active for this process."
        );
    } else if removed {
        println!("Removed the selected profile's stored credential from this device.");
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

fn configure_git(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
    remove: bool,
) -> Result<(), CliError> {
    reject_git_auth_flags(cli)?;
    let selected =
        load_profile_credential(cli.profile.as_deref(), getenv, read_file)?.ok_or_else(|| {
            CliError::NotAuthenticated(
                "Git setup needs a saved credential; run `myelin auth login` first".into(),
            )
        })?;
    let credential = selected.credential;
    if !remove {
        credential.ensure_not_expired()?;
    }
    let edge_url = credential.edge_url.as_deref().ok_or_else(|| {
        CliError::NotAuthenticated(
            "the saved credential predates Edge-aware login; run `myelin auth login` again".into(),
        )
    })?;
    let scope = CredentialScope::from_edge_url(edge_url)?;
    let executable = std::env::current_exe().map_err(|error| {
        CliError::Config(format!("cannot locate the Myelin executable: {error}"))
    })?;
    let configuration = GitConfiguration::new(
        &scope,
        &executable,
        &credential.scheme,
        &selected.profile_name,
    )?;
    let changed = if remove {
        git_credential::unconfigure(&configuration)?
    } else {
        git_credential::configure(&configuration)?
    };
    match (remove, changed) {
        (false, true) => println!(
            "Git is ready to use your saved Myelin session for {}.",
            scope.edge_origin()
        ),
        (false, false) => println!("Git was already configured for {}.", scope.edge_origin()),
        (true, true) => println!(
            "Removed the Myelin credential helper for {}.",
            scope.edge_origin()
        ),
        (true, false) => println!(
            "No Myelin credential helper was configured for {}.",
            scope.edge_origin()
        ),
    }
    Ok(())
}

fn serve_git_credential(
    cli: &Cli,
    operation: &str,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
) -> Result<(), CliError> {
    reject_git_auth_flags(cli)?;
    let operation = GitCredentialOperation::parse(operation)?;
    let credential = load_profile_credential(cli.profile.as_deref(), getenv, read_file)?
        .map(|selected| selected.credential)
        .ok_or_else(|| {
            CliError::NotAuthenticated(
                "Git authentication needs a saved credential; run `myelin auth login` first".into(),
            )
        })?;
    git_credential::serve(
        operation,
        &credential,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}

fn reject_git_auth_flags(cli: &Cli) -> Result<(), CliError> {
    if cli.json
        || cli.edge.is_some()
        || cli.token.is_some()
        || cli.scheme.is_some()
        || cli.idempotency_key.is_some()
    {
        return Err(CliError::Usage(
            "Git credential setup uses only the Edge-bound credential saved by `myelin auth login`"
                .into(),
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

fn reject_context_flags(cli: &Cli, allow_profile: bool) -> Result<(), CliError> {
    if cli.edge.is_some()
        || cli.token.is_some()
        || cli.scheme.is_some()
        || cli.project.is_some()
        || cli.idempotency_key.is_some()
        || (!allow_profile && cli.profile.is_some())
    {
        return Err(CliError::Usage(
            "context commands use saved profiles and do not accept credential or Edge overrides"
                .into(),
        ));
    }
    Ok(())
}

fn reject_context_use_flags(cli: &Cli) -> Result<(), CliError> {
    if cli.edge.is_some()
        || cli.token.is_some()
        || cli.scheme.is_some()
        || cli.idempotency_key.is_some()
        || cli.profile.is_some()
    {
        return Err(CliError::Usage(
            "context use changes a saved profile and does not accept credential or Edge overrides"
                .into(),
        ));
    }
    Ok(())
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
    let credential = resolve_profile_credential(
        cli.profile.as_deref(),
        cli.token.as_deref(),
        cli.scheme.as_deref(),
        getenv,
        read_file,
    )?;
    credential.ensure_not_expired()?;
    let saved_profile = selected_saved_profile(cli.profile.as_deref(), getenv, read_file)?;
    let edge = resolve_edge(
        cli.edge.as_deref(),
        Some(&credential.scheme),
        credential.edge_url.as_deref().or_else(|| {
            saved_profile
                .as_ref()
                .map(|profile| profile.edge_url.as_str())
        }),
        getenv,
    );
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

    #[test]
    fn canonical_product_nouns_parse_without_legacy_wrappers() {
        for args in [
            &["myelin", "repo", "list"][..],
            &["myelin", "repo", "pr", "list"][..],
            &["myelin", "issue", "list"][..],
            &["myelin", "inbox", "list"][..],
            &["myelin", "doc", "page", "list"][..],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|error| panic!("{args:?}: {error}"));
        }
    }

    #[test]
    fn project_is_a_global_override_and_context_use_can_set_it_without_switching_profiles() {
        let project = "11111111-1111-1111-1111-111111111111";
        let context =
            Cli::try_parse_from(["myelin", "context", "use", "--project", project]).unwrap();
        assert_eq!(context.project.as_deref(), Some(project));
        assert!(matches!(
            context.command,
            Command::Context {
                command: ContextCommand::Use { name: None }
            }
        ));

        let issue = Cli::try_parse_from([
            "myelin",
            "--project",
            project,
            "issue",
            "create",
            "--type",
            "22222222-2222-2222-2222-222222222222",
            "--prefix",
            "ENG",
            "--title",
            "Contextual",
        ])
        .unwrap();
        assert_eq!(issue.project.as_deref(), Some(project));
    }

    #[test]
    fn repo_noun_projects_repo_and_nested_git_commands() {
        assert_eq!(repo_dispatch(&["list"]).unwrap().path, "/v1/git/repos");
        assert_eq!(repo_dispatch(&["pr", "list"]).unwrap().path, "/v1/git/prs");
        assert_eq!(
            repo_dispatch(&["search", "code", "needle"]).unwrap().path,
            "/v1/git/search/code"
        );
    }
}
