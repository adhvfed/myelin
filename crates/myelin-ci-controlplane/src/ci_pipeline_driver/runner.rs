use std::sync::Arc;

use myelin_ci_sandbox::{IdemToken, JobSpec as SandboxJobSpec};
use myelin_flow::{ActivityError, JobRunner, JobSpec as FlowJobSpec};

use crate::ci_pipeline::PipelineStage;
use crate::job_queue_store::DurableEnqueue;
use crate::job_schedule::JobScheduleTerms;
use crate::job_spec_store::{CiJobSpecStore, DurableCiJobLaunchTemplate, MAX_JOB_TIMEOUT_SECS};

use super::bridge;

pub type StageSpecBuilder =
    Arc<dyn Fn(&FlowJobSpec) -> Result<SandboxJobSpec, String> + Send + Sync>;

pub fn unresolved_stage_spec_builder() -> StageSpecBuilder {
    Arc::new(|spec: &FlowJobSpec| {
        Err(format!(
            "no pinned-snapshot → JobSpec resolver yet (CT-004d follow-on) for stage target `{}`; \
             the driver cannot fabricate an executable spec - dispatch refused fail-closed",
            spec.target
        ))
    })
}

pub struct DurableJobRunner {
    store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    terms: JobScheduleTerms,
    build_spec: StageSpecBuilder,
    targets: Vec<(String, String)>,
}

impl DurableJobRunner {
    pub fn new(
        store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
        terms: JobScheduleTerms,
        build_spec: StageSpecBuilder,
        stages: &[PipelineStage],
    ) -> DurableJobRunner {
        let targets = stages
            .iter()
            .map(|s| (s.engine.target.clone(), s.engine.name.clone()))
            .collect();
        DurableJobRunner {
            store,
            rt,
            terms,
            build_spec,
            targets,
        }
    }

    pub(super) fn stage_job_id(idem_token: &str) -> String {
        deterministic_uuid(&format!("jobq:{idem_token}"))
    }

    fn build_dispatch(
        &self,
        flow_spec: &FlowJobSpec,
    ) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
        build_dispatch_parts(&self.terms, &self.build_spec, flow_spec)
    }
}

pub(super) fn build_dispatch_parts(
    terms: &JobScheduleTerms,
    build_spec: &StageSpecBuilder,
    flow_spec: &FlowJobSpec,
) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
    let mut spec = (build_spec)(flow_spec).map_err(ActivityError::retryable)?;

    spec.trust_tier = terms.trust_tier;
    spec.idem_token = IdemToken(flow_spec.idem_token.clone());
    if spec.limits.timeout_secs > MAX_JOB_TIMEOUT_SECS {
        spec.limits.timeout_secs = MAX_JOB_TIMEOUT_SECS;
    }

    let claim_window_secs = crate::ci_claim_window::claim_window_secs(
        spec.kind,
        &spec.workspace,
        spec.limits.timeout_secs,
    )
    .map_err(|error| ActivityError::retryable(error.to_string()))?;

    let enq = DurableEnqueue {
        tenant_id: terms.tenant_id.clone(),
        region: terms.region.clone(),
        job_id: DurableJobRunner::stage_job_id(&flow_spec.idem_token),
        run_id: terms.run_id.clone(),
        lane: terms.lane,
        labels: terms.labels.clone(),
        trust_tier: terms.trust_tier,
        concurrency_group: terms.concurrency_group.clone(),
        fair_key: terms.fair_key.clone(),
        idem_token: flow_spec.idem_token.clone(),
        stage: flow_spec.target.clone(),
        claim_window_secs,
        reservation_write_version: crate::ReservationWriteVersionMarker::derive_from_reserve_handle(
            &spec.meter_to.reserve_id,
        ),
    };
    Ok((enq, spec))
}

impl JobRunner for DurableJobRunner {
    fn dispatch(&self, flow_spec: &FlowJobSpec) -> Result<(), ActivityError> {
        let (mut enq, spec) = self.build_dispatch(flow_spec)?;

        let stage = self
            .targets
            .iter()
            .find(|(t, _)| t == &flow_spec.target)
            .map(|(_, name)| name.clone())
            .ok_or_else(|| {
                ActivityError::retryable(format!(
                    "ci.pipeline dispatch refused: target `{}` is not a known pipeline stage - the \
                     verdict could not be durably attributed (fail-closed)",
                    flow_spec.target
                ))
            })?;
        enq.stage = stage.clone();
        let authority = format!("legacy-test-authority:{}", spec.run_token.jti);
        let (spec, _previous_token) = spec.into_template();
        let launch = DurableCiJobLaunchTemplate {
            ci_run_id: enq.run_id.clone(),
            project_id: "00000000-0000-0000-0000-000000000000".into(),
            spec,
            token_authority_handle: authority,
        };

        bridge(
            &self.rt,
            self.store.co_persist_dispatch(&enq, &launch, &stage),
        )
        .map_err(|e| {
            ActivityError::retryable(format!("durable co_persist_dispatch refused: {e}"))
        })?;
        Ok(())
    }
}

fn deterministic_uuid(seed: &str) -> String {
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
