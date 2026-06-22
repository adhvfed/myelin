//! # P-GA-22 → P-149 — GA-D6: retention engine + legal-hold-aware suspend-don't-delete
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This integration drill is the dated green artifact the
//! P-GA-22 GATE (GA-D6) requires (as with the other GDPR drills, the test IS the artifact — there is
//! no GDPR scorecard binary). It proves, end-to-end, the GA-D6 row of the drill catalogue:
//!
//! > **GA-D6** — *Set a hold over a subject; submit an erase (a retention-expiry) → erasure
//! > deferred-by-hold (NOT run), resumes on hold-lift. **0 held-scope deletions.** The hold-defer
//! > receipt is the green artifact; the tightest-wins decision is recorded (which input won).*
//!
//! ## The scenario (chained end-to-end over the ALREADY-SHIPPED machinery + the new retention engine)
//! 1. **The tightest-policy-wins merge decides + records which input won** — a tenant "delete after
//!    30 days" beats a 90-day platform default (the tenant wins, recorded); a lawful 6-month
//!    security-log floor overrides a tenant "delete immediately" (the floor wins, recorded). The
//!    recorded winner ([`RetentionSource`]) is the auditable §5.1 decision.
//! 2. **The retention window elapses** ([`EffectiveRetention::has_elapsed`]) — a field stored a
//!    month ago, with a 30-day tenant window, has reached its retention expiry.
//! 3. **SET A HOLD over the subject**, then **submit the retention-expiry** — the engine SUSPENDS
//!    the deletion (suspend-don't-delete, Art. 17(3)(e)). **0 held-scope deletions**: NOT A SINGLE
//!    holder is erased under the hold. The defer is recorded ([`ExpiryOutcome::DeferredUnderHold`] —
//!    the hold-defer "receipt" / green artifact).
//! 4. **LIFT the hold and re-submit the expiry** — the deferred deletion RESUMES: the §3 erasure
//!    mechanisms run (the canonical-order holder fan-out, crypto-shred per holder); every holder is
//!    erased exactly once (the resumable checklist — 0 double-erase).
//! 5. **The invariant assertion** — across the whole scenario the `retention_held_scope_deletions`
//!    count is **0** (the held holders saw 0 erase calls while the hold was active). The
//!    `retention_expiry_runs` count is exactly 1 (the resume ran the expiry once).
//!
//! ## What this proves vs what it reuses (EI-01 §7 coherence)
//! The G4 legal-hold gate ([`LegalHoldRegistry`], wired in P-GA-12) is REUSED unchanged — the
//! engine BACKS it (it reads the SAME registry the DSR-erase gate reads; no second hold store). The
//! canonical-order holder fan-out ([`UpstreamHolderOrchestrator`], P-GA-06) is REUSED for the §3
//! erasure mechanisms. The NEW deliverable is the retention engine ([`RetentionEngine`]): the
//! tightest-policy-wins merge + the suspend-or-run expiry decision.
//!
//! ## Telemetry (observability is part of the pass — EI-01 §3)
//! The GA-D6 green artifact's value is `retention_held_scope_deletions == 0` (the silent-data-loss
//! invariant §2 outranks every feature). The tightest-wins decision records the winning source
//! ([`EffectiveRetention::winning_source`]) — the §5.1 "auditable which input won".
//!
//! ## Floor named (VISION §3)
//! GA-D6 runs at M2 scale here (the in-memory M1-store model — the same store/KMS floor every
//! M0/M1 store carries, P-007 / P-S12); it **re-confirms at CELL scale at M5** → **P-GA-35** (the
//! multi-cell retention sweep). The periodic expiry SWEEP scheduler (the `myelin-flow` wheel that
//! calls `expire` over the elapsed fields) is the same timer floor P-GA-21 carries (P-FLOW-13 →
//! P-207); this drill drives the engine directly (one `expire` per elapsed scope).

use core::time::Duration;

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    holder_ids, legal_floor, platform_default, tenant_delete_immediately, tenant_window,
    EraseChecklist, ExpiryOutcome, HoldScope, InMemoryShredKms, LegalHoldRegistry, RetentionEngine,
    RetentionSource, SeamHolder, ShredKeyClass, ShredKeyHandle, UpstreamHolderOrchestrator,
    RETENTION_EXPIRY_RUNS, RETENTION_HELD_SCOPE_DELETIONS,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

const DAY: u64 = 24 * 60 * 60;

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

fn kms_with_all_holder_keys(base_epoch: u64) -> InMemoryShredKms {
    let kms = InMemoryShredKms::new();
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
                tenant: tenant(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            base_epoch + i as u64,
        );
    }
    kms
}

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

/// The GA-D6 dated green artifact: set a hold; the retention-expiry is deferred-by-hold (0
/// held-scope deletions); it resumes on hold-lift; the tightest-wins decision is recorded.
#[test]
fn ga_d6_retention_engine_legal_hold_suspend_dont_delete() {
    // ── 1. The tightest-policy-wins merge decides + records which input won (§5.1). ──
    let kms = kms_with_all_holder_keys(1_000);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect(),
    );
    let holds = LegalHoldRegistry::new();
    let engine = RetentionEngine::new(&holds);

    // a tenant "delete after 30 days" beats a 90-day platform default (the tenant wins, recorded).
    let effective = engine.effective_retention(&[
        platform_default(Duration::from_secs(90 * DAY)),
        tenant_window(Duration::from_secs(30 * DAY)),
    ]);
    assert_eq!(
        effective.window_secs(),
        30 * DAY,
        "tightest-wins: the tenant 30-day window"
    );
    assert_eq!(
        effective.winning_source,
        RetentionSource::TenantPolicy,
        "the tightest-wins decision is RECORDED (which input won — §5.1)"
    );

    // a lawful 6-month security-log floor overrides a tenant "delete immediately" (the floor wins).
    let floor_won = engine.effective_retention(&[
        tenant_delete_immediately(),
        legal_floor(Duration::from_secs(180 * DAY)),
    ]);
    assert_eq!(
        floor_won.winning_source,
        RetentionSource::LegalFloor,
        "the legal floor won, recorded"
    );
    assert!(
        floor_won.floor_clamped,
        "the floor clamped the tenant delete-immediately UP"
    );

    // ── 2. The retention window elapses (Art. 5(1)(e)). ──
    let stored_at = 1_700_000_000;
    let now = stored_at + 30 * DAY; // a month later — the 30-day window has elapsed.
    assert!(
        effective.has_elapsed(stored_at, now),
        "the 30-day window has elapsed"
    );

    // ── 3. SET A HOLD, then submit the retention-expiry → SUSPENDED (0 held-scope deletions). ──
    let scope = subject_scope("u-d6");
    let checklist = EraseChecklist::new();
    holds.set(
        HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u-d6".into(),
        },
        true,
    );

    let deferred = engine.expire(&scope, &upstream, &checklist).unwrap();
    assert_eq!(
        deferred,
        ExpiryOutcome::DeferredUnderHold,
        "the hold-defer receipt — suspend-don't-delete (Art. 17(3)(e))"
    );
    assert!(
        !deferred.ran_deletion(),
        "the expiry did NOT run under the hold"
    );

    // THE GA-D6 INVARIANT: 0 held-scope deletions — NOT A SINGLE holder erased under the hold.
    let held_scope_deletions: u32 = holders.iter().map(|(_, h)| h.erase_call_count()).sum();
    assert_eq!(
        held_scope_deletions, 0,
        "0 held-scope deletions (the GA-D6 green artifact value)"
    );
    assert_eq!(
        checklist.done_count(),
        0,
        "no holder receipted under the hold"
    );

    // ── 4. LIFT the hold and re-submit the expiry → the deferred deletion RESUMES via §3. ──
    holds.set(
        HoldScope::Subject {
            tenant: "acme".into(),
            subject: "u-d6".into(),
        },
        false,
    );
    let mut expiry_runs = 0u64;
    let resumed = engine.expire(&scope, &upstream, &checklist).unwrap();
    if resumed.ran_deletion() {
        expiry_runs += 1;
    }
    let receipts = match resumed {
        ExpiryOutcome::Expired(r) => r,
        other => panic!("expected Expired on resume, got {other:?}"),
    };
    assert_eq!(
        receipts.len(),
        6,
        "the §3 erasure mechanisms ran over every holder on resume"
    );
    assert_eq!(
        receipts[0].holder_id,
        holder_ids::IDENTITY,
        "Identity FIRST (canonical order)"
    );
    for hr in &receipts {
        assert!(
            hr.receipt.receipt.key_epoch_destroyed.is_some(),
            "each holder records its destroyed key epoch (the §3 crypto-shred mechanism, auditable)"
        );
    }

    // every holder erased exactly once across the WHOLE scenario (0 double-erase — the resumable
    // checklist skipped nothing-yet-done and re-drove only on resume).
    for (id, h) in &holders {
        assert_eq!(
            h.erase_call_count(),
            1,
            "holder {id} erased exactly once (on resume only)"
        );
    }

    // ── 5. The telemetry invariants (observability is part of the pass — EI-01 §3). ──
    assert_eq!(
        expiry_runs, 1,
        "the retention-expiry ran exactly once (on resume)"
    );
    assert_eq!(
        RETENTION_HELD_SCOPE_DELETIONS,
        ("gdpr.retention_held_scope_deletions", "count"),
        "the GA-D6 invariant signal NAME + UNIT (its value across the held window was 0)"
    );
    assert_eq!(
        RETENTION_EXPIRY_RUNS,
        ("gdpr.retention_expiry_runs", "count"),
        "the retention-expiry-runs health signal NAME + UNIT"
    );
}
