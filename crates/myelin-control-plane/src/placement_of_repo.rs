use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

use crate::placement_of::{CellGateway, GatewayReject, Misroute, MisrouteAuditRecord};
use crate::registry::{PlacementError, Registry};
use crate::schema::PlacementStatus;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageGroup(String);

impl StorageGroup {
    #[inline]
    pub fn from_token(token: impl Into<String>) -> StorageGroup {
        StorageGroup(token.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoPlacement {
    pub cell_id: CellId,
    pub group: StorageGroup,
    pub region: Region,
    pub status: PlacementStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoPlacementRow {
    pub(crate) cell_id: CellId,
    pub(crate) group: StorageGroup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoPlacementError {
    NotARepoRef { repo: ArtifactRef },
    TenantNotPlaced { repo: ArtifactRef, tenant: TenantId },
    Invariant(PlacementError),
}

impl std::fmt::Display for RepoPlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoPlacementError::NotARepoRef { repo } => write!(
                f,
                "repo placement REJECTED: `{}` is not a `myelin://<tenant>/git/repo/<id>` reference \
                 - there is no tenant to pin the repo's residency to (§5.2).",
                repo.0
            ),
            RepoPlacementError::TenantNotPlaced { repo, tenant } => write!(
                f,
                "repo placement REJECTED: repo `{}` - its tenant `{}` is not placed; a repo cannot be \
                 homed onto a region of record that does not exist (fail-closed, §5.2).",
                repo.0,
                tenant.as_str()
            ),
            RepoPlacementError::Invariant(e) => write!(
                f,
                "repo placement REJECTED by the placement invariant (the residency pin holds at repo \
                 grain - a repo cannot move out of its tenant's region): {e}"
            ),
        }
    }
}

impl std::error::Error for RepoPlacementError {}

fn parse_repo_ref(repo: &ArtifactRef) -> Option<(TenantId, &str)> {
    let rest = repo.0.strip_prefix("myelin://")?;
    if rest.contains('#') {
        return None;
    }
    let segments: Vec<&str> = rest.split('/').collect();
    if segments.len() != 4 {
        return None;
    }
    let (tenant, subsystem, type_, id) = (segments[0], segments[1], segments[2], segments[3]);
    if tenant.is_empty() || id.is_empty() || subsystem != "git" || type_ != "repo" {
        return None;
    }
    Some((TenantId::from_token(tenant), id))
}

impl Registry {
    pub fn register_repo(
        &mut self,
        repo: &ArtifactRef,
        group: StorageGroup,
    ) -> Result<(), RepoPlacementError> {
        let (tenant, _id) = parse_repo_ref(repo)
            .ok_or_else(|| RepoPlacementError::NotARepoRef { repo: repo.clone() })?;
        let placement =
            self.placement(&tenant)
                .ok_or_else(|| RepoPlacementError::TenantNotPlaced {
                    repo: repo.clone(),
                    tenant: tenant.clone(),
                })?;
        let home_cell = placement.home_cell.clone();
        let region = placement.region.clone();
        self.assert_cell_in_region(&home_cell, &region, repo)?;
        self.upsert_repo_placement_row(
            &repo.0,
            &tenant,
            RepoPlacementRow {
                cell_id: home_cell,
                group,
            },
        );
        Ok(())
    }

    pub fn relocate_repo(
        &mut self,
        repo: &ArtifactRef,
        target_cell: CellId,
        target_group: StorageGroup,
    ) -> Result<(), RepoPlacementError> {
        let (tenant, _id) = parse_repo_ref(repo)
            .ok_or_else(|| RepoPlacementError::NotARepoRef { repo: repo.clone() })?;
        let placement =
            self.placement(&tenant)
                .ok_or_else(|| RepoPlacementError::TenantNotPlaced {
                    repo: repo.clone(),
                    tenant: tenant.clone(),
                })?;
        let region = placement.region.clone();
        self.assert_cell_in_region(&target_cell, &region, repo)?;
        self.upsert_repo_placement_row(
            &repo.0,
            &tenant,
            RepoPlacementRow {
                cell_id: target_cell,
                group: target_group,
            },
        );
        Ok(())
    }

    pub fn placement_of_repo(&self, repo: &ArtifactRef) -> Option<RepoPlacement> {
        let (tenant, _id) = parse_repo_ref(repo)?;
        let row = self.repo_placement_row(&repo.0)?;
        let tenant_placement = self.placement(&tenant)?;
        Some(RepoPlacement {
            cell_id: row.cell_id.clone(),
            group: row.group.clone(),
            region: tenant_placement.region.clone(),
            status: tenant_placement.status,
        })
    }

    fn assert_cell_in_region(
        &self,
        cell_id: &CellId,
        region: &Region,
        repo: &ArtifactRef,
    ) -> Result<(), RepoPlacementError> {
        match self.cell(cell_id) {
            None => Err(RepoPlacementError::Invariant(PlacementError::UnknownCell {
                tenant: parse_repo_ref(repo)
                    .map(|(t, _)| t)
                    .unwrap_or_else(|| TenantId::from_token(repo.0.clone())),
                cell: cell_id.clone(),
            })),
            Some(cell) if &cell.region != region => Err(RepoPlacementError::Invariant(
                PlacementError::CrossRegionMemberCell {
                    tenant: parse_repo_ref(repo)
                        .map(|(t, _)| t)
                        .unwrap_or_else(|| TenantId::from_token(repo.0.clone())),
                    tenant_region: region.clone(),
                    cell: cell_id.clone(),
                    cell_region: cell.region.clone(),
                },
            )),
            Some(_) => Ok(()),
        }
    }
}

impl CellGateway {
    pub fn route_repo(
        &self,
        registry: &Registry,
        repo: &ArtifactRef,
    ) -> Result<RepoPlacement, GatewayReject> {
        let Some(placement) = registry.placement_of_repo(repo) else {
            self.record_repo_misroute(repo, registry, None);
            return Err(GatewayReject::NoSuchTenant {
                tenant_id: self.repo_tenant_or_placeholder(repo),
            });
        };

        if placement.cell_id == *self.cell_id() {
            return Ok(placement);
        }

        let correct_cell = placement.cell_id.clone();
        self.record_repo_misroute(repo, registry, Some(correct_cell.clone()));
        let correct_cell_endpoint = registry
            .cell(&correct_cell)
            .map(|c| c.endpoint.clone())
            .unwrap_or_else(|| format!("cell-unresolved:{}", correct_cell.as_str()));
        Err(GatewayReject::Misroute(Misroute {
            tenant_id: self.repo_tenant_or_placeholder(repo),
            correct_cell,
            correct_cell_endpoint,
        }))
    }

    fn repo_tenant_or_placeholder(&self, repo: &ArtifactRef) -> TenantId {
        parse_repo_ref(repo)
            .map(|(t, _)| t)
            .unwrap_or_else(|| TenantId::from_token(repo.0.clone()))
    }

    fn record_repo_misroute(
        &self,
        repo: &ArtifactRef,
        _registry: &Registry,
        home_cell: Option<CellId>,
    ) {
        self.bump_misroute_count();
        self.audit().record_misroute(MisrouteAuditRecord {
            tenant_id: self.repo_tenant_or_placeholder(repo),
            received_by_cell: self.cell_id().clone(),
            home_cell,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };

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

    fn repo(tenant: &str, id: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/git/repo/{id}"))
    }

    fn registry_with_repo() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        reg.insert_cell(cell("cell-n-1", "eu-north"));
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("single-region placement admitted");
        reg.register_repo(&repo("01J0ACME", "web"), StorageGroup::from_token("pack-0"))
            .expect("a repo on its tenant's home cell is registered");
        reg
    }

    #[test]
    fn placement_of_repo_returns_the_repo_grain_tuple() {
        let reg = registry_with_repo();
        let answer = reg
            .placement_of_repo(&repo("01J0ACME", "web"))
            .expect("a registered repo resolves to a placement_of_repo answer");
        assert_eq!(
            answer.cell_id.as_str(),
            "cell-w-1",
            "cell_id defaults to the tenant home cell"
        );
        assert_eq!(answer.group.as_str(), "pack-0");
        assert_eq!(
            answer.region.as_str(),
            "eu-west",
            "region is the TENANT's region (the pin)"
        );
        assert_eq!(answer.status, PlacementStatus::Active);
    }

    #[test]
    fn placement_of_repo_unregistered_is_none() {
        let reg = registry_with_repo();
        assert!(reg.placement_of_repo(&repo("01J0ACME", "ghost")).is_none());
    }

    #[test]
    fn placement_of_repo_malformed_ref_is_none() {
        let reg = registry_with_repo();
        assert!(reg
            .placement_of_repo(&ArtifactRef("myelin://01J0ACME/git/blob/web".into()))
            .is_none());
        assert!(reg
            .placement_of_repo(&ArtifactRef("not-a-ref".into()))
            .is_none());
    }

    #[test]
    fn register_repo_rejects_empty_tenant_or_id_segments() {
        let mut reg = registry_with_repo();
        let empty_tenant = ArtifactRef("myelin:///git/repo/web".into());
        assert!(
            matches!(
                reg.register_repo(&empty_tenant, StorageGroup::from_token("g")),
                Err(RepoPlacementError::NotARepoRef { .. })
            ),
            "an empty tenant segment is not a repo ref (no residency pin)"
        );
        assert!(reg.placement_of_repo(&empty_tenant).is_none());
        let empty_id = ArtifactRef("myelin://01J0ACME/git/repo/".into());
        assert!(
            matches!(
                reg.register_repo(&empty_id, StorageGroup::from_token("g")),
                Err(RepoPlacementError::NotARepoRef { .. })
            ),
            "an empty repo-id segment is not a repo ref"
        );
        assert!(reg.placement_of_repo(&empty_id).is_none());
    }

    #[test]
    fn register_repo_unplaced_tenant_is_refused() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        let e = reg
            .register_repo(
                &repo("01J0GHOST", "web"),
                StorageGroup::from_token("pack-0"),
            )
            .expect_err("a repo for an unplaced tenant is refused");
        assert!(
            matches!(e, RepoPlacementError::TenantNotPlaced { .. }),
            "{e}"
        );
    }

    #[test]
    fn register_repo_non_repo_ref_is_refused() {
        let mut reg = registry_with_repo();
        let e = reg
            .register_repo(
                &ArtifactRef("myelin://01J0ACME/git/blob/web".into()),
                StorageGroup::from_token("g"),
            )
            .expect_err("a non-repo ref is refused");
        assert!(matches!(e, RepoPlacementError::NotARepoRef { .. }), "{e}");
    }

    #[test]
    fn repo_relocation_does_not_recompute_a_hash() {
        let mut reg = registry_with_repo();
        let r = repo("01J0ACME", "web");
        let before = reg.placement_of_repo(&r).expect("placed");
        assert_eq!(before.cell_id.as_str(), "cell-w-1");

        reg.relocate_repo(
            &r,
            CellId::from_token("cell-w-2"),
            StorageGroup::from_token("pack-7"),
        )
        .expect("a same-region relocation is admitted");

        let after = reg
            .placement_of_repo(&r)
            .expect("still placed after relocation");
        assert_eq!(
            after.cell_id.as_str(),
            "cell-w-2",
            "only the stored cell_id flipped"
        );
        assert_eq!(
            after.group.as_str(),
            "pack-7",
            "the group moved to the target cell's group"
        );
        assert_eq!(
            after.region.as_str(),
            "eu-west",
            "region UNCHANGED - same-region move (the pin)"
        );
        let r_after = repo("01J0ACME", "web");
        assert_eq!(
            r, r_after,
            "the repo's clone URL identity is unchanged by relocation"
        );
    }

    #[test]
    fn relocate_repo_cross_region_is_rejected() {
        let mut reg = registry_with_repo();
        let r = repo("01J0ACME", "web");
        let e = reg
            .relocate_repo(
                &r,
                CellId::from_token("cell-n-1"),
                StorageGroup::from_token("g"),
            )
            .expect_err(
                "a cross-region relocation target is rejected (the residency pin at repo grain)",
            );
        assert!(
            matches!(
                e,
                RepoPlacementError::Invariant(PlacementError::CrossRegionMemberCell { .. })
            ),
            "{e}"
        );
        let still = reg.placement_of_repo(&r).expect("still placed");
        assert_eq!(
            still.cell_id.as_str(),
            "cell-w-1",
            "the rejected relocation did not move the repo"
        );
        assert_eq!(still.region.as_str(), "eu-west");
    }

    #[test]
    fn relocate_repo_unknown_cell_is_rejected() {
        let mut reg = registry_with_repo();
        let e = reg
            .relocate_repo(
                &repo("01J0ACME", "web"),
                CellId::from_token("cell-ghost"),
                StorageGroup::from_token("g"),
            )
            .expect_err("an unknown target cell is refused");
        assert!(
            matches!(
                e,
                RepoPlacementError::Invariant(PlacementError::UnknownCell { .. })
            ),
            "{e}"
        );
    }

    #[test]
    fn gateway_accepts_a_repo_it_hosts() {
        let reg = registry_with_repo();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let answer = gw
            .route_repo(&reg, &repo("01J0ACME", "web"))
            .expect("the home cell serves its own repo");
        assert_eq!(answer.cell_id.as_str(), "cell-w-1");
        assert_eq!(gw.misroute_count(), 0, "an accept is not a misroute");
        assert_eq!(gw.audit().count(), 0);
        assert_eq!(gw.cross_tenant_reads(), 0);
    }

    #[test]
    fn gateway_redirects_a_relocated_repo() {
        let mut reg = registry_with_repo();
        let r = repo("01J0ACME", "web");
        reg.relocate_repo(
            &r,
            CellId::from_token("cell-w-2"),
            StorageGroup::from_token("pack-7"),
        )
        .expect("same-region relocation");

        let old = CellGateway::new(CellId::from_token("cell-w-1"));
        let reject = old
            .route_repo(&reg, &r)
            .expect_err("cell-w-1 no longer homes the relocated repo → REJECTED (not proxied)");

        assert_eq!(
            reject,
            GatewayReject::Misroute(Misroute {
                tenant_id: TenantId::from_token("01J0ACME"),
                correct_cell: CellId::from_token("cell-w-2"),
                correct_cell_endpoint: "cell.eu-west.cell-w-2.myelin.eu".into(),
            }),
            "the redirect points at the relocated repo's CURRENT cell-endpoint"
        );
        assert_eq!(old.audit().count(), 1, "the misroute is audited");
        assert_eq!(
            old.audit().records()[0],
            MisrouteAuditRecord {
                tenant_id: TenantId::from_token("01J0ACME"),
                received_by_cell: CellId::from_token("cell-w-1"),
                home_cell: Some(CellId::from_token("cell-w-2")),
            }
        );
        assert_eq!(
            old.misroute_count(),
            1,
            "misroute_count increments on a repo-grain misroute"
        );
        assert_eq!(
            old.cross_tenant_reads(),
            0,
            "0 cross-tenant/cross-cell rows read (the GIT zero)"
        );

        let GatewayReject::Misroute(redirect) = reject else {
            panic!("expected a misroute")
        };
        let current = CellGateway::new(redirect.correct_cell.clone());
        let served = current
            .route_repo(&reg, &r)
            .expect("the current cell serves the redirected clone");
        assert_eq!(served.cell_id, redirect.correct_cell);
        assert_eq!(
            current.misroute_count(),
            0,
            "the current cell does not misroute its own repo"
        );
        assert_eq!(current.cross_tenant_reads(), 0);
    }

    #[test]
    fn gateway_cross_tenant_repo_misroute_reads_zero() {
        let mut reg = registry_with_repo();
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0BETA"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-2"),
            isolation_tier: IsolationKind::Pool,
            slug: "beta".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-2")],
        })
        .expect("placed");
        reg.register_repo(&repo("01J0BETA", "api"), StorageGroup::from_token("pack-0"))
            .expect("repo");

        let gw = CellGateway::new(CellId::from_token("cell-w-2"));
        let reject = gw
            .route_repo(&reg, &repo("01J0ACME", "web"))
            .expect_err("cell-w-2 does not home ACME's repo → rejected, never served");
        assert!(matches!(reject, GatewayReject::Misroute(_)));
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "0 cross-tenant repo content read (the GIT residency zero)"
        );
        assert_eq!(gw.misroute_count(), 1);
    }

    #[test]
    fn gateway_rejects_an_unregistered_repo_with_no_redirect() {
        let reg = registry_with_repo();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let reject = gw
            .route_repo(&reg, &repo("01J0ACME", "ghost"))
            .expect_err("an unregistered repo is rejected (no route)");
        assert_eq!(
            reject,
            GatewayReject::NoSuchTenant {
                tenant_id: TenantId::from_token("01J0ACME")
            }
        );
        assert_eq!(
            gw.audit().count(),
            1,
            "the unregistered-repo rejection is audited"
        );
        assert_eq!(
            gw.audit().records()[0].home_cell,
            None,
            "no redirect target for a stale clone URL"
        );
        assert_eq!(gw.misroute_count(), 1);
        assert_eq!(gw.cross_tenant_reads(), 0);
    }
}
