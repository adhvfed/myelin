use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use myelin_storage::agent_thread_durable::{DurableAgentThreadBacking, LiveWorkspaceSshAdmission};
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
}
