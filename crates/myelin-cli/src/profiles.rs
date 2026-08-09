use crate::config::{config_dir, env};
use crate::credential_store::{validate_reference, write_owner_only_atomic};
use crate::error::CliError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_PROFILE: &str = "default";

const CONFIG_VERSION: u8 = 1;
const MAX_CONFIG_BYTES: usize = 256 * 1024;
const MAX_PROFILE_NAME_BYTES: usize = 64;
const MAX_CONTEXT_VALUE_BYTES: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub credential_ref: String,
    pub scheme: String,
    pub edge_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    profiles: BTreeMap<String, Profile>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            active_profile: None,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileCatalog {
    path: PathBuf,
    file: ConfigFile,
}

impl ProfileCatalog {
    pub(crate) fn load(
        getenv: &dyn Fn(&str) -> Option<String>,
        read_file: &dyn Fn(&Path) -> Option<String>,
    ) -> Result<Self, CliError> {
        let path = config_dir(getenv)?.join("config.toml");
        let Some(encoded) = read_file(&path) else {
            return Ok(Self {
                path,
                file: ConfigFile::default(),
            });
        };
        if encoded.len() > MAX_CONFIG_BYTES {
            return Err(CliError::Config(format!(
                "CLI config {} exceeds the byte limit",
                path.display()
            )));
        }
        let file: ConfigFile = toml::from_str(&encoded).map_err(|_| malformed_config(&path))?;
        let catalog = Self { path, file };
        catalog.validate()?;
        Ok(catalog)
    }

    pub(crate) fn selected_name(
        &self,
        explicit: Option<&str>,
        getenv: &dyn Fn(&str) -> Option<String>,
    ) -> Result<String, CliError> {
        let name = explicit
            .map(str::to_string)
            .or_else(|| getenv(env::PROFILE).filter(|value| !value.is_empty()))
            .or_else(|| self.file.active_profile.clone())
            .unwrap_or_else(|| DEFAULT_PROFILE.into());
        validate_profile_name(&name)?;
        Ok(name)
    }

    pub(crate) fn active_name(&self) -> Option<&str> {
        self.file.active_profile.as_deref()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Profile> {
        self.file.profiles.get(name)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Profile)> {
        self.file
            .profiles
            .iter()
            .map(|(name, profile)| (name.as_str(), profile))
    }

    pub(crate) fn upsert(&mut self, name: String, profile: Profile) -> Result<(), CliError> {
        validate_profile_name(&name)?;
        validate_profile(&profile)?;
        self.file.profiles.insert(name.clone(), profile);
        self.file.active_profile = Some(name);
        Ok(())
    }

    pub(crate) fn activate(&mut self, name: &str) -> Result<(), CliError> {
        validate_profile_name(name)?;
        if !self.file.profiles.contains_key(name) {
            return Err(CliError::NotAuthenticated(format!(
                "profile `{name}` is not signed in; run `myelin --profile {name} auth login`"
            )));
        }
        self.file.active_profile = Some(name.to_string());
        Ok(())
    }

    pub(crate) fn set_project(&mut self, name: &str, project: &str) -> Result<(), CliError> {
        validate_profile_name(name)?;
        validate_context_value("project", project).map_err(CliError::Usage)?;
        let profile = self.file.profiles.get_mut(name).ok_or_else(|| {
            CliError::NotAuthenticated(format!(
                "profile `{name}` is not signed in; run `myelin --profile {name} auth login`"
            ))
        })?;
        profile.project = Some(project.to_string());
        Ok(())
    }

    pub(crate) fn remove(&mut self, name: &str) -> Option<Profile> {
        let removed = self.file.profiles.remove(name);
        if removed.is_some() && self.file.active_profile.as_deref() == Some(name) {
            self.file.active_profile = self.file.profiles.keys().next().cloned();
        }
        removed
    }

    pub(crate) fn save(&self) -> Result<(), CliError> {
        if self.file.profiles.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(CliError::Config(format!(
                    "cannot remove CLI config {}: {error}",
                    self.path.display()
                ))),
            };
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| CliError::Config("CLI config path has no parent directory".into()))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            CliError::Config(format!(
                "cannot create config directory {}: {error}",
                parent.display()
            ))
        })?;
        let mut encoded = toml::to_string_pretty(&self.file)
            .map_err(|error| CliError::Config(format!("cannot encode CLI config: {error}")))?;
        encoded.push('\n');
        write_owner_only_atomic(&self.path, encoded.as_bytes())
    }

    fn validate(&self) -> Result<(), CliError> {
        if self.file.version != CONFIG_VERSION {
            return Err(CliError::Config(format!(
                "CLI config {} has unsupported version {}",
                self.path.display(),
                self.file.version
            )));
        }
        for (name, profile) in &self.file.profiles {
            if !canonical_profile_name(name) {
                return Err(malformed_config(&self.path));
            }
            validate_profile(profile)?;
        }
        match self.file.active_profile.as_deref() {
            Some(active) if !self.file.profiles.contains_key(active) => {
                Err(CliError::Config(format!(
                    "CLI config {} selects missing profile `{active}`",
                    self.path.display()
                )))
            }
            None if !self.file.profiles.is_empty() => Err(CliError::Config(format!(
                "CLI config {} has profiles but no active profile",
                self.path.display()
            ))),
            Some(active) if canonical_profile_name(active) => Ok(()),
            Some(_) => Err(malformed_config(&self.path)),
            None => Ok(()),
        }
    }
}

pub(crate) fn validate_profile_name(name: &str) -> Result<(), CliError> {
    if !canonical_profile_name(name) {
        return Err(CliError::Usage(
            "profile names must start with a letter or number and contain only letters, numbers, '.', '-', or '_'"
                .into(),
        ));
    }
    Ok(())
}

fn canonical_profile_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_PROFILE_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn validate_profile(profile: &Profile) -> Result<(), CliError> {
    validate_reference(&profile.credential_ref)?;
    for (label, value, maximum) in [
        ("scheme", Some(profile.scheme.as_str()), 32),
        ("Edge URL", Some(profile.edge_url.as_str()), 2_048),
        ("tenant", profile.tenant.as_deref(), MAX_CONTEXT_VALUE_BYTES),
        ("region", profile.region.as_deref(), MAX_CONTEXT_VALUE_BYTES),
        (
            "project",
            profile.project.as_deref(),
            MAX_CONTEXT_VALUE_BYTES,
        ),
    ] {
        if let Some(value) = value {
            validate_bounded_context_value(label, value, maximum).map_err(CliError::Config)?;
        }
    }
    if profile
        .expires_at_unix
        .is_some_and(|expires_at| expires_at <= 0)
    {
        return Err(CliError::Config(
            "saved profile expiry must be a positive Unix timestamp".into(),
        ));
    }
    Ok(())
}

fn validate_context_value(label: &str, value: &str) -> Result<(), String> {
    validate_bounded_context_value(label, value, MAX_CONTEXT_VALUE_BYTES)
}

fn validate_bounded_context_value(label: &str, value: &str, maximum: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > maximum
        || !value.as_bytes().iter().all(u8::is_ascii_graphic)
    {
        return Err(format!(
            "{label} must be bounded printable ASCII without spaces"
        ));
    }
    Ok(())
}

fn malformed_config(path: &Path) -> CliError {
    CliError::Config(format!(
        "CLI config {} is malformed; repair it or move it aside",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::new_reference;
    use std::collections::BTreeMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    fn profile(edge: &str) -> Profile {
        Profile {
            credential_ref: new_reference(),
            scheme: "session".into(),
            edge_url: edge.into(),
            expires_at_unix: Some(4_102_444_800),
            tenant: Some("acme".into()),
            region: Some("eu-north".into()),
            project: None,
        }
    }

    #[test]
    fn profiles_round_trip_in_stable_order_without_secrets() {
        let directory = std::env::temp_dir().join(format!(
            "myelin-cli-profiles-{}-{}",
            std::process::id(),
            new_reference()
        ));
        let encoded_directory = directory.to_string_lossy().to_string();
        let env = env_from(&[("MYELIN_CONFIG_DIR", &encoded_directory)]);
        let read = |path: &Path| std::fs::read_to_string(path).ok();
        let mut catalog = ProfileCatalog::load(&env, &read).unwrap();

        catalog
            .upsert("work".into(), profile("https://work.example"))
            .unwrap();
        catalog
            .upsert("personal".into(), profile("https://personal.example"))
            .unwrap();
        catalog.save().unwrap();

        let encoded = std::fs::read_to_string(directory.join("config.toml")).unwrap();
        assert!(
            encoded.find("[profiles.personal]").unwrap() < encoded.find("[profiles.work]").unwrap()
        );
        assert!(!encoded.contains("token"));
        let mut loaded = ProfileCatalog::load(&env, &read).unwrap();
        assert_eq!(loaded.active_name(), Some("personal"));
        assert_eq!(loaded.iter().count(), 2);
        loaded.activate("work").unwrap();
        loaded
            .set_project("work", "11111111-1111-1111-1111-111111111111")
            .unwrap();
        loaded.save().unwrap();
        let mut loaded = ProfileCatalog::load(&env, &read).unwrap();
        assert_eq!(loaded.active_name(), Some("work"));
        assert_eq!(
            loaded.get("work").unwrap().project.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        for malformed in ["", "two projects", &"x".repeat(257)] {
            assert!(
                loaded.set_project("work", malformed).is_err(),
                "{malformed:?}"
            );
        }

        loaded.remove("work");
        assert_eq!(loaded.active_name(), Some("personal"));
        loaded.remove("personal");
        loaded.save().unwrap();
        assert!(!directory.join("config.toml").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn explicit_then_environment_then_active_profile_controls_selection() {
        let mut catalog = ProfileCatalog {
            path: PathBuf::from("/unused/config.toml"),
            file: ConfigFile::default(),
        };
        catalog
            .upsert("active".into(), profile("https://edge.example"))
            .unwrap();

        let environment = env_from(&[("MYELIN_PROFILE", "environment")]);
        assert_eq!(
            catalog
                .selected_name(Some("explicit"), &environment)
                .unwrap(),
            "explicit"
        );
        assert_eq!(
            catalog.selected_name(None, &environment).unwrap(),
            "environment"
        );
        assert_eq!(
            catalog.selected_name(None, &env_from(&[])).unwrap(),
            "active"
        );
    }

    #[test]
    fn malformed_names_and_dangling_active_profiles_fail_loudly() {
        for name in ["", "two words", "/escape", &"x".repeat(65)] {
            assert!(validate_profile_name(name).is_err(), "{name:?}");
        }
        let encoded = format!(
            "version = 1\nactive_profile = \"missing\"\n\n[profiles.present]\ncredential_ref = \"{}\"\nscheme = \"session\"\nedge_url = \"https://edge.example\"\n",
            new_reference()
        );
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read =
            |path: &Path| (path == Path::new("/config/config.toml")).then(|| encoded.clone());
        assert!(ProfileCatalog::load(&env, &read)
            .unwrap_err()
            .to_string()
            .contains("missing profile"));
    }
}
