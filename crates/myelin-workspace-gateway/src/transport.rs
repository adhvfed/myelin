use std::collections::HashMap;

use myelin_ci_sandbox::gvisor::{
    ConfinedWorkspaceSessionHandle, WorkspaceSessionCommand, WorkspaceTerminal,
};
use russh::keys::ssh_key::PublicKey;
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId, Pty};

use crate::session_bridge::spawn_session_bridge;
use crate::{
    AuthenticatedWorkspace, LocalConfinedWorkspaceLauncher, WorkspaceSshAdmissionStore,
    WorkspaceSshAuthenticationError, WorkspaceSshAuthenticator,
};

#[derive(Clone)]
pub struct WorkspaceSshGateway<A> {
    authenticator: WorkspaceSshAuthenticator<A>,
    launcher: LocalConfinedWorkspaceLauncher,
}

impl<A> WorkspaceSshGateway<A>
where
    A: WorkspaceSshAdmissionStore,
{
    pub fn new(
        authenticator: WorkspaceSshAuthenticator<A>,
        launcher: LocalConfinedWorkspaceLauncher,
    ) -> Self {
        Self {
            authenticator,
            launcher,
        }
    }
}

impl<A> Server for WorkspaceSshGateway<A>
where
    A: WorkspaceSshAdmissionStore,
{
    type Handler = WorkspaceSshConnection<A>;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        WorkspaceSshConnection {
            authenticator: self.authenticator.clone(),
            launcher: self.launcher.clone(),
            authenticated: None,
            pending_channels: HashMap::new(),
            pending_terminals: HashMap::new(),
            active_session: None,
            session_started: false,
        }
    }
}

pub struct WorkspaceSshConnection<A> {
    authenticator: WorkspaceSshAuthenticator<A>,
    launcher: LocalConfinedWorkspaceLauncher,
    authenticated: Option<AuthenticatedWorkspace>,
    pending_channels: HashMap<ChannelId, Channel<Msg>>,
    pending_terminals: HashMap<ChannelId, WorkspaceTerminal>,
    active_session: Option<ActiveWorkspaceSession>,
    session_started: bool,
}

struct ActiveWorkspaceSession {
    channel: ChannelId,
    handle: ConfinedWorkspaceSessionHandle,
}

fn requested_terminal(
    term: &str,
    columns: u32,
    rows: u32,
    pixel_width: u32,
    pixel_height: u32,
    modes: &[(Pty, u32)],
) -> Option<WorkspaceTerminal> {
    if modes.len() > 128 {
        return None;
    }
    WorkspaceTerminal::new(term, columns, rows, pixel_width, pixel_height).ok()
}

impl<A> WorkspaceSshConnection<A>
where
    A: WorkspaceSshAdmissionStore,
{
    pub fn authenticated_workspace(&self) -> Option<&AuthenticatedWorkspace> {
        self.authenticated.as_ref()
    }

    async fn start_session(
        &mut self,
        channel_id: ChannelId,
        command: WorkspaceSessionCommand,
        session: &mut Session,
    ) -> Result<(), WorkspaceSshAuthenticationError> {
        let Some(channel) = self.pending_channels.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        let terminal = self.pending_terminals.remove(&channel_id);
        if self.session_started {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        self.session_started = true;
        let Some(authenticated) = self.authenticated.clone() else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        let Some(revalidated) = self.authenticator.revalidate(&authenticated).await? else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        let Ok(confined) = self.launcher.launch(&revalidated, command, terminal).await else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };

        self.active_session = Some(ActiveWorkspaceSession {
            channel: channel_id,
            handle: confined.handle(),
        });
        session.channel_success(channel_id)?;
        spawn_session_bridge(confined, channel, self.authenticator.clone(), revalidated);
        Ok(())
    }
}

impl<A> Drop for WorkspaceSshConnection<A> {
    fn drop(&mut self) {
        if let Some(active) = self.active_session.as_ref() {
            active.handle.terminate();
        }
    }
}

impl<A> Handler for WorkspaceSshConnection<A>
where
    A: WorkspaceSshAdmissionStore,
{
    type Error = WorkspaceSshAuthenticationError;

    async fn auth_publickey(
        &mut self,
        route_username: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.authenticated = self
            .authenticator
            .authenticate(route_username, public_key)
            .await?;
        Ok(if self.authenticated.is_some() {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.authenticated.is_some() && !self.session_started && self.pending_channels.is_empty()
        {
            self.pending_channels.insert(channel.id(), channel);
            reply.accept().await;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.pending_channels.remove(&channel);
        self.pending_terminals.remove(&channel);
        if self
            .active_session
            .as_ref()
            .is_some_and(|active| active.channel == channel)
        {
            if let Some(active) = self.active_session.take() {
                active.handle.terminate();
            }
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let terminal =
            requested_terminal(term, col_width, row_height, pix_width, pix_height, modes);
        if let Some(terminal) = terminal.filter(|_| {
            self.pending_channels.contains_key(&channel)
                && !self.pending_terminals.contains_key(&channel)
        }) {
            self.pending_terminals.insert(channel, terminal);
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let resized = if let Some(terminal) = self.pending_terminals.get_mut(&channel) {
            terminal
                .resize(col_width, row_height, pix_width, pix_height)
                .is_ok()
        } else if let Some(active) = self
            .active_session
            .as_ref()
            .filter(|active| active.channel == channel)
        {
            active
                .handle
                .resize_terminal(col_width, row_height, pix_width, pix_height)
                .is_ok()
        } else {
            false
        };
        if resized {
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        _variable_name: &str,
        _variable_value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.start_session(channel, WorkspaceSessionCommand::Shell, session)
            .await
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Ok(command) = std::str::from_utf8(data) else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        self.start_session(
            channel,
            WorkspaceSessionCommand::Exec(command.to_string()),
            session,
        )
        .await
    }
}
