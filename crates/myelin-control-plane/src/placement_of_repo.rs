//! # `placement_of(repo)` — repo-granular placement (C-1): region-pinned, relocatable, NEVER node-pinned
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §5.2 (**repo-granular placement, SHARPENED — C-1**: `placement_of(repo: ArtifactRef) → {cell_id,
//! group, region, status}`; a repo's `cell_id` is its tenant's `home_cell` (single-cell) / the member
//! cell that homes its workload (multi-cell, M5); `group` is the repo-storage group within the cell
//! (Storage 11.2 object-backed pack tier); **region-pinned + relocatable, NEVER node-pinned** — the
//! placement is a *stored fact* (like tenant placement) that can be relocated within its region without
//! a hash recompute, but is never derived from a node hash on the hot path; the git wire
//! `git@<cell-endpoint>:tenant/repo.git` discovers + gets a **misroute redirect** if relocated),
//! §7.3 (the git-wire discovery — re-discovers on a misroute redirect at repo grain). Contract-index
//! row 12.2 (`placement_of(repo)` repo-grain, C-1). Reconciliation §10 (the C-1 rationale).
//!
//! ## What this prompt (P-CP-15 / P-250) ships
//! 1. **`placement_of(repo: &ArtifactRef) → RepoPlacement {cell_id, group, region, status}`**
//!    ([`Registry::placement_of_repo`]) — the LIVE repo-grain routing answer. A repo's `region` is its
//!    **tenant's** region (residency stays pinned to the tenant — a repo NEVER leaves its tenant's
//!    region); `cell_id` is a **stored fact** that defaults to the tenant's `home_cell` and is
//!    relocatable within-region; `group` is the repo-storage group within the cell; `status` is the
//!    repo's placement lifecycle. It is a *routing* answer, never an authz answer (no
//!    principal/permission/grant on [`RepoPlacement`] by construction).
//! 2. **Region-pinned + relocatable, never node-pinned** ([`Registry::register_repo`] /
//!    [`Registry::relocate_repo`]) — the repo's `cell_id` is stored, not hashed: registering a repo
//!    stores `{cell_id = tenant.home_cell, group}`; [`relocate_repo`] moves the repo to a DIFFERENT
//!    in-region cell **without recomputing any hash** (a stored-fact update). A relocation to a
//!    cross-region cell is REJECTED (the residency pin holds at repo grain). The clone URL identity
//!    (`tenant/repo`) is unchanged by a relocation — the cell is rediscovered, the URL is not a node pin.
//! 3. **The repo-grain misroute redirect** ([`CellGateway::route_repo`]) — a git-wire request arriving
//!    at a cell for a repo whose `placement_of(repo).cell_id` is a DIFFERENT cell is **REJECTED** (not
//!    proxied) with a [`Misroute`] redirect to the current cell-endpoint, audited (PII-free), reading
//!    **0** cross-tenant/cross-cell rows. `misroute_count` increments; `cross_tenant_reads` stays 0.
//!    This is the GIT residency leg of CP-D2/CP-D3 (rides GIT-D8) at repo grain.
//!
//! ## A repo's residency is its TENANT's residency (the load-bearing pin — §5.2 / EI-04 §1)
//! `placement_of(repo).region` is read from the repo's **tenant_placement** row, NEVER stored
//! independently. This is what makes "a repo's data stays in its region" structural: a repo cannot be
//! placed (or relocated) onto a cell in a different region than its tenant, because the only region of
//! record is the tenant's (immutable) region. Relocation is therefore a **same-region** move by
//! construction — [`relocate_repo`] re-runs the placement invariant (the target cell must be in the
//! tenant's region) and refuses a cross-region target. There is no repo-grain region field to drift.
//!
//! ## Never node-pinned (§5.2 — the property the git wire needs)
//! A repo's `cell_id` is a **stored fact** keyed by the repo's opaque [`ArtifactRef`], not a function of
//! `hash(repo) mod cell_set`. The `repo_relocation_does_not_recompute_a_hash` test pins this: relocating
//! a repo flips ONLY the stored `cell_id`; the repo's identity (its `ArtifactRef` / clone URL `tenant/repo`)
//! is byte-identical before and after. A node-hash placement would move the repo on every cell-set change
//! (forbidden — §5.2); a stored fact moves a repo ONLY when an operator relocates it.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The repo-relocation misroute-redirect path ([`CellGateway::route_repo`] +
//! [`Registry::placement_of_repo`] + [`Registry::relocate_repo`]) is mandatory-core: a cross-tenant
//! repo read is stop-the-bleeding (EI-01 §2). The floor is **>= 80%**; the load-bearing mutants — the
//! `placement.cell_id == self.cell_id` accept-vs-reject branch, the unknown-repo fail-closed branch, the
//! `misroute_count` increment, the redirect-endpoint resolution, the relocation cross-region reject, and
//! the tenant-region read (residency pin) — are each killed by an assertion in the unit + drill tests.
//! `cross_tenant_reads` is a documented equivalent-mutant tripwire (NEVER incremented by construction;
//! the `git_repo_grain_gate_is_not_vacuous` drill proves a non-zero value WOULD read RED), exactly as in
//! the tenant-grain [`super::placement_of`].
//!
//! ## Floor named (deferred body → filling prompt) — VISION §3 name-your-floors
//! - **The relocation MECHANISM (the actual data move) is the M5 build (P-CP-22).** This prompt ships the
//!   live repo-grain routing ANSWER + the redirect-on-relocation PROPERTY: [`relocate_repo`] flips the
//!   stored `cell_id` (the control-plane fact) and the gateway redirects accordingly; the durable
//!   workflow that COPIES the repo's bytes (reindex-from-source + crypto-shred cut-over) is P-CP-22. The
//!   routing contract is complete now and does not change shape when the move mechanism lands.
//! - **Multi-cell repo homing (a member cell that is not the home cell) is M5 (P-CP-19/P-CP-20).** In v1
//!   a repo's default cell is its tenant's single `home_cell`; the member-cell-by-aggregate sharding is
//!   the M5 follow-on. The [`RepoPlacement::cell_id`] field is general (any in-region cell), so the shape
//!   is frozen; v1 placements default to the home cell.

use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

use crate::placement_of::{CellGateway, GatewayReject, Misroute, MisrouteAuditRecord};
use crate::registry::{PlacementError, Registry};
use crate::schema::PlacementStatus;

/// **The repo-storage group within a cell (architecture §5.2; Storage 11.2 object-backed pack tier).**
/// PII-free — an opaque storage-group label (the placement of the repo's object-backed pack tier
/// inside its cell), never personal data. A repo's `group` is a stored fact alongside its `cell_id`;
/// relocation may change the group (the target cell's group) without changing the repo's identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageGroup(String);

impl StorageGroup {
    /// Construct a `StorageGroup` from an opaque storage-group token (never personal data — same
    /// opaqueness discipline as [`CellId::from_token`]).
    #[inline]
    pub fn from_token(token: impl Into<String>) -> StorageGroup {
        StorageGroup(token.into())
    }

    /// The opaque storage-group token as a string slice (a routing/placement label — no PII inside).
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// **The `placement_of(repo)` answer (architecture §5.2, C-1; contract 12.2).** The PII-free repo-grain
/// ROUTING answer: `{cell_id, group, region, status}`. It carries **no** authz answer — no principal,
/// no permission, no grant (routing ≠ authorization). Every field is an opaque id / storage group /
/// region code / status enum — PII-free by construction.
///
/// **`region` is the repo's TENANT's region** (§5.2): a repo never leaves its tenant's region — the
/// region is read from the tenant placement, never stored independently, so there is no repo-grain
/// region to drift across a relocation. `cell_id` is a **stored fact** (relocatable within-region,
/// never a node hash); `group` is the repo-storage group within the cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoPlacement {
    /// The cell that homes the repo's workload — a **stored fact** (defaults to the tenant's
    /// `home_cell`, relocatable within-region, NEVER a node hash). Opaque id, PII-free.
    pub cell_id: CellId,
    /// The repo-storage group within the cell (Storage 11.2 object-backed pack tier). Opaque,
    /// PII-free.
    pub group: StorageGroup,
    /// The repo's residency region — read from its TENANT's (immutable) region (§5.2). A repo's data
    /// stays in this region; relocation is a same-region move by construction.
    pub region: Region,
    /// The repo's placement lifecycle status (mirrors the tenant placement status — a repo on an
    /// offboarding tenant is offboarding). PII-free closed enum.
    pub status: PlacementStatus,
}

/// One repo's stored placement fact (the `repo_placement` row, architecture §5.2). PII-free — the repo
/// opaque [`ArtifactRef`], the cell it lives on (stored, relocatable), and its storage group. The
/// `region`/`status` are NOT stored here — they derive from the repo's tenant placement (so the
/// residency pin cannot drift and a tenant status change is reflected at repo grain automatically).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoPlacementRow {
    /// The cell the repo lives on (a stored fact — relocatable within-region, never node-hashed).
    pub(crate) cell_id: CellId,
    /// The repo-storage group within the cell.
    pub(crate) group: StorageGroup,
}

/// **The reason a repo placement / relocation is rejected.** Either the repo's tenant is not placed
/// (no region of record to pin the repo to), the repo's [`ArtifactRef`] is not a parseable
/// `myelin://<tenant>/git/repo/<id>` ref, or a relocation target is in a different region than the
/// repo's tenant (the residency pin holds at repo grain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoPlacementError {
    /// The repo's [`ArtifactRef`] is not a `myelin://<tenant>/git/repo/<id>` reference — there is no
    /// tenant to pin the repo's residency to. Carries the offending ref string (opaque — a repo ref
    /// is PII-free, but it is still an internal id, so we keep it minimal).
    NotARepoRef {
        /// The ref that failed the `myelin://<tenant>/git/repo/<id>` shape.
        repo: ArtifactRef,
    },
    /// The repo's tenant is not placed (the control plane knows no `tenant_placement` for it) — a repo
    /// cannot be placed onto a region of record that does not exist. Fail-closed.
    TenantNotPlaced {
        /// The repo whose tenant has no placement.
        repo: ArtifactRef,
        /// The tenant extracted from the repo ref.
        tenant: TenantId,
    },
    /// The placement invariant rejected the (re)placement — the target cell is unknown OR in a
    /// different region than the repo's tenant (the residency pin at repo grain). Wraps the underlying
    /// [`PlacementError`].
    Invariant(PlacementError),
}

impl std::fmt::Display for RepoPlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoPlacementError::NotARepoRef { repo } => write!(
                f,
                "repo placement REJECTED: `{}` is not a `myelin://<tenant>/git/repo/<id>` reference \
                 — there is no tenant to pin the repo's residency to (§5.2).",
                repo.0
            ),
            RepoPlacementError::TenantNotPlaced { repo, tenant } => write!(
                f,
                "repo placement REJECTED: repo `{}` — its tenant `{}` is not placed; a repo cannot be \
                 homed onto a region of record that does not exist (fail-closed, §5.2).",
                repo.0,
                tenant.as_str()
            ),
            RepoPlacementError::Invariant(e) => write!(
                f,
                "repo placement REJECTED by the placement invariant (the residency pin holds at repo \
                 grain — a repo cannot move out of its tenant's region): {e}"
            ),
        }
    }
}

impl std::error::Error for RepoPlacementError {}

/// **Parse the tenant + repo id out of a `myelin://<tenant>/git/repo/<id>` reference.**
///
/// **DEVIATION (EI-01 §1, documented):** the canonical `ArtifactRef` `parse`/`format` lives in
/// `myelin-refs` (REF-3), which the control-plane SERVICE crate does NOT (and must not, per the §2.9
/// DAG) depend on. This is a deliberately NARROW, repo-specific extractor over the opaque
/// `ArtifactRef` value (which lives in `myelin-tenancy`, already a dependency). It does NOT
/// re-implement the full grammar (no `#sub` kinds, no subsystem/type token table); it validates ONLY
/// the shape the repo-grain placement needs (`myelin://<tenant>/git/repo/<id>`) and returns the tenant
/// plus repo id, with anything else a [`RepoPlacementError::NotARepoRef`]. The full canonical parse
/// stays the single `myelin-refs` authority; this is a routing-key extraction, not a second parser.
fn parse_repo_ref(repo: &ArtifactRef) -> Option<(TenantId, &str)> {
    let rest = repo.0.strip_prefix("myelin://")?;
    // Exactly four `/`-segments: tenant / "git" / "repo" / id. No `#sub` on a repo root.
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
    /// **Register a repo's placement (architecture §5.2 — region-pinned, stored fact).** Stores the
    /// repo's `{cell_id, group}` as a **stored fact** keyed by the repo's opaque [`ArtifactRef`]. The
    /// repo's `cell_id` defaults to its **tenant's `home_cell`** (single-cell v1); its residency
    /// region is its tenant's (immutable) region — NOT stored independently (so the pin cannot drift).
    ///
    /// Fails [`RepoPlacementError::NotARepoRef`] if `repo` is not a `myelin://<tenant>/git/repo/<id>`
    /// reference, [`RepoPlacementError::TenantNotPlaced`] if the repo's tenant has no placement
    /// (fail-closed — no region of record), or [`RepoPlacementError::Invariant`] if the home cell is
    /// not in the tenant's region (the residency pin; should never fire on the tenant's own home cell,
    /// but checked for symmetry with relocation).
    ///
    /// This is **NEVER node-pinned**: the `cell_id` is a stored fact, not `hash(repo) mod cells`. A
    /// later [`Self::relocate_repo`] flips ONLY the stored cell, without recomputing any hash and
    /// without changing the repo's identity (its clone URL `tenant/repo`).
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
        // The residency pin at repo grain: the repo's home cell MUST be in the tenant's region. (On a
        // register this is the tenant's own home cell, which the tenant placement invariant already
        // verified; we re-assert it so register + relocate share one residency check.)
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

    /// **Relocate a repo to a DIFFERENT in-region cell (architecture §5.2 — relocatable, never
    /// node-pinned; the redirect-on-relocation property).** Flips ONLY the stored `cell_id` (and the
    /// target cell's `group`) — a **stored-fact update, NOT a hash recompute**. The repo's identity (its
    /// [`ArtifactRef`] / clone URL `tenant/repo`) is unchanged; clients rediscover the new cell via a
    /// misroute redirect.
    ///
    /// The residency pin holds at repo grain: a relocation `target_cell` in a DIFFERENT region than the
    /// repo's tenant is **REJECTED** ([`RepoPlacementError::Invariant`] / [`PlacementError`]). A repo
    /// can only move *within* its tenant's region — there is no cross-region repo move.
    ///
    /// **FLOOR (M5, P-CP-22):** this flips the control-plane routing FACT; the durable workflow that
    /// actually COPIES the repo's bytes (reindex-from-source + crypto-shred cut-over) is P-CP-22. The
    /// routing answer + the redirect are live now.
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
        // The residency pin: the relocation TARGET must be in the repo's tenant's region. A cross-region
        // target is refused — a repo cannot leave its region (§5.2). This is a STORED-FACT update; no
        // node hash is consulted or recomputed.
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

    /// **`placement_of(repo) → RepoPlacement {cell_id, group, region, status}` (architecture §5.2, C-1,
    /// LIVE; contract 12.2).** The repo-grain routing answer: the repo's stored `cell_id` + `group`,
    /// its TENANT's `region` (the residency pin — read from the tenant placement, never stored at repo
    /// grain) and `status` (mirrors the tenant placement). Returns `None` when the repo is not
    /// registered OR its tenant is not placed (the caller treats it as a misroute / no-route, never
    /// fabricates an answer).
    ///
    /// This is the *routing* answer — never an authz answer ([`RepoPlacement`] carries no
    /// grant/principal/permission field by construction). It is what makes the git-wire's repo-grain
    /// misroute-reject ([`CellGateway::route_repo`]) structural.
    pub fn placement_of_repo(&self, repo: &ArtifactRef) -> Option<RepoPlacement> {
        let (tenant, _id) = parse_repo_ref(repo)?;
        let row = self.repo_placement_row(&repo.0)?;
        // The residency pin: the region + status come from the TENANT placement, never stored at repo
        // grain — so a repo NEVER reports a region different from its tenant's.
        let tenant_placement = self.placement(&tenant)?;
        Some(RepoPlacement {
            cell_id: row.cell_id.clone(),
            group: row.group.clone(),
            region: tenant_placement.region.clone(),
            status: tenant_placement.status,
        })
    }

    /// Re-assert the placement invariant at repo grain: the given cell must be registered AND in the
    /// repo's tenant's region (the residency pin). Reuses the registry's authoritative cell inventory
    /// (never reaches into a foreign cell's data).
    fn assert_cell_in_region(
        &self,
        cell_id: &CellId,
        region: &Region,
        repo: &ArtifactRef,
    ) -> Result<(), RepoPlacementError> {
        match self.cell(cell_id) {
            None => Err(RepoPlacementError::Invariant(PlacementError::UnknownCell {
                // Re-use the tenant id from the repo ref for the loud error (PII-free opaque id).
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
    /// **`route_repo(registry, repo) → Ok(RepoPlacement) | Err(GatewayReject)` (architecture §5.2 /
    /// §7.3 — the GIT residency leg of CP-D2/CP-D3, rides GIT-D8).** Decide whether THIS cell may serve
    /// a git-wire request for `repo`, at REPO grain.
    ///
    /// 1. Ask the control-plane authoritative repo-grain routing answer
    ///    ([`Registry::placement_of_repo`]) which cell homes the repo. (A routing lookup — never a read
    ///    of the repo's content; 0 cross-tenant rows by construction.)
    /// 2. If the repo is unknown / its tenant is unplaced → [`GatewayReject::NoSuchTenant`] (reject,
    ///    audit, no redirect — a stale clone URL).
    /// 3. If the repo's `cell_id` is THIS cell → **accept**: return the [`RepoPlacement`] (served
    ///    entirely within this cell).
    /// 4. Otherwise (the repo was RELOCATED to another cell) → **reject (do NOT proxy)**: increment
    ///    `misroute_count`, audit the misroute (PII-free), return a [`GatewayReject::Misroute`] redirect
    ///    to the current cell-endpoint. The git wire re-discovers and connects there.
    ///
    /// In NO branch is a foreign repo's content read — `cross_tenant_reads` stays 0 (the GIT residency
    /// zero). The decision is made off the control-plane routing answer BEFORE any repo content is
    /// touched.
    pub fn route_repo(
        &self,
        registry: &Registry,
        repo: &ArtifactRef,
    ) -> Result<RepoPlacement, GatewayReject> {
        // 1. The authoritative repo-grain routing answer (a routing lookup — not a repo-content read).
        let Some(placement) = registry.placement_of_repo(repo) else {
            // 2. Unknown repo / unplaced tenant: reject + audit (no redirect target). A stale clone URL.
            self.record_repo_misroute(repo, registry, None);
            return Err(GatewayReject::NoSuchTenant {
                tenant_id: self.repo_tenant_or_placeholder(repo),
            });
        };

        // 3. This cell homes the repo → accept. Served entirely within this cell.
        if placement.cell_id == *self.cell_id() {
            return Ok(placement);
        }

        // 4. The repo was RELOCATED to a DIFFERENT cell → REJECT (not proxy) + REDIRECT + AUDIT. The
        //    redirect endpoint comes from the control-plane cell inventory (a routing fact, PII-free) —
        //    never by reaching into the foreign cell.
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

    /// The repo's tenant (opaque id) for a PII-free reject/audit record — falls back to the whole ref
    /// string only if the ref is unparseable (still opaque, still PII-free; a repo ref carries no PII).
    fn repo_tenant_or_placeholder(&self, repo: &ArtifactRef) -> TenantId {
        parse_repo_ref(repo)
            .map(|(t, _)| t)
            .unwrap_or_else(|| TenantId::from_token(repo.0.clone()))
    }

    /// Record a repo-grain misroute (loud, never swallowed) + bump `misroute_count`. Shares the SAME
    /// PII-free [`MisrouteAuditRecord`] shape the tenant-grain path uses (the GDPR audit consumer reads
    /// one shape, P-GA-19).
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

    /// A registry: two cells (cell-w-1, cell-w-2) in eu-west + one cell (cell-n-1) in eu-north; ACME
    /// placed on cell-w-1, with a repo `web` registered on its home cell.
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

    // ----- `placement_of(repo)` returns the frozen repo-grain tuple (architecture §5.2) -----

    /// **`placement_of(repo)` returns `{cell_id, group, region, status}`; `cell_id` defaults to the
    /// tenant's `home_cell`; `region` is the TENANT's region (the residency pin).** It is a routing
    /// answer, never an authz answer.
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

    /// `placement_of(repo)` of an UNREGISTERED repo returns `None` (never a fabricated answer).
    #[test]
    fn placement_of_repo_unregistered_is_none() {
        let reg = registry_with_repo();
        assert!(reg.placement_of_repo(&repo("01J0ACME", "ghost")).is_none());
    }

    /// `placement_of(repo)` of a malformed ref (not `myelin://<tenant>/git/repo/<id>`) is `None`.
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

    /// **`register_repo` REJECTS a ref with an EMPTY tenant or EMPTY repo-id segment** (each guard in
    /// `parse_repo_ref` is independently load-bearing — an empty tenant has no residency pin; an empty
    /// id is not a repo). Each is refused `NotARepoRef` even when every OTHER segment is well-formed.
    #[test]
    fn register_repo_rejects_empty_tenant_or_id_segments() {
        let mut reg = registry_with_repo();
        // Empty tenant segment (`myelin:///git/repo/web`) — `git`/`repo`/`web` all valid, tenant empty.
        let empty_tenant = ArtifactRef("myelin:///git/repo/web".into());
        assert!(
            matches!(
                reg.register_repo(&empty_tenant, StorageGroup::from_token("g")),
                Err(RepoPlacementError::NotARepoRef { .. })
            ),
            "an empty tenant segment is not a repo ref (no residency pin)"
        );
        assert!(reg.placement_of_repo(&empty_tenant).is_none());
        // Empty id segment (`myelin://01J0ACME/git/repo/`) — tenant/`git`/`repo` all valid, id empty.
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

    /// **Registering a repo whose tenant is NOT placed is refused fail-closed** (no region of record to
    /// pin the repo to).
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

    /// **A non-repo ref is refused** (`NotARepoRef`).
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

    // ----- region-pinned + relocatable, NEVER node-pinned (architecture §5.2) -----

    /// **`placement_of(repo)` is a STORED FACT, not a node-hash: relocating a repo within-region does
    /// NOT recompute a hash; the repo's identity (its ArtifactRef / clone URL `tenant/repo`) is
    /// byte-identical before + after.** This is the core C-1 property the git wire needs.
    #[test]
    fn repo_relocation_does_not_recompute_a_hash() {
        let mut reg = registry_with_repo();
        let r = repo("01J0ACME", "web");
        let before = reg.placement_of_repo(&r).expect("placed");
        assert_eq!(before.cell_id.as_str(), "cell-w-1");

        // Relocate the repo to cell-w-2 (same region) — a stored-fact update.
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
            "region UNCHANGED — same-region move (the pin)"
        );
        // The repo's IDENTITY (its ArtifactRef / clone URL) is byte-identical — the cell is NOT a node
        // pin: the URL did not change, only the discovered cell.
        let r_after = repo("01J0ACME", "web");
        assert_eq!(
            r, r_after,
            "the repo's clone URL identity is unchanged by relocation"
        );
    }

    /// **THE REPO RESIDENCY PIN: relocating a repo to a CROSS-REGION cell is REJECTED.** A repo can only
    /// move WITHIN its tenant's region — there is no cross-region repo move (§5.2). 0 repos leave region.
    #[test]
    fn relocate_repo_cross_region_is_rejected() {
        let mut reg = registry_with_repo();
        let r = repo("01J0ACME", "web");
        // cell-n-1 is in eu-north; ACME is pinned to eu-west.
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
        // The repo did NOT move — it is still on cell-w-1, still in eu-west.
        let still = reg.placement_of_repo(&r).expect("still placed");
        assert_eq!(
            still.cell_id.as_str(),
            "cell-w-1",
            "the rejected relocation did not move the repo"
        );
        assert_eq!(still.region.as_str(), "eu-west");
    }

    /// Relocating to an UNKNOWN cell is refused fail-closed.
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

    // ----- the repo-grain gateway misroute redirect (GIT residency leg, CP-D2/CP-D3) -----

    /// **The gateway ACCEPTS a repo request for a repo it HOSTS.** cell-w-1 serves the repo homed on
    /// cell-w-1 — no misroute, no audit, 0 cross-tenant reads.
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

    /// **THE GIT RESIDENCY LEG — a RELOCATED repo's clone/push to the OLD cell is REJECTED (not
    /// proxied), REDIRECTED to the current cell-endpoint, and AUDITED, with 0 cross-tenant/cross-cell
    /// read.** The most load-bearing repo-grain property.
    #[test]
    fn gateway_redirects_a_relocated_repo() {
        let mut reg = registry_with_repo();
        let r = repo("01J0ACME", "web");
        // The repo is relocated cell-w-1 -> cell-w-2 (same region).
        reg.relocate_repo(
            &r,
            CellId::from_token("cell-w-2"),
            StorageGroup::from_token("pack-7"),
        )
        .expect("same-region relocation");

        // A git-wire clone still pointing at cell-w-1 (the OLD cell) is a misroute.
        let old = CellGateway::new(CellId::from_token("cell-w-1"));
        let reject = old
            .route_repo(&reg, &r)
            .expect_err("cell-w-1 no longer homes the relocated repo → REJECTED (not proxied)");

        // REJECTED + REDIRECTED to the CURRENT cell-endpoint (cell-w-2), never proxied.
        assert_eq!(
            reject,
            GatewayReject::Misroute(Misroute {
                tenant_id: TenantId::from_token("01J0ACME"),
                correct_cell: CellId::from_token("cell-w-2"),
                correct_cell_endpoint: "cell.eu-west.cell-w-2.myelin.eu".into(),
            }),
            "the redirect points at the relocated repo's CURRENT cell-endpoint"
        );
        // AUDITED (PII-free, opaque tenant id only) + counted; 0 cross-tenant reads.
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

        // The redirect is then SERVED by the CURRENT cell (a redirect, never a proxy).
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

    /// **A CROSS-TENANT repo access via repo-grain misroute reads 0 cross-tenant rows.** cell-w-2
    /// (which homes a DIFFERENT tenant's repo) receives a request for ACME's repo (homed on cell-w-1)
    /// → rejected + redirected, never serving ACME's repo from cell-w-2.
    #[test]
    fn gateway_cross_tenant_repo_misroute_reads_zero() {
        let mut reg = registry_with_repo();
        // A second tenant BETA on cell-w-2 with its own repo.
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

        // cell-w-2 (BETA's home) receives a request for ACME's repo (homed on cell-w-1) — a misroute.
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

    /// An UNREGISTERED repo (stale clone URL) is rejected with no redirect target + audited.
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
