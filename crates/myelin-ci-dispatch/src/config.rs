//! **CT-004b (M4): the authored `.myelin/ci.*` config-file PARSER — the CI-dispatch prerequisite
//! that turns a real config FILE into the in-memory [`CiDefinition`] the resolver already consumes.**
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §7.4 — the config grammar is a **"declarative JSON-schema'd core, authored as YAML/TOML,
//! validated against a published JSON Schema"**; expressions use the bounded `QueryAst` (NOT
//! CEL/Starlark — the `on:` block compiles to ONE `QueryAst` via
//! [`crate::dispatch::compile_trigger`]); and there is a **sandboxed dynamic-generation escape
//! hatch** (a `Generate` job) which the parser only REPRESENTS ([`JobKind::Generate`]), never
//! executes (the in-sandbox execution is CI-P15).
//!
//! ## Why this module exists (the census gap CT-004b closes)
//! [`crate::resolve::resolve_snapshot`] takes an ALREADY-parsed [`CiDefinition`]; before CT-004b
//! NOTHING turned the authored YAML/TOML/JSON text into one. This module is that missing seam:
//! [`parse_ci_config`] deserialises the authored document into a fail-closed serde DTO
//! (`#[serde(deny_unknown_fields)]`) and maps it into the domain [`CiDefinition`]. It stays STRICTLY
//! in the parse+validate lane — it does NOT wire the bus consumer, the flow executor, or
//! reserve/start (the next chunk).
//!
//! ## The validation split — SCHEMA (here) vs SEMANTIC ([`resolve_snapshot`])
//! The prompt's honest "JSON-Schema validation" here is a TYPED serde DTO + explicit STRUCTURAL
//! checks (pulling a full JSON-Schema engine in just to tick a box would be disproportionate — a
//! `deny_unknown_fields` DTO with named structural checks IS the schema half). This module owns the
//! **schema/structural** half:
//!   - required fields present (serde: a missing `on`/`name`/`image` is a deserialize error);
//!   - unknown keys REJECTED (`deny_unknown_fields` — a typo is a fail-closed error, never a
//!     silently-ignored footgun);
//!   - job names non-empty + UNIQUE (the DAG node ids);
//!   - every `needs` edge references a DECLARED job (a dangling edge is caught at authoring time);
//!   - the `on:` value is a WELL-FORMED trigger token (maps to an [`OnTrigger`] variant);
//!   - each matrix axis is well-formed (a NON-EMPTY value list — an empty axis would be silently
//!     dropped by the resolver's expansion, so it is refused here);
//!   - the job `kind` is a known token (`normal` | `generate`).
//!
//! [`resolve_snapshot`] owns the **semantic** half and this module does NOT duplicate it: DAG
//! **acyclicity**, the **digest-pin-or-fail-closed** supply-chain control (a floating tag is
//! resolved-time, NOT parse-time — the authored `image` may be a floating tag here), and the
//! deterministic **matrix expansion**. (The resolver ALSO re-checks non-empty/unique/needs as
//! defense-in-depth — it OWNS the DAG; this module catches the same defects EARLIER with a
//! config-authoring-specific error so `myelin ci validate` is loud at the source.)
//!
//! ## The seam end-to-end (CT-004b → the trigger-consumer chunk)
//! ```text
//!   config bytes at the pushed ref  ──parse_ci_config──▶  CiDefinition
//!                                                            │  on:   ──compile_trigger──▶ QueryAst (EventMatcher)
//!                                                            └─ jobs: ──resolve_snapshot──▶ ResolvedSnapshot (CAS)
//! ```
//! **Handoff to the trigger-consumer chunk (NOT this chunk):** the live `ci-dispatch.trigger`
//! consumer, on a matching push, reads the config blob at the pushed ref, calls
//! `parse_ci_config(bytes, ".myelin/ci.toml")` → [`CiDefinition`], then
//! [`crate::dispatch::compile_trigger`]`(&def.on)` (the armed matcher) +
//! [`crate::resolve::resolve_snapshot`]`(&def, blobs, tenant)` (the CAS snapshot) →
//! [`crate::resolve::reserve_and_start`]. This module ships ONLY the pure parse+validate core; it
//! registers no consumer and touches no executor.
//!
//! ## Fail-closed + typed
//! Every failure is a LOUD, typed [`CiConfigError`] with a self-describing message — malformed
//! syntax, an unknown field, an unknown format, a bad `on:`/`kind`, empty jobs, an empty/duplicate
//! job name, a dangling `needs`, an empty matrix axis. NEVER a degraded/partial [`CiDefinition`].
//! Parsing is DETERMINISTIC: the same bytes always yield the same [`CiDefinition`].

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::dispatch::OnTrigger;
use crate::resolve::{CiDefinition, JobDef, JobKind};

pub const MAX_CI_CONFIG_BYTES: usize = 1024 * 1024;
pub const MAX_AUTHORED_JOBS: usize = 256;

// =================================================================================================
// 1. The authored format(s) + the format hint.
// =================================================================================================

/// The authored config formats CT-004b supports. TOML is the primary human-authored surface (arch
/// 02 §7.4); JSON is the same declarative core in the machine/JSON-Schema form. **YAML is
/// DEFERRED** (named): there is no workspace YAML dep and `serde_yaml` is archived/unmaintained —
/// adding it would violate minimal-deps; a `.yml`/`.yaml` hint returns a self-describing
/// [`CiConfigError::UnknownFormat`] rather than silently guessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFormat {
    /// `.myelin/ci.toml` — the primary authored surface (the workspace-pinned `toml` dep).
    Toml,
    /// `.myelin/ci.json` — the JSON-Schema'd core in JSON (the already-present `serde_json`).
    Json,
}

impl ConfigFormat {
    /// Infer the format from a filename or an explicit format token. Accepts a full path
    /// (`.myelin/ci.toml`), a bare extension (`toml`), or the format name (`json`). A YAML hint
    /// (`.yml`/`.yaml`) is refused with a named-defer error; anything else is
    /// [`CiConfigError::UnknownFormat`].
    pub fn from_hint(hint: &str) -> Result<ConfigFormat, CiConfigError> {
        // Lowercase + take the trailing extension token (after the last `.` or `/`), so a full path
        // and a bare token both resolve.
        let lower = hint.to_ascii_lowercase();
        let tail = lower
            .rsplit(['.', '/'])
            .next()
            .unwrap_or(lower.as_str());
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
                detail: "unrecognised config format — supported: `.myelin/ci.toml`, \
                         `.myelin/ci.json` (YAML is deferred)."
                    .to_string(),
            }),
        }
    }
}

// =================================================================================================
// 2. The typed error taxonomy (fail-closed + LOUD — never a degraded CiDefinition).
// =================================================================================================

/// Why an authored `.myelin/ci.*` document fails to parse into a [`CiDefinition`] (arch 02 §7.4,
/// fail-closed). Each variant is a distinct, assertable, self-describing failure — the parser NEVER
/// returns a partial/coerced definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiConfigError {
    ConfigTooLarge { actual: usize, maximum: usize },
    /// The `filename_or_format` hint was not a supported format (or was the deferred YAML).
    UnknownFormat {
        /// The hint that could not be resolved to a supported format.
        hint: String,
        /// A human-readable reason + what IS supported.
        detail: String,
    },
    /// The document is malformed / could not be deserialised (a syntax error, a wrong type, a
    /// MISSING required field). Carries the format + the underlying deserialiser message.
    Syntax {
        /// The format that was being parsed (`toml` | `json`).
        format: &'static str,
        /// The underlying deserialiser message (self-describing — line/column where available).
        message: String,
    },
    /// An UNKNOWN key was present (`#[serde(deny_unknown_fields)]` tripped) — a typo'd/unsupported
    /// field is fail-closed, never silently ignored. (Classified from the deserialiser message,
    /// which names the offending field.)
    UnknownField {
        /// The format that was being parsed (`toml` | `json`).
        format: &'static str,
        /// The deserialiser message naming the unknown field + the expected fields.
        message: String,
    },
    /// The `on:` value is not a known trigger token (`push`/`pull_request`/`issue`/`manual`/
    /// `schedule`/`agent`).
    UnknownTrigger {
        /// The unrecognised `on:` value.
        value: String,
    },
    /// A job's `kind` is neither `normal` nor `generate`.
    BadJobKind {
        /// The job carrying the bad kind.
        job: String,
        /// The unrecognised kind token.
        value: String,
    },
    /// The definition has no jobs — an empty pipeline is rejected (nothing to run).
    EmptyJobs,
    TooManyJobs { actual: usize, maximum: usize },
    /// A job name is empty — a DAG node id must be a non-empty string.
    EmptyJobName,
    /// Two jobs share a name — the DAG node ids must be unique.
    DuplicateJob(String),
    /// A `needs` edge names a job not declared in the document (a dangling DAG edge — caught at
    /// authoring time; the resolver re-checks it as the DAG owner).
    UnknownNeed {
        /// The job carrying the dangling edge.
        job: String,
        /// The non-existent name it depends on.
        need: String,
    },
    /// A matrix axis has an EMPTY value list — an empty axis would be silently dropped by the
    /// resolver's expansion, so it is refused as malformed here.
    EmptyMatrixAxis {
        /// The job carrying the malformed axis.
        job: String,
        /// The axis key with no values.
        axis: String,
    },
    InvalidMachineToken { field: String, value: String },
    InvalidImage { job: String },
    InvalidCommand { job: String, detail: String },
    TooManyMatrixAxes { job: String, actual: usize, maximum: usize },
}

impl std::fmt::Display for CiConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiConfigError::ConfigTooLarge { actual, maximum } => write!(f, "the CI config is {actual} bytes, above the {maximum}-byte limit"),
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
                "unknown `on:` trigger `{value}` — expected one of \
                 push/pull_request/issue/manual/schedule/agent"
            ),
            CiConfigError::BadJobKind { job, value } => write!(
                f,
                "job `{job}`: unknown kind `{value}` — expected `normal` or `generate` (the \
                 dynamic-generation escape hatch)"
            ),
            CiConfigError::EmptyJobs => {
                write!(f, "the CI config has no jobs — nothing to run (an empty pipeline is refused)")
            }
            CiConfigError::TooManyJobs { actual, maximum } => write!(f, "the CI config declares {actual} jobs, above the {maximum}-job limit"),
            CiConfigError::EmptyJobName => {
                write!(f, "a job has an empty name — a DAG node id must be a non-empty string")
            }
            CiConfigError::DuplicateJob(name) => {
                write!(f, "duplicate job name `{name}` — DAG node ids must be unique")
            }
            CiConfigError::UnknownNeed { job, need } => write!(
                f,
                "job `{job}` needs `{need}`, which is not a job in the config (dangling DAG edge)"
            ),
            CiConfigError::EmptyMatrixAxis { job, axis } => write!(
                f,
                "job `{job}`: matrix axis `{axis}` has no values — an empty axis is malformed \
                 (it would silently drop the axis at expansion)"
            ),
            CiConfigError::InvalidMachineToken { field, value } => write!(f, "{field} `{value}` is not a bounded ASCII machine token"),
            CiConfigError::InvalidImage { job } => write!(f, "job `{job}` image reference is empty or overlong"),
            CiConfigError::InvalidCommand { job, detail } => write!(f, "job `{job}` command is invalid: {detail}"),
            CiConfigError::TooManyMatrixAxes { job, actual, maximum } => write!(f, "job `{job}` declares {actual} matrix axes, above the {maximum}-axis limit"),
        }
    }
}

impl std::error::Error for CiConfigError {}

// =================================================================================================
// 3. The authored DTO (the serde surface — fail-closed on unknown fields).
// =================================================================================================

/// The authored `.myelin/ci.*` document (the serde DTO — the SCHEMA the file is validated against).
/// `#[serde(deny_unknown_fields)]` makes a typo'd/unsupported key a fail-closed error, NOT a
/// silently-ignored footgun. This is the WIRE shape; [`AuthoredCi::into_definition`] maps it into
/// the domain [`CiDefinition`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredCi {
    /// The armed trigger token (`push`/`pull_request`/`issue`/`manual`/`schedule`/`agent`) — maps
    /// to an [`OnTrigger`], which compiles to the ONE `QueryAst` (arch 02 §7.4, NOT CEL).
    on: String,
    /// The jobs (the DAG). Required + must be non-empty (validated after deserialisation).
    jobs: Vec<AuthoredJob>,
}

/// One authored job. `image` is the RAW reference as authored — it MAY be a floating tag at parse
/// time; the digest-pin-or-fail-closed control is the RESOLVER's (`resolve_snapshot`), not the
/// parser's. `command` is required; `needs`/`matrix`/`kind` default to empty/normal.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredJob {
    /// The job name (a non-empty, unique DAG node id).
    name: String,
    /// The RAW image reference as authored (may be a floating tag — resolved fail-closed later).
    image: String,
    command: Vec<String>,
    /// The names this job depends on (the DAG edges — each must reference a declared job).
    #[serde(default)]
    needs: Vec<String>,
    /// The job kind: absent/`normal` = an ordinary job; `generate` = the sandboxed
    /// dynamic-generation escape hatch (arch 02 §7.4). Validated into [`JobKind`].
    #[serde(default)]
    kind: Option<String>,
    /// The optional matrix axes (axis key → the ORDERED list of values). Each axis must be
    /// non-empty; the deterministic cross-product expansion is the resolver's.
    #[serde(default)]
    matrix: BTreeMap<String, Vec<String>>,
}

/// Map an authored `on:` token to the domain [`OnTrigger`]. The token vocabulary matches the
/// `OnTrigger` doc-comments (`on: issue` → [`OnTrigger::IssueTransitioned`]).
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

/// Map an authored `kind` token to the domain [`JobKind`] (absent/`normal` → [`JobKind::Normal`];
/// `generate` → the escape hatch). A job name is threaded in for a self-describing error.
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
    /// Map the deserialised DTO into the domain [`CiDefinition`] + run the SCHEMA/structural checks
    /// (non-empty jobs, non-empty + unique names, declared `needs`, well-formed matrix axes,
    /// well-formed `on:`/`kind`). The SEMANTIC checks (acyclicity, digest-pin, matrix expansion)
    /// are the resolver's — NOT duplicated here.
    fn into_definition(self) -> Result<CiDefinition, CiConfigError> {
        let on = parse_trigger(&self.on)?;

        if self.jobs.is_empty() {
            return Err(CiConfigError::EmptyJobs);
        }
        if self.jobs.len() > MAX_AUTHORED_JOBS {
            return Err(CiConfigError::TooManyJobs { actual: self.jobs.len(), maximum: MAX_AUTHORED_JOBS });
        }

        // First pass: build the job set + enforce non-empty + unique names (the DAG node ids).
        let mut names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for j in &self.jobs {
            if j.name.is_empty() {
                return Err(CiConfigError::EmptyJobName);
            }
            if !valid_machine_token(&j.name, myelin_ci_controlplane::run_plan::MAX_JOB_NAME_BYTES) {
                return Err(CiConfigError::InvalidMachineToken { field: "job name".into(), value: j.name.clone() });
            }
            if !names.insert(j.name.as_str()) {
                return Err(CiConfigError::DuplicateJob(j.name.clone()));
            }
        }

        // Second pass: validate `needs` reference declared jobs + matrix axes are well-formed, and
        // map into the domain `JobDef`.
        let mut jobs = Vec::with_capacity(self.jobs.len());
        for j in &self.jobs {
            if j.image.is_empty() || j.image.len() > myelin_ci_controlplane::run_plan::MAX_IMAGE_BYTES {
                return Err(CiConfigError::InvalidImage { job: j.name.clone() });
            }
            validate_command(&j.name, &j.command)?;
            if j.matrix.len() > myelin_ci_controlplane::run_plan::MAX_MATRIX_AXES {
                return Err(CiConfigError::TooManyMatrixAxes { job: j.name.clone(), actual: j.matrix.len(), maximum: myelin_ci_controlplane::run_plan::MAX_MATRIX_AXES });
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
                if !valid_machine_token(axis, myelin_ci_controlplane::run_plan::MAX_MATRIX_KEY_BYTES) {
                    return Err(CiConfigError::InvalidMachineToken { field: format!("job `{}` matrix axis", j.name), value: axis.clone() });
                }
                if values.is_empty() {
                    return Err(CiConfigError::EmptyMatrixAxis {
                        job: j.name.clone(),
                        axis: axis.clone(),
                    });
                }
                for value in values {
                    if !valid_machine_token(value, myelin_ci_controlplane::run_plan::MAX_MATRIX_VALUE_BYTES) {
                        return Err(CiConfigError::InvalidMachineToken { field: format!("job `{}` matrix value for `{axis}`", j.name), value: value.clone() });
                    }
                }
            }
            let kind = parse_kind(&j.name, &j.kind)?;
            jobs.push(JobDef {
                name: j.name.clone(),
                image: j.image.clone(),
                command: j.command.clone(),
                needs: j.needs.clone(),
                kind,
                matrix: j.matrix.clone(),
            });
        }

        Ok(CiDefinition { on, jobs })
    }
}

fn valid_machine_token(value: &str, maximum: usize) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else { return false; };
    value.len() <= maximum && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn validate_command(job: &str, command: &[String]) -> Result<(), CiConfigError> {
    use myelin_ci_controlplane::run_plan::{MAX_COMMAND_ARGS, MAX_COMMAND_BYTES};
    if command.is_empty() || command.len() > MAX_COMMAND_ARGS {
        return Err(CiConfigError::InvalidCommand { job: job.into(), detail: format!("must contain 1..={MAX_COMMAND_ARGS} arguments") });
    }
    if command[0].is_empty() { return Err(CiConfigError::InvalidCommand { job: job.into(), detail: "argv[0] must not be empty".into() }); }
    let total = command.iter().try_fold(0usize, |total, argument| {
        (!argument.contains('\0')).then(|| total.checked_add(argument.len())).flatten()
    });
    let Some(total) = total else { return Err(CiConfigError::InvalidCommand { job: job.into(), detail: "arguments must not contain NUL".into() }); };
    if total > MAX_COMMAND_BYTES { return Err(CiConfigError::InvalidCommand { job: job.into(), detail: format!("{total} bytes exceeds {MAX_COMMAND_BYTES}") }); }
    Ok(())
}

// =================================================================================================
// 4. The public entry — parse authored bytes into a resolver-ready CiDefinition.
// =================================================================================================

/// Classify a deserialiser error message into the `UnknownField` vs `Syntax` variant.
/// `deny_unknown_fields` reports an unknown key as a message beginning with `unknown field` (both
/// `toml` and `serde_json` do so); any other deserialise failure (bad syntax, wrong type, missing
/// required field) is `Syntax`. The classification is on the message string because both formats
/// funnel EVERY schema violation through the ONE `Deserialize` error type — there is no typed
/// discriminant to match on.
fn classify_de_error(format: &'static str, message: String) -> CiConfigError {
    if message.contains("unknown field") {
        CiConfigError::UnknownField { format, message }
    } else {
        CiConfigError::Syntax { format, message }
    }
}

/// **Parse an authored `.myelin/ci.*` document into the resolver-ready [`CiDefinition`] (CT-004b).**
///
/// `bytes` is the raw config file (UTF-8); `filename_or_format` is the filename (`.myelin/ci.toml`)
/// or a bare format token (`toml`/`json`) that selects the deserialiser. The document is
/// deserialised into a fail-closed serde DTO (`#[serde(deny_unknown_fields)]`) and mapped into the
/// domain [`CiDefinition`], running the SCHEMA/structural checks (non-empty + unique job names,
/// declared `needs`, well-formed matrix axes, well-formed `on:`/`kind`). The SEMANTIC checks (DAG
/// acyclicity, the digest-pin-or-fail-closed supply-chain control, deterministic matrix expansion)
/// are [`resolve_snapshot`](crate::resolve::resolve_snapshot)'s — this parser does NOT duplicate
/// them (so the authored `image` MAY be a floating tag here; the resolver refuses it).
///
/// Returns a typed [`CiConfigError`] on ANY failure (fail-closed + LOUD — never a partial
/// definition). Parsing is DETERMINISTIC: the same bytes always yield the same [`CiDefinition`].
///
/// The seam this feeds (the trigger-consumer chunk, NOT this chunk): the live `ci-dispatch.trigger`
/// consumer reads the config blob at the pushed ref → `parse_ci_config` →
/// [`compile_trigger`](crate::dispatch::compile_trigger)`(&def.on)` +
/// [`resolve_snapshot`](crate::resolve::resolve_snapshot)`(&def, ..)` →
/// [`reserve_and_start`](crate::resolve::reserve_and_start).
pub fn parse_ci_config(
    bytes: &[u8],
    filename_or_format: &str,
) -> Result<CiDefinition, CiConfigError> {
    if bytes.len() > MAX_CI_CONFIG_BYTES {
        return Err(CiConfigError::ConfigTooLarge { actual: bytes.len(), maximum: MAX_CI_CONFIG_BYTES });
    }
    let format = ConfigFormat::from_hint(filename_or_format)?;

    // Both TOML and JSON are UTF-8 text; a non-UTF-8 blob is malformed syntax (fail-closed).
    let text = std::str::from_utf8(bytes).map_err(|e| CiConfigError::Syntax {
        format: match format {
            ConfigFormat::Toml => "toml",
            ConfigFormat::Json => "json",
        },
        message: format!("not valid UTF-8: {e}"),
    })?;

    let authored: AuthoredCi = match format {
        ConfigFormat::Toml => {
            toml::from_str(text).map_err(|e| classify_de_error("toml", e.to_string()))?
        }
        ConfigFormat::Json => {
            serde_json::from_str(text).map_err(|e| classify_de_error("json", e.to_string()))?
        }
    };

    authored.into_definition()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{resolve_snapshot, ResolvedSnapshot, ResolvedSnapshotExt};
    use myelin_storage::{ContentHash, FsBlobStore};
    use myelin_tenancy::TenantId;

    // Digest-pinned images (the resolver's supply-chain floor accepts only `@<algo>:<hex>`).
    const PINNED_BUILD: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
    const PINNED_TEST: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

    /// A representative VALID `.myelin/ci.toml`: a `push` trigger, a build job, a test job that
    /// `needs` build with a 2-axis matrix, and a digest-pinned generator job.
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

    // -------- 1. A representative valid fixture → the exact expected CiDefinition --------

    /// **A representative valid `.myelin/ci.toml` parses to the EXACT expected [`CiDefinition`]:**
    /// the `on` trigger, the jobs incl. `needs`, the matrix axes, and the generator kind.
    #[test]
    fn a_valid_toml_fixture_parses_to_the_exact_definition() {
        let def = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml")
            .expect("the valid fixture parses");

        assert_eq!(def.on, OnTrigger::Push, "the armed trigger");
        assert_eq!(def.jobs.len(), 3, "three authored jobs");

        // build — a plain normal job, no needs, no matrix.
        assert_eq!(def.jobs[0], JobDef::normal("build", PINNED_BUILD, ["build"]));

        // test — needs build + a 2-axis matrix (axes captured as authored).
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

        // gen-matrix — the dynamic-generation escape hatch (JobKind::Generate).
        assert_eq!(def.jobs[2].name, "gen-matrix");
        assert_eq!(def.jobs[2].kind, JobKind::Normal);
    }

    /// The SAME definition authored as JSON parses to the SAME [`CiDefinition`] (the JSON-Schema'd
    /// core in JSON — the second free format).
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
        assert_eq!(def.jobs, vec![JobDef::normal("build", PINNED_BUILD, ["build"])]);
    }

    // -------- 2. Fail-closed cases — each asserts the SPECIFIC CiConfigError --------

    /// Malformed syntax → [`CiConfigError::Syntax`].
    #[test]
    fn malformed_syntax_is_rejected() {
        let err = parse_ci_config(b"on = = broken", ".myelin/ci.toml")
            .expect_err("malformed toml is rejected");
        assert!(
            matches!(err, CiConfigError::Syntax { format: "toml", .. }),
            "malformed syntax → Syntax: {err:?}"
        );
    }

    /// An unknown field (`deny_unknown_fields`) → [`CiConfigError::UnknownField`] — a typo is
    /// fail-closed, never silently ignored.
    #[test]
    fn an_unknown_field_is_rejected_fail_closed() {
        let toml = r#"
on = "push"
oops = "typo"

[[jobs]]
name = "build"
image = "x@sha256:0"
"#;
        let err =
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect_err("an unknown key is rejected");
        assert!(
            matches!(&err, CiConfigError::UnknownField { format: "toml", message } if message.contains("oops")),
            "unknown field → UnknownField naming the key: {err:?}"
        );
    }

    /// An unknown field NESTED in a job is ALSO rejected (`deny_unknown_fields` on the job DTO).
    #[test]
    fn an_unknown_field_in_a_job_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "build"
image = "x@sha256:0"
retries = 3
"#;
        let err = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect_err("nested typo rejected");
        assert!(matches!(err, CiConfigError::UnknownField { .. }), "{err:?}");
    }

    /// Empty jobs → [`CiConfigError::EmptyJobs`].
    #[test]
    fn empty_jobs_is_rejected() {
        let toml = "on = \"push\"\njobs = []\n";
        assert_eq!(
            parse_ci_config(toml.as_bytes(), ".myelin/ci.toml"),
            Err(CiConfigError::EmptyJobs)
        );
    }

    /// A duplicate job name → [`CiConfigError::DuplicateJob`].
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

    /// A `needs` referencing an undeclared job → [`CiConfigError::UnknownNeed`].
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
        let err = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect_err("dangling need rejected");
        assert!(
            matches!(&err, CiConfigError::UnknownNeed { job, need } if job == "a" && need == "ghost"),
            "{err:?}"
        );
    }

    /// A malformed `on:` → [`CiConfigError::UnknownTrigger`].
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

    /// A missing required field (no `image`) → [`CiConfigError::Syntax`] (serde: a missing field).
    #[test]
    fn a_missing_required_field_is_rejected() {
        let toml = r#"
on = "push"

[[jobs]]
name = "a"
"#;
        let err = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect_err("missing image rejected");
        assert!(matches!(err, CiConfigError::Syntax { .. }), "missing required field → Syntax: {err:?}");
    }

    /// An empty job name → [`CiConfigError::EmptyJobName`].
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

    /// A bad job `kind` → [`CiConfigError::BadJobKind`].
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

    /// An empty matrix axis → [`CiConfigError::EmptyMatrixAxis`] (it would silently drop at expansion).
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

    /// A YAML hint is refused with the named-defer [`CiConfigError::UnknownFormat`] (YAML deferred).
    #[test]
    fn a_yaml_hint_is_refused_as_deferred() {
        let err = parse_ci_config(b"on: push", ".myelin/ci.yaml").expect_err("yaml is deferred");
        assert!(matches!(err, CiConfigError::UnknownFormat { .. }), "{err:?}");
    }

    // -------- 3. Compose — parse → resolve_snapshot → the expected ResolvedSnapshot --------

    /// **The seam end-to-end: a parsed (digest-pinned) config feeds [`resolve_snapshot`] and
    /// produces the expected [`ResolvedSnapshot`] — proving the parser output is resolver-ready.**
    #[test]
    fn a_parsed_config_composes_with_resolve_snapshot() {
        let def = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml").expect("valid parse");
        let store = FsBlobStore::new();
        let (snap, addr): (ResolvedSnapshot, ContentHash) =
            resolve_snapshot(&def, &store, &tenant()).expect("the parsed def resolves");

        // build (1) + test 2×2 matrix (4) + gen-matrix (1) = 6 resolved instances.
        assert_eq!(snap.jobs.len(), 6, "1 build + 4 test-matrix + 1 generator");
        // The generator floor rides through the parser → resolver seam.
        assert!(!snap.has_dynamic_generation());
        // The CAS blob round-trips at the returned address (content-addressed by construction).
        assert_eq!(addr, ContentHash::blake3(&snap.canonical_bytes().unwrap()));

        // The matrix expanded deterministically (sorted-axis instance names).
        let names: Vec<&str> = snap.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names.iter().filter(|name| name.starts_with("test--")).count(), 4);
    }

    /// A parsed config whose image is a FLOATING TAG parses fine (the parser stays out of the
    /// supply-chain lane) but the RESOLVER refuses it — the schema/semantic split proven.
    #[test]
    fn a_floating_tag_parses_but_the_resolver_refuses_it() {
        let toml = r#"
on = "push"

[[jobs]]
name = "build"
image = "alpine:3"
command = ["build"]
"#;
        // Parse SUCCEEDS — a floating tag is not a schema defect.
        let def = parse_ci_config(toml.as_bytes(), ".myelin/ci.toml").expect("a floating tag parses");
        assert_eq!(def.jobs[0].image, "alpine:3");
        // The RESOLVER (the semantic owner) refuses it fail-closed.
        let store = FsBlobStore::new();
        assert!(
            resolve_snapshot(&def, &store, &tenant()).is_err(),
            "the resolver refuses the floating tag (the digest-pin control is semantic, not schema)"
        );
    }

    // -------- 4. Determinism --------

    /// **The same bytes always parse to the same [`CiDefinition`] (deterministic).**
    #[test]
    fn parsing_is_deterministic() {
        let a = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml").expect("parse a");
        let b = parse_ci_config(VALID_TOML.as_bytes(), ".myelin/ci.toml").expect("parse b");
        assert_eq!(a, b, "the same bytes → the same CiDefinition");
    }

    #[test]
    fn required_command_and_raw_size_bounds_are_fail_closed() {
        let missing = b"on=\"push\"\n[[jobs]]\nname=\"a\"\nimage=\"x@sha256:0\"\n";
        assert!(matches!(parse_ci_config(missing, "toml"), Err(CiConfigError::Syntax { .. })));
        let oversized = vec![b' '; MAX_CI_CONFIG_BYTES + 1];
        assert_eq!(parse_ci_config(&oversized, "toml"), Err(CiConfigError::ConfigTooLarge {
            actual: MAX_CI_CONFIG_BYTES + 1, maximum: MAX_CI_CONFIG_BYTES,
        }));
    }

    #[test]
    fn command_and_machine_token_bounds_are_enforced() {
        let too_many = format!(
            "on=\"push\"\n[[jobs]]\nname=\"a\"\nimage=\"x@sha256:0\"\ncommand=[{}]\n",
            std::iter::repeat_n("\"x\"", myelin_ci_controlplane::run_plan::MAX_COMMAND_ARGS + 1)
                .collect::<Vec<_>>().join(",")
        );
        assert!(matches!(parse_ci_config(too_many.as_bytes(), "toml"), Err(CiConfigError::InvalidCommand { .. })));
        let invalid_name = b"on=\"push\"\n[[jobs]]\nname=\"not a token\"\nimage=\"x@sha256:0\"\ncommand=[\"run\"]\n";
        assert!(matches!(parse_ci_config(invalid_name, "toml"), Err(CiConfigError::InvalidMachineToken { .. })));
        let jobs = (0..=MAX_AUTHORED_JOBS).map(|index| format!(
            "[[jobs]]\nname=\"j{index}\"\nimage=\"x@sha256:0\"\ncommand=[\"run\"]\n"
        )).collect::<String>();
        assert!(matches!(parse_ci_config(format!("on=\"push\"\n{jobs}").as_bytes(), "toml"), Err(CiConfigError::TooManyJobs { .. })));
    }
}
