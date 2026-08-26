use super::*;

const MAX_PENDING_MERGES: usize = 10_000;
const MAX_PENDING_MERGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RETAINED_OUTBOX_ROWS: usize = 100_000;
const MAX_RETAINED_OUTBOX_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitBootRecoveryReport {
    pub repos_reconciled: usize,
    pub refs_reapplied: usize,
    pub merges_recovered: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitCellBootRecoveryReport {
    pub tenants_recovered: usize,
    pub repos_reconciled: usize,
    pub refs_reapplied: usize,
    pub merges_recovered: usize,
}

pub async fn recover_placed_git_at_boot(
    backend: &DurableGitBackend,
    provider: &SubstrateProvider,
    cell_id: &str,
) -> Result<GitCellBootRecoveryReport, DurableError> {
    if cell_id.trim().is_empty() {
        return Err(DurableError::Io("Git recovery cell id is empty".into()));
    }
    let placements = DurablePlacementBacking::new(provider.db_pool().clone());
    let local = placements
        .local_tenants(cell_id)
        .await
        .map_err(|_| DurableError::Io("read local tenant recovery directory".into()))?;
    let mut report = GitCellBootRecoveryReport::default();
    for entry in local.into_iter().filter(|entry| entry.active) {
        if entry.cell_id != cell_id {
            return Err(DurableError::Io(
                "local tenant recovery directory returned a foreign cell".into(),
            ));
        }
        let placement = placements
            .get_placement(&entry.tenant_id)
            .await
            .map_err(|_| DurableError::Io("read tenant recovery placement".into()))?
            .ok_or_else(|| {
                DurableError::Io("active local tenant has no canonical placement".into())
            })?;
        if placement.status != "Active" {
            return Err(DurableError::Io(
                "active local tenant has a non-active canonical placement".into(),
            ));
        }
        if placement.tenant_id != entry.tenant_id
            || placement.region != provider.config().region
            || (placement.home_cell != cell_id
                && !placement
                    .member_cells
                    .iter()
                    .any(|member| member == cell_id))
        {
            return Err(DurableError::Io(
                "active local tenant recovery placement does not match this cell/region".into(),
            ));
        }
        let principal = Principal::new(
            TenantId(entry.tenant_id),
            Region(placement.region),
            PrincipalId(format!("git-recovery:{cell_id}")),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let tenant_report = backend.recover_tenant_at_boot(&scope, &principal)?;
        report.tenants_recovered += 1;
        report.repos_reconciled += tenant_report.repos_reconciled;
        report.refs_reapplied += tenant_report.refs_reapplied;
        report.merges_recovered += tenant_report.merges_recovered;
    }
    Ok(report)
}

impl DurableGitBackend {
    pub fn reconcile_repo(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
    ) -> Result<myelin_git::reconcile::ReconcileReport, DurableError> {
        let records = myelin_git::reconcile::refs_from_outbox_scoped_bounded(
            &self.outbox,
            tenant,
            region,
            slug,
            MAX_RETAINED_OUTBOX_ROWS,
            MAX_RETAINED_OUTBOX_BYTES,
        )?;
        self.reconcile_repo_records(tenant, region, slug, &records)
    }

    fn reconcile_repo_records(
        &self,
        tenant: &str,
        region: &str,
        slug: &str,
        records: &[myelin_git::reconcile::GitRefUpdatedRecord],
    ) -> Result<myelin_git::reconcile::ReconcileReport, DurableError> {
        let loc = Self::loc(tenant, region, slug);
        let repo = self.store.open_repo(&loc)?;
        myelin_git::reconcile::reconcile_refs(&repo, records)
    }

    pub fn recover_tenant_at_boot(
        &self,
        scope: &TenantScope,
        recovery_principal: &Principal,
    ) -> Result<GitBootRecoveryReport, DurableError> {
        if recovery_principal.tenant != *scope.tenant()
            || recovery_principal.region != *scope.region()
        {
            return Err(DurableError::NotFound("repository partition".into()));
        }
        let tenant = scope.tenant().0.as_str();
        let region = scope.region().0.as_str();
        let pending = match &self.pg_prs {
            Some(store) => store.list_pending_merges_bounded(
                scope,
                MAX_PENDING_MERGES,
                MAX_PENDING_MERGE_BYTES,
            )?,
            None => Vec::new(),
        };
        let mut ref_records = myelin_git::reconcile::refs_by_repo_from_outbox_scoped_bounded(
            &self.outbox,
            tenant,
            region,
            MAX_RETAINED_OUTBOX_ROWS,
            MAX_RETAINED_OUTBOX_BYTES,
        )?;
        let mut repos = ref_records
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        repos.extend(pending.iter().map(|item| item.repo_slug.clone()));

        let mut report = GitBootRecoveryReport::default();
        for slug in &repos {
            let records = ref_records.remove(slug).unwrap_or_default();
            let reconciled = self.reconcile_repo_records(tenant, region, slug, &records)?;
            report.repos_reconciled += 1;
            report.refs_reapplied += reconciled.reapplied.len();
        }

        if let Some(store) = &self.pg_prs {
            for item in pending {
                let loc = Self::loc(tenant, region, &item.repo_slug);
                let repo = Arc::new(self.store.open_repo(&loc)?);
                let ref_store = self.open_durable_refstore(
                    repo.clone(),
                    &item.repo_slug,
                    tenant,
                    region,
                    recovery_principal,
                )?;
                if store
                    .recover_pending_merge_target(
                        scope,
                        &item.repo_slug,
                        item.number,
                        recovery_principal,
                        &loc,
                        &repo,
                        &ref_store,
                    )?
                    .is_some()
                {
                    report.merges_recovered += 1;
                }
            }
        }
        Ok(report)
    }
}
