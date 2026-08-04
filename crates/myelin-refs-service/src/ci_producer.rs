use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::ArtifactRef;
use myelin_git::check_status::{
    ApplyOutcome, CheckState, CheckStatus, CheckStatusProjection, CheckStatusRow,
};
use myelin_identity::{Decision, Permission, Principal};
use myelin_refs::{sub_kind, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::ladder::SubState;
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

pub const CI_OWNER_TOKEN: &str = "ci";

pub trait StepAnchorResolver: Send + Sync {
    fn resolve_step(&self, anchor: &ArtifactRef) -> StepResolution;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepResolution {
    Live { byte_len: u64 },
    Gone,
    Erased,
}

impl StepResolution {
    fn into_sub_state(self, mut projection: OwnerProjection) -> SubState {
        match self {
            StepResolution::Live { byte_len } => {
                projection.state = format!("failing-step ({byte_len} bytes)");
                SubState::Live(projection)
            }
            StepResolution::Gone => SubState::Gone,
            StepResolution::Erased => SubState::Erased,
        }
    }
}

fn check_state_into_sub_state(state: CheckState, projection: OwnerProjection) -> SubState {
    match state {
        CheckState::Success
        | CheckState::Failure
        | CheckState::Error
        | CheckState::Neutral
        | CheckState::Cancelled => SubState::Live(projection),
        CheckState::Queued | CheckState::InProgress => SubState::Outdated(projection),
    }
}

#[derive(Clone)]
pub struct CiOwner {
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    checks: Arc<Mutex<BTreeMap<String, CheckState>>>,
    projection: Arc<Mutex<CheckStatusProjection>>,
    step_resolver: Arc<Mutex<Option<Arc<dyn StepAnchorResolver>>>>,
}

impl Default for CiOwner {
    fn default() -> Self {
        CiOwner {
            acl: Arc::new(Mutex::new(BTreeMap::new())),
            checks: Arc::new(Mutex::new(BTreeMap::new())),
            projection: Arc::new(Mutex::new(CheckStatusProjection::new())),
            step_resolver: Arc::new(Mutex::new(None)),
        }
    }
}

impl CiOwner {
    pub fn new() -> CiOwner {
        CiOwner::default()
    }

    pub fn check_root(tenant: &str, commit: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/ci/check/{commit}"))
    }

    pub fn check_anchor(tenant: &str, commit: &str, context: &str) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{tenant}/ci/check/{commit}#check-{context}"
        ))
    }

    pub fn run_root(tenant: &str, run: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/ci/run/{run}"))
    }

    pub fn step_anchor(tenant: &str, run: &str, step_no: u32) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/ci/run/{run}#step-{step_no}"))
    }

    fn acl_key(
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        root: &ArtifactRef,
    ) -> String {
        format!(
            "{}|{}|{}|{}",
            tenant.0, region.0, viewer.principal_id.0, root.0
        )
    }

    pub fn grant_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        root: &ArtifactRef,
    ) {
        self.acl
            .lock()
            .unwrap()
            .insert(Self::acl_key(tenant, region, viewer, root), Decision::Allow);
    }

    pub fn ingest_check(&self, ref_: &ArtifactRef, fact: &CheckStatus) -> ApplyOutcome {
        let outcome = self.projection.lock().unwrap().apply(fact);
        let key = fact.key();
        if let Some(row) = self.projection.lock().unwrap().current(&key) {
            self.checks
                .lock()
                .unwrap()
                .insert(ref_.0.clone(), row.state);
        }
        outcome
    }

    pub fn current_row(&self, fact: &CheckStatus) -> Option<CheckStatusRow> {
        self.projection
            .lock()
            .unwrap()
            .current(&fact.key())
            .cloned()
    }

    pub fn wire_step_resolver(&self, resolver: Arc<dyn StepAnchorResolver>) {
        *self.step_resolver.lock().unwrap() = Some(resolver);
    }

    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "a CI check".into(),
            state: "live".into(),
            icon: "ci".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    fn resolve_ci_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            None => SubState::Live(projection),
            Some(Sub::Check(_)) => {
                let state = self
                    .checks
                    .lock()
                    .unwrap()
                    .get(&ref_.0)
                    .copied()
                    .unwrap_or(CheckState::InProgress);
                check_state_into_sub_state(state, projection)
            }
            Some(Sub::Step(_)) => match self.step_resolver.lock().unwrap().as_ref() {
                Some(resolver) => resolver.resolve_step(ref_).into_sub_state(projection),
                None => SubState::Gone,
            },
            Some(_) => SubState::Live(projection),
        }
    }
}

impl ProjectApi for CiOwner {
    fn check_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        _permission: &Permission,
    ) -> std::result::Result<Decision, ProjectApiError> {
        let key = Self::acl_key(tenant, region, viewer, object);
        Ok(self
            .acl
            .lock()
            .unwrap()
            .get(&key)
            .copied()
            .unwrap_or(Decision::Deny))
    }

    fn project(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _mode: ResolveMode,
    ) -> std::result::Result<ProjectOutcome, ProjectApiError> {
        let sub = sub_kind(ref_);
        Ok(self.resolve_ci_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for CiOwner {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_ci_sub(ref_, sub)
    }
}

#[cfg(test)]
mod tests;
