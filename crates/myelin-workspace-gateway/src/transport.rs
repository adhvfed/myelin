use std::collections::HashMap;

use myelin_ci_sandbox::gvisor::WorkspaceSessionCommand;
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
            session_started: false,
        }
    }
}

pub struct WorkspaceSshConnection<A> {
    authenticator: WorkspaceSshAuthenticator<A>,
    launcher: LocalConfinedWorkspaceLauncher,
    authenticated: Option<AuthenticatedWorkspace>,
    pending_channels: HashMap<ChannelId, Channel<Msg>>,
    session_started: bool,
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
        let Ok(confined) = self.launcher.launch(&revalidated, command).await else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };

        session.channel_success(channel_id)?;
        spawn_session_bridge(confined, channel, self.authenticator.clone(), revalidated);
        Ok(())
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
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
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
