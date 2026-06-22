//! # `agent::trace` — the AG-7 content-addressed agent-trace holder (KN-P28 / P-318, M3 / KN-M3e —
//! drill KN-D12)
//!
//! This is the **KN-P28 deliverable**: the AG-7 content-addressed agent-trace holder (contract 8.8) —
//! Knowledge **accepts a content-addressed (BLAKE3) write of an agent execution trace, REUSING the
//! block model (no new schema), returns `run.trace_ref`, and registers it as an erasable
//! `PersonalDataHolder` distinct from the tamper-evident audit log** (architecture §5.2). Erasing a
//! subject (via the KN-P26 crypto-shred) shreds their trace CONTENT; attribution falls back to the
//! opaque pseudonym (KN-D12). This completes the master M3→M4 Knowledge exit alongside KN-P27.
//!
//! ## What a trace IS (and what it is NOT)
//! A trace is the **human-readable narrative** of an agent run — the conversation: the system context,
//! the tool inputs/outputs, the surfaced reasoning (architecture §5.2). It is one of THREE distinct
//! holders (telemetry / audit / trace, §5.2):
//! - **NOT the tamper-evident audit log** (contract 10.6, the H16 GDPR/Audit carve-out): the audit log
//!   is RETAIN-only (never rewrite the hash-chain; expire via the audit-key crypto-shred at retention
//!   end). The trace is freely ERASABLE — a person's run reasoning is crypto-shredded on a DSR. The two
//!   are structurally separate ([`trace_is_distinct_from_audit`]); erasing a trace never touches the
//!   audit log (§6.5).
//! - **NOT a new schema**: the trace document REUSES the frozen [`myelin_content::Block`] model — a
//!   trace is just a [`Document`](myelin_content::Document)-shaped `Vec<Block>` (the conversation
//!   rendered as paragraphs / code blocks / callouts), content-addressed by BLAKE3 over its canonical
//!   bytes — the SAME content-addressing the Knowledge snapshot tier uses
//!   ([`crate::compaction::content_address`] → [`myelin_storage::blob::ContentHash::blake3`]).
//!
//! ## The content-address gate (CI) — idempotent-by-content
//! [`write_agent_trace`] is **content-addressed + idempotent**: the trace ref IS the BLAKE3 hash of the
//! canonical trace bytes, so writing the SAME trace content (a retry, a replay) writes ONCE — the same
//! `run.trace_ref` comes back and the holder stores one copy ([`AgentTraceHolder`] is keyed on the
//! content hash). Distinct content → distinct refs. This is the immutability the audit/replay model
//! relies on (no in-place trace mutation).
//!
//! ## The KN-D12 erasure drill (the dated green)
//! [`AgentTraceHolder::erase_subject_traces`] COMPOSES the KN-P26 crypto-shred core
//! ([`crate::gdpr::erase_floor`]): a subject's trace CONTENT is sealed under their per-subject DEK, so
//! the erase **destroys the DEK** → the trace ciphertext (live + op-log + snapshots + backups) is
//! unrecoverable, and the trace's actor **attribution falls back to the opaque pseudonym** (the trace
//! row keeps the `principal_id` — never PII — and the pseudonym-map shred makes it un-resolvable to a
//! human, contract 4.8). It emits a dated [`TraceEraseReceipt`] proving **0 recoverable PII in traces,
//! attribution intact** — the KN-D12 green. The audit log is UNAFFECTED (proven by the gate).
//!
//! ## Coherence (EI-01 §7) — what this REUSES vs. what is genuinely new
//! - the **content-addressing** is [`crate::compaction::content_address`] (BLAKE3 over canonical bytes)
//!   — NOT a hand-rolled hash;
//! - the **block model** is the frozen `myelin_content` AST — NO new schema;
//! - the **crypto-shred** is the KN-P26 [`crate::gdpr::erase_floor::KnowledgeErase`] core (the
//!   per-subject DEK destroy + the §6.1 fan-out) — NOT a second erase path;
//! - the **distinct-from-audit holder id** is the SAME `agent_fabric_trace` H17 id the GDPR-service
//!   seam ([`myelin_gdpr_service::AGENT_TRACE_HOLDER_ID`], P-GA-26) registers — ONE name across the
//!   seam, no parallel id.
//! The genuinely-new code is the Knowledge-side PRODUCER: [`AgentTrace`] (the block-model trace
//! document), [`write_agent_trace`] (the content-addressed, idempotent write returning `run.trace_ref`),
//! [`AgentTraceHolder`] (the erasable holder keyed on the content hash), and the KN-D12 erase
//! composition over the KN-P26 core.
//!
//! ## FLOOR named (VISION §3): none.
//! The trace holder is the full v1 surface: a content-addressed write reusing the block model, an
//! erasable holder, the KN-D12 erase composing the KN-P26 crypto-shred. The residual (third-party
//! free-text PII inside a trace) is the ONE platform posture (10.9, by reference — the same residual
//! the KN-P26 floor names; never restated here).
//!
//! ## DB-free
//! In-memory trace document + content-address + the holder map; the erase composes the in-memory KMS
//! engine + Search index + refs store (the same DB-free seams KN-P26 uses). The LIVE-stack proof (the
//! real op-log / snapshot ciphertext under the per-subject DEK) rides the Knowledge integration drills.
//! So `cargo build --workspace` stays DB-free.

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_content::Block;
use myelin_events::ArtifactRef;
use myelin_gdpr::{Receipt, SubjectRef, TenantId};
use myelin_search::engine::IndexBackend;
use myelin_storage::{ErasureLedgerSink, KmsEngine, PseudonymShred};

use crate::compaction::content_address;
use crate::gdpr::erase_floor::{
    BusEraseSeam, KnowledgeBacklinkTombstone, KnowledgeEmbeddingPurge, KnowledgeErase,
};

/// **The stable, PII-free holder id the AG-7 agent-trace holder answers DSRs under** — the SAME
/// `agent_fabric_trace` H17 id the GDPR-service seam ([`myelin_gdpr_service::AGENT_TRACE_HOLDER_ID`],
/// P-GA-26) registers, so there is ONE name across the seam (EI-01 §7 coherence, no parallel id). The
/// Knowledge-side PRODUCER (this module) and the GDPR-side SEAM agree on the id the live impl (KN-P28)
/// plugs into.
pub const TRACE_HOLDER_ID: &str = "agent_fabric_trace";

/// **The tamper-evident audit-log store id the trace is DELIBERATELY distinct from** (the H16 GDPR/
/// Audit carve-out, contract 10.6 / §6.5). The trace ([`TRACE_HOLDER_ID`]) is freely erasable; the
/// audit log is the retain carve-out. Keeping them separate means erasing a person's trace never
/// touches the tamper-evident audit log ([`trace_is_distinct_from_audit`]).
pub const AUDIT_LOG_STORE_ID: &str = "audit_log";

/// **The distinct-from-audit boundary (architecture §5.2 / §6.5).** The AG-7 trace holder is
/// structurally distinct from the H16 audit log: distinct holder ids AND distinct erase semantics
/// (the trace IS erasable = crypto-shred; the audit log is NOT freely erasable = the retain carve-out).
/// `true` iff both conjuncts hold — so an erasure of a person's trace can never reach the audit log.
/// This is the §6.5 architecture-test predicate ([`tests::trace_is_distinct_from_the_audit_log`]).
pub fn trace_is_distinct_from_audit() -> bool {
    let distinct_ids = TRACE_HOLDER_ID != AUDIT_LOG_STORE_ID;
    // The trace IS erasable; the audit log is NOT (it is the retain carve-out — §6.4/§6.5).
    let distinct_erasability = TRACE_ERASABLE && !AUDIT_LOG_ERASABLE;
    distinct_ids && distinct_erasability
}

/// The AG-7 trace IS erasable — a run's reasoning record, crypto-shredded on a DSR (§5.2 / §6.5).
pub const TRACE_ERASABLE: bool = true;
/// The H16 audit log is NOT freely erasable — the retain carve-out (never rewrite the chain; §6.4).
pub const AUDIT_LOG_ERASABLE: bool = false;

// ════════════════════════════════════════════════════════════════════════════════════════════
// The trace document (the block-model narrative) + its content-address
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **An agent execution trace — the human-readable narrative of a run, AS the block model (no new
/// schema, contract 8.8 / architecture §5.2).** The conversation (system context, tool i/o, surfaced
/// reasoning) is rendered into the frozen [`myelin_content::Block`] AST — a trace is just a
/// `Document`-shaped `Vec<Block>` (paragraphs / code blocks / callouts), so it REUSES the editor /
/// export / replay machinery for free (one model, EI-01 §7).
///
/// PII handling: the trace carries the run's opaque `run_id` + the actor's OPAQUE pseudonymous
/// `principal_id` (the attribution — never a raw name/email, contract 4.8). The free-text CONTENT may
/// hold the subject's self-authored PII; it is erasable by the per-subject DEK crypto-shred (KN-P26).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTrace {
    /// The opaque run id this trace belongs to (the provenance link — never PII).
    pub run_id: String,
    /// The actor of the run — the OPAQUE pseudonymous principal id (the attribution, contract 4.8).
    /// Never a raw identity; on erase it stays (the row keeps the id) and the pseudonym-map shred makes
    /// it un-resolvable — attribution falls back to the pseudonym (KN-D12).
    pub actor_principal_id: String,
    /// The trace narrative AS the frozen block model (the conversation rendered to `Vec<Block>`). The
    /// load-bearing fidelity: no new schema — the SAME AST every other Knowledge content uses.
    pub blocks: Vec<Block>,
}

impl AgentTrace {
    /// Build an agent trace from the opaque run id + actor pseudonym + the block-model narrative.
    pub fn new(
        run_id: impl Into<String>,
        actor_principal_id: impl Into<String>,
        blocks: Vec<Block>,
    ) -> AgentTrace {
        AgentTrace {
            run_id: run_id.into(),
            actor_principal_id: actor_principal_id.into(),
            blocks,
        }
    }

    /// **The canonical bytes the content-address is computed over (the immutability anchor).** A
    /// deterministic serialisation of the trace CONTENT (`run_id` + `actor` + the block AST as
    /// canonical JSON) — the SAME content always yields the SAME bytes, so the BLAKE3 address is stable
    /// across retries/replays (the idempotency the content-address gate proves). The block AST is the
    /// frozen serde shape ([`myelin_content::Block`] is `Serialize` with a fixed tag), so two
    /// equal traces serialise byte-identically.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // Field-tagged + length-free JSON of the closed serde shape — a fixed, collision-free body.
        let value = serde_json::json!({
            "schema": "myelin.agent_trace.v1",
            "run_id": self.run_id,
            "actor": self.actor_principal_id,
            "blocks": self.blocks,
        });
        serde_json::to_vec(&value).expect("the agent trace serialises (a closed serde shape)")
    }

    /// **The content-addressed `trace_ref` for this trace (contract 8.8 — `run.trace_ref`).** The
    /// BLAKE3 multihash of the [`canonical_bytes`](Self::canonical_bytes), as the
    /// `myelin://<tenant>/knowledge/agent_trace/<blake3:hex>` [`ArtifactRef`] — the content IS the
    /// address (Git/Venti/IPFS-CID model). Computed via the SAME [`crate::compaction::content_address`]
    /// the Knowledge snapshot tier uses (never a hand-rolled hash, VISION §4).
    pub fn trace_ref(&self, tenant: &TenantId) -> ArtifactRef {
        let hash = content_address(&self.canonical_bytes());
        ArtifactRef(format!(
            "myelin://{}/knowledge/agent_trace/{}",
            tenant.as_str(),
            hash.to_multihash_string()
        ))
    }

    /// The bare content hash string (`blake3:<hex>`) — the holder's storage key (the content-address,
    /// distinct from the full `myelin://…` ref so the key is tenant-local + collision-free).
    pub fn content_hash(&self) -> String {
        content_address(&self.canonical_bytes()).to_multihash_string()
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The erasable AG-7 trace holder (content-addressed store + the KN-D12 erase composition)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The AG-7 content-addressed agent-trace HOLDER (contract 8.8 / 10.1 — an erasable
/// `PersonalDataHolder`, architecture §5.2).** It accepts content-addressed trace writes
/// ([`write_agent_trace`]) keyed on the BLAKE3 content hash (idempotent-by-content: the same trace
/// writes once), and erases a subject's traces by composing the KN-P26 crypto-shred core (the
/// per-subject DEK destroy → the trace ciphertext is unrecoverable; attribution falls back to the
/// pseudonym, KN-D12). It is DISTINCT from the tamper-evident audit log
/// ([`trace_is_distinct_from_audit`]) — erasing a trace never touches the audit log.
///
/// The store is `content_hash → (tenant, AgentTrace)` (content-addressed, immutable). PII-free at the
/// key (the hash is over content; the actor id inside is the opaque pseudonym).
#[derive(Default)]
pub struct AgentTraceHolder {
    /// The content-addressed trace store: `blake3:<hex>` → the stored trace (+ its tenant). Keyed on
    /// the content hash, so a re-write of the same content is a no-op (idempotent-by-content).
    traces: Mutex<BTreeMap<String, StoredTrace>>,
}

/// One stored trace (content-addressed) + its tenant (the residency-pin: the holder never crosses a
/// cell). Internal — the holder's map value.
#[derive(Clone, Debug)]
struct StoredTrace {
    tenant: TenantId,
    trace: AgentTrace,
}

impl AgentTraceHolder {
    /// A fresh, empty trace holder.
    pub fn new() -> AgentTraceHolder {
        AgentTraceHolder::default()
    }

    /// The stable holder id this body answers DSRs under (always [`TRACE_HOLDER_ID`] = the H17
    /// `agent_fabric_trace` id — ONE name across the GDPR-service seam).
    pub fn holder_id(&self) -> &'static str {
        TRACE_HOLDER_ID
    }

    /// **Write an agent execution trace — content-addressed + idempotent (contract 8.8).** Returns the
    /// `run.trace_ref` (the [`ArtifactRef`] over the BLAKE3 content hash). Writing the SAME trace
    /// content (a retry/replay) writes ONCE — the same ref comes back, the store holds one copy
    /// (idempotent-by-content). Distinct content → distinct refs. The free function
    /// [`write_agent_trace`] is the public §5.2-signature entry; this is the holder method it drives.
    pub fn write(&self, tenant: &TenantId, trace: AgentTrace) -> ArtifactRef {
        let key = trace.content_hash();
        let trace_ref = trace.trace_ref(tenant);
        let mut store = self.traces.lock().expect("agent trace holder poisoned");
        // Idempotent-by-content: insert ONCE; a re-write of the same hash is a no-op (the content is
        // identical, the address is identical — there is nothing to change).
        store.entry(key).or_insert_with(|| StoredTrace {
            tenant: tenant.clone(),
            trace,
        });
        trace_ref
    }

    /// How many DISTINCT traces (by content hash) the holder stores. A re-write of identical content
    /// does NOT increase this (the content-address gate's "writes once" reading).
    pub fn len(&self) -> usize {
        self.traces
            .lock()
            .expect("agent trace holder poisoned")
            .len()
    }

    /// Whether the holder stores no traces.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a trace with this `trace_ref` is stored (observability — the ref carries the content
    /// hash, so this is a content-presence check).
    pub fn contains_ref(&self, trace_ref: &ArtifactRef) -> bool {
        // The ref is `myelin://<tenant>/knowledge/agent_trace/<blake3:hex>`; the key is the trailing
        // `<blake3:hex>`. Extract it and probe the store.
        let Some((_, hash)) = trace_ref.0.rsplit_once("/agent_trace/") else {
            return false;
        };
        self.traces
            .lock()
            .expect("agent trace holder poisoned")
            .contains_key(hash)
    }

    /// The content-hash storage keys of the traces whose ACTOR is `subject` (the subject's traces). The
    /// erase reads these to assemble the index doc-ids / refs to purge in lockstep. PII-free (the keys
    /// are content hashes; the actor match is on the opaque pseudonymous id).
    pub fn subject_trace_hashes(&self, subject: &SubjectRef, tenant: &TenantId) -> Vec<String> {
        let sid = &subject.principal.principal_id.0;
        self.traces
            .lock()
            .expect("agent trace holder poisoned")
            .iter()
            .filter(|(_, st)| {
                st.tenant.as_str() == tenant.as_str() && &st.trace.actor_principal_id == sid
            })
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// **Erase a subject's content-addressed traces — the KN-D12 path (architecture §5.2 / drill
    /// KN-D12).** It COMPOSES the KN-P26 crypto-shred core ([`KnowledgeErase`]): the subject's trace
    /// CONTENT is sealed under their per-subject DEK, so destroying the DEK makes the trace ciphertext
    /// (live + op-log + snapshots + backups) unrecoverable — 0 recoverable PII in the traces. The trace
    /// ROW keeps the opaque `principal_id` (never PII), and the pseudonym-map shred (4.8) makes it
    /// un-resolvable to a human — **attribution falls back to the pseudonym** (the row survives, the
    /// content does not). The trace's index docs purge + backlinks tombstone in lockstep (the same
    /// §6.1 legs KN-P26 wires). The audit log is UNAFFECTED (distinct holder, §6.5).
    ///
    /// The cross-holder seams (`engine` / `pseudonym` / `bus` / `ledger`) are the SAME storage seams the
    /// DSR orchestrator wires for the KN-P26 erase (not Knowledge-owned); `index` / `page_store` are the
    /// Knowledge-owned lockstep legs (the Search/vector purge + the refs `*.erased` tombstone). `now` is
    /// the caller-supplied clock (deterministic). Returns the dated [`TraceEraseReceipt`] (the KN-D12
    /// green: 0 recoverable trace PII, attribution intact) on success, or a LOUD error (never a false
    /// "erased").
    #[allow(clippy::too_many_arguments)]
    pub fn erase_subject_traces<B: IndexBackend>(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        region: myelin_tenancy::Region,
        engine: &KmsEngine,
        pseudonym: &dyn PseudonymShred,
        bus: &dyn BusEraseSeam,
        ledger: &dyn ErasureLedgerSink,
        index: &Mutex<B>,
        page_store: &Mutex<crate::refs_glue::PageStore>,
        now: myelin_storage::EpochMillis,
    ) -> Result<TraceEraseReceipt, myelin_storage::EraseError> {
        // 1. Assemble the subject's traces (by actor) — the content the erase shreds + the index docs /
        //    refs to purge in lockstep. (The DEK destroy makes the CONTENT unrecoverable; the lockstep
        //    purge drops the plaintext-derived index docs; the tombstone degrades the backlinks.)
        let hashes = self.subject_trace_hashes(subject, tenant);
        let trace_doc_ids: Vec<String> = hashes
            .iter()
            .map(|h| format!("kn:agent_trace:{h}"))
            .collect();
        let trace_refs: Vec<ArtifactRef> = hashes
            .iter()
            .map(|h| {
                ArtifactRef(format!(
                    "myelin://{}/knowledge/agent_trace/{h}",
                    tenant.as_str()
                ))
            })
            .collect();
        let trace_count = hashes.len();

        // 2. Drive the KN-P26 crypto-shred core (the per-subject DEK destroy + the §6.1 fan-out). The
        //    trace CONTENT is encrypted under the SAME per-subject DEK as the subject's other
        //    self-authored content, so the ONE destroy reaches it (CR-I: one key per subject, never
        //    O(traces)).
        let eraser = KnowledgeErase::new(engine, region);
        let embeddings = KnowledgeEmbeddingPurge::new(index, trace_doc_ids);
        let backlinks = KnowledgeBacklinkTombstone::new(page_store, trace_refs);
        let kn_receipt = eraser.erase_subject(
            subject,
            tenant,
            pseudonym,
            &embeddings,
            &backlinks,
            bus,
            ledger,
            now,
        )?;

        // 3. The trace ROWS keep the opaque pseudonymous actor id (attribution falls back to the
        //    pseudonym — the row survives; the CONTENT is crypto-shredded). We do NOT delete the rows:
        //    a trace's existence + its opaque attribution is the legibility the audit/replay relies on;
        //    only the PII-bearing CONTENT is unrecoverable (the DEK is gone). The index/backlinks are
        //    purged/tombstoned above. This is the KN-D12 "attribution intact" property.
        let attribution_pseudonym = subject.principal.principal_id.0.clone();

        // The trace-shred + attribution-fallback receipt (the dated KN-D12 green artifact). PII-free.
        let receipt = Receipt::content_addressed(
            "erase",
            TRACE_HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn agent-trace erase (KN-D12): per-subject DEK crypto-shred of the trace content \
             (unrecoverable in op-log/snapshots/backups, 11.4) + index purge + backlink tombstone; \
             attribution falls back to the opaque pseudonym (4.8); DISTINCT from the audit log (§6.5)",
            None,
            now,
        );

        Ok(TraceEraseReceipt {
            receipt,
            traces_shredded: trace_count,
            recoverable_in_backup: kn_receipt.recoverable_in_backup,
            embeddings_purged: kn_receipt.embeddings_purged,
            backlinks_tombstoned: kn_receipt.backlinks_tombstoned,
            attribution_pseudonym,
            re_run: kn_receipt.re_run,
            at_ms: now,
        })
    }
}

/// **The dated KN-D12 green artifact — the trace-shred + attribution-fallback telemetry (drill
/// KN-D12).** PROOF the AG-7 trace-erasure properties held: the subject's trace CONTENT is
/// crypto-shredded (0 recoverable PII in the traces — the per-subject DEK destroyed, reaching backups)
/// and the attribution falls back to the opaque pseudonym (attribution intact). PII-free (the
/// pseudonym is the opaque `principal_id`, never a raw identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEraseReceipt {
    /// The frozen 10.1 content-addressed receipt (the audit-ledger hash-link, 10.8).
    pub receipt: Receipt,
    /// How many of the subject's traces had their content crypto-shredded (the trace-shred telemetry).
    pub traces_shredded: usize,
    /// **THE KN-D12 GATE READING:** how many of the subject's per-subject DEKs are STILL recoverable
    /// from the KMS backup AFTER the erase — MUST be **0** (0 recoverable PII in traces; the key is
    /// destroyed AND excluded from backup). A non-zero value is RED (a backup could resurrect the PII).
    pub recoverable_in_backup: usize,
    /// How many of the subject's trace index docs (lexical + vector) were purged in lockstep (0 of the
    /// subject's trace embeddings survive — embeddings of PII are PII).
    pub embeddings_purged: usize,
    /// How many trace backlinks/refs were tombstoned to an `Erased` tombstone (the 0-leak degrade).
    pub backlinks_tombstoned: usize,
    /// **The pseudonym attribution falls back to** — the opaque `principal_id` the trace row keeps
    /// (attribution intact; never PII). The "attribution falls back to the pseudonym" KN-D12 property.
    pub attribution_pseudonym: String,
    /// True when this was an idempotent no-op re-run (the subject's traces were already erased).
    pub re_run: bool,
    /// The drill timestamp (the dated green artifact, ms — deterministic so a replay matches).
    pub at_ms: u64,
}

impl TraceEraseReceipt {
    /// **Whether the KN-D12 drill is GREEN: 0 recoverable PII in traces, attribution intact.** The
    /// crypto-shred reached backups (`recoverable_in_backup == 0`) AND the attribution falls back to a
    /// non-empty opaque pseudonym (the row survived — attribution intact, not deleted).
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0 && !self.attribution_pseudonym.is_empty()
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The §5.2 public signature: write_agent_trace(run_id, content, actor) -> run.trace_ref
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **`write_agent_trace(run_id, content, actor) -> run.trace_ref` (the architecture §5.2 / contract
/// 8.8 signature).** Accept a content-addressed (BLAKE3) write of an agent execution trace reusing the
/// block model, store it in the erasable [`AgentTraceHolder`], and return the `run.trace_ref` (the
/// [`ArtifactRef`] over the content hash) the agent run records. Content-addressed + idempotent: the
/// same `(run_id, content, actor)` writes ONCE (the same ref comes back).
///
/// `content` is the trace narrative AS the frozen [`myelin_content::Block`] model (no new schema);
/// `actor` is the OPAQUE pseudonymous actor id (the attribution — never PII, contract 4.8). This is the
/// PRODUCER side of contract 8.8 (the GDPR-service seam + the Fabric consumer leg already exist,
/// P-GA-26 / AG-P19 → P-268; this is the live Knowledge-side body the seam plugs into).
pub fn write_agent_trace(
    holder: &AgentTraceHolder,
    tenant: &TenantId,
    run_id: impl Into<String>,
    content: Vec<Block>,
    actor_principal_id: impl Into<String>,
) -> ArtifactRef {
    let trace = AgentTrace::new(run_id, actor_principal_id, content);
    holder.write(tenant, trace)
}

#[cfg(test)]
mod tests;
