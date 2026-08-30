use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use myelin_storage::agent_thread_durable::{
    DurableAgentThreadBacking, LiveWorkspaceSshAdmission, NewWorkspaceSshSession,
    ReservedWorkspaceSshSession, WorkspaceSessionMode,
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveAdmissionRequest {
    pub tenant: String,
    pub grant_id: Uuid,
    pub route_username: String,
    pub public_key_fingerprint: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSessionRequest {
    pub tenant: String,
    pub grant_id: Uuid,
    pub route_username: String,
    pub public_key_fingerprint: String,
    pub admitted_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSessionStartRequest {
    pub tenant: String,
    pub session_id: String,
    pub grant_id: Uuid,
    pub route_username: String,
    pub public_key_fingerprint: String,
    pub admitted_at: DateTime<Utc>,
    pub reserved_at: DateTime<Utc>,
    pub mode: WorkspaceSessionMode,
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSessionConfirmRequest {
    pub tenant: String,
    pub grant_id: Uuid,
    pub session_id: String,
    pub launched_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionLookupError {
    operation: &'static str,
}

impl AdmissionLookupError {
    pub fn unavailable(operation: &'static str) -> Self {
        Self { operation }
    }
}

impl core::fmt::Display for AdmissionLookupError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "workspace SSH admission store unavailable while {}",
            self.operation
        )
    }
}

impl std::error::Error for AdmissionLookupError {}

pub trait WorkspaceSshAdmissionStore: Clone + Send + Sync + 'static {
    fn live_admission<'a>(
        &'a self,
        request: &'a LiveAdmissionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>>
                + Send
                + 'a,
        >,
    >;

    fn live_session<'a>(
        &'a self,
        request: &'a LiveSessionRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>>
                + Send
                + 'a,
        >,
    >;

    fn reserve_session<'a>(
        &'a self,
        request: &'a WorkspaceSessionStartRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<ReservedWorkspaceSshSession>, AdmissionLookupError>>
                + Send
                + 'a,
        >,
    >;

    fn confirm_session<'a>(
        &'a self,
        request: &'a WorkspaceSessionConfirmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AdmissionLookupError>> + Send + 'a>>;
}

impl WorkspaceSshAdmissionStore for DurableAgentThreadBacking {
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
            self.live_ssh_admission(
                &request.tenant,
                request.grant_id,
                &request.route_username,
                &request.public_key_fingerprint,
                request.observed_at,
            )
            .await
            .map_err(|_| AdmissionLookupError::unavailable("checking a live grant"))
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
            self.live_ssh_session(
                &request.tenant,
                request.grant_id,
                &request.route_username,
                &request.public_key_fingerprint,
                request.admitted_at,
                request.observed_at,
            )
            .await
            .map_err(|_| AdmissionLookupError::unavailable("rechecking a live session"))
        })
    }

    fn reserve_session<'a>(
        &'a self,
        request: &'a WorkspaceSessionStartRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<ReservedWorkspaceSshSession>, AdmissionLookupError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.reserve_ssh_session(
                &request.tenant,
                NewWorkspaceSshSession {
                    session_id: request.session_id.clone(),
                    grant_id: request.grant_id,
                    route_username: request.route_username.clone(),
                    public_key_fingerprint: request.public_key_fingerprint.clone(),
                    admitted_at: request.admitted_at,
                    reserved_at: request.reserved_at,
                    mode: request.mode,
                    terminal: request.terminal,
                },
            )
            .await
            .map_err(|_| AdmissionLookupError::unavailable("reserving a live session"))
        })
    }

    fn confirm_session<'a>(
        &'a self,
        request: &'a WorkspaceSessionConfirmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AdmissionLookupError>> + Send + 'a>> {
        Box::pin(async move {
            self.confirm_ssh_session_started(
                &request.tenant,
                request.grant_id,
                &request.session_id,
                request.launched_at,
            )
            .await
            .map(|session| session.is_some())
            .map_err(|_| AdmissionLookupError::unavailable("publishing a live session"))
        })
    }
}
