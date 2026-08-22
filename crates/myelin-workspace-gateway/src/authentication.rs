use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_identity_service::workspace_ssh_public_key_fingerprint;
use myelin_storage::agent_thread_durable::{
    LiveWorkspaceSshAdmission, WorkspaceSessionMode, WorkspaceSshRouteKey,
};
use russh::keys::ssh_key::{Algorithm, PublicKey};
use russh::keys::PublicKeyBase64;
use uuid::Uuid;

use crate::{
    AdmissionLookupError, LiveAdmissionRequest, LiveSessionRequest, WorkspaceSessionStartRequest,
    WorkspaceSshAdmissionStore,
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

struct SessionCredential<'a> {
    grant_id: Uuid,
    route_username: &'a str,
    public_key_fingerprint: String,
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
        let Some(public_key_fingerprint) = fingerprint(public_key) else {
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

    pub(crate) async fn revalidate(
        &self,
        authenticated: &AuthenticatedWorkspace,
    ) -> Result<Option<AuthenticatedWorkspace>, AdmissionLookupError> {
        let Some(credential) = session_credential(authenticated) else {
            return Ok(None);
        };
        let current = self
            .admissions
            .live_session(&LiveSessionRequest {
                tenant: authenticated.tenant.clone(),
                grant_id: credential.grant_id,
                route_username: credential.route_username.to_string(),
                public_key_fingerprint: credential.public_key_fingerprint,
                admitted_at: authenticated.admitted_at,
                observed_at: (self.clock)(),
            })
            .await?
            .map(|admission| AuthenticatedWorkspace {
                tenant: authenticated.tenant.clone(),
                admission,
                admitted_at: authenticated.admitted_at,
                credential: authenticated.credential.clone(),
            });
        Ok(current.filter(|current| same_workspace_authority(current, authenticated)))
    }

    pub(crate) async fn start_session(
        &self,
        authenticated: &AuthenticatedWorkspace,
        session_id: String,
        mode: WorkspaceSessionMode,
        terminal: bool,
    ) -> Result<Option<AuthenticatedWorkspace>, AdmissionLookupError> {
        let Some(credential) = session_credential(authenticated) else {
            return Ok(None);
        };
        let started_at = (self.clock)();
        let current = self
            .admissions
            .start_session(&WorkspaceSessionStartRequest {
                tenant: authenticated.tenant.clone(),
                session_id,
                grant_id: credential.grant_id,
                route_username: credential.route_username.to_string(),
                public_key_fingerprint: credential.public_key_fingerprint,
                admitted_at: authenticated.admitted_at,
                started_at,
                mode,
                terminal,
            })
            .await?
            .map(|started| AuthenticatedWorkspace {
                tenant: authenticated.tenant.clone(),
                admission: started.admission,
                admitted_at: authenticated.admitted_at,
                credential: authenticated.credential.clone(),
            });
        Ok(current.filter(|current| same_workspace_authority(current, authenticated)))
    }
}

fn session_credential(authenticated: &AuthenticatedWorkspace) -> Option<SessionCredential<'_>> {
    let credential = authenticated.credential.as_ref()?;
    Some(SessionCredential {
        grant_id: Uuid::parse_str(&authenticated.admission.grant_id).ok()?,
        route_username: &credential.route_username,
        public_key_fingerprint: fingerprint(&credential.public_key)?,
    })
}

fn fingerprint(public_key: &PublicKey) -> Option<String> {
    let authorized_key = format!("ssh-ed25519 {}", public_key.public_key_base64());
    workspace_ssh_public_key_fingerprint(&authorized_key).ok()
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

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use myelin_identity_service::WorkspaceSshHostIdentity;
    use myelin_storage::{DurableWorkspaceSession, SealKey, StartedWorkspaceSshSession, KEY_LEN};
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    use russh::keys::PrivateKey;

    use super::*;

    #[derive(Clone)]
    struct RecordingAdmissions {
        admissions: Arc<Mutex<Vec<LiveAdmissionRequest>>>,
        sessions: Arc<Mutex<Vec<LiveSessionRequest>>>,
        starts: Arc<Mutex<Vec<WorkspaceSessionStartRequest>>>,
        result: Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>,
    }

    impl RecordingAdmissions {
        fn returning(
            result: Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>,
        ) -> Self {
            Self {
                admissions: Arc::new(Mutex::new(Vec::new())),
                sessions: Arc::new(Mutex::new(Vec::new())),
                starts: Arc::new(Mutex::new(Vec::new())),
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

        fn start_session<'a>(
            &'a self,
            request: &'a WorkspaceSessionStartRequest,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<Option<StartedWorkspaceSshSession>, AdmissionLookupError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.starts.lock().unwrap().push(request.clone());
                self.result.clone().map(|result| {
                    result.map(|admission| StartedWorkspaceSshSession {
                        session: DurableWorkspaceSession {
                            session_id: request.session_id.clone(),
                            thread_id: admission.thread_id.clone(),
                            owner_principal_id: admission.owner_principal_id.clone(),
                            workspace_id: admission.workspace_id.clone(),
                            workspace_generation: admission.workspace_generation,
                            access_method: "ssh".into(),
                            mode: request.mode,
                            terminal: request.terminal,
                            started_at: request.started_at.to_rfc3339(),
                        },
                        admission,
                    })
                })
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

    #[tokio::test]
    async fn starting_a_confined_session_records_only_its_minimized_shape() {
        let admitted_at = Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap();
        let started_at = admitted_at + chrono::Duration::seconds(1);
        let now = Arc::new(Mutex::new(admitted_at));
        let grant_id = Uuid::from_u128(51);
        let routes = WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([0xc1; KEY_LEN]));
        let username = routes.seal("acme", grant_id).unwrap();
        let admissions = RecordingAdmissions::returning(Ok(Some(admission(grant_id))));
        let starts = admissions.starts.clone();
        let clock = now.clone();
        let authenticator = WorkspaceSshAuthenticator::new(routes, admissions)
            .with_clock(move || *clock.lock().unwrap());
        let authenticated = authenticator
            .authenticate(&username, &public_key([0xc2; KEY_LEN]))
            .await
            .unwrap()
            .unwrap();

        *now.lock().unwrap() = started_at;
        assert!(authenticator
            .start_session(
                &authenticated,
                "01J00000000000000000000001".into(),
                WorkspaceSessionMode::Command,
                true,
            )
            .await
            .unwrap()
            .is_some());

        let starts = starts.lock().unwrap();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].tenant, "acme");
        assert_eq!(starts[0].grant_id, grant_id);
        assert_eq!(starts[0].admitted_at, admitted_at);
        assert_eq!(starts[0].started_at, started_at);
        assert_eq!(starts[0].mode, WorkspaceSessionMode::Command);
        assert!(starts[0].terminal);
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
