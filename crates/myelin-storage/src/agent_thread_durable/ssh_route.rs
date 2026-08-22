use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sqlx::types::Uuid;
use zeroize::Zeroizing;

use crate::{SealKey, KEY_LEN, NONCE_LEN};

const ROUTE_PREFIX: &str = "ws1_";
const ROUTE_AAD: &[u8] = b"myelin workspace ssh route v1";
const ROUTE_KEY_CONTEXT: &str = "myelin 2026-08-22 workspace ssh route v1";
const MAX_ROUTED_TENANT_BYTES: usize = 128;

#[derive(Clone)]
pub struct WorkspaceSshRouteKey {
    key: Zeroizing<[u8; KEY_LEN]>,
}

impl WorkspaceSshRouteKey {
    pub fn from_seal_key(seal_key: &SealKey) -> Self {
        Self {
            key: seal_key.derive_service_key(ROUTE_KEY_CONTEXT),
        }
    }

    pub fn seal(&self, tenant: &str, grant_id: Uuid) -> Result<String, WorkspaceSshRouteError> {
        if tenant.is_empty()
            || tenant.len() > MAX_ROUTED_TENANT_BYTES
            || tenant.chars().any(char::is_control)
        {
            return Err(WorkspaceSshRouteError);
        }
        let tenant_len = u8::try_from(tenant.len()).map_err(|_| WorkspaceSshRouteError)?;
        let mut plaintext = Zeroizing::new(Vec::with_capacity(2 + tenant.len() + 16));
        plaintext.push(1);
        plaintext.push(tenant_len);
        plaintext.extend_from_slice(tenant.as_bytes());
        plaintext.extend_from_slice(grant_id.as_bytes());

        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref())
            .expect("a derived workspace SSH route key is exactly 32 bytes");
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_slice(),
                    aad: ROUTE_AAD,
                },
            )
            .map_err(|_| WorkspaceSshRouteError)?;
        let mut encoded = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        encoded.extend_from_slice(&nonce);
        encoded.extend_from_slice(&ciphertext);
        Ok(format!("{ROUTE_PREFIX}{}", URL_SAFE_NO_PAD.encode(encoded)))
    }

    pub fn open(&self, username: &str) -> Result<WorkspaceSshRoute, WorkspaceSshRouteError> {
        let encoded = username
            .strip_prefix(ROUTE_PREFIX)
            .ok_or(WorkspaceSshRouteError)?;
        let sealed = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| WorkspaceSshRouteError)?,
        );
        let (nonce, ciphertext) = sealed
            .split_at_checked(NONCE_LEN)
            .ok_or(WorkspaceSshRouteError)?;
        let cipher = Aes256Gcm::new_from_slice(self.key.as_ref())
            .expect("a derived workspace SSH route key is exactly 32 bytes");
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(nonce),
                    Payload {
                        msg: ciphertext,
                        aad: ROUTE_AAD,
                    },
                )
                .map_err(|_| WorkspaceSshRouteError)?,
        );
        let version = plaintext.first().copied().ok_or(WorkspaceSshRouteError)?;
        let tenant_len = usize::from(*plaintext.get(1).ok_or(WorkspaceSshRouteError)?);
        if version != 1 || tenant_len == 0 || tenant_len > MAX_ROUTED_TENANT_BYTES {
            return Err(WorkspaceSshRouteError);
        }
        let tenant_end = 2usize
            .checked_add(tenant_len)
            .ok_or(WorkspaceSshRouteError)?;
        let expected_len = tenant_end.checked_add(16).ok_or(WorkspaceSshRouteError)?;
        if plaintext.len() != expected_len {
            return Err(WorkspaceSshRouteError);
        }
        let tenant = std::str::from_utf8(&plaintext[2..tenant_end])
            .map_err(|_| WorkspaceSshRouteError)?
            .to_string();
        let grant_id =
            Uuid::from_slice(&plaintext[tenant_end..]).map_err(|_| WorkspaceSshRouteError)?;
        Ok(WorkspaceSshRoute { tenant, grant_id })
    }
}

impl std::fmt::Debug for WorkspaceSshRouteKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WorkspaceSshRouteKey(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSshRoute {
    pub tenant: String,
    pub grant_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceSshRouteError;

impl std::fmt::Display for WorkspaceSshRouteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("workspace SSH route is invalid")
    }
}

impl std::error::Error for WorkspaceSshRouteError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_claim_is_opaque_round_trippable_and_domain_keyed() {
        let route_key = WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([7; KEY_LEN]));
        let grant_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let username = route_key.seal("acme", grant_id).unwrap();

        assert!(username.starts_with(ROUTE_PREFIX));
        assert!(!username.contains("acme"));
        assert!(!username.contains(&grant_id.to_string()));
        assert_eq!(
            route_key.open(&username).unwrap(),
            WorkspaceSshRoute {
                tenant: "acme".into(),
                grant_id,
            }
        );

        let wrong_key = WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([8; KEY_LEN]));
        assert_eq!(wrong_key.open(&username), Err(WorkspaceSshRouteError));
    }

    #[test]
    fn routing_claim_refuses_tampering_and_unbounded_tenants() {
        let route_key = WorkspaceSshRouteKey::from_seal_key(&SealKey::from_bytes([7; KEY_LEN]));
        let grant_id = Uuid::from_u128(1);
        let mut username = route_key.seal("acme", grant_id).unwrap();
        username.push('A');
        assert_eq!(route_key.open(&username), Err(WorkspaceSshRouteError));
        assert_eq!(
            route_key.seal(&"x".repeat(129), grant_id),
            Err(WorkspaceSshRouteError)
        );
    }
}
