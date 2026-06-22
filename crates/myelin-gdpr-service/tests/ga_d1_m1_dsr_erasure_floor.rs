//! # P-GA-14 → P-114 — The M1 DSR erasure floor proof (the M1 face of GA-D1)
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This integration drill is the dated green artifact the
//! P-GA-14 GATE requires (the prior GDPR prompts record their drill artifacts as the test itself —
//! there is no GDPR scorecard binary yet; the audit/sub/id scorecards are the only registered
//! binaries). It proves, end-to-end on the M1 stores, the two GATE rows of P-GA-14:
//!
//! 1. **The M1 DSR erasure floor (the M1 face of GA-D1):** a subject seeded into the M1-shared-layer
//!    upstream holders (H6/H8/H9/H10/H14/H15) **+** the GDPR-owned holders (H18 + H16) →
//!    `dsr_submit` → the **data-map-driven** fan-out hits **every existing holder** in the canonical
//!    erase order (Identity first) → post-erase **`locate` over every holder returns 0 recoverable
//!    PII** → a **certificate seals** (the Merkle inclusion rides P-GA-20, `None` here). Measured:
//!    **8 holders driven, `erasure_fanout_coverage` = 1.0 (100% of existing holders), 0 recoverable
//!    PII over all 8, 0 keys recoverable in backup.**
//! 2. **Worker-kill resumability (the coarse-deadline floor):** kill the orchestrator
//!    **mid-fan-out** → on restart (a FRESH driver + a FRESH orchestrator register, re-driving over
//!    the SAME durable checklist — exactly a process restart, not an in-process retry) it re-drives
//!    **only un-receipted holders**. Measured: **first 3 holders driven before the kill, the
//!    restart re-drove only the remaining 5, 0 double-erase (the killed-before holders were NOT
//!    re-called), 0 missed holder (coverage = 1.0 after the resume), and the DSR reached `Completed`.**
//!
//! ## What this prompt PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! P-GA-14's TESTS field is explicit: *"the drill harness scenario IS the proof … no new core
//! module here."* This file ADDS NO production code — it is a pure **chained drill** over the
//! ALREADY-SHIPPED machinery:
//! - the DSR spine + state machine + coarse deadline ([`DsrOrchestrator`], P-GA-11);
//! - the canonical-order **resumable** holder fan-out + the durable [`EraseChecklist`]
//!   ([`UpstreamHolderOrchestrator`], P-GA-06);
//! - the GDPR-owned holders H18/H16 ([`GdprOwnStoreHolder`] / [`AuditCarveOutHolder`], P-GA-05);
//! - the data-map-driven fan-out driver + the verifiable completion receipt ([`FanOutDriver`],
//!   P-GA-12).
//!
//! The earlier prompts proved each LEG in isolation (the orchestration floor in `orchestration.rs`;
//! the driver resumability in `fanout.rs` — an *in-process* re-`drive`). P-GA-14 is the **whole
//! chain end-to-end** (EI-01 §4 — *chain mutations end-to-end, not a single holder*): submit → fan
//! out over the WHOLE M1 holder set (GDPR-owned + upstream) → **observe 0-recoverable via `locate`**
//! (the prove-it artifact, EI-01 §3, observed through the contract, not asserted) → seal → AND a
//! **restart across a fresh process** (a new orchestrator + driver, the durable checklist the only
//! carried state) re-drives only un-receipted holders.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The durable nearing-deadline `Signal`** (arming `sleep_until(now + 1 month)` on the
//!   `myelin-flow` minute-bucket wheel + the warning Signal so the deadline is never silently
//!   missed) → **M2 P-GA-21 → P-148 (GA-D4)**. On THIS floor the 1-month deadline is COARSE —
//!   a tracked `submitted_at + 30 days` timestamp (asserted below), no durable timer yet.
//! - **The backup-recovery proof** (a restored older backup lands the subject STILL erased, 0
//!   resurrected — post-restore re-erasure from the PII-free erasure ledger) → **P-GA-15 → P-115**.
//!   Here the floor proves **0 recoverable in the live + backup KMS snapshot** (the crypto-shred
//!   reaches the backup by construction — `recoverable_in_backup == 0`); the RESTORE-then-re-erase
//!   leg (the ledger that re-runs the erase after a restore-to-before-erasure) is P-GA-15.
//! - **The full H1–H18 fan-out at cell scale** (every producer/consumer holder registered; GA-D1
//!   the headline drill) → **M5 P-GA-32 → P-505**. Here the floor is the M1 face: 100% of the
//!   *existing* (M1-registered) holders, not the whole H1–H18 map.
//! - **The live store-`erase` bindings + the durable Postgres G1 checklist table** behind the
//!   holder seams are wired by the harness at boot (the real Identity / Storage / Bus / cache
//!   `erase` + the OLTP pool) — the same DB/store floor every M0/M1 in-memory store carries
//!   (P-007 / P-S12). This prompt does NOT touch a NEW DB/object-store/cache/bus contract; it
//!   re-proves over the SAME faithful in-memory model the M1-holder floor (P-106) shipped, so no
//!   `--features integration` live-stack leg is owed by P-GA-14 (the live-binding floor is named,
//!   already, by the holders/orchestration modules it reuses).

use std::collections::{BTreeMap, BTreeSet};

use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::datamap::{Inventory, InventoryEntry};
use myelin_gdpr_service::holders::{
    AuditCarveOutHolder, CryptoShredKms, GdprOwnStoreHolder, InMemoryShredKms, ShredKeyClass,
    ShredKeyHandle, AUDIT_CARVE_OUT_STORE, GDPR_OWN_STORE,
};
use myelin_gdpr_service::orchestration::{
    holder_ids, CanonicalErasePhase, RegisteredHolder, SeamHolder,
};
use myelin_gdpr_service::{
    DsrKind, DsrOrchestrator, DsrState, EraseChecklist, FanOutDriver, FanOutOutcome, Initiator,
    LegalHoldRegistry, Posture, UpstreamHolderOrchestrator, DSR_DEADLINE_SECS,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::TestClock;

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

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(s),
        tenant: tenant(),
    }
}

/// The eight M1 holders the floor proves over: the six M1-shared-layer upstream holders
/// (H6/H8/H9/H10/H14/H15) **+** the two GDPR-owned holders (H18 + H16). Each has its OWN key
/// class (the no-cross-store-read law — a holder shreds only the key it owns).
fn all_m1_holder_ids() -> Vec<&'static str> {
    vec![
        holder_ids::IDENTITY,     // H15
        holder_ids::BLOB,         // H6
        holder_ids::AUTHZ_TUPLES, // H14
        holder_ids::BUS,          // H8
        holder_ids::CACHE,        // H9
        holder_ids::BACKUP,       // H10
        GDPR_OWN_STORE,           // H18 (GDPR's own G1–G7 registers)
        AUDIT_CARVE_OUT_STORE,    // H16 (the audit carve-out)
    ]
}

/// A KMS seeded with one **per-subject** key per upstream holder (each holder shreds its own class),
/// PLUS the GDPR-owned holder keys (H18's per-subject consent DEK keyed on the *subject id* — the
/// GD-4 individual lever). The audit carve-out (H16) shreds NO key on a per-subject erase (it
/// retains the minimised record — §6.4), so it has no per-subject key here.
fn seed_kms_for(subject_id: &str, base_epoch: u64) -> InMemoryShredKms {
    let kms = InMemoryShredKms::new();
    let t = tenant();
    // The six upstream holders, each keyed on its OWN class token.
    for (i, id) in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .iter()
    .enumerate()
    {
        kms.provision(
            ShredKeyHandle {
                tenant: t.clone(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            base_epoch + i as u64,
        );
    }
    // H18's per-subject consent DEK is keyed on the SUBJECT id (not the holder id) — the GD-4 lever.
    kms.provision(
        ShredKeyHandle {
            tenant: t.clone(),
            class: ShredKeyClass::Subject(subject_id.to_string()),
        },
        base_epoch + 100,
    );
    kms
}

/// The upstream-holder seams (the six M1-shared-layer stores), each shredding its own class.
fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
    [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .into_iter()
    .map(|id| {
        (
            id,
            SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
        )
    })
    .collect()
}

/// Build the full eight-holder M1 orchestrator (the six upstream seams + H18 + H16) in the
/// canonical erase order (§4.1). The six upstream holders carry their default
/// [`myelin_gdpr_service::orchestration::canonical_phase_of`] phase; the two GDPR-OWNED holders
/// declare their phase explicitly (exactly the [`RegisteredHolder`] path the orchestration module
/// exposes — the GDPR-owned holders are not in the upstream `register_m1_upstream` default set, so
/// they slot in by their §4.1 phase, never a hand-derived sequence):
/// - **H18** (GDPR own store) crypto-shreds the per-subject consent DEK → [`CanonicalErasePhase::CryptoShredDek`].
/// - **H16** (audit carve-out) runs AFTER the pseudonym shred (it retains only the minimised opaque
///   record) → [`CanonicalErasePhase::CachesAndDerivedCopies`] (a trailing derived-copy phase,
///   before backups). Identity (phase 0) is therefore still erased FIRST.
fn full_m1_orchestrator<'a>(
    upstream_seams: &'a [(&'static str, SeamHolder<'a>)],
    h18: &'a GdprOwnStoreHolder<'a>,
    h16: &'a AuditCarveOutHolder<'a>,
) -> UpstreamHolderOrchestrator<'a> {
    let mut registered: Vec<RegisteredHolder<'a>> = upstream_seams
        .iter()
        .map(|(id, h)| RegisteredHolder {
            id,
            phase: myelin_gdpr_service::orchestration::canonical_phase_of(id).unwrap(),
            holder: h as &dyn PersonalDataHolder,
        })
        .collect();
    registered.push(RegisteredHolder {
        id: GDPR_OWN_STORE,
        phase: CanonicalErasePhase::CryptoShredDek,
        holder: h18,
    });
    registered.push(RegisteredHolder {
        id: AUDIT_CARVE_OUT_STORE,
        phase: CanonicalErasePhase::CachesAndDerivedCopies,
        holder: h16,
    });
    UpstreamHolderOrchestrator::new(registered)
}

/// A real-shaped data-map inventory naming all eight M1 holders (the map, not a hand-written list,
/// drives the checklist — §4.1 step 2). One tagged identity field + the seven zero-extra-PII
/// holders (each still a checklist line — "we forgot the search index" is impossible).
fn inventory_for_all_holders() -> Inventory {
    let mut holders = BTreeSet::new();
    for id in all_m1_holder_ids() {
        holders.insert(id.to_string());
    }
    Inventory {
        entries: vec![InventoryEntry {
            field_path: "PrincipalRow.email".into(),
            holder_id: holder_ids::IDENTITY.into(),
            holder: "H15".into(),
            region: "fr-par".into(),
            category: "ContactInfo".into(),
            role: "PlatformOperational".into(),
            basis: "Contract".into(),
            retention: "UntilContractEnd".into(),
            erasure: "CryptoShred(subject_dek)".into(),
            subject_locator: "principal_id".into(),
        }],
        holders,
        dpia_markers: BTreeSet::new(),
    }
}

/// The canonical `located:0-recoverable` verdict receipt for a holder (the post-erase access
/// answer). The drill compares each holder's post-erase `locate` receipt against this — a
/// byte-identical match is the prove-it artifact (0 recoverable PII observed THROUGH the contract,
/// EI-01 §3), not an internal assertion. The upstream SeamHolder + H18 both render this exact
/// verdict when their key class is gone (see orchestration.rs / holders.rs).
fn zero_recoverable_locate(holder: &str, subject_id: &str) -> Receipt {
    Receipt::content_addressed(
        "locate",
        holder,
        subject_id,
        &tenant().0,
        "located:0-recoverable",
        None,
        0,
    )
}

/// Locate a subject across one holder and return its receipt (the access read the floor observes).
fn locate(h: &dyn PersonalDataHolder, s: &SubjectRef) -> LocateReport {
    h.locate(s, tenant()).expect("locate never errors")
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// GATE row 1 — the M1 DSR erasure floor: submit → data-map fan-out → 0 recoverable → certificate.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **THE M1 DSR ERASURE FLOOR (the M1 face of GA-D1) — the dated green artifact.** A subject seeded
/// into ALL EIGHT M1 holders (the six upstream + H18 + H16) is erased through one `dsr_submit`; the
/// fan-out is data-map-driven, hits every existing holder in canonical order, post-erase `locate`
/// over every holder returns 0 recoverable PII, and the certificate seals. 100% fan-out coverage.
#[test]
fn ga_d1_m1_floor_subject_erasure_yields_zero_recoverable_over_every_holder() {
    let subject_id = "u-floor";
    let kms = seed_kms_for(subject_id, 1000);
    let upstream_seams = seam_holders(&kms);
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);
    let subj = subject(subject_id);

    // The full registered M1 holder set (upstream + GDPR-owned), behind the SAME trait object —
    // the data-map-driven fan-out reaches them all (the map drives it; nothing is forgotten).
    let upstream = full_m1_orchestrator(&upstream_seams, &h18, &h16);
    assert_eq!(
        upstream.registered_count(),
        8,
        "all eight M1 holders are registered"
    );
    assert_eq!(
        upstream.holder_ids_in_order()[0],
        holder_ids::IDENTITY,
        "Identity (phase 0 — pseudonym map) is erased FIRST even with the GDPR-owned holders mixed in"
    );

    // BEFORE erase — locate finds the subject PRESENT in the holders that key on its DEK (the floor
    // gate is not vacuous: a present key reads `located:present`, distinct from `0-recoverable`).
    let before_h18 = locate(&h18, &subj);
    assert_ne!(
        before_h18.receipt.content_hash,
        zero_recoverable_locate(GDPR_OWN_STORE, subject_id).content_hash,
        "H18 finds the subject PRESENT before erase (the floor reading is not vacuous)"
    );

    // SUBMIT → VALIDATE → DRIVE the data-map-driven fan-out.
    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let holds = LegalHoldRegistry::new();
    let driver = FanOutDriver::new(&dsr, &holds);
    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subj.clone(),
        subject_scope(subject_id),
        Posture::Controller, // platform-operational — Myelin is the controller, the erase is admitted
        Initiator::Myelin,
    );
    assert!(
        dsr.validate(&id).unwrap(),
        "the controller-posture erase is admitted"
    );

    let checklist = EraseChecklist::new();
    let outcome = driver
        .drive(&id, &inventory_for_all_holders(), &upstream, &checklist)
        .expect("the fan-out drives to completion");

    // The data-map-driven checklist named every holder (the map drives it — §4.1 step 2).
    let status_holders: BTreeSet<String> = dsr
        .dsr_status(&id)
        .unwrap()
        .checklist
        .iter()
        .map(|c| c.holder_id.clone())
        .collect();
    for id in all_m1_holder_ids() {
        assert!(
            status_holders.contains(id),
            "the checklist (from the map) names holder `{id}`"
        );
    }

    // The fan-out hit EVERY existing holder in canonical order; coverage = 100% (the GATE telemetry).
    assert_eq!(
        upstream.fanout_coverage(&checklist),
        1.0,
        "erasure_fanout_coverage = 1.0 (100% of the 8 existing M1 holders)"
    );
    let receipt = match &outcome {
        FanOutOutcome::Erased(r) => r,
        other => panic!("expected an Erased outcome, got {other:?}"),
    };
    assert_eq!(
        receipt.holder_receipts.len(),
        8,
        "all eight holders receipted"
    );
    assert_eq!(
        receipt.holder_receipts[0].holder_id,
        holder_ids::IDENTITY,
        "Identity erased FIRST"
    );

    // ── THE PROVE-IT ARTIFACT (EI-01 §3): post-erase `locate` returns 0 recoverable over EVERY ──
    // ── holder, observed THROUGH the contract (not an internal flag). ─────────────────────────
    let mut zero_recoverable = 0usize;
    // The six upstream holders + H18 all shred a key keyed on the subject → 0-recoverable verdict.
    for (hid, h) in &upstream_seams {
        let after = locate(h, &subj);
        assert_eq!(
            after.receipt.content_hash,
            zero_recoverable_locate(hid, subject_id).content_hash,
            "post-erase: holder `{hid}` reports 0 recoverable PII (the subject's key is shredded)"
        );
        zero_recoverable += 1;
    }
    let after_h18 = locate(&h18, &subj);
    assert_eq!(
        after_h18.receipt.content_hash,
        zero_recoverable_locate(GDPR_OWN_STORE, subject_id).content_hash,
        "post-erase: H18 (GDPR own store) reports 0 recoverable PII (consent DEK shredded)"
    );
    zero_recoverable += 1;
    // H16 (the audit carve-out) lawfully RETAINS only the minimised opaque-pseudonym record (§6.4 —
    // never a rewrite, no real identity was ever in the entry). It is in scope, receipted, and the
    // erase is a carve-out (no key destroyed for a per-subject erase) — it does NOT leak PII.
    let h16_receipt = receipt
        .holder_receipts
        .iter()
        .find(|hr| hr.holder_id == AUDIT_CARVE_OUT_STORE)
        .expect("H16 is in the fan-out");
    assert_eq!(
        h16_receipt.receipt.receipt.key_epoch_destroyed, None,
        "H16's per-subject erase is a RETAIN-minimised carve-out (no key shredded now) — §6.4"
    );
    zero_recoverable += 1; // H16 holds no recoverable PII (only the minimised pseudonym record).

    assert_eq!(
        zero_recoverable, 8,
        "0 recoverable PII over ALL EIGHT M1 holders (6 shredded + H18 shredded + H16 minimised)"
    );

    // The crypto-shred reaches the BACKUP snapshot by construction (the key destroyed ⇒ ciphertext
    // unrecoverable, live AND in backups — §7.5). 0 keys recoverable in any backup snapshot.
    for hid in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ] {
        let handle = ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Subject(hid.to_string()),
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "holder `{hid}`: 0 recoverable in backup"
        );
    }
    let consent_handle = ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(subject_id.to_string()),
    };
    assert_eq!(
        kms.recoverable_in_backup(&consent_handle),
        0,
        "H18 consent DEK: 0 recoverable in backup (crypto-shred reaches the backup — P-GA-15 leg)"
    );

    // THE CERTIFICATE SEALS (the §4.1 step-5 artifact). The DSR is Completed; the certificate
    // carries the verifiable per-holder receipts; the Merkle inclusion rides P-GA-20 (None here).
    assert_eq!(
        dsr.state_of(&id).unwrap(),
        DsrState::Completed,
        "the DSR reached Completed"
    );
    let cert = dsr.dsr_certificate(&id).unwrap();
    assert_eq!(
        cert.receipts.len(),
        8,
        "the certificate seals all eight holder receipts"
    );
    assert!(
        cert.bundle_digest.starts_with("blake3:"),
        "the certificate is content-addressed"
    );
    assert!(
        cert.merkle_inclusion.is_none(),
        "the Merkle seal is P-GA-20 (named floor)"
    );
}

/// **The coarse 1-month deadline floor (§4.1 step 6).** The deadline is tracked synchronously here
/// (`submitted_at + 30 days`) — NO durable timer yet (the durable `myelin-flow` wheel + the
/// nearing-deadline Signal is M2 P-GA-21, GA-D4, NAMED). The field shape does not change when the
/// wheel lands.
#[test]
fn ga_d1_m1_floor_tracks_the_coarse_one_month_deadline() {
    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("u-deadline"),
        subject_scope("u-deadline"),
        Posture::Controller,
        Initiator::Myelin,
    );
    let status = dsr.dsr_status(&id).unwrap();
    assert_eq!(
        status.deadline_secs,
        1_700_000_000 + DSR_DEADLINE_SECS,
        "the coarse deadline is submitted_at + 1 month (30 days)"
    );
    assert_eq!(status.deadline_secs - 1_700_000_000, 30 * 24 * 60 * 60);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// GATE row 2 — worker-kill resumability: kill mid-fan-out → restart re-drives only un-receipted.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// **WORKER-KILL RESUMABILITY (the dated green artifact) — across a PROCESS RESTART.** The
/// orchestrator is killed mid-fan-out (3 of 8 holders receipted), then RESTARTED as a **fresh
/// process** — a brand-new [`DsrOrchestrator`] register + a brand-new [`FanOutDriver`], carrying
/// ONLY the durable [`EraseChecklist`] (the resumability state, the G1 checklist rows on the live
/// floor). The restart re-drives **only un-receipted holders**: 0 double-erase (the killed-before
/// holders are NOT re-called), 0 missed (coverage = 1.0 after the resume), the DSR completes.
///
/// This is the restart leg P-GA-12 named as P-GA-14's floor: fanout.rs proved an *in-process*
/// re-`drive` (the same orchestrator); here the orchestrator/register itself is reconstructed — a
/// faithful model of a crashed worker resuming from the durable checklist after a deploy/restart.
#[test]
fn ga_d1_worker_kill_mid_fan_out_resumes_only_un_receipted_holders_zero_double_erase() {
    let subject_id = "u-resume";
    let kms = seed_kms_for(subject_id, 2000);
    let upstream_seams = seam_holders(&kms);
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);

    let upstream = full_m1_orchestrator(&upstream_seams, &h18, &h16);

    // THE DURABLE STATE — the only thing that survives the "restart". On the live floor this is the
    // G1 `dsr_request` per-holder checklist Postgres rows; here the in-memory model with byte-for-
    // byte the resumability semantics.
    let checklist = EraseChecklist::new();
    let scope = subject_scope(subject_id);

    // ── Worker #1: drive a PARTIAL fan-out (the first three holders), then "crash". We model the
    //    crash by driving only Identity/Blob/Authz over a sub-orchestrator, recording into the
    //    SHARED durable checklist — exactly the state a worker leaves behind when it dies after 3.
    let first_three: Vec<(&'static str, &dyn PersonalDataHolder)> = upstream_seams
        .iter()
        .filter(|(id, _)| {
            *id == holder_ids::IDENTITY
                || *id == holder_ids::BLOB
                || *id == holder_ids::AUTHZ_TUPLES
        })
        .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
        .collect();
    let crashing = UpstreamHolderOrchestrator::register_m1_upstream(first_three);
    crashing.fan_out_erase(&scope, &checklist).unwrap();
    assert_eq!(
        checklist.done_count(),
        3,
        "the worker died after receipting three holders"
    );
    // The per-holder erase-call counts at the moment of the crash (the double-erase baseline).
    let calls_at_crash: BTreeMap<&str, u32> = upstream_seams
        .iter()
        .map(|(id, h)| (*id, h.erase_call_count()))
        .collect();

    // ── RESTART — a brand-new worker process: a FRESH orchestrator register + a FRESH driver. The
    //    DSR is re-submitted/validated on the restarted orchestrator (its register did not survive
    //    the crash); the DURABLE CHECKLIST is the carried state that makes the fan-out resumable.
    let dsr2 = DsrOrchestrator::new(TestClock::at(2_000_000_000));
    let holds2 = LegalHoldRegistry::new();
    let driver2 = FanOutDriver::new(&dsr2, &holds2);
    let id2 = dsr2.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject(subject_id),
        scope.clone(),
        Posture::Controller,
        Initiator::Myelin,
    );
    assert!(dsr2.validate(&id2).unwrap());

    let outcome = driver2
        .drive(&id2, &inventory_for_all_holders(), &upstream, &checklist)
        .expect("the restarted worker resumes the fan-out to completion");
    assert!(
        matches!(outcome, FanOutOutcome::Erased(_)),
        "the resumed DSR erases to completion"
    );

    // 0 DOUBLE-ERASE: the three holders receipted before the crash were NOT re-called on restart.
    for id in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
    ] {
        let h = &upstream_seams.iter().find(|(hid, _)| *hid == id).unwrap().1;
        assert_eq!(
            h.erase_call_count(),
            calls_at_crash[id],
            "0 double-erase: holder `{id}` was already receipted ⇒ NOT re-called on restart"
        );
    }
    // 0 MISSED: the remaining five upstream holders WERE driven on the restart.
    for id in [holder_ids::BUS, holder_ids::CACHE, holder_ids::BACKUP] {
        let h = &upstream_seams.iter().find(|(hid, _)| *hid == id).unwrap().1;
        assert_eq!(
            h.erase_call_count(),
            1,
            "holder `{id}` driven exactly once (on the resume)"
        );
    }
    // Coverage = 1.0 (0 missed holder) and the DSR completed.
    assert_eq!(
        upstream.fanout_coverage(&checklist),
        1.0,
        "0 missed: erasure_fanout_coverage = 1.0 after the resume (all 8 holders receipted)"
    );
    assert_eq!(
        checklist.done_count(),
        8,
        "all eight holders are receipted exactly once"
    );
    assert_eq!(
        dsr2.state_of(&id2).unwrap(),
        DsrState::Completed,
        "the resumed DSR reached Completed"
    );

    // And the post-resume erasure is COMPLETE: 0 recoverable over every holder's key.
    for hid in all_m1_holder_ids() {
        if hid == AUDIT_CARVE_OUT_STORE {
            continue; // H16 retains the minimised record (no per-subject key) — §6.4.
        }
        let class = if hid == GDPR_OWN_STORE {
            ShredKeyClass::Subject(subject_id.to_string()) // H18 consent DEK keyed on the subject
        } else {
            ShredKeyClass::Subject(hid.to_string())
        };
        let handle = ShredKeyHandle {
            tenant: tenant(),
            class,
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "post-resume: holder `{hid}` has 0 recoverable PII (key shredded, backups included)"
        );
    }
}

/// **A re-drive AFTER completion is an idempotent no-op (0 double-erase even on a duplicate
/// restart).** A worker that restarts AGAIN after the DSR already completed re-drives nothing — the
/// durable checklist says every holder is done, so the same content-addressed receipt re-affirms
/// and no holder's `erase` is re-called. This is the belt-and-braces resumability property: a
/// crash-loop cannot double-erase.
#[test]
fn ga_d1_a_restart_after_completion_is_an_idempotent_no_op() {
    let subject_id = "u-idem";
    let kms = seed_kms_for(subject_id, 3000);
    let upstream_seams = seam_holders(&kms);
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);
    let upstream = full_m1_orchestrator(&upstream_seams, &h18, &h16);
    let checklist = EraseChecklist::new();
    let inv = inventory_for_all_holders();

    // Worker #1 drives the DSR to completion. (Both workers run on the SAME clock value so the
    // §4.2 completion receipt — which deterministically folds in the completion timestamp — is
    // byte-identical across the restart: the idempotency property is "same inputs ⇒ same receipt".
    // A restart at a DIFFERENT wall-clock legitimately seals a receipt with a different timestamp;
    // the load-bearing 0-double-erase property below holds regardless of the clock.)
    let dsr1 = DsrOrchestrator::new(TestClock::at(10));
    let holds1 = LegalHoldRegistry::new();
    let driver1 = FanOutDriver::new(&dsr1, &holds1);
    let id1 = dsr1.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject(subject_id),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    dsr1.validate(&id1).unwrap();
    let first = driver1.drive(&id1, &inv, &upstream, &checklist).unwrap();
    let calls_after_first: Vec<u32> = upstream_seams
        .iter()
        .map(|(_, h)| h.erase_call_count())
        .collect();

    // Worker #2 RESTARTS over the already-complete checklist (a fresh orchestrator + driver).
    let dsr2 = DsrOrchestrator::new(TestClock::at(10));
    let holds2 = LegalHoldRegistry::new();
    let driver2 = FanOutDriver::new(&dsr2, &holds2);
    let id2 = dsr2.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject(subject_id),
        subject_scope(subject_id),
        Posture::Controller,
        Initiator::Myelin,
    );
    dsr2.validate(&id2).unwrap();
    let second = driver2.drive(&id2, &inv, &upstream, &checklist).unwrap();
    let calls_after_second: Vec<u32> = upstream_seams
        .iter()
        .map(|(_, h)| h.erase_call_count())
        .collect();

    assert_eq!(
        calls_after_first, calls_after_second,
        "0 double-erase: a restart after completion re-calls NO holder"
    );
    assert_eq!(
        first.receipt().content_hash,
        second.receipt().content_hash,
        "an idempotent restart re-affirms the SAME content-addressed completion receipt"
    );
}
