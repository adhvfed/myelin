use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::dispatch::OnTrigger;
use crate::resolve::{
    CiDefinition, CiPlanContract, JobDef, JobKind, StructuredBuildV1, VersionedCiDefinition,
};

pub const MAX_CI_CONFIG_BYTES: usize = 1024 * 1024;
pub const MAX_AUTHORED_JOBS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Json,
}

impl ConfigFormat {
    pub fn from_hint(hint: &str) -> Result<ConfigFormat, CiConfigError> {
        let lower = hint.to_ascii_lowercase();
        let tail = lower.rsplit(['.', '/']).next().unwrap_or(lower.as_str());
        match tail {
            "toml" => Ok(ConfigFormat::Toml),
            "json" => Ok(ConfigFormat::Json),
            "yml" | "yaml" => Err(CiConfigError::UnknownFormat {
                hint: hint.to_string(),
                detail: "YAML is not yet supported (CT-004b deferred it: no workspace YAML dep, \
                         serde_yaml is archived/unmaintained). Author `.myelin/ci.toml` or \
                         `.myelin/ci.json`."
                    .to_string(),
            }),
            _ => Err(CiConfigError::UnknownFormat {
                hint: hint.to_string(),
                detail: "unrecognised config format - supported: `.myelin/ci.toml`, \
                         `.myelin/ci.json` (YAML is deferred)."
                    .to_string(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiConfigError {
    ConfigTooLarge {
        actual: usize,
        maximum: usize,
    },
    UnknownFormat {
        hint: String,
        detail: String,
    },
    Syntax {
        format: &'static str,
        message: String,
    },
    UnknownField {
        format: &'static str,
        message: String,
    },
    UnknownTrigger {
        value: String,
    },
    BadJobKind {
        job: String,
        value: String,
    },
    EmptyJobs,
    TooManyJobs {
        actual: usize,
        maximum: usize,
    },
    EmptyJobName,
    DuplicateJob(String),
    UnknownNeed {
        job: String,
        need: String,
    },
    EmptyMatrixAxis {
        job: String,
        axis: String,
    },
    InvalidMachineToken {
        field: String,
        value: String,
    },
    InvalidImage {
        job: String,
    },
    InvalidCommand {
        job: String,
        detail: String,
    },
    InvalidBuild {
        job: String,
        detail: String,
    },
    TooManyMatrixAxes {
        job: String,
        actual: usize,
        maximum: usize,
    },
}

impl std::fmt::Display for CiConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiConfigError::ConfigTooLarge { actual, maximum } => write!(
                f,
                "the CI config is {actual} bytes, above the {maximum}-byte limit"
            ),
            CiConfigError::UnknownFormat { hint, detail } => {
                write!(f, "unsupported `.myelin/ci.*` format `{hint}`: {detail}")
            }
            CiConfigError::Syntax { format, message } => {
                write!(f, "malformed {format} config: {message}")
            }
            CiConfigError::UnknownField { format, message } => write!(
                f,
                "unknown field in the {format} config (fail-closed on typos/unsupported keys): \
                 {message}"
            ),
            CiConfigError::UnknownTrigger { value } => write!(
                f,
                "unknown `on:` trigger `{value}` - expected one of \
                 push/pull_request/issue/manual/schedule/agent"
            ),
            CiConfigError::BadJobKind { job, value } => write!(
                f,
                "job `{job}`: unknown kind `{value}` - expected `normal` or `generate` (the \
                 dynamic-generation escape hatch)"
            ),
            CiConfigError::EmptyJobs => {
                write!(
                    f,
                    "the CI config has no jobs - nothing to run (an empty pipeline is refused)"
                )
            }
            CiConfigError::TooManyJobs { actual, maximum } => write!(
                f,
                "the CI config declares {actual} jobs, above the {maximum}-job limit"
            ),
            CiConfigError::EmptyJobName => {
                write!(
                    f,
                    "a job has an empty name - a DAG node id must be a non-empty string"
                )
            }
            CiConfigError::DuplicateJob(name) => {
                write!(
                    f,
                    "duplicate job name `{name}` - DAG node ids must be unique"
                )
            }
            CiConfigError::UnknownNeed { job, need } => write!(
                f,
                "job `{job}` needs `{need}`, which is not a job in the config (dangling DAG edge)"
            ),
            CiConfigError::EmptyMatrixAxis { job, axis } => write!(
                f,
                "job `{job}`: matrix axis `{axis}` has no values - an empty axis is malformed \
                 (it would silently drop the axis at expansion)"
            ),
            CiConfigError::InvalidMachineToken { field, value } => {
                write!(f, "{field} `{value}` is not a bounded ASCII machine token")
            }
            CiConfigError::InvalidImage { job } => {
                write!(f, "job `{job}` image reference is empty or overlong")
            }
            CiConfigError::InvalidCommand { job, detail } => {
                write!(f, "job `{job}` command is invalid: {detail}")
            }
            CiConfigError::InvalidBuild { job, detail } => {
                write!(f, "job `{job}` structured build is invalid: {detail}")
            }
            CiConfigError::TooManyMatrixAxes {
                job,
                actual,
                maximum,
            } => write!(
                f,
                "job `{job}` declares {actual} matrix axes, above the {maximum}-axis limit"
            ),
        }
    }
}

impl std::error::Error for CiConfigError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionedCiConfigError {
    Legacy(CiConfigError),
    PartialExecutionContract,
    UnsupportedSchemaVersion { version: u32 },
    UnsupportedExecutionProfile { profile: String },
}

impl std::fmt::Display for VersionedCiConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Legacy(error) => error.fmt(f),
            Self::PartialExecutionContract => write!(f, "version-2 CI execution requests require both `schema_version = 2` and a supported `[execution] profile`; legacy version 1 must omit both fields"),
            Self::UnsupportedSchemaVersion { version } => write!(f, "unsupported authored CI schema version `{version}` - omit it for legacy version 1 or use schema version 2"),
            Self::UnsupportedExecutionProfile { profile } => write!(f, "unsupported CI execution profile `{profile}` - version 2 supports `linux-small-v1` and `linux-build-v1`"),
        }
    }
}

impl std::error::Error for VersionedCiConfigError {}

impl From<CiConfigError> for VersionedCiConfigError {
    fn from(error: CiConfigError) -> Self {
        Self::Legacy(error)
    }
}

impl VersionedCiConfigError {
    pub(crate) fn into_legacy_surface(self) -> CiConfigError {
        match self {
            Self::Legacy(error) => error,
            error => CiConfigError::Syntax {
                format: "versioned",
                message: error.to_string(),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredCi {
    on: String,
    jobs: Vec<AuthoredJob>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedAuthoredCi {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    execution: Option<AuthoredExecution>,
    on: String,
    jobs: Vec<AuthoredJob>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredExecution {
    #[serde(default)]
    profile: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredJob {
    name: String,
    image: String,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    build: Option<StructuredBuildV1>,
    #[serde(default)]
    needs: Vec<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    matrix: BTreeMap<String, Vec<String>>,
}

fn parse_trigger(token: &str) -> Result<OnTrigger, CiConfigError> {
    match token {
        "push" => Ok(OnTrigger::Push),
        "pull_request" => Ok(OnTrigger::PullRequest),
        "issue" => Ok(OnTrigger::IssueTransitioned),
        "manual" => Ok(OnTrigger::Manual),
        "schedule" => Ok(OnTrigger::Schedule),
        "agent" => Ok(OnTrigger::Agent),
        other => Err(CiConfigError::UnknownTrigger {
            value: other.to_string(),
        }),
    }
}

fn parse_kind(job: &str, kind: &Option<String>) -> Result<JobKind, CiConfigError> {
    match kind.as_deref() {
        None | Some("normal") => Ok(JobKind::Normal),
        Some("generate") => Ok(JobKind::Generate),
        Some(other) => Err(CiConfigError::BadJobKind {
            job: job.to_string(),
            value: other.to_string(),
        }),
    }
}

impl AuthoredCi {
    fn into_definition(self, allow_structured_build: bool) -> Result<CiDefinition, CiConfigError> {
        let on = parse_trigger(&self.on)?;

        if self.jobs.is_empty() {
            return Err(CiConfigError::EmptyJobs);
        }
        if self.jobs.len() > MAX_AUTHORED_JOBS {
            return Err(CiConfigError::TooManyJobs {
                actual: self.jobs.len(),
                maximum: MAX_AUTHORED_JOBS,
            });
        }

        let mut names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for j in &self.jobs {
            if j.name.is_empty() {
                return Err(CiConfigError::EmptyJobName);
            }
            if !valid_machine_token(
                &j.name,
                myelin_ci_controlplane::run_plan::MAX_JOB_NAME_BYTES,
            ) {
                return Err(CiConfigError::InvalidMachineToken {
                    field: "job name".into(),
                    value: j.name.clone(),
                });
            }
            if !names.insert(j.name.as_str()) {
                return Err(CiConfigError::DuplicateJob(j.name.clone()));
            }
        }

        let mut jobs = Vec::with_capacity(self.jobs.len());
        for j in &self.jobs {
            if j.image.is_empty()
                || j.image.len() > myelin_ci_controlplane::run_plan::MAX_IMAGE_BYTES
            {
                return Err(CiConfigError::InvalidImage {
                    job: j.name.clone(),
                });
            }
            match (j.command.as_deref(), &j.build) {
                (Some(command), None) => validate_command(&j.name, command)?,
                (None, Some(build)) if allow_structured_build => build
                    .validate_for_job(&j.name)
                    .map_err(|error| CiConfigError::InvalidBuild {
                        job: j.name.clone(),
                        detail: error.to_string(),
                    })?,
                (None, Some(_)) => {
                    return Err(CiConfigError::InvalidBuild {
                        job: j.name.clone(),
                        detail: "requires `schema_version = 2` and an execution profile".into(),
                    })
                }
                (Some(_), Some(_)) => {
                    return Err(CiConfigError::InvalidBuild {
                        job: j.name.clone(),
                        detail: "declare either `command` or `build`, never both".into(),
                    })
                }
                (None, None) => {
                    return Err(CiConfigError::Syntax {
                        format: "schema",
                        message: format!(
                            "job `{}` must declare either `command` or a structured `build`",
                            j.name
                        ),
                    })
                }
            }
            if j.matrix.len() > myelin_ci_controlplane::run_plan::MAX_MATRIX_AXES {
                return Err(CiConfigError::TooManyMatrixAxes {
                    job: j.name.clone(),
                    actual: j.matrix.len(),
                    maximum: myelin_ci_controlplane::run_plan::MAX_MATRIX_AXES,
                });
            }
            for need in &j.needs {
                if !names.contains(need.as_str()) {
                    return Err(CiConfigError::UnknownNeed {
                        job: j.name.clone(),
                        need: need.clone(),
                    });
                }
            }
            for (axis, values) in &j.matrix {
                if !valid_machine_token(
                    axis,
                    myelin_ci_controlplane::run_plan::MAX_MATRIX_KEY_BYTES,
                ) {
                    return Err(CiConfigError::InvalidMachineToken {
                        field: format!("job `{}` matrix axis", j.name),
                        value: axis.clone(),
                    });
                }
                if values.is_empty() {
                    return Err(CiConfigError::EmptyMatrixAxis {
                        job: j.name.clone(),
                        axis: axis.clone(),
                    });
                }
                for value in values {
                    if !valid_machine_token(
                        value,
                        myelin_ci_controlplane::run_plan::MAX_MATRIX_VALUE_BYTES,
                    ) {
                        return Err(CiConfigError::InvalidMachineToken {
                            field: format!("job `{}` matrix value for `{axis}`", j.name),
                            value: value.clone(),
                        });
                    }
                }
            }
            let kind = parse_kind(&j.name, &j.kind)?;
            jobs.push(JobDef {
                name: j.name.clone(),
                image: j.image.clone(),
                command: j.command.clone().unwrap_or_default(),
                build: j.build.clone(),
                needs: j.needs.clone(),
                kind,
                matrix: j.matrix.clone(),
            });
        }

        Ok(CiDefinition { on, jobs })
    }
}

impl VersionedAuthoredCi {
    fn into_definition(self) -> Result<VersionedCiDefinition, VersionedCiConfigError> {
        let contract = match (self.schema_version, self.execution) {
            (None, None) => CiPlanContract::V1,
            (Some(2), Some(execution)) => {
                let Some(profile_name) = execution.profile else {
                    return Err(VersionedCiConfigError::PartialExecutionContract);
                };
                let profile = match profile_name.as_str() {
                    "linux-small-v1" => myelin_ci_controlplane::CiExecutionProfileV1::LinuxSmallV1,
                    "linux-build-v1" => myelin_ci_controlplane::CiExecutionProfileV1::LinuxBuildV1,
                    _ => {
                        return Err(VersionedCiConfigError::UnsupportedExecutionProfile {
                            profile: profile_name,
                        })
                    }
                };
                CiPlanContract::V2(myelin_ci_controlplane::CiExecutionRequestV1 {
                    schema_version: myelin_ci_controlplane::EXECUTION_REQUEST_SCHEMA_V1,
                    profile,
                })
            }
            (Some(version), _) if version != 2 => {
                return Err(VersionedCiConfigError::UnsupportedSchemaVersion { version })
            }
            _ => return Err(VersionedCiConfigError::PartialExecutionContract),
        };
        let allow_structured_build = matches!(contract, CiPlanContract::V2(_));
        let definition = AuthoredCi {
            on: self.on,
            jobs: self.jobs,
        }
        .into_definition(allow_structured_build)
        .map_err(VersionedCiConfigError::Legacy)?;
        Ok(VersionedCiDefinition {
            contract,
            on: definition.on,
            jobs: definition.jobs,
        })
    }
}

fn valid_machine_token(value: &str, maximum: usize) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    value.len() <= maximum
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn validate_command(job: &str, command: &[String]) -> Result<(), CiConfigError> {
    use myelin_ci_controlplane::run_plan::{MAX_COMMAND_ARGS, MAX_COMMAND_BYTES};
    if command.is_empty() || command.len() > MAX_COMMAND_ARGS {
        return Err(CiConfigError::InvalidCommand {
            job: job.into(),
            detail: format!("must contain 1..={MAX_COMMAND_ARGS} arguments"),
        });
    }
    if command[0].is_empty() {
        return Err(CiConfigError::InvalidCommand {
            job: job.into(),
            detail: "argv[0] must not be empty".into(),
        });
    }
    let total = command.iter().try_fold(0usize, |total, argument| {
        (!argument.contains('\0'))
            .then(|| total.checked_add(argument.len()))
            .flatten()
    });
    let Some(total) = total else {
        return Err(CiConfigError::InvalidCommand {
            job: job.into(),
            detail: "arguments must not contain NUL".into(),
        });
    };
    if total > MAX_COMMAND_BYTES {
        return Err(CiConfigError::InvalidCommand {
            job: job.into(),
            detail: format!("{total} bytes exceeds {MAX_COMMAND_BYTES}"),
        });
    }
    Ok(())
}

fn classify_de_error(format: &'static str, message: String) -> CiConfigError {
    if message.contains("unknown field") {
        CiConfigError::UnknownField { format, message }
    } else {
        CiConfigError::Syntax { format, message }
    }
}

pub fn parse_ci_config(
    bytes: &[u8],
    filename_or_format: &str,
) -> Result<CiDefinition, CiConfigError> {
    let authored: AuthoredCi = deserialize_ci_config(bytes, filename_or_format)?;
    authored.into_definition(false)
}

pub fn parse_versioned_ci_config(
    bytes: &[u8],
    filename_or_format: &str,
) -> Result<VersionedCiDefinition, VersionedCiConfigError> {
    let authored: VersionedAuthoredCi =
        deserialize_ci_config(bytes, filename_or_format).map_err(VersionedCiConfigError::Legacy)?;
    authored.into_definition()
}

fn deserialize_ci_config<T: DeserializeOwned>(
    bytes: &[u8],
    filename_or_format: &str,
) -> Result<T, CiConfigError> {
    if bytes.len() > MAX_CI_CONFIG_BYTES {
        return Err(CiConfigError::ConfigTooLarge {
            actual: bytes.len(),
            maximum: MAX_CI_CONFIG_BYTES,
        });
    }
    let format = ConfigFormat::from_hint(filename_or_format)?;

    let text = std::str::from_utf8(bytes).map_err(|e| CiConfigError::Syntax {
        format: match format {
            ConfigFormat::Toml => "toml",
            ConfigFormat::Json => "json",
        },
        message: format!("not valid UTF-8: {e}"),
    })?;

    Ok(match format {
        ConfigFormat::Toml => {
            toml::from_str(text).map_err(|e| classify_de_error("toml", e.to_string()))?
        }
        ConfigFormat::Json => {
            serde_json::from_str(text).map_err(|e| classify_de_error("json", e.to_string()))?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{
        resolve_snapshot, resolve_versioned_snapshot, resolve_versioned_snapshot_with_cargo_vendor,
        CiPlanContract, ResolveError, ResolvedSnapshot, ResolvedSnapshotExt, VersionedCiDefinition,
        VersionedResolvedSnapshot,
    };
    use myelin_storage::{BlobStore, ContentHash, FsBlobStore};
    use myelin_tenancy::TenantId;

    const SMOKE_CARGO_LOCK: &[u8] =
        include_bytes!("../../../testing/fixtures/cargo-vendor-smoke/Cargo.lock");

    const PINNED_BUILD: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
    const PINNED_TEST: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

    const VALID_TOML: &str = r#"
on = "push"

[[jobs]]
name = "build"
image = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000"
command = ["build"]

[[jobs]]
name = "test"
image = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000"
command = ["test"]
needs = ["build"]

[jobs.matrix]
os = ["linux", "macos"]
rust = ["stable", "beta"]

[[jobs]]
name = "gen-matrix"
image = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000"
command = ["generate"]
"#;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn a_valid_toml_fixture_parses_to_the_exact_definition() {
        let def = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml")
            .expect("the valid fixture parses");

        assert_eq!(def.on, OnTrigger::Push, "the armed trigger");
        assert_eq!(def.jobs.len(), 3, "three authored jobs");

        assert_eq!(
            def.jobs[0],
            JobDef::normal("build", PINNED_BUILD, ["build"])
        );

        let test = &def.jobs[1];
        assert_eq!(test.name, "test");
        assert_eq!(test.image, PINNED_TEST);
        assert_eq!(test.needs, vec!["build".to_string()]);
        assert_eq!(test.kind, JobKind::Normal);
        assert_eq!(
            test.matrix.get("os"),
            Some(&vec!["linux".to_string(), "macos".to_string()])
        );
        assert_eq!(
            test.matrix.get("rust"),
            Some(&vec!["stable".to_string(), "beta".to_string()])
        );

        assert_eq!(def.jobs[2].name, "gen-matrix");
        assert_eq!(def.jobs[2].kind, JobKind::Normal);
    }

    #[test]
    fn the_same_definition_parses_from_json() {
        let json = r#"{
            "on": "push",
            "jobs": [
                { "name": "build", "image": "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000", "command": ["build"] }
            ]
        }"#;
        let def = parse_ci_config(json.as_bytes(), ".myelin/ci.json").expect("valid json parses");
        assert_eq!(def.on, OnTrigger::Push);
        assert_eq!(
            def.jobs,
            vec![JobDef::normal("build", PINNED_BUILD, ["build"])]
        );
    }

    #[test]
    fn exact_v2_toml_and_json_parse_to_the_shared_execution_request() {
        let toml = format!(
            r#"schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "build"
image = "{PINNED_BUILD}"
command = ["build"]
"#
        );
        let json = format!(
            r#"{{"schema_version":2,"execution":{{"profile":"linux-small-v1"}},"on":"push","jobs":[{{"name":"build","image":"{PINNED_BUILD}","command":["build"]}}]}}"#
        );
        for (bytes, hint) in [
            (toml.as_bytes(), ".myelin/ci.toml"),
            (json.as_bytes(), ".myelin/ci.json"),
        ] {
            let def = parse_versioned_ci_config(bytes, hint).expect("valid V2 request");
            assert_eq!(
                def.contract,
                CiPlanContract::V2(myelin_ci_controlplane::CiExecutionRequestV1 {
                    schema_version: myelin_ci_controlplane::EXECUTION_REQUEST_SCHEMA_V1,
                    profile: myelin_ci_controlplane::CiExecutionProfileV1::LinuxSmallV1,
                })
            );
            assert!(matches!(
                parse_ci_config(bytes, hint),
                Err(CiConfigError::UnknownField { .. })
            ));
        }
        assert_eq!(
            parse_versioned_ci_config(VALID_TOML.as_bytes(), "toml").unwrap(),
            VersionedCiDefinition::v1(
                OnTrigger::Push,
                parse_ci_config(VALID_TOML.as_bytes(), "toml").unwrap().jobs,
            )
        );
    }

    #[test]
    fn linux_build_v1_parses_to_the_build_execution_request() {
        let authored = format!(
            "schema_version=2\non=\"push\"\n[execution]\nprofile=\"linux-build-v1\"\n[[jobs]]\nname=\"build\"\nimage=\"{PINNED_BUILD}\"\ncommand=[\"build\"]\n"
        );
        let definition = parse_versioned_ci_config(authored.as_bytes(), "toml").unwrap();

        assert_eq!(
            definition.contract,
            CiPlanContract::V2(myelin_ci_controlplane::CiExecutionRequestV1 {
                schema_version: myelin_ci_controlplane::EXECUTION_REQUEST_SCHEMA_V1,
                profile: myelin_ci_controlplane::CiExecutionProfileV1::LinuxBuildV1,
            })
        );
    }

    #[test]
    fn structured_cargo_build_parses_and_resolves_only_on_v2() {
        let toml = format!(
            r#"schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "build"
image = "{PINNED_BUILD}"
build = {{ tool = "cargo", args = ["build", "--locked"] }}
"#
        );
        let definition = parse_versioned_ci_config(toml.as_bytes(), "toml").unwrap();
        assert!(definition.jobs[0].command.is_empty());
        assert_eq!(
            definition.jobs[0].build.as_ref().unwrap().platform_argv(),
            [
                "cargo",
                "build",
                "--locked",
                "--config",
                myelin_ci_sandbox::gvisor::CARGO_SOURCE_REPLACE_CONFIG,
                "--config",
                myelin_ci_sandbox::gvisor::CARGO_VENDOR_DIRECTORY_CONFIG,
            ]
        );

        let store = FsBlobStore::new();
        let (snapshot, _) = resolve_versioned_snapshot_with_cargo_vendor(
            &definition,
            &store,
            &TenantId("acme".into()),
            Some(SMOKE_CARGO_LOCK),
        )
        .unwrap();
        let VersionedResolvedSnapshot::V2(plan) = snapshot else {
            panic!("structured builds require and retain the V2 wire")
        };
        assert!(plan.jobs[0].command.is_empty());
        assert!(plan.jobs[0].build.is_some());
        assert_eq!(
            plan.jobs[0].selected_cargo_vendor.as_deref(),
            Some(myelin_ci_sandbox::cargo_vendor_smoke_reference().as_str())
        );

        assert!(matches!(
            resolve_versioned_snapshot_with_cargo_vendor(
                &definition,
                &FsBlobStore::new(),
                &TenantId("acme".into()),
                None,
            ),
            Err(ResolveError::CargoVendorLockMissing)
        ));
        assert!(matches!(
            resolve_versioned_snapshot_with_cargo_vendor(
                &definition,
                &FsBlobStore::new(),
                &TenantId("acme".into()),
                Some(b"# not a registered lock\n"),
            ),
            Err(ResolveError::CargoVendorUnmatched { .. })
        ));

        let legacy = format!(
            r#"on = "push"
[[jobs]]
name = "build"
image = "{PINNED_BUILD}"
build = {{ tool = "cargo", args = ["build", "--locked"] }}
"#
        );
        assert!(matches!(
            parse_ci_config(legacy.as_bytes(), "toml"),
            Err(CiConfigError::InvalidBuild { detail, .. }) if detail.contains("schema_version = 2")
        ));
    }

    #[test]
    fn structured_test_and_clippy_recipes_parse_and_lower_on_v2() {
        let job = |name: &str, recipe: &str| {
            format!(
                "[[jobs]]\nname=\"{name}\"\nimage=\"{PINNED_BUILD}\"\nbuild = {{ tool = \"cargo\", args = {recipe} }}\n"
            )
        };
        let toml = format!(
            "schema_version = 2\non = \"push\"\n[execution]\nprofile = \"linux-build-v1\"\n{}{}{}",
            job("build", "[\"build\", \"--locked\"]"),
            job("unit", "[\"test\", \"--locked\", \"--lib\"]"),
            job(
                "lint",
                "[\"clippy\", \"--locked\", \"--all-targets\", \"--\", \"-D\", \"warnings\"]"
            ),
        );
        let def = parse_versioned_ci_config(toml.as_bytes(), "toml").expect("v2 recipes parse");
        let replace = myelin_ci_sandbox::gvisor::CARGO_SOURCE_REPLACE_CONFIG;
        let vendor = myelin_ci_sandbox::gvisor::CARGO_VENDOR_DIRECTORY_CONFIG;

        assert_eq!(
            def.jobs[1].build.as_ref().unwrap().platform_argv(),
            ["cargo", "test", "--locked", "--lib", "--config", replace, "--config", vendor]
        );
        assert_eq!(
            def.jobs[2].build.as_ref().unwrap().platform_argv(),
            [
                "cargo",
                "clippy",
                "--locked",
                "--all-targets",
                "--config",
                replace,
                "--config",
                vendor,
                "--",
                "-D",
                "warnings"
            ]
        );

        let (snapshot, _) = resolve_versioned_snapshot_with_cargo_vendor(
            &def,
            &FsBlobStore::new(),
            &TenantId("acme".into()),
            Some(SMOKE_CARGO_LOCK),
        )
        .unwrap();
        let VersionedResolvedSnapshot::V2(plan) = snapshot else {
            panic!("structured builds require the V2 wire")
        };
        assert!(plan.jobs.iter().all(|job| job.build.is_some()));
        assert!(plan
            .jobs
            .iter()
            .all(|job| job.selected_cargo_vendor.as_deref()
                == Some(myelin_ci_sandbox::cargo_vendor_smoke_reference().as_str())));

        let rejected = format!(
            "schema_version = 2\non = \"push\"\n[execution]\nprofile = \"linux-build-v1\"\n{}",
            job("bad", "[\"test\", \"--locked\"]"),
        );
        assert!(matches!(
            parse_versioned_ci_config(rejected.as_bytes(), "toml"),
            Err(VersionedCiConfigError::Legacy(CiConfigError::InvalidBuild { detail, .. }))
                if detail.contains("not in the platform allowlist")
        ));
    }

    #[test]
    fn structured_build_authored_fields_fail_closed() {
        let config = |build: &str| {
            format!(
                r#"schema_version = 2
on = "push"
[execution]
profile = "linux-small-v1"
[[jobs]]
name = "build"
image = "{PINNED_BUILD}"
{build}
"#
            )
        };
        for invalid in [
            "build = { tool = \"cargo\", args = [\"build\", \"--locked;touch-pwned\"] }"
                .to_string(),
            format!(
                "build = {{ tool = \"cargo\", args = [\"{}\"] }}",
                "x".repeat(257)
            ),
        ] {
            assert!(matches!(
                parse_versioned_ci_config(config(&invalid).as_bytes(), "toml"),
                Err(VersionedCiConfigError::Legacy(
                    CiConfigError::InvalidBuild { .. }
                ))
            ));
        }

        let unknown = config("build = { tool = \"make\", args = [\"build\"] }");
        assert!(matches!(
            parse_versioned_ci_config(unknown.as_bytes(), "toml"),
            Err(VersionedCiConfigError::Legacy(CiConfigError::Syntax { .. }))
        ));

        let both = config(
            "command = [\"/bin/sh\", \"-c\", \"cargo build\"]\nbuild = { tool = \"cargo\", args = [\"build\", \"--locked\"] }",
        );
        assert!(matches!(
            parse_versioned_ci_config(both.as_bytes(), "toml"),
            Err(VersionedCiConfigError::Legacy(CiConfigError::InvalidBuild { detail, .. }))
                if detail.contains("never both")
        ));
    }

    #[test]
    fn partial_unknown_and_nested_unknown_v2_contracts_are_typed_refusals() {
        for authored in [
            "schema_version=2\non=\"push\"\njobs=[]\n",
            "on=\"push\"\njobs=[]\n[execution]\nprofile=\"linux-small-v1\"\n",
            "schema_version=2\non=\"push\"\njobs=[]\n[execution]\n",
        ] {
            assert_eq!(
                parse_versioned_ci_config(authored.as_bytes(), "toml"),
                Err(VersionedCiConfigError::PartialExecutionContract)
            );
        }
        assert_eq!(
            parse_versioned_ci_config(b"schema_version=9\non=\"push\"\njobs=[]\n", "toml"),
            Err(VersionedCiConfigError::UnsupportedSchemaVersion { version: 9 })
        );
        let profile =
            "schema_version=2\non=\"push\"\njobs=[]\n[execution]\nprofile=\"gpu-large\"\n";
        assert_eq!(
            parse_versioned_ci_config(profile.as_bytes(), "toml"),
            Err(VersionedCiConfigError::UnsupportedExecutionProfile {
                profile: "gpu-large".into()
            })
        );
        let unknown = r#"{"schema_version":2,"execution":{"profile":"linux-small-v1","egress":true},"on":"push","jobs":[]}"#;
        assert!(matches!(
            parse_versioned_ci_config(unknown.as_bytes(), "json"),
            Err(VersionedCiConfigError::Legacy(
                CiConfigError::UnknownField { .. }
            ))
        ));
    }

    #[test]
    fn malformed_syntax_is_rejected() {
        let err = parse_ci_config(b"on = = broken", ".myelin/ci.toml")
            .expect_err("malformed toml is rejected");
        assert!(
            matches!(err, CiConfigError::Syntax { format: "toml", .. }),
            "malformed syntax → Syntax: {err:?}"
        );
    }

    #[test]
    fn an_unknown_field_is_rejected_fail_closed() {
        let toml = r#"
on = "push"
oops = "typo"

[[jobs]]
name = "build"
image = "x@sha256:0"
"#;
        let err = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml")
            .expect_err("an unknown key is rejected");
        assert!(
            matches!(&err, CiConfigError::UnknownField { format: "toml", message } if message.contains("oops")),
            "unknown field → UnknownField naming the key: {err:?}"
        );
    }

    #[test]
    fn an_unknown_field_in_a_job_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "build"
image = "x@sha256:0"
retries = 3
"#;
        let err =
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect_err("nested typo rejected");
        assert!(matches!(err, CiConfigError::UnknownField { .. }), "{err:?}");
    }

    #[test]
    fn empty_jobs_is_rejected() {
        let toml = "on = \"push\"\njobs = []\n";
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::EmptyJobs)
        );
    }

    #[test]
    fn a_duplicate_job_name_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "a"
image = "x@sha256:0"
command = ["a"]

[[jobs]]
name = "a"
image = "y@sha256:1"
command = ["a"]
"#;
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::DuplicateJob("a".to_string()))
        );
    }

    #[test]
    fn a_needs_referencing_an_undeclared_job_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "a"
image = "x@sha256:0"
command = ["a"]
needs = ["ghost"]
"#;
        let err = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml")
            .expect_err("dangling need rejected");
        assert!(
            matches!(&err, CiConfigError::UnknownNeed { job, need } if job == "a" && need == "ghost"),
            "{err:?}"
        );
    }

    #[test]
    fn a_malformed_on_trigger_is_rejected() {
        let toml = r#"
on = "whenever"

[[jobs]]
name = "a"
image = "x@sha256:0"
command = ["a"]
"#;
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::UnknownTrigger {
                value: "whenever".to_string()
            })
        );
    }

    #[test]
    fn a_missing_required_field_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "a"
"#;
        let err = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml")
            .expect_err("missing image rejected");
        assert!(
            matches!(err, CiConfigError::Syntax { .. }),
            "missing required field → Syntax: {err:?}"
        );
    }

    #[test]
    fn an_empty_job_name_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = ""
image = "x@sha256:0"
command = ["a"]
"#;
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::EmptyJobName)
        );
    }

    #[test]
    fn a_bad_job_kind_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "a"
image = "x@sha256:0"
command = ["a"]
kind = "wizard"
"#;
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::BadJobKind {
                job: "a".to_string(),
                value: "wizard".to_string()
            })
        );
    }

    #[test]
    fn an_empty_matrix_axis_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "a"
image = "x@sha256:0"
command = ["a"]

[jobs.matrix]
os = []
"#;
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::EmptyMatrixAxis {
                job: "a".to_string(),
                axis: "os".to_string()
            })
        );
    }

    #[test]
    fn a_yaml_hint_is_refused_as_deferred() {
        let err = parse_ci_config(b"on: push", ".myelin/ci.yaml").expect_err("yaml is deferred");
        assert!(
            matches!(err, CiConfigError::UnknownFormat { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_parsed_config_composes_with_resolve_snapshot() {
        let def = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml").expect("valid parse");
        let store = FsBlobStore::new();
        let (snap, addr): (ResolvedSnapshot, ContentHash) =
            resolve_snapshot(&def, &store, &tenant()).expect("the parsed def resolves");
        let versioned = parse_versioned_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml")
            .expect("the same legacy document parses through the production versioned path");
        let (versioned_snap, versioned_addr) =
            resolve_versioned_snapshot(&versioned, &store, &tenant()).unwrap();
        assert_eq!(
            versioned_snap,
            myelin_ci_controlplane::VersionedResolvedRunPlan::V1(snap.clone())
        );
        assert_eq!(
            versioned_addr, addr,
            "legacy bytes and CAS address stay exact"
        );

        assert_eq!(snap.jobs.len(), 6, "1 build + 4 test-matrix + 1 generator");
        assert!(!snap.has_dynamic_generation());
        assert_eq!(addr, ContentHash::blake3(&snap.canonical_bytes().unwrap()));

        let names: Vec<&str> = snap.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(
            names
                .iter()
                .filter(|name| name.starts_with("test--"))
                .count(),
            4
        );
    }

    #[test]
    fn v2_matrix_preserves_stage_and_is_reproducible_in_cas() {
        let toml = format!(
            r#"schema_version = 2
on = "push"

[execution]
profile = "linux-small-v1"

[[jobs]]
name = "test"
image = "{PINNED_TEST}"
command = ["test"]

[jobs.matrix]
os = ["linux", "macos"]
"#
        );
        let definition = parse_versioned_ci_config(toml.as_bytes(), "toml").unwrap();
        let store = FsBlobStore::new();
        let (first, first_hash) =
            resolve_versioned_snapshot(&definition, &store, &tenant()).unwrap();
        let (second, second_hash) =
            resolve_versioned_snapshot(&definition, &store, &tenant()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first_hash, second_hash);
        let plan = first.as_v2().expect("V2 snapshot");
        assert_eq!(plan.jobs.len(), 2);
        assert!(plan.jobs.iter().all(|job| job.stage == "test"));
        assert!(plan.jobs.iter().all(|job| job.name.starts_with("test--")));
        let bytes = store.get(&tenant(), &first_hash).unwrap();
        assert_eq!(bytes, first.canonical_bytes().unwrap());
        assert_eq!(
            myelin_ci_controlplane::decode_resolved_run_plan(&bytes).unwrap(),
            first
        );
        assert_eq!(
            plan.launch_request_digest_v1().unwrap(),
            second.as_v2().unwrap().launch_request_digest_v1().unwrap()
        );
    }

    #[test]
    fn v2_generator_request_is_refused_before_cas_write() {
        let definition = VersionedCiDefinition::v2(
            OnTrigger::Push,
            myelin_ci_controlplane::CiExecutionRequestV1 {
                schema_version: myelin_ci_controlplane::EXECUTION_REQUEST_SCHEMA_V1,
                profile: myelin_ci_controlplane::CiExecutionProfileV1::LinuxSmallV1,
            },
            vec![JobDef::normal("generate", PINNED_BUILD, ["generate"]).as_generator()],
        );
        assert!(matches!(
            resolve_versioned_snapshot(&definition, &FsBlobStore::new(), &tenant()),
            Err(ResolveError::InvalidPlan(detail)) if detail.contains("fragment ingestion")
        ));
    }

    #[test]
    fn a_floating_tag_parses_but_the_resolver_refuses_it() {
        let toml = r#"
on = "push"

[[jobs]]
name = "build"
image = "alpine:3"
command = ["build"]
"#;
        let def =
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect("a floating tag parses");
        assert_eq!(def.jobs[0].image, "alpine:3");
        let store = FsBlobStore::new();
        assert!(
            resolve_snapshot(&def, &store, &tenant()).is_err(),
            "the resolver refuses the floating tag (the digest-pin control is semantic, not schema)"
        );
    }

    #[test]
    fn parsing_is_deterministic() {
        let a = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml").expect("parse a");
        let b = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml").expect("parse b");
        assert_eq!(a, b, "the same bytes → the same CiDefinition");
    }

    #[test]
    fn required_command_and_raw_size_bounds_are_fail_closed() {
        let missing = b"on=\"push\"\n[[jobs]]\nname=\"a\"\nimage=\"x@sha256:0\"\n";
        assert!(matches!(
            parse_ci_config(missing, "toml"),
            Err(CiConfigError::Syntax { .. })
        ));
        let oversized = vec![b' '; MAX_CI_CONFIG_BYTES + 1];
        assert_eq!(
            parse_ci_config(&oversized, "toml"),
            Err(CiConfigError::ConfigTooLarge {
                actual: MAX_CI_CONFIG_BYTES + 1,
                maximum: MAX_CI_CONFIG_BYTES,
            })
        );
    }

    #[test]
    fn command_and_machine_token_bounds_are_enforced() {
        let too_many = format!(
            "on=\"push\"\n[[jobs]]\nname=\"a\"\nimage=\"x@sha256:0\"\ncommand=[{}]\n",
            std::iter::repeat_n(
                "\"x\"",
                myelin_ci_controlplane::run_plan::MAX_COMMAND_ARGS + 1
            )
            .collect::<Vec<_>>()
            .join(",")
        );
        assert!(matches!(
            parse_ci_config(too_many.as_bytes(), "toml"),
            Err(CiConfigError::InvalidCommand { .. })
        ));
        let invalid_name = b"on=\"push\"\n[[jobs]]\nname=\"not a token\"\nimage=\"x@sha256:0\"\ncommand=[\"run\"]\n";
        assert!(matches!(
            parse_ci_config(invalid_name, "toml"),
            Err(CiConfigError::InvalidMachineToken { .. })
        ));
        let jobs = (0..=MAX_AUTHORED_JOBS)
            .map(|index| {
                format!("[[jobs]]\nname=\"j{index}\"\nimage=\"x@sha256:0\"\ncommand=[\"run\"]\n")
            })
            .collect::<String>();
        assert!(matches!(
            parse_ci_config(format!("on=\"push\"\n{jobs}").as_bytes(), "toml"),
            Err(CiConfigError::TooManyJobs { .. })
        ));
    }
}
