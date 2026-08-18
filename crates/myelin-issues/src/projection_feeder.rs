use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_storage::migration::is_blocking_alter;
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::cost_bounder::FacetCatalog;
use crate::events::ISSUE_UPDATED;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FacetKey {
    pub tenant: String,
    pub type_: String,
    pub field_id: String,
}

impl FacetKey {
    pub fn new(
        tenant: impl Into<String>,
        type_: impl Into<String>,
        field_id: impl Into<String>,
    ) -> FacetKey {
        FacetKey {
            tenant: tenant.into(),
            type_: type_.into(),
            field_id: field_id.into(),
        }
    }

    pub fn collection(&self) -> CollectionKey {
        CollectionKey {
            tenant: self.tenant.clone(),
            type_: self.type_.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CollectionKey {
    pub tenant: String,
    pub type_: String,
}

impl CollectionKey {
    pub fn new(tenant: impl Into<String>, type_: impl Into<String>) -> CollectionKey {
        CollectionKey {
            tenant: tenant.into(),
            type_: type_.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PromotionThreshold {
    pub share: f64,
}

impl PromotionThreshold {
    pub const OQ_C_DEFAULT_TO_BEAT: f64 = 0.05;

    pub const DEFAULT: PromotionThreshold = PromotionThreshold {
        share: Self::OQ_C_DEFAULT_TO_BEAT,
    };

    pub fn new(share: f64) -> PromotionThreshold {
        PromotionThreshold { share }
    }
}

impl Default for PromotionThreshold {
    fn default() -> PromotionThreshold {
        PromotionThreshold::DEFAULT
    }
}

#[derive(Clone, Debug, Default)]
pub struct FrequencyCounter {
    appearances: BTreeMap<FacetKey, u64>,
    executions: BTreeMap<CollectionKey, u64>,
}

impl FrequencyCounter {
    pub fn new() -> FrequencyCounter {
        FrequencyCounter::default()
    }

    pub fn record_view_execution(&mut self, collection: &CollectionKey, filtered_facets: &[&str]) {
        *self.executions.entry(collection.clone()).or_insert(0) += 1;
        for &field_id in filtered_facets {
            let key = FacetKey {
                tenant: collection.tenant.clone(),
                type_: collection.type_.clone(),
                field_id: field_id.to_string(),
            };
            *self.appearances.entry(key).or_insert(0) += 1;
        }
    }

    pub fn executions(&self, collection: &CollectionKey) -> u64 {
        self.executions.get(collection).copied().unwrap_or(0)
    }

    pub fn appearances(&self, facet: &FacetKey) -> u64 {
        self.appearances.get(facet).copied().unwrap_or(0)
    }

    pub fn share(&self, facet: &FacetKey) -> f64 {
        let execs = self.executions(&facet.collection());
        if execs == 0 {
            return 0.0;
        }
        self.appearances(facet) as f64 / execs as f64
    }
}

pub const ISSUE_HOT_TABLE: &str = "issue";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexProvisioning {
    pub facet: FacetKey,
    pub index_name: String,
    pub ddl: String,
    pub table: &'static str,
}

impl IndexProvisioning {
    pub fn for_facet(facet: &FacetKey) -> IndexProvisioning {
        let index_name = index_name_for(facet);
        let field = sql_literal(&facet.field_id);
        let tenant = sql_literal(&facet.tenant);
        let type_ = sql_literal(&facet.type_);
        let ddl = format!(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS {index_name} \
             ON {ISSUE_HOT_TABLE} ((props ->> {field})) \
             WHERE tenant_id = {tenant} AND type_id::text = {type_} AND deleted_at IS NULL",
        );
        IndexProvisioning {
            facet: facet.clone(),
            index_name,
            ddl,
            table: ISSUE_HOT_TABLE,
        }
    }

    pub fn is_non_blocking(&self) -> bool {
        !is_blocking_alter(&self.ddl)
    }

    pub fn is_forward_only(&self) -> bool {
        let up = self.ddl.to_ascii_uppercase();
        !up.contains("DROP ") && !up.contains("DROP\t")
    }
}

fn sanitize_ident(field_id: &str) -> String {
    field_id
        .chars()
        .take(22)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn index_name_for(facet: &FacetKey) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.issue.facet-index.v1\0");
    for part in [&facet.tenant, &facet.type_, &facet.field_id] {
        hash.update(&(part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize().to_hex();
    format!(
        "issue_facet_{}_{:.24}",
        sanitize_ident(&facet.field_id),
        digest
    )
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[derive(Clone, Debug, PartialEq)]
pub enum PromotionDecision {
    Promoted(IndexProvisioning),
    StayedOnGin { share: f64 },
    AlreadyPromoted,
}

impl PromotionDecision {
    pub fn is_promoted(&self) -> bool {
        matches!(self, PromotionDecision::Promoted(_))
    }
}

pub struct ProjectionFeeder {
    threshold: PromotionThreshold,
    state: Mutex<FeederState>,
}

#[derive(Default)]
struct FeederState {
    counter: FrequencyCounter,
    catalog: FacetCatalog,
    promoted: std::collections::BTreeSet<FacetKey>,
    seen_events: std::collections::BTreeSet<String>,
}

fn feeder_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| vec![SubjectPattern(ISSUE_UPDATED.to_string())])
        .as_slice()
}

impl ProjectionFeeder {
    pub fn new() -> ProjectionFeeder {
        ProjectionFeeder::with_threshold(PromotionThreshold::DEFAULT)
    }

    pub fn with_threshold(threshold: PromotionThreshold) -> ProjectionFeeder {
        ProjectionFeeder {
            threshold,
            state: Mutex::new(FeederState::default()),
        }
    }

    pub fn record_view_execution(&self, collection: &CollectionKey, filtered_facets: &[&str]) {
        let mut state = self.state.lock().expect("feeder state lock");
        state
            .counter
            .record_view_execution(collection, filtered_facets);
    }

    pub fn should_promote(&self, facet: &FacetKey) -> bool {
        let state = self.state.lock().expect("feeder state lock");
        Self::should_promote_locked(&state, self.threshold, facet)
    }

    fn should_promote_locked(
        state: &FeederState,
        threshold: PromotionThreshold,
        facet: &FacetKey,
    ) -> bool {
        if state.promoted.contains(facet) {
            return false;
        }
        state.counter.share(facet) > threshold.share
    }

    pub fn evaluate_facet(&self, facet: &FacetKey) -> PromotionDecision {
        let mut state = self.state.lock().expect("feeder state lock");
        Self::evaluate_facet_locked(&mut state, self.threshold, facet)
    }

    fn evaluate_facet_locked(
        state: &mut FeederState,
        threshold: PromotionThreshold,
        facet: &FacetKey,
    ) -> PromotionDecision {
        if state.promoted.contains(facet) {
            return PromotionDecision::AlreadyPromoted;
        }
        let share = state.counter.share(facet);
        if share <= threshold.share {
            return PromotionDecision::StayedOnGin { share };
        }
        let provisioning = IndexProvisioning::for_facet(facet);
        debug_assert!(
            provisioning.is_non_blocking() && provisioning.is_forward_only(),
            "the feeder must only provision a non-blocking, forward-only online migration"
        );
        state.catalog.promote(facet.field_id.clone());
        state.promoted.insert(facet.clone());
        PromotionDecision::Promoted(provisioning)
    }

    pub fn catalog_snapshot(&self) -> FacetCatalog {
        let state = self.state.lock().expect("feeder state lock");
        state.catalog.clone()
    }

    pub fn is_promoted(&self, facet: &FacetKey) -> bool {
        let state = self.state.lock().expect("feeder state lock");
        state.promoted.contains(facet)
    }

    fn facets_in_event(ev: &EventEnvelope) -> Result<Vec<FacetKey>, String> {
        let subject = myelin_refs::parse_scoped(&ev.subject.0)
            .map_err(|error| format!("invalid issue subject: {error}"))?;
        if subject.artifact_ref != ev.subject {
            return Err("issue subject must use its exact canonical representation".into());
        }
        if subject.tenant != ev.tenant {
            return Err("issue subject belongs to another tenant".into());
        }
        if subject.subsystem != "issue" || subject.type_ != "issue" || subject.sub.is_some() {
            return Err("issue.updated subject must identify an issue root".into());
        }

        let obj = ev
            .payload
            .as_object()
            .ok_or_else(|| "issue.updated payload must be an object".to_string())?;
        let required_string = |key: &str| {
            obj.get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("issue.updated payload `{key}` must be a string"))
        };

        let payload_issue = required_string("issue")?;
        if payload_issue != ev.subject.0 {
            return Err("issue.updated payload `issue` must equal the envelope subject".into());
        }
        let issue_local_id = required_string("issue_local_id")?;
        if issue_local_id != subject.id {
            return Err("issue.updated payload `issue_local_id` must equal the subject id".into());
        }

        let type_id = required_string("type_id")?.to_string();
        crate::write_path::validate_type_id(&type_id)?;
        let changed = obj
            .get("changed_facets")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "issue.updated payload `changed_facets` must be an array".to_string())?;
        let changed_facets = changed
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    "issue.updated payload `changed_facets` must contain only strings".to_string()
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::write_path::validate_changed_facets(&changed_facets)?;

        Ok(changed_facets
            .into_iter()
            .map(|field_id| FacetKey::new(ev.tenant.0.clone(), type_id.clone(), field_id))
            .collect())
    }
}

impl Default for ProjectionFeeder {
    fn default() -> ProjectionFeeder {
        ProjectionFeeder::new()
    }
}

impl EventHandler for ProjectionFeeder {
    fn subjects(&self) -> &'static [SubjectPattern] {
        feeder_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.type_.0 != ISSUE_UPDATED {
            return HandleOutcome::NonRetryable(Reason(format!(
                "projection feeder bound to `{ISSUE_UPDATED}` received `{}` - misroute",
                ev.type_.0
            )));
        }
        let facets = match Self::facets_in_event(ev) {
            Ok(facets) => facets,
            Err(error) => {
                return HandleOutcome::NonRetryable(Reason(format!(
                    "malformed `{ISSUE_UPDATED}` event: {error}"
                )))
            }
        };
        let mut state = self.state.lock().expect("feeder state lock");
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        for facet in facets {
            let _ = Self::evaluate_facet_locked(&mut state, self.threshold, &facet);
        }
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests;
