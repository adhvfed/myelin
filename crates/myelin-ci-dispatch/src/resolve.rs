use std::collections::{BTreeMap, BTreeSet};

use myelin_ci_sandbox::ImageRef;
use myelin_events::EventId;
use myelin_events::{ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;

pub use myelin_ci_controlplane::{
    CiExecutionRequestV1, ResolvedJobV1, ResolvedJobV2, ResolvedRunPlanV1, ResolvedRunPlanV2,
    StructuredBuildToolV1, StructuredBuildV1,
    VersionedResolvedRunPlan as VersionedResolvedSnapshot,
};
pub type ResolvedJob = ResolvedJobV1;
pub type ResolvedSnapshot = ResolvedRunPlanV1;

pub trait ResolvedSnapshotExt {
    fn has_dynamic_generation(&self) -> bool;
}

impl ResolvedSnapshotExt for ResolvedSnapshot {
    fn has_dynamic_generation(&self) -> bool {
        self.jobs.iter().any(|job| job.is_generator)
    }
}

impl ResolvedSnapshotExt for VersionedResolvedSnapshot {
    fn has_dynamic_generation(&self) -> bool {
        match self {
            VersionedResolvedSnapshot::V1(plan) => plan.jobs.iter().any(|job| job.is_generator),
            VersionedResolvedSnapshot::V2(plan) => plan.jobs.iter().any(|job| job.is_generator),
        }
    }
}

use myelin_ci_sandbox::events::CI_RUN_STARTED;

use crate::dispatch::{OnTrigger, TrustStamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Normal,
    Generate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobDef {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub build: Option<StructuredBuildV1>,
    pub needs: Vec<String>,
    pub kind: JobKind,
    pub matrix: BTreeMap<String, Vec<String>>,
}

impl JobDef {
    pub fn normal(
        name: impl Into<String>,
        image: impl Into<String>,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> JobDef {
        JobDef {
            name: name.into(),
            image: image.into(),
            command: command.into_iter().map(Into::into).collect(),
            build: None,
            needs: Vec::new(),
            kind: JobKind::Normal,
            matrix: BTreeMap::new(),
        }
    }

    pub fn with_structured_build(mut self, build: StructuredBuildV1) -> JobDef {
        self.command.clear();
        self.build = Some(build);
        self
    }

    pub fn with_needs(mut self, needs: impl IntoIterator<Item = impl Into<String>>) -> JobDef {
        self.needs = needs.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_matrix(mut self, key: impl Into<String>, values: Vec<String>) -> JobDef {
        self.matrix.insert(key.into(), values);
        self
    }

    pub fn as_generator(mut self) -> JobDef {
        self.kind = JobKind::Generate;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiDefinition {
    pub on: OnTrigger,
    pub jobs: Vec<JobDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiPlanContract {
    V1,
    V2(CiExecutionRequestV1),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedCiDefinition {
    pub contract: CiPlanContract,
    pub on: OnTrigger,
    pub jobs: Vec<JobDef>,
}

impl VersionedCiDefinition {
    pub fn v1(on: OnTrigger, jobs: Vec<JobDef>) -> Self {
        Self {
            contract: CiPlanContract::V1,
            on,
            jobs,
        }
    }

    pub fn v2(on: OnTrigger, execution: CiExecutionRequestV1, jobs: Vec<JobDef>) -> Self {
        Self {
            contract: CiPlanContract::V2(execution),
            on,
            jobs,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    EmptyDefinition,
    FloatingTag { job: String, reference: String },
    DuplicateJob(String),
    UnknownNeed { job: String, need: String },
    SelfNeed(String),
    DuplicateNeed { job: String, need: String },
    Cyclic,
    BlobWrite(myelin_storage::BlobError),
    MatrixTooLarge { count: usize, cap: usize },
    InvalidPlan(String),
    ConcreteNameCollision(String),
    CargoVendorLockMissing,
    CargoVendorUnmatched { lock_sha256: String },
}

pub const MAX_TOTAL_MATRIX_INSTANCES: usize = 1024;

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::EmptyDefinition => {
                write!(f, "the CI definition has no jobs - nothing to run")
            }
            ResolveError::FloatingTag { job, reference } => write!(
                f,
                "job `{job}`: image `{reference}` is a FLOATING TAG - rejected fail-closed (resolve \
                 every reference to a digest `@<algo>:<hex>`; the tag→digest registry resolution is \
                 CI-P23). 0 un-digested references reach a snapshot."
            ),
            ResolveError::DuplicateJob(name) => {
                write!(f, "duplicate job name `{name}` - DAG node ids must be unique")
            }
            ResolveError::UnknownNeed { job, need } => write!(
                f,
                "job `{job}` needs `{need}`, which is not a job in the definition (dangling DAG edge)"
            ),
            ResolveError::SelfNeed(job) => write!(f, "job `{job}` depends on itself"),
            ResolveError::DuplicateNeed { job, need } => write!(f, "job `{job}` repeats dependency `{need}`"),
            ResolveError::Cyclic => write!(f, "the job DAG has a cycle - it is not a DAG"),
            ResolveError::BlobWrite(e) => {
                write!(f, "the CAS snapshot blob write failed: {e} (no snapshot ⇒ no start)")
            }
            ResolveError::MatrixTooLarge { count, cap } => write!(
                f,
                "the matrix cross-product resolves to {count} job instances, over the {cap} ceiling - \
                 rejected fail-closed (a push cannot fan out unbounded; raise the config's matrix or \
                 split the pipeline)"
            ),
            ResolveError::InvalidPlan(detail) => write!(f, "invalid resolved CI plan: {detail}"),
            ResolveError::ConcreteNameCollision(name) => write!(f, "matrix expansion produced duplicate concrete job name `{name}`"),
            ResolveError::CargoVendorLockMissing => write!(
                f,
                "a structured Cargo build was authored but the repository has no root `Cargo.lock` at \
                 the pushed ref - refused fail-closed (an offline `cargo build --locked` needs a \
                 committed lock, and the server-trusted vendor tree is selected from it)"
            ),
            ResolveError::CargoVendorUnmatched { lock_sha256 } => write!(
                f,
                "the repository's root `Cargo.lock` (sha256:{lock_sha256}) matches no registered, \
                 server-trusted Cargo vendor tree - the structured build is refused fail-closed \
                 rather than mounting an unregistered or default vendor (register a vendor built \
                 from exactly this lock)"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

fn instance_name(job: &str, assignment: &BTreeMap<String, String>) -> String {
    myelin_ci_controlplane::derive_concrete_job_name(job, assignment)
}

fn expand_matrix(matrix: &BTreeMap<String, Vec<String>>) -> Vec<BTreeMap<String, String>> {
    let mut out: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    for (axis, values) in matrix {
        let mut next = Vec::with_capacity(out.len() * values.len().max(1));
        for base in &out {
            for v in values {
                let mut a = base.clone();
                a.insert(axis.clone(), v.clone());
                next.push(a);
            }
        }
        if !values.is_empty() {
            out = next;
        }
    }
    out
}

fn validate_dag(jobs: &[JobDef]) -> Result<(), ResolveError> {
    let names: BTreeSet<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
    for j in jobs {
        let mut seen_needs = BTreeSet::new();
        for need in &j.needs {
            if need == &j.name {
                return Err(ResolveError::SelfNeed(j.name.clone()));
            }
            if !seen_needs.insert(need.as_str()) {
                return Err(ResolveError::DuplicateNeed {
                    job: j.name.clone(),
                    need: need.clone(),
                });
            }
            if !names.contains(need.as_str()) {
                return Err(ResolveError::UnknownNeed {
                    job: j.name.clone(),
                    need: need.clone(),
                });
            }
        }
    }
    let mut indeg: BTreeMap<&str, usize> = jobs
        .iter()
        .map(|j| (j.name.as_str(), j.needs.len()))
        .collect();
    let mut queue: Vec<&str> = indeg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&n, _)| n)
        .collect();
    let mut removed = 0usize;
    while let Some(n) = queue.pop() {
        removed += 1;
        for j in jobs {
            if j.needs.iter().any(|d| d == n) {
                let e = indeg.get_mut(j.name.as_str()).expect("name indexed");
                *e -= 1;
                if *e == 0 {
                    queue.push(j.name.as_str());
                }
            }
        }
    }
    if removed != jobs.len() {
        return Err(ResolveError::Cyclic);
    }
    Ok(())
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

fn validate_authored_tokens(jobs: &[JobDef]) -> Result<(), ResolveError> {
    use myelin_ci_controlplane::run_plan::{
        MAX_JOB_NAME_BYTES, MAX_MATRIX_AXES, MAX_MATRIX_KEY_BYTES, MAX_MATRIX_VALUE_BYTES,
    };
    for job in jobs {
        if !valid_machine_token(&job.name, MAX_JOB_NAME_BYTES) {
            return Err(ResolveError::InvalidPlan(format!(
                "authored job name `{}` is not a bounded machine token",
                job.name
            )));
        }
        if job.matrix.len() > MAX_MATRIX_AXES {
            return Err(ResolveError::InvalidPlan(format!(
                "job `{}` declares more than {MAX_MATRIX_AXES} matrix axes",
                job.name
            )));
        }
        for (axis, values) in &job.matrix {
            if !valid_machine_token(axis, MAX_MATRIX_KEY_BYTES) || values.is_empty() {
                return Err(ResolveError::InvalidPlan(format!(
                    "job `{}` matrix axis `{axis}` is invalid or empty",
                    job.name
                )));
            }
            if values
                .iter()
                .any(|value| !valid_machine_token(value, MAX_MATRIX_VALUE_BYTES))
            {
                return Err(ResolveError::InvalidPlan(format!(
                    "job `{}` has an invalid matrix value for `{axis}`",
                    job.name
                )));
            }
        }
    }
    Ok(())
}

pub fn resolve_versioned_snapshot(
    def: &VersionedCiDefinition,
    blobs: &dyn BlobStore,
    tenant: &TenantId,
) -> Result<(VersionedResolvedSnapshot, ContentHash), ResolveError> {
    resolve_versioned_snapshot_with_cargo_vendor(def, blobs, tenant, None)
}

pub fn resolve_versioned_snapshot_with_cargo_vendor(
    def: &VersionedCiDefinition,
    blobs: &dyn BlobStore,
    tenant: &TenantId,
    root_cargo_lock: Option<&[u8]>,
) -> Result<(VersionedResolvedSnapshot, ContentHash), ResolveError> {
    if def.jobs.is_empty() {
        return Err(ResolveError::EmptyDefinition);
    }
    if matches!(def.contract, CiPlanContract::V1) && def.jobs.iter().any(|job| job.build.is_some())
    {
        return Err(ResolveError::InvalidPlan(
            "structured build jobs require the authored V2 execution contract".into(),
        ));
    }
    validate_authored_tokens(&def.jobs)?;
    let mut seen = BTreeSet::new();
    for j in &def.jobs {
        if !seen.insert(j.name.as_str()) {
            return Err(ResolveError::DuplicateJob(j.name.clone()));
        }
    }
    validate_dag(&def.jobs)?;

    let mut total_instances: usize = 0;
    for j in &def.jobs {
        let job_instances = j
            .matrix
            .values()
            .fold(1usize, |acc, vals| acc.saturating_mul(vals.len().max(1)));
        total_instances = total_instances.saturating_add(job_instances);
        if total_instances > MAX_TOTAL_MATRIX_INSTANCES {
            return Err(ResolveError::MatrixTooLarge {
                count: total_instances,
                cap: MAX_TOTAL_MATRIX_INSTANCES,
            });
        }
    }

    let selected_cargo_vendor: Option<String> = if def.jobs.iter().any(|j| j.build.is_some()) {
        let lock = root_cargo_lock.ok_or(ResolveError::CargoVendorLockMissing)?;
        let lock_sha256 = myelin_ci_sandbox::cargo_lock_sha256_hex(lock);
        Some(
            myelin_ci_sandbox::select_registered_cargo_vendor(&lock_sha256)
                .ok_or(ResolveError::CargoVendorUnmatched { lock_sha256 })?,
        )
    } else {
        None
    };

    let mut resolved: Vec<ResolvedJobV2> = Vec::new();
    for j in &def.jobs {
        let image = ImageRef {
            reference: j.image.clone(),
        };
        if !image.digest_pinned() {
            return Err(ResolveError::FloatingTag {
                job: j.name.clone(),
                reference: j.image.clone(),
            });
        }
        for assignment in expand_matrix(&j.matrix) {
            resolved.push(ResolvedJobV2 {
                stage: j.name.clone(),
                name: instance_name(&j.name, &assignment),
                image: j.image.clone(),
                command: j.command.clone(),
                selected_cargo_vendor: j.build.as_ref().and(selected_cargo_vendor.clone()),
                build: j.build.clone(),
                needs: Vec::new(),
                is_generator: j.kind == JobKind::Generate,
                matrix_key: assignment,
            });
        }
    }
    let concrete_by_authored: BTreeMap<&str, Vec<String>> = def
        .jobs
        .iter()
        .map(|job| {
            let mut names: Vec<_> = expand_matrix(&job.matrix)
                .iter()
                .map(|assignment| instance_name(&job.name, assignment))
                .collect();
            names.sort();
            names.dedup();
            (job.name.as_str(), names)
        })
        .collect();
    for (job, authored) in resolved.iter_mut().zip(
        def.jobs
            .iter()
            .flat_map(|job| std::iter::repeat_n(job, expand_matrix(&job.matrix).len())),
    ) {
        job.needs = authored
            .needs
            .iter()
            .flat_map(|need| concrete_by_authored[need.as_str()].iter().cloned())
            .collect();
        job.needs.sort();
        job.needs.dedup();
    }
    resolved.sort_by(|a, b| a.name.cmp(&b.name));
    for pair in resolved.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(ResolveError::ConcreteNameCollision(pair[0].name.clone()));
        }
    }

    let snapshot = match &def.contract {
        CiPlanContract::V1 => VersionedResolvedSnapshot::V1(ResolvedRunPlanV1 {
            schema_version: myelin_ci_controlplane::RUN_PLAN_SCHEMA_V1,
            jobs: resolved
                .into_iter()
                .map(|job| ResolvedJobV1 {
                    name: job.name,
                    image: job.image,
                    command: job.command,
                    needs: job.needs,
                    is_generator: job.is_generator,
                    matrix_key: job.matrix_key,
                })
                .collect(),
        }),
        CiPlanContract::V2(execution) => VersionedResolvedSnapshot::V2(ResolvedRunPlanV2 {
            schema_version: myelin_ci_controlplane::RUN_PLAN_SCHEMA_V2,
            execution: execution.clone(),
            jobs: resolved,
        }),
    };
    let bytes = snapshot
        .canonical_bytes()
        .map_err(|error| ResolveError::InvalidPlan(error.to_string()))?;
    let address = blobs.put(tenant, &bytes).map_err(ResolveError::BlobWrite)?;
    Ok((snapshot, address))
}

pub fn resolve_snapshot(
    def: &CiDefinition,
    blobs: &dyn BlobStore,
    tenant: &TenantId,
) -> Result<(ResolvedSnapshot, ContentHash), ResolveError> {
    let versioned = VersionedCiDefinition::v1(def.on.clone(), def.jobs.clone());
    let (snapshot, address) = resolve_versioned_snapshot(&versioned, blobs, tenant)?;
    let VersionedResolvedSnapshot::V1(snapshot) = snapshot else {
        unreachable!("legacy definitions always resolve to V1")
    };
    Ok((snapshot, address))
}

pub fn snapshot_ref(tenant: &TenantId, address: &ContentHash) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/ci/snapshot/{}",
        tenant.0,
        address.to_multihash_string()
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckContext {
    pub name: String,
}

impl CheckContext {
    pub fn ci(name: impl Into<String>) -> CheckContext {
        CheckContext { name: name.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunWrite {
    pub run_id: String,
    pub definition_snapshot: ArtifactRef,
    pub trigger_kind: String,
    pub trust_tier: String,
    pub state: String,
    pub cause_event_id: String,
}

#[derive(Clone, Debug)]
pub struct StartHandoff {
    pub start_spec: StartSpec,
    pub run_write: CiRunWrite,
    pub run_started: EventDraft,
    pub queued_checks: Vec<EventDraft>,
}

pub use myelin_flow::StartSpec;

pub const CI_PIPELINE_WF_TYPE: &str = "ci.pipeline";

impl StartHandoff {
    pub fn is_atomic_bundle(&self) -> bool {
        let row_queued = self.run_write.state == "queued";
        let started_is_run_started = self.run_started.type_.0 == CI_RUN_STARTED;
        let has_queued_check =
            !self.queued_checks.is_empty() && self.queued_checks.iter().all(is_queued_check_draft);
        let snapshot_matches =
            self.start_spec.input.first() == Some(&self.run_write.definition_snapshot);
        row_queued && started_is_run_started && has_queued_check && snapshot_matches
    }
}

fn is_queued_check_draft(d: &EventDraft) -> bool {
    d.type_.0 == myelin_ci_sandbox::events::CI_CHECK_UPDATED
        && d.payload.get("state").and_then(|s| s.as_str()) == Some("queued")
}

fn trigger_kind_token(on: &OnTrigger) -> &'static str {
    match on {
        OnTrigger::Push => "push",
        OnTrigger::PullRequest => "pull_request",
        OnTrigger::IssueTransitioned => "issue_transition",
        OnTrigger::Manual => "manual",
        OnTrigger::Schedule => "schedule",
        OnTrigger::Agent => "agent",
    }
}

fn trust_tier_token(stamp: &TrustStamp) -> &'static str {
    use crate::dispatch::TrustTier;
    match stamp.job_tier {
        TrustTier::Trusted => "trusted",
        TrustTier::UntrustedFork => "untrusted_fork",
        TrustTier::SelfHosted => "self_hosted",
    }
}

fn check_subject(repo: &str, commit_oid: &str, context: &str) -> ArtifactRef {
    myelin_events::check_seam::check_subject(repo, commit_oid, context)
}

#[derive(Clone, Debug)]
pub struct RunFacts {
    pub run_id: String,
    pub tenant_id: String,
    pub repo_ref: String,
    pub source_ref: Option<String>,
    pub commit_oid: String,
    pub contexts: Vec<CheckContext>,
    pub cause_event_id: EventId,
    pub started_at: String,
}

pub fn reserve_and_start(
    snapshot: &ArtifactRef,
    stamp: &TrustStamp,
    on: &OnTrigger,
    facts: &RunFacts,
) -> StartHandoff {
    let trigger_kind = trigger_kind_token(on).to_string();
    let trust_tier = trust_tier_token(stamp).to_string();

    let start_spec = StartSpec {
        wf_type: CI_PIPELINE_WF_TYPE.to_string(),
        input: vec![snapshot.clone()],
        budget: None,
        idem_key: format!("{}:{}", facts.run_id, facts.cause_event_id.0),
    };

    let run_write = CiRunWrite {
        run_id: facts.run_id.clone(),
        definition_snapshot: snapshot.clone(),
        trigger_kind: trigger_kind.clone(),
        trust_tier: trust_tier.clone(),
        state: "queued".to_string(),
        cause_event_id: facts.cause_event_id.0.clone(),
    };

    let run_started = EventDraft {
        type_: EventType(CI_RUN_STARTED.to_string()),
        subject: ArtifactRef(format!(
            "myelin://{}/ci/run/{}",
            facts.tenant_id, facts.run_id
        )),
        aggregate: myelin_ci_sandbox::events::run_aggregate(&facts.run_id),
        payload: serde_json::json!({
            "run": format!("ci/run/{}", facts.run_id),
            "trust_tier": trust_tier,
            "trigger_kind": trigger_kind,
            "definition_snapshot": snapshot.0,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };

    let run_ref = format!("myelin://{}/ci/run/{}", facts.tenant_id, facts.run_id);
    let queued_checks = facts
        .contexts
        .iter()
        .map(|ctx| {
            let emit_context = myelin_ci_controlplane::CheckEmitContext {
                tenant: facts.tenant_id.clone(),
                repo: facts.repo_ref.clone(),
                commit_oid: facts.commit_oid.clone(),
                run_ref: run_ref.clone(),
                run_attempt: 0,
                trust_tier: match stamp.check_tier {
                    myelin_git::check_status::TrustTier::Trusted => {
                        myelin_ci_controlplane::TrustTier::Trusted
                    }
                    myelin_git::check_status::TrustTier::UntrustedFork => {
                        myelin_ci_controlplane::TrustTier::UntrustedFork
                    }
                },
                started_at: facts.started_at.clone(),
                completed_at: None,
            };
            let status = myelin_ci_controlplane::CheckStatusUpdate::required(
                myelin_ci_controlplane::CheckProvider::Ci,
                &ctx.name,
                myelin_ci_controlplane::CheckState::Queued,
            );
            let check_status = myelin_ci_controlplane::check_status_payload(&emit_context, &status);
            EventDraft {
                type_: EventType(myelin_ci_sandbox::events::CI_CHECK_UPDATED.to_string()),
                subject: check_subject(&facts.repo_ref, &facts.commit_oid, &ctx.name),
                aggregate: myelin_events::check_seam::check_aggregate(
                    &facts.repo_ref,
                    &facts.commit_oid,
                ),
                payload: check_status,
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            }
        })
        .collect();

    StartHandoff {
        start_spec,
        run_write,
        run_started,
        queued_checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{stamp_trust, RunProvenance};
    use myelin_storage::FsBlobStore;

    const PINNED: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";
    const PINNED2: &str = "registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn blobs() -> FsBlobStore {
        FsBlobStore::new()
    }

    fn member_stamp() -> TrustStamp {
        stamp_trust(&RunProvenance {
            is_fork: false,
            targets_self_hosted: false,
            read_excludes_fork: true,
        })
    }

    fn fork_stamp() -> TrustStamp {
        stamp_trust(&RunProvenance {
            is_fork: true,
            targets_self_hosted: false,
            read_excludes_fork: false,
        })
    }

    #[test]
    fn a_floating_tag_is_rejected_fail_closed() {
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("build", "alpine:3", ["build"])],
        };
        let err = resolve_snapshot(&def, &blobs(), &tenant())
            .expect_err("a floating tag must be rejected fail-closed");
        assert!(
            matches!(&err, ResolveError::FloatingTag { job, reference }
                if job == "build" && reference == "alpine:3"),
            "the floating tag is refused with its job + reference: {err:?}"
        );
    }

    #[test]
    fn every_undigested_reference_shape_is_rejected() {
        for bad in [
            "alpine",
            "alpine:latest",
            "registry/foo@sha256:",
            "foo@:abc",
        ] {
            let def = CiDefinition {
                on: OnTrigger::Push,
                jobs: vec![JobDef::normal("j", bad, ["run"])],
            };
            assert!(
                matches!(
                    resolve_snapshot(&def, &blobs(), &tenant()),
                    Err(ResolveError::FloatingTag { .. })
                ),
                "the un-digested reference `{bad}` must be rejected fail-closed"
            );
        }
    }

    #[test]
    fn an_unbounded_matrix_is_refused_before_it_is_materialized() {
        let mut job = JobDef::normal("build", PINNED, ["build"]);
        for a in 0..8u32 {
            job = job.with_matrix(
                format!("axis{a}"),
                (0..10u32).map(|v| v.to_string()).collect(),
            );
        }
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![job],
        };
        let err = resolve_snapshot(&def, &blobs(), &tenant())
            .expect_err("an astronomical matrix must be refused");
        assert!(
            matches!(err, ResolveError::MatrixTooLarge { count, cap }
                if count > cap && cap == MAX_TOTAL_MATRIX_INSTANCES),
            "the over-cap matrix is refused fail-closed: {err:?}"
        );

        let ok = JobDef::normal("test", PINNED2, ["test"])
            .with_matrix("os", vec!["linux".into(), "mac".into(), "win".into()])
            .with_matrix("v", vec!["1".into(), "2".into(), "3".into(), "4".into()]);
        let def_ok = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![ok],
        };
        let (snap, _addr) = resolve_snapshot(&def_ok, &blobs(), &tenant())
            .expect("a modestly-sized matrix resolves");
        assert_eq!(snap.jobs.len(), 12, "3×4 expands to 12 instances");
    }

    #[test]
    fn a_digest_pinned_definition_resolves_and_content_addresses() {
        let store = blobs();
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![
                JobDef::normal("build", PINNED, ["build"]),
                JobDef::normal("test", PINNED2, ["test"]).with_needs(["build"]),
            ],
        };
        let (snap, addr) =
            resolve_snapshot(&def, &store, &tenant()).expect("a digest-pinned def resolves");
        assert_eq!(snap.jobs.len(), 2, "two resolved jobs");
        let bytes = store
            .get(&tenant(), &addr)
            .expect("the snapshot blob is present");
        assert_eq!(
            bytes,
            snap.canonical_bytes().unwrap(),
            "the blob IS the snapshot bytes"
        );
        assert_eq!(addr, ContentHash::blake3(&snap.canonical_bytes().unwrap()));
    }

    #[test]
    fn the_matrix_expands_deterministically() {
        let store = blobs();
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("test", PINNED, ["test"])
                .with_matrix("os", vec!["linux".into(), "macos".into()])
                .with_matrix("rust", vec!["stable".into(), "beta".into()])],
        };
        let (snap, addr) = resolve_snapshot(&def, &store, &tenant()).expect("resolves");
        assert_eq!(snap.jobs.len(), 4, "the 2×2 matrix expands to 4 instances");
        let names: Vec<&str> = snap.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names.len(), 4);
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(names.iter().all(|name| name.starts_with("test--")));
        let (_snap2, addr2) = resolve_snapshot(&def, &blobs(), &tenant()).expect("re-resolves");
        assert_eq!(
            addr, addr2,
            "the same input → the same content address (reproducible)"
        );
        assert_eq!(
            addr.to_multihash_string(),
            "blake3:fc837be9ceab0d70f24be239f1c4c0a5167e95e2904087f5b7443dfe354b858d",
            "the legacy V1 matrix wire stays byte-identical"
        );
    }

    #[test]
    fn command_hash_and_concrete_matrix_fan_in_are_deterministic() {
        let definition = |command: &str| CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![
                JobDef::normal("build", PINNED, [command])
                    .with_matrix("os", vec!["linux".into(), "macos".into()]),
                JobDef::normal("test", PINNED2, ["test"])
                    .with_needs(["build"])
                    .with_matrix("rust", vec!["stable".into(), "beta".into()]),
            ],
        };
        let (first, hash) = resolve_snapshot(&definition("build"), &blobs(), &tenant()).unwrap();
        let (_, repeat) = resolve_snapshot(&definition("build"), &blobs(), &tenant()).unwrap();
        let (_, changed) = resolve_snapshot(&definition("build-v2"), &blobs(), &tenant()).unwrap();
        assert_eq!(hash, repeat);
        assert_ne!(hash, changed);
        let builds: Vec<String> = first
            .jobs
            .iter()
            .filter(|job| job.name.starts_with("build--"))
            .map(|job| job.name.clone())
            .collect();
        assert_eq!(builds.len(), 2);
        for test in first
            .jobs
            .iter()
            .filter(|job| job.name.starts_with("test--"))
        {
            assert_eq!(test.needs, builds);
        }
    }

    #[test]
    fn malformed_programmatic_plans_fail_closed_without_name_collisions() {
        let prefix = "a".repeat(70);
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![
                JobDef::normal(format!("{prefix}x"), PINNED, ["a"])
                    .with_matrix("os", vec!["linux".into()]),
                JobDef::normal(format!("{prefix}y"), PINNED2, ["b"])
                    .with_matrix("os", vec!["linux".into()]),
            ],
        };
        let (plan, _) = resolve_snapshot(&def, &blobs(), &tenant()).unwrap();
        assert_ne!(plan.jobs[0].name, plan.jobs[1].name);
        let bad = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("unicode-雪", PINNED, ["run"])],
        };
        assert!(matches!(
            resolve_snapshot(&bad, &blobs(), &tenant()),
            Err(ResolveError::InvalidPlan(_))
        ));
        let empty = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal(
                "build",
                PINNED,
                std::iter::empty::<String>(),
            )],
        };
        assert!(matches!(
            resolve_snapshot(&empty, &blobs(), &tenant()),
            Err(ResolveError::InvalidPlan(_))
        ));
    }

    #[test]
    fn snapshot_ref_is_exact_tenant_scoped_lowercase_blake3() {
        let address = ContentHash::blake3(b"snapshot");
        let reference = snapshot_ref(&tenant(), &address);
        assert_eq!(
            reference.0,
            format!(
                "myelin://acme/ci/snapshot/{}",
                address.to_multihash_string()
            )
        );
        let digest = reference
            .0
            .strip_prefix("myelin://acme/ci/snapshot/blake3:")
            .unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn structural_defects_are_rejected() {
        let store = blobs();
        assert_eq!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![]
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::EmptyDefinition)
        );
        assert!(matches!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![JobDef::normal("a", PINNED, ["a"]).with_needs(["ghost"])],
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::UnknownNeed { need, .. }) if need == "ghost"
        ));
        assert_eq!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![
                        JobDef::normal("a", PINNED, ["a"]).with_needs(["b"]),
                        JobDef::normal("b", PINNED, ["b"]).with_needs(["a"]),
                    ],
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::Cyclic)
        );
        assert!(matches!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![JobDef::normal("a", PINNED, ["a"]), JobDef::normal("a", PINNED2, ["a"])],
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::DuplicateJob(n)) if n == "a"
        ));
        assert_eq!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![JobDef::normal("a", PINNED, ["a"]).with_needs(["a"])]
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::SelfNeed("a".into()))
        );
        assert_eq!(
            resolve_snapshot(
                &CiDefinition {
                    on: OnTrigger::Push,
                    jobs: vec![
                        JobDef::normal("a", PINNED, ["a"]),
                        JobDef::normal("b", PINNED2, ["b"]).with_needs(["a", "a"]),
                    ]
                },
                &store,
                &tenant()
            ),
            Err(ResolveError::DuplicateNeed {
                job: "b".into(),
                need: "a".into()
            })
        );
    }

    #[test]
    fn dynamic_generation_is_refused_until_fragment_ingestion_exists() {
        let def = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("gen-matrix", PINNED, ["generate"]).as_generator()],
        };
        assert!(matches!(
            resolve_snapshot(&def, &blobs(), &tenant()),
            Err(ResolveError::InvalidPlan(_))
        ));
        let plain = CiDefinition {
            on: OnTrigger::Push,
            jobs: vec![JobDef::normal("build", PINNED, ["build"])],
        };
        let (s2, _) = resolve_snapshot(&plain, &blobs(), &tenant()).unwrap();
        assert!(!s2.has_dynamic_generation());
    }

    fn facts() -> RunFacts {
        RunFacts {
            run_id: "run-0001".into(),
            tenant_id: "acme".into(),
            repo_ref: "myelin://acme/git/repo/web".into(),
            source_ref: Some("refs/heads/main".into()),
            commit_oid: "deadbeef".into(),
            contexts: vec![CheckContext::ci("build"), CheckContext::ci("test/unit")],
            cause_event_id: EventId("ev-push-1".into()),
            started_at: "2026-07-23T00:00:00Z".into(),
        }
    }

    #[test]
    fn the_reserve_start_handoff_is_an_atomic_bundle() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        let handoff = reserve_and_start(&snap, &member_stamp(), &OnTrigger::Push, &facts());

        assert!(
            handoff.is_atomic_bundle(),
            "the row + ci.run.started + the queued checks are one atomic bundle"
        );
        assert_eq!(handoff.run_write.state, "queued");
        assert_eq!(handoff.run_write.definition_snapshot, snap);
        assert_eq!(handoff.run_write.trust_tier, "trusted");
        assert_eq!(handoff.run_write.trigger_kind, "push");
        assert_eq!(
            handoff.run_started.subject.0,
            "myelin://acme/ci/run/run-0001",
            "the run subject is a canonical scoped ref, never a bare path"
        );
        assert_eq!(
            handoff.run_started.aggregate.0, "run:run-0001",
            "the run aggregate is the canonical type:id partition"
        );
        assert_eq!(handoff.run_started.type_.0, CI_RUN_STARTED);
        assert_eq!(handoff.run_started.payload["trust_tier"], "trusted");
        assert_eq!(handoff.run_started.payload["definition_snapshot"], snap.0);
        assert_eq!(
            handoff.queued_checks.len(),
            2,
            "one queued check per context"
        );
        for c in &handoff.queued_checks {
            assert_eq!(c.type_.0, myelin_ci_sandbox::events::CI_CHECK_UPDATED);
            assert_eq!(c.payload["state"], "queued");
            assert_eq!(
                c.payload["run_attempt"], 0,
                "the pure planner carries a non-emittable template; durable reserve allocates it"
            );
        }
        assert_eq!(handoff.start_spec.wf_type, CI_PIPELINE_WF_TYPE);
        assert_eq!(handoff.start_spec.input, vec![snap]);
        assert_eq!(handoff.start_spec.idem_key, "run-0001:ev-push-1");
    }

    #[test]
    fn the_trust_tier_rides_the_row_and_every_check_zero_divergence() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        let handoff = reserve_and_start(&snap, &fork_stamp(), &OnTrigger::PullRequest, &facts());
        assert_eq!(
            handoff.run_write.trust_tier, "untrusted_fork",
            "the 3-way row tier"
        );
        assert_eq!(handoff.run_write.trigger_kind, "pull_request");
        for c in &handoff.queued_checks {
            assert_eq!(
                c.payload["trust_tier"], "untrusted_fork",
                "the queued check carries the SAME fork verdict (X-1, 0 divergence)"
            );
        }
        assert!(handoff.is_atomic_bundle());
    }

    #[test]
    fn the_queued_check_subject_is_the_x1_seam_grammar() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        let handoff = reserve_and_start(&snap, &member_stamp(), &OnTrigger::Push, &facts());
        let build = &handoff.queued_checks[0];
        assert_eq!(
            build.subject.0, "myelin://acme/git/repo/web#commit-deadbeef/check-build",
            "the X-1 check subject grammar"
        );
        for c in &handoff.queued_checks {
            assert_eq!(
                c.aggregate,
                myelin_events::check_seam::check_aggregate(
                    "myelin://acme/git/repo/web",
                    "deadbeef",
                ),
                "the per-commit aggregate (the ordering partition the contexts share)"
            );
        }
    }

    #[test]
    fn trigger_kinds_map_to_the_frozen_check_tokens() {
        let snap = snapshot_ref(&tenant(), &ContentHash::blake3(b"snap"));
        for (on, tok) in [
            (OnTrigger::Push, "push"),
            (OnTrigger::PullRequest, "pull_request"),
            (OnTrigger::IssueTransitioned, "issue_transition"),
            (OnTrigger::Manual, "manual"),
            (OnTrigger::Schedule, "schedule"),
            (OnTrigger::Agent, "agent"),
        ] {
            let h = reserve_and_start(&snap, &member_stamp(), &on, &facts());
            assert_eq!(h.run_write.trigger_kind, tok, "trigger_kind for {on:?}");
        }
    }
}
