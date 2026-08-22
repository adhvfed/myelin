use std::sync::Arc;
use std::time::Duration;

use myelin_identity_service::WorkspaceSshHostIdentity;
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::PrivateKey;
use russh::{MethodKind, MethodSet};

const AUTH_REJECTION_DELAY: Duration = Duration::from_millis(750);
const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
pub struct HostKeyError(russh::keys::ssh_key::Error);

impl core::fmt::Display for HostKeyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("derived workspace SSH host key is invalid")
    }
}

impl std::error::Error for HostKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub fn workspace_ssh_server_config(
    identity: &WorkspaceSshHostIdentity,
) -> Result<Arc<russh::server::Config>, HostKeyError> {
    let keypair = Ed25519Keypair::from_seed(identity.seed());
    let host_key = PrivateKey::new(KeypairData::Ed25519(keypair), "myelin workspace gateway")
        .map_err(HostKeyError)?;
    Ok(Arc::new(russh::server::Config {
        methods: MethodSet::from(&[MethodKind::PublicKey][..]),
        auth_rejection_time: AUTH_REJECTION_DELAY,
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key],
        max_auth_attempts: 3,
        inactivity_timeout: Some(CONNECTION_IDLE_TIMEOUT),
        nodelay: true,
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use myelin_identity_service::workspace_ssh_public_key_fingerprint;
    use myelin_storage::{SealKey, KEY_LEN};
    use russh::keys::PublicKeyBase64;

    use super::*;

    #[test]
    fn configured_host_key_is_the_identity_advertised_to_clients() {
        let identity =
            WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x71; KEY_LEN]));
        let config = workspace_ssh_server_config(&identity).unwrap();

        assert_eq!(config.keys.len(), 1);
        assert_eq!(config.methods.as_ref(), &[MethodKind::PublicKey]);
        let actual = format!(
            "ssh-ed25519 {}",
            config.keys[0].public_key().public_key_base64()
        );
        assert_eq!(actual, identity.public_key());
        assert_eq!(
            workspace_ssh_public_key_fingerprint(&actual).unwrap(),
            identity.fingerprint()
        );
    }
}
