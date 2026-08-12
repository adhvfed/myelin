use std::collections::BTreeMap;

use myelin_identity::{Decision, IdentityService, Permission, Principal, Zookie};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, CellId, CrossCellPointer, Region, TenantId};

use crate::pseudonym_erase::ErasureReceipt;
use crate::StoreBackedCheck;

pub struct CellPartition {
    cell_id: CellId,
    region: Region,
    engine: StoreBackedCheck,
}

impl CellPartition {
    pub fn new(cell_id: CellId, region: Region, engine: StoreBackedCheck) -> CellPartition {
        CellPartition {
            cell_id,
            region,
            engine,
        }
    }

    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn engine(&self) -> &StoreBackedCheck {
        &self.engine
    }

    pub fn current_zookie(&self) -> Zookie {
        self.engine.current_zookie()
    }

    fn resolve_local(
        &self,
        viewer: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
    ) -> (Decision, Zookie) {
        let zookie = self.current_zookie();
        let at = myelin_identity::Consistency {
            at_least: zookie.clone(),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        let decision = self
            .engine
            .check(viewer, permission, object, &at, None)
            .unwrap_or(Decision::Deny);
        (decision, zookie)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossCellResolution {
    Projection { home_cell: CellId, zookie: Zookie },
    Tombstone { home_cell: CellId },
}

impl CrossCellResolution {
    pub fn is_authorized(&self) -> bool {
        matches!(self, CrossCellResolution::Projection { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellAudit {
    pub viewer_cell: CellId,
    pub home_cell: CellId,
    pub cross_region_tuple_pulls: usize,
    pub cell_local: bool,
}

impl CrossCellAudit {
    pub fn is_pii_free(&self) -> bool {
        self.cross_region_tuple_pulls == 0 && self.cell_local
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellGrant {
    pub home_cell: CellId,
    pub decision: Decision,
    pub zookie: Zookie,
}

impl CrossCellGrant {
    pub fn is_bounded_allow(&self) -> bool {
        self.decision == Decision::Allow && !self.zookie.0.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReceipt {
    pub tenant: TenantId,
    pub from_cell: CellId,
    pub to_cell: CellId,
    pub region: Region,
    pub authority_before: usize,
    pub authority_after: usize,
    pub authority_lost: usize,
}

impl MigrationReceipt {
    pub fn is_green(&self) -> bool {
        self.authority_lost == 0 && self.authority_before == self.authority_after
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCellDsrReceiptSet {
    pub subject: myelin_identity::PrincipalId,
    pub tenant: TenantId,
    pub member_cells: Vec<CellId>,
    pub per_cell: Vec<(CellId, ErasureReceipt)>,
    pub ran_at: myelin_events::Timestamp,
}

impl MultiCellDsrReceiptSet {
    pub fn is_complete(&self) -> bool {
        self.per_cell.len() == self.member_cells.len()
            && self
                .member_cells
                .iter()
                .all(|c| self.per_cell.iter().any(|(rc, _)| rc == c))
    }

    pub fn cells_missed(&self) -> usize {
        self.member_cells
            .iter()
            .filter(|c| !self.per_cell.iter().any(|(rc, _)| &rc == c))
            .count()
    }

    pub fn summary(&self) -> String {
        format!(
            "GA-D8 per-cell DSR receipt set [{}]: subject={} tenant={} member_cells={} \
             receipts={} cells_missed={} → {}",
            self.ran_at.0,
            self.subject.0,
            self.tenant.0,
            self.member_cells.len(),
            self.per_cell.len(),
            self.cells_missed(),
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

#[derive(Default)]
pub struct MultiCellAuthority {
    cells: BTreeMap<CellId, CellPartition>,
}

impl MultiCellAuthority {
    pub fn new() -> MultiCellAuthority {
        MultiCellAuthority {
            cells: BTreeMap::new(),
        }
    }

    pub fn register_cell(&mut self, partition: CellPartition) {
        self.cells.insert(partition.cell_id.clone(), partition);
    }

    pub fn cell(&self, cell_id: &CellId) -> Option<&CellPartition> {
        self.cells.get(cell_id)
    }

    pub fn cell_ids(&self) -> Vec<CellId> {
        self.cells.keys().cloned().collect()
    }

    pub fn resolve_cross_cell(
        &self,
        viewer_cell: &CellId,
        viewer: &Principal,
        pointer: &CrossCellPointer,
        permission: &Permission,
        object: &ArtifactRef,
    ) -> (CrossCellResolution, CrossCellAudit) {
        let home_cell = pointer.home_cell().clone();
        let resolution = match self.cells.get(&home_cell) {
            None => CrossCellResolution::Tombstone {
                home_cell: home_cell.clone(),
            },
            Some(partition) => {
                let (decision, zookie) = partition.resolve_local(viewer, permission, object);
                match decision {
                    Decision::Allow => CrossCellResolution::Projection {
                        home_cell: home_cell.clone(),
                        zookie,
                    },
                    _ => CrossCellResolution::Tombstone {
                        home_cell: home_cell.clone(),
                    },
                }
            }
        };
        let audit = CrossCellAudit {
            viewer_cell: viewer_cell.clone(),
            home_cell,
            cross_region_tuple_pulls: 0,
            cell_local: true,
        };
        (resolution, audit)
    }

    pub fn read_through_coarse_grant(
        &self,
        viewer: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        home_cell: &CellId,
    ) -> CrossCellGrant {
        let (decision, zookie) = match self.cells.get(home_cell) {
            None => (Decision::Deny, Zookie(String::new())),
            Some(partition) => partition.resolve_local(viewer, permission, object),
        };
        CrossCellGrant {
            home_cell: home_cell.clone(),
            decision,
            zookie,
        }
    }

    pub fn migrate_cell(
        &self,
        tenant: &TenantId,
        from_cell: &CellId,
        to_cell: &CellId,
        grants: &[(Principal, Permission, ArtifactRef)],
    ) -> MigrationReceipt {
        let from = self.cells.get(from_cell);
        let to = self.cells.get(to_cell);
        let (from, to) = match (from, to) {
            (Some(f), Some(t)) => (f, t),
            _ => {
                return MigrationReceipt {
                    tenant: tenant.clone(),
                    from_cell: from_cell.clone(),
                    to_cell: to_cell.clone(),
                    region: from
                        .map(|f| f.region.clone())
                        .or_else(|| to.map(|t| t.region.clone()))
                        .unwrap_or(Region(String::new())),
                    authority_before: grants.len(),
                    authority_after: 0,
                    authority_lost: grants.len(),
                };
            }
        };
        if from.region != to.region {
            return MigrationReceipt {
                tenant: tenant.clone(),
                from_cell: from_cell.clone(),
                to_cell: to_cell.clone(),
                region: from.region.clone(),
                authority_before: grants.len(),
                authority_after: 0,
                authority_lost: grants.len(),
            };
        }
        let authority_before = grants
            .iter()
            .filter(|(v, p, o)| from.resolve_local(v, p, o).0 == Decision::Allow)
            .count();
        let authority_after = grants
            .iter()
            .filter(|(v, p, o)| {
                from.resolve_local(v, p, o).0 == Decision::Allow
                    && to.resolve_local(v, p, o).0 == Decision::Allow
            })
            .count();
        let authority_lost = authority_before.saturating_sub(authority_after);
        MigrationReceipt {
            tenant: tenant.clone(),
            from_cell: from_cell.clone(),
            to_cell: to_cell.clone(),
            region: from.region.clone(),
            authority_before,
            authority_after,
            authority_lost,
        }
    }

    pub fn dsr_erase_across_cells(
        &self,
        subject: &myelin_identity::PrincipalId,
        tenant: &TenantId,
        home_cell: &CellId,
        member_cells: &[CellId],
        now: myelin_events::Timestamp,
    ) -> MultiCellDsrReceiptSet {
        let mut fan_out: Vec<CellId> = Vec::new();
        for c in std::iter::once(home_cell).chain(member_cells.iter()) {
            if !fan_out.contains(c) {
                fan_out.push(c.clone());
            }
        }
        let mut per_cell = Vec::with_capacity(fan_out.len());
        for cell_id in &fan_out {
            if let Some(partition) = self.cells.get(cell_id) {
                let scope_principal = Principal::new(
                    tenant.clone(),
                    partition.region.clone(),
                    subject.clone(),
                    myelin_identity::PrincipalKind::Human,
                    myelin_identity::DataRole::Controller,
                    myelin_identity::PrincipalStatus::Active,
                );
                let scope =
                    TenantScope::from_verified_token(&scope_principal, partition.region.clone());
                let receipt = partition.engine.erase_in(&scope, subject, now.clone());
                per_cell.push((cell_id.clone(), receipt));
            }
        }
        MultiCellDsrReceiptSet {
            subject: subject.clone(),
            tenant: tenant.clone(),
            member_cells: fan_out,
            per_cell,
            ran_at: now,
        }
    }
}
