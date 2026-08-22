use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::access::{known_host_name, unsafe_access, WorkspaceSshAccess};
use super::process::{exit_description, isolated_command, missing_openssh};
use crate::error::CliError;

const MAX_PUBLIC_KEY_BYTES: usize = 4 * 1024;

pub(super) struct EphemeralSshKey {
    directory: TempDir,
    private_key: PathBuf,
    public_key: String,
    pub(super) fingerprint: String,
}

impl EphemeralSshKey {
    pub(super) fn generate() -> Result<Self, CliError> {
        let directory = tempfile::Builder::new()
            .prefix("myelin-workspace-ssh-")
            .tempdir()
            .map_err(|error| {
                CliError::Config(format!(
                    "could not create temporary SSH key directory: {error}"
                ))
            })?;
        let private_key = directory.path().join("id_ed25519");
        let output = isolated_command("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "myelin-one-shot",
                "-f",
            ])
            .arg(&private_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| missing_openssh("ssh-keygen", error))?;
        if !output.status.success() {
            return Err(CliError::Unsupported(format!(
                "ssh-keygen could not create a one-shot Ed25519 key ({})",
                exit_description(output.status)
            )));
        }
        let encoded = fs::read(private_key.with_extension("pub")).map_err(|error| {
            CliError::Config(format!(
                "could not read the temporary SSH public key: {error}"
            ))
        })?;
        if encoded.len() > MAX_PUBLIC_KEY_BYTES {
            return Err(CliError::Config(
                "ssh-keygen returned an unexpectedly large public key".into(),
            ));
        }
        let public_key = String::from_utf8(encoded)
            .map_err(|_| CliError::Config("ssh-keygen returned a non-UTF-8 public key".into()))?;
        let public_key = public_key.trim_end_matches(['\r', '\n']).to_string();
        let fingerprint = ed25519_fingerprint(&public_key, true)?;
        Ok(Self {
            directory,
            private_key,
            public_key,
            fingerprint,
        })
    }

    pub(super) fn public_key(&self) -> &str {
        &self.public_key
    }

    pub(super) fn private_key(&self) -> &Path {
        &self.private_key
    }

    pub(super) fn pin_host(&self, access: &WorkspaceSshAccess) -> Result<PathBuf, CliError> {
        let known_hosts = self.directory.path().join("known_hosts");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&known_hosts).map_err(|error| {
            CliError::Config(format!(
                "could not create the temporary known-hosts file: {error}"
            ))
        })?;
        writeln!(
            file,
            "{} {}",
            known_host_name(&access.host, access.port),
            access.host_public_key
        )
        .map_err(|error| {
            CliError::Config(format!("could not pin the workspace SSH host key: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            CliError::Config(format!(
                "could not persist the workspace SSH host pin: {error}"
            ))
        })?;
        Ok(known_hosts)
    }
}

pub(super) fn ed25519_fingerprint(
    public_key: &str,
    allow_comment: bool,
) -> Result<String, CliError> {
    if public_key.is_empty()
        || public_key.len() > MAX_PUBLIC_KEY_BYTES
        || public_key.trim() != public_key
        || public_key.chars().any(char::is_control)
    {
        return Err(unsafe_access("SSH public key is not one bounded line"));
    }
    let fields = public_key.split_ascii_whitespace().collect::<Vec<_>>();
    let expected_fields = if allow_comment { 2..=3 } else { 2..=2 };
    if !expected_fields.contains(&fields.len()) || fields[0] != "ssh-ed25519" {
        return Err(unsafe_access("SSH public key is not Ed25519"));
    }
    let blob = STANDARD
        .decode(fields[1])
        .map_err(|_| unsafe_access("SSH public key is not canonical base64"))?;
    if STANDARD.encode(&blob) != fields[1] || !canonical_ed25519_blob(&blob) {
        return Err(unsafe_access(
            "SSH public key blob is not canonical Ed25519",
        ));
    }
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(&blob))
    ))
}

fn canonical_ed25519_blob(blob: &[u8]) -> bool {
    let mut cursor = 0;
    read_ssh_string(blob, &mut cursor).is_some_and(|value| value == b"ssh-ed25519")
        && read_ssh_string(blob, &mut cursor).is_some_and(|value| value.len() == 32)
        && cursor == blob.len()
}

fn read_ssh_string<'a>(blob: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let length_end = cursor.checked_add(4)?;
    let length = u32::from_be_bytes(blob.get(*cursor..length_end)?.try_into().ok()?) as usize;
    let value_end = length_end.checked_add(length)?;
    let value = blob.get(length_end..value_end)?;
    *cursor = value_end;
    Some(value)
}

#[cfg(test)]
mod tests {
    use myelin_identity_service::WorkspaceSshHostIdentity;
    use myelin_storage::{SealKey, KEY_LEN};

    use super::*;

    #[test]
    fn fingerprints_match_the_server_for_canonical_ed25519_keys() {
        let host = WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x71; KEY_LEN]));
        assert_eq!(
            ed25519_fingerprint(&host.public_key(), false).unwrap(),
            host.fingerprint()
        );
        assert_eq!(
            ed25519_fingerprint(&format!("{} myelin-one-shot", host.public_key()), true).unwrap(),
            host.fingerprint()
        );
        for malformed in [
            "ssh-rsa AAAA",
            "ssh-ed25519",
            " ssh-ed25519 AAAA",
            "ssh-ed25519 !!!",
            "ssh-ed25519 AAAA\nsecond-line",
        ] {
            assert!(ed25519_fingerprint(malformed, false).is_err());
        }
    }
}
