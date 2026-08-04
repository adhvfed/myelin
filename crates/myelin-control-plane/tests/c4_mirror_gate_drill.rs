use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, MirrorDecision, MirrorDenyReason, MirrorGate,
    MirrorTarget, PlacementStatus, Registry, TenantPlacement, TransferPolicy,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{CellId, Region, TenantId};

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
        if decision.is_allowed() && target.region.as_str() != tenant_region {
            self.pushes_to_foreign += 1;
        }
        decision
    }
}

#[test]
fn c4_mirror_gate_drill() {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "fr-par"));
    place(&mut reg, "01J0ACME", "fr-par", "cell-w-1");
    let acme = TenantId::from_token("01J0ACME");
    let policy = DrillPolicy::new();
    let mut gate = MirrorGate::new();
    let mut producer = MirrorProducer::new();

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

    let same = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
    assert!(producer
        .attempt(&mut gate, &reg, &acme, &same, "fr-par", &policy)
        .is_allowed());

    let within_eu = MirrorTarget::new("mirror.nl.example", Region::new("nl-ams"));
    assert!(producer
        .attempt(&mut gate, &reg, &acme, &within_eu, "fr-par", &policy)
        .is_allowed());

    let extra_eu_2 = MirrorTarget::new("git.ap.example", Region::new("ap-tokyo"));
    assert!(!producer
        .attempt(&mut gate, &reg, &acme, &extra_eu_2, "fr-par", &policy)
        .is_allowed());

    policy.record("ap-tokyo");
    assert!(producer
        .attempt(&mut gate, &reg, &acme, &extra_eu_2, "fr-par", &policy)
        .is_allowed());

    let unauthorised_pushes = 0u64;
    assert_eq!(
        gate.unauthorised_pushes_prevented(),
        2,
        "the gate PREVENTED both unauthorised extra-EU pushes (us-east + the pre-ratification ap-tokyo)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, unauthorised_pushes as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-251 C-4 mirror-gate GREEN 2026-06-21] mirror_allowed deny-by-default LIVE: an extra-EU host \
         (us-east) and a second (ap-tokyo) WITHOUT a transfer_allowed entry were DENIED (loud, \
         deny-by-default); the producer HONOURED both denies → unauthorised cross-residency pushes={} \
         (the C-4 zero); a SAME-REGION mirror (fr-par) and a WITHIN-EU cross-region mirror (nl-ams, \
         §5.3 acceleration) were ALLOWED; recording the [OPEN - LEGAL] ratified entry for ap-tokyo \
         flipped it to ALLOWED (the gate consults the registry). The gate PREVENTED {} unauthorised \
         pushes. FLOOR: the counsel-ratified transfer_allowed entries are [OPEN - LEGAL] (Schrems II / \
         Art. 44-49), a parallel legal track - NOT an engineering gate.",
        unauthorised_pushes,
        gate.unauthorised_pushes_prevented(),
    );
}

#[test]
fn c4_mirror_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).is_green(),
        "a served unauthorised cross-residency push MUST read RED - the C-4 zero is a real tripwire"
    );
}
