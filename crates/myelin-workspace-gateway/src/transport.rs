use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_identity_service::workspace_ssh_public_key_fingerprint;
use myelin_storage::agent_thread_durable::{LiveWorkspaceSshAdmission, WorkspaceSshRouteKey};
use russh::keys::ssh_key::{Algorithm, PublicKey};
use russh::keys::PublicKeyBase64;
use russh::server::{Auth, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId};

use crate::{AdmissionLookupError, LiveAdmissionRequest, WorkspaceSshAdmissionStore};

type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedWorkspace {
    pub tenant: String,
    pub admission: LiveWorkspaceSshAdmission,
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
        Ok(self
            .admissions
            .live_admission(&request)
            .await?
            .map(|admission| AuthenticatedWorkspace {
                tenant: route.tenant,
                admission,
            }))
    }
}

#[derive(Clone)]
pub struct WorkspaceSshGateway<A> {
    authenticator: WorkspaceSshAuthenticator<A>,
}

impl<A> WorkspaceSshGateway<A>
where
    A: WorkspaceSshAdmissionStore,
{
    pub fn new(authenticator: WorkspaceSshAuthenticator<A>) -> Self {
        Self { authenticator }
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
            authenticated: None,
        }
    }
}

pub struct WorkspaceSshConnection<A> {
    authenticator: WorkspaceSshAuthenticator<A>,
    authenticated: Option<AuthenticatedWorkspace>,
}

impl<A> WorkspaceSshConnection<A>
where
    A: WorkspaceSshAdmissionStore,
{
    pub fn authenticated_workspace(&self) -> Option<&AuthenticatedWorkspace> {
        self.authenticated.as_ref()
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
        _channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.authenticated.is_some() {
            reply.accept().await;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }
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
        seen: Arc<Mutex<Vec<LiveAdmissionRequest>>>,
        result: Result<Option<LiveWorkspaceSshAdmission>, AdmissionLookupError>,
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
                self.seen.lock().unwrap().push(request.clone());
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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let authenticator = WorkspaceSshAuthenticator::new(
            routes,
            RecordingAdmissions {
                seen: seen.clone(),
                result: Ok(Some(admission(grant_id))),
            },
        )
        .with_clock(move || now);

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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let authenticator = WorkspaceSshAuthenticator::new(
            WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([0x91; KEY_LEN])),
            RecordingAdmissions {
                seen: seen.clone(),
                result: Ok(Some(admission(Uuid::from_u128(21)))),
            },
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
            RecordingAdmissions {
                seen: Arc::new(Mutex::new(Vec::new())),
                result: Ok(None),
            },
        );
        assert!(expired
            .authenticate(&username, &key)
            .await
            .unwrap()
            .is_none());

        let unavailable = WorkspaceSshAuthenticator::new(
            routes,
            RecordingAdmissions {
                seen: Arc::new(Mutex::new(Vec::new())),
                result: Err(AdmissionLookupError::unavailable("checking a live grant")),
            },
        );
        assert!(unavailable.authenticate(&username, &key).await.is_err());
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
