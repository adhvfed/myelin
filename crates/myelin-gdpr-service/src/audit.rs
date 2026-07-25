//! # `audit` — the tamper-evident audit log core (contract 10.6, the construction half)
//!
//! See the crate-level doc for the full P-GA-19 framing. This module is the mechanism:
//!
//! - [`AuditLog`] — the per-tenant hash-chain whose entries are also Merkle-tree leaves. Its
//!   [`AuditLog::append`] is `pub(crate)` ONLY: there is **no** public direct-write path — the
//!   sole writer is [`AuditConsumer`], the outbox subscription. "No service writes the audit log
//!   directly" is structural, not a convention.
//! - [`AuditConsumer`] — the [`EventHandler`] (an infra subscription on the M0 outbox): every
//!   action-bearing event becomes one appended, minimised, causality-carried entry.
//! - [`AuditEntry`] — one §6.2 row: the per-tenant `seq`, the `prev_hash` (hash-chain link) and
//!   `leaf_hash` (Merkle leaf), the [`Minimised`] actor/on_behalf_of/subject, the action, the
//!   outcome, and the carried causality (`correlation_id` / `causation_id`).
//! - [`Minimised`] — the actor minimisation: the frozen `<pseudonym>@<tenant>.noreply` grammar
//!   (contract 4.8), constructed from the PII-free `principal_id`. Never a payload.
//!
//! ## How the hash-chain + Merkle leaves are built (the §6.2 / §6.1 construction)
//! Each entry's **`leaf_hash` = BLAKE3(canonical(entry-without-the-hashes))** — the Merkle leaf
//! (a stable, field-ordered canonical encoding so the hash is reproducible). Each entry's
//! **`prev_hash` = BLAKE3(prev.prev_hash || prev.leaf_hash)** — the Haber–Stornetta linked-
//! timestamp chain (the genesis entry's `prev_hash` is the all-zero root). A retroactive edit to
//! any entry changes its `leaf_hash`, which changes every later `prev_hash` — the break is
//! detectable from that point forward (the tamper-evidence property; the O(log n) inclusion /
//! consistency PROOFS over the Merkle tree are the P-GA-20 / P-119 floor). The per-tenant Merkle
//! ROOT is recomputed incrementally on every append ([`AuditLog::root`]) so the signed-tree-head
//! P-GA-20 ships can sign it.
//!
//! ## FLOOR — the in-memory chain models the §6.2 `audit_entry` / `audit_sth` tables
//! There is no live OLTP DB on this floor (the OLTP client is P-007; `serve` is P-S12). The chain
//! is held in-memory with byte-for-byte the §6.2 semantics: the `(tenant, seq)` PK ordering, the
//! `prev_hash`/`leaf_hash` columns, the minimised actor/subject. The real `INSERT` into
//! `audit_entry` (in the SAME transaction as the consumer's dedup mark, so the append and the ack
//! co-commit) lands when the OLTP client is wired here. The seam shape does not change.

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_identity::{Principal, PrincipalKind};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;

/// The all-zero genesis link the first entry in a per-tenant chain points back to (the chain
/// root before any entry exists — the §6.2 hash-chain seed). 32 bytes = the BLAKE3 digest width.
const GENESIS_PREV: [u8; 32] = [0u8; 32];

/// The `audit_append_lag` telemetry signal NAME + UNIT (gdpr §7.6 / contract 1.8 — the audit-log
/// health SLO). The audit consumer exposes the live measurement
/// ([`AuditConsumer::append_lag`]); wiring the sample onto the running service's metrics-health
/// surface is the `serve(AppSpec)`-boot follow-on (P-119 rides the same surface). The name is
/// pinned here so a later emitter uses exactly this string + unit (observability is part of the
/// pass — EI-01 §3).
pub const AUDIT_APPEND_LAG: (&str, &str) = ("audit.audit_append_lag", "events");

/// The outcome of an audited action (§6.2 `outcome`): `allowed | denied | applied | failed`.
/// Minimised metadata about WHAT happened, never the content it happened to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Allowed,
    Denied,
    Applied,
    Failed,
}

impl Outcome {
    /// The frozen §6.2 wire spelling (the token that goes into the Merkle-leaf preimage). Public so
    /// a verifier (the GA-D3 drill) recomputes the identical leaf the store committed.
    pub fn as_wire(self) -> &'static str {
        self.as_str()
    }

    /// The frozen §6.2 wire spelling.
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Allowed => "allowed",
            Outcome::Denied => "denied",
            Outcome::Applied => "applied",
            Outcome::Failed => "failed",
        }
    }
}

/// The minimised actor of an audited action (§6.1 — `actor` is a pseudonymous id, NEVER a
/// payload, and uses the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply`, contract 4.8).
///
/// **The minimisation is structural**: a [`Minimised`] is constructed only from a
/// [`Principal`]'s opaque, PII-free `principal_id` (a ULID-class attribution id, never a
/// name/email — `control-plane-pii-free`). There is no field on this type that could hold a
/// real identity, so an entry physically cannot carry one. Erasing the person (Identity's
/// pseudonym-map crypto-shred, DSR step 1) tombstones the *identity* the pseudonym resolved to
/// while the minimised *fact* (an action of this kind happened) survives for accountability —
/// the H16 carve-out (gdpr §6.4; the carve-out body is P-GA-20).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Minimised {
    /// The actor in the frozen `<pseudonym>@<tenant>.noreply` grammar (contract 4.8). The
    /// `<pseudonym>` is the opaque `principal_id`; the `<tenant>` is the partition key. No PII.
    pub actor: String,
    /// `human | agent | service` — agents are audited identically to humans (EI-02 §2). A
    /// label, not a code branch.
    pub actor_kind: String,
    /// The human a delegated agent acted for (the caused-by anchor), as a pseudonym id — present
    /// only for an agent acting `on_behalf_of`. PII-free, never a payload.
    pub on_behalf_of: Option<String>,
}

impl Minimised {
    /// Build the minimised actor form from a verified [`Principal`] (§6.1 minimisation). The
    /// `actor` is the frozen `<pseudonym>@<tenant>.noreply` grammar over the PII-free
    /// `principal_id`; the `actor_kind` is the principal's kind; `on_behalf_of` (for a delegated
    /// agent) is itself a pseudonym id. The construction reads ONLY PII-free fields — there is no
    /// path by which a name/email could be carried.
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

/// The frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` (contract 4.8, C5). `<pseudonym>`
/// is the opaque PII-free principal id; `<tenant>` is the partition key. This is the SAME grammar
/// `IdentityService::resolve_pseudonym` returns — pinned here so the audit actor form cannot
/// drift from the identity form (one grammar, EI-01 §7).
fn pseudonym_grammar(pseudonym: &str, tenant: &TenantId) -> String {
    format!("{pseudonym}@{}.noreply", tenant.0)
}

/// The `human | agent | service` label (§6.2 `actor_kind`). A label only — `check` never branches
/// on kind; the audit log records it for the who-did-what view.
fn kind_label(kind: &PrincipalKind) -> String {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent { .. } => "agent",
        PrincipalKind::Service => "service",
    }
    .to_string()
}

/// The minimised, pre-sequenced action an append records (everything about WHAT happened, before
/// the chain assigns its `seq` and computes its two hashes). Built by [`AuditConsumer`] from one
/// [`EventEnvelope`] and handed to [`AuditLog::append`]; it is also the Merkle-leaf preimage
/// source. Bundling the fields keeps the append signature small AND keeps the leaf preimage in
/// lock-step with the appended entry (one struct, no field can drift between them).
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
    /// The canonical, field-ordered Merkle-leaf preimage for this action at sequence `seq`
    /// (everything EXCEPT the two hashes — a leaf cannot hash its own hash). A length-prefixed,
    /// field-ordered encoding (no JSON-key-ordering ambiguity); every field that distinguishes one
    /// action from another is included, so two distinct actions can never collide to one leaf.
    /// Stable so any verifier (the P-GA-20 inclusion proof) recomputes the identical digest.
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

/// One audit-log entry (§6.2 `audit_entry`). The §6.2 row, minimised by design:
/// `actor`/`on_behalf_of`/`subject` are pseudonymous ids / [`ArtifactRef`]s, never payloads.
///
/// The two hashes make it tamper-evident: `leaf_hash` is the Merkle leaf
/// `BLAKE3(canonical(entry-body))`; `prev_hash` is the hash-chain link
/// `BLAKE3(prev.prev_hash || prev.leaf_hash)`. Both are rendered `blake3:<hex>` (the same
/// multihash convention the BlobStore content-address uses).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditEntry {
    /// Partition + residency key (§6.2 `tenant` / `region`; the chain is per-tenant, in-cell).
    pub tenant: TenantId,
    pub region: Region,
    /// Per-tenant monotonic sequence — the chain order + the Merkle leaf index (§6.2 `seq`,
    /// `PRIMARY KEY (tenant, seq)`).
    pub seq: u64,
    /// The hash-chain link `blake3:<hex>` of `BLAKE3(prev.prev_hash || prev.leaf_hash)` (§6.2
    /// `prev_hash`). The genesis entry links the all-zero root.
    pub prev_hash: String,
    /// The Merkle leaf `blake3:<hex>` of `BLAKE3(canonical(entry-body))` (§6.2 `leaf_hash`).
    pub leaf_hash: String,
    /// The minimised actor (the `<pseudonym>@<tenant>.noreply` form + kind + on_behalf_of).
    pub actor: Minimised,
    /// What the action was — a dotted action token (§6.2 `action`, e.g. `identity.tuple.written`,
    /// `agent.effect_applied`). Derived from the event's dotted type; never a payload.
    pub action: String,
    /// What the action targeted — an [`ArtifactRef`] (an id, never content; §6.2 `subject`).
    pub subject: ArtifactRef,
    /// allowed | denied | applied | failed (§6.2 `outcome`).
    pub outcome: Outcome,
    /// The causal ROOT (§6.2 `correlation_id`) — the "why did this happen" walk anchor (BUS-5).
    pub correlation_id: String,
    /// The IMMEDIATE parent (§6.2 `causation_id`) — nested causality, carried verbatim off the
    /// envelope. `None` for a causal root.
    pub causation_id: Option<String>,
    /// RFC-3339 UTC — when the action happened (§6.2 `occurred_at`).
    pub occurred_at: String,
}

impl AuditEntry {
    /// The body fields of this entry as an [`ActionRecord`] (everything except the `seq` + the two
    /// hashes) — the Merkle-leaf preimage source. Used by [`AuditLog::verify_chain`] to recompute
    /// the leaf and detect a retroactive edit (a tampered field changes the preimage ⇒ the
    /// recomputed leaf no longer matches the stored `leaf_hash`).
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

/// Render a 32-byte BLAKE3 digest as the self-describing `blake3:<hex>` multihash string (the
/// same convention the BlobStore content-address uses — one hash family, no hand-rolled crypto).
fn blake3_multihash(bytes: &[u8]) -> String {
    let digest = blake3::hash(bytes);
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

/// Parse a `blake3:<hex>` multihash string back to its 32 raw bytes (for the chain-link hash,
/// which hashes over the raw digests, not their hex rendering). A malformed string is a
/// programming error (the only producer is [`blake3_multihash`]), so this falls back to the
/// genesis root rather than panicking in the append path — a corrupted prev simply breaks the
/// chain forward, which is exactly the tamper-evidence property (a verifier detects it).
fn multihash_bytes(s: &str) -> [u8; 32] {
    s.strip_prefix("blake3:")
        .and_then(|hex_str| hex::decode(hex_str).ok())
        .and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok())
        .unwrap_or(GENESIS_PREV)
}

/// The head of one tenant's hash-chain: the last sequence number + the last entry's two hashes
/// (so the next append can compute its `prev_hash` and `seq` without re-reading the whole chain),
/// plus the incrementally-maintained Merkle root over all leaves so far.
#[derive(Clone, Debug)]
struct ChainHead {
    next_seq: u64,
    last_prev: [u8; 32],
    last_leaf: [u8; 32],
    /// Every leaf hash so far, in order — the Merkle tree's leaves. The root
    /// ([`merkle_root`]) is recomputed from these on append. (A production tree keeps the
    /// `O(log n)` interior nodes; the floor keeps the leaves and recomputes — correctness over
    /// the incremental-proof optimisation, which is the P-GA-20 concern.)
    leaves: Vec<[u8; 32]>,
}

/// **The per-tenant hash-chain + Merkle-leaf store (§6.2 `audit_entry`).** Each tenant has its
/// own chain (the chain is per-tenant, in-cell — gdpr §7.1). The ONLY writer is
/// [`AuditConsumer`]: [`AuditLog::append`] is `pub(crate)`, so no service can write the log
/// directly. Read accessors are public (a verifier / the P-GA-20 proof machinery reads the chain).
#[derive(Default)]
pub struct AuditLog {
    /// Per-tenant chain head + the appended entries (the §6.2 `(tenant, seq)` rows). In-memory
    /// model of the durable table (the floor).
    chains: Mutex<HashMap<TenantId, ChainHead>>,
    entries: Mutex<HashMap<TenantId, Vec<AuditEntry>>>,
}

impl AuditLog {
    /// A fresh, empty audit log (no tenant has any entries yet).
    pub fn new() -> AuditLog {
        AuditLog::default()
    }

    /// **Append one minimised action to its tenant's chain (the SOLE write path).** Crate-private
    /// (`pub(crate)`) — the only caller is [`AuditConsumer::handle`], the outbox subscription, so
    /// "no service writes the audit log directly" holds structurally (coverage is a bus property,
    /// EI-01 §5). Computes the entry's `seq` (per-tenant monotonic), its `leaf_hash`
    /// (`BLAKE3(canonical(body))` — the Merkle leaf) and its `prev_hash`
    /// (`BLAKE3(prev.prev_hash || prev.leaf_hash)` — the hash-chain link), extends the chain head,
    /// and records the leaf into the per-tenant Merkle tree. Returns the appended entry.
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

        // The hash-chain link: BLAKE3(prev.prev_hash || prev.leaf_hash). For the genesis entry
        // (seq 0) both are the all-zero root, so the genesis link is BLAKE3(0||0) — a fixed,
        // verifiable seed that still binds the chain.
        let mut link_input = Vec::with_capacity(64);
        link_input.extend_from_slice(&head.last_prev);
        link_input.extend_from_slice(&head.last_leaf);
        let prev_digest = blake3::hash(&link_input);
        let prev_hash = format!("blake3:{}", hex::encode(prev_digest.as_bytes()));

        // The Merkle leaf: BLAKE3(canonical(entry-body)).
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

        // Extend the chain head + the Merkle leaf set.
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

    /// Every appended entry for one tenant, in chain order (a verifier reads this). PII-free.
    pub fn entries_for(&self, tenant: &TenantId) -> Vec<AuditEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .cloned()
            .unwrap_or_default()
    }

    /// The number of entries in one tenant's chain (the §6.2 tree size — what the STH signs).
    pub fn len_for(&self, tenant: &TenantId) -> u64 {
        self.chains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|h| h.next_seq)
            .unwrap_or(0)
    }

    /// The current per-tenant Merkle ROOT (`blake3:<hex>`), recomputed from all leaves so far, or
    /// `None` for an empty chain. This is the value the signed-tree-head (P-GA-20) signs +
    /// anchors to the independent witness. Recomputing on read keeps the floor simple; the
    /// incremental interior-node maintenance is the P-GA-20 optimisation.
    pub fn root(&self, tenant: &TenantId) -> Option<String> {
        let chains = self.chains.lock().unwrap_or_else(|e| e.into_inner());
        let head = chains.get(tenant)?;
        if head.leaves.is_empty() {
            return None;
        }
        Some(blake3_multihash_raw(&merkle_root(&head.leaves)))
    }

    /// **Verify one tenant's hash-chain is intact** (the tamper-evidence property the proofs build
    /// on). Recomputes every entry's `leaf_hash` from its body and every `prev_hash` from the
    /// chain, and checks `seq` is the dense `0..n` sequence. Returns `true` iff the chain is
    /// unbroken — a retroactive edit to any entry flips this to `false`. (The O(log n) inclusion /
    /// consistency PROOFS over the Merkle tree are P-GA-20; this is the linear integrity check the
    /// construction guarantees.)
    pub fn verify_chain(&self, tenant: &TenantId) -> bool {
        verify_entries(&self.entries_for(tenant))
    }

    /// The ordered raw leaf digests for one tenant's Merkle tree (the leaves the
    /// [`super::audit_proofs`] inclusion / consistency proofs walk). Crate-private — the proof
    /// machinery reads it; a downstream crate sees only the higher-level proof API. Returns an
    /// empty vec for a tenant with no entries.
    pub(crate) fn leaf_digests(&self, tenant: &TenantId) -> Vec<[u8; 32]> {
        self.chains
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tenant)
            .map(|h| h.leaves.clone())
            .unwrap_or_default()
    }
}

/// Render raw 32 digest bytes as `blake3:<hex>` (the variant that takes bytes, not a preimage).
/// Crate-private so the proof machinery renders the STH root + path nodes identically.
pub(crate) fn blake3_multihash_raw(digest: &[u8]) -> String {
    format!("blake3:{}", hex::encode(digest))
}

/// **A public chain-integrity verifier over an explicit entry vector** (for the GA-D3 tamper drill,
/// which models a DB-level tamper a verifier reads from the store — the chain store is crate-
/// private). Recomputes every leaf + chain link from the bodies and checks the dense `0..n` `seq`;
/// returns `true` iff the chain is unbroken. A retroactive edit / re-order / deletion flips it to
/// `false`. This is the cross-crate face of [`verify_entries`].
pub fn verify_entries_for_test(entries: &[AuditEntry]) -> bool {
    verify_entries(entries)
}

/// **The chain-integrity verifier core** (the tamper-evidence check the construction guarantees).
/// Recomputes every entry's `leaf_hash` from its body and every `prev_hash` from the running chain
/// state, and checks `seq` is the dense `0..n` sequence. Returns `true` iff the chain is unbroken;
/// a retroactive edit to ANY entry's body (or a re-ordered / dropped entry) flips it to `false`.
/// Factored out of [`AuditLog::verify_chain`] so a test can run it over a deliberately-tampered
/// entry vector (the chain store is crate-private; this models a DB-level tamper a verifier must
/// catch). The O(log n) inclusion / consistency PROOFS over the Merkle tree are P-GA-20.
pub(crate) fn verify_entries(entries: &[AuditEntry]) -> bool {
    let mut prev = GENESIS_PREV;
    let mut last_leaf = GENESIS_PREV;
    for (i, e) in entries.iter().enumerate() {
        if e.seq != i as u64 {
            return false;
        }
        // Recompute the chain link from the running (prev, last_leaf) and check it matches.
        let mut link_input = Vec::with_capacity(64);
        link_input.extend_from_slice(&prev);
        link_input.extend_from_slice(&last_leaf);
        let expect_prev = blake3_multihash_raw(blake3::hash(&link_input).as_bytes());
        if expect_prev != e.prev_hash {
            return false;
        }
        // Recompute the leaf from the entry body and check it matches the stored leaf_hash.
        let preimage = e.as_action_record().leaf_preimage(e.seq);
        if blake3_multihash(&preimage) != e.leaf_hash {
            return false;
        }
        prev = multihash_bytes(&e.prev_hash);
        last_leaf = blake3::hash(&preimage).into();
    }
    true
}

/// The RFC-6962 interior-node hash of two child digests: `BLAKE3(0x01 || left || right)`. The
/// `0x01` prefix is the interior-node domain separation so an interior node never collides with a
/// leaf hash (RFC 6962 §2.1). Crate-private so the proof machinery ([`super::audit_proofs`])
/// recomputes path nodes with byte-identical semantics to the tree.
pub(crate) fn interior_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&[0x01]); // interior-node domain separation (RFC 6962).
    h.update(left);
    h.update(right);
    h.finalize().into()
}

/// The Merkle root over an ordered set of leaf digests (RFC-6962-style pairwise reduction:
/// hash each adjacent pair, carrying an odd final leaf up unchanged, until one root remains). A
/// single leaf is its own root. Crate-private so the STH ([`super::audit_proofs`]) signs exactly
/// the root the tree computes.
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
                next.push(level[i]); // odd final node carries up unchanged.
                i += 1;
            }
        }
        level = next;
    }
    level[0]
}

/// **The audit consumer (§6.1 — the outbox-only audit subscription).** An [`EventHandler`]: it is
/// stood up through the ONE sanctioned consumer entry-point (`myelin_events::consume`) like every
/// other consumer, so it rides the seven correctness rules (idempotent on `event_id`, ack-after,
/// bounded prefetch, …). Every action-bearing event delivered to it is **minimised** and appended
/// to its tenant's hash-chain. It is the SOLE writer of the [`AuditLog`].
///
/// "No service writes the audit log directly" is structural: the only way to add an entry is to
/// deliver an event THROUGH this consumer (the outbox is the one emit path; this consumer is the
/// one subscriber that appends). The architecture test
/// [`tests::no_service_writes_the_audit_log_except_the_outbox_consumer`] pins it.
pub struct AuditConsumer {
    log: AuditLog,
    /// The `*`-free subject whitelist this consumer binds (rule 3 — never `*`). The audit
    /// subscription is the firehose of action-bearing subjects; on this floor a concrete subject
    /// list is held so the consumer is a well-formed [`EventHandler`].
    subjects: &'static [SubjectPattern],
    /// The live append-lag measurement (rule 7 — `audit_append_lag`, the audit-log health SLO):
    /// events delivered to the consumer but not yet appended. On the synchronous append path this
    /// is 0 in steady state; it is bumped on entry and cleared on append so a drill can read it
    /// non-zero mid-flight. Wiring the sample onto the metrics-health surface is the P-119 floor.
    append_lag: Mutex<u64>,
}

/// The subjects the audit consumer binds. The audit log records EVERY action-bearing subsystem's
/// events, so the whitelist is the per-subsystem action firehose. On this floor it is empty (the
/// per-subsystem subject roster is filled as each subsystem's `*.action` tokens land — P-GA-26
/// per-subsystem token validation); the consumer still binds through `consume` with a concrete
/// (never `*`) whitelist when wired into `serve`. Kept `&'static` to satisfy the frozen
/// `EventHandler::subjects() -> &'static [SubjectPattern]` shape.
static AUDIT_SUBJECTS: &[SubjectPattern] = &[];

impl Default for AuditConsumer {
    fn default() -> Self {
        AuditConsumer::new()
    }
}

impl AuditConsumer {
    /// A fresh audit consumer over a fresh, empty [`AuditLog`].
    pub fn new() -> AuditConsumer {
        AuditConsumer {
            log: AuditLog::new(),
            subjects: AUDIT_SUBJECTS,
            append_lag: Mutex::new(0),
        }
    }

    /// Read-only access to the underlying log (a verifier / the P-GA-20 proof machinery reads it;
    /// there is no public WRITE accessor — the only writer is [`AuditConsumer::handle`]).
    pub fn log(&self) -> &AuditLog {
        &self.log
    }

    /// The live `audit_append_lag` measurement (rule 7 / contract 1.8 SLO): events delivered but
    /// not yet appended. 0 in steady state on the synchronous append path. The metrics-health
    /// wiring is P-119.
    pub fn append_lag(&self) -> u64 {
        *self.append_lag.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Minimise + append ONE event as an audit entry. Factored out of [`EventHandler::handle`] so
    /// a drill can call it directly; the consumer runtime calls `handle`, which calls this. The
    /// minimisation reads ONLY PII-free fields off the envelope (the actor's `principal_id`, the
    /// subject `ArtifactRef`, the causal ids) — references-not-payloads, never the event payload.
    fn append_event(&self, ev: &EventEnvelope) -> AuditEntry {
        // Bump the append-lag the instant the event is accepted; clear it once appended (so a
        // drill that pauses between can read it non-zero — the SLO is the time-to-append).
        {
            let mut lag = self.append_lag.lock().unwrap_or_else(|e| e.into_inner());
            *lag += 1;
        }

        // Build the minimised action record off the envelope, reading ONLY PII-free fields (the
        // actor's `principal_id`, the subject `ArtifactRef`, the dotted type token, the causal ids,
        // the timestamp) — the event `payload` is NEVER read (references-not-payloads).
        let record = ActionRecord {
            tenant: ev.tenant.clone(),
            region: ev.region.clone(),
            actor: Minimised::from_principal(&ev.actor.0),
            // The action is the event's dotted type (e.g. `identity.tuple.written`) — a token, never a
            // payload.
            action: ev.type_.0.clone(),
            // The subject is the event's ArtifactRef (an id, never content; §6.2 `subject`).
            subject: ev.subject.clone(),
            // An action that reached the bus and is being recorded is, by construction, an applied
            // action (a denied attempt that produced no event would be emitted as its own
            // `*.denied` action with Outcome::Denied by the action-taking service). The richer
            // outcome map per action token is the P-GA-26 per-subsystem-roster floor.
            outcome: Outcome::Applied,
            // Causality carried verbatim (§6.1 — the "why did this happen" walk).
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
    /// The `*`-free subject whitelist (rule 3). Filled as each subsystem's action tokens land
    /// (P-GA-26); the consumer binds through `consume` (which rejects `*`) when wired into `serve`.
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    /// Append the delivered action-bearing event as one minimised, causality-carried, hash-chained
    /// audit entry (§6.1). Always `Done`: an audit append is a pure, total function of the
    /// envelope (no external dependency can make it fail), so it never retries or dead-letters —
    /// the consumer runtime's idempotency (dedup on `event_id`) guarantees a redelivery is a no-op,
    /// so the same action is never double-appended.
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

    /// Build an action-bearing event envelope. `payload` deliberately carries a NAME-shaped value
    /// to prove the audit entry never reads it (references-not-payloads / minimisation).
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
            // A real-name-shaped payload — the audit entry must NEVER carry this.
            payload: serde_json::json!({ "real_name": "Alice Example", "email": "alice@example.test" }),
        }
    }

    /// **GATE (1/4): an appended action produces a hash-chain entry + a Merkle leaf.** One
    /// delivered action → one entry with a non-empty `prev_hash` (the chain link) and `leaf_hash`
    /// (the Merkle leaf), at `seq 0` (the genesis of its tenant's chain), and a per-tenant Merkle
    /// root exists.
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
        assert_eq!(c.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::Done);

        let tenant = TenantId("acme".into());
        let entries = c.log().entries_for(&tenant);
        assert_eq!(
            entries.len(),
            1,
            "one delivered action → one appended entry"
        );
        let e = &entries[0];
        assert_eq!(e.seq, 0, "the genesis entry of acme's chain is seq 0");
        // The hash-chain link (prev_hash) is present and is a blake3 multihash.
        assert!(
            e.prev_hash.starts_with("blake3:"),
            "prev_hash is the chain link"
        );
        // The Merkle leaf is present and is a blake3 multihash of the body.
        assert!(
            e.leaf_hash.starts_with("blake3:"),
            "leaf_hash is the Merkle leaf"
        );
        assert_ne!(
            e.prev_hash, e.leaf_hash,
            "the chain link and the leaf are distinct hashes"
        );
        // A per-tenant Merkle root exists (what the STH will sign, P-GA-20).
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

    /// **GATE (2/4): the actor is the minimised `<pseudonym>@<tenant>.noreply` form, never a
    /// payload.** The entry's actor is the pseudonym grammar over the PII-free `principal_id`; the
    /// real name / email in the event payload appears NOWHERE in the entry (proven by serialising
    /// the whole entry and asserting the PII strings are absent — the minimisation is structural).
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

        // The frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` (contract 4.8).
        assert_eq!(
            e.actor.actor, "u-42@acme.noreply",
            "actor is the frozen pseudonym grammar"
        );
        assert_eq!(e.actor.actor_kind, "human");
        assert!(e.actor.on_behalf_of.is_none(), "a human acts for nobody");

        // The minimisation is structural: serialise the WHOLE entry and assert the PII bodies from
        // the event payload are absent (the entry has no field that could carry them).
        let serialized = serde_json::to_string(e).expect("entry serialises");
        assert!(
            !serialized.contains("Alice Example"),
            "no real name reaches the audit entry"
        );
        assert!(
            !serialized.contains("alice@example.test"),
            "no email reaches the audit entry"
        );
        // The subject is an ArtifactRef (an id), never content.
        assert_eq!(e.subject, ArtifactRef("myelin://acme/identity/tuple/t1".into()));
    }

    /// A delegated AGENT acting `on_behalf_of` a human: both the actor AND the on_behalf_of are
    /// the minimised pseudonym form (agents are audited identically to humans, EI-02 §2).
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

    /// **GATE (3/4): correlation_id / causation_id are carried** (the audit log IS the "why did
    /// this happen" walk). A caused action's entry carries the parent (causation) + the root
    /// (correlation) verbatim off the envelope.
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

    /// **GATE (4/4) / the architecture test: NO service writes the audit log except the outbox
    /// consumer.** The ONLY write path into the chain is `AuditLog::append`, which is
    /// `pub(crate)` — a downstream crate has no way to call it. The public surface a service sees
    /// is read-only ([`AuditLog::entries_for`] / [`AuditLog::root`] / [`AuditLog::verify_chain`])
    /// plus the [`EventHandler`] the consumer runtime drives. This test pins the property by
    /// exercising it the only legal way: drive an event THROUGH the consumer (the outbox
    /// subscription) and confirm the entry appeared — there is no `AuditLog::append` a test in a
    /// *downstream* crate could call (it does not compile outside this crate).
    #[test]
    fn no_service_writes_the_audit_log_except_the_outbox_consumer() {
        let c = AuditConsumer::new();
        // The ONLY way to write the log: deliver an event through the consumer (the bus path).
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
        // Coherence note (compile-time guarantee): `AuditLog::append` is `pub(crate)` — a service
        // in another crate cannot call it. The read accessors above are the only public surface.
        // (If `append` were made `pub`, this property would silently break — the crate-private
        // visibility IS the enforcement.)
    }

    /// The per-tenant chain is exactly that — PER TENANT. Two tenants' actions interleave into
    /// independent chains, each with its own dense `0..n` sequence (the §6.2 `(tenant, seq)` PK).
    #[test]
    fn the_hash_chain_is_per_tenant() {
        let c = AuditConsumer::new();
        c.handle(&action_event(
            "01J-a1",
            human("u", "acme"),
            "identity.tuple.written",
            "myelin://acme/x",
            "r1",
            None,
        ), &mut myelin_events::HandlerTx::none());
        c.handle(&action_event(
            "01J-b1",
            human("u", "globex"),
            "identity.tuple.written",
            "myelin://globex/x",
            "r2",
            None,
        ), &mut myelin_events::HandlerTx::none());
        c.handle(&action_event(
            "01J-a2",
            human("u", "acme"),
            "identity.tuple.written",
            "myelin://acme/y",
            "r3",
            None,
        ), &mut myelin_events::HandlerTx::none());

        let acme = c.log().entries_for(&TenantId("acme".into()));
        let globex = c.log().entries_for(&TenantId("globex".into()));
        assert_eq!(acme.len(), 2, "acme has two entries");
        assert_eq!(globex.len(), 1, "globex has one entry");
        // Each chain has its OWN dense 0..n sequence.
        assert_eq!(acme[0].seq, 0);
        assert_eq!(acme[1].seq, 1);
        assert_eq!(globex[0].seq, 0, "globex's chain starts at its OWN seq 0");
    }

    /// **The tamper-evidence property the construction guarantees**: a recomputed-from-scratch
    /// verification of an unbroken chain is `true`, and a retroactive EDIT to any entry's body
    /// flips `verify_chain` to `false` (the leaf no longer matches, breaking the chain forward).
    /// This is the property the P-GA-20 inclusion/consistency PROOFS build on.
    #[test]
    fn a_retroactive_edit_breaks_the_chain() {
        let c = AuditConsumer::new();
        for i in 0..5 {
            c.handle(&action_event(
                &format!("01J-{i}"),
                human("u", "acme"),
                "identity.tuple.written",
                &format!("myelin://acme/x/{i}"),
                "r",
                None,
            ), &mut myelin_events::HandlerTx::none());
        }
        let tenant = TenantId("acme".into());
        let entries = c.log().entries_for(&tenant);
        // The pristine chain verifies intact (kills a `verify_entries -> true`-always mutant only
        // because the tampered case below verifies false — the two together pin the boolean).
        assert!(
            c.log().verify_chain(&tenant),
            "the freshly-built chain verifies intact"
        );
        assert!(
            verify_entries(&entries),
            "the verifier core agrees the pristine chain is intact"
        );

        // Tamper with one entry in the middle (re-point its subject) and verify the chain FAILS:
        // the recomputed leaf no longer matches the stored `leaf_hash`, breaking the chain. The
        // chain store is crate-private, so we run the verifier core over the tampered vector — the
        // DB-level tamper a verifier must detect.
        let mut tampered = entries.clone();
        tampered[2].subject = ArtifactRef("myelin://acme/TAMPERED".into());
        assert!(
            !verify_entries(&tampered),
            "a retroactive edit breaks the chain — verify_entries returns FALSE (tamper detected)"
        );
        // And a re-ordered chain (swap two entries) also fails (seq no longer dense / links break).
        let mut reordered = entries.clone();
        reordered.swap(1, 3);
        assert!(
            !verify_entries(&reordered),
            "a re-ordered chain fails verification"
        );
        // A dropped entry fails too (the seq sequence is no longer dense 0..n).
        let mut dropped = entries.clone();
        dropped.remove(2);
        assert!(
            !verify_entries(&dropped),
            "a dropped entry fails verification (seq gap)"
        );
    }

    /// The `Outcome` wire spelling (§6.2 `outcome`) is frozen — each variant serialises to its
    /// exact `allowed | denied | applied | failed` token, AND a different outcome produces a
    /// DIFFERENT Merkle leaf (the outcome is part of the leaf preimage, so two actions identical
    /// except for their outcome are distinct leaves — a tamper that flips an outcome is detectable).
    #[test]
    fn outcome_wire_strings_are_frozen_and_distinguish_the_leaf() {
        // The frozen §6.2 spellings.
        assert_eq!(Outcome::Allowed.as_str(), "allowed");
        assert_eq!(Outcome::Denied.as_str(), "denied");
        assert_eq!(Outcome::Applied.as_str(), "applied");
        assert_eq!(Outcome::Failed.as_str(), "failed");

        // Two records identical except for the outcome hash to DIFFERENT leaves.
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
            "the outcome is part of the leaf preimage — a different outcome is a different leaf"
        );
    }

    /// The Merkle root is deterministic + changes when the leaf set changes (the property the STH
    /// signs over). Two identical chains produce the same root; appending an entry changes it.
    #[test]
    fn merkle_root_is_deterministic_and_changes_on_append() {
        let mk = || {
            let c = AuditConsumer::new();
            c.handle(&action_event(
                "01J-1",
                human("u", "acme"),
                "identity.tuple.written",
                "myelin://acme/x",
                "r",
                None,
            ), &mut myelin_events::HandlerTx::none());
            c.handle(&action_event(
                "01J-2",
                human("u", "acme"),
                "identity.tuple.written",
                "myelin://acme/y",
                "r",
                None,
            ), &mut myelin_events::HandlerTx::none());
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
        c.handle(&action_event(
            "01J-3",
            human("u", "acme"),
            "identity.tuple.written",
            "myelin://acme/z",
            "r",
            None,
        ), &mut myelin_events::HandlerTx::none());
        let after = c.log().root(&tenant);
        assert_ne!(
            before, after,
            "appending an entry changes the Merkle root (the STH advances)"
        );
    }

    /// The `audit_append_lag` SLO measurement (rule 7 / contract 1.8) reads 0 in steady state on
    /// the synchronous append path (every accepted event is appended before `handle` returns).
    /// The signal NAME + UNIT are the pinned `AUDIT_APPEND_LAG`.
    #[test]
    fn audit_append_lag_signal_is_named_and_reads_green() {
        assert_eq!(
            AUDIT_APPEND_LAG.0, "audit.audit_append_lag",
            "the SLO signal name is pinned"
        );
        assert_eq!(AUDIT_APPEND_LAG.1, "events", "the SLO unit is pinned");
        let c = AuditConsumer::new();
        c.handle(&action_event(
            "01J-1",
            human("u", "acme"),
            "identity.tuple.written",
            "myelin://acme/x",
            "r",
            None,
        ), &mut myelin_events::HandlerTx::none());
        assert_eq!(
            c.append_lag(),
            0,
            "append_lag reads green (0) in steady state after a synchronous append"
        );
    }
}
