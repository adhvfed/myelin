use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use myelin_identity::AuthzError;
use myelin_storage::{SealKey, KEY_LEN};
use zeroize::Zeroizing;

use crate::ssh_fingerprint;

const HOST_KEY_CONTEXT: &str = "myelin 2026-08-22 workspace ssh host key v1";
const ED25519_KEY_TYPE: &[u8] = b"ssh-ed25519";

#[derive(Clone)]
pub struct WorkspaceSshHostIdentity {
    seed: Zeroizing<[u8; KEY_LEN]>,
    public_key_blob: Vec<u8>,
}

impl WorkspaceSshHostIdentity {
    pub fn from_seal_key(seal_key: &SealKey) -> WorkspaceSshHostIdentity {
        use ring::signature::{Ed25519KeyPair, KeyPair};

        let seed = seal_key.derive_service_key(HOST_KEY_CONTEXT);
        let pair = Ed25519KeyPair::from_seed_unchecked(seed.as_ref())
            .expect("a 32-byte derived seed is a valid Ed25519 host identity");
        WorkspaceSshHostIdentity {
            public_key_blob: encode_public_key_blob(pair.public_key().as_ref()),
            seed,
        }
    }

    pub fn public_key(&self) -> String {
        format!("ssh-ed25519 {}", B64.encode(&self.public_key_blob))
    }

    pub fn fingerprint(&self) -> String {
        ssh_fingerprint(&self.public_key_blob)
    }

    pub fn seed(&self) -> &[u8; KEY_LEN] {
        &self.seed
    }
}

impl std::fmt::Debug for WorkspaceSshHostIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceSshHostIdentity")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

pub fn workspace_ssh_public_key_fingerprint(authorized_key: &str) -> Result<String, AuthzError> {
    if authorized_key.is_empty()
        || authorized_key.len() > 4096
        || authorized_key.trim() != authorized_key
        || authorized_key.chars().any(char::is_control)
    {
        return Err(malformed(
            "workspace SSH public key must be one bounded OpenSSH line",
        ));
    }
    let mut fields = authorized_key.split_ascii_whitespace();
    if fields.next() != Some("ssh-ed25519") {
        return Err(malformed(
            "workspace SSH access accepts only ephemeral ssh-ed25519 keys",
        ));
    }
    let encoded = fields
        .next()
        .ok_or_else(|| malformed("workspace SSH public key has no key body"))?;
    let public_key_blob = B64
        .decode(encoded)
        .map_err(|_| malformed("workspace SSH public key body is not canonical base64"))?;
    validate_public_key_blob(&public_key_blob)?;
    Ok(ssh_fingerprint(&public_key_blob))
}

fn validate_public_key_blob(blob: &[u8]) -> Result<(), AuthzError> {
    let mut cursor = 0;
    let key_type = read_string(blob, &mut cursor)?;
    let public_key = read_string(blob, &mut cursor)?;
    if key_type != ED25519_KEY_TYPE || public_key.len() != 32 || cursor != blob.len() {
        return Err(malformed(
            "workspace SSH key is not a canonical Ed25519 public-key blob",
        ));
    }
    Ok(())
}

fn read_string<'a>(blob: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], AuthzError> {
    let length_end = cursor
        .checked_add(4)
        .ok_or_else(|| malformed("workspace SSH key length overflowed"))?;
    let length_bytes = blob
        .get(*cursor..length_end)
        .ok_or_else(|| malformed("workspace SSH key is truncated"))?;
    let length = u32::from_be_bytes(
        length_bytes
            .try_into()
            .expect("the checked SSH length prefix is four bytes"),
    ) as usize;
    let value_end = length_end
        .checked_add(length)
        .ok_or_else(|| malformed("workspace SSH key field overflowed"))?;
    let value = blob
        .get(length_end..value_end)
        .ok_or_else(|| malformed("workspace SSH key field is truncated"))?;
    *cursor = value_end;
    Ok(value)
}

fn encode_public_key_blob(public_key: &[u8]) -> Vec<u8> {
    fn push_string(target: &mut Vec<u8>, value: &[u8]) {
        let length = u32::try_from(value.len()).expect("an Ed25519 SSH field is tiny");
        target.extend_from_slice(&length.to_be_bytes());
        target.extend_from_slice(value);
    }

    let mut blob = Vec::with_capacity(4 + ED25519_KEY_TYPE.len() + 4 + public_key.len());
    push_string(&mut blob, ED25519_KEY_TYPE);
    push_string(&mut blob, public_key);
    blob
}

fn malformed(message: impl Into<String>) -> AuthzError {
    AuthzError::BadRequest(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_ephemeral_ed25519_public_key_line_is_accepted() {
        let identity =
            WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x51; KEY_LEN]));
        let authorized = format!("{} myelin-one-shot", identity.public_key());
        assert_eq!(
            workspace_ssh_public_key_fingerprint(&authorized).unwrap(),
            identity.fingerprint()
        );

        for malformed in [
            "ssh-rsa AAAA",
            "ssh-ed25519",
            " ssh-ed25519 AAAA",
            "ssh-ed25519 !!!",
            "ssh-ed25519 AAAA\nsecond-line",
        ] {
            assert!(workspace_ssh_public_key_fingerprint(malformed).is_err());
        }
    }

    #[test]
    fn host_identity_is_stable_pinned_and_redacted() {
        let first = WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x61; KEY_LEN]));
        let again = WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x61; KEY_LEN]));
        let another =
            WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x62; KEY_LEN]));

        assert_eq!(first.public_key(), again.public_key());
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_ne!(first.fingerprint(), another.fingerprint());
        assert_eq!(
            workspace_ssh_public_key_fingerprint(&first.public_key()).unwrap(),
            first.fingerprint()
        );
        assert!(!format!("{first:?}").contains(&B64.encode(first.seed())));
    }
}
