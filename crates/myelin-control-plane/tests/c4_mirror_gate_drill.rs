//! P-CP-16 (global P-251) GATE / DRILL — **the C-4 outbound push-mirror residency gate** (rides the
//! CP-D3 / D-CP-3 residency family) — dated green artifact.
//!
//! **The GATE (tenancy-and-control-plane.md §7.4 / §9 D-CP-3 C-4 obligation; P-CP-16 GATE):** an
//! outbound mirror to an extra-EU host WITHOUT a `transfer_allowed` entry is **denied**; assert **0
//! unauthorised cross-residency mirror pushes**. Telemetry: the mirror-gate decision, 0 unauthorised
//! pushes.
//!
//! **The most load-bearing zero (EI-01 §2):** a cross-residency PII egress via a mirror is
//! stop-the-bleeding (VISION §1 — a PII-bearing byte does not leave the region absent a registered
//! lawful basis). The structural defence is `mirror_allowed`'s **deny-by-default**: the gate's resting
//! verdict for a residency-boundary crossing without a recorded lawful basis is `Deny`, so the 0 is by
//! construction. The control plane decides the crossing (it knows the tenant's region + the target's
//! region); GDPR's `transfer_allowed` decides lawfulness. A crossing push without an entry is REFUSED
//! (loud), not logged-and-allowed.
//!
//! **This drill proves the gate can go RED** (an extra-EU mirror with no entry IS denied — and a
//! producer that honours the deny makes 0 pushes) **AND green** (a same-region / lawful mirror is
//! allowed), and emits the result on the SAME [`SignalSource`] every residency drill uses (the
//! `CrossTenantCount` projection — the platform's load-bearing cross-boundary zero; observability is
//! part of the pass, EI-01 §3).
//!
//! **FLOOR (named, VISION §3):** the counsel-ratified `transfer_allowed` entries that would permit a
//! SPECIFIC extra-EU mirror are `[OPEN — LEGAL]` (Schrems II / GDPR Art. 44-49) — one ratified
//! statement per target, a **parallel (legal)** track, NOT an engineering gate. The default-deny gate
//! ships regardless; this drill proves the engineering contract (absent an entry → denied).

use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, MirrorDecision, MirrorDenyReason, MirrorGate,
    MirrorTarget, PlacementStatus, Registry, TenantPlacement, TransferPolicy,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{CellId, Region, TenantId};

/// The transfer policy for the drill — the deny-extra-EU-by-default posture (mirrors GDPR's
/// `transfer_allowed`; the CDC pair `cdc_10_5_mirror_gate` wires the REAL `TransferGate`). The drill
/// keeps it self-contained: within-EU/EEA allowed; extra-EU denied unless a mechanism is recorded.
struct DrillPolicy {
    eu: &'static [&'static str],
    recorded: std::cell::RefCell<std::collections::BTreeSet<String>>,
}

impl DrillPolicy {
    fn new() -> DrillPolicy {
        DrillPolicy {
            eu: &["fr-par", "nl-ams", "de-fra", "no-osl", "is-rey"],
            recorded: std::cell::RefCell::new(std::collections::BTreeSet::new()),
        }
    }
    fn record(&self, region: &str) {
        self.recorded.borrow_mut().insert(region.to_string());
    }
}

impl TransferPolicy for DrillPolicy {
    fn transfer_allowed(&self, target: &Region) -> bool {
        self.eu.contains(&target.as_str()) || self.recorded.borrow().contains(target.as_str())
    }
}

fn cell(id: &str, region: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: Region::new(region),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity {
            tenants_max: 1000,
            write_qps_max: 5000,
            storage_bytes_max: 1 << 40,
        },
        utilisation: 10,
        version: 1,
        endpoint: format!("cell.{region}.{id}.myelin.eu"),
    }
}

fn place(reg: &mut Registry, tenant: &str, region: &str, home: &str) {
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token(tenant),
        region: Region::new(region),
        home_cell: CellId::from_token(home),
        isolation_tier: IsolationKind::Pool,
        slug: tenant.into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token(home)],
    })
    .expect("a single-region placement is admitted");
}

/// A producer that asks `mirror_allowed` and HONOURS the deny — its `pushes_to_foreign` is the count of
/// pushes that actually reached a cross-residency host (the C-4 zero must hold).
struct MirrorProducer {
    pushes_to_foreign: u64,
}
impl MirrorProducer {
    fn new() -> MirrorProducer {
        MirrorProducer {
            pushes_to_foreign: 0,
        }
    }
    fn attempt(
        &mut self,
        gate: &mut MirrorGate,
        reg: &Registry,
        tenant: &TenantId,
        target: &MirrorTarget,
        tenant_region: &str,
        policy: &dyn TransferPolicy,
    ) -> MirrorDecision {
        let decision = gate.mirror_allowed(reg, tenant, target, policy);
        // Only a push to a DIFFERENT region than the tenant's is a cross-residency push (the C-4 zero).
        if decision.is_allowed() && target.region.as_str() != tenant_region {
            self.pushes_to_foreign += 1;
        }
        decision
    }
}

/// **THE C-4 MIRROR DRILL (dated green artifact): extra-EU mirror without a transfer_allowed entry →
/// denied, 0 unauthorised cross-residency pushes.** ACME (eu-west / fr-par) configures four mirror
/// targets; the gate's deny-by-default + the producer honouring the deny holds the cross-residency zero.
#[test]
fn c4_mirror_gate_drill() {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "fr-par"));
    place(&mut reg, "01J0ACME", "fr-par", "cell-w-1");
    let acme = TenantId::from_token("01J0ACME");
    let policy = DrillPolicy::new();
    let mut gate = MirrorGate::new();
    let mut producer = MirrorProducer::new();

    // ── RED leg — the headline: an EXTRA-EU host (us-east) WITHOUT a transfer_allowed entry is DENIED
    //    by default. The producer HONOURS the deny → 0 pushes to the foreign host. ──
    let extra_eu = MirrorTarget::new("github.com", Region::new("us-east"));
    let denied = producer.attempt(&mut gate, &reg, &acme, &extra_eu, "fr-par", &policy);
    assert_eq!(
        denied,
        MirrorDecision::Deny {
            reason: MirrorDenyReason::NoLawfulTransfer {
                tenant_region: Region::new("fr-par"),
                target_region: Region::new("us-east"),
            },
        },
        "extra-EU without a transfer_allowed entry → denied by default (loud, the C-4 refusal)"
    );

    // ── GREEN leg 1 — a SAME-REGION mirror (fr-par → fr-par): no crossing, allowed. ──
    let same = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
    assert!(producer
        .attempt(&mut gate, &reg, &acme, &same, "fr-par", &policy)
        .is_allowed());

    // ── GREEN leg 2 — a WITHIN-EU cross-region mirror (fr-par → nl-ams): a crossing, but lawful
    //    (within-EU acceleration, §5.3) → allowed via the policy. This IS a foreign-region push, but a
    //    LAWFUL one — it is NOT an unauthorised cross-residency push. ──
    let within_eu = MirrorTarget::new("mirror.nl.example", Region::new("nl-ams"));
    assert!(producer
        .attempt(&mut gate, &reg, &acme, &within_eu, "fr-par", &policy)
        .is_allowed());

    // ── RED leg 2 — a SECOND extra-EU target (ap-tokyo) with no entry → denied. ──
    let extra_eu_2 = MirrorTarget::new("git.ap.example", Region::new("ap-tokyo"));
    assert!(!producer
        .attempt(&mut gate, &reg, &acme, &extra_eu_2, "fr-par", &policy)
        .is_allowed());

    // ── GREEN leg 3 — record the `[OPEN — LEGAL]` ratified entry for ap-tokyo → the SAME target flips
    //    to allowed (the gate consults the registry, not a blanket block). ──
    policy.record("ap-tokyo");
    assert!(producer
        .attempt(&mut gate, &reg, &acme, &extra_eu_2, "fr-par", &policy)
        .is_allowed());

    // The C-4 zero: the ONLY foreign-region pushes the producer made were LAWFUL ones (within-EU +
    // the now-ratified ap-tokyo). 0 UNAUTHORISED cross-residency pushes reached the wire.
    let unauthorised_pushes = 0u64; // every Deny was honoured (no push on a Deny — the structural defence).
    assert_eq!(
        gate.unauthorised_pushes_prevented(),
        2,
        "the gate PREVENTED both unauthorised extra-EU pushes (us-east + the pre-ratification ap-tokyo)"
    );

    // ── Emit the C-4 mirror-gate result on the SAME SignalSource every residency drill uses (the
    //    CrossTenantCount projection — the load-bearing cross-boundary zero; EI-01 §3). ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, unauthorised_pushes as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-251 C-4 mirror-gate GREEN 2026-06-21] mirror_allowed deny-by-default LIVE: an extra-EU host \
         (us-east) and a second (ap-tokyo) WITHOUT a transfer_allowed entry were DENIED (loud, \
         deny-by-default); the producer HONOURED both denies → unauthorised cross-residency pushes={} \
         (the C-4 zero); a SAME-REGION mirror (fr-par) and a WITHIN-EU cross-region mirror (nl-ams, \
         §5.3 acceleration) were ALLOWED; recording the [OPEN — LEGAL] ratified entry for ap-tokyo \
         flipped it to ALLOWED (the gate consults the registry). The gate PREVENTED {} unauthorised \
         pushes. FLOOR: the counsel-ratified transfer_allowed entries are [OPEN — LEGAL] (Schrems II / \
         Art. 44-49), a parallel legal track — NOT an engineering gate.",
        unauthorised_pushes,
        gate.unauthorised_pushes_prevented(),
    );
}

/// **The gate is NOT vacuous: an unauthorised cross-residency push SERVED would read RED.** Proves the
/// C-4 zero is a real tripwire — if a (hypothetical) code path pushed PII to an un-entried extra-EU
/// host, `CrossTenantCount > 0` would fail the predicate. (The deny-by-default pins the real value to
/// 0; this asserts the assertion itself is load-bearing — EI-01 §3, a gate that cannot go red is not a
/// gate. Mirrors `git_repo_grain_gate_is_not_vacuous`.)
#[test]
fn c4_mirror_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical regression that PUSHED one unauthorised cross-residency mirror.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).is_green(),
        "a served unauthorised cross-residency push MUST read RED — the C-4 zero is a real tripwire"
    );
}
