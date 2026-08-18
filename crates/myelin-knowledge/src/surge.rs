use std::collections::HashMap;

use myelin_identity::Principal;
use myelin_query::field::{Jitter, OrderKey};
use myelin_substrate::shed::{
    BoundedQueue, RunClass, RunClassHeader, ShedDecision, ShedLane, Surface as ShedSurface,
    SurfaceBudget,
};
use myelin_substrate::Thresholds;
use myelin_tenancy::TenantId;

pub const COLLAB_SURGE_MULTIPLIER: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollabShedRejection {
    pub lane: RunClass,
    pub reason: CollabShedReason,
    pub retry_after_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollabShedReason {
    OpStreamLane,
    PerDocOpCap,
    ReadFanout,
}

pub struct CollabSurgeGate {
    lane: ShedLane,
    per_doc_op: HashMap<String, BoundedQueue>,
    per_doc_op_cap: u32,
    read_fanout: HashMap<String, BoundedQueue>,
    read_fanout_cap: u32,
    retry_after_secs: u64,
}

impl CollabSurgeGate {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<CollabSurgeGate, String> {
        let budget = thresholds
            .shed_budget(ShedSurface::CollabOpStream)
            .map_err(|e| format!("Knowledge shed budget for CollabOpStream unavailable: {e}"))?;
        Ok(CollabSurgeGate::with_budget(budget))
    }

    pub fn with_budget(budget: SurfaceBudget) -> CollabSurgeGate {
        CollabSurgeGate::with_budget_and_bounds(
            budget,
            budget.per_tenant_in_flight_cap,
            budget.per_tenant_in_flight_cap,
        )
    }

    pub fn with_budget_and_bounds(
        budget: SurfaceBudget,
        per_doc_op_cap: u32,
        read_fanout_cap: u32,
    ) -> CollabSurgeGate {
        CollabSurgeGate {
            lane: ShedLane::with_budget(ShedSurface::CollabOpStream, budget),
            per_doc_op: HashMap::new(),
            per_doc_op_cap,
            read_fanout: HashMap::new(),
            read_fanout_cap,
            retry_after_secs: budget.retry_after_secs,
        }
    }

    pub fn derive_class(principal: &Principal, header: Option<RunClassHeader>) -> RunClass {
        RunClass::derive(&principal.kind, header)
    }

    pub fn admit_for(
        &mut self,
        principal: &Principal,
        page_id: &str,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, CollabShedRejection> {
        let class = Self::derive_class(principal, header);
        self.admit_doc_op(&principal.tenant, page_id, class)
            .map(|()| class)
    }

    pub fn admit_doc_op(
        &mut self,
        tenant: &TenantId,
        page_id: &str,
        class: RunClass,
    ) -> Result<(), CollabShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => {}
            ShedDecision::Shed { retry_after_secs } => {
                return Err(CollabShedRejection {
                    lane: class,
                    reason: CollabShedReason::OpStreamLane,
                    retry_after_secs,
                });
            }
        }
        let cap = self.per_doc_op_cap;
        let q = self
            .per_doc_op
            .entry(page_id.to_string())
            .or_insert_with(|| BoundedQueue::new(cap));
        if q.try_acquire() {
            Ok(())
        } else {
            self.lane.release(tenant, class);
            Err(CollabShedRejection {
                lane: class,
                reason: CollabShedReason::PerDocOpCap,
                retry_after_secs: self.retry_after_secs,
            })
        }
    }

    pub fn admit_read_fanout(&mut self, page_id: &str) -> Result<(), CollabShedRejection> {
        let cap = self.read_fanout_cap;
        let q = self
            .read_fanout
            .entry(page_id.to_string())
            .or_insert_with(|| BoundedQueue::new(cap));
        if q.try_acquire() {
            Ok(())
        } else {
            Err(CollabShedRejection {
                lane: RunClass::Speculative,
                reason: CollabShedReason::ReadFanout,
                retry_after_secs: self.retry_after_secs,
            })
        }
    }

    pub fn release_op(&mut self, tenant: &TenantId, page_id: &str, class: RunClass) {
        self.lane.release(tenant, class);
        if let Some(q) = self.per_doc_op.get_mut(page_id) {
            q.release();
        }
    }

    pub fn release_read_fanout(&mut self, page_id: &str) {
        if let Some(q) = self.read_fanout.get_mut(page_id) {
            q.release();
        }
    }

    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }

    pub fn doc_in_flight(&self, page_id: &str) -> u32 {
        self.per_doc_op
            .get(page_id)
            .map(|q| q.in_flight())
            .unwrap_or(0)
    }

    pub fn doc_op_shed_count(&self, page_id: &str) -> u64 {
        self.per_doc_op
            .get(page_id)
            .map(|q| q.shed_count())
            .unwrap_or(0)
    }

    pub fn read_fanout_shed_count(&self, page_id: &str) -> u64 {
        self.read_fanout
            .get(page_id)
            .map(|q| q.shed_count())
            .unwrap_or(0)
    }

    pub fn surface(&self) -> ShedSurface {
        self.lane.surface()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexoStormReport {
    pub inserts: usize,
    pub distinct_keys: usize,
    pub all_within_gap: bool,
    pub rebalance_triggers: usize,
}

impl LexoStormReport {
    pub fn is_green(&self) -> bool {
        self.distinct_keys == self.inserts && self.all_within_gap && self.rebalance_triggers == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "KN-D8 LexoRank-storm: inserts={} distinct_keys={} all_within_gap={} \
             rebalance_triggers={} → {}",
            self.inserts,
            self.distinct_keys,
            self.all_within_gap,
            self.rebalance_triggers,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

pub fn run_lexorank_storm(
    lo: Option<&OrderKey>,
    hi: Option<&OrderKey>,
    inserts: usize,
) -> LexoStormReport {
    let mut keys: Vec<OrderKey> = Vec::with_capacity(inserts);
    for i in 0..inserts {
        let a = i % 62;
        let b = (i / 62) % 62;
        let jitter = Jitter::from_ranks(a, b).expect("ranks < 62 are in-alphabet");
        keys.push(OrderKey::rank_between(lo, hi, jitter));
    }

    let distinct: std::collections::BTreeSet<&str> = keys.iter().map(|k| k.as_str()).collect();
    let distinct_keys = distinct.len();

    let all_within_gap = keys.iter().all(|k| {
        let above_lo = lo.map(|l| k.as_str() > l.as_str()).unwrap_or(true);
        let below_hi = hi.map(|h| k.as_str() < h.as_str()).unwrap_or(true);
        above_lo && below_hi
    });

    let rebalance_triggers = keys.iter().filter(|k| k.needs_rebalance()).count();

    LexoStormReport {
        inserts,
        distinct_keys,
        all_within_gap,
        rebalance_triggers,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollabSurgeReport {
    pub surging_agent_shed_count: u64,
    pub surging_viewer_shed_count: u64,
    pub surging_human_shed_count: u64,
    pub surging_human_admitted: bool,
    pub quiet_human_admitted: bool,
    pub cross_tenant_impact: u32,
    pub hot_doc_op_cap_shed_count: u64,
    pub hot_doc_read_fanout_shed_count: u64,
}

impl CollabSurgeReport {
    pub fn is_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_viewer_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
            && self.hot_doc_op_cap_shed_count > 0
            && self.hot_doc_read_fanout_shed_count > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "KN-D8/F6: surging agent_shed={} viewer_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} hot_doc_op_cap_shed={} \
             hot_doc_read_fanout_shed={} → {}",
            self.surging_agent_shed_count,
            self.surging_viewer_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            self.hot_doc_op_cap_shed_count,
            self.hot_doc_read_fanout_shed_count,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

pub fn run_collab_surge(
    gate: &mut CollabSurgeGate,
    surging: &TenantId,
    quiet: &TenantId,
    hot_doc: &str,
    storm_agent_ops: u64,
    storm_viewer_reads: u64,
    _multiplier: u32,
) -> CollabSurgeReport {
    let mut held: Vec<(String, RunClass)> = Vec::new();
    for i in 0..storm_viewer_reads {
        let doc = format!("spread-doc-{}", i % 997);
        if gate
            .admit_doc_op(surging, &doc, RunClass::Speculative)
            .is_ok()
        {
            held.push((doc, RunClass::Speculative));
        }
    }
    for i in 0..storm_agent_ops {
        let doc = format!("spread-doc-{}", i % 997);
        if gate.admit_doc_op(surging, &doc, RunClass::Agent).is_ok() {
            held.push((doc, RunClass::Agent));
        }
    }
    let surging_human_admitted = gate
        .admit_doc_op(surging, "surging-fresh-doc", RunClass::Human)
        .is_ok();
    for (doc, class) in held.drain(..) {
        gate.release_op(surging, &doc, class);
    }

    let mut hot_held = 0u64;
    for _ in 0..storm_agent_ops {
        if gate.admit_doc_op(surging, hot_doc, RunClass::Agent).is_ok() {
            hot_held += 1;
        }
    }
    for _ in 0..hot_held {
        gate.release_op(surging, hot_doc, RunClass::Agent);
    }

    let mut fanout_held = 0u64;
    for _ in 0..storm_viewer_reads {
        if gate.admit_read_fanout(hot_doc).is_ok() {
            fanout_held += 1;
        }
    }
    for _ in 0..fanout_held {
        gate.release_read_fanout(hot_doc);
    }

    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate
        .admit_doc_op(quiet, "quiet-doc", RunClass::Human)
        .is_ok();

    CollabSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_viewer_shed_count: gate.shed_count(RunClass::Speculative),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        hot_doc_op_cap_shed_count: gate.doc_op_shed_count(hot_doc),
        hot_doc_read_fanout_shed_count: gate.read_fanout_shed_count(hot_doc),
    }
}

#[cfg(test)]
mod tests;
