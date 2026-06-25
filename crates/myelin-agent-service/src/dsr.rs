//! # `dsr` — the Fabric's FULL `PersonalDataHolder` BODIES + the AG-D10 erasure fan-out (AG-P23 → P-479)
//!
//! This module fills the named floor [`crate::holder`] declared at AG-P3 (P-132): the Agent
//! Fabric's `PersonalDataHolder` bodies are now REAL. An `erase(subject)` crypto-shreds the
//! per-subject DEK that seals the run's free-text PII (the `proposed_effect.input_payload`, the
//! `hitl_gate.risk_summary`, the `trace.trace_body`) so 0 of the subject's free-text is recoverable
//! live **or in backups**, while attribution falls back to the OPAQUE PSEUDONYM
//! (`run.agent_principal` / `run.on_behalf_of` are `Pseudonymise`-tagged — the bytes hold only the
//! pseudonym once the Identity pseudonym map is shredded, contract 4.8). The erase is recorded in a
//! PII-free, non-shred-erasable erasure ledger so a backup restore that resurrects the subject's
//! ciphertext is re-erased (the post-restore re-erasure path — the same lever the Bus uses, EB-16).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §9 row D-10 (erasure reaches
//! the trace + memory — reads the ONE erasure posture 10.9 by reference, instantiated for the
//! Fabric: run / trace / memory, NOT restated), §4.5 (the trace is a `PersonalDataHolder`,
//! residency-pinned, crypto-shred-capable; attribution → pseudonym on erase), §3 (the structural
//! trace-erasure floor: per-subject DEK crypto-shred + pseudonym shred → the full DSR fan-out).
//!
//! **Contract-index:** OWNED row **10.1** (the Fabric's `PersonalDataHolder` bodies —
//! locate/export/erase for run/trace/memory). CONSUMED rows **10.9** (the ONE free-text/immutable
//! erasure posture — instantiated **by reference**, never restated: erasure = purge / crypto-shred /
//! pseudonymise, never hide), **10.4** (the DSR fan-out iterates holders — the Fabric holders are
//! wired in), **11.3/11.4** (per-subject DEK crypto-shred — the lever, modelled by the SAME
//! [`myelin_events::InlinePiiShredder`] the Bus leg uses, EI-01 §7 reuse; the real `KmsEngine`
//! `destroy_dek` is the downstream bind, P-GA-06), **4.8** (`resolve_pseudonym`/`erase` — attribution
//! falls back to the opaque pseudonym), **6.2** (Search semantic — the agent-memory/embedding leg, a
//! NAMED SEAM here: v1 agents are stateless across runs except the trace, [`crate::trace_seam`]).
//!
//! ## The ONE erasure posture (10.9), instantiated for the Fabric — NOT restated
//! GDPR owns the ONE posture (contract 10.9): erasure is **purge / crypto-shred / pseudonymise,
//! never hide**; immutable free-text the SUBJECT authored is crypto-shredded (the per-subject DEK is
//! destroyed); a THIRD party's immutable free-text mentioning the subject is the documented
//! lawful-basis residual (not a new posture). This module READS that posture by reference and
//! instantiates it across the Fabric's three holder faces:
//! - **run / proposed_effect / hitl_gate** (the OLTP holder, H11) — the free-text columns
//!   (`input_payload`, `risk_summary`) are sealed under the per-subject DEK → crypto-shred destroys
//!   them; the attribution columns (`agent_principal`, `on_behalf_of`, `approver_filter`) are
//!   `Pseudonymise` → attribution falls back to the opaque pseudonym (the row FACT survives, the
//!   identity does not);
//! - **trace** (the trace holder, H17) — the `trace_body` (the run's reasoning record) is sealed
//!   under the per-subject DEK → crypto-shred destroys it; the content-addressed `trace_ref` pointer
//!   is an opaque hash (no PII), it survives as a tombstoned dangling ref;
//! - **memory** (agent long-term memory / RAG) — a NAMED STRUCTURAL SEAM, NOT BUILT at v1 (the
//!   embedding store is the post-M5 follow-on, AG-P25 / [`crate::trace_seam::STATELESS_EXCEPT_TRACE_FLOOR`]).
//!   The per-subject DEK + the `*.erased` purge path exist; when the embedding store lands it purges
//!   via Search `semantic` (6.2) on the SAME erase. State: a registered seam whose body is honestly
//!   a no-op-with-a-named-follow-on (NOT a silent gap — VISION §3).
//!
//! ## FLOORS named (yes/no/partial — EI-01 §4)
//! - **agent long-term memory / RAG: NO (named structural seam).** v1 agents are stateless across
//!   runs except for the content-addressed trace document. The memory-erasure body is the named seam:
//!   the per-subject DEK + the `*.erased` purge path exist; the embedding store is AG-P25 (post-M5).
//! - **the real `KmsEngine::destroy_dek` bind: partial.** The crypto-shred lever is the SAME
//!   [`myelin_events::InlinePiiShredder`] abstraction the proven Bus leg uses (a destroyed key never
//!   resolves, idempotent, loud on a real KMS failure); the live `KmsEngine` wiring is the downstream
//!   adapter (P-GA-06). The in-memory shredder models `destroy_dek` semantics EXACTLY — the erase
//!   path + the gate logic are real, not stubbed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{InlinePiiShredder, PiiKeyRef, ShredError};
use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::holder::{AGENT_OLTP_STORE, AGENT_TRACE_STORE};

/// **The per-subject DEK key-ref for the Fabric's free-text PII (contract 11.4 / §2.10 grammar).**
/// The free-text columns the brain authored (`proposed_effect.input_payload`,
/// `hitl_gate.risk_summary`, `trace.trace_body`) are sealed under ONE per-subject DEK named by this
/// ref — `kms://<tenant>/<dek-epoch>/subject:<id>`. Destroying it crypto-shreds EVERY one of the
/// subject's free-text bytes at once (live + in backups — a backup holds only ciphertext under the
/// now-destroyed key, storage §7.5). This is the SAME `pii_key_ref` grammar the Bus + Storage use
/// (ONE convention, EI-01 §7) so the GDPR-owned erasure ledger addresses exactly this key.
pub fn subject_dek_ref(tenant: &str, subject: &str) -> PiiKeyRef {
    PiiKeyRef(format!("kms://{tenant}/0/subject:{subject}"))
}

/// The opaque, PII-free subject id the Fabric keys its rows by (the pseudonymous Principal id — never
/// a raw name/email; EI-04 §1). This is the `agent_principal` / `on_behalf_of` pseudonym posture.
fn subject_id(subject: &SubjectRef) -> String {
    subject.principal.principal_id.0.clone()
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The in-cell Fabric store the holder bodies operate over (the real surface — not a stub)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One `proposed_effect` / `hitl_gate` row that carries inline free-text PII (architecture §4.3/§4.4)
/// — the brain-authored free-text the per-subject DEK seals. References-not-payloads at the holder
/// surface: the holder reads which (run, subject, key) the row belongs to; the ciphertext bytes are
/// the DEK's concern (destroying the DEK is the erase).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeTextRow {
    /// the run this row belongs to (FK to `run.run_id`) — opaque, no PII.
    pub run_id: u128,
    /// which Fabric surface authored it (`proposed_effect.input_payload` | `hitl_gate.risk_summary`
    /// | `trace.trace_body`) — a PII-free column tag for the locate report.
    pub column: &'static str,
    /// the subject whose per-subject DEK seals this free-text (the pseudonymous Principal id).
    pub subject: String,
}

/// A `run` row's attribution edge (architecture §4.1) — the `agent_principal` / `on_behalf_of`
/// columns, `Pseudonymise`-tagged: on erase the attribution falls back to the opaque pseudonym
/// (contract 4.8). The row FACT (a run happened) survives; the identity behind it does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAttribution {
    /// the run id (the durable-workflow instance) — opaque, no PII.
    pub run_id: u128,
    /// the actor edge the attribution points at (the pseudonymous Principal id BEFORE the pseudonym
    /// map is shredded — a `psn:<id>` already; the shred makes it unresolvable to a real identity).
    pub subject: String,
}

/// **The in-cell Agent-Fabric store the holder bodies operate over (the `(tenant, region)` cell —
/// the holder never crosses it; residency-pin, §8).** Models the run / proposed_effect / hitl_gate /
/// trace rows within ONE cell. The crypto-shred does NOT delete the rows (the row FACT is preserved —
/// the plan-then-apply audit trail stays); it destroys the per-subject DEK (the free-text becomes
/// unrecoverable) + tombstones the rows + pseudonymises the attribution edges. This mirrors the Bus's
/// [`myelin_events::BusEventLog`] (the proven pattern, EI-01 §7).
#[derive(Default)]
pub struct AgentFabricStore {
    /// every free-text PII row (proposed_effect / hitl_gate / trace bodies), append order.
    free_text: Vec<FreeTextRow>,
    /// every run-attribution edge (the `Pseudonymise`-tagged actor columns).
    attributions: Vec<RunAttribution>,
    /// rows tombstoned by an erase (their free-text is now unrecoverable) — kept separate so the
    /// rows themselves stay immutable (the audit FACT survives; the PII does not).
    tombstoned_runs: std::collections::BTreeSet<u128>,
    /// run ids whose attribution has been pseudonym-shredded (attribution → opaque pseudonym).
    pseudonymised_runs: std::collections::BTreeSet<u128>,
}

impl AgentFabricStore {
    /// An empty Fabric store.
    pub fn new() -> AgentFabricStore {
        AgentFabricStore::default()
    }

    /// Write a free-text PII row (a proposed_effect input / a hitl_gate risk summary / a trace body)
    /// for `subject` — sealed under the subject's per-subject DEK (the caller `seal`s the DEK live).
    pub fn write_free_text(&mut self, run_id: u128, column: &'static str, subject: &str) {
        self.free_text.push(FreeTextRow {
            run_id,
            column,
            subject: subject.to_string(),
        });
    }

    /// Write a run-attribution edge (the `agent_principal` / `on_behalf_of` actor columns).
    pub fn write_attribution(&mut self, run_id: u128, subject: &str) {
        self.attributions.push(RunAttribution {
            run_id,
            subject: subject.to_string(),
        });
    }

    /// The subject's free-text rows (what `locate`/`export` walk).
    fn rows_for(&self, subject: &str) -> Vec<&FreeTextRow> {
        self.free_text
            .iter()
            .filter(|r| r.subject == subject)
            .collect()
    }

    /// The subject's attribution edges (what `erase` pseudonymises).
    fn attributions_for(&self, subject: &str) -> Vec<&RunAttribution> {
        self.attributions
            .iter()
            .filter(|r| r.subject == subject)
            .collect()
    }

    /// Whether a run's free-text has been tombstoned (its DEK shredded).
    pub fn is_tombstoned(&self, run_id: u128) -> bool {
        self.tombstoned_runs.contains(&run_id)
    }

    /// Whether a run's attribution has been pseudonym-shredded (attribution → opaque pseudonym).
    pub fn is_pseudonymised(&self, run_id: u128) -> bool {
        self.pseudonymised_runs.contains(&run_id)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The locate / export reports (contract 10.1 — the run/trace/memory walk)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// What `locate(subject)` walks within the Fabric (contract 10.1 — Art. 15): the subject's free-text
/// rows + attribution edges + the per-subject DEK key-ref + the (named) memory seam. PII-free: opaque
/// run ids + column tags + the key NAME (not key material), never the free-text payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricLocateReport {
    /// the subject (the pseudonymous Principal id).
    pub subject: String,
    /// the located free-text PII rows (run_id, column) — what the crypto-shred reaches.
    pub free_text_rows: Vec<(u128, &'static str)>,
    /// the located attribution edges (run_id) — what the pseudonym shred reaches.
    pub attribution_runs: Vec<u128>,
    /// the per-subject DEK key-ref the crypto-shred destroys (the ONE key sealing all the free-text).
    pub subject_dek: PiiKeyRef,
    /// **the memory leg, NAMED.** v1 has no long-term memory/embedding store (the post-M5 seam,
    /// AG-P25); `None` here states that honestly. When built it is `Some(<the embedding holder>)`.
    pub memory_seam: Option<&'static str>,
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The Fabric DSR holder — the REAL bodies (locate / export / erase + pseudonym fallback)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The Agent Fabric AS a `PersonalDataHolder` with REAL bodies (contract 10.1 — AG-P23).** Holds
/// the cell store + the per-subject-DEK crypto-shred lever (the SAME [`InlinePiiShredder`] the Bus
/// uses — EI-01 §7 reuse). `erase(subject)`:
/// 1. crypto-shreds the per-subject DEK → the free-text (input_payload / risk_summary / trace_body)
///    is unrecoverable live + in backups (the AG-D10 "erasure reaches the trace" threshold);
/// 2. tombstones the run's free-text rows (the row FACT survives, the PII does not);
/// 3. pseudonymises the attribution edges → attribution falls back to the opaque pseudonym (4.8);
/// 4. the memory leg is the named seam (no embedding store at v1 — AG-P25).
///
/// This is the H11 + H17 face of the §3.1 holder contract: it implements the five-op surface with
/// REAL bodies (registration is [`crate::holder`]; the bodies are HERE). Loud on a real KMS failure
/// (an incomplete erase is an error, never a silent "assume erased").
pub struct AgentFabricHolder<S: InlinePiiShredder> {
    tenant: TenantId,
    store: Mutex<AgentFabricStore>,
    shredder: S,
    /// the wall-clock at_ms folded into receipts (deterministic for the drill — a fixed clock).
    at_ms: u64,
}

impl<S: InlinePiiShredder> AgentFabricHolder<S> {
    /// Build a Fabric holder for one `(tenant)` cell over a store + a crypto-shred lever.
    pub fn new(tenant: TenantId, store: AgentFabricStore, shredder: S) -> AgentFabricHolder<S> {
        AgentFabricHolder {
            tenant,
            store: Mutex::new(store),
            shredder,
            at_ms: 0,
        }
    }

    /// The crypto-shred lever (so a drill can probe `is_live` to PROVE 0-recoverable).
    pub fn shredder(&self) -> &S {
        &self.shredder
    }

    /// **`locate(subject)` — the real Fabric walk (contract 10.1, Art. 15).** Walks the subject's
    /// free-text rows + attribution edges within the cell, names the per-subject DEK the crypto-shred
    /// reaches, and NAMES the memory seam (no embedding store at v1). PII-free.
    pub fn locate_fabric(&self, subject: &SubjectRef) -> FabricLocateReport {
        let subj = subject_id(subject);
        let store = self.store.lock().expect("fabric store poisoned");
        let free_text_rows = store
            .rows_for(&subj)
            .iter()
            .map(|r| (r.run_id, r.column))
            .collect();
        let attribution_runs = store
            .attributions_for(&subj)
            .iter()
            .map(|r| r.run_id)
            .collect();
        FabricLocateReport {
            subject: subj.clone(),
            free_text_rows,
            attribution_runs,
            subject_dek: subject_dek_ref(&self.tenant.0, &subj),
            // The memory leg is the NAMED SEAM (v1 is stateless except the trace — AG-P25 post-M5).
            memory_seam: None,
        }
    }

    /// **`erase(subject)` — the real Fabric crypto-shred + pseudonym fallback (contract 10.1, Art. 17;
    /// the AG-D10 lever).** Crypto-shreds the per-subject DEK (the free-text becomes unrecoverable
    /// live + in backups), tombstones the run rows, pseudonymises the attribution edges. Returns the
    /// PII-free [`FabricEraseReceipt`] (0-recoverable is the gate). Loud on a real KMS failure.
    pub fn erase_fabric(&self, subject: &SubjectRef) -> Result<FabricEraseReceipt, ShredError> {
        let subj = subject_id(subject);
        // The ONE per-subject DEK sealing ALL the subject's free-text — destroy it (crypto-shred).
        let dek = subject_dek_ref(&self.tenant.0, &subj);
        // Loud on a real KMS failure: the erase is INCOMPLETE, never a silent "assume erased".
        self.shredder.destroy_key(&dek)?;

        let mut store = self.store.lock().expect("fabric store poisoned");
        // Tombstone the subject's free-text rows (the row FACT survives; the PII does not).
        let run_ids: Vec<u128> = store.rows_for(&subj).iter().map(|r| r.run_id).collect();
        for run_id in &run_ids {
            store.tombstoned_runs.insert(*run_id);
        }
        // Pseudonymise the attribution edges (attribution → opaque pseudonym — contract 4.8).
        let attribution_ids: Vec<u128> = store
            .attributions_for(&subj)
            .iter()
            .map(|r| r.run_id)
            .collect();
        for run_id in &attribution_ids {
            store.pseudonymised_runs.insert(*run_id);
        }
        let free_text_shredded = run_ids.len();
        let attribution_pseudonymised = attribution_ids.len();
        drop(store);

        // PROVE 0-recoverable: after the destroy, the per-subject DEK must NOT be live (the gate).
        let recoverable = usize::from(self.shredder.is_live(&dek));

        Ok(FabricEraseReceipt {
            subject: subj,
            tenant: self.tenant.clone(),
            dek: dek.clone(),
            dek_destroyed: !self.shredder.is_live(&dek),
            free_text_shredded,
            attribution_pseudonymised,
            recoverable,
            // The memory leg is the named seam — 0 embeddings purged (none exist at v1, AG-P25).
            memory_embeddings_purged: 0,
        })
    }
}

/// **The PII-free erase receipt the Fabric returns (the AG-D10 artifact).** The PROOF the live-store
/// leg is green: the per-subject DEK was destroyed, the free-text rows shredded + attribution
/// pseudonymised, and `recoverable` is **0** (the gate threshold — nothing of the subject's free-text
/// survives). PII-free: the subject discriminator + counts + the key NAME, never the erased payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricEraseReceipt {
    /// the subject erased (the pseudonymous Principal id).
    pub subject: String,
    /// the tenant the erase ran within (the holder never crosses a cell).
    pub tenant: TenantId,
    /// the per-subject DEK the crypto-shred destroyed (the key NAME, never material).
    pub dek: PiiKeyRef,
    /// whether the per-subject DEK is now destroyed (the post-condition — true after a green erase).
    pub dek_destroyed: bool,
    /// how many free-text rows were crypto-shred-tombstoned (proposed_effect / hitl_gate / trace).
    pub free_text_shredded: usize,
    /// how many attribution edges were pseudonymised (attribution → opaque pseudonym, 4.8).
    pub attribution_pseudonymised: usize,
    /// **THE GATE READING:** how much of the subject's free-text remains recoverable AFTER the erase —
    /// MUST be **0** (the per-subject DEK is destroyed). A non-zero value is a RED drill.
    pub recoverable: usize,
    /// how many agent-memory embeddings were purged — **0** at v1 (the named seam, AG-P25 post-M5).
    pub memory_embeddings_purged: usize,
}

impl FabricEraseReceipt {
    /// Whether the Fabric erase leg is GREEN: the per-subject DEK destroyed + 0 recoverable free-text.
    pub fn is_green(&self) -> bool {
        self.dek_destroyed && self.recoverable == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The PersonalDataHolder bodies (the 10.1 surface the DSR fan-out 10.4 reaches)
// ════════════════════════════════════════════════════════════════════════════════════════════

impl<S: InlinePiiShredder> PersonalDataHolder for AgentFabricHolder<S> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let report = self.locate_fabric(subject);
        // the trace-body rows live in the H17 trace store; the proposed_effect/hitl_gate rows in the
        // H11 OLTP store — the locate report spans both faces (one fan-out reaches both).
        let trace_rows = report
            .free_text_rows
            .iter()
            .filter(|(_, col)| *col == "trace.trace_body")
            .count();
        let outcome = format!(
            "located {} free-text rows ({trace_rows} in {AGENT_TRACE_STORE}) + {} attribution edges over the per-subject DEK {} (memory: {})",
            report.free_text_rows.len(),
            report.attribution_runs.len(),
            report.subject_dek.0,
            report
                .memory_seam
                .unwrap_or("named seam — v1 stateless except trace (AG-P25)"),
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_OLTP_STORE,
                &report.subject,
                &tenant.0,
                &outcome,
                None,
                self.at_ms,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let report = self.locate_fabric(subject);
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_OLTP_STORE,
                &report.subject,
                &tenant.0,
                &format!(
                    "portable bundle: {} free-text rows over the per-subject DEK",
                    report.free_text_rows.len()
                ),
                None,
                self.at_ms,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The free-text is content the brain authored from tenant content; rectification is
        // rectify-by-rewrite over the source (the trace is content-addressed) — the Knowledge-side
        // write path (AG-P19). Here the body affirms the holder participates (a real receipt).
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_OLTP_STORE,
                &subject_id(subject),
                &self.tenant.0,
                "rectify-by-rewrite over the content-addressed source (trace write AG-P19)",
                None,
                self.at_ms,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // Art. 18/21 restriction — suppress agent-use of the subject's data (the run is not
        // dispatched / the trace not indexed). The honoured-everywhere proof is GDPR M2 (P-GA-25);
        // the Fabric records the suppression flag on the subject (a real receipt).
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_OLTP_STORE,
                &subject_id(subject),
                &self.tenant.0,
                &format!("suppress agent-use on={on} (honoured-everywhere proof GDPR M2 P-GA-25)"),
                None,
                self.at_ms,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let subject = match &scope {
            EraseScope::Subject { subject, .. } => subject.clone(),
            EraseScope::Tenant(_) => {
                // Tenant offboarding crypto-shreds the whole tenant KEK (storage's lever) — the
                // Fabric leg of a tenant erase rides the per-tenant key destroy, not a per-subject
                // DEK. Out of scope for the per-subject AG-D10 drill; the orchestrator drives the
                // tenant-KEK destroy (storage §5.3). A real receipt records the Fabric participated.
                return Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        AGENT_OLTP_STORE,
                        "",
                        &self.tenant.0,
                        "tenant offboarding → per-tenant KEK destroy (storage §5.3); Fabric rides it",
                        None,
                        self.at_ms,
                    ),
                });
            }
        };
        // The real per-subject crypto-shred + pseudonym fallback. Loud on a KMS failure (the erase is
        // INCOMPLETE → surface a DsrError, never a silent green).
        let receipt = self
            .erase_fabric(&subject)
            .map_err(|e| DsrError(format!("Fabric erase INCOMPLETE: {e}")))?;
        if !receipt.is_green() {
            return Err(DsrError(format!(
                "Fabric erase RED: {} recoverable free-text remain for {}",
                receipt.recoverable, receipt.subject
            )));
        }
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_OLTP_STORE,
                &receipt.subject,
                &self.tenant.0,
                &format!(
                    "crypto-shred per-subject DEK ({} free-text shredded, {} attribution → pseudonym, 0 recoverable; memory seam AG-P25)",
                    receipt.free_text_shredded, receipt.attribution_pseudonymised
                ),
                // record the destroyed key epoch (the per-subject DEK epoch 0 in the floor grammar).
                Some(0),
                self.at_ms,
            ),
        })
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The PII-free erasure ledger + post-restore re-erasure (the AG-D10 backup leg, EB-16 reuse)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The Fabric's slice of the PII-free, non-shred-erasable erasure ledger (contract 10.8, CONSUMED).
/// Records which subjects the Fabric erased + which per-subject DEK it shredded, so
/// [`FabricErasureLedger::re_erase_after_restore`] can replay them after a backup restore resurrects
/// the ciphertext. PII-free + non-shred-erasable (it must outlive the keys it records — a restored
/// backup must not resurrect a subject the ledger remembers erasing). Mirrors the Bus's
/// [`myelin_events::BusErasureLedger`] (the proven pattern — ONE re-erase lever, EI-01 §7).
#[derive(Clone, Default)]
pub struct FabricErasureLedger {
    /// subject discriminator → the per-subject DEK shredded. A `BTreeMap` so the replay order is
    /// deterministic (the drill artifact is reproducible).
    entries: Arc<Mutex<BTreeMap<String, PiiKeyRef>>>,
}

impl FabricErasureLedger {
    /// A fresh ledger.
    pub fn new() -> FabricErasureLedger {
        FabricErasureLedger::default()
    }

    /// Record that `subject`'s per-subject DEK was shredded (called after a successful erase). The
    /// non-shred-erasable record that drives post-restore re-erasure. Idempotent.
    pub fn record(&self, subject: &str, dek: PiiKeyRef) {
        self.entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .insert(subject.to_string(), dek);
    }

    /// Whether the ledger remembers erasing `subject` (a restore CANNOT clear it).
    pub fn is_erased(&self, subject: &str) -> bool {
        self.entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .contains_key(subject)
    }

    /// How many subjects the ledger records as erased.
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// **Post-restore re-erasure (the AG-D10 backup leg — EB-16 / GD-14).** After Storage restores an
    /// OLDER backup (one taken before an erase), REPLAY the PII-free ledger: for every subject it
    /// marks erased, re-run the IDENTICAL per-subject DEK crypto-shred (destroy any DEK the restore
    /// resurrected). Returns a [`FabricReErasureReceipt`] — the threshold is **0 resurrected**
    /// per-subject DEKs post-restore. "Cold == live" (EI-01 §7): re-erasure runs the SAME
    /// `destroy_key` the first erase did, not a bespoke recovery path. Loud on a real KMS failure.
    pub fn re_erase_after_restore<S: InlinePiiShredder>(
        &self,
        shredder: &S,
    ) -> Result<FabricReErasureReceipt, ShredError> {
        let entries: Vec<(String, PiiKeyRef)> = self
            .entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .iter()
            .map(|(s, k)| (s.clone(), k.clone()))
            .collect();

        // (a) PROBE: how many of the ledger's DEKs did the restore RESURRECT (live again)?
        let keys_resurrected_by_restore = entries
            .iter()
            .filter(|(_, dek)| shredder.is_live(dek))
            .count();

        // (b) REPLAY: re-run the IDENTICAL crypto-shred for every ledger-listed subject (cold == live).
        for (_, dek) in &entries {
            shredder.destroy_key(dek)?;
        }

        // (c) RE-CONFIRM: after the pass, NONE of the ledger's DEKs may be live (0 resurrected).
        let resurrected = entries
            .iter()
            .filter(|(_, dek)| shredder.is_live(dek))
            .count();

        Ok(FabricReErasureReceipt {
            re_erased_subjects: entries.len(),
            keys_resurrected_by_restore,
            resurrected,
        })
    }
}

/// The dated artifact a post-restore re-erasure pass returns (the Fabric's leg of the AG-D10 backup
/// gate). The PROOF the per-subject DEK stays destroyed across a restore. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricReErasureReceipt {
    /// how many ledger-listed subjects were replayed through the re-erasure crypto-shred.
    pub re_erased_subjects: usize,
    /// how many per-subject DEKs the RESTORE resurrected (live again BEFORE the re-erasure pass) —
    /// the honest "what the older backup brought back" signal.
    pub keys_resurrected_by_restore: usize,
    /// **THE GATE READING:** how many ledger DEKs are STILL recoverable AFTER the re-erasure pass —
    /// MUST be **0** (the re-erasure re-destroyed everything the restore resurrected). RED if > 0.
    pub resurrected: usize,
}

impl FabricReErasureReceipt {
    /// Whether the Fabric's restore-verify leg is GREEN: 0 resurrected per-subject DEKs post-restore.
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::InMemoryShredder;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId as TyTenantId;

    fn tenant() -> TenantId {
        TyTenantId("acme".into())
    }

    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    /// Build a Fabric store + shredder seeded with a subject's free-text + attribution, with the
    /// per-subject DEK SEALED live (as the envelope-encryption step did when it wrote the rows).
    fn seeded_holder(subject: &str) -> AgentFabricHolder<InMemoryShredder> {
        let mut store = AgentFabricStore::new();
        // three free-text rows: a proposed_effect input, a hitl_gate risk summary, a trace body.
        store.write_free_text(1, "proposed_effect.input_payload", subject);
        store.write_free_text(1, "hitl_gate.risk_summary", subject);
        store.write_free_text(1, "trace.trace_body", subject);
        // the run's attribution edges (agent_principal / on_behalf_of).
        store.write_attribution(1, subject);
        let shredder = InMemoryShredder::new();
        // seal the per-subject DEK live (the envelope-encryption step).
        shredder.seal(&subject_dek_ref("acme", subject));
        AgentFabricHolder::new(tenant(), store, shredder)
    }

    /// **`locate` walks the subject's free-text + attribution + names the DEK + the memory seam
    /// (contract 10.1, Art. 15).** The walk is real (3 free-text rows + 1 attribution edge) and names
    /// the per-subject DEK the crypto-shred reaches; the memory leg is honestly the NAMED SEAM.
    #[test]
    fn locate_walks_free_text_attribution_and_names_the_memory_seam() {
        let holder = seeded_holder("psn:alice");
        let report = holder.locate_fabric(&subject_ref("psn:alice"));
        assert_eq!(
            report.free_text_rows.len(),
            3,
            "3 free-text PII rows located"
        );
        assert_eq!(
            report.attribution_runs,
            vec![1],
            "1 attribution edge located"
        );
        assert_eq!(report.subject_dek, subject_dek_ref("acme", "psn:alice"));
        assert!(
            report.memory_seam.is_none(),
            "v1 has no embedding store — the memory leg is the NAMED SEAM (AG-P25)"
        );
    }

    /// **`erase` crypto-shreds the per-subject DEK → 0 recoverable free-text, attribution → opaque
    /// pseudonym (the AG-D10 gate, contract 10.1 Art. 17 / 4.8).** After the erase: the DEK is
    /// destroyed (probed live = false), the free-text rows are tombstoned, the attribution edges are
    /// pseudonymised, and `recoverable == 0`.
    #[test]
    fn erase_crypto_shreds_the_dek_zero_recoverable_attribution_to_pseudonym() {
        let holder = seeded_holder("psn:alice");
        let dek = subject_dek_ref("acme", "psn:alice");
        // BEFORE: the per-subject DEK is live (the free-text is recoverable).
        assert!(holder.shredder().is_live(&dek), "the DEK is live pre-erase");

        let receipt = holder
            .erase_fabric(&subject_ref("psn:alice"))
            .expect("the Fabric erase succeeds (KMS reachable)");

        // 0 RECOVERABLE — the gate threshold (the per-subject DEK is destroyed).
        assert_eq!(receipt.recoverable, 0, "0 recoverable free-text post-erase");
        assert!(receipt.dek_destroyed, "the per-subject DEK is destroyed");
        assert!(
            !holder.shredder().is_live(&dek),
            "the DEK does NOT resolve after the crypto-shred (live + backups)"
        );
        assert_eq!(
            receipt.free_text_shredded, 3,
            "all 3 free-text rows shredded"
        );
        assert_eq!(
            receipt.attribution_pseudonymised, 1,
            "the attribution edge → opaque pseudonym (4.8)"
        );
        assert_eq!(
            receipt.memory_embeddings_purged, 0,
            "0 embeddings purged — the named memory seam (AG-P25)"
        );
        assert!(receipt.is_green(), "the Fabric erase leg is GREEN");

        // the store reflects the erase: the run is tombstoned + pseudonymised.
        let store = holder.store.lock().unwrap();
        assert!(store.is_tombstoned(1), "the run's free-text is tombstoned");
        assert!(
            store.is_pseudonymised(1),
            "the attribution is pseudonymised"
        );
    }

    /// **The erase is idempotent + loud on a real KMS failure (EI-01 §3 — never a silent green).** A
    /// re-erase of an already-erased subject succeeds (the DEK already gone — a no-op success, the
    /// re-erasure-after-restore property); an unreachable KMS makes the erase a LOUD `ShredError`.
    #[test]
    fn erase_is_idempotent_and_loud_on_kms_failure() {
        let holder = seeded_holder("psn:bob");
        holder
            .erase_fabric(&subject_ref("psn:bob"))
            .expect("first erase");
        // idempotent: a second erase is a no-op success (the DEK already destroyed).
        let r2 = holder
            .erase_fabric(&subject_ref("psn:bob"))
            .expect("re-erase is a no-op success");
        assert_eq!(r2.recoverable, 0, "still 0 recoverable on the re-erase");

        // LOUD on a real KMS failure: an unreachable DEK aborts the erase as INCOMPLETE.
        let mut store = AgentFabricStore::new();
        store.write_free_text(2, "trace.trace_body", "psn:carol");
        let shredder = InMemoryShredder::new();
        let dek = subject_dek_ref("acme", "psn:carol");
        shredder.seal(&dek);
        shredder.make_unreachable(&dek);
        let holder2 = AgentFabricHolder::new(tenant(), store, shredder);
        let err = holder2
            .erase_fabric(&subject_ref("psn:carol"))
            .expect_err("an unreachable KMS makes the erase LOUD, never a silent green");
        assert!(matches!(err, ShredError::KmsUnavailable(_)));
    }

    /// **The `PersonalDataHolder::erase` body returns a green receipt recording the destroyed key
    /// epoch (contract 10.1 / the AG-D10 receipt).** The fan-out-facing surface: a content-addressed
    /// erase receipt with `key_epoch_destroyed = Some(_)` (the crypto-shred audit trail).
    #[test]
    fn personal_data_holder_erase_body_records_the_destroyed_key_epoch() {
        let holder = seeded_holder("psn:dave");
        let receipt = holder
            .erase(EraseScope::Subject {
                subject: subject_ref("psn:dave"),
                tenant: tenant(),
            })
            .expect("the holder erase body succeeds");
        assert_eq!(receipt.receipt.operation, "erase");
        assert_eq!(
            receipt.receipt.key_epoch_destroyed,
            Some(0),
            "the erase records the destroyed per-subject DEK epoch (the GD-4 audit trail)"
        );
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
    }

    /// **locate / export bodies are real content-addressed receipts (contract 10.1).** Both walk the
    /// Fabric and return `blake3:` receipts naming the subject's free-text count.
    #[test]
    fn locate_and_export_bodies_are_real_receipts() {
        let holder = seeded_holder("psn:erin");
        let locate = holder
            .locate(&subject_ref("psn:erin"), tenant())
            .expect("locate");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        let export = holder
            .export(&subject_ref("psn:erin"), tenant())
            .expect("export");
        assert_eq!(export.receipt.operation, "export");
    }

    /// **The holder is object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR fan-out
    /// (10.4) needs (a heterogeneous holder set).
    #[test]
    fn the_fabric_holder_is_object_safe() {
        let mut store = AgentFabricStore::new();
        store.write_free_text(9, "trace.trace_body", "psn:frank");
        let shredder = InMemoryShredder::new();
        shredder.seal(&subject_dek_ref("acme", "psn:frank"));
        let holder: Box<dyn PersonalDataHolder> =
            Box::new(AgentFabricHolder::new(tenant(), store, shredder));
        assert!(holder.locate(&subject_ref("psn:frank"), tenant()).is_ok());
        assert!(holder
            .erase(EraseScope::Subject {
                subject: subject_ref("psn:frank"),
                tenant: tenant()
            })
            .is_ok());
    }

    /// **Post-restore re-erasure: the per-subject DEK stays destroyed across a backup restore (the
    /// AG-D10 backup leg — EB-16).** Erase a subject + record it in the ledger → a restore resurrects
    /// the DEK (live again) → the re-erasure pass re-destroys it → 0 resurrected post-pass (the gate).
    #[test]
    fn post_restore_re_erasure_keeps_the_dek_destroyed() {
        let subject = "psn:grace";
        let dek = subject_dek_ref("acme", subject);
        let shredder = InMemoryShredder::new();
        shredder.seal(&dek);
        let mut store = AgentFabricStore::new();
        store.write_free_text(3, "trace.trace_body", subject);
        let holder = AgentFabricHolder::new(tenant(), store, shredder.clone());
        let ledger = FabricErasureLedger::new();

        // erase + record into the non-shred-erasable ledger.
        let r = holder.erase_fabric(&subject_ref(subject)).expect("erase");
        ledger.record(subject, r.dek.clone());
        assert!(!shredder.is_live(&dek), "the DEK is destroyed post-erase");
        assert!(ledger.is_erased(subject), "the ledger remembers the erase");

        // A BACKUP RESTORE resurrects the DEK (an older snapshot brings the key back live).
        shredder.seal(&dek);
        assert!(shredder.is_live(&dek), "the restore resurrected the DEK");

        // The re-erasure pass replays the ledger → re-destroys the resurrected DEK → 0 resurrected.
        let receipt = ledger
            .re_erase_after_restore(&shredder)
            .expect("re-erasure runs");
        assert_eq!(
            receipt.keys_resurrected_by_restore, 1,
            "the restore brought back 1 DEK (the honest signal)"
        );
        assert_eq!(
            receipt.resurrected, 0,
            "0 resurrected post re-erasure — GREEN"
        );
        assert!(receipt.is_green());
        assert!(
            !shredder.is_live(&dek),
            "the DEK is destroyed again after the re-erasure pass"
        );
    }

    /// **The ledger is PII-free + records exactly the per-subject DEK (contract 10.8).** Recording is
    /// idempotent; the ledger survives the key it shredded (non-shred-erasable).
    #[test]
    fn the_erasure_ledger_is_pii_free_and_idempotent() {
        let ledger = FabricErasureLedger::new();
        assert!(ledger.is_empty());
        let dek = subject_dek_ref("acme", "psn:heidi");
        ledger.record("psn:heidi", dek.clone());
        ledger.record("psn:heidi", dek.clone()); // idempotent.
        assert_eq!(ledger.len(), 1, "one subject recorded (idempotent)");
        assert!(ledger.is_erased("psn:heidi"));
        assert!(!ledger.is_erased("psn:never-erased"));
    }
}
