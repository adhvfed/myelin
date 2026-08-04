use myelin_tenancy::{Region, TenantId};

use crate::registry::Registry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorTarget {
    pub host: String,
    pub region: Region,
}

impl MirrorTarget {
    pub fn new(host: impl Into<String>, region: Region) -> MirrorTarget {
        MirrorTarget {
            host: host.into(),
            region,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorDecision {
    Allow {
        reason: MirrorAllowReason,
    },
    Deny {
        reason: MirrorDenyReason,
    },
}

impl MirrorDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, MirrorDecision::Allow { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorAllowReason {
    SameRegion {
        region: Region,
    },
    LawfulTransfer {
        tenant_region: Region,
        target_region: Region,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorDenyReason {
    NoLawfulTransfer {
        tenant_region: Region,
        target_region: Region,
    },
    UnknownTenant {
        tenant_id: TenantId,
    },
}

pub trait TransferPolicy {
    fn transfer_allowed(&self, target: &Region) -> bool;
}

#[derive(Default)]
pub struct MirrorGate {
    unauthorised_pushes_prevented: u64,
}

impl MirrorGate {
    pub fn new() -> MirrorGate {
        MirrorGate::default()
    }

    pub fn mirror_allowed(
        &mut self,
        registry: &Registry,
        tenant_id: &TenantId,
        mirror_target: &MirrorTarget,
        policy: &dyn TransferPolicy,
    ) -> MirrorDecision {
        let Some(placement) = registry.placement(tenant_id) else {
            return MirrorDecision::Deny {
                reason: MirrorDenyReason::UnknownTenant {
                    tenant_id: tenant_id.clone(),
                },
            };
        };
        let tenant_region = placement.region.clone();

        if mirror_target.region == tenant_region {
            return MirrorDecision::Allow {
                reason: MirrorAllowReason::SameRegion {
                    region: tenant_region,
                },
            };
        }

        if policy.transfer_allowed(&mirror_target.region) {
            MirrorDecision::Allow {
                reason: MirrorAllowReason::LawfulTransfer {
                    tenant_region,
                    target_region: mirror_target.region.clone(),
                },
            }
        } else {
            self.unauthorised_pushes_prevented += 1;
            MirrorDecision::Deny {
                reason: MirrorDenyReason::NoLawfulTransfer {
                    tenant_region,
                    target_region: mirror_target.region.clone(),
                },
            }
        }
    }

    pub fn unauthorised_pushes_prevented(&self) -> u64 {
        self.unauthorised_pushes_prevented
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{IsolationKind, PlacementStatus, TenantPlacement};
    use myelin_tenancy::CellId;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    struct PolicyDouble {
        allowed: RefCell<BTreeSet<String>>,
    }

    impl PolicyDouble {
        fn new() -> PolicyDouble {
            let mut allowed = BTreeSet::new();
            for r in ["fr-par", "nl-ams", "de-fra", "no-osl"] {
                allowed.insert(r.to_string());
            }
            PolicyDouble {
                allowed: RefCell::new(allowed),
            }
        }
        fn record_mechanism(&self, region: &str) {
            self.allowed.borrow_mut().insert(region.to_string());
        }
    }

    impl TransferPolicy for PolicyDouble {
        fn transfer_allowed(&self, target: &Region) -> bool {
            self.allowed.borrow().contains(target.as_str())
        }
    }

    fn registry_with(tenant: &str, region: &str, home: &str) -> Registry {
        use crate::schema::{Capacity, Cell, CellStatus};
        let mut reg = Registry::new();
        reg.insert_cell(Cell {
            cell_id: CellId::from_token(home),
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
            endpoint: format!("cell.{region}.{home}.myelin.eu"),
        });
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
        reg
    }

    #[test]
    fn extra_eu_target_denied_by_default() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();

        let target = MirrorTarget::new("github.com", Region::new("us-east"));
        let decision =
            gate.mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy);

        assert_eq!(
            decision,
            MirrorDecision::Deny {
                reason: MirrorDenyReason::NoLawfulTransfer {
                    tenant_region: Region::new("fr-par"),
                    target_region: Region::new("us-east"),
                },
            },
            "an extra-EU PII-bearing target with no transfer_allowed entry is denied by default (loud)"
        );
        assert!(!decision.is_allowed(), "the caller must NOT push on a Deny");
        assert_eq!(
            gate.unauthorised_pushes_prevented(),
            1,
            "the prevented unauthorised cross-residency push is counted (the C-4 zero)"
        );
    }

    #[test]
    fn extra_eu_target_allowed_only_with_recorded_lawful_basis() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();
        let target = MirrorTarget::new("github.com", Region::new("us-east"));

        assert!(
            !gate
                .mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy)
                .is_allowed(),
            "denied by default before a lawful basis is recorded"
        );

        policy.record_mechanism("us-east");
        let decision =
            gate.mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy);
        assert_eq!(
            decision,
            MirrorDecision::Allow {
                reason: MirrorAllowReason::LawfulTransfer {
                    tenant_region: Region::new("fr-par"),
                    target_region: Region::new("us-east"),
                },
            },
            "a crossing WITH a recorded transfer mechanism is permitted"
        );
        assert_eq!(gate.unauthorised_pushes_prevented(), 1);
    }

    #[test]
    fn same_region_target_allowed_without_policy_consult() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let mut gate = MirrorGate::new();

        struct NeverConsulted;
        impl TransferPolicy for NeverConsulted {
            fn transfer_allowed(&self, _target: &Region) -> bool {
                panic!("the same-region path must NOT consult the transfer policy (no crossing)");
            }
        }

        let target = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
        let decision = gate.mirror_allowed(
            &reg,
            &TenantId::from_token("01J0ACME"),
            &target,
            &NeverConsulted,
        );
        assert_eq!(
            decision,
            MirrorDecision::Allow {
                reason: MirrorAllowReason::SameRegion {
                    region: Region::new("fr-par"),
                },
            },
            "a same-region mirror crosses no boundary - allowed without a policy consult"
        );
        assert_eq!(
            gate.unauthorised_pushes_prevented(),
            0,
            "a same-region allow prevents nothing - the zero holds"
        );
    }

    #[test]
    fn within_eu_cross_region_target_allowed_via_policy() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();

        let target = MirrorTarget::new("mirror.nl.example", Region::new("nl-ams"));
        let decision =
            gate.mirror_allowed(&reg, &TenantId::from_token("01J0ACME"), &target, &policy);
        assert_eq!(
            decision,
            MirrorDecision::Allow {
                reason: MirrorAllowReason::LawfulTransfer {
                    tenant_region: Region::new("fr-par"),
                    target_region: Region::new("nl-ams"),
                },
            },
            "within-EU acceleration (a different EU region) is permitted by the policy (§5.3)"
        );
        assert_eq!(
            gate.unauthorised_pushes_prevented(),
            0,
            "a policy-permitted crossing prevents nothing"
        );
    }

    #[test]
    fn unknown_tenant_fails_closed() {
        let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
        let policy = PolicyDouble::new();
        let mut gate = MirrorGate::new();

        let target = MirrorTarget::new("git.acme.internal.fr", Region::new("fr-par"));
        let decision =
            gate.mirror_allowed(&reg, &TenantId::from_token("01J0GHOST"), &target, &policy);
        assert_eq!(
            decision,
            MirrorDecision::Deny {
                reason: MirrorDenyReason::UnknownTenant {
                    tenant_id: TenantId::from_token("01J0GHOST"),
                },
            },
            "a tenant with no placement of record cannot mirror - fail closed"
        );
        assert!(!decision.is_allowed());
    }
}
