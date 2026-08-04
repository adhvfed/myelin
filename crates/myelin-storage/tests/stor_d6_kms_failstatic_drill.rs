use std::sync::atomic::{AtomicBool, Ordering};

use myelin_storage::{
    DekHandle, KekId, KeyClass, KmsAdapter, KmsEngine, KmsError, KmsReadPath, KmsReadResult,
    KmsReadiness, PiiKeyRef,
};
use myelin_tenancy::{Region, TenantId};

struct OutageInjectingKms {
    inner: KmsEngine,
    down: AtomicBool,
}

impl OutageInjectingKms {
    fn new(inner: KmsEngine) -> Self {
        OutageInjectingKms {
            inner,
            down: AtomicBool::new(false),
        }
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
            Err(KmsError::KekUnavailable(KekId::new(
                key_ref.tenant.clone(),
                region.clone(),
            )))
        } else {
            self.inner.resolve_dek(key_ref, region)
        }
    }
}

const FRESH_TTL_SECS: u64 = 30;
const STATIC_MAX_SECS: u64 = 300;

#[test]
fn stor_d6_kms_outage_fails_static_then_not_ready_zero_fail_open() {
    let kms = KmsEngine::new();
    let (tenant, region) = (TenantId("acme".into()), Region("eu-west".into()));
    kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));

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

    for kr in &refs {
        let out = path.resolve(kr, &region);
        assert!(
            out.is_resolved() && !out.is_degraded(),
            "warm read is fresh"
        );
        assert_eq!(out.readiness(), KmsReadiness::Ready);
    }

    path.engine().inject_outage();
    path.clock().advance(FRESH_TTL_SECS + 60);
    let mut survived = 0usize;
    for kr in &refs {
        match path.resolve(kr, &region) {
            KmsReadResult::Resolved { degraded: true, .. } => survived += 1,
            other => panic!("STOR-D6: expected degraded-survival within budget, got {other:?}"),
        }
    }
    assert_eq!(
        survived, BATCH,
        "every resolved-DEK read survived the transient outage"
    );
    let peak_staleness = path.signals().last_staleness_secs;
    assert!(
        peak_staleness > FRESH_TTL_SECS && peak_staleness <= STATIC_MAX_SECS,
        "the survival was served STALE within the budget (peak staleness {peak_staleness}s)"
    );

    path.clock().advance(STATIC_MAX_SECS);
    let mut shed = 0usize;
    for kr in &refs {
        match path.resolve(kr, &region) {
            KmsReadResult::NotReady(KmsError::KekUnavailable(_)) => shed += 1,
            KmsReadResult::Resolved { .. } => panic!(
                "STOR-D6 FLOOR BREACHED: a read returned a usable DEK past the staleness budget \
                 with the KMS hard-down - that is FAIL-OPEN (0-fail-open invariant violated)"
            ),
            other => panic!("STOR-D6: expected NotReady past budget, got {other:?}"),
        }
    }
    assert_eq!(shed, BATCH, "every read shed to not-ready past the budget");

    path.engine().recover();
    let out = path.resolve(&refs[0], &region);
    assert!(
        out.is_resolved() && !out.is_degraded(),
        "recovered → fresh again"
    );

    let s = path.signals();
    assert_eq!(s.fail_open, 0, "0 fail-open across the whole outage");
    assert!(
        peak_staleness <= path.static_max(),
        "staleness never exceeded the budget"
    );
    assert!(
        s.stale >= BATCH as u64,
        "the transient survivals were counted"
    );
    assert!(
        s.not_ready >= BATCH as u64,
        "the past-budget sheds were counted"
    );

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
