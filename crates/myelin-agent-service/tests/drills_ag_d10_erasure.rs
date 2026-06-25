//! # AG-D10 — Erasure reaches the trace + agent memory (AG-P23 → P-479): the end-to-end drill
//!
//! **Drill (catalogue row AG-D10):** erase a subject → the run trace + the agent memory/embeddings
//! are crypto-shredded/purged; attribution → an opaque pseudonym; **0 recoverable PII** (including in
//! backups, via the post-restore re-erasure path). The gate threshold (**0 recoverable**) is NEVER
//! softened. Reads the ONE erasure posture (contract 10.9) **by reference** — erasure = purge /
//! crypto-shred / pseudonymise, never hide — instantiated for the Fabric's run / trace / memory.
//!
//! **What this drill proves (the dated green artifact):**
//! 1. **write** a run + a proposed_effect input + a hitl_gate risk summary + a trace body + an
//!    attribution edge for a subject, with the per-subject DEK SEALED live (the envelope-encryption
//!    step) — the free-text is recoverable PRE-erase;
//! 2. **erase(subject)** → the per-subject DEK is crypto-shredded → 0 of the subject's free-text is
//!    recoverable (the trace body, the proposed_effect input, the hitl_gate risk summary all become
//!    unrecoverable); the attribution edge falls back to the OPAQUE PSEUDONYM (4.8); the memory leg
//!    is the named seam (0 embeddings — v1 stateless except the trace, AG-P25);
//! 3. **post-restore re-erasure** → a backup restore resurrects the per-subject DEK → the re-erasure
//!    pass replays the PII-free ledger → 0 resurrected DEKs post-pass (the key stays destroyed across
//!    a restore — the backup half of the 0-recoverable threshold).
//!
//! The agent-memory/embedding leg is the NAMED FLOOR (EI-04 §1 / VISION §3): v1 agents are stateless
//! across runs EXCEPT for the content-addressed trace document (a Knowledge doc, erasable). The
//! per-subject DEK + the `*.erased` purge path exist; the embedding store is the post-M5 follow-on
//! (AG-P25, indexing via Search `semantic` 6.2, purged on `*.erased`). State: a registered seam whose
//! body is honestly a no-op-with-a-named-follow-on — NOT a silent gap.

use myelin_agent_service::{
    subject_dek_ref, AgentFabricHolder, AgentFabricStore, FabricErasureLedger,
};
use myelin_events::{InMemoryShredder, InlinePiiShredder};
use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
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

/// Seed a Fabric store for `subject`: a run with a proposed_effect input, a hitl_gate risk summary, a
/// trace body, and an attribution edge — the per-subject DEK sealed live (the envelope-encryption
/// step). Returns the holder + the per-subject DEK ref (so the drill probes recoverability).
fn seed(subject: &str) -> AgentFabricHolder<InMemoryShredder> {
    let mut store = AgentFabricStore::new();
    store.write_free_text(7, "proposed_effect.input_payload", subject);
    store.write_free_text(7, "hitl_gate.risk_summary", subject);
    store.write_free_text(7, "trace.trace_body", subject);
    store.write_attribution(7, subject);
    let shredder = InMemoryShredder::new();
    shredder.seal(&subject_dek_ref("acme", subject)); // the DEK is live (free-text recoverable).
    AgentFabricHolder::new(tenant(), store, shredder)
}

/// **AG-D10 (the full drill): erase reaches the run/trace/memory — 0 recoverable PII, attribution →
/// pseudonym, including post-restore.** The single end-to-end artifact the gate reads.
#[test]
fn ag_d10_erasure_reaches_the_trace_and_memory_zero_recoverable() {
    let subject = "psn:alice";
    let dek = subject_dek_ref("acme", subject);
    let holder = seed(subject);

    // ── PRE-erase: the free-text is recoverable (the per-subject DEK is live). ──
    assert!(
        holder.shredder().is_live(&dek),
        "PRE-erase the per-subject DEK is live — the free-text is recoverable"
    );
    let pre = holder.locate_fabric(&subject_ref(subject));
    assert_eq!(
        pre.free_text_rows.len(),
        3,
        "3 free-text PII rows (proposed_effect / hitl_gate / trace) located pre-erase"
    );
    assert!(
        pre.memory_seam.is_none(),
        "the memory leg is the NAMED SEAM — v1 has no embedding store (AG-P25)"
    );

    // ── ERASE: crypto-shred the per-subject DEK → 0 recoverable + attribution → pseudonym. ──
    let receipt = holder
        .erase_fabric(&subject_ref(subject))
        .expect("the Fabric erase succeeds (KMS reachable)");

    // THE GATE: 0 recoverable PII (the per-subject DEK is destroyed — live AND backups).
    assert_eq!(
        receipt.recoverable, 0,
        "0 recoverable PII post-erase — THE GATE"
    );
    assert!(
        !holder.shredder().is_live(&dek),
        "the per-subject DEK does NOT resolve after the crypto-shred — the trace body, the \
         proposed_effect input, and the hitl_gate risk summary are all unrecoverable"
    );
    assert!(receipt.dek_destroyed, "the per-subject DEK was destroyed");
    assert_eq!(
        receipt.free_text_shredded, 3,
        "all 3 free-text rows crypto-shred-tombstoned (the trace reached too)"
    );
    // attribution → opaque pseudonym (contract 4.8) — the row FACT survives, the identity does not.
    assert_eq!(
        receipt.attribution_pseudonymised, 1,
        "the attribution edge falls back to the opaque pseudonym (4.8)"
    );
    // the memory leg is the named seam — 0 embeddings (none exist at v1).
    assert_eq!(
        receipt.memory_embeddings_purged, 0,
        "0 embeddings purged — the named memory seam (AG-P25 post-M5)"
    );
    assert!(receipt.is_green(), "the AG-D10 live-store leg is GREEN");

    // the holder erase BODY (the fan-out face) records the destroyed key epoch (the GD-4 audit trail).
    let body = holder
        .erase(myelin_gdpr::EraseScope::Subject {
            subject: subject_ref(subject),
            tenant: tenant(),
        })
        .expect("the holder erase body succeeds (idempotent re-erase)");
    assert_eq!(
        body.receipt.key_epoch_destroyed,
        Some(0),
        "the erase receipt records the destroyed per-subject DEK epoch"
    );

    // ── POST-RESTORE RE-ERASURE: the key stays destroyed across a backup restore. ──
    let ledger = FabricErasureLedger::new();
    ledger.record(subject, receipt.dek.clone());
    assert!(
        ledger.is_erased(subject),
        "the PII-free ledger remembers the erase"
    );

    // A BACKUP RESTORE resurrects the per-subject DEK (an older snapshot brings the key back live).
    holder.shredder().seal(&dek);
    assert!(
        holder.shredder().is_live(&dek),
        "the restore resurrected the per-subject DEK (the older backup brought it back)"
    );

    // The re-erasure pass replays the ledger → re-destroys the resurrected DEK → 0 resurrected.
    let re = ledger
        .re_erase_after_restore(holder.shredder())
        .expect("the re-erasure pass runs (cold == live)");
    assert_eq!(
        re.keys_resurrected_by_restore, 1,
        "the restore brought back 1 per-subject DEK (the honest signal)"
    );
    assert_eq!(
        re.resurrected, 0,
        "0 resurrected per-subject DEKs post re-erasure — THE BACKUP GATE"
    );
    assert!(re.is_green(), "the AG-D10 backup leg is GREEN");
    assert!(
        !holder.shredder().is_live(&dek),
        "the per-subject DEK is destroyed AGAIN after the re-erasure pass — 0 recoverable in backups"
    );
}

/// **The erase is loud on a real KMS failure (EI-01 §3 — never a silent false green).** An
/// unreachable KMS aborts the erase as INCOMPLETE (a `DsrError`), so a DSR never reports "erased"
/// over a trace it could not crypto-shred.
#[test]
fn ag_d10_erase_is_loud_on_an_incomplete_crypto_shred() {
    let subject = "psn:bob";
    let mut store = AgentFabricStore::new();
    store.write_free_text(1, "trace.trace_body", subject);
    let shredder = InMemoryShredder::new();
    let dek = subject_dek_ref("acme", subject);
    shredder.seal(&dek);
    shredder.make_unreachable(&dek); // the KMS cannot reach the key.
    let holder = AgentFabricHolder::new(tenant(), store, shredder);

    let err = holder
        .erase(myelin_gdpr::EraseScope::Subject {
            subject: subject_ref(subject),
            tenant: tenant(),
        })
        .expect_err("an unreachable KMS makes the erase LOUD, never a silent green");
    assert!(
        err.0.contains("INCOMPLETE"),
        "the erase names itself INCOMPLETE: {}",
        err.0
    );
}
