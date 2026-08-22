use std::collections::HashMap;
use std::fs::File;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use myelin_identity_service::workspace_ssh_public_key_fingerprint;
use myelin_storage::agent_thread_durable::{LiveWorkspaceSshAdmission, WorkspaceSshRouteKey};
use russh::keys::ssh_key::{Algorithm, PublicKey};
use russh::keys::PublicKeyBase64;
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId, Pty};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

use crate::{
    AdmissionLookupError, LiveAdmissionRequest, LiveSessionRequest, LocalConfinedWorkspaceLauncher,
    WorkspaceSshAdmissionStore,
};
use myelin_ci_sandbox::gvisor::{
    ConfinedWorkspaceSession, ConfinedWorkspaceSessionIo, WorkspaceSessionCommand,
};

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Clone)]
pub struct AuthenticatedWorkspace {
    pub tenant: String,
    pub admission: LiveWorkspaceSshAdmission,
    admitted_at: DateTime<Utc>,
    credential: Option<AuthenticatedCredential>,
}

#[derive(Clone)]
struct AuthenticatedCredential {
    route_username: String,
    public_key: PublicKey,
}

impl AuthenticatedWorkspace {
    #[cfg(test)]
    pub(crate) fn from_admission(tenant: String, admission: LiveWorkspaceSshAdmission) -> Self {
        Self {
            tenant,
            admission,
            admitted_at: Utc::now(),
            credential: None,
        }
    }
}

impl std::fmt::Debug for AuthenticatedWorkspace {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AuthenticatedWorkspace")
            .field("tenant", &self.tenant)
            .field("thread_id", &self.admission.thread_id)
            .field("workspace_id", &self.admission.workspace_id)
            .field("workspace_generation", &self.admission.workspace_generation)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum WorkspaceSshAuthenticationError {
    Admission(AdmissionLookupError),
    Transport(russh::Error),
}

impl core::fmt::Display for WorkspaceSshAuthenticationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Admission(error) => error.fmt(formatter),
            Self::Transport(error) => write!(formatter, "workspace SSH transport failed: {error}"),
        }
    }
}

impl std::error::Error for WorkspaceSshAuthenticationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::Transport(error) => Some(error),
        }
    }
}

impl From<AdmissionLookupError> for WorkspaceSshAuthenticationError {
    fn from(error: AdmissionLookupError) -> Self {
        Self::Admission(error)
    }
}

impl From<russh::Error> for WorkspaceSshAuthenticationError {
    fn from(error: russh::Error) -> Self {
        Self::Transport(error)
    }
}

#[derive(Clone)]
pub struct WorkspaceSshAuthenticator<A> {
    routes: WorkspaceSshRouteKey,
    admissions: A,
    clock: Clock,
}

impl<A> WorkspaceSshAuthenticator<A>
where
    A: WorkspaceSshAdmissionStore,
{
    pub fn new(routes: WorkspaceSshRouteKey, admissions: A) -> Self {
        Self {
            routes,
            admissions,
            clock: Arc::new(Utc::now),
        }
    }

    pub fn with_clock(mut self, clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub async fn authenticate(
        &self,
        route_username: &str,
        public_key: &PublicKey,
    ) -> Result<Option<AuthenticatedWorkspace>, AdmissionLookupError> {
        if public_key.algorithm() != Algorithm::Ed25519 {
            return Ok(None);
        }
        let Ok(route) = self.routes.open(route_username) else {
            return Ok(None);
        };
        let authorized_key = format!("ssh-ed25519 {}", public_key.public_key_base64());
        let Ok(public_key_fingerprint) = workspace_ssh_public_key_fingerprint(&authorized_key)
        else {
            return Ok(None);
        };
        let request = LiveAdmissionRequest {
            tenant: route.tenant.clone(),
            grant_id: route.grant_id,
            route_username: route_username.to_string(),
            public_key_fingerprint,
            observed_at: (self.clock)(),
        };
        let admitted_at = request.observed_at;
        Ok(self
            .admissions
            .live_admission(&request)
            .await?
            .map(|admission| AuthenticatedWorkspace {
                tenant: route.tenant,
                admission,
                admitted_at,
                credential: Some(AuthenticatedCredential {
                    route_username: route_username.to_string(),
                    public_key: public_key.clone(),
                }),
            }))
    }

    async fn revalidate(
        &self,
        authenticated: &AuthenticatedWorkspace,
    ) -> Result<Option<AuthenticatedWorkspace>, AdmissionLookupError> {
        let Some(credential) = authenticated.credential.as_ref() else {
            return Ok(None);
        };
        let authorized_key = format!("ssh-ed25519 {}", credential.public_key.public_key_base64());
        let Ok(public_key_fingerprint) = workspace_ssh_public_key_fingerprint(&authorized_key)
        else {
            return Ok(None);
        };
        let Ok(grant_id) = Uuid::parse_str(&authenticated.admission.grant_id) else {
            return Ok(None);
        };
        let current = self
            .admissions
            .live_session(&LiveSessionRequest {
                tenant: authenticated.tenant.clone(),
                grant_id,
                route_username: credential.route_username.clone(),
                public_key_fingerprint,
                admitted_at: authenticated.admitted_at,
                observed_at: (self.clock)(),
            })
            .await?
            .map(|admission| AuthenticatedWorkspace {
                tenant: authenticated.tenant.clone(),
                admission,
                admitted_at: authenticated.admitted_at,
                credential: Some(credential.clone()),
            });
        Ok(current.filter(|current| same_workspace_authority(current, authenticated)))
    }
}

fn same_workspace_authority(
    current: &AuthenticatedWorkspace,
    authenticated: &AuthenticatedWorkspace,
) -> bool {
    current.tenant == authenticated.tenant
        && current.admission.grant_id == authenticated.admission.grant_id
        && current.admission.thread_id == authenticated.admission.thread_id
        && current.admission.owner_principal_id == authenticated.admission.owner_principal_id
        && current.admission.workspace_id == authenticated.admission.workspace_id
        && current.admission.workspace_generation == authenticated.admission.workspace_generation
        && current.admission.storage_locator == authenticated.admission.storage_locator
}

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

fn spawn_session_bridge<A>(
    mut confined: ConfinedWorkspaceSession,
    channel: Channel<Msg>,
    authenticator: WorkspaceSshAuthenticator<A>,
    authenticated: AuthenticatedWorkspace,
) where
    A: WorkspaceSshAdmissionStore,
{
    tokio::spawn(async move {
        let Ok(io) = confined.take_io() else {
            return;
        };
        let session_handle = confined.handle();
        let mut wait = tokio::task::spawn_blocking(move || confined.wait());
        let (mut channel_input, channel_output) = channel.split();
        let ConfinedWorkspaceSessionIo {
            stdin,
            stdout,
            stderr,
        } = io;
        let mut child_input = async_pipe(stdin);
        let mut child_output = async_pipe(stdout);
        let mut child_error = async_pipe(stderr);
        let mut output_destination = channel_output.make_writer();
        let mut error_destination = channel_output.make_writer_ext(Some(1));

        let input_handle = session_handle.clone();
        let input = tokio::spawn(async move {
            let result = tokio::io::copy(&mut channel_input.make_reader(), &mut child_input).await;
            let _ = child_input.shutdown().await;
            if result.is_err() {
                input_handle.terminate();
            }
        });
        let output_handle = session_handle.clone();
        let mut output = tokio::spawn(async move {
            let result = tokio::io::copy(&mut child_output, &mut output_destination).await;
            if result.is_err() {
                output_handle.terminate();
            }
        });
        let error_handle = session_handle.clone();
        let mut error = tokio::spawn(async move {
            let result = tokio::io::copy(&mut child_error, &mut error_destination).await;
            if result.is_err() {
                error_handle.terminate();
            }
        });

        let mut recheck = tokio::time::interval(Duration::from_secs(15));
        recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        recheck.tick().await;
        let exit = loop {
            tokio::select! {
                result = &mut wait => break result,
                _ = recheck.tick() => {
                    match authenticator.revalidate(&authenticated).await {
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => session_handle.terminate(),
                    }
                }
            }
        };

        input.abort();
        let _ = input.await;
        if tokio::time::timeout(Duration::from_secs(1), &mut output)
            .await
            .is_err()
        {
            output.abort();
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut error)
            .await
            .is_err()
        {
            error.abort();
        }
        let code = exit
            .ok()
            .and_then(Result::ok)
            .and_then(|exit| exit.code)
            .and_then(|code| u32::try_from(code).ok())
            .unwrap_or(255);
        let _ = channel_output.exit_status(code).await;
        let _ = channel_output.eof().await;
        let _ = channel_output.close().await;
    });
}

fn async_pipe(pipe: impl IntoRawFd) -> tokio::fs::File {
    let file = unsafe { File::from_raw_fd(pipe.into_raw_fd()) };
    tokio::fs::File::from_std(file)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use myelin_identity_service::WorkspaceSshHostIdentity;
    use myelin_storage::agent_thread_durable::WorkspaceSshRouteKey;
    use myelin_storage::{SealKey, KEY_LEN};
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    use russh::keys::PrivateKey;
    use uuid::Uuid;

    use super::*;

    #[derive(Clone)]
    struct RecordingAdmissions {
        admissions: Arc<Mutex<Vec<LiveAdmissionRequest>>>,
        sessions: Arc<Mutex<Vec<LiveSessionRequest>>>,
        result: Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>,
    }

    impl RecordingAdmissions {
        fn returning(
            result: Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>,
        ) -> Self {
            Self {
                admissions: Arc::new(Mutex::new(Vec::new())),
                sessions: Arc::new(Mutex::new(Vec::new())),
                result,
            }
        }
    }

    impl WorkspaceSshAdmissionStore for RecordingAdmissions {
        fn live_admission<'a>(
            &'a self,
            request: &'a LiveAdmissionRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.admissions.lock().unwrap().push(request.clone());
                self.result.clone()
            })
        }

        fn live_session<'a>(
            &'a self,
            request: &'a LiveSessionRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.sessions.lock().unwrap().push(request.clone());
                self.result.clone()
            })
        }
    }

    fn public_key(seed: [u8; KEY_LEN]) -> PublicKey {
        PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&seed)),
            "ephemeral client",
        )
        .unwrap()
        .public_key()
        .clone()
    }

    fn admission(grant_id: Uuid) -> LiveWorkspaceSshAdmission {
        LiveWorkspaceSshAdmission {
            grant_id: grant_id.to_string(),
            thread_id: Uuid::from_u128(12).to_string(),
            owner_principal_id: "user:alice".into(),
            workspace_id: Uuid::from_u128(13).to_string(),
            workspace_generation: 2,
            storage_locator: "workspace:v1:opaque".into(),
            expires_at: "2026-08-22T12:05:00Z".into(),
        }
    }

    #[tokio::test]
    async fn opaque_route_and_owned_key_resolve_one_exact_live_workspace() {
        let now = Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let seal_key = SealKey::from_bytes([0x81; KEY_LEN]);
        let routes = WorkspaceSshRouteKey::from_seal_key(&seal_key);
        let grant_id = Uuid::from_u128(11);
        let username = routes.seal("acme", grant_id).unwrap();
        let key = public_key([0x82; KEY_LEN]);
        let admissions = RecordingAdmissions::returning(Ok(Some(admission(grant_id))));
        let seen = admissions.admissions.clone();
        let authenticator =
            WorkspaceSshAuthenticator::new(routes, admissions).with_clock(move || now);

        let authenticated = authenticator
            .authenticate(&username, &key)
            .await
            .unwrap()
            .expect("Alice's live ephemeral key should enter its exact workspace");

        assert_eq!(authenticated.tenant, "acme");
        assert_eq!(authenticated.admission.workspace_generation, 2);
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tenant, "acme");
        assert_eq!(requests[0].grant_id, grant_id);
        assert_eq!(requests[0].route_username, username);
        assert_eq!(requests[0].observed_at, now);
        assert_eq!(
            requests[0].public_key_fingerprint,
            key.fingerprint(russh::keys::HashAlg::Sha256).to_string()
        );
    }

    #[tokio::test]
    async fn malformed_routes_are_refused_without_touching_tenant_storage() {
        let admissions = RecordingAdmissions::returning(Ok(Some(admission(Uuid::from_u128(21)))));
        let seen = admissions.admissions.clone();
        let authenticator = WorkspaceSshAuthenticator::new(
            WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([0x91; KEY_LEN])),
            admissions,
        );

        assert!(authenticator
            .authenticate("alice", &public_key([0x92; KEY_LEN]))
            .await
            .unwrap()
            .is_none());
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unavailable_or_expired_grants_never_become_authenticated_workspaces() {
        let seal_key = SealKey::from_bytes([0xa1; KEY_LEN]);
        let routes = WorkspaceSshRouteKey::from_seal_key(&seal_key);
        let username = routes.seal("acme", Uuid::from_u128(31)).unwrap();
        let key = public_key([0xa2; KEY_LEN]);

        let expired = WorkspaceSshAuthenticator::new(
            routes.clone(),
            RecordingAdmissions::returning(Ok(None)),
        );
        assert!(expired
            .authenticate(&username, &key)
            .await
            .unwrap()
            .is_none());

        let unavailable = WorkspaceSshAuthenticator::new(
            routes,
            RecordingAdmissions::returning(Err(AdmissionLookupError::unavailable(
                "checking a live grant",
            ))),
        );
        assert!(unavailable.authenticate(&username, &key).await.is_err());
    }

    #[tokio::test]
    async fn session_rechecks_preserve_the_admission_instant() {
        let admitted_at = Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let later = admitted_at + chrono::Duration::hours(1);
        let now = Arc::new(Mutex::new(admitted_at));
        let grant_id = Uuid::from_u128(41);
        let routes = WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([0xb1; KEY_LEN]));
        let username = routes.seal("acme", grant_id).unwrap();
        let admissions = RecordingAdmissions::returning(Ok(Some(admission(grant_id))));
        let seen_sessions = admissions.sessions.clone();
        let clock = now.clone();
        let authenticator = WorkspaceSshAuthenticator::new(routes, admissions)
            .with_clock(move || *clock.lock().unwrap());
        let authenticated = authenticator
            .authenticate(&username, &public_key([0xb2; KEY_LEN]))
            .await
            .unwrap()
            .unwrap();

        *now.lock().unwrap() = later;
        assert!(authenticator
            .revalidate(&authenticated)
            .await
            .unwrap()
            .is_some());

        let sessions = seen_sessions.lock().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].admitted_at, admitted_at);
        assert_eq!(sessions[0].observed_at, later);
        assert_eq!(sessions[0].grant_id, grant_id);
    }

    #[test]
    fn authentication_types_never_debug_host_or_client_private_keys() {
        let host = WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0xb1; KEY_LEN]));
        let error = WorkspaceSshAuthenticationError::Admission(AdmissionLookupError::unavailable(
            "checking a live grant",
        ));
        assert!(!format!("{host:?} {error:?}").contains("b1b1b1b1"));
    }
}
