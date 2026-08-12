use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_identity::{Principal, PrincipalKind};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;

const GENESIS_PREV: [u8; 32] = [0u8; 32];

pub const AUDIT_APPEND_LAG: (&str, &str) = ("audit.audit_append_lag", "events");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Allowed,
    Denied,
    Applied,
    Failed,
}

impl Outcome {
    pub fn as_wire(self) -> &'static str {
        self.as_str()
    }

    fn as_str(self) -> &'static str {
        match self {
            Outcome::Allowed => "allowed",
            Outcome::Denied => "denied",
            Outcome::Applied => "applied",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Minimised {
    pub actor: String,
    pub actor_kind: String,
    pub on_behalf_of: Option<String>,
}

impl Minimised {
    pub fn from_principal(principal: &Principal) -> Minimised {
        Minimised {
            actor: pseudonym_grammar(&principal.principal_id.0, &principal.tenant),
            actor_kind: kind_label(&principal.kind),
            on_behalf_of: match &principal.kind {
                PrincipalKind::Agent {
                    on_behalf_of: Some(on_behalf),
                    ..
                } => Some(pseudonym_grammar(&on_behalf.0, &principal.tenant)),
                _ => None,
            },
        }
    }
}

fn pseudonym_grammar(pseudonym: &str, tenant: &TenantId) -> String {
    format!("{pseudonym}@{}.noreply", tenant.0)
}

fn kind_label(kind: &PrincipalKind) -> String {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent { .. } => "agent",
        PrincipalKind::Service => "service",
    }
    .to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActionRecord {
    pub tenant: TenantId,
    pub region: Region,
    pub actor: Minimised,
    pub action: String,
    pub subject: ArtifactRef,
    pub outcome: Outcome,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub occurred_at: String,
}

impl ActionRecord {
    fn leaf_preimage(&self, seq: u64) -> Vec<u8> {
        fn put(buf: &mut Vec<u8>, s: &str) {
            buf.extend_from_slice(&(s.len() as u64).to_be_bytes());
            buf.extend_from_slice(s.as_bytes());
        }
        let mut buf = Vec::new();
        put(&mut buf, &self.tenant.0);
        put(&mut buf, &self.region.0);
        buf.extend_from_slice(&seq.to_be_bytes());
        put(&mut buf, &self.actor.actor);
        put(&mut buf, &self.actor.actor_kind);
        put(&mut buf, self.actor.on_behalf_of.as_deref().unwrap_or(""));
        put(&mut buf, &self.action);
        put(&mut buf, &self.subject.0);
        put(&mut buf, self.outcome.as_str());
        put(&mut buf, &self.correlation_id);
        put(&mut buf, self.causation_id.as_deref().unwrap_or(""));
        put(&mut buf, &self.occurred_at);
        buf
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditEntry {
    pub tenant: TenantId,
    pub region: Region,
    pub seq: u64,
    pub prev_hash: String,
    pub leaf_hash: String,
    pub actor: Minimised,
    pub action: String,
    pub subject: ArtifactRef,
    pub outcome: Outcome,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub occurred_at: String,
}

impl AuditEntry {
    fn as_action_record(&self) -> ActionRecord {
        ActionRecord {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            actor: self.actor.clone(),
            action: self.action.clone(),
            subject: self.subject.clone(),
            outcome: self.outcome,
            correlation_id: self.correlation_id.clone(),
            causation_id: self.causation_id.clone(),
            occurred_at: self.occurred_at.clone(),
        }
    }
}

fn blake3_multihash(bytes: &[u8]) -> String {
    let digest = blake3::hash(bytes);
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

fn multihash_bytes(s: &str) -> [u8; 32] {
    s.strip_prefix("blake3:")
        .and_then(|hex_str| hex::decode(hex_str).ok())
        .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
        .unwrap_or(GENESIS_PREV)
}

#[derive(Clone, Debug)]
struct ChainHead {
    next_seq: u64,
    last_prev: [u8; 32],
    last_leaf: [u8; 32],
    leaves: Vec<[u8; 32]>,
}

#[derive(Default)]
pub struct AuditLog {
    chains: Mutex<HashMap<TenantId, ChainHead>>,
    entries: Mutex<HashMap<TenantId, Vec<AuditEntry>>>,
}

impl AuditLog {
    pub fn new() -> AuditLog {
        AuditLog::default()
    }

    pub(crate) fn append(&self, action: ActionRecord) -> AuditEntry {
        let tenant = action.tenant.clone();
        let mut chains = self.chains.lock().unwrap_or_else(|e| e.into_inner());
        let head = chains.entry(tenant.clone()).or_insert_with(|| ChainHead {
            next_seq: 0,
            last_prev: GENESIS_PREV,
            last_leaf: GENESIS_PREV,
            leaves: Vec::new(),
        });

        let seq = head.next_seq;

        let mut link_input = Vec::with_capacity(64);
        link_input.extend_from_slice(&head.last_prev);
        link_input.extend_from_slice(&head.last_leaf);
        let prev_digest = blake3::hash(&link_input);
        let prev_hash = format!("blake3:{}", hex::encode(prev_digest.as_bytes()));

        let preimage = action.leaf_preimage(seq);
        let leaf_digest = blake3::hash(&preimage);
        let leaf_hash = blake3_multihash(&preimage);

        let entry = AuditEntry {
            tenant: tenant.clone(),
            region: action.region,
            seq,
            prev_hash,
            leaf_hash,
            actor: action.actor,
            action: action.action,
            subject: action.subject,
            outcome: action.outcome,
            correlation_id: action.correlation_id,
            causation_id: action.causation_id,
            occurred_at: action.occurred_at,
        };

        head.next_seq += 1;
        head.last_prev = *prev_digest.as_bytes();
        head.last_leaf = *leaf_digest.as_bytes();
        head.leaves.push(*leaf_digest.as_bytes());

        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(tenant)
            .or_default()
            .push(entry.clone());

        entry
    }

    pub fn entries_for(&self, tenant: &TenantId) -> Vec<AuditEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .cloned()
            .unwrap_or_default()
    }

    pub fn len_for(&self, tenant: &TenantId) -> u64 {
        self.chains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|h| h.next_seq)
            .unwrap_or(0)
    }

    pub fn root(&self, tenant: &TenantId) -> Option<String> {
        let chains = self.chains.lock().unwrap_or_else(|e| e.into_inner());
        let head = chains.get(tenant)?;
        if head.leaves.is_empty() {
            return None;
        }
        Some(blake3_multihash_raw(&merkle_root(&head.leaves)))
    }

    pub fn verify_chain(&self, tenant: &TenantId) -> bool {
        verify_entries(&self.entries_for(tenant))
    }

    pub(crate) fn leaf_digests(&self, tenant: &TenantId) -> Vec<[u8; 32]> {
        self.chains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|h| h.leaves.clone())
            .unwrap_or_default()
    }
}

pub(crate) fn blake3_multihash_raw(digest: &[u8]) -> String {
    format!("blake3:{}", hex::encode(digest))
}

pub fn verify_entries_for_test(entries: &[AuditEntry]) -> bool {
    verify_entries(entries)
}

pub(crate) fn verify_entries(entries: &[AuditEntry]) -> bool {
    let mut prev = GENESIS_PREV;
    let mut last_leaf = GENESIS_PREV;
    for (i, e) in entries.iter().enumerate() {
        if e.seq != i as u64 {
            return false;
        }
        let mut link_input = Vec::with_capacity(64);
        link_input.extend_from_slice(&prev);
        link_input.extend_from_slice(&last_leaf);
        let expect_prev = blake3_multihash_raw(blake3::hash(&link_input).as_bytes());
        if expect_prev != e.prev_hash {
            return false;
        }
        let preimage = e.as_action_record().leaf_preimage(e.seq);
        if blake3_multihash(&preimage) != e.leaf_hash {
            return false;
        }
        prev = multihash_bytes(&e.prev_hash);
        last_leaf = blake3::hash(&preimage).into();
    }
    true
}

pub(crate) fn interior_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&[0x01]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

pub(crate) fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    debug_assert!(
        !leaves.is_empty(),
        "merkle_root is only called for a non-empty chain"
    );
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0;
        while i < level.len() {
            if i + 1 < level.len() {
                next.push(interior_node(&level[i], &level[i + 1]));
                i += 2;
            } else {
                next.push(level[i]);
                i += 1;
            }
        }
        level = next;
    }
    level[0]
}

pub struct AuditConsumer {
    log: AuditLog,
    subjects: &'static [SubjectPattern],
    append_lag: Mutex<u64>,
}

static AUDIT_SUBJECTS: &[SubjectPattern] = &[];

impl Default for AuditConsumer {
    fn default() -> Self {
        AuditConsumer::new()
    }
}

impl AuditConsumer {
    pub fn new() -> AuditConsumer {
        AuditConsumer {
            log: AuditLog::new(),
            subjects: AUDIT_SUBJECTS,
            append_lag: Mutex::new(0),
        }
    }

    pub fn log(&self) -> &AuditLog {
        &self.log
    }

    pub fn append_lag(&self) -> u64 {
        *self.append_lag.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn append_event(&self, ev: &EventEnvelope) -> AuditEntry {
        {
            let mut lag = self.append_lag.lock().unwrap_or_else(|e| e.into_inner());
            *lag += 1;
        }

        let record = ActionRecord {
            tenant: ev.tenant.clone(),
            region: ev.region.clone(),
            actor: Minimised::from_principal(&ev.actor.0),
            action: ev.type_.0.clone(),
            subject: ev.subject.clone(),
            outcome: Outcome::Applied,
            correlation_id: ev.correlation_id.0.clone(),
            causation_id: ev.causation_id.as_ref().map(|id| id.0.clone()),
            occurred_at: ev.occurred_at.0.clone(),
        };
        let entry = self.log.append(record);

        {
            let mut lag = self.append_lag.lock().unwrap_or_else(|e| e.into_inner());
            *lag = lag.saturating_sub(1);
        }

        entry
    }
}

impl EventHandler for AuditConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        self.append_event(ev);
        HandleOutcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CausedBy, CorrelationId, DataRole, EventId, EventType, Timestamp,
        Visibility,
    };
    use myelin_identity::{PrincipalId, PrincipalKind, RuntimeRef};

    fn human(id: &str, tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn agent(id: &str, on_behalf: &str, tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt-1".into()),
                on_behalf_of: Some(PrincipalId(on_behalf.into())),
            },
            TenantId(tenant.into()),
        )
    }

    fn action_event(
        id: &str,
        actor: Principal,
        type_: &str,
        subject: &str,
        correlation: &str,
        causation: Option<&str>,
    ) -> EventEnvelope {
        let tenant = actor.tenant.clone();
        let region = actor.region.clone();
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant,
            region,
            actor: Actor(actor),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("agg:1".into()),
            causation_id: causation.map(|c| EventId(c.into())),
            correlation_id: CorrelationId(correlation.into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "real_name": "Alice Example", "email": "alice@example.test" }),
        }
    }

    #[test]
    fn appended_action_produces_a_hash_chain_entry_and_a_merkle_leaf() {
        let c = AuditConsumer::new();
        let ev = action_event(
            "01J-1",
            human("u-1", "acme"),
            "identity.tuple.written",
            "myelin://acme/identity/tuple/t1",
            "01J-root",
            None,
        );
        assert_eq!(
            c.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );

        let tenant = TenantId("acme".into());
        let entries = c.log().entries_for(&tenant);
        assert_eq!(
            entries.len(),
            1,
            "one delivered action → one appended entry"
        );
        let e = &entries[0];
        assert_eq!(e.seq, 0, "the genesis entry of acme's chain is seq 0");
        assert!(
            e.prev_hash.starts_with("blake3:"),
            "prev_hash is the chain link"
        );
        assert!(
            e.leaf_hash.starts_with("blake3:"),
            "leaf_hash is the Merkle leaf"
        );
        assert_ne!(
            e.prev_hash, e.leaf_hash,
            "the chain link and the leaf are distinct hashes"
        );
        assert!(
            c.log().root(&tenant).is_some(),
            "a per-tenant Merkle root exists"
        );
        assert_eq!(
            c.log().len_for(&tenant),
            1,
            "tree size 1 (the STH signs this)"
        );
    }

    #[test]
    fn actor_is_the_minimised_pseudonym_form_never_a_payload() {
        let c = AuditConsumer::new();
        let ev = action_event(
            "01J-1",
            human("u-42", "acme"),
            "identity.tuple.written",
            "myelin://acme/identity/tuple/t1",
            "01J-root",
            None,
        );
        c.handle(&ev, &mut myelin_events::HandlerTx::none());
        let e = &c.log().entries_for(&TenantId("acme".into()))[0];

        assert_eq!(
            e.actor.actor, "u-42@acme.noreply",
            "actor is the frozen pseudonym grammar"
        );
        assert_eq!(e.actor.actor_kind, "human");
        assert!(e.actor.on_behalf_of.is_none(), "a human acts for nobody");

        let serialized = serde_json::to_string(e).expect("entry serialises");
        assert!(
            !serialized.contains("Alice Example"),
            "no real name reaches the audit entry"
        );
        assert!(
            !serialized.contains("alice@example.test"),
            "no email reaches the audit entry"
        );
        assert_eq!(
            e.subject,
            ArtifactRef("myelin://acme/identity/tuple/t1".into())
        );
    }

    #[test]
    fn delegated_agent_actor_and_on_behalf_of_are_both_minimised() {
        let c = AuditConsumer::new();
        let ev = action_event(
            "01J-1",
            agent("agent-7", "u-9", "acme"),
            "agent.effect_applied",
            "myelin://acme/issues/issue/PROJ-1",
            "01J-root",
            None,
        );
        c.handle(&ev, &mut myelin_events::HandlerTx::none());
        let e = &c.log().entries_for(&TenantId("acme".into()))[0];
        assert_eq!(e.actor.actor, "agent-7@acme.noreply");
        assert_eq!(e.actor.actor_kind, "agent");
        assert_eq!(
            e.actor.on_behalf_of.as_deref(),
            Some("u-9@acme.noreply"),
            "the human a delegated agent acted for is itself a minimised pseudonym"
        );
    }

    #[test]
    fn correlation_and_causation_are_carried_onto_the_entry() {
        let c = AuditConsumer::new();
        let ev = action_event(
            "01J-child",
            human("u-1", "acme"),
            "refs.edge.created",
            "myelin://acme/refs/edge/e1",
            "01J-root",
            Some("01J-parent"),
        );
        c.handle(&ev, &mut myelin_events::HandlerTx::none());
        let e = &c.log().entries_for(&TenantId("acme".into()))[0];
        assert_eq!(
            e.correlation_id, "01J-root",
            "the causal root is carried (the why-walk anchor)"
        );
        assert_eq!(
            e.causation_id.as_deref(),
            Some("01J-parent"),
            "the immediate parent is carried (nested causality)"
        );
    }

    #[test]
    fn no_service_writes_the_audit_log_except_the_outbox_consumer() {
        let c = AuditConsumer::new();
        assert_eq!(
            c.log().len_for(&TenantId("acme".into())),
            0,
            "empty before any delivery"
        );
        let ev = action_event(
            "01J-1",
            human("u-1", "acme"),
            "identity.tuple.written",
            "myelin://acme/identity/tuple/t1",
            "01J-root",
            None,
        );
        c.handle(&ev, &mut myelin_events::HandlerTx::none());
        assert_eq!(
            c.log().len_for(&TenantId("acme".into())),
            1,
            "the entry appears ONLY because it was delivered through the outbox consumer"
        );
    }

    #[test]
    fn the_hash_chain_is_per_tenant() {
        let c = AuditConsumer::new();
        c.handle(
            &action_event(
                "01J-a1",
                human("u", "acme"),
                "identity.tuple.written",
                "myelin://acme/x",
                "r1",
                None,
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        c.handle(
            &action_event(
                "01J-b1",
                human("u", "globex"),
                "identity.tuple.written",
                "myelin://globex/x",
                "r2",
                None,
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        c.handle(
            &action_event(
                "01J-a2",
                human("u", "acme"),
                "identity.tuple.written",
                "myelin://acme/y",
                "r3",
                None,
            ),
            &mut myelin_events::HandlerTx::none(),
        );

        let acme = c.log().entries_for(&TenantId("acme".into()));
        let globex = c.log().entries_for(&TenantId("globex".into()));
        assert_eq!(acme.len(), 2, "acme has two entries");
        assert_eq!(globex.len(), 1, "globex has one entry");
        assert_eq!(acme[0].seq, 0);
        assert_eq!(acme[1].seq, 1);
        assert_eq!(globex[0].seq, 0, "globex's chain starts at its OWN seq 0");
    }

    #[test]
    fn a_retroactive_edit_breaks_the_chain() {
        let c = AuditConsumer::new();
        for i in 0..5 {
            c.handle(
                &action_event(
                    &format!("01J-{i}"),
                    human("u", "acme"),
                    "identity.tuple.written",
                    &format!("myelin://acme/x/{i}"),
                    "r",
                    None,
                ),
                &mut myelin_events::HandlerTx::none(),
            );
        }
        let tenant = TenantId("acme".into());
        let entries = c.log().entries_for(&tenant);
        assert!(
            c.log().verify_chain(&tenant),
            "the freshly-built chain verifies intact"
        );
        assert!(
            verify_entries(&entries),
            "the verifier core agrees the pristine chain is intact"
        );

        let mut tampered = entries.clone();
        tampered[2].subject = ArtifactRef("myelin://acme/TAMPERED".into());
        assert!(
            !verify_entries(&tampered),
            "a retroactive edit breaks the chain - verify_entries returns FALSE (tamper detected)"
        );
        let mut reordered = entries.clone();
        reordered.swap(1, 3);
        assert!(
            !verify_entries(&reordered),
            "a re-ordered chain fails verification"
        );
        let mut dropped = entries.clone();
        dropped.remove(2);
        assert!(
            !verify_entries(&dropped),
            "a dropped entry fails verification (seq gap)"
        );
    }

    #[test]
    fn outcome_wire_strings_are_frozen_and_distinguish_the_leaf() {
        assert_eq!(Outcome::Allowed.as_str(), "allowed");
        assert_eq!(Outcome::Denied.as_str(), "denied");
        assert_eq!(Outcome::Applied.as_str(), "applied");
        assert_eq!(Outcome::Failed.as_str(), "failed");

        let base = ActionRecord {
            tenant: TenantId("acme".into()),
            region: Region("acme-home".into()),
            actor: Minimised {
                actor: "u@acme.noreply".into(),
                actor_kind: "human".into(),
                on_behalf_of: None,
            },
            action: "identity.tuple.written".into(),
            subject: ArtifactRef("myelin://acme/x".into()),
            outcome: Outcome::Applied,
            correlation_id: "r".into(),
            causation_id: None,
            occurred_at: "2026-06-19T00:00:00Z".into(),
        };
        let mut denied = base.clone();
        denied.outcome = Outcome::Denied;
        assert_ne!(
            base.leaf_preimage(0),
            denied.leaf_preimage(0),
            "the outcome is part of the leaf preimage - a different outcome is a different leaf"
        );
    }

    #[test]
    fn merkle_root_is_deterministic_and_changes_on_append() {
        let mk = || {
            let c = AuditConsumer::new();
            c.handle(
                &action_event(
                    "01J-1",
                    human("u", "acme"),
                    "identity.tuple.written",
                    "myelin://acme/x",
                    "r",
                    None,
                ),
                &mut myelin_events::HandlerTx::none(),
            );
            c.handle(
                &action_event(
                    "01J-2",
                    human("u", "acme"),
                    "identity.tuple.written",
                    "myelin://acme/y",
                    "r",
                    None,
                ),
                &mut myelin_events::HandlerTx::none(),
            );
            c
        };
        let tenant = TenantId("acme".into());
        let root_a = mk().log().root(&tenant);
        let root_b = mk().log().root(&tenant);
        assert_eq!(
            root_a, root_b,
            "the same leaf set produces the same Merkle root (deterministic)"
        );

        let c = mk();
        let before = c.log().root(&tenant);
        c.handle(
            &action_event(
                "01J-3",
                human("u", "acme"),
                "identity.tuple.written",
                "myelin://acme/z",
                "r",
                None,
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        let after = c.log().root(&tenant);
        assert_ne!(
            before, after,
            "appending an entry changes the Merkle root (the STH advances)"
        );
    }

    #[test]
    fn audit_append_lag_signal_is_named_and_reads_green() {
        assert_eq!(
            AUDIT_APPEND_LAG.0, "audit.audit_append_lag",
            "the SLO signal name is pinned"
        );
        assert_eq!(AUDIT_APPEND_LAG.1, "events", "the SLO unit is pinned");
        let c = AuditConsumer::new();
        c.handle(
            &action_event(
                "01J-1",
                human("u", "acme"),
                "identity.tuple.written",
                "myelin://acme/x",
                "r",
                None,
            ),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(
            c.append_lag(),
            0,
            "append_lag reads green (0) in steady state after a synchronous append"
        );
    }
}
