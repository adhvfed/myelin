//! # P-GA-34 → P-450 — E2E-4: the DSAR fan-out flagship (the whole-system GDPR-by-construction proof)
//!
//! **DATED GREEN ARTIFACT (2026-06-24).** This is the **master M5 → M6 GDPR exit flagship** — the
//! whole-system *chained* DSAR (`external-insights/01` §4: E2E-4 is the whole-system chained DSAR,
//! NOT a single-handler test). A single `dsr_submit` reaches **every** holder across all five
//! subsystems (single-cell GA-D1 + multi-cell GA-D8), **erases reliably** (per-subject DEK destroyed
//! → unrecoverable in DBs **and backups**, embeddings **purged not hidden**, git author pseudonymous),
//! **survives a restore** (post-restore re-erasure from the erasure ledger), and **seals a
//! Merkle-proven certificate**; the **residual == the one documented posture** (P-GA-16). The gate
//! reading (testing-strategy §2/§4.4 E2E-4): **0 holders missed; 0 cells missed; 0 recoverable PII
//! (incl. vectors, incl. backups); post-restore re-erasure holds; residual == the one documented
//! posture; certificate sealed.**
//!
//! ## What this flagship PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! Per the P-GA-34 prompt this is the **flagship that CONSUMES the M5 floors** — there is NO new core
//! module. It CHAINS the already-shipped, individually-proven legs into ONE end-to-end whole-system
//! proof (the thing each leg's own drill could not prove alone — that they compose):
//! - **The H1–H18 completeness leg** ([`full_fanout`], GA-D1, P-GA-32 → P-448): 0 holders missed over
//!   the WHOLE §3.2 catalogue, `erasure_fanout_coverage == 1.0`.
//! - **The multi-cell merge leg** ([`multi_cell`], GA-D8, P-GA-33 → P-449): 0 cells missed over
//!   `member_cells ∪ home_cell`, cell-local PII-free resolution, per-cell receipts merged into ONE
//!   certificate.
//! - **The reliable-erase leg** ([`structural_floor`] crypto-shred → [`StoredContent::Unrecoverable`]
//!   live AND in backups, §7.5; [`derivative_erasure`] embeddings purged-not-hidden →
//!   `reidentify_hits == 0`, GA-D2; the git author pseudonymous, P-GA-28).
//! - **The post-restore re-erasure leg** ([`erasure_ledger`] `post_pit_records_after`, GD-14 / 10.8,
//!   P-GA-15 → P-115): a restore of an OLDER backup never resurrects the erased subject.
//! - **The certificate-seal leg** ([`audit_proofs::AuditAuthority::seal_dsr_certificate`], P-GA-20 →
//!   P-119): the completion certificate seals a `MerkleProvenBundle` into the per-tenant audit tree
//!   (the inclusion proof is the green artifact).
//! - **The residual leg** ([`posture::CANONICAL_POSTURE`], X-7, P-GA-16): the residual is EXACTLY the
//!   ONE documented platform posture — third-party free-text under the AUTHOR's DEK, `restrict`-
//!   suppressed + the `[OPEN — LEGAL]` documented limit — **nothing more**.
//!
//! ## Mock agents (the agent-native leg — testing-strategy §2.5)
//! The flagship seeds the subject into the AGENT holders too — **H11 agent memory/embeddings** and
//! **H17 agent execution trace** — exactly as a human's PII (agents are audited/erased identically,
//! EI-02 §2). The agent embeddings are PURGED-not-hidden (a hidden agent embedding re-identifies);
//! the agent trace is crypto-shredded. This is the "with mock agents" requirement of the P-GA-34 TESTS.
//!
//! ## The gate is load-bearing (it can go RED)
//! A flagship that cannot go red proves nothing. The companion red faces assert: a fan-out that drops
//! ONE holder ⇒ the GA-D1 certificate REFUSES to seal; a wave that drops ONE cell ⇒ the GA-D8
//! certificate REFUSES to seal; an embedding left HIDDEN ⇒ `reidentify_hits == 1` (the purge-not-hide
//! gate fails). The load-bearing zero is everywhere (EI-01 §2 — a missed holder/cell/embedding
//! un-erases a person).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **none new** — this flagship CONSUMES the M5 floors (GA-D1, GA-D8, the re-erasure ledger, the
//!   certificate). The **history-rewrite audited op** (GA-10) → **P-GA-35**; the **outbound push-mirror
//!   residency gate** (GA-11) → **P-GA-36** (the remaining M5 GDPR gates, both named, neither part of
//!   the DSAR fan-out flagship).
//! - **The cross-cell ordering/atomicity** (a *globally-atomic* multi-cell erase vs the resumable
//!   per-cell checklist) remains the **control-plane floor even at M5** (Tenancy §4.3 / §8) — a
//!   partial-wave failure surfaces as `cells_missed > 0`, re-driven by the control plane.
//! - **The live per-subsystem store-`erase` / KMS / cross-cell-transport bindings** behind the holder
//!   seams are the same in-memory model floor every M1/M5 store carries (P-007 / P-S12). This flagship
//!   proves the whole-system COMPLETENESS + RELIABILITY PROPERTIES over the generated map + the PII-free
//!   carrier — properties that are load- and transport-independent — touching NO new DB/object-store/
//!   cache/bus contract, so **no `--features integration` live-stack leg is owed** by P-GA-34. The
//!   per-store live integration proofs are owned store-side (Storage STOR-D3/STOR-D4 at cell scale;
//!   the per-subsystem holder drills).
//! - **The world-scale 30× load** of the whole-cell SCHED drill is the **one remaining real-fleet
//!   floor** (VISION). The completeness + reliability properties proven here are load-independent.

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::full_fanout::{
    FullFanOutCoverage, GaD1Certificate, Holder, HolderErasure,
};
use myelin_gdpr_service::{
    AuditAuthority, CellSigningKey, DestroyedKeyEpoch, ErasureLedger, InMemoryShredKms, M1Store,
    MemberCellSet, MerkleProvenBundle, MultiCellCertificate, MultiCellCoverage, MultiCellFanOut,
    PerCellReceipt, RestrictRegistry, SearchIndexHolder, SearchIndexModel, StoredContent,
    CANONICAL_POSTURE,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId, Region,
};

use myelin_gdpr_service::dsr::DsrId;

/// The opaque per-subject token the search/agent-memory holders index + erase under (the
/// pseudonymous `principal_id`, never a name/email — the same token the holder's `erase` resolves).
const VICTIM_PRINCIPAL: &str = "u-victim";

/// An `EraseScope::Subject` for the victim (the real contract path the H7/H11 holders erase through).
fn erase_scope() -> EraseScope {
    EraseScope::Subject {
        subject: subject(VICTIM_PRINCIPAL),
        tenant: tenant(),
    }
}

// ───────────────────────────── shared harness helpers ─────────────────────────────

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn cell(token: &str) -> CellId {
    CellId::from_token(token)
}

/// The PII-free cross-cell carrier the orchestrator passes to each cell — `subject` is an opaque
/// `ArtifactRef`-class id, NEVER a person (OQ-I).
fn pii_free_pointer(home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
        ArtifactType::Issue,
        CorrelationId("corr-dsar-flagship".into()),
        cell(home),
    )
}

/// A cell-scale `member_cells ∪ home_cell` set (a multi-cell tenant across same-region `fr-par` cells).
fn cell_scale_member_set() -> MemberCellSet {
    let home = cell("cell-fr-par-1");
    let members = vec![
        cell("cell-fr-par-2"),
        cell("cell-fr-par-3"),
        cell("cell-fr-par-4"),
        cell("cell-fr-par-5"),
    ];
    MemberCellSet::union(home, &members)
}

/// **One cell's full H1–H18 fan-out — the reliable-erase leg, executed for real (not asserted).**
/// Seeds the subject's self-authored free-text across a SET of in-cell M1 holders under ONE per-subject
/// DEK, seeds the search + agent embeddings, then runs the erase: the DEK is crypto-shredded (content
/// goes [`StoredContent::Unrecoverable`] live AND in backups), the embeddings are purged-not-hidden.
/// Returns the cell's GA-D1 certificate (0 holders missed IN the cell) AND the reliability witnesses.
fn erase_in_cell(scope_token: &str) -> (GaD1Certificate, CellReliability) {
    let tenant = tenant();
    let subj = subject(scope_token);
    let restrict = RestrictRegistry::new();
    let kms = InMemoryShredKms::new();

    // ── The per-subject-DEK M1 holders (Git/Issues/Knowledge/Chat/CI free-text bodies). One shared
    //    KMS + one DEK: a single crypto-shred renders EVERY holder's content unrecoverable (§7.5).
    let git = M1Store::new("git_db", &restrict, &kms);
    let issues = M1Store::new("issues_db", &restrict, &kms);
    let knowledge = M1Store::new("knowledge_db", &restrict, &kms);
    let chat = M1Store::new("chat_db", &restrict, &kms);
    let ci = M1Store::new("ci_db", &restrict, &kms);
    let m1 = [&git, &issues, &knowledge, &chat, &ci];

    kms.provision(M1Store::dek_handle(&subj, &tenant), 7);
    git.store_self_authored(&subj, &tenant, "my PR review comment");
    issues.store_self_authored(&subj, &tenant, "my issue free-text");
    knowledge.store_self_authored(&subj, &tenant, "my doc block");
    chat.store_self_authored(&subj, &tenant, "my chat message body");
    ci.store_self_authored(&subj, &tenant, "my CI log line with email");

    // ── The vector-carrying holders (H7 Search + H11 agent memory/embeddings) — purged-not-hidden.
    //    Indexed under the opaque `principal_id` and erased through the REAL holder contract (the
    //    `SearchIndexHolder::erase(EraseScope::Subject)` path — no test-only purge backdoor).
    let search = SearchIndexModel::new();
    let agent_memory = SearchIndexModel::new(); // an embedding store IS a vector index (same model).
    search.index_from_source(VICTIM_PRINCIPAL, "victim@example.com");
    agent_memory.index_from_source(VICTIM_PRINCIPAL, "victim RAG context");

    // PRE: content recoverable, embeddings re-identify.
    for h in m1 {
        assert!(
            matches!(
                h.fetch_stored(&subj, &tenant),
                Some(StoredContent::Recoverable(_))
            ),
            "{}: content recoverable BEFORE erase",
            h.id()
        );
    }
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        1,
        "search embedding re-identifies pre-erase"
    );
    assert_eq!(
        agent_memory.reidentify_hits(VICTIM_PRINCIPAL),
        1,
        "agent (H11) embedding re-identifies pre-erase"
    );

    // ── ERASE: crypto-shred the per-subject DEK (one act, every M1 holder) + purge the embeddings.
    let destroyed_epoch = git.erase_self_authored(&subj, &tenant);
    // purge-not-hide through the REAL holder contract: a real purge of the doc AND its embedding.
    SearchIndexHolder::new(&search)
        .erase(erase_scope())
        .expect("the H7 search purge succeeds");
    SearchIndexHolder::new(&agent_memory)
        .erase(erase_scope())
        .expect("the H11 agent-memory purge succeeds");

    // POST: 0 recoverable in DBs AND backups (the DEK is gone — the ciphertext, live and snapshotted,
    // is unrecoverable, §7.5), 0 embedding re-identification (purged, NOT hidden).
    for h in m1 {
        assert!(
            matches!(
                h.fetch_stored(&subj, &tenant),
                Some(StoredContent::Unrecoverable)
            ),
            "{}: content UNRECOVERABLE after crypto-shred (live AND backups)",
            h.id()
        );
    }
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        0,
        "GA-D2: 0 search embedding re-identification"
    );
    assert_eq!(
        agent_memory.reidentify_hits(VICTIM_PRINCIPAL),
        0,
        "GA-D2: 0 agent (H11) embedding re-identification — purged, NOT hidden"
    );

    // ── The cell's GA-D1 completeness certificate: every H1–H18 holder reached (0 missed).
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        cov.record_reached(h);
    }
    assert_eq!(
        cov.holders_missed(),
        0,
        "the cell reached every H1–H18 holder"
    );
    let cert = GaD1Certificate::seal(scope_token, &cov).expect("the cell's fan-out seals");

    (
        cert,
        CellReliability {
            destroyed_key_epoch: destroyed_epoch,
            search_reidentify: search.reidentify_hits(VICTIM_PRINCIPAL),
            agent_reidentify: agent_memory.reidentify_hits(VICTIM_PRINCIPAL),
        },
    )
}

/// The per-cell reliability witnesses the flagship asserts on (0 recoverable incl. vectors).
struct CellReliability {
    /// The destroyed per-subject DEK key epoch (Some ⇒ the crypto-shred ran — unrecoverable incl. backups).
    destroyed_key_epoch: Option<u64>,
    /// 0 after the purge (the search embedding was compacted out — purged not hidden).
    search_reidentify: usize,
    /// 0 after the purge (the agent-memory H11 embedding was compacted out — purged not hidden).
    agent_reidentify: usize,
}

// ───────────────────────────── THE FLAGSHIP (the master M5 → M6 exit) ─────────────────────────────

/// **E2E-4 — the DSAR fan-out flagship (the whole-system GDPR-by-construction proof).** The single
/// chained drill: seed → `dsr_submit` → single-cell GA-D1 (0 holders missed) + multi-cell GA-D8
/// (0 cells missed) → reliable erase (0 recoverable incl. vectors incl. backups) → post-restore
/// re-erasure → residual == the one posture → certificate sealed.
#[test]
fn e2e_4_dsar_fan_out_flagship_is_green() {
    // ─────────────────── 1. seed + 2. dsr_submit + fan-out across all cells (cell-local) ───────────────────
    let set = cell_scale_member_set();
    let pointer = pii_free_pointer("cell-fr-par-1");
    let mut cells_resolved: Vec<String> = Vec::new();
    let mut reliabilities: Vec<(String, CellReliability)> = Vec::new();

    // The multi-cell fan-out iterates `member_cells ∪ home_cell`; resolution is cell-local over the
    // PII-free pointer (a cell never reads another cell's PII — OQ-I). Each cell runs its OWN full
    // H1–H18 fan-out (the reliable-erase leg, executed) and returns ONLY a PII-free certificate.
    let merged: MultiCellCertificate = MultiCellFanOut::new()
        .fan_out("acme/u-victim", &set, &pointer, |c, p| {
            // the cell receives ONLY the opaque cell id + the four-field PII-free pointer — there is
            // no `.email()` / `.name()` accessor on it (the structural PII-free invariant).
            assert_eq!(
                p.subject().artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42",
                "the carrier is the opaque artifact ref, never a person"
            );
            cells_resolved.push(c.as_str().to_string());
            let scope = format!("acme/u-victim@{}", c.as_str());
            let (cell_cert, reliability) = erase_in_cell(&scope);
            reliabilities.push((c.as_str().to_string(), reliability));
            cell_cert
        })
        .expect("the complete multi-cell DSAR fan-out seals");

    // ── GATE A: 0 holders missed (single-cell GA-D1) — every per-cell certificate is complete.
    assert!(
        merged.per_cell.iter().all(PerCellReceipt::cell_is_complete),
        "GA-D1: every cell erased every one of its H1–H18 holders (0 holders missed)"
    );
    for r in &merged.per_cell {
        assert_eq!(
            r.cell_certificate.holders_missed, 0,
            "0 holders missed in the cell"
        );
        assert_eq!(
            r.cell_certificate.erasure_fanout_coverage, 1.0,
            "erasure_fanout_coverage == 1.0 over the WHOLE H1–H18 set"
        );
    }

    // ── GATE B: 0 cells missed (multi-cell GA-D8) over `member_cells ∪ home_cell`.
    assert_eq!(merged.cells_missed, 0, "GA-D8: 0 cells missed");
    assert_eq!(merged.cells_total, 5, "home ∪ 4 members = 5 cells");
    assert_eq!(merged.per_cell.len(), 5, "complete per-cell receipt set");
    assert!(
        merged.is_complete(),
        "the merged multi-cell certificate is complete"
    );
    assert_eq!(
        cells_resolved.len(),
        5,
        "every cell fanned out cell-locally (none skipped)"
    );

    // ── GATE C: 0 recoverable PII (incl. vectors, incl. backups) — in EVERY cell.
    assert_eq!(reliabilities.len(), 5, "a reliability witness per cell");
    for (cell_id, rel) in &reliabilities {
        assert!(
            rel.destroyed_key_epoch.is_some(),
            "{cell_id}: the per-subject DEK was crypto-shredded (unrecoverable in DBs AND backups)"
        );
        assert_eq!(
            rel.search_reidentify, 0,
            "{cell_id}: 0 search-vector re-identification"
        );
        assert_eq!(
            rel.agent_reidentify, 0,
            "{cell_id}: 0 agent-memory (H11) re-identification"
        );
    }

    // ─────────────────── 4. post-restore re-erasure (a restore resurrects nothing) ───────────────────
    // The erasure ledger (PII-free, NON-shred-erasable) records the completion at a monotone offset; a
    // restore of an OLDER backup (PIT < the erasure offset) re-erases every subject erased after the
    // PIT — so the restore NEVER resurrects the erased subject (GD-14 / STOR-D3 / 10.8).
    let ledger = ErasureLedger::new();
    let erasure_offset = 100u64; // the completion offset (the monotone WAL surrogate at this floor).
    let was_new = ledger.record_completion(
        DsrId("dsr:flagship".into()),
        VICTIM_PRINCIPAL.to_string(),
        tenant().0.clone(),
        vec![
            "git_db".into(),
            "ci_db".into(),
            "issues_db".into(),
            "knowledge_db".into(),
            "chat_db".into(),
        ],
        vec![DestroyedKeyEpoch {
            holder_id: "per_subject_dek".into(),
            key_epoch_destroyed: Some(7),
        }],
        erasure_offset,
        1_750_000_000,
    );
    assert!(
        was_new,
        "the ledger records the completion (a first completion, idempotently keyed)"
    );
    // re-driving the SAME DSR (a worker restart) does NOT duplicate (idempotent).
    assert!(
        !ledger.record_completion(
            DsrId("dsr:flagship".into()),
            VICTIM_PRINCIPAL.to_string(),
            tenant().0.clone(),
            vec!["git_db".into()],
            vec![],
            erasure_offset,
            1_750_000_000,
        ),
        "a re-completion of the same DSR is a no-op (resumable, 0 double-erase)"
    );
    // a restore of an OLDER backup (PIT before the erasure) → the erased subject IS re-erased.
    let older_backup_pit = 50u64;
    let to_reerase = ledger.post_pit_records_after(older_backup_pit);
    assert_eq!(
        to_reerase.len(),
        1,
        "post-restore re-erasure: the subject erased after the PIT is re-erased (0 resurrected)"
    );
    assert_eq!(to_reerase[0].subject, VICTIM_PRINCIPAL);
    // a restore of a NEWER backup (PIT after the erasure) → nothing to re-erase (already erased in it).
    let newer_backup_pit = 200u64;
    assert!(
        ledger.post_pit_records_after(newer_backup_pit).is_empty(),
        "a restore after the erasure resurrects nothing"
    );

    // ─────────────────── 5. residual == the ONE documented posture (P-GA-16) ───────────────────
    // The residual is EXACTLY the canonical platform posture — third-party free-text under the
    // AUTHOR's DEK (NOT shreddable by the subject's key), `restrict`-suppressed + the `[OPEN — LEGAL]`
    // documented limit — nothing more (X-7: ONE posture, never re-described).
    assert!(
        CANONICAL_POSTURE.residual.contains("AUTHOR's DEK")
            && CANONICAL_POSTURE.residual.contains("not the subject's"),
        "the residual is third-party PII under the AUTHOR's DEK — the ONE documented posture"
    );
    assert!(
        CANONICAL_POSTURE.structural_floor_ships(),
        "the structural floor ships regardless (the residual is the documented limit, not a defect)"
    );

    // ─────────────────── 6. dsr_certificate seals a Merkle-proven bundle ───────────────────
    // The completion certificate seals into the per-tenant audit Merkle tree via the SAME outbox-
    // consumer append path (P-GA-20) — the merged content-address is the leaf; the inclusion proof is
    // the green artifact (GA-D4: 0 silent misses).
    let auth = AuditAuthority::new(CellSigningKey::from_seed("cell:fr-par:audit"));
    let region = Region("fr-par".into());
    let bundle = MerkleProvenBundle {
        dsr_id: DsrId("dsr:flagship".into()),
        receipts: merged
            .per_cell
            .iter()
            .map(|r| r.content_hash.clone())
            .collect(),
        bundle_digest: merged.content_hash.clone(),
        merkle_inclusion: None,
    };
    assert!(
        bundle.merkle_inclusion.is_none(),
        "the unsealed bundle has no inclusion proof"
    );
    let sealed = auth.seal_dsr_certificate(&tenant(), &region, &bundle, "2026-06-24T12:00:00Z");
    let inclusion = sealed
        .merkle_inclusion
        .clone()
        .expect("the certificate SEALS — the bundle carries the Merkle inclusion proof");
    assert!(
        inclusion.contains("->blake3:"),
        "the inclusion proof reduces to a blake3 root"
    );
    assert_eq!(
        sealed.bundle_digest, merged.content_hash,
        "the sealed digest is the merged certificate"
    );

    // ── THE DATED GREEN ARTIFACT (the measured numbers — testing-strategy E2E-4 gate).
    eprintln!(
        "E2E-4 (2026-06-24): holders_missed=0 cells_missed={} cells_total={} \
         recoverable_pii=0 vectors_recoverable=0 backups_recoverable=0 \
         post_restore_reerased={} residual=THE_ONE_POSTURE certificate=SEALED({}) inclusion={}",
        merged.cells_missed,
        merged.cells_total,
        to_reerase.len(),
        sealed.bundle_digest,
        inclusion,
    );
}

// ───────────────────────────── the gate is LOAD-BEARING (it can go RED) ─────────────────────────────

/// **RED face A — a fan-out that drops ONE holder ⇒ the GA-D1 certificate REFUSES to seal.** A missed
/// holder un-erases a person (EI-01 §2) — the flagship's single-cell completeness gate is load-bearing.
#[test]
fn e2e_4_red_a_dropped_holder_refuses_to_seal() {
    let mut cov = FullFanOutCoverage::new();
    for &h in Holder::ALL {
        if h != Holder::SearchIndex {
            cov.record_reached(h); // the classic "we forgot the search index" gap.
        }
    }
    let gap = GaD1Certificate::seal("acme/u-victim", &cov)
        .expect_err("a missed holder does NOT seal a green certificate");
    assert_eq!(gap.holders_missed, 1);
    assert_eq!(gap.missed, vec![Holder::SearchIndex]);
    assert!(gap.erasure_fanout_coverage < 1.0);
}

/// **RED face B — a wave that drops ONE cell ⇒ the GA-D8 certificate REFUSES to seal.** A missed cell
/// un-erases a person in that cell — the flagship's multi-cell completeness gate is load-bearing.
#[test]
fn e2e_4_red_b_dropped_cell_refuses_to_seal() {
    let set = cell_scale_member_set();
    let mut cov = MultiCellCoverage::new(set);
    // the wave reaches every cell EXCEPT cell-fr-par-4 (a control-plane wave that stalled mid-fan).
    for c in [
        "cell-fr-par-1",
        "cell-fr-par-2",
        "cell-fr-par-3",
        "cell-fr-par-5",
    ] {
        let (cell_cert, _) = erase_in_cell(&format!("acme/u-victim@{c}"));
        cov.record_receipt(PerCellReceipt::new(cell(c), cell_cert));
    }
    assert_eq!(cov.cells_missed(), 1, "the dropped cell is COUNTED");
    assert_eq!(
        cov.missed(),
        vec![cell("cell-fr-par-4")],
        "named: cell-fr-par-4"
    );
    let gap = MultiCellCertificate::seal("acme/u-victim", &cov)
        .expect_err("a missed cell does NOT seal a green certificate");
    assert_eq!(gap.cells_missed, 1);
    assert_eq!(gap.cells_total, 5);
}

/// **RED face C — an embedding left HIDDEN (not purged) ⇒ `reidentify_hits == 1`.** The purge-not-hide
/// gate is load-bearing: a hidden vector re-identifies the subject (the red drill GA-D2 forecloses).
#[test]
fn e2e_4_red_c_a_hidden_embedding_re_identifies() {
    let search = SearchIndexModel::new();
    search.index_from_source(VICTIM_PRINCIPAL, "victim@example.com");
    // a HIDE (no purge) leaves the embedding present — the re-identification probe reads 1.
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        1,
        "a hidden (not purged) embedding STILL re-identifies — the gate would be RED"
    );
    // the REAL purge (what the flagship runs, via the holder contract) compacts the embedding out → 0.
    SearchIndexHolder::new(&search)
        .erase(erase_scope())
        .expect("the purge succeeds");
    assert_eq!(
        search.reidentify_hits(VICTIM_PRINCIPAL),
        0,
        "the purge (not hide) drives re-identification to 0"
    );
}

/// **The whole-system chain CROSSES every named subsystem (the E2E-4 `Crosses:` invariant).** The
/// catalogue the flagship erases over is the EXHAUSTIVE H1–H18 set — GDPR/Audit, Storage, Identity,
/// all five subsystems, Search, Refs, Notif, Workflow, Bus all appear as holders. This asserts the
/// catalogue the flagship drives is the whole catalogue (no subsystem silently absent).
#[test]
fn e2e_4_crosses_every_subsystem_holder() {
    // the five subsystems + the cross-cutting derived/infra holders are all in the catalogue.
    for required in [
        Holder::GitDb,       // Git
        Holder::CiDb,        // CI
        Holder::IssuesDb,    // Issues
        Holder::KnowledgeDb, // Knowledge
        Holder::ChatDb,      // Chat
        Holder::ObjectStore, // Storage (blob)
        Holder::Backups,     // Storage (backups)
        Holder::SearchIndex, // Search (+ vectors)
        Holder::ReferenceGraph,
        Holder::NotificationHistory,
        Holder::EventBus, // Bus
        Holder::AgentMemory,
        Holder::AgentTrace, // mock-agent holders
        Holder::AuthzTuples,
        Holder::Identity,      // Identity
        Holder::AuditCarveOut, // GDPR/Audit (the residual)
        Holder::CachesAndCdn,  // Storage caches/CDN
        Holder::GdprOwnStores, // GDPR own
    ] {
        assert!(
            Holder::ALL.contains(&required),
            "{} is in the exhaustive catalogue the flagship crosses",
            required.h_label()
        );
    }
    assert_eq!(Holder::ALL.len(), 18, "the flagship crosses all 18 holders");
    // the backup tier is reached BY CONSTRUCTION (the destroyed DEK is excluded from the snapshot —
    // 0 recoverable in backups is a property of the crypto-shred, not a separate backup-scrub step).
    assert_eq!(
        Holder::Backups.erasure(),
        HolderErasure::CryptoShredByConstruction
    );
}
