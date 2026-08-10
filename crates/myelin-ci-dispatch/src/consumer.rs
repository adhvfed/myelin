use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(any(test, feature = "test-support", feature = "integration"))]
use myelin_events::OutboxStore;
use myelin_events::{
    Actor, EmitContextBase, EventEnvelope, EventHandler, EventId, HandleOutcome, IdMinter,
    OutboxTransaction, SubjectPattern, UpcasterRegistry,
};
use myelin_storage::BlobStore;
use myelin_tenancy::TenantId;

use crate::config::{parse_versioned_ci_config, CiConfigError, ConfigFormat};
use crate::dispatch::{
    compile_trigger, stamp_trust, OnTrigger, RunProvenance, TrustStamp, TRIGGER_CONSUMER,
};
use crate::resolve::{
    reserve_and_start, resolve_versioned_snapshot_with_cargo_vendor, snapshot_ref, CheckContext,
    ResolveError, RunFacts, StartHandoff, VersionedCiDefinition,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoritativeGitRoot(std::path::PathBuf);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRootError(pub String);

impl std::fmt::Display for GitRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "authoritative git root is unavailable: {}", self.0)
    }
}

impl std::error::Error for GitRootError {}

impl AuthoritativeGitRoot {
    pub fn validate(path: impl AsRef<std::path::Path>) -> Result<Self, GitRootError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(GitRootError(format!(
                "{} is not an absolute path",
                path.display()
            )));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| GitRootError(format!("{}: {error}", path.display())))?;
        if !canonical.is_dir() {
            return Err(GitRootError(format!(
                "{} is not a directory",
                canonical.display()
            )));
        }
        std::fs::read_dir(&canonical).map_err(|error| {
            GitRootError(format!("{} is not readable: {error}", canonical.display()))
        })?;
        Ok(Self(canonical))
    }

    pub fn as_path(&self) -> &std::path::Path {
        &self.0
    }
}

pub fn ci_trigger_subjects() -> &'static [SubjectPattern] {
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            CI_TRIGGER_SUBJECT_STRS
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect()
        })
        .as_slice()
}

pub const CI_TRIGGER_SUBJECT_STRS: &[&str] = &["myelin://"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitReadError {
    Unavailable(String),
    Invalid(String),
}

impl GitReadError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, GitReadError::Unavailable(_))
    }
}

impl std::fmt::Display for GitReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitReadError::Unavailable(message) => {
                write!(f, "git config backing unavailable: {message}")
            }
            GitReadError::Invalid(message) => write!(f, "invalid git config read: {message}"),
        }
    }
}

impl std::error::Error for GitReadError {}

pub trait GitConfigReader: Send + Sync {
    fn read_repo_file(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError>;
    fn read_repo_file_bounded(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        let bytes = self.read_repo_file(tenant, region, repo, oid, path)?;
        if bytes
            .as_ref()
            .is_some_and(|bytes| bytes.len() > maximum_bytes)
        {
            return Err(GitReadError::Invalid(format!(
                "{path}@{oid} exceeds the {maximum_bytes}-byte config limit"
            )));
        }
        Ok(bytes)
    }
}

const CI_CONFIG_CANDIDATES: &[(&str, ConfigFormat)] = &[
    (".myelin/ci.toml", ConfigFormat::Toml),
    (".myelin/ci.json", ConfigFormat::Json),
];

const ROOT_CARGO_LOCK_PATH: &str = "Cargo.lock";

const MAX_CARGO_LOCK_BYTES: usize = 8 * 1024 * 1024;

pub fn resolve_ci_config(
    reader: &dyn GitConfigReader,
    tenant: &str,
    region: &str,
    repo: &str,
    oid: &str,
) -> Result<Option<(Vec<u8>, ConfigFormat)>, GitReadError> {
    for (path, format) in CI_CONFIG_CANDIDATES {
        if let Some(bytes) = reader.read_repo_file_bounded(
            tenant,
            region,
            repo,
            oid,
            path,
            crate::config::MAX_CI_CONFIG_BYTES,
        )? {
            return Ok(Some((bytes, *format)));
        }
    }
    Ok(None)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveFacts {
    pub region: String,
    pub project_id: String,
    pub pipeline_id: String,
    pub wf_run_id: String,
    pub correlation_id: String,
    pub repo_ref: String,
    pub source_ref: Option<String>,
    pub commit_oid: String,
    pub concurrency_group: Option<String>,
    pub pr_head_generation: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ArmedRun {
    pub handoff: StartHandoff,
    pub reserve: ReserveFacts,
    pub tenant: TenantId,
    pub actor: Actor,
    pub emit_ctx: EmitContextBase,
    pub cause: EventEnvelope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveError(pub String);

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "reserve/start persistence failed: {}", self.0)
    }
}

impl std::error::Error for ReserveError {}

pub trait ReserveStore: Send + Sync {
    fn persist(
        &self,
        armed: &ArmedRun,
        tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError>;
}

#[cfg(any(test, feature = "test-support", feature = "integration"))]
pub struct OutboxReserveStore {
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
}

#[cfg(any(test, feature = "test-support", feature = "integration"))]
impl OutboxReserveStore {
    pub fn new(outbox: OutboxStore, minter: Arc<dyn IdMinter>) -> OutboxReserveStore {
        OutboxReserveStore { outbox, minter }
    }
}

#[cfg(any(test, feature = "test-support", feature = "integration"))]
impl ReserveStore for OutboxReserveStore {
    fn persist(
        &self,
        armed: &ArmedRun,
        _tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError> {
        let mut tx = self
            .outbox
            .begin(Arc::clone(&self.minter), armed.emit_ctx.clone());
        tx.stage_state_change(format!(
            "ci_run {} reserved (queued) - the durable ROW co-commit is CoCommitReserveStore (chunk 4)",
            armed.handoff.run_write.run_id
        ));
        let attempts = queued_contexts(armed)?
            .into_iter()
            .map(|context| (context, 1))
            .collect();
        emit_reserve_events(&mut tx, armed, &attempts)?;
        tx.commit_absorb()
            .map_err(|e| ReserveError(format!("outbox commit_absorb: {e:?}")))
    }
}

fn emit_reserve_events(
    tx: &mut myelin_events::OutboxTransaction,
    armed: &ArmedRun,
    attempts: &BTreeMap<String, u32>,
) -> Result<(), ReserveError> {
    let started_id = EventId(deterministic_uuid(&format!(
        "evt:{}",
        armed.handoff.run_started.subject.0
    )));
    tx.emit_with_id(
        started_id,
        armed.handoff.run_started.clone(),
        Some(&armed.cause),
    )
    .map_err(|e| ReserveError(format!("ci.run.started emit: {e:?}")))?;
    for check in &armed.handoff.queued_checks {
        let context = queued_context(check)?;
        let attempt = attempts.get(&context).copied().ok_or_else(|| {
            ReserveError(format!(
                "queued check {context:?} lacks an allocated attempt"
            ))
        })?;
        let mut check = check.clone();
        check.payload["run_attempt"] = serde_json::json!(attempt);
        let check_id = check_event_id(&armed.handoff.run_write.run_id, &check.subject.0);
        tx.emit_with_id(check_id, check, Some(&armed.cause))
            .map_err(|e| ReserveError(format!("queued ci.check.updated emit: {e:?}")))?;
    }
    Ok(())
}

pub fn ci_run_insert_from_armed(armed: &ArmedRun) -> myelin_ci_controlplane::CiRunInsert {
    let rw = &armed.handoff.run_write;
    myelin_ci_controlplane::CiRunInsert {
        tenant_id: armed.tenant.0.clone(),
        region: armed.reserve.region.clone(),
        run_id: rw.run_id.clone(),
        project_id: armed.reserve.project_id.clone(),
        pipeline_id: armed.reserve.pipeline_id.clone(),
        wf_run_id: armed.reserve.wf_run_id.clone(),
        definition_snapshot: rw.definition_snapshot.0.clone(),
        trigger_kind: rw.trigger_kind.clone(),
        concurrency_group: armed.reserve.concurrency_group.clone(),
        pr_head_generation: armed.reserve.pr_head_generation,
        trust_tier: rw.trust_tier.clone(),
        state: rw.state.clone(),
        correlation_id: armed.reserve.correlation_id.clone(),
        cause_event_id: Some(rw.cause_event_id.clone()),
        cause_depth: i64::from(armed.cause.depth),
        caused_by: armed.cause.caused_by.as_ref().map(|value| value.0.clone()),
        repo_ref: Some(armed.reserve.repo_ref.clone()),
        source_ref: armed.reserve.source_ref.clone(),
        commit_oid: Some(armed.reserve.commit_oid.clone()),
        triggered_by: Some(armed.actor.0.principal_id.0.clone()),
    }
}

pub struct CoCommitReserveStore {
    ci_run: myelin_ci_controlplane::CiRunStore,
    minter: Arc<dyn IdMinter>,
    rt: tokio::runtime::Handle,
}

impl CoCommitReserveStore {
    pub fn new(
        ci_run: myelin_ci_controlplane::CiRunStore,
        minter: Arc<dyn IdMinter>,
        rt: tokio::runtime::Handle,
    ) -> CoCommitReserveStore {
        CoCommitReserveStore { ci_run, minter, rt }
    }
}

impl ReserveStore for CoCommitReserveStore {
    fn persist(
        &self,
        armed: &ArmedRun,
        tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError> {
        let row = ci_run_insert_from_armed(armed);
        let contexts = queued_contexts(armed)?;
        let minter = Arc::clone(&self.minter);
        self.ci_run
            .co_commit_reserve(tx, &row, &contexts, &self.rt, |attempts| {
                stage_reserve_events(armed, attempts, minter)
            })
            .map(|_| ())
            .map_err(|e| ReserveError(format!("reserve co-commit: {e}")))
    }
}

fn queued_contexts(armed: &ArmedRun) -> Result<BTreeSet<String>, ReserveError> {
    armed
        .handoff
        .queued_checks
        .iter()
        .map(queued_context)
        .collect()
}

fn queued_context(check: &myelin_events::EventDraft) -> Result<String, ReserveError> {
    let context = check
        .payload
        .get("context")
        .and_then(|value| value.as_object())
        .ok_or_else(|| ReserveError("queued check lacks context".into()))?;
    if context.get("provider").and_then(|value| value.as_str()) != Some("ci") {
        return Err(ReserveError(
            "queued check is outside the CI provider namespace".into(),
        ));
    }
    context
        .get("name")
        .and_then(|value| value.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ReserveError("queued check lacks context name".into()))
}

fn stage_reserve_events(
    armed: &ArmedRun,
    attempts: &BTreeMap<String, u32>,
    minter: Arc<dyn IdMinter>,
) -> Result<Vec<myelin_events::OutboxRow>, String> {
    let mut tx = OutboxTransaction::detached(minter, armed.emit_ctx.clone());
    emit_reserve_events(&mut tx, armed, attempts).map_err(|error| error.to_string())?;
    tx.into_staged_rows().map_err(|error| error.0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    NotATrigger(String),
    MalformedPayload(String),
    InvalidProvenance(String),
    ReadFailed(GitReadError),
    NoConfig,
    ConfigError(CiConfigError),
    TriggerNotMatched,
    ResolveError(ResolveError),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::NotATrigger(t) => write!(f, "not a CI trigger event: `{t}`"),
            SkipReason::MalformedPayload(m) => write!(f, "malformed trigger payload: {m}"),
            SkipReason::InvalidProvenance(m) => write!(f, "invalid trigger provenance: {m}"),
            SkipReason::ReadFailed(e) => write!(f, "{e}"),
            SkipReason::NoConfig => {
                write!(f, "no `.myelin/ci.*` at the pushed ref - no pipeline armed")
            }
            SkipReason::ConfigError(e) => write!(f, "malformed `.myelin/ci.*` (fail-closed): {e}"),
            SkipReason::TriggerNotMatched => {
                write!(f, "the armed trigger does not match this event - no run")
            }
            SkipReason::ResolveError(e) => {
                write!(f, "the definition failed to resolve (fail-closed): {e}")
            }
        }
    }
}

#[derive(Debug)]
pub enum DispatchOutcome {
    Arm(Box<ArmedRun>),
    Skip(SkipReason),
}

struct TriggerFacts {
    repo: String,
    new_oid: String,
    is_fork: bool,
    source_ref: Option<String>,
    concurrency_group: Option<String>,
    pr_head_generation: Option<i64>,
}

fn trigger_facts(ev: &EventEnvelope) -> Result<TriggerFacts, SkipReason> {
    let p = &ev.payload;
    let repo = p
        .get("repo")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SkipReason::MalformedPayload("missing `repo`".into()))?
        .to_string();
    myelin_git::gix_backend::validate_repo_slug(&repo).map_err(|error| {
        SkipReason::InvalidProvenance(format!("invalid payload repository {repo:?}: {error}"))
    })?;

    let (oid_field, source_ref, concurrency_group, pr_head_generation) =
        if ev.type_.0 == myelin_git::events::GIT_REF_UPDATED {
            let ref_name = p
                .get("ref")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| SkipReason::MalformedPayload("missing `ref`".into()))?;
            let ref_name = myelin_git::receive_pack::RefName::new(ref_name);
            let ref_key =
                myelin_git::receive_pack::GitRefEventKey::new(&repo, &ref_name).map_err(|_| {
                    SkipReason::InvalidProvenance("invalid canonical Git ref event key".into())
                })?;
            validate_envelope_provenance(
                ev,
                &ref_key
                    .subject(&ev.tenant.0)
                    .map_err(|_| {
                        SkipReason::InvalidProvenance("invalid canonical Git ref subject".into())
                    })?
                    .0,
                &ref_key.aggregate().0,
            )?;
            ("new_oid", Some(ref_name.0.clone()), None, None)
        } else {
            let number = p
                .get("number")
                .and_then(|v| v.as_u64())
                .filter(|number| *number > 0)
                .ok_or_else(|| {
                    SkipReason::InvalidProvenance("PR `number` must be a positive integer".into())
                })?;
            validate_envelope_provenance(
                ev,
                &format!("myelin://{}/git/pr/{repo}:{number}", ev.tenant.0),
                &format!("git/pr/{repo}:{number}"),
            )?;
            let group = format!("pr:{repo}:{number}");
            if group.len() > 512 {
                return Err(SkipReason::InvalidProvenance(
                    "PR concurrency identity exceeds 512 bytes".into(),
                ));
            }
            if ev.schema_ver < myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2 {
                return Err(SkipReason::InvalidProvenance(
                    "PR head-trigger event did not pass the required schema upcaster".into(),
                ));
            }
            let generation = p
                .get("head_generation")
                .and_then(|value| value.as_i64())
                .filter(|generation| *generation > 0)
                .ok_or_else(|| {
                    SkipReason::InvalidProvenance(
                        "PR `head_generation` must be a positive signed 64-bit integer".into(),
                    )
                })?;
            ("head_oid", None, Some(group), Some(generation))
        };

    let raw_oid = p
        .get(oid_field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SkipReason::MalformedPayload(format!("missing `{oid_field}`")))?;
    let new_oid = canonical_commit_oid(raw_oid)?;
    let requires_fork_evidence = ev.type_.0 == myelin_git::events::GIT_PR_OPENED
        || ev.type_.0 == myelin_git::events::GIT_PR_SYNCHRONIZED;
    let is_fork = parse_fork_evidence(p, requires_fork_evidence)?;
    Ok(TriggerFacts {
        repo,
        new_oid,
        is_fork,
        source_ref,
        concurrency_group,
        pr_head_generation,
    })
}

fn validate_envelope_provenance(
    ev: &EventEnvelope,
    expected_subject: &str,
    expected_aggregate: &str,
) -> Result<(), SkipReason> {
    if ev.subject.0 != expected_subject {
        return Err(SkipReason::InvalidProvenance(format!(
            "subject/payload provenance mismatch: expected {expected_subject:?}, got {:?}",
            ev.subject.0
        )));
    }
    if ev.aggregate.0 != expected_aggregate {
        return Err(SkipReason::InvalidProvenance(format!(
            "aggregate/payload provenance mismatch: expected {expected_aggregate:?}, got {:?}",
            ev.aggregate.0
        )));
    }
    Ok(())
}

fn canonical_commit_oid(raw: &str) -> Result<String, SkipReason> {
    if raw.len() != 40 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SkipReason::ReadFailed(GitReadError::Invalid(
            "commit oid must be exactly 40 hexadecimal characters; revspecs and abbreviated ids are refused".into(),
        )));
    }
    Ok(raw.to_ascii_lowercase())
}

fn parse_fork_evidence(payload: &serde_json::Value, required: bool) -> Result<bool, SkipReason> {
    let canonical = payload.get("is_fork");
    let legacy = payload.get("forked");
    let parse = |name: &str, value: &serde_json::Value| {
        value
            .as_bool()
            .ok_or_else(|| SkipReason::MalformedPayload(format!("`{name}` must be a boolean")))
    };

    match (canonical, legacy) {
        (None, None) if required => Err(SkipReason::MalformedPayload(
            "missing explicit boolean fork evidence (`is_fork` or legacy `forked`)".into(),
        )),
        (None, None) => Ok(false),
        (Some(value), None) => parse("is_fork", value),
        (None, Some(value)) => parse("forked", value),
        (Some(canonical), Some(legacy)) => {
            let canonical = parse("is_fork", canonical)?;
            let legacy = parse("forked", legacy)?;
            if canonical != legacy {
                return Err(SkipReason::MalformedPayload(
                    "conflicting fork evidence: `is_fork` and `forked` must agree".into(),
                ));
            }
            Ok(canonical)
        }
    }
}

fn on_trigger_for_type(event_type: &str) -> Option<OnTrigger> {
    if event_type == myelin_git::events::GIT_REF_UPDATED {
        Some(OnTrigger::Push)
    } else if event_type == myelin_git::events::GIT_PR_OPENED
        || event_type == myelin_git::events::GIT_PR_SYNCHRONIZED
    {
        Some(OnTrigger::PullRequest)
    } else {
        None
    }
}

pub fn check_event_id(run_id: &str, check_subject: &str) -> EventId {
    EventId(deterministic_uuid(&format!("evt:{run_id}:{check_subject}")))
}

pub fn deterministic_uuid(seed: &str) -> String {
    let fill = |salt: u64| -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
        for b in seed.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let a = fill(0);
    let b = fill(0x00ff_00ff_00ff_00ff);
    let bytes = [a.to_be_bytes(), b.to_be_bytes()].concat();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

pub fn plan_dispatch(
    ev: &EventEnvelope,
    reader: &dyn GitConfigReader,
    blobs: &dyn BlobStore,
) -> DispatchOutcome {
    let Some(on) = on_trigger_for_type(&ev.type_.0) else {
        return DispatchOutcome::Skip(SkipReason::NotATrigger(ev.type_.0.clone()));
    };

    let facts = match trigger_facts(ev) {
        Ok(f) => f,
        Err(reason) => return DispatchOutcome::Skip(reason),
    };

    let tenant = ev.tenant.clone();
    let region = ev.region.0.clone();

    let config = match resolve_ci_config(reader, &tenant.0, &region, &facts.repo, &facts.new_oid) {
        Ok(Some(c)) => c,
        Ok(None) => return DispatchOutcome::Skip(SkipReason::NoConfig),
        Err(e) => return DispatchOutcome::Skip(SkipReason::ReadFailed(e)),
    };
    let (bytes, format) = config;

    let format_hint = match format {
        ConfigFormat::Toml => ".myelin/ci.toml",
        ConfigFormat::Json => ".myelin/ci.json",
    };
    let def: VersionedCiDefinition = match parse_versioned_ci_config(&bytes, format_hint) {
        Ok(d) => d,
        Err(e) => return DispatchOutcome::Skip(SkipReason::ConfigError(e.into_legacy_surface())),
    };

    if let Err(e) = compile_trigger(&def.on) {
        return DispatchOutcome::Skip(SkipReason::MalformedPayload(format!(
            "trigger compile: {e}"
        )));
    }
    if def.on != on || !def.on.event_types().contains(&ev.type_.0.as_str()) {
        return DispatchOutcome::Skip(SkipReason::TriggerNotMatched);
    }

    let provenance = RunProvenance {
        is_fork: facts.is_fork,
        targets_self_hosted: false,
        read_excludes_fork: !facts.is_fork,
    };
    let stamp: TrustStamp = stamp_trust(&provenance);

    let root_cargo_lock: Option<Vec<u8>> = if def.jobs.iter().any(|job| job.build.is_some()) {
        match reader.read_repo_file_bounded(
            &tenant.0,
            &region,
            &facts.repo,
            &facts.new_oid,
            ROOT_CARGO_LOCK_PATH,
            MAX_CARGO_LOCK_BYTES,
        ) {
            Ok(bytes) => bytes,
            Err(e) => return DispatchOutcome::Skip(SkipReason::ReadFailed(e)),
        }
    } else {
        None
    };
    let (_snapshot, address) = match resolve_versioned_snapshot_with_cargo_vendor(
        &def,
        blobs,
        &tenant,
        root_cargo_lock.as_deref(),
    ) {
        Ok(r) => r,
        Err(e) => return DispatchOutcome::Skip(SkipReason::ResolveError(e)),
    };
    let snapshot = snapshot_ref(&tenant, &address);

    let run_id = deterministic_uuid(&format!("run:{}", ev.event_id.0));
    let wf_run_id = deterministic_uuid(&format!("wf:{}", ev.event_id.0));
    let contexts: Vec<CheckContext> = def
        .jobs
        .iter()
        .map(|j| CheckContext::ci(myelin_ci_controlplane::ci_check_context_v1(&j.name)))
        .collect();
    let repo_ref = format!("myelin://{}/git/repo/{}", tenant.0, facts.repo);
    let run_facts = RunFacts {
        run_id: run_id.clone(),
        tenant_id: tenant.0.clone(),
        repo_ref: repo_ref.clone(),
        source_ref: facts.source_ref.clone(),
        commit_oid: facts.new_oid.clone(),
        contexts,
        cause_event_id: ev.event_id.clone(),
        started_at: ev.occurred_at.0.clone(),
    };
    let handoff = reserve_and_start(&snapshot, &stamp, &def.on, &run_facts);

    let reserve = ReserveFacts {
        region: region.clone(),
        project_id: deterministic_uuid(&format!("project:{}", facts.repo)),
        pipeline_id: deterministic_uuid(&format!("pipeline:{}", facts.repo)),
        wf_run_id,
        correlation_id: ev.correlation_id.0.clone(),
        repo_ref,
        source_ref: facts.source_ref,
        commit_oid: facts.new_oid,
        concurrency_group: facts.concurrency_group,
        pr_head_generation: facts.pr_head_generation,
    };
    let emit_ctx = EmitContextBase {
        tenant: tenant.clone(),
        region: ev.region.clone(),
        actor: ev.actor.clone(),
        schema_ver: 1,
        occurred_at: ev.occurred_at.clone(),
        recorded_at: ev.recorded_at.clone(),
        caused_by: None,
    };

    DispatchOutcome::Arm(Box::new(ArmedRun {
        handoff,
        reserve,
        tenant,
        actor: ev.actor.clone(),
        emit_ctx,
        cause: ev.clone(),
    }))
}

pub struct CiTriggerHandler {
    reader: Arc<dyn GitConfigReader>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    reserve: Arc<dyn ReserveStore>,
    expected_region: Option<String>,
    trace: Mutex<Vec<String>>,
}

impl CiTriggerHandler {
    pub fn new(
        reader: Arc<dyn GitConfigReader>,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        reserve: Arc<dyn ReserveStore>,
    ) -> CiTriggerHandler {
        CiTriggerHandler {
            reader,
            blobs,
            reserve,
            expected_region: None,
            trace: Mutex::new(Vec::new()),
        }
    }

    pub fn for_region(
        reader: Arc<dyn GitConfigReader>,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        reserve: Arc<dyn ReserveStore>,
        expected_region: impl Into<String>,
    ) -> CiTriggerHandler {
        CiTriggerHandler {
            reader,
            blobs,
            reserve,
            expected_region: Some(expected_region.into()),
            trace: Mutex::new(Vec::new()),
        }
    }

    pub fn consumer_name(&self) -> &'static str {
        TRIGGER_CONSUMER
    }

    pub fn trace(&self) -> Vec<String> {
        self.trace.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record(&self, line: String) {
        let mut t = self.trace.lock().unwrap_or_else(|e| e.into_inner());
        if t.len() >= 256 {
            t.remove(0);
        }
        t.push(line);
    }
}

pub fn build_trigger_consumer(
    reader: Arc<dyn GitConfigReader>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    reserve: Arc<dyn ReserveStore>,
    dedup: myelin_events::DedupLedger,
    expected_region: impl Into<String>,
    dead_letters: Arc<dyn myelin_events::DurableDeadLetter>,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    let handler = CiTriggerHandler::for_region(reader, blobs, reserve, expected_region);
    let subscription = myelin_events::consumer::Subscription::bind(
        myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
        CI_TRIGGER_SUBJECT_STRS,
        myelin_events::PrefetchBound::DEFAULT,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(
        myelin_events::Consumer::new(handler, subscription, dedup)
            .with_upcaster(pr_trigger_upcasters().into_hook())
            .with_dead_letter_sink(myelin_events::DeadLetterSink::durable(dead_letters)),
    ))
}

fn pr_trigger_upcasters() -> UpcasterRegistry {
    let mut registry = UpcasterRegistry::new();
    registry
        .register(
            myelin_events::EventType(myelin_git::events::GIT_PR_OPENED.into()),
            1,
            myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2,
            |mut event| {
                if let Some(payload) = event.payload.as_object_mut() {
                    payload.insert("head_generation".into(), serde_json::json!(1));
                }
                event.schema_ver = myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2;
                event
            },
        )
        .expect("git.pr.opened has one static adjacent schema hop");
    registry
        .register(
            myelin_events::EventType(myelin_git::events::GIT_PR_SYNCHRONIZED.into()),
            1,
            myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2,
            |mut event| {
                event.schema_ver = myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2;
                event
            },
        )
        .expect("git.pr.synchronized has one static adjacent schema hop");
    registry
}

impl EventHandler for CiTriggerHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        ci_trigger_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if let Some(expected) = &self.expected_region {
            if ev.region.0 != *expected {
                self.record(format!(
                    "region mismatch: envelope={} configured_cell={expected}",
                    ev.region.0
                ));
                return HandleOutcome::NonRetryable(myelin_events::Reason(format!(
                    "event region {} does not match configured cell region {expected}",
                    ev.region.0
                )));
            }
        }
        match plan_dispatch(ev, self.reader.as_ref(), self.blobs.as_ref()) {
            DispatchOutcome::Arm(armed) => match self.reserve.persist(&armed, tx) {
                Ok(()) => {
                    self.record(format!(
                        "armed run_id={} repo={} snapshot={}",
                        armed.handoff.run_write.run_id,
                        armed.reserve.correlation_id,
                        armed.handoff.run_write.definition_snapshot.0
                    ));
                    HandleOutcome::Done
                }
                Err(e) => {
                    self.record(format!("reserve FAILED (retry): {e}"));
                    HandleOutcome::Retry(myelin_events::Backoff { seconds: 5 })
                }
            },
            DispatchOutcome::Skip(reason) => {
                match &reason {
                    SkipReason::NotATrigger(_) | SkipReason::NoConfig => {}
                    other => self.record(format!("skip: {other}")),
                }
                if matches!(&reason, SkipReason::ReadFailed(error) if error.is_retryable()) {
                    HandleOutcome::Retry(myelin_events::Backoff { seconds: 5 })
                } else if matches!(reason, SkipReason::InvalidProvenance(_)) {
                    HandleOutcome::NonRetryable(myelin_events::Reason(
                        "invalid trigger provenance".into(),
                    ))
                } else if let SkipReason::ReadFailed(error) = reason {
                    HandleOutcome::NonRetryable(myelin_events::Reason(error.to_string()))
                } else if matches!(
                    reason,
                    SkipReason::ResolveError(ResolveError::BlobWrite(
                        myelin_storage::BlobError::Backend(_)
                    ))
                ) {
                    HandleOutcome::DependencyUnavailable {
                        dependency: myelin_events::relay::IntakeDependency::Blob,
                        backoff: myelin_events::Backoff { seconds: 5 },
                    }
                } else if matches!(reason, SkipReason::ResolveError(ResolveError::BlobWrite(_))) {
                    HandleOutcome::Retry(myelin_events::Backoff { seconds: 5 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct MapGitConfigReader {
    files: std::collections::HashMap<(String, String, String), Vec<u8>>,
    fail: std::collections::HashSet<(String, String, String)>,
}

#[cfg(any(test, feature = "test-support"))]
impl MapGitConfigReader {
    pub fn new() -> MapGitConfigReader {
        MapGitConfigReader::default()
    }

    pub fn with_file(
        mut self,
        repo: &str,
        oid: &str,
        path: &str,
        bytes: impl Into<Vec<u8>>,
    ) -> MapGitConfigReader {
        self.files
            .insert((repo.into(), oid.into(), path.into()), bytes.into());
        self
    }

    pub fn with_failure(mut self, repo: &str, oid: &str, path: &str) -> MapGitConfigReader {
        self.fail.insert((repo.into(), oid.into(), path.into()));
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl GitConfigReader for MapGitConfigReader {
    fn read_repo_file(
        &self,
        _tenant: &str,
        _region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        let key = (repo.to_string(), oid.to_string(), path.to_string());
        if self.fail.contains(&key) {
            return Err(GitReadError::Unavailable(format!(
                "injected read failure at {path}"
            )));
        }
        Ok(self.files.get(&key).cloned())
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct RecordingReserveStore {
    persisted: Mutex<Vec<ArmedRun>>,
}

#[cfg(any(test, feature = "test-support"))]
impl RecordingReserveStore {
    pub fn new() -> RecordingReserveStore {
        RecordingReserveStore::default()
    }

    pub fn persisted(&self) -> Vec<ArmedRun> {
        self.persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ReserveStore for RecordingReserveStore {
    fn persist(
        &self,
        armed: &ArmedRun,
        _tx: &mut myelin_events::HandlerTx<'_>,
    ) -> Result<(), ReserveError> {
        self.persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(armed.clone());
        Ok(())
    }
}

pub struct DurableGitConfigReader<P: myelin_git::gix_backend::RepoPathResolver + Send + Sync> {
    store: myelin_git::durable::DurableGitStore<P>,
}

impl<P: myelin_git::gix_backend::RepoPathResolver + Send + Sync> DurableGitConfigReader<P> {
    pub fn new(store: myelin_git::durable::DurableGitStore<P>) -> DurableGitConfigReader<P> {
        DurableGitConfigReader { store }
    }
}

impl<P: myelin_git::gix_backend::RepoPathResolver + Send + Sync> GitConfigReader
    for DurableGitConfigReader<P>
{
    fn read_repo_file(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        self.read_repo_file_bounded(tenant, region, repo, oid, path, usize::MAX)
    }

    fn read_repo_file_bounded(
        &self,
        tenant: &str,
        region: &str,
        repo: &str,
        oid: &str,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        let loc = myelin_git::core::RepoLoc::new(tenant, region, repo);
        if !self.store.repo_exists(&loc) {
            return Err(GitReadError::Unavailable(format!(
                "repository {repo} is unavailable for tenant={tenant} region={region}"
            )));
        }
        let git = self
            .store
            .open_repo(&loc)
            .map_err(|e| GitReadError::Unavailable(format!("open {repo}: {e}")))?;
        let oid = myelin_git::core::Oid::new(oid);
        match git
            .read_blob_at_commit_oid_bounded(&oid, path, maximum_bytes)
            .map_err(|e| GitReadError::Unavailable(format!("read {path}@{}: {e}", oid.as_str())))?
        {
            myelin_git::durable::BlobPathLookup::Found { bytes, .. } => Ok(Some(bytes)),
            myelin_git::durable::BlobPathLookup::TooLarge { size, maximum, .. } => {
                Err(GitReadError::Invalid(format!(
                    "{path}@{} is {size} bytes, above the {maximum}-byte config limit",
                    oid.as_str()
                )))
            }
            myelin_git::durable::BlobPathLookup::Missing
            | myelin_git::durable::BlobPathLookup::IsDir => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::TrustTier;
    use myelin_events::{
        consumer::{Consumer, ConsumerName, Delivered, Message, PrefetchBound, Subscription},
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, EventId, EventType,
        Timestamp, Visibility,
    };
    use myelin_git::events::{
        GIT_PR_HEAD_TRIGGER_SCHEMA_V2, GIT_PR_OPENED, GIT_PR_SYNCHRONIZED, GIT_REF_UPDATED,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::{ContentHash, FsBlobStore};
    use myelin_tenancy::{Region, TenantId};

    const TEST_OID: &str = "dddddddddddddddddddddddddddddddddddddddd";

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("pusher".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn push_envelope(repo: &str, new_oid: &str) -> EventEnvelope {
        let ref_key = myelin_git::receive_pack::GitRefEventKey::new(
            repo,
            &myelin_git::receive_pack::RefName::new("refs/heads/main"),
        )
        .unwrap();
        EventEnvelope {
            event_id: EventId("ev-push-1".into()),
            type_: EventType(GIT_REF_UPDATED.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ref_key.subject("acme").unwrap(),
            aggregate: ref_key.aggregate(),
            causation_id: None,
            correlation_id: CorrelationId("corr-1".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-07-16T00:00:00Z".into()),
            recorded_at: Timestamp("2026-07-16T00:00:00Z".into()),
            payload: serde_json::json!({
                "repo": repo,
                "ref": "refs/heads/main",
                "new_oid": new_oid,
                "old_oid": "0000000000000000000000000000000000000000",
                "forced": false,
            }),
        }
    }

    fn pr_envelope(event_type: &str, fork_fields: serde_json::Value) -> EventEnvelope {
        let mut ev = push_envelope("web", TEST_OID);
        ev.type_ = EventType(event_type.into());
        ev.schema_ver = myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2;
        ev.subject = ArtifactRef("myelin://acme/git/pr/web:42".into());
        ev.aggregate = AggregateKey("git/pr/web:42".into());
        ev.payload = serde_json::json!({
            "repo": "web",
            "number": 42,
            "head_oid": TEST_OID,
            "head_generation": 1,
        });
        ev.payload
            .as_object_mut()
            .unwrap()
            .extend(fork_fields.as_object().unwrap().clone());
        ev
    }

    fn valid_toml() -> &'static [u8] {
        concat!(
            "on = \"push\"\n\n",
            "[[jobs]]\n",
            "name = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n",
        )
        .as_bytes()
    }

    fn valid_v2_toml() -> &'static [u8] {
        concat!(
            "schema_version = 2\n",
            "on = \"push\"\n\n",
            "[execution]\n",
            "profile = \"linux-small-v1\"\n\n",
            "[[jobs]]\n",
            "name = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n",
        )
        .as_bytes()
    }

    fn valid_pr_toml() -> &'static [u8] {
        concat!(
            "on = \"pull_request\"\n\n",
            "[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n"
        )
        .as_bytes()
    }

    struct NoConfigRead;

    impl GitConfigReader for NoConfigRead {
        fn read_repo_file(
            &self,
            _tenant: &str,
            _region: &str,
            _repo: &str,
            _oid: &str,
            _path: &str,
        ) -> Result<Option<Vec<u8>>, GitReadError> {
            panic!("malformed provenance reached a config read")
        }
    }

    struct NoCasAccess;

    impl BlobStore for NoCasAccess {
        fn put(
            &self,
            _tenant: &TenantId,
            _bytes: &[u8],
        ) -> myelin_storage::blob::Result<ContentHash> {
            panic!("malformed provenance reached CAS")
        }
        fn get(
            &self,
            _tenant: &TenantId,
            _hash: &ContentHash,
        ) -> myelin_storage::blob::Result<Vec<u8>> {
            panic!("malformed provenance reached CAS")
        }
        fn head(
            &self,
            _tenant: &TenantId,
            _hash: &ContentHash,
        ) -> myelin_storage::blob::Result<myelin_storage::blob::BlobMeta> {
            panic!("malformed provenance reached CAS")
        }
        fn delete(
            &self,
            _tenant: &TenantId,
            _hash: &ContentHash,
        ) -> myelin_storage::blob::Result<()> {
            panic!("malformed provenance reached CAS")
        }
    }

    fn assert_malformed_before_side_effects(ev: &EventEnvelope) {
        assert!(matches!(
            plan_dispatch(ev, &NoConfigRead, &NoCasAccess),
            DispatchOutcome::Skip(SkipReason::MalformedPayload(_))
        ));
    }

    fn arm(ev: &EventEnvelope, reader: &dyn GitConfigReader) -> DispatchOutcome {
        let blobs = FsBlobStore::new();
        plan_dispatch(ev, reader, &blobs)
    }

    fn runtime(handler: CiTriggerHandler, ledger: DedupLedger) -> Consumer<CiTriggerHandler> {
        let subscription = Subscription::bind(
            ConsumerName(TRIGGER_CONSUMER.into()),
            CI_TRIGGER_SUBJECT_STRS,
            PrefetchBound::DEFAULT,
        )
        .expect("CI trigger subscription");
        Consumer::new(handler, subscription, ledger)
            .with_upcaster(pr_trigger_upcasters().into_hook())
    }

    fn message(envelope: EventEnvelope) -> Message {
        Message {
            subject: envelope.subject.0.clone(),
            envelope,
        }
    }

    fn temp_git_root(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "myelin-ci-dispatch-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temporary git root");
        root
    }

    #[test]
    fn authoritative_git_root_requires_an_existing_absolute_directory() {
        assert!(matches!(
            AuthoritativeGitRoot::validate("relative/git-root"),
            Err(GitRootError(message)) if message.contains("absolute")
        ));
        let missing = std::env::temp_dir().join(format!(
            "myelin-ci-dispatch-missing-root-{}",
            std::process::id()
        ));
        assert!(AuthoritativeGitRoot::validate(missing).is_err());

        let root = temp_git_root("validated-root");
        let validated = AuthoritativeGitRoot::validate(&root).expect("valid root");
        assert_eq!(validated.as_path(), root.canonicalize().unwrap());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn region_mismatch_dlqs_before_git_cas_or_reserve_then_records_terminal_tombstone() {
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            Arc::new(NoConfigRead),
            Arc::new(NoCasAccess),
            reserve.clone(),
            "us-east",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert!(matches!(
            consumer.deliver(&message(push_envelope("web", TEST_OID))),
            Delivered::DeadLettered(_)
        ));
        assert!(
            reserve.persisted().is_empty(),
            "region mismatch has zero reserve effects"
        );
        assert_eq!(
            ledger.len(),
            1,
            "DLQ persistence precedes the terminal tombstone"
        );
    }

    #[test]
    fn missing_repository_or_exact_commit_retries_with_zero_dedup_or_effect() {
        let root = temp_git_root("unavailable-git");
        let store = myelin_git::durable::DurableGitStore::rooted(&root);
        let loc = myelin_git::core::RepoLoc::new("acme", "fr-par", "web");

        for (event_id, setup_repo) in [("missing-repo", false), ("missing-commit", true)] {
            if setup_repo {
                store.create_repo(&loc).expect("create bare repo");
            }
            let reader: Arc<dyn GitConfigReader> = Arc::new(DurableGitConfigReader::new(
                myelin_git::durable::DurableGitStore::rooted(&root),
            ));
            let reserve = Arc::new(RecordingReserveStore::new());
            let handler = CiTriggerHandler::for_region(
                reader,
                Arc::new(FsBlobStore::new()),
                reserve.clone(),
                "fr-par",
            );
            let ledger = DedupLedger::new();
            let consumer = runtime(handler, ledger.clone());
            let mut event = push_envelope("web", "1111111111111111111111111111111111111111");
            event.event_id = EventId(event_id.into());

            assert_eq!(consumer.deliver(&message(event)), Delivered::Retried(5));
            assert!(reserve.persisted().is_empty());
            assert!(ledger.is_empty(), "Git retry rolls back the dedup mark");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn existing_commit_without_ci_config_is_a_genuine_acked_no_config() {
        let root = temp_git_root("no-config");
        let store = myelin_git::durable::DurableGitStore::rooted(&root);
        let repo = store
            .create_repo(&myelin_git::core::RepoLoc::new("acme", "fr-par", "web"))
            .expect("create bare repo");
        let (commit, _, _) = repo
            .build_file_commit(
                "refs/heads/main",
                "README.md",
                b"no CI definition\n",
                "seed",
                "ci",
                "ci@invalid",
            )
            .expect("seed commit");
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            Arc::new(DurableGitConfigReader::new(store)),
            Arc::new(FsBlobStore::new()),
            reserve.clone(),
            "fr-par",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert_eq!(
            consumer.deliver(&message(push_envelope("web", commit.as_str()))),
            Delivered::Acked
        );
        assert!(reserve.persisted().is_empty());
        assert_eq!(
            ledger.len(),
            1,
            "genuine NoConfig is terminally acknowledged"
        );
        drop(consumer);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn oversized_config_is_terminal_poison_without_business_effect() {
        let reader: Arc<dyn GitConfigReader> = Arc::new(MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            vec![b' '; crate::config::MAX_CI_CONFIG_BYTES + 1],
        ));
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler =
            CiTriggerHandler::for_region(reader, Arc::new(NoCasAccess), reserve.clone(), "fr-par");
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert!(matches!(
            consumer.deliver(&message(push_envelope("web", TEST_OID))),
            Delivered::DeadLettered(_)
        ));
        assert!(reserve.persisted().is_empty());
        assert_eq!(
            ledger.len(),
            1,
            "DLQ persistence precedes the terminal tombstone"
        );
    }

    struct FailingBlobStore;

    impl BlobStore for FailingBlobStore {
        fn put(
            &self,
            _tenant: &TenantId,
            _bytes: &[u8],
        ) -> myelin_storage::blob::Result<ContentHash> {
            Err(myelin_storage::blob::BlobError::Backend(
                myelin_storage::blob::BlobDependencyError::Transient,
            ))
        }
        fn get(&self, _: &TenantId, _: &ContentHash) -> myelin_storage::blob::Result<Vec<u8>> {
            panic!("CAS read reached during write-failure test")
        }
        fn head(
            &self,
            _: &TenantId,
            _: &ContentHash,
        ) -> myelin_storage::blob::Result<myelin_storage::blob::BlobMeta> {
            panic!("CAS head reached during write-failure test")
        }
        fn delete(&self, _: &TenantId, _: &ContentHash) -> myelin_storage::blob::Result<()> {
            panic!("CAS delete reached during write-failure test")
        }
    }

    #[test]
    fn cas_snapshot_write_failure_retries_without_dedup_or_reserve_effect() {
        let reader: Arc<dyn GitConfigReader> = Arc::new(MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            valid_toml(),
        ));
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            reader,
            Arc::new(FailingBlobStore),
            reserve.clone(),
            "fr-par",
        );
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());

        assert_eq!(
            consumer.deliver(&message(push_envelope("web", TEST_OID))),
            Delivered::DependencyUnavailable(myelin_events::relay::IntakeDependency::Blob, 5,)
        );
        assert!(reserve.persisted().is_empty());
        assert!(ledger.is_empty(), "S3 retry rolls back the dedup mark");
    }

    #[test]
    fn subjects_is_a_whitelist_never_wildcard() {
        let sub = myelin_events::consumer::Subscription::bind(
            myelin_events::ConsumerName(TRIGGER_CONSUMER.into()),
            CI_TRIGGER_SUBJECT_STRS,
            myelin_events::PrefetchBound::DEFAULT,
        );
        assert!(sub.is_ok(), "the CI trigger whitelist binds (never `*`)");
        for s in CI_TRIGGER_SUBJECT_STRS {
            assert_ne!(*s, "*", "no `*` in the whitelist");
            assert_ne!(*s, ">", "no `>` in the whitelist");
            assert!(!s.is_empty(), "no empty (over-broad) subject");
        }
        let patterns = ci_trigger_subjects();
        assert!(!patterns.is_empty(), "subjects() is a non-empty whitelist");
        for p in patterns {
            assert!(
                !p.0.is_empty(),
                "no EMPTY (match-all) SubjectPattern in subjects() - finding #12"
            );
            assert_ne!(p.0, "*");
            assert_ne!(p.0, ">");
        }
        assert_eq!(
            patterns.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
            CI_TRIGGER_SUBJECT_STRS.to_vec(),
            "subjects() mirrors the &str whitelist exactly"
        );
    }

    #[test]
    fn no_config_is_a_clean_skip() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new();
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::NoConfig)
        ));
    }

    #[test]
    fn a_malformed_config_is_a_surfaced_skip() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            &b"on = = broken"[..],
        );
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ConfigError(_))
        ));
    }

    #[test]
    fn a_read_failure_is_fail_closed() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_failure("web", TEST_OID, ".myelin/ci.toml");
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ReadFailed(_))
        ));
    }

    #[test]
    fn an_oversized_config_is_refused_before_parsing() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            vec![b' '; crate::config::MAX_CI_CONFIG_BYTES + 1],
        );
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ReadFailed(_))
        ));
    }

    #[test]
    fn a_non_matching_trigger_skips() {
        let ev = push_envelope("web", TEST_OID);
        let pr_config = concat!(
            "on = \"pull_request\"\n\n",
            "[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n"
        );
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            pr_config.as_bytes(),
        );
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::TriggerNotMatched)
        ));
    }

    #[test]
    fn a_floating_tag_is_a_surfaced_resolve_skip() {
        let ev = push_envelope("web", TEST_OID);
        let floating = "on = \"push\"\n\n[[jobs]]\nname = \"build\"\nimage = \"alpine:3\"\ncommand = [\"build\"]\n";
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            floating.as_bytes(),
        );
        assert!(matches!(
            arm(&ev, &reader),
            DispatchOutcome::Skip(SkipReason::ResolveError(ResolveError::FloatingTag { .. }))
        ));
    }

    #[test]
    fn a_malformed_payload_skips() {
        let mut ev = push_envelope("web", TEST_OID);
        ev.payload = serde_json::json!({ "ref": "refs/heads/main" });
        assert!(matches!(
            arm(&ev, &MapGitConfigReader::new()),
            DispatchOutcome::Skip(SkipReason::MalformedPayload(_))
        ));
    }

    fn assert_invalid_provenance_is_poison_before_effects(ev: EventEnvelope) {
        let reserve = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::for_region(
            Arc::new(NoConfigRead),
            Arc::new(NoCasAccess),
            reserve.clone(),
            "fr-par",
        );
        match handler.handle(&ev, &mut myelin_events::HandlerTx::none()) {
            HandleOutcome::NonRetryable(myelin_events::Reason(reason)) => {
                assert_eq!(reason, "invalid trigger provenance");
                assert!(!reason.contains("ATTACKER_SENTINEL"));
            }
            other => panic!("invalid provenance must be permanent poison, got {other:?}"),
        }
        let ledger = DedupLedger::new();
        let consumer = runtime(handler, ledger.clone());
        assert!(matches!(
            consumer.deliver(&message(ev)),
            Delivered::DeadLettered(_)
        ));
        assert!(
            reserve.persisted().is_empty(),
            "invalid provenance has zero reserve effects"
        );
        assert_eq!(
            ledger.len(),
            1,
            "permanent poison records a terminal tombstone after DLQ"
        );
    }

    #[test]
    fn push_subject_aggregate_payload_and_ref_provenance_must_cohere_before_effects() {
        let mut cases = Vec::new();
        let mut wrong_subject_repo = push_envelope("team/web", TEST_OID);
        wrong_subject_repo.subject =
            ArtifactRef("myelin://acme/git/ref/other%2Fweb:refs%2Fheads%2Fmain".into());
        cases.push(wrong_subject_repo);
        let mut wrong_tenant = push_envelope("web", TEST_OID);
        wrong_tenant.subject = ArtifactRef("myelin://other/git/ref/web:refs%2Fheads%2Fmain".into());
        cases.push(wrong_tenant);
        let mut wrong_aggregate = push_envelope("web", TEST_OID);
        wrong_aggregate.aggregate =
            AggregateKey("ref:ATTACKER_SENTINEL:refs%2Fheads%2Fmain".into());
        cases.push(wrong_aggregate);
        let mut invalid_repo = push_envelope("web", TEST_OID);
        invalid_repo.payload["repo"] = serde_json::json!("../web");
        cases.push(invalid_repo);
        let mut invalid_ref = push_envelope("web", TEST_OID);
        invalid_ref.payload["ref"] = serde_json::json!("HEAD");
        cases.push(invalid_ref);
        let mut hidden_ref_component = push_envelope("web", TEST_OID);
        hidden_ref_component.payload["ref"] = serde_json::json!("refs/heads/.hidden");
        cases.push(hidden_ref_component);
        for event in cases {
            assert_invalid_provenance_is_poison_before_effects(event);
        }
    }

    #[test]
    fn pr_subject_aggregate_payload_and_number_provenance_must_cohere_before_effects() {
        let mut cases = Vec::new();
        let mut wrong_subject = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        wrong_subject.subject = ArtifactRef("myelin://acme/git/pr/other:42".into());
        cases.push(wrong_subject);
        let mut wrong_aggregate =
            pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        wrong_aggregate.aggregate = AggregateKey("git/pr/web:41".into());
        cases.push(wrong_aggregate);
        let mut invalid_repo = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        invalid_repo.payload["repo"] = serde_json::json!("group//web");
        cases.push(invalid_repo);
        let mut invalid_number =
            pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        invalid_number.payload["number"] = serde_json::json!(0);
        cases.push(invalid_number);
        let oversized_repo = "a".repeat(510);
        let mut oversized_group =
            pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        oversized_group.payload["repo"] = serde_json::json!(&oversized_repo);
        oversized_group.subject = ArtifactRef(format!("myelin://acme/git/pr/{oversized_repo}:42"));
        oversized_group.aggregate = AggregateKey(format!("git/pr/{oversized_repo}:42"));
        cases.push(oversized_group);
        for event in cases {
            assert_invalid_provenance_is_poison_before_effects(event);
        }
    }

    #[test]
    fn pr_head_generation_is_versioned_upcasted_and_never_invented_for_legacy_sync() {
        for invalid in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!("1"),
            serde_json::json!(1.5),
        ] {
            let mut event =
                pr_envelope(GIT_PR_SYNCHRONIZED, serde_json::json!({ "is_fork": false }));
            event.payload["head_generation"] = invalid;
            assert_invalid_provenance_is_poison_before_effects(event);
        }

        let event = pr_envelope(GIT_PR_SYNCHRONIZED, serde_json::json!({ "is_fork": false }));
        let facts = trigger_facts(&event).expect("canonical producer generation is admitted");
        assert_eq!(facts.pr_head_generation, Some(1));

        let mut current_missing =
            pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        current_missing
            .payload
            .as_object_mut()
            .unwrap()
            .remove("head_generation");
        assert!(matches!(
            trigger_facts(&current_missing),
            Err(SkipReason::InvalidProvenance(_))
        ));

        let mut legacy_opened = current_missing.clone();
        legacy_opened.schema_ver = 1;
        let upcasted = pr_trigger_upcasters()
            .upcast(legacy_opened)
            .expect("legacy opened has deterministic initial generation");
        assert_eq!(upcasted.schema_ver, GIT_PR_HEAD_TRIGGER_SCHEMA_V2);
        assert_eq!(
            trigger_facts(&upcasted).unwrap().pr_head_generation,
            Some(1)
        );

        let mut conflicting_legacy_opened =
            pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": false }));
        conflicting_legacy_opened.schema_ver = 1;
        conflicting_legacy_opened.payload["head_generation"] = serde_json::json!(999);
        let upcasted = pr_trigger_upcasters()
            .upcast(conflicting_legacy_opened)
            .expect("unknown v1 key cannot override the deterministic opened generation");
        assert_eq!(
            trigger_facts(&upcasted).unwrap().pr_head_generation,
            Some(1)
        );

        let mut legacy_sync =
            pr_envelope(GIT_PR_SYNCHRONIZED, serde_json::json!({ "is_fork": false }));
        legacy_sync.schema_ver = 1;
        legacy_sync
            .payload
            .as_object_mut()
            .unwrap()
            .remove("head_generation");
        let upcasted = pr_trigger_upcasters()
            .upcast(legacy_sync)
            .expect("the adjacent hop preserves the legacy event for typed validation");
        assert!(matches!(
            trigger_facts(&upcasted),
            Err(SkipReason::InvalidProvenance(_))
        ));
    }

    #[test]
    fn revspecs_refs_head_abbreviations_and_non_hex_oids_are_permanent_before_git_read() {
        for invalid in [
            "HEAD",
            "main",
            "refs/heads/main",
            "deadbeef",
            "ATTACKER_SENTINEL",
        ] {
            let mut event = push_envelope("web", TEST_OID);
            event.payload["new_oid"] = serde_json::json!(invalid);
            assert!(matches!(
                plan_dispatch(&event, &NoConfigRead, &NoCasAccess),
                DispatchOutcome::Skip(SkipReason::ReadFailed(GitReadError::Invalid(_)))
            ));
        }
        let mut event = push_envelope("web", TEST_OID);
        event.payload["new_oid"] = serde_json::json!("ATTACKER_SENTINEL");
        let handler = CiTriggerHandler::for_region(
            Arc::new(NoConfigRead),
            Arc::new(NoCasAccess),
            Arc::new(RecordingReserveStore::new()),
            "fr-par",
        );
        let HandleOutcome::NonRetryable(myelin_events::Reason(reason)) =
            handler.handle(&event, &mut myelin_events::HandlerTx::none())
        else {
            panic!("invalid oid must be permanent poison");
        };
        assert!(!reason.contains("ATTACKER_SENTINEL"));
    }

    #[test]
    fn uppercase_exact_oid_is_canonicalized_before_git_read_and_persistence() {
        let uppercase = "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml());
        let DispatchOutcome::Arm(armed) = arm(&push_envelope("web", uppercase), &reader) else {
            panic!("uppercase exact oid must canonicalize and arm");
        };
        assert_eq!(armed.reserve.commit_oid, TEST_OID);
    }

    #[test]
    fn durable_reader_preserves_namespaced_repository_slugs() {
        let root = temp_git_root("namespaced-repo");
        let store = myelin_git::durable::DurableGitStore::rooted(&root);
        let repo = store
            .create_repo(&myelin_git::core::RepoLoc::new(
                "acme", "fr-par", "team/web",
            ))
            .expect("create namespaced bare repo");
        let raw = git2::Repository::open_bare(repo.path()).expect("open namespaced repo");
        let blob = raw.blob(valid_toml()).expect("write CI config blob");
        let mut ci = raw.treebuilder(None).expect("CI tree builder");
        ci.insert("ci.toml", blob, 0o100644).expect("insert config");
        let ci_tree = ci.write().expect("write CI tree");
        let mut root_tree = raw.treebuilder(None).expect("root tree builder");
        root_tree
            .insert(".myelin", ci_tree, 0o040000)
            .expect("insert .myelin tree");
        let root_tree = raw
            .find_tree(root_tree.write().expect("write root tree"))
            .unwrap();
        let signature = git2::Signature::now("ci", "ci@invalid").unwrap();
        let commit = raw
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "seed",
                &root_tree,
                &[],
            )
            .expect("seed CI config")
            .to_string();
        let reader = DurableGitConfigReader::new(store);
        let DispatchOutcome::Arm(armed) = arm(&push_envelope("team/web", &commit), &reader) else {
            panic!("the full namespaced slug must select its own repository");
        };
        assert_eq!(armed.reserve.repo_ref, "myelin://acme/git/repo/team/web");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn both_pr_events_refuse_missing_mistyped_or_conflicting_fork_evidence_before_side_effects() {
        let malformed = [
            serde_json::json!({}),
            serde_json::json!({ "is_fork": "false" }),
            serde_json::json!({ "forked": 0 }),
            serde_json::json!({ "is_fork": true, "forked": false }),
            serde_json::json!({ "is_fork": true, "forked": "true" }),
        ];
        for event_type in [GIT_PR_OPENED, GIT_PR_SYNCHRONIZED] {
            for fields in &malformed {
                assert_malformed_before_side_effects(&pr_envelope(event_type, fields.clone()));
            }
        }
    }

    #[test]
    fn both_pr_events_accept_boolean_canonical_legacy_and_equal_alias_evidence() {
        let cases = [
            (serde_json::json!({ "is_fork": false }), "trusted"),
            (serde_json::json!({ "forked": false }), "trusted"),
            (
                serde_json::json!({ "is_fork": false, "forked": false }),
                "trusted",
            ),
            (serde_json::json!({ "is_fork": true }), "untrusted_fork"),
            (serde_json::json!({ "forked": true }), "untrusted_fork"),
            (
                serde_json::json!({ "is_fork": true, "forked": true }),
                "untrusted_fork",
            ),
        ];
        for event_type in [GIT_PR_OPENED, GIT_PR_SYNCHRONIZED] {
            for (fields, expected_tier) in &cases {
                let ev = pr_envelope(event_type, fields.clone());
                let reader = MapGitConfigReader::new().with_file(
                    "web",
                    TEST_OID,
                    ".myelin/ci.toml",
                    valid_pr_toml(),
                );
                let DispatchOutcome::Arm(armed) = arm(&ev, &reader) else {
                    panic!("explicit boolean fork evidence must reach the matching PR config");
                };
                assert_eq!(armed.handoff.run_write.trust_tier, *expected_tier);
            }
        }
    }

    #[test]
    fn push_preserves_absence_and_true_but_refuses_mistyped_or_conflicting_fork_evidence() {
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml());
        let DispatchOutcome::Arm(absent) = arm(&push_envelope("web", TEST_OID), &reader) else {
            panic!("legacy push without fork evidence remains compatible");
        };
        assert_eq!(absent.handoff.run_write.trust_tier, "trusted");

        let mut fork = push_envelope("web", TEST_OID);
        fork.payload["is_fork"] = serde_json::json!(true);
        let DispatchOutcome::Arm(fork) = arm(&fork, &reader) else {
            panic!("well-typed push fork evidence must be preserved");
        };
        assert_eq!(fork.handoff.run_write.trust_tier, "untrusted_fork");

        for fields in [
            serde_json::json!({ "is_fork": "false" }),
            serde_json::json!({ "forked": null }),
            serde_json::json!({ "is_fork": false, "forked": true }),
        ] {
            let mut ev = push_envelope("web", TEST_OID);
            ev.payload
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            assert_malformed_before_side_effects(&ev);
        }
    }

    #[test]
    fn unknown_event_type_remains_not_a_trigger_before_payload_parsing() {
        let mut ev = push_envelope("web", TEST_OID);
        ev.type_ = EventType("git.pr.closed".into());
        ev.payload["is_fork"] = serde_json::json!("not-a-boolean");
        assert!(matches!(
            plan_dispatch(&ev, &NoConfigRead, &NoCasAccess),
            DispatchOutcome::Skip(SkipReason::NotATrigger(t)) if t == "git.pr.closed"
        ));
    }

    #[test]
    fn the_happy_path_arms_the_atomic_bundle() {
        let ev = push_envelope("web", TEST_OID);
        let reader =
            MapGitConfigReader::new().with_file("web", TEST_OID, ".myelin/ci.toml", valid_toml());
        let DispatchOutcome::Arm(armed) = arm(&ev, &reader) else {
            panic!("the digest-pinned config on a matching push must arm a run");
        };
        assert!(
            armed.handoff.is_atomic_bundle(),
            "the reserve bundle is atomic"
        );
        assert_eq!(armed.handoff.run_write.state, "queued");
        assert_eq!(
            armed.handoff.run_write.trust_tier, "trusted",
            "a member push is trusted"
        );
        assert_eq!(armed.handoff.run_write.trigger_kind, "push");
        assert_eq!(armed.handoff.run_write.cause_event_id, "ev-push-1");
        assert_eq!(
            armed.handoff.queued_checks.len(),
            1,
            "one queued check per job"
        );
        assert_eq!(
            armed.handoff.run_write.run_id,
            deterministic_uuid("run:ev-push-1")
        );
        assert_eq!(armed.reserve.wf_run_id, deterministic_uuid("wf:ev-push-1"));
        assert_eq!(armed.reserve.correlation_id, "corr-1");
        assert_eq!(armed.reserve.repo_ref, "myelin://acme/git/repo/web");
        assert_eq!(
            armed.reserve.source_ref.as_deref(),
            Some("refs/heads/main"),
            "the immutable branch identity survives Git intake"
        );
        assert_eq!(armed.reserve.commit_oid, TEST_OID);
        assert!(armed.reserve.concurrency_group.is_none());
        let insert = ci_run_insert_from_armed(&armed);
        assert_eq!(insert.source_ref, armed.reserve.source_ref);
        assert_eq!(insert.triggered_by.as_deref(), Some("pusher"));
        assert!(insert.concurrency_group.is_none());
        assert_eq!(armed.tenant, TenantId("acme".into()));
    }

    #[test]
    fn exact_v2_config_reaches_a_v2_cas_snapshot_through_the_production_consumer() {
        let ev = push_envelope("web", TEST_OID);
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            valid_v2_toml(),
        );
        let blobs = FsBlobStore::new();
        let DispatchOutcome::Arm(armed) = plan_dispatch(&ev, &reader, &blobs) else {
            panic!("the exact V2 request must arm and persist its requested wire");
        };
        let digest = armed
            .handoff
            .run_write
            .definition_snapshot
            .0
            .rsplit('/')
            .next()
            .unwrap();
        let address = ContentHash::parse(digest).unwrap();
        let bytes = blobs.get(&TenantId("acme".into()), &address).unwrap();
        let decoded = myelin_ci_controlplane::decode_resolved_run_plan(&bytes).unwrap();
        assert!(decoded.as_v2().is_some());
    }

    #[test]
    fn a_fork_pr_stamps_untrusted_fork() {
        let ev = pr_envelope(GIT_PR_OPENED, serde_json::json!({ "is_fork": true }));
        let pr_config = concat!(
            "on = \"pull_request\"\n\n[[jobs]]\nname = \"build\"\n",
            "image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"\n",
            "command = [\"build\"]\n"
        );
        let reader = MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            pr_config.as_bytes(),
        );
        let DispatchOutcome::Arm(armed) = arm(&ev, &reader) else {
            panic!("a fork PR with a matching config arms an untrusted-fork run");
        };
        assert_eq!(armed.handoff.run_write.trust_tier, "untrusted_fork");
        assert_eq!(armed.handoff.run_write.trigger_kind, "pull_request");
        assert_eq!(
            armed.reserve.concurrency_group.as_deref(),
            Some("pr:web:42")
        );
        assert_eq!(armed.reserve.pr_head_generation, Some(1));
        assert_eq!(
            ci_run_insert_from_armed(&armed)
                .concurrency_group
                .as_deref(),
            Some("pr:web:42")
        );
        assert_eq!(ci_run_insert_from_armed(&armed).pr_head_generation, Some(1));
        for c in &armed.handoff.queued_checks {
            assert_eq!(
                c.payload["trust_tier"], "untrusted_fork",
                "X-1: 0 divergence"
            );
        }
        assert_eq!(
            stamp_trust(&RunProvenance {
                is_fork: true,
                targets_self_hosted: false,
                read_excludes_fork: false
            })
            .job_tier,
            TrustTier::UntrustedFork
        );
    }

    #[test]
    fn the_handler_persists_and_is_idempotent() {
        let ev = push_envelope("web", TEST_OID);
        let reader: Arc<dyn GitConfigReader> = Arc::new(MapGitConfigReader::new().with_file(
            "web",
            TEST_OID,
            ".myelin/ci.toml",
            valid_toml(),
        ));
        let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
        let store = Arc::new(RecordingReserveStore::new());
        let handler = CiTriggerHandler::new(reader, blobs, store.clone());

        assert_eq!(
            handler.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            handler.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "redelivery is handled"
        );
        let persisted = store.persisted();
        assert_eq!(persisted.len(), 2, "the handler ran on both deliveries");
        assert_eq!(
            persisted[0].handoff.run_write.run_id, persisted[1].handoff.run_write.run_id,
            "the redelivery mints the SAME deterministic run_id (exactly-once run)"
        );
        assert!(handler
            .trace()
            .iter()
            .any(|l| l.starts_with("armed run_id=")));
    }

    #[test]
    fn check_event_id_diverges_across_runs_stable_within_a_run() {
        let subject = "myelin://acme/git/ref/web:refs/heads/main#commit-deadbeef/check-build";
        let run_a = deterministic_uuid("run:ev-A");
        let run_b = deterministic_uuid("run:ev-B");
        assert_eq!(
            check_event_id(&run_a, subject),
            check_event_id(&run_a, subject),
            "same run + subject is stable (redelivery dedups)"
        );
        assert_ne!(
            check_event_id(&run_a, subject),
            check_event_id(&run_b, subject),
            "distinct runs on the same (repo, commit, context) must NOT collide (H3)"
        );
    }

    #[test]
    fn deterministic_uuid_is_stable_and_shaped() {
        let a = deterministic_uuid("run:ev-1");
        assert_eq!(a, deterministic_uuid("run:ev-1"), "stable per seed");
        assert_ne!(a, deterministic_uuid("run:ev-2"), "distinct across seeds");
        assert_eq!(a.len(), 36, "canonical uuid length");
        assert_eq!(a.matches('-').count(), 4, "canonical uuid dashes");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}
