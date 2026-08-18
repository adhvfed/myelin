use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventHandler, EventType,
    HandleOutcome, Reason, SubjectPattern, Visibility,
};

use crate::events;
use crate::refs_glue::{IssueLifecycleRel, IssueRelationGraph, TRAVERSE_MAX_DEPTH};
use crate::workflow::StateCategory;


#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafFact {
    pub estimate: Option<i64>,
    pub category: StateCategory,
}

impl LeafFact {
    pub fn new(estimate: Option<i64>, category: StateCategory) -> LeafFact {
        LeafFact { estimate, category }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RollupAggregate {
    pub total: u64,
    pub done: u64,
    pub estimate_sum: i64,
    pub input_hash: u64,
}

impl RollupAggregate {
    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.done as f64 / self.total as f64
        }
    }
}

pub fn recompute_incremental(leaves: &[LeafFact]) -> RollupAggregate {
    let mut total: u64 = 0;
    let mut done: u64 = 0;
    let mut estimate_sum: i64 = 0;
    let mut hash_acc: u64 = 0;
    for leaf in leaves {
        if leaf.category != StateCategory::Cancelled {
            total += 1;
            if leaf.category == StateCategory::Completed {
                done += 1;
            }
            estimate_sum = estimate_sum.saturating_add(leaf.estimate.unwrap_or(0));
        }
        hash_acc ^= leaf_hash(leaf);
    }
    RollupAggregate {
        total,
        done,
        estimate_sum,
        input_hash: hash_acc,
    }
}

fn leaf_hash(leaf: &LeafFact) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    leaf.estimate.hash(&mut h);
    leaf.category.wire_token().hash(&mut h);
    h.finish()
}

pub fn walk_parent_edges(graph: &IssueRelationGraph, child: &ArtifactRef) -> Vec<ArtifactRef> {
    graph
        .traverse(child, Some(IssueLifecycleRel::Parent))
        .into_iter()
        .filter(|n| n.depth <= TRAVERSE_MAX_DEPTH)
        .map(|n| n.node)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebounceWindow {
    pub width: u64,
}

impl DebounceWindow {
    pub const DEFAULT: DebounceWindow = DebounceWindow { width: 1 };
}

#[derive(Clone, Debug, Default)]
pub struct DebounceCoalescer {
    dirty: BTreeSet<String>,
}

impl DebounceCoalescer {
    pub fn new() -> DebounceCoalescer {
        DebounceCoalescer::default()
    }

    pub fn mark_dirty(&mut self, ancestor: &ArtifactRef) {
        self.dirty.insert(ancestor.0.clone());
    }

    pub fn recompute_count(&self) -> usize {
        self.dirty.len()
    }

    pub fn drain(&mut self) -> Vec<ArtifactRef> {
        let out: Vec<ArtifactRef> = self.dirty.iter().map(|s| ArtifactRef(s.clone())).collect();
        self.dirty.clear();
        out
    }
}

#[derive(Clone, Debug, Default)]
pub struct RollupStore {
    aggregates: HashMap<String, RollupAggregate>,
    leaves: HashMap<String, LeafFact>,
}

impl RollupStore {
    pub fn new() -> RollupStore {
        RollupStore::default()
    }

    pub fn put_leaf(&mut self, issue: &ArtifactRef, fact: LeafFact) {
        self.leaves.insert(issue.0.clone(), fact);
    }

    pub fn leaf(&self, issue: &ArtifactRef) -> Option<&LeafFact> {
        self.leaves.get(&issue.0)
    }

    pub fn aggregate(&self, ancestor: &ArtifactRef) -> Option<&RollupAggregate> {
        self.aggregates.get(&ancestor.0)
    }

    pub fn clear_aggregates(&mut self) {
        self.aggregates.clear();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecomputeOutcome {
    Recomputed(RollupAggregate),
    Suppressed,
}

impl RecomputeOutcome {
    pub fn is_suppressed(&self) -> bool {
        matches!(self, RecomputeOutcome::Suppressed)
    }

    pub fn aggregate(&self) -> Option<&RollupAggregate> {
        match self {
            RecomputeOutcome::Recomputed(a) => Some(a),
            RecomputeOutcome::Suppressed => None,
        }
    }
}

pub struct RollupConsumer {
    state: std::sync::Mutex<RollupState>,
}

#[derive(Default)]
struct RollupState {
    graph: IssueRelationGraph,
    store: RollupStore,
    coalescer: DebounceCoalescer,
    seen_events: BTreeSet<String>,
}

fn rollup_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            vec![
                SubjectPattern(events::ISSUE_UPDATED.to_string()),
                SubjectPattern(events::ISSUE_TRANSITIONED.to_string()),
                SubjectPattern(events::ISSUE_PARENT_CHANGED.to_string()),
            ]
        })
        .as_slice()
}

impl RollupConsumer {
    pub fn new() -> RollupConsumer {
        RollupConsumer {
            state: std::sync::Mutex::new(RollupState::default()),
        }
    }

    pub fn add_parent_edge(&self, child: &ArtifactRef, parent: &ArtifactRef) {
        let mut state = self.state.lock().expect("rollup state lock");
        state
            .graph
            .add_edge(child, parent, IssueLifecycleRel::Parent);
    }

    pub fn put_leaf(&self, issue: &ArtifactRef, fact: LeafFact) {
        let mut state = self.state.lock().expect("rollup state lock");
        state.store.put_leaf(issue, fact);
    }

    pub fn aggregate(&self, ancestor: &ArtifactRef) -> Option<RollupAggregate> {
        let state = self.state.lock().expect("rollup state lock");
        state.store.aggregate(ancestor).cloned()
    }

    pub fn pending_recompute_count(&self) -> usize {
        let state = self.state.lock().expect("rollup state lock");
        state.coalescer.recompute_count()
    }

    pub fn mark_changed(&self, child: &ArtifactRef) {
        let mut state = self.state.lock().expect("rollup state lock");
        let ancestors = walk_parent_edges(&state.graph, child);
        for ancestor in &ancestors {
            state.coalescer.mark_dirty(ancestor);
        }
    }

    pub fn recompute(&self, ancestor: &ArtifactRef) -> RecomputeOutcome {
        let mut state = self.state.lock().expect("rollup state lock");
        Self::recompute_locked(&mut state, ancestor)
    }

    fn recompute_locked(state: &mut RollupState, ancestor: &ArtifactRef) -> RecomputeOutcome {
        let leaves = Self::descendant_leaves(state, ancestor);
        let new = recompute_incremental(&leaves);
        if let Some(existing) = state.store.aggregates.get(&ancestor.0) {
            if existing.input_hash == new.input_hash {
                return RecomputeOutcome::Suppressed;
            }
        }
        state
            .store
            .aggregates
            .insert(ancestor.0.clone(), new.clone());
        RecomputeOutcome::Recomputed(new)
    }

    fn descendant_leaves(state: &RollupState, ancestor: &ArtifactRef) -> Vec<LeafFact> {
        let mut out = Vec::new();
        for (child_ref, fact) in &state.store.leaves {
            let child = ArtifactRef(child_ref.clone());
            let ancestors = walk_parent_edges(&state.graph, &child);
            if ancestors.iter().any(|a| a.0 == ancestor.0) {
                out.push(fact.clone());
            }
        }
        out
    }

    pub fn flush(&self) -> Vec<(ArtifactRef, RollupAggregate)> {
        let mut state = self.state.lock().expect("rollup state lock");
        let dirty = state.coalescer.drain();
        let mut out = Vec::new();
        for ancestor in dirty {
            if let RecomputeOutcome::Recomputed(agg) = Self::recompute_locked(&mut state, &ancestor)
            {
                out.push((ancestor, agg));
            }
        }
        out
    }

    pub fn reindex_from(&self) -> usize {
        let mut state = self.state.lock().expect("rollup state lock");
        state.store.clear_aggregates();
        let mut ancestors: BTreeSet<String> = BTreeSet::new();
        let leaf_refs: Vec<ArtifactRef> = state
            .store
            .leaves
            .keys()
            .map(|k| ArtifactRef(k.clone()))
            .collect();
        for leaf in &leaf_refs {
            for a in walk_parent_edges(&state.graph, leaf) {
                ancestors.insert(a.0);
            }
        }
        let count = ancestors.len();
        for a in ancestors {
            let _ = Self::recompute_locked(&mut state, &ArtifactRef(a));
        }
        count
    }
}

impl Default for RollupConsumer {
    fn default() -> RollupConsumer {
        RollupConsumer::new()
    }
}

impl EventHandler for RollupConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        rollup_subjects()
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        let mut state = self.state.lock().expect("rollup state lock");
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        let child = ev.subject.clone();
        if child.0.is_empty() {
            return HandleOutcome::NonRetryable(Reason(
                "rollup: event carries no subject ref - cannot locate the changed leaf".into(),
            ));
        }
        let ancestors = walk_parent_edges(&state.graph, &child);
        for ancestor in &ancestors {
            state.coalescer.mark_dirty(ancestor);
        }
        HandleOutcome::Done
    }
}

pub fn rollup_recomputed_draft(ancestor: &ArtifactRef, agg: &RollupAggregate) -> EventDraft {
    EventDraft {
        type_: EventType(events::ROLLUP_RECOMPUTED.into()),
        subject: ancestor.clone(),
        aggregate: AggregateKey(ancestor.0.clone()),
        payload: serde_json::json!({
            "ancestor": ancestor.0,
            "total": agg.total,
            "done": agg.done,
            "estimate_sum": agg.estimate_sum,
            "input_hash": agg.input_hash,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn aggregate_snapshot(consumer: &RollupConsumer) -> BTreeMap<String, RollupAggregate> {
    let state = consumer.state.lock().expect("rollup state lock");
    state
        .store
        .aggregates
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
#[path = "rollup/tests.rs"]
mod tests;
