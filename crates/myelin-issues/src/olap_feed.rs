use std::collections::{BTreeMap, BTreeSet};

use myelin_events::{
    EventEnvelope, EventHandler, HandleOutcome, Reason, ReindexSource, SubjectPattern,
};
use myelin_storage::olap::{OlapApply, OlapEvent, OlapIngestError, OlapReadStore};
use myelin_storage::olap_restrict::{AnalyticsAggregate, OlapAnalytics, RestrictionLeakAudit};

use crate::events;
use crate::holder::RestrictionFlag;
use crate::replay::IssueReindexSource;
use crate::workflow::StateCategory;

pub struct IssueOlapFeedFloors;

impl IssueOlapFeedFloors {
    pub const MONTE_CARLO_FORECAST: &'static str =
        "linear forecast over OLAP throughput → Monte-Carlo forecast agent (ADR-08, ISS-P32 / P-495)";

    pub const COLUMNAR_BACKEND: &'static str =
        "ClickHouse-class columnar OLAP backend behind OlapReadStore (Storage P-ST-18, wired)";

    pub const WORKLOG_ELIGIBILITY: &'static str =
        "per-individual worklog analytics-eligibility (OQ-H, [OPEN - LEGAL]) via \
         myelin_storage::olap_restrict::AnalyticsEligibility";
}

pub const ISSUE_ANALYTICS_OLAP: &str = "issue_analytics_olap";

fn issue_olap_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            vec![
                SubjectPattern(events::ISSUE_TRANSITIONED.to_string()),
                SubjectPattern(events::ISSUE_CLOSED.to_string()),
                SubjectPattern(events::ISSUE_REOPENED.to_string()),
                SubjectPattern(events::CYCLE_ISSUE_ADDED.to_string()),
                SubjectPattern(events::CYCLE_ISSUE_REMOVED.to_string()),
                SubjectPattern(events::CYCLE_COMPLETED.to_string()),
                SubjectPattern(events::SLA_BREACHED.to_string()),
                SubjectPattern(events::SLA_MET.to_string()),
            ]
        })
        .as_slice()
}

fn is_analytics_token(type_token: &str) -> bool {
    issue_olap_subjects().iter().any(|p| p.0 == type_token)
}

pub struct IssueOlapConsumer {
    state: std::sync::Mutex<ConsumerState>,
    restriction: RestrictionFlag,
}

struct ConsumerState {
    store: OlapReadStore,
    seen_events: BTreeSet<String>,
    sla_outcomes: BTreeMap<String, SlaOutcome>,
    categories: BTreeMap<String, StateCategory>,
}

impl Default for ConsumerState {
    fn default() -> ConsumerState {
        ConsumerState {
            store: OlapReadStore::pinned_to(myelin_tenancy::Region("fr-par".into())),
            seen_events: BTreeSet::new(),
            sla_outcomes: BTreeMap::new(),
            categories: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlaOutcome {
    Met,
    Breached,
}

impl IssueOlapConsumer {
    pub fn new(region: myelin_tenancy::Region, restriction: RestrictionFlag) -> IssueOlapConsumer {
        IssueOlapConsumer {
            state: std::sync::Mutex::new(ConsumerState {
                store: OlapReadStore::pinned_to(region),
                ..ConsumerState::default()
            }),
            restriction,
        }
    }

    pub fn oltp_read_count(&self) -> u64 {
        0
    }

    pub fn doc_count(&self) -> usize {
        self.state
            .lock()
            .expect("olap consumer lock")
            .store
            .doc_count()
    }

    pub fn parity_bytes(&self) -> Vec<u8> {
        self.state
            .lock()
            .expect("olap consumer lock")
            .store
            .parity_bytes()
    }

    pub fn projection_fingerprint(&self) -> Vec<u8> {
        let state = self.state.lock().expect("olap consumer lock");
        let view: Vec<(String, Option<String>)> = state
            .store
            .docs()
            .map(|d| (d.aggregate_row.clone(), d.subject.clone()))
            .collect();
        serde_json::to_vec(&view)
            .expect("the OLAP projection fingerprint serializes deterministically")
    }

    pub fn analytics<R>(&self, f: impl FnOnce(&IssueOlapAnalytics) -> R) -> R {
        let mut state = self.state.lock().expect("olap consumer lock");
        self.sync_restriction_from_holder(&mut state);
        let view = IssueOlapAnalytics {
            inner: OlapAnalytics::over(&state.store),
            store: &state.store,
            sla_outcomes: &state.sla_outcomes,
        };
        f(&view)
    }

    fn sync_restriction_from_holder(&self, state: &mut ConsumerState) {
        let subjects: Vec<String> = state
            .store
            .docs()
            .filter_map(|d| d.subject.clone())
            .collect();
        for sid in subjects {
            let restricted = self.restriction.is_restricted(&sid);
            state.store.set_restricted(sid, restricted);
        }
    }

    fn project_locked(
        &self,
        state: &mut ConsumerState,
        ev: &EventEnvelope,
    ) -> Result<OlapApply, OlapIngestError> {
        let row = ev.aggregate.0.clone();
        if ev.type_.0 == events::SLA_MET {
            state.sla_outcomes.insert(row.clone(), SlaOutcome::Met);
        } else if ev.type_.0 == events::SLA_BREACHED {
            state.sla_outcomes.insert(row.clone(), SlaOutcome::Breached);
        } else if ev.type_.0 == events::ISSUE_TRANSITIONED || ev.type_.0 == events::ISSUE_CLOSED {
            if let Some(cat) = category_from_payload(&ev.payload) {
                state.categories.insert(row.clone(), cat);
            }
        }
        let olap_event = OlapEvent::from_envelope(ev);
        state.store.apply(&olap_event)
    }

    pub fn reindex_from(&self, source: &IssueReindexSource, ctx: &ReindexCtx) -> usize {
        let mut state = self.state.lock().expect("olap consumer lock");
        state.store = OlapReadStore::pinned_to(ctx.region.clone());
        state.seen_events.clear();
        state.sla_outcomes.clear();
        state.categories.clear();
        let mut projected = 0;
        for env in ctx.replay_envelopes(source) {
            if !is_analytics_token(&env.type_.0) {
                continue;
            }
            if state.seen_events.insert(env.event_id.0.clone())
                && self.project_locked(&mut state, &env).is_ok()
            {
                projected += 1;
            }
        }
        projected
    }
}

impl EventHandler for IssueOlapConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        issue_olap_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if !is_analytics_token(&ev.type_.0) {
            return HandleOutcome::Done;
        }
        let mut state = self.state.lock().expect("olap consumer lock");
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        match self.project_locked(&mut state, ev) {
            Ok(_) => HandleOutcome::Done,
            Err(OlapIngestError::OutOfRegion { .. }) => HandleOutcome::NonRetryable(Reason(
                "olap feed: event region ≠ the OLAP store's pinned region - a misroute the residency \
                 boundary rejects (per-cell, not a global warehouse)"
                    .into(),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReindexCtx {
    pub tenant: myelin_tenancy::TenantId,
    pub region: myelin_tenancy::Region,
}

impl ReindexCtx {
    pub fn new(tenant: myelin_tenancy::TenantId, region: myelin_tenancy::Region) -> ReindexCtx {
        ReindexCtx { tenant, region }
    }

    fn replay_envelopes(&self, source: &IssueReindexSource) -> Vec<EventEnvelope> {
        use myelin_events::snapshot_event_id;
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
        };
        let scope = myelin_events::SnapshotScope::new("issue", "issue:all");
        source
            .replay(&scope, None)
            .into_iter()
            .filter_map(|draft| {
                let token = draft
                    .payload
                    .get("olap_token")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())?;
                Some(EventEnvelope {
                    event_id: EventId(format!(
                        "olap-reindex:{}",
                        snapshot_event_id(&self.tenant, &draft.aggregate, draft.version).0
                    )),
                    type_: EventType(token),
                    schema_ver: 1,
                    tenant: self.tenant.clone(),
                    region: self.region.clone(),
                    actor: Actor(myelin_identity::Principal::stub(
                        myelin_identity::PrincipalId("reindex".into()),
                        myelin_identity::PrincipalKind::Service,
                        self.tenant.clone(),
                    )),
                    subject: draft.subject.clone(),
                    aggregate: AggregateKey(draft.aggregate.0.clone()),
                    causation_id: None,
                    correlation_id: CorrelationId(format!("olap-reindex:{}", draft.aggregate.0)),
                    caused_by: None,
                    depth: 0,
                    contains_personal_data: false,
                    data_role: DataRole::Controller,
                    visibility: Visibility::Internal,
                    pii_key_ref: None,
                    occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
                    recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
                    payload: draft.payload.clone(),
                })
            })
            .collect()
    }
}

pub struct IssueOlapAnalytics<'a> {
    inner: OlapAnalytics<'a>,
    store: &'a OlapReadStore,
    sla_outcomes: &'a BTreeMap<String, SlaOutcome>,
}

impl<'a> IssueOlapAnalytics<'a> {
    pub fn cfd(&self) -> BTreeMap<String, u64> {
        self.inner.cfd()
    }

    pub fn cycle_time_sample_size(&self) -> u64 {
        self.inner.cycle_time_sample_size()
    }

    pub fn velocity(&self) -> u64 {
        self.inner.velocity()
    }

    pub fn sla_compliance(&self) -> Option<f64> {
        let mut met = 0u64;
        let mut total = 0u64;
        for (row, outcome) in self.sla_outcomes {
            if self.row_is_restricted(row) {
                continue;
            }
            total += 1;
            if *outcome == SlaOutcome::Met {
                met += 1;
            }
        }
        if total == 0 {
            None
        } else {
            Some(met as f64 / total as f64)
        }
    }

    pub fn sla_sample_size(&self) -> u64 {
        self.sla_outcomes
            .keys()
            .filter(|row| !self.row_is_restricted(row))
            .count() as u64
    }

    fn row_is_restricted(&self, row: &str) -> bool {
        self.store
            .doc(row)
            .and_then(|d| d.subject.as_deref())
            .is_some_and(|s| self.store.is_restricted(s))
    }

    pub fn leak_audit(&self) -> IssueRestrictionLeakAudit {
        let cross_team = self.inner.leak_audit();
        let restricted: BTreeSet<String> = self.store.restricted_subjects().cloned().collect();
        let mut sla_leaked: BTreeSet<String> = BTreeSet::new();
        for row in self.sla_outcomes.keys() {
            if let Some(subj) = self.store.doc(row).and_then(|d| d.subject.clone()) {
                if restricted.contains(&subj) && !self.row_is_restricted(row) {
                    sla_leaked.insert(subj);
                }
            }
        }
        let mut leaked = cross_team.leaked_subjects.clone();
        leaked.extend(sla_leaked);
        IssueRestrictionLeakAudit {
            restricted_subject_leak: leaked.len() as u64,
            cross_team,
            leaked_subjects: leaked,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueRestrictionLeakAudit {
    pub restricted_subject_leak: u64,
    pub cross_team: RestrictionLeakAudit,
    pub leaked_subjects: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueOlapFeedSignal {
    pub store: &'static str,
    pub oltp_read_count: u64,
    pub restricted_subject_leak: u64,
    pub subjects_restricted: u64,
    pub reindex_matches_live: bool,
}

impl IssueOlapFeedSignal {
    pub fn is_green(&self) -> bool {
        self.oltp_read_count == 0
            && self.restricted_subject_leak == 0
            && self.reindex_matches_live
            && self.subjects_restricted >= 1
    }
}

fn category_from_payload(payload: &serde_json::Value) -> Option<StateCategory> {
    payload
        .get("category")
        .and_then(|v| v.as_str())
        .and_then(|tok| StateCategory::parse(tok).ok())
}

pub fn issue_analytics_aggregate_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = AnalyticsAggregate::ALL.iter().map(|a| a.name()).collect();
    names.push("sla_compliance");
    names
}

#[cfg(test)]
#[path = "olap_feed/tests.rs"]
mod tests;
