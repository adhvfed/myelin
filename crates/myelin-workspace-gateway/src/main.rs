use std::sync::Arc;

use myelin_agent_service::workspace::LocalDevelopmentWorkspaceProvisioner;
use myelin_ci_sandbox::gvisor::{preflight_gvisor_runner_host, verified_gvisor_git_rootfs};
use myelin_config::Mode;
use myelin_identity_service::WorkspaceSshHostIdentity;
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, DurableAgentThreadBacking, HotTables, PgBootstrap,
    WorkspaceSshRouteKey,
};
use myelin_workspace_gateway::{
    workspace_ssh_server_config, LocalConfinedWorkspaceLauncher, WorkspaceGatewayRuntimeConfig,
    WorkspaceSshAuthenticator, WorkspaceSshGateway,
};
use russh::server::Server as _;

#[tokio::main]
async fn main() {
    let runtime = WorkspaceGatewayRuntimeConfig::from_env()
        .unwrap_or_else(|error| refuse_start("runtime configuration", error));
    let rootfs = verified_gvisor_git_rootfs()
        .unwrap_or_else(|error| refuse_start("pinned gVisor rootfs", error));
    if rootfs != canonical(&runtime.gvisor_rootfs) {
        refuse_start(
            "pinned gVisor rootfs",
            "the verified rootfs differs from the configured rootfs",
        );
    }
    preflight_gvisor_runner_host(&runtime.runsc, &rootfs)
        .unwrap_or_else(|error| refuse_start("gVisor host preflight", error));
    let workspaces = LocalDevelopmentWorkspaceProvisioner::open(&runtime.workspace_root)
        .unwrap_or_else(|error| refuse_start("workspace storage", error));

    let bootstrap = PgBootstrap::from_env(Mode::RequireEnv)
        .await
        .unwrap_or_else(|error| refuse_start("database bootstrap", error));
    bootstrap
        .migrate_foundation()
        .await
        .unwrap_or_else(|error| refuse_start("substrate foundation migration", error));
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap_or_else(|error| refuse_start("durable migration aggregate", error));
    let provider = bootstrap
        .into_runtime()
        .await
        .unwrap_or_else(|error| refuse_start("database runtime handoff", error));
    let seal_key = seal_key_from_env().unwrap_or_else(|error| refuse_start("seal key", error));
    let server_config =
        workspace_ssh_server_config(&WorkspaceSshHostIdentity::from_seal_key(&seal_key))
            .unwrap_or_else(|error| refuse_start("SSH host key", error));
    let authenticator = WorkspaceSshAuthenticator::new(
        WorkspaceSshRouteKey::from_seal_key(&seal_key),
        DurableAgentThreadBacking::new(provider),
    );
    let launcher = LocalConfinedWorkspaceLauncher::new(workspaces);
    let mut gateway = WorkspaceSshGateway::new(authenticator, launcher);
    let listener = tokio::net::TcpListener::bind(runtime.listen_addr)
        .await
        .unwrap_or_else(|error| refuse_start("TCP listener", error));
    let local_addr = listener
        .local_addr()
        .unwrap_or_else(|error| refuse_start("TCP listener address", error));
    println!("workspace-gateway: listening on {local_addr}");

    let running = gateway.run_on_socket(Arc::clone(&server_config), &listener);
    let shutdown = running.handle();
    tokio::pin!(running);
    tokio::select! {
        result = &mut running => {
            result.unwrap_or_else(|error| refuse_start("SSH server loop", error));
        }
        () = shutdown_signal() => {
            shutdown.shutdown("workspace gateway is shutting down".into());
            running.await.unwrap_or_else(|error| refuse_start("SSH server shutdown", error));
        }
    }
}

fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize()
        .unwrap_or_else(|error| refuse_start("configured gVisor rootfs", error))
}

fn refuse_start(context: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("workspace-gateway: {context} refused to start: {error}");
    std::process::exit(1)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| refuse_start("SIGTERM handler", error));
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.unwrap_or_else(|error| refuse_start("SIGINT handler", error));
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    refuse_start("SIGTERM stream", "closed unexpectedly");
                }
            }
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .unwrap_or_else(|error| refuse_start("shutdown handler", error));
}
