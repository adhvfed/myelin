pub mod trace;
pub use trace::{
    write_agent_trace, AgentTrace, AgentTraceHolder, TraceEraseReceipt, AUDIT_LOG_ERASABLE,
    AUDIT_LOG_STORE_ID, TRACE_ERASABLE, TRACE_HOLDER_ID,
};

use myelin_content::rebac_fragment::object_types as kn_objects;
use myelin_content::rebac_fragment::{COMMENT, DRAFT, EDIT, PUBLISH};

use crate::transport::{DocOp, OpId, OpKind};

pub const KNOWLEDGE_SUBSYSTEM: &str = "knowledge";

pub const PUBLISH_TOOL: &str = "publish";

pub const EDIT_CONFIDENTIAL_TOOL: &str = "edit_confidential";

pub const DRAFT_TOOL: &str = "draft";

pub const COMMENT_TOOL: &str = "comment";

pub const APPEND_TOOL: &str = "append";

pub const ALL_TOOLS: [&str; 5] = [
    PUBLISH_TOOL,
    EDIT_CONFIDENTIAL_TOOL,
    DRAFT_TOOL,
    COMMENT_TOOL,
    APPEND_TOOL,
];

pub fn publish_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, PUBLISH)]
}

pub fn edit_confidential_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, EDIT)]
}

pub fn draft_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, DRAFT)]
}

pub fn comment_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, COMMENT)]
}

pub fn append_required_caps() -> Vec<String> {
    vec![format!("{}.{}", kn_objects::PAGE, EDIT)]
}

pub fn required_caps_for(tool: &str) -> Vec<String> {
    match tool {
        PUBLISH_TOOL => publish_required_caps(),
        EDIT_CONFIDENTIAL_TOOL => edit_confidential_required_caps(),
        DRAFT_TOOL => draft_required_caps(),
        COMMENT_TOOL => comment_required_caps(),
        APPEND_TOOL => append_required_caps(),
        _ => Vec::new(),
    }
}

pub fn requires_approval_default(tool: &str) -> bool {
    match tool {
        PUBLISH_TOOL | EDIT_CONFIDENTIAL_TOOL => true,
        DRAFT_TOOL | COMMENT_TOOL | APPEND_TOOL => false,
        _ => true,
    }
}

pub fn is_consequential(tool: &str) -> bool {
    requires_approval_default(tool)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEditAttribution {
    pub agent_pseudonym: String,
    pub run_id: String,
    pub rationale: String,
}

impl AgentEditAttribution {
    pub fn new(
        agent_pseudonym: impl Into<String>,
        run_id: impl Into<String>,
        rationale: impl Into<String>,
    ) -> AgentEditAttribution {
        AgentEditAttribution {
            agent_pseudonym: agent_pseudonym.into(),
            run_id: run_id.into(),
            rationale: rationale.into(),
        }
    }

    pub fn actor(&self) -> String {
        format!("agent:{}", self.agent_pseudonym)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditAuthor {
    Human {
        author_pseudonym: String,
    },
    Agent(AgentEditAttribution),
}

impl EditAuthor {
    pub fn is_agent(&self) -> bool {
        matches!(self, EditAuthor::Agent(_))
    }

    pub fn agent_provenance(&self) -> Option<&AgentEditAttribution> {
        match self {
            EditAuthor::Agent(a) => Some(a),
            EditAuthor::Human { .. } => None,
        }
    }

    pub fn actor(&self) -> String {
        match self {
            EditAuthor::Human { author_pseudonym } => author_pseudonym.clone(),
            EditAuthor::Agent(a) => a.actor(),
        }
    }

    pub fn stamp_op(&self, op_id: OpId, kind: OpKind, payload: impl Into<Vec<u8>>) -> DocOp {
        DocOp::cas(op_id, self.actor(), kind, payload)
    }
}

pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(total_effects >= 1, "a card has at least one effect");
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        card_id.to_string()
    } else {
        format!("{card_id}:{effect_idx}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectRefusal {
    Withheld { card_id: String },
    Denied(String),
}

impl core::fmt::Display for EffectRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EffectRefusal::Withheld { card_id } => write!(
                f,
                "knowledge agent effect WITHHELD pending HITL approval (card {card_id}) - \
                 the consequential edit does NOT mutate until a human approves (AG-8)"
            ),
            EffectRefusal::Denied(reason) => write!(
                f,
                "knowledge agent effect DENIED (ordinary tool error, no privileged fallback): {reason}"
            ),
        }
    }
}

impl std::error::Error for EffectRefusal {}

#[derive(Clone, Debug)]
pub struct KnowledgeEffectGate {
    applied_keys: std::collections::BTreeSet<String>,
}

impl Default for KnowledgeEffectGate {
    fn default() -> KnowledgeEffectGate {
        KnowledgeEffectGate::new()
    }
}

impl KnowledgeEffectGate {
    pub fn new() -> KnowledgeEffectGate {
        KnowledgeEffectGate {
            applied_keys: std::collections::BTreeSet::new(),
        }
    }

    pub fn decide(
        &self,
        tool: &str,
        approved: &std::collections::BTreeSet<String>,
        card_id: &str,
    ) -> Result<(), EffectRefusal> {
        if requires_approval_default(tool) && !approved.contains(tool) {
            return Err(EffectRefusal::Withheld {
                card_id: card_id.to_string(),
            });
        }
        Ok(())
    }

    pub fn apply_once(&mut self, idem_key: &str) -> bool {
        self.applied_keys.insert(idem_key.to_string())
    }

    pub fn has_applied(&self, idem_key: &str) -> bool {
        self.applied_keys.contains(idem_key)
    }

    pub fn applied_count(&self) -> usize {
        self.applied_keys.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettle {
    remaining: u64,
    settled: u64,
}

impl ReserveSettle {
    pub fn reserve(amount: u64) -> ReserveSettle {
        ReserveSettle {
            remaining: amount,
            settled: 0,
        }
    }

    pub fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }

    pub fn settle(&mut self, cost: u64) -> u64 {
        self.remaining = self.remaining.saturating_sub(cost);
        self.settled = self.settled.saturating_add(cost);
        cost
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    pub fn settled(&self) -> u64 {
        self.settled
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnD11Receipt {
    pub withheld: u64,
    pub denied: u64,
    pub applied: u64,
    pub mutations_before_approval: u64,
    pub double_applies: u64,
    pub ungoverned_mutations: u64,
    pub settled_minor_units: u64,
    pub at_ms: u64,
}

impl KnD11Receipt {
    pub fn is_green(&self) -> bool {
        self.ungoverned_mutations == 0
            && self.mutations_before_approval == 0
            && self.double_applies == 0
            && self.applied >= 1
    }
}

pub struct KnowledgeAgentRun {
    attribution: AgentEditAttribution,
    gate: KnowledgeEffectGate,
    budget: ReserveSettle,
    approved: std::collections::BTreeSet<String>,
    applied_ops: Vec<DocOp>,
    withheld: u64,
    denied: u64,
    mutations_before_approval: u64,
    double_applies: u64,
    ungoverned_mutations: u64,
    next_lamport: u64,
}

impl KnowledgeAgentRun {
    pub fn begin(attribution: AgentEditAttribution, reserve: u64) -> KnowledgeAgentRun {
        KnowledgeAgentRun {
            attribution,
            gate: KnowledgeEffectGate::new(),
            budget: ReserveSettle::reserve(reserve),
            approved: std::collections::BTreeSet::new(),
            applied_ops: Vec::new(),
            withheld: 0,
            denied: 0,
            mutations_before_approval: 0,
            double_applies: 0,
            ungoverned_mutations: 0,
            next_lamport: 0,
        }
    }

    pub fn approve(&mut self, tool: &str) {
        self.approved.insert(tool.to_string());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose(
        &mut self,
        tool: &str,
        kind: OpKind,
        payload: impl Into<Vec<u8>>,
        cost: u64,
        card_id: &str,
        effect_idx: usize,
        total_effects: usize,
    ) -> Result<Option<DocOp>, EffectRefusal> {
        if !self.budget.has_remaining(cost) {
            self.denied = self.denied.saturating_add(1);
            return Err(EffectRefusal::Denied(format!(
                "reserve exhausted - no remaining balance for cost {cost} minor-units (11.7)"
            )));
        }

        if let Err(refusal) = self.gate.decide(tool, &self.approved, card_id) {
            self.withheld = self.withheld.saturating_add(1);
            return Err(refusal);
        }

        let idem_key = per_effect_idem_key(card_id, effect_idx, total_effects);
        if self.gate.has_applied(&idem_key) {
            return Ok(None);
        }

        let applied_fresh = self.gate.apply_once(&idem_key);
        if !applied_fresh {
            self.double_applies = self.double_applies.saturating_add(1);
            return Ok(None);
        }
        let author = EditAuthor::Agent(self.attribution.clone());
        let op_id = OpId::new(self.attribution.run_id.clone(), self.next_lamport);
        self.next_lamport += 1;
        let op = author.stamp_op(op_id, kind, payload);
        self.applied_ops.push(op.clone());

        self.budget.settle(cost);
        Ok(Some(op))
    }

    pub fn applied_ops(&self) -> &[DocOp] {
        &self.applied_ops
    }

    pub fn budget(&self) -> &ReserveSettle {
        &self.budget
    }

    pub fn seal(&self, at_ms: u64) -> KnD11Receipt {
        let agent_actor = self.attribution.actor();
        let ungoverned = self
            .applied_ops
            .iter()
            .filter(|op| op.actor != agent_actor)
            .count() as u64;
        KnD11Receipt {
            withheld: self.withheld,
            denied: self.denied,
            applied: self.gate.applied_count() as u64,
            mutations_before_approval: self.mutations_before_approval,
            double_applies: self.double_applies,
            ungoverned_mutations: self.ungoverned_mutations.saturating_add(ungoverned),
            settled_minor_units: self.budget.settled(),
            at_ms,
        }
    }
}

#[cfg(test)]
mod tests;
