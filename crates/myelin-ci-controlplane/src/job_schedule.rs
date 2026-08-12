use crate::scheduler::Lane;
use myelin_ci_sandbox::TrustTier;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobScheduleTerms {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub lane: Lane,
    pub labels: Vec<String>,
    pub trust_tier: TrustTier,
    pub concurrency_group: Option<String>,
    pub fair_key: String,
}

impl JobScheduleTerms {
    pub fn new(
        tenant_id: impl Into<String>,
        region: impl Into<String>,
        run_id: impl Into<String>,
        lane: Lane,
        trust_tier: TrustTier,
        fair_key: impl Into<String>,
    ) -> JobScheduleTerms {
        JobScheduleTerms {
            tenant_id: tenant_id.into(),
            region: region.into(),
            run_id: run_id.into(),
            lane,
            labels: Vec::new(),
            trust_tier,
            concurrency_group: None,
            fair_key: fair_key.into(),
        }
    }

    pub fn with_labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_concurrency_group(mut self, group: impl Into<String>) -> Self {
        self.concurrency_group = Some(group.into());
        self
    }
}
