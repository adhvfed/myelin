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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn create(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "myelin-cli-{label}-{}-{}",
                std::process::id(),
                new_reference()
            ));
            std::fs::create_dir_all(&path).expect("the test directory should be creatable");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key| values.get(key).cloned()
    }

    #[test]
    fn references_are_opaque_canonical_and_fresh() {
        let first = new_reference();
        let second = new_reference();

        validate_reference(&first).unwrap();
        validate_reference(&second).unwrap();
        assert_eq!(first.len(), REFERENCE_LENGTH);
        assert_ne!(first, second);
        assert!(!first.contains('='), "references use unpadded base64url");
    }

    #[test]
    fn malformed_references_never_reach_a_secret_backend() {
        for malformed in [
            "",
            "too-short",
            "abcdefghijklmnopqrstu/",
            "abcdefghijklmnopqrstu=",
            "abcdefghijklmnopqrstuvv",
        ] {
            let error = validate_reference(malformed).unwrap_err();
            assert!(error.to_string().contains("malformed"));
        }
    }

    #[test]
    fn the_os_keyring_is_the_only_default_secret_store() {
        assert!(matches!(
            CredentialSecretStore::selected(Path::new("/unused"), &env_from(&[])).unwrap(),
            CredentialSecretStore::Keyring
        ));
        assert!(matches!(
            CredentialSecretStore::selected(
                Path::new("/unused"),
                &env_from(&[(TEST_STORE_ENV, "")])
            )
            .unwrap(),
            CredentialSecretStore::Keyring
        ));

        let error = CredentialSecretStore::selected(
            Path::new("/unused"),
            &env_from(&[(TEST_STORE_ENV, "anything-else")]),
        )
        .err()
        .expect("unknown test stores must be rejected");
        assert!(error.to_string().contains("debug-only test seam"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn the_test_store_exercises_the_secret_lifecycle_without_a_real_keyring() {
        let directory = TestDirectory::create("credential-lifecycle");
        let config_directory = directory.path().join("config");
        let store = CredentialSecretStore::selected(
            &config_directory,
            &env_from(&[(TEST_STORE_ENV, TEST_STORE_VALUE)]),
        )
        .unwrap();
        let reference = new_reference();

        let missing = store.get(&reference).unwrap_err();
        assert!(matches!(missing, CliError::NotAuthenticated(_)));

        store.put(&reference, "first secret").unwrap();
        assert_eq!(store.get(&reference).unwrap(), "first secret");
        store.put(&reference, "replacement secret").unwrap();
        assert_eq!(store.get(&reference).unwrap(), "replacement secret");

        let path = store.test_secret_path(&reference).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        assert!(store.delete(&reference).unwrap());
        assert!(!store.delete(&reference).unwrap());
        assert!(matches!(
            store.get(&reference).unwrap_err(),
            CliError::NotAuthenticated(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn protected_writes_replace_atomically_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::create("protected-write");
        let path = directory.path().join("credential");
        write_owner_only_atomic(&path, b"first").unwrap();
        write_owner_only_atomic(&path, b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1,
            "successful writes leave no temporary files behind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn protected_writes_fail_closed_when_every_temporary_name_is_taken() {
        let directory = TestDirectory::create("protected-write-exhausted");
        for attempt in 0..100 {
            std::fs::write(
                directory.path().join(format!(
                    ".myelin-credential.tmp-{}-{attempt}",
                    std::process::id()
                )),
                b"occupied",
            )
            .unwrap();
        }
        let destination = directory.path().join("credential");

        let error = write_owner_only_atomic(&destination, b"must not be written").unwrap_err();

        assert!(error.to_string().contains("cannot allocate"));
        assert!(!destination.exists());
    }
}
