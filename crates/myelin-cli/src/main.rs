use clap::{Args, Parser, Subcommand};
use myelin_cli::client::execute;
use myelin_cli::config::{
    self, load_profile_credential, remove_profile_credential, resolve_edge,
    resolve_profile_credential, resolve_project, selected_saved_profile, set_profile_project,
    store_profile_credential, EdgeConfig, SESSION_SCHEME,
};
use myelin_cli::context as cli_context;
use myelin_cli::device_auth::{
    begin_authorization, new_authorization_request, wait_for_authorization,
};
use myelin_cli::dispatch::{
    agent_dispatch_with_project, automation_dispatch, chat_dispatch_with_project, ci_dispatch,
    git_dispatch, is_canonical_project_id, issues_dispatch_with_context, knowledge_dispatch,
    notif_dispatch, privacy_dispatch, project_dispatch, refs_dispatch, repo_dispatch,
    tool_dispatch, EdgeCall, HttpMethod, RetryPolicy,
};
use myelin_cli::error::CliError;
use myelin_cli::git_credential::{
    self, CredentialScope, GitConfiguration, Operation as GitCredentialOperation,
};
use myelin_issues::api::MAX_ISSUE_IMPORT_JSON_BYTES;

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
    /// Create, discover, and inspect authorization-scoped projects.
    Project {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show the principal authenticated by the selected credential.
    Whoami,
    /// Diagnose the saved development context without changing it.
    Doctor,
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
    /// Follow permission-filtered links between Myelin artifacts.
    #[command(name = "ref", visible_alias = "refs")]
    Ref {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read, snooze, and complete personal notifications.
    #[command(name = "inbox", visible_alias = "notif")]
    Notif {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Discover the typed operations shared by people and agents.
    Tool {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Activate and inspect durable external-agent identities.
    Agent {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Create, inspect, pause, and retire governed event-driven agent work.
    #[command(visible_alias = "trigger")]
    Automation {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Inspect or erase your own agent traces and replay journals.
    Privacy {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Serve governed MCP tools through a short-lived external-agent run.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
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

#[derive(Subcommand, Debug)]
enum McpCommand {
    /// Proxy newline-delimited JSON-RPC over stdin/stdout for an activated agent.
    Serve {
        /// Run as this activated external-agent identity.
        #[arg(long = "as", value_name = "AGENT_ID")]
        agent_id: String,
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
        Command::Project { args } => {
            let project = resolve_project(
                cli.project.as_deref(),
                cli.profile.as_deref(),
                &getenv,
                &read_file,
            )?;
            let (call, command_key) =
                dispatch_command(args, |args| project_dispatch(args, project.as_deref()))?;
            let effect = if call.method == HttpMethod::Post && call.path == "/v1/projects" {
                ResponseEffect::SelectCreatedProject
            } else {
                ResponseEffect::None
            };
            run_call_with_effect(&cli, &getenv, &read_file, call, command_key, effect).await
        }
        Command::Whoami => run_call(&cli, &getenv, &read_file, whoami_call(), None).await,
        Command::Doctor => doctor(&cli, &getenv, &read_file).await,
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
                issues_dispatch_with_context(args, project.as_deref(), &read_import_input)
            })?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Ci { args } => {
            let (call, command_key) = dispatch_command(args, ci_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Chat { args } => {
            let project = resolve_project(
                cli.project.as_deref(),
                cli.profile.as_deref(),
                &getenv,
                &read_file,
            )?;
            let (call, command_key) = dispatch_command(args, |args| {
                chat_dispatch_with_project(args, project.as_deref())
            })?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Knowledge { args } => {
            let (call, command_key) = dispatch_command(args, knowledge_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Ref { args } => {
            let (call, command_key) = dispatch_command(args, refs_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Notif { args } => {
            let (call, command_key) = dispatch_command(args, notif_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Tool { args } => {
            let (call, command_key) = dispatch_command(args, tool_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Agent { args } => {
            let project = resolve_project(
                cli.project.as_deref(),
                cli.profile.as_deref(),
                &getenv,
                &read_file,
            )?;
            let (call, command_key) = dispatch_command(args, |args| {
                agent_dispatch_with_project(args, project.as_deref())
            })?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Automation { args } => {
            let (call, command_key) = dispatch_command(args, automation_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Privacy { args } => {
            let (call, command_key) = dispatch_command(args, privacy_dispatch)?;
            run_call(&cli, &getenv, &read_file, call, command_key).await
        }
        Command::Mcp { command } => match command {
            McpCommand::Serve { agent_id } => serve_mcp(&cli, agent_id, &getenv, &read_file).await,
        },
    }
}

async fn doctor(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
) -> Result<(), CliError> {
    if cli.edge.is_some()
        || cli.token.is_some()
        || cli.scheme.is_some()
        || cli.project.is_some()
        || cli.idempotency_key.is_some()
    {
        return Err(CliError::Usage(
            "doctor inspects a saved browser-approved context and accepts only --profile and --json"
                .into(),
        ));
    }
    let report = myelin_cli::doctor::diagnose(cli.profile.as_deref(), getenv, read_file).await?;
    print!("{}", report.render(cli.json));
    Ok(())
}

async fn serve_mcp(
    cli: &Cli,
    agent_id: &str,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
) -> Result<(), CliError> {
    if cli.json
        || cli.edge.is_some()
        || cli.token.is_some()
        || cli.scheme.is_some()
        || cli.project.is_some()
        || cli.idempotency_key.is_some()
    {
        return Err(CliError::Usage(
            "MCP serving uses only the Edge-bound browser session saved by `myelin auth login`"
                .into(),
        ));
    }
    let selected =
        load_profile_credential(cli.profile.as_deref(), getenv, read_file)?.ok_or_else(|| {
            CliError::NotAuthenticated(
                "MCP serving needs a saved browser session; run `myelin auth login` first".into(),
            )
        })?;
    let credential = selected.credential;
    credential.ensure_not_expired()?;
    if credential.scheme != SESSION_SCHEME {
        return Err(CliError::NotAuthenticated(
            "MCP serving needs a browser-approved session; run `myelin auth login` first".into(),
        ));
    }
    let edge_url = credential.edge_url.as_deref().ok_or_else(|| {
        CliError::NotAuthenticated(
            "the saved credential predates Edge-aware login; run `myelin auth login` again".into(),
        )
    })?;
    let edge = EdgeConfig {
        url: edge_url.into(),
        scheme: SESSION_SCHEME.into(),
    };
    myelin_cli::mcp_bridge::serve_stdio(&edge, &credential.token, agent_id).await
}

fn read_import_input(path: &str) -> Result<String, CliError> {
    use std::io::Read as _;

    fn read_bounded(reader: impl std::io::Read) -> Result<String, CliError> {
        let mut contents = Vec::new();
        reader
            .take((MAX_ISSUE_IMPORT_JSON_BYTES + 1) as u64)
            .read_to_end(&mut contents)
            .map_err(|error| {
                CliError::Usage(format!("could not read issue import input: {error}"))
            })?;
        if contents.len() > MAX_ISSUE_IMPORT_JSON_BYTES {
            return Err(CliError::Usage(format!(
                "issue import input exceeds the {MAX_ISSUE_IMPORT_JSON_BYTES}-byte request limit"
            )));
        }
        String::from_utf8(contents)
            .map_err(|error| CliError::Usage(format!("issue import input is not UTF-8: {error}")))
    }

    if path == "-" {
        read_bounded(std::io::stdin().lock())
    } else {
        let file = std::fs::File::open(path).map_err(|error| {
            CliError::Usage(format!("could not open issue import input: {error}"))
        })?;
        read_bounded(file)
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
        retry_policy: RetryPolicy::None,
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
    run_call_with_effect(
        cli,
        getenv,
        read_file,
        call,
        command_key,
        ResponseEffect::None,
    )
    .await
}

#[derive(Clone, Copy)]
enum ResponseEffect {
    None,
    SelectCreatedProject,
}

async fn run_call_with_effect(
    cli: &Cli,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
    call: EdgeCall,
    command_key: Option<&str>,
    effect: ResponseEffect,
) -> Result<(), CliError> {
    let idempotency_key = match (cli.idempotency_key.as_deref(), command_key) {
        (Some(_), Some(_)) => return Err(CliError::Usage("duplicate --idempotency-key".into())),
        (top_level, trailing) => top_level.or(trailing),
    };
    let call = match call.retry_policy {
        RetryPolicy::CallerKeyRequired => {
            let key = idempotency_key.ok_or_else(|| {
                CliError::Usage(
                    "mutating commands require --idempotency-key <key>; reuse the same key when \
                     retrying after a lost response"
                        .into(),
                )
            })?;
            call.with_idempotency_key(key)?
        }
        RetryPolicy::None => {
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
    let context_profile = saved_profile
        .as_ref()
        .filter(|profile| {
            credential.edge_url.as_deref() == Some(profile.edge_url.as_str())
                && edge.url == profile.edge_url
        })
        .map(|profile| profile.name.clone());
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
    let context_message =
        apply_response_effect(effect, &value, context_profile.as_deref(), getenv)?;
    print!(
        "{}",
        myelin_cli::render::render_for_call(&value, cli.json, &call)
    );
    if !cli.json {
        if let Some(message) = context_message {
            println!("{message}");
        }
    }
    Ok(())
}

fn apply_response_effect(
    effect: ResponseEffect,
    value: &serde_json::Value,
    context_profile: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<String>, CliError> {
    match effect {
        ResponseEffect::None => Ok(None),
        ResponseEffect::SelectCreatedProject => {
            let project_id = value
                .get("project")
                .and_then(|project| project.get("id"))
                .and_then(serde_json::Value::as_str)
                .filter(|value| is_canonical_project_id(value))
                .ok_or_else(|| {
                    CliError::Transport(
                        "Edge returned a malformed project creation response: id is missing or invalid"
                            .into(),
                    )
                })?;
            let Some(profile_name) = context_profile else {
                return Ok(Some(format!(
                    "To make it the default for a saved context: myelin context use --project {project_id}"
                )));
            };
            set_profile_project(profile_name, project_id, getenv).map_err(|error| {
                CliError::Config(format!(
                    "project {project_id} was created, but profile `{profile_name}` could not save it as the default: {error}; run `myelin context use --project {project_id}`"
                ))
            })?;
            Ok(Some(format!(
                "Default project for `{profile_name}`: {project_id}"
            )))
        }
    }
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
        let (call, key) = dispatch_command(&args, |args| {
            chat_dispatch_with_project(args, Some("11111111-1111-1111-1111-111111111111"))
        })
        .unwrap();
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
                dispatch_command(&args, myelin_cli::dispatch::chat_dispatch)
                    .unwrap_err()
                    .code(),
                2
            );
        }
    }

    #[test]
    fn canonical_product_nouns_parse_without_legacy_wrappers() {
        for args in [
            &["myelin", "repo", "list"][..],
            &["myelin", "repo", "pr", "list"][..],
            &["myelin", "project", "list"][..],
            &["myelin", "agent", "list"][..],
            &[
                "myelin",
                "mcp",
                "serve",
                "--as",
                "11111111-1111-1111-1111-111111111111",
            ][..],
            &["myelin", "doctor"][..],
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
    fn successful_project_creation_without_a_saved_context_returns_an_actionable_hint() {
        let value = serde_json::json!({
            "project": { "id": "11111111-1111-1111-1111-111111111111" }
        });
        let message =
            apply_response_effect(ResponseEffect::SelectCreatedProject, &value, None, &|_| {
                None
            })
            .unwrap()
            .unwrap();
        assert!(message.contains("context use --project 11111111-1111-1111-1111-111111111111"));

        assert!(apply_response_effect(
            ResponseEffect::SelectCreatedProject,
            &serde_json::json!({"project": {"id": "not-a-uuid"}}),
            None,
            &|_| None,
        )
        .is_err());
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
