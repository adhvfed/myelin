//! P-ST-06 (global P-058) GATE / DRILL — STOR-D6 (the KMS fail-static availability posture),
//! dated green artifact.
//!
//! **STOR-D6 (storage.md §4.5 / testing-strategy §4.2 row STOR-D6):** inject a KMS outage. A
//! TRANSIENT outage → resolved-DEK reads survive a bounded TTL (the availability win). A SUSTAINED
//! hard-down (past the staleness budget) → **not-ready + shed**, NEVER fail-open. The
//! load-bearing zeros: **0 fail-open** (no read returns a usable DEK without a fresh resolve or an
//! in-budget cache) and **0 plaintext-without-key** (a shed read carries a loud cause, never a
//! key/plaintext). Telemetry: the `fail_static` survival ratio + the `0 fail-open` counter.
//!
//! The drill injects the fault through the [`KmsAdapter`] seam (an outage-toggleable proxy over a
//! real [`KmsEngine`]) and asserts on the read path's own [`KmsFailStaticSignals`] — loudly: a
//! single read that returns a DEK past the budget `panic!`s (the threshold is NOT weakened to
//! pass, EI-01 §3). The bounded-staleness window is seeded from the thresholds-file engineering
//! seed (`static_max_default_secs = 300`, `agent_token_ttl_secs = 60` → a `fresh_ttl ≤ static_max
//! ≤ revocation-SLA` window); the VALUE W stays `[OPEN — LEGAL]` (DPO-ratified, named not
//! hardcoded green).

use std::sync::atomic::{AtomicBool, Ordering};

use myelin_storage::{
    DekHandle, KeyClass, KekId, KmsAdapter, KmsEngine, KmsError, KmsReadPath, KmsReadResult,
    KmsReadiness, PiiKeyRef,
};
use myelin_tenancy::{Region, TenantId};

/// The STOR-D6 fault-injection adapter: proxies a real [`KmsEngine`], but flips "down" to model a
/// transient/sustained KMS outage. When down, every resolve returns a LOUD [`KmsError`] — never a
/// fabricated key (the fault is "the KMS cannot answer", not "the KMS lies").
struct OutageInjectingKms {
    inner: KmsEngine,
    down: AtomicBool,
}

impl OutageInjectingKms {
    fn new(inner: KmsEngine) -> Self {
        OutageInjectingKms { inner, down: AtomicBool::new(false) }
    }
    fn inject_outage(&self) {
        self.down.store(true, Ordering::SeqCst);
    }
    fn recover(&self) {
        self.down.store(false, Ordering::SeqCst);
    }
}

impl KmsAdapter for OutageInjectingKms {
    fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        if self.down.load(Ordering::SeqCst) {
            Err(KmsError::KekUnavailable(KekId::new(key_ref.tenant.clone(), region.clone())))
        } else {
            self.inner.resolve_dek(key_ref, region)
        }
    }
}

/// Re-stated from the thresholds file (`thresholds.toml [fail_static]`): the engineering seed the
/// mechanism is drilled against — `static_max_default_secs = 300` (== the revocation SLA, the
/// largest the `static_max ≤ revocation-SLA` constraint admits) and a `fresh_ttl` well under it.
/// The ratified W is `[OPEN — LEGAL]`; this is the SEED, not the green-washed value.
const FRESH_TTL_SECS: u64 = 30;
const STATIC_MAX_SECS: u64 = 300;

/// **THE STOR-D6 drill.** A batch of resolved-DEK reads across a KMS outage: prove (a) a transient
/// outage within the budget keeps serving the resolved DEK (survival), (b) a sustained hard-down
/// past the budget sheds to not-ready, and (c) 0 fail-open + 0 plaintext-without-key throughout.
#[test]
fn stor_d6_kms_outage_fails_static_then_not_ready_zero_fail_open() {
    let kms = KmsEngine::new();
    let (tenant, region) = (TenantId("acme".into()), Region("eu-west".into()));
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));

    // A batch of per-subject DEKs (the 1x load unit) — each is a free-text/profile erasure class.
    const BATCH: usize = 16;
    let mut refs = Vec::with_capacity(BATCH);
    for i in 0..BATCH {
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject(format!("subject-{i}")))
            .expect("ensure per-subject DEK");
        refs.push(kr);
    }

    let path = KmsReadPath::with_clock(
        OutageInjectingKms::new(kms),
        FRESH_TTL_SECS,
        STATIC_MAX_SECS,
        myelin_storage::kms_failstatic::TestClock::at(0),
    );

    // 1) Warm every DEK with a fresh resolve (engine healthy).
    for kr in &refs {
        let out = path.resolve(kr, &region);
        assert!(out.is_resolved() && !out.is_degraded(), "warm read is fresh");
        assert_eq!(out.readiness(), KmsReadiness::Ready);
    }

    // 2) TRANSIENT outage within the budget — resolved-DEK reads SURVIVE (degraded, still serving).
    path.engine().inject_outage();
    path.clock().advance(FRESH_TTL_SECS + 60); // age 90 ≤ static_max(300): inside the budget
    let mut survived = 0usize;
    for kr in &refs {
        match path.resolve(kr, &region) {
            KmsReadResult::Resolved { degraded: true, .. } => survived += 1,
            // A served read MUST be degraded here (we are past fresh_ttl); a fresh/closed here is
            // a bug. A NotReady here would be a survival FAILURE (but not a safety breach).
            other => panic!("STOR-D6: expected degraded-survival within budget, got {other:?}"),
        }
    }
    assert_eq!(survived, BATCH, "every resolved-DEK read survived the transient outage");
    // Snapshot the peak survival staleness while still inside the budget (the recovery read below
    // re-freshes and zeroes last_staleness, so capture it HERE for the honest green artifact).
    let peak_staleness = path.signals().last_staleness_secs;
    assert!(peak_staleness > FRESH_TTL_SECS && peak_staleness <= STATIC_MAX_SECS,
        "the survival was served STALE within the budget (peak staleness {peak_staleness}s)");

    // 3) SUSTAINED hard-down PAST the budget — every read sheds to NOT-READY. A read that returns
    //    a usable DEK here is a FAIL-OPEN floor breach and panics loudly.
    path.clock().advance(STATIC_MAX_SECS); // age now > static_max → budget exhausted
    let mut shed = 0usize;
    for kr in &refs {
        match path.resolve(kr, &region) {
            KmsReadResult::NotReady(KmsError::KekUnavailable(_)) => shed += 1,
            KmsReadResult::Resolved { .. } => panic!(
                "STOR-D6 FLOOR BREACHED: a read returned a usable DEK past the staleness budget \
                 with the KMS hard-down — that is FAIL-OPEN (0-fail-open invariant violated)"
            ),
            other => panic!("STOR-D6: expected NotReady past budget, got {other:?}"),
        }
    }
    assert_eq!(shed, BATCH, "every read shed to not-ready past the budget");

    // 4) Recovery: the KMS comes back → reads are FRESH again (the hiccup degraded, did not cascade).
    path.engine().recover();
    let out = path.resolve(&refs[0], &region);
    assert!(out.is_resolved() && !out.is_degraded(), "recovered → fresh again");

    // THE green artifact: the survival ratio + the load-bearing zeros.
    let s = path.signals();
    assert_eq!(s.fail_open, 0, "0 fail-open across the whole outage");
    assert!(peak_staleness <= path.static_max(), "staleness never exceeded the budget");
    assert!(s.stale >= BATCH as u64, "the transient survivals were counted");
    assert!(s.not_ready >= BATCH as u64, "the past-budget sheds were counted");

    println!(
        "[P-058 DRILL GREEN 2026-06-19] STOR-D6 KMS fail-static: batch={BATCH} per-subject DEKs; \
         transient outage (age={}s <= static_max={}s) -> {survived} resolved-DEK reads SURVIVED \
         (fail_static stale-survival, peak_staleness={peak_staleness}s <= {}s); sustained \
         hard-down (age > static_max) -> {shed} reads NOT-READY+shed; fail_open={}, \
         plaintext_without_key=0 (NEVER fail open - storage.md section 4.5)",
        FRESH_TTL_SECS + 60,
        STATIC_MAX_SECS,
        path.static_max(),
        s.fail_open,
    );
}
