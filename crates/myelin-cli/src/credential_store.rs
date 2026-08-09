use crate::error::CliError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use keyring::v1::{Entry, Error as KeyringError};
use rand::{rngs::OsRng, RngCore};
use std::path::Path;

#[cfg(debug_assertions)]
use std::path::PathBuf;

pub(crate) const TEST_STORE_ENV: &str = "MYELIN_TEST_CREDENTIAL_STORE";

const KEYRING_SERVICE: &str = "dev.myelin.cli";
#[cfg(debug_assertions)]
const TEST_STORE_VALUE: &str = "file";
const REFERENCE_BYTES: usize = 16;
const REFERENCE_LENGTH: usize = 22;

pub(crate) enum CredentialSecretStore {
    Keyring,
    #[cfg(debug_assertions)]
    TestFile {
        directory: PathBuf,
    },
}

impl CredentialSecretStore {
    pub(crate) fn selected(
        config_directory: &Path,
        getenv: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, CliError> {
        #[cfg(not(debug_assertions))]
        let _ = config_directory;

        match getenv(TEST_STORE_ENV).as_deref() {
            None | Some("") => Ok(Self::Keyring),
            #[cfg(debug_assertions)]
            Some(TEST_STORE_VALUE) => Ok(Self::TestFile {
                directory: config_directory.join(".test-credentials"),
            }),
            Some(_) => Err(CliError::Config(format!(
                "{TEST_STORE_ENV} is a debug-only test seam and must be unset"
            ))),
        }
    }

    pub(crate) fn put(&self, reference: &str, token: &str) -> Result<(), CliError> {
        validate_reference(reference)?;
        match self {
            Self::Keyring => keyring_entry(reference)?
                .set_password(token)
                .map_err(|_| keyring_unavailable()),
            #[cfg(debug_assertions)]
            Self::TestFile { directory } => {
                std::fs::create_dir_all(directory).map_err(|error| {
                    CliError::Config(format!(
                        "cannot create the test credential store {}: {error}",
                        directory.display()
                    ))
                })?;
                write_owner_only_atomic(&directory.join(reference), token.as_bytes())
            }
        }
    }

    pub(crate) fn get(&self, reference: &str) -> Result<String, CliError> {
        validate_reference(reference)?;
        match self {
            Self::Keyring => keyring_entry(reference)?.get_password().map_err(|error| {
                if matches!(error, KeyringError::NoEntry) {
                    missing_secret()
                } else {
                    keyring_unavailable()
                }
            }),
            #[cfg(debug_assertions)]
            Self::TestFile { directory } => std::fs::read_to_string(directory.join(reference))
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        missing_secret()
                    } else {
                        CliError::Config(format!(
                            "cannot read the test credential store {}: {error}",
                            directory.display()
                        ))
                    }
                }),
        }
    }

    pub(crate) fn delete(&self, reference: &str) -> Result<bool, CliError> {
        validate_reference(reference)?;
        match self {
            Self::Keyring => match keyring_entry(reference)?.delete_credential() {
                Ok(()) => Ok(true),
                Err(KeyringError::NoEntry) => Ok(false),
                Err(_) => Err(keyring_unavailable()),
            },
            #[cfg(debug_assertions)]
            Self::TestFile { directory } => match std::fs::remove_file(directory.join(reference)) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(CliError::Config(format!(
                    "cannot remove the test credential from {}: {error}",
                    directory.display()
                ))),
            },
        }
    }

    #[cfg(all(test, debug_assertions))]
    pub(crate) fn test_secret_path(&self, reference: &str) -> Option<PathBuf> {
        match self {
            Self::Keyring => None,
            Self::TestFile { directory } => Some(directory.join(reference)),
        }
    }
}

pub(crate) fn new_reference() -> String {
    let mut bytes = [0_u8; REFERENCE_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn validate_reference(reference: &str) -> Result<(), CliError> {
    if reference.len() != REFERENCE_LENGTH
        || !reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CliError::Config(
            "stored credential reference is malformed; run `myelin auth login` again".into(),
        ));
    }
    Ok(())
}

fn keyring_entry(reference: &str) -> Result<Entry, CliError> {
    Entry::new(KEYRING_SERVICE, reference).map_err(|_| keyring_unavailable())
}

fn keyring_unavailable() -> CliError {
    CliError::Config(
        "the OS credential store is unavailable; unlock or start the system keyring and try again"
            .into(),
    )
}

fn missing_secret() -> CliError {
    CliError::NotAuthenticated(
        "the saved credential is missing from the OS credential store".into(),
    )
}

#[cfg(unix)]
pub(crate) fn write_owner_only_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| CliError::Config("credential path has no parent directory".into()))?;
    let mut temporary = None;
    for attempt in 0..100 {
        let candidate = parent.join(format!(
            ".myelin-credential.tmp-{}-{attempt}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(CliError::Config(format!(
                    "cannot create protected file in {}: {error}",
                    parent.display()
                )))
            }
        }
    }
    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        CliError::Config(format!(
            "cannot allocate a temporary protected file in {}",
            parent.display()
        ))
    })?;
    let result = (|| {
        file.write_all(bytes).map_err(|error| {
            CliError::Config(format!(
                "cannot write protected file {}: {error}",
                temporary_path.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            CliError::Config(format!(
                "cannot sync protected file {}: {error}",
                temporary_path.display()
            ))
        })?;
        drop(file);
        std::fs::rename(&temporary_path, path).map_err(|error| {
            CliError::Config(format!(
                "cannot install protected file {}: {error}",
                path.display()
            ))
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(unix))]
pub(crate) fn write_owner_only_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(path, bytes).map_err(|error| {
        CliError::Config(format!(
            "cannot write protected file {}: {error}",
            path.display()
        ))
    })
}
