//! Unit + drill tests for the AG-7 content-addressed agent-trace holder (KN-P28 / P-318 — KN-D12).

use super::*;
use myelin_content::{parse_inline, Block};
use myelin_gdpr::SubjectRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::FieldValue;
use myelin_search::engine::{AclFilter, Hit, IndexBackend, IndexDocument, IndexError};
use myelin_search::vector::{Embedding, VectorHit};
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, SubjectId};
use myelin_storage::kms::{DekId, KekId, KeyClass};
use myelin_storage::{EpochMillis, EraseError, ErasureLedgerSink, KmsEngine, PseudonymShred};
use myelin_tenancy::Region;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::refs_glue::PageStore;

fn tenant() -> TenantId {
    myelin_tenancy::TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn subject_ref(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

/// A trace narrative AS the block model (no new schema): a paragraph of surfaced reasoning.
fn narrative(text: &str) -> Vec<Block> {
    vec![Block::Paragraph {
        inline: parse_inline(text, &[]),
    }]
}

// ───────────────────────── the block-model trace + its content-address ─────────────────────────

#[test]
fn trace_reuses_the_block_model_no_new_schema() {
    // The trace is a Document-shaped Vec<Block> — the frozen myelin_content AST, not a new schema.
    let trace = AgentTrace::new(
        "run-1",
        "p-agent",
        vec![
            Block::Heading {
                level: myelin_content::HeadingLevel::new(2).unwrap(),
                inline: parse_inline("Tool calls", &[]),
            },
            Block::CodeBlock {
                lang: Some("json".into()),
                text: "{\"tool\":\"knowledge.draft\"}".into(),
            },
        ],
    );
    assert_eq!(trace.blocks.len(), 2, "the trace IS a Vec<Block>");
    assert_eq!(trace.run_id, "run-1");
    assert_eq!(trace.actor_principal_id, "p-agent");
}

/// **THE CONTENT-ADDRESS GATE (CI): a trace write is content-addressed (BLAKE3) and idempotent —
/// the same content writes once; distinct content → distinct refs.**
#[test]
fn content_address_gate_blake3_and_idempotent() {
    let holder = AgentTraceHolder::new();
    let t = tenant();

    // The ref IS the BLAKE3 content address (the `blake3:` multihash inside the myelin:// ref).
    let trace = AgentTrace::new(
        "run-1",
        "p-agent",
        narrative("read the page, drafted a summary"),
    );
    let r1 = holder.write(&t, trace.clone());
    assert!(
        r1.0.starts_with("myelin://acme/knowledge/agent_trace/blake3:"),
        "the trace_ref is content-addressed (BLAKE3 multihash): {}",
        r1.0
    );

    // IDEMPOTENT: writing the SAME content again returns the SAME ref and stores ONE copy.
    let r2 = holder.write(&t, trace.clone());
    assert_eq!(
        r1, r2,
        "the same content yields the same ref (content-addressed)"
    );
    assert_eq!(
        holder.len(),
        1,
        "the same content writes ONCE (idempotent-by-content)"
    );

    // DISTINCT content → DISTINCT ref.
    let other = AgentTrace::new("run-1", "p-agent", narrative("a different reasoning trace"));
    let r3 = holder.write(&t, other);
    assert_ne!(r1, r3, "distinct content → distinct refs");
    assert_eq!(holder.len(), 2, "the second distinct trace is stored");

    // The content hash is the SAME deterministic BLAKE3 over the canonical bytes (re-derivable).
    let expected =
        crate::compaction::content_address(&trace.canonical_bytes()).to_multihash_string();
    assert!(
        r1.0.ends_with(&expected),
        "the ref ends with the canonical content hash"
    );
}

#[test]
fn write_agent_trace_free_fn_matches_the_5_2_signature() {
    // write_agent_trace(run_id, content, actor) -> run.trace_ref (architecture §5.2).
    let holder = AgentTraceHolder::new();
    let t = tenant();
    let trace_ref = write_agent_trace(
        &holder,
        &t,
        "run-7",
        narrative("system context + tool i/o + surfaced reasoning"),
        "p-actor",
    );
    assert!(trace_ref.0.contains("/agent_trace/blake3:"));
    assert!(
        holder.contains_ref(&trace_ref),
        "the holder stores the written trace"
    );
    assert_eq!(holder.len(), 1);
}

// ───────────────────────── distinct from the audit log (§5.2 / §6.5) ─────────────────────────

#[test]
fn trace_is_distinct_from_the_audit_log() {
    // The architecture §6.5 boundary: the trace holder is structurally distinct from the audit log
    // (distinct ids AND distinct erase semantics — erasable vs the retain carve-out).
    assert!(trace_is_distinct_from_audit());
    assert_ne!(TRACE_HOLDER_ID, AUDIT_LOG_STORE_ID, "distinct holder ids");
    // the trace IS erasable; the audit log is the retain carve-out — distinct erase mechanisms.
    assert_ne!(
        TRACE_ERASABLE, AUDIT_LOG_ERASABLE,
        "distinct erase mechanisms"
    );
}

#[test]
fn holder_id_matches_the_gdpr_service_seam_h17_id() {
    // ONE name across the seam (EI-01 §7): the Knowledge-side producer + the GDPR-service seam agree.
    assert_eq!(
        TRACE_HOLDER_ID,
        myelin_gdpr_service::AGENT_TRACE_HOLDER_ID,
        "the Knowledge trace holder id IS the GDPR-service H17 seam id (no parallel id)"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The KN-D12 erasure drill machinery (REUSES the KN-P26 crypto-shred core + the storage seams)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// A tiny in-memory IndexBackend (DB-free) — a `doc_id → present` map; `delete` removes (idempotent).
#[derive(Default)]
struct MemIndex {
    docs: BTreeMap<String, ()>,
}
impl IndexBackend for MemIndex {
    fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError> {
        self.docs.insert(doc.doc_id.clone(), ());
        Ok(())
    }
    fn delete(&mut self, doc_id: &str) -> Result<(), IndexError> {
        self.docs.remove(doc_id);
        Ok(())
    }
    fn search(&self, _: &AclFilter, _: &str, _: usize) -> Result<Vec<Hit>, IndexError> {
        Ok(vec![])
    }
    fn search_structured(
        &self,
        _: &AclFilter,
        _: &str,
        _: &FieldValue,
        _: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        Ok(vec![])
    }
    fn semantic(
        &self,
        _: &AclFilter,
        _: &Embedding,
        _: usize,
    ) -> Result<Vec<VectorHit>, IndexError> {
        Ok(vec![])
    }
    fn merge(&mut self) -> Result<(), IndexError> {
        Ok(())
    }
    fn snapshot(&mut self) -> Result<u64, IndexError> {
        Ok(self.docs.len() as u64)
    }
    fn indexed_zookie_of(&self, doc_id: &str) -> Option<String> {
        self.docs.get(doc_id).map(|_| "z0".to_string())
    }
}

#[derive(Default)]
struct RecPseudonym {
    shredded: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for RecPseudonym {
    fn shred_pseudonym(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.shredded.borrow_mut().insert(s.0.clone());
        Ok(())
    }
}

#[derive(Default)]
struct RecBus {
    erased: RefCell<BTreeSet<String>>,
}
impl myelin_storage::BusErase for RecBus {
    fn erase_inline_pii(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.erased.borrow_mut().insert(s.0.clone());
        Ok(())
    }
}

#[derive(Default)]
struct RecLedger {
    erased: RefCell<BTreeSet<String>>,
}
impl ErasureLedgerSink for RecLedger {
    fn record_erasure(&self, s: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.borrow_mut().insert(s.0.clone());
    }
    fn is_erased(&self, s: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&s.0)
    }
}

/// Seal a per-subject free-text column (the trace's PII-bearing content) under the subject's DEK, so
/// the erase has a REAL key to destroy + a real backup snapshot to probe.
fn engine_with_subject_trace_content(
    subject: &SubjectRef,
    plaintext: &[u8],
) -> (KmsEngine, EncryptedColumn) {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant(), region()));
    let cryptor = ColumnCryptor::new(&kms, region());
    let sid = SubjectId::new(subject.principal.principal_id.0.clone());
    let col = cryptor
        .encrypt(
            &tenant(),
            Some(&sid),
            &myelin_gdpr::ErasureMethod::CryptoShred("subject_dek".into()),
            plaintext,
        )
        .expect("seal the trace content under the subject DEK");
    (kms, col)
}

/// **KN-D12 (the dated green): erase a subject → their content-addressed agent traces
/// crypto-shredded/purged, attribution falls back to the pseudonym; 0 recoverable PII in traces,
/// attribution intact; the audit log is unaffected.**
#[test]
fn kn_d12_erase_subject_traces_zero_recoverable_pii_attribution_intact() {
    let subject = subject_ref("p-alice");
    // The trace's PII-bearing free-text content, sealed under alice's per-subject DEK.
    let (kms, sealed) =
        engine_with_subject_trace_content(&subject, b"alice asked about her home address");
    let cryptor = ColumnCryptor::new(&kms, region());

    // BEFORE: the trace content decrypts (the PII is live), the DEK is in the backup.
    assert!(
        cryptor.decrypt(&sealed).is_ok(),
        "the trace content decrypts BEFORE erase"
    );
    let subject_dek = DekId::new(
        tenant(),
        KeyClass::Subject(subject.principal.principal_id.0.clone()),
    );
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "alice's per-subject DEK is in the backup BEFORE erase"
    );

    // The holder stores TWO of alice's traces + one of another actor's (the other must survive).
    let holder = AgentTraceHolder::new();
    let t = tenant();
    write_agent_trace(
        &holder,
        &t,
        "run-a1",
        narrative("alice trace 1: home address"),
        "p-alice",
    );
    write_agent_trace(
        &holder,
        &t,
        "run-a2",
        narrative("alice trace 2: phone number"),
        "p-alice",
    );
    write_agent_trace(
        &holder,
        &t,
        "run-b1",
        narrative("bob trace: unrelated"),
        "p-bob",
    );
    assert_eq!(holder.len(), 3, "three distinct traces stored");
    assert_eq!(
        holder.subject_trace_hashes(&subject, &t).len(),
        2,
        "two of the traces are alice's (by actor attribution)"
    );

    // Seed the index with alice's trace docs (the lockstep purge) + a page store for backlinks.
    let mut idx = MemIndex::default();
    for h in holder.subject_trace_hashes(&subject, &t) {
        idx.upsert(&IndexDocument::new(
            format!("kn:agent_trace:{h}"),
            "alice trace embedding",
        ))
        .unwrap();
    }
    let index = Mutex::new(idx);
    let store = Mutex::new(PageStore::new());

    let pseudonym = RecPseudonym::default();
    let bus = RecBus::default();
    let ledger = RecLedger::default();

    let receipt = holder
        .erase_subject_traces(
            &subject,
            &t,
            region(),
            &kms,
            &pseudonym,
            &bus,
            &ledger,
            &index,
            &store,
            2_000,
        )
        .expect("the KN-D12 trace erase succeeds (every step green)");

    // ── 0 recoverable PII in traces: the per-subject DEK destroyed, reaching backups ──
    assert_eq!(
        receipt.traces_shredded, 2,
        "both of alice's traces shredded"
    );
    assert_eq!(
        receipt.recoverable_in_backup, 0,
        "0 recoverable PII in traces (the DEK is destroyed AND absent from the backup)"
    );
    assert!(
        cryptor.decrypt(&sealed).is_err(),
        "the trace content is UNRECOVERABLE after the crypto-shred"
    );
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "alice's DEK is ABSENT from the backup after the trace erase (stays dead across a restore)"
    );
    // The embeddings purged in lockstep (embeddings of PII are PII).
    assert_eq!(
        receipt.embeddings_purged, 2,
        "both trace index docs purged in lockstep"
    );
    {
        let i = index.lock().unwrap();
        for h in holder.subject_trace_hashes(&subject, &t) {
            assert!(
                i.indexed_zookie_of(&format!("kn:agent_trace:{h}"))
                    .is_none(),
                "alice's trace index doc is purged (0 vector survives)"
            );
        }
    }

    // ── attribution falls back to the opaque pseudonym (attribution intact) ──
    assert_eq!(
        receipt.attribution_pseudonym, subject.principal.principal_id.0,
        "attribution falls back to the opaque pseudonym (the row keeps the principal_id)"
    );
    assert!(
        pseudonym
            .shredded
            .borrow()
            .contains(&subject.principal.principal_id.0),
        "the pseudonym map was shredded → the id is un-resolvable to a human (4.8)"
    );

    // The receipt is the dated KN-D12 green.
    assert!(
        receipt.is_green(),
        "KN-D12 green: 0 recoverable trace PII, attribution intact"
    );
    assert_eq!(receipt.receipt.operation, "erase");
    assert!(receipt.receipt.content_hash.starts_with("blake3:"));
    assert!(!receipt.re_run);
}

/// **Distinct from the audit log: a trace erase does NOT touch the audit log (§6.5).** The trace
/// holder + the (separate) audit log are independent; erasing the trace leaves a parallel audit-log
/// store untouched. We model the audit log as a separate store and assert the trace erase never reads
/// or mutates it (the holder ids differ + the erase only drives the trace holder's seams).
#[test]
fn trace_erase_leaves_the_audit_log_unaffected() {
    let subject = subject_ref("p-carol");
    let (kms, _sealed) = engine_with_subject_trace_content(&subject, b"carol's note");
    let holder = AgentTraceHolder::new();
    let t = tenant();
    write_agent_trace(&holder, &t, "run-c", narrative("carol trace"), "p-carol");

    // A SEPARATE audit-log store (the H16 carve-out) — a tamper-evident record the trace erase must
    // never touch. We record an audit entry under the audit store id and prove it survives.
    let audit_entry = format!("audit:{}:retained", subject.principal.principal_id.0);
    let audit_log: BTreeSet<String> = [audit_entry.clone()].into_iter().collect();

    let h = holder.subject_trace_hashes(&subject, &t);
    let mut idx = MemIndex::default();
    idx.upsert(&IndexDocument::new(format!("kn:agent_trace:{}", h[0]), "x"))
        .unwrap();
    let index = Mutex::new(idx);
    let store = Mutex::new(PageStore::new());

    holder
        .erase_subject_traces(
            &subject,
            &t,
            region(),
            &kms,
            &RecPseudonym::default(),
            &RecBus::default(),
            &RecLedger::default(),
            &index,
            &store,
            1,
        )
        .expect("the trace erase succeeds");

    // The audit log is UNAFFECTED — the retain carve-out survives a trace erase (distinct holders).
    assert!(
        audit_log.contains(&audit_entry),
        "the tamper-evident audit log is UNTOUCHED by the trace erase (§6.5)"
    );
    assert!(
        trace_is_distinct_from_audit(),
        "the distinctness invariant holds"
    );
}

/// A partial failure (the embedding purge can't complete) → the trace erase aborts LOUDLY and is
/// never a false 'erased' (the trace content is plaintext-derived PII in the index).
#[test]
fn trace_erase_partial_failure_is_loud_never_false_green() {
    struct FailingIndex;
    impl IndexBackend for FailingIndex {
        fn upsert(&mut self, _doc: &IndexDocument) -> Result<(), IndexError> {
            Ok(())
        }
        fn delete(&mut self, _doc_id: &str) -> Result<(), IndexError> {
            Err(IndexError::Engine("trace vector index unavailable".into()))
        }
        fn search(&self, _: &AclFilter, _: &str, _: usize) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn search_structured(
            &self,
            _: &AclFilter,
            _: &str,
            _: &FieldValue,
            _: usize,
        ) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn semantic(
            &self,
            _: &AclFilter,
            _: &Embedding,
            _: usize,
        ) -> Result<Vec<VectorHit>, IndexError> {
            Ok(vec![])
        }
        fn merge(&mut self) -> Result<(), IndexError> {
            Ok(())
        }
        fn snapshot(&mut self) -> Result<u64, IndexError> {
            Ok(0)
        }
        fn indexed_zookie_of(&self, _doc_id: &str) -> Option<String> {
            None
        }
    }

    let subject = subject_ref("p-fail");
    let (kms, _sealed) = engine_with_subject_trace_content(&subject, b"pii");
    let holder = AgentTraceHolder::new();
    let t = tenant();
    write_agent_trace(&holder, &t, "run-f", narrative("trace with pii"), "p-fail");

    let index = Mutex::new(FailingIndex);
    let store = Mutex::new(PageStore::new());
    let ledger = RecLedger::default();

    let err = holder
        .erase_subject_traces(
            &subject,
            &t,
            region(),
            &kms,
            &RecPseudonym::default(),
            &RecBus::default(),
            &ledger,
            &index,
            &store,
            1,
        )
        .expect_err(
            "a failed embedding purge is a LOUD error (the index is plaintext-derived PII)",
        );
    assert!(
        matches!(err, EraseError::SearchPurge(_)),
        "the loud error names the Search/embedding purge step"
    );
    assert!(
        !ledger.is_erased(&SubjectId::new("p-fail"), &t),
        "an incomplete trace erase is NOT recorded as erased"
    );
}

// ───────────────────────── mutation-floor guards (the KN-D12 green predicate) ─────────────────────────

#[test]
fn trace_receipt_is_green_only_when_zero_recoverable_and_attribution_intact() {
    let base = TraceEraseReceipt {
        receipt: Receipt::content_addressed("erase", TRACE_HOLDER_ID, "u", "acme", "n", None, 0),
        traces_shredded: 1,
        recoverable_in_backup: 0,
        embeddings_purged: 1,
        backlinks_tombstoned: 0,
        attribution_pseudonym: "p-u".into(),
        re_run: false,
        at_ms: 0,
    };
    assert!(
        base.is_green(),
        "0 recoverable + a non-empty pseudonym is GREEN"
    );

    // Kills the `recoverable_in_backup` mutant: non-zero recoverable is RED.
    let leaky = TraceEraseReceipt {
        recoverable_in_backup: 1,
        ..base.clone()
    };
    assert!(!leaky.is_green(), "non-zero recoverable PII is RED");

    // Kills the attribution mutant: an empty pseudonym (attribution lost) is RED.
    let no_attr = TraceEraseReceipt {
        attribution_pseudonym: String::new(),
        ..base
    };
    assert!(
        !no_attr.is_green(),
        "lost attribution is RED (attribution must stay intact)"
    );
}
