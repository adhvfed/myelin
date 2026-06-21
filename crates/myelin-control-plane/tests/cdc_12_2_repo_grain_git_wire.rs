//! # Contract 12.2 (repo-grain, C-1) CDC pair — the **git wire calling `placement_of(repo)`**
//!
//! **DATED GREEN ARTIFACT (2026-06-21).** P-CP-15 / P-250. This file is the consumer-driven contract
//! pair for the repo-grain half of contract 12.2 (`placement_of(repo) → {cell_id, group, region,
//! status}`, region-pinned + relocatable, never node-pinned).
//!
//! ## The CDC pair (contract 12.2 repo-grain)
//! - **PROVIDER** = `myelin-control-plane` — [`Registry::placement_of_repo`] + the repo-grain gateway
//!   [`CellGateway::route_repo`] (the live repo-grain routing answer + the misroute redirect).
//! - **CONSUMER** = the **git wire** — modelled here by [`GitWire`], the SSH/HTTPS front the architecture
//!   §7.3 names (`git@<cell-endpoint>:tenant/repo.git`). It encodes the cell in the clone URL, parses a
//!   clone URL into the repo's canonical [`ArtifactRef`] (`myelin://<tenant>/git/repo/<id>`), asks the
//!   control plane which cell homes the repo, and **re-discovers on a misroute redirect** — the exact
//!   §7.3 git-wire use of `placement_of(repo)` (C-1).
//!
//! **Why the git wire is modelled in-test (documented deviation, EI-01 §1):** `myelin-git` is a leaf
//! SERVICE crate ABOVE the control-plane in the §2.9 DAG; the control-plane cannot depend back on it
//! (an upward edge). So the CONSUMER contract is exercised here by a faithful [`GitWire`] that uses
//! ONLY the public provider surface (`route_repo` / `placement_of_repo`) the real git wire calls — the
//! repo `ArtifactRef` grammar (`myelin://<tenant>/git/repo/<id>`) is the SAME one `myelin-git` mints
//! (subs §2). If the provider's repo-grain answer shape or the redirect contract drifts, this stops
//! passing. The two halves agree WITHOUT a shared type (the DAG forbids the edge).
//!
//! ## What the pair proves
//! 1. The git wire's clone URL `git@<cell-endpoint>:tenant/repo.git` resolves (via `placement_of(repo)`)
//!    to the repo's current cell + group + region (the TENANT's region — the residency pin).
//! 2. A clone URL still pointing at a repo's OLD cell after a relocation gets a **misroute redirect** to
//!    the current cell-endpoint, and the git wire RE-DISCOVERS there (corrected, never proxied).
//! 3. A stale clone URL (an unregistered repo) is rejected with no redirect (no route).

use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellStatus, GatewayReject, IsolationKind, PlacementStatus, Registry,
    RepoPlacement, StorageGroup, TenantPlacement,
};
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// THE CONSUMER — the git wire (architecture §7.3). It encodes the cell in the clone URL, parses a
// clone URL into the repo's canonical ArtifactRef, calls the PROVIDER's `route_repo`, and re-discovers
// on a misroute redirect. It uses ONLY the public provider surface the real git wire calls.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The git-wire front (`git@<cell-endpoint>:tenant/repo.git`, architecture §7.3). It holds the cell it
/// is fronting (the cell the clone URL's `<cell-endpoint>` resolved to) and routes a repo request at
/// REPO grain through the control plane's `placement_of(repo)`, re-discovering on a misroute redirect.
struct GitWire {
    /// The cell this git-wire front is currently connected to (encoded in the clone URL's endpoint).
    gateway: CellGateway,
}

impl GitWire {
    /// Connect a git-wire front to the cell `cell_id` (the cell the clone URL's `<cell-endpoint>`
    /// resolved to).
    fn connect(cell_id: &str) -> GitWire {
        GitWire { gateway: CellGateway::new(CellId::from_token(cell_id)) }
    }

    /// **Parse a git clone URL into the repo's canonical [`ArtifactRef`]** (architecture §7.3 — the git
    /// wire encodes `tenant/repo` in the URL path). `git@<cell-endpoint>:<tenant>/<repo>.git` →
    /// `myelin://<tenant>/git/repo/<repo>` (the SAME grammar `myelin-git` mints, subs §2). The
    /// `<cell-endpoint>` is the discovered cell (a routing host, NOT part of the repo identity — the URL
    /// is not a node pin).
    fn repo_ref_from_clone_url(clone_url: &str) -> ArtifactRef {
        // git@<cell-endpoint>:<tenant>/<repo>.git
        let after_host = clone_url.split_once(':').expect("clone url has a host:path split").1;
        let path = after_host.strip_suffix(".git").unwrap_or(after_host);
        let (tenant, repo) = path.split_once('/').expect("clone url path is tenant/repo");
        ArtifactRef(format!("myelin://{tenant}/git/repo/{repo}"))
    }

    /// **The git-wire's use of `placement_of(repo)` (C-1, §7.3): route a clone/push at repo grain,
    /// re-discovering on a misroute redirect.** Returns the served [`RepoPlacement`] (the cell that
    /// homed the repo) + the number of redirects the wire followed (0 if the clone URL was already
    /// correct, 1 if it had to re-discover after a relocation).
    fn clone(
        &mut self,
        registry: &Registry,
        clone_url: &str,
    ) -> Result<(RepoPlacement, u32), GatewayReject> {
        let repo = Self::repo_ref_from_clone_url(clone_url);
        let mut redirects = 0u32;
        // The git wire follows AT MOST one redirect (a misroute correction); a healthy clone is direct.
        loop {
            match self.gateway.route_repo(registry, &repo) {
                Ok(placement) => return Ok((placement, redirects)),
                Err(GatewayReject::Misroute(m)) if redirects == 0 => {
                    // Re-discover: reconnect to the redirect's current cell-endpoint and retry ONCE.
                    redirects += 1;
                    self.gateway = CellGateway::new(m.correct_cell.clone());
                }
                Err(other) => return Err(other),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────

fn cell(id: &str, region: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: Region::new(region),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
        utilisation: 10,
        version: 1,
        endpoint: format!("cell.{region}.{id}.myelin.eu"),
    }
}

fn registry() -> Registry {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "eu-west"));
    reg.insert_cell(cell("cell-w-2", "eu-west"));
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token("01J0ACME"),
        region: Region::new("eu-west"),
        home_cell: CellId::from_token("cell-w-1"),
        isolation_tier: IsolationKind::Pool,
        slug: "acme".into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token("cell-w-1")],
    })
    .expect("placed");
    reg.register_repo(&ArtifactRef("myelin://01J0ACME/git/repo/web".into()), StorageGroup::from_token("pack-0"))
        .expect("repo registered on the home cell");
    reg
}

/// **CDC GREEN: the git wire's clone URL resolves (via `placement_of(repo)`) to the repo's current cell,
/// group, and TENANT region (the residency pin) — 0 redirects on a correct URL.**
#[test]
fn cdc_12_2_git_wire_resolves_a_repo_to_its_cell() {
    let reg = registry();
    // A clone URL whose <cell-endpoint> already points at the repo's home cell.
    let mut wire = GitWire::connect("cell-w-1");
    let (placement, redirects) = wire
        .clone(&reg, "git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/web.git")
        .expect("the git wire resolves the repo to its cell");
    assert_eq!(placement.cell_id.as_str(), "cell-w-1", "PROVIDER: the repo's current cell");
    assert_eq!(placement.group.as_str(), "pack-0", "PROVIDER: the repo-storage group");
    assert_eq!(placement.region.as_str(), "eu-west", "PROVIDER: the repo region = the TENANT region (pin)");
    assert_eq!(placement.status, PlacementStatus::Active);
    assert_eq!(redirects, 0, "a correct clone URL routes directly — no redirect");
}

/// **CDC GREEN: a RELOCATED repo's stale clone URL gets a misroute redirect; the git wire re-discovers
/// to the current cell-endpoint (corrected, never proxied).** This is the §7.3 git-wire-on-relocation
/// contract at repo grain — the property the C-1 sharpening exists for.
#[test]
fn cdc_12_2_git_wire_redirects_a_relocated_repo() {
    let mut reg = registry();
    // The repo is relocated cell-w-1 -> cell-w-2 (same region) — a stored-fact flip.
    reg.relocate_repo(
        &ArtifactRef("myelin://01J0ACME/git/repo/web".into()),
        CellId::from_token("cell-w-2"),
        StorageGroup::from_token("pack-9"),
    )
    .expect("same-region relocation");

    // A git wire still using the OLD clone URL (cell-w-1) re-discovers to cell-w-2.
    let mut wire = GitWire::connect("cell-w-1");
    let (placement, redirects) = wire
        .clone(&reg, "git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/web.git")
        .expect("the git wire follows the misroute redirect to the current cell");
    assert_eq!(redirects, 1, "the git wire re-discovered ONCE after the relocation");
    assert_eq!(placement.cell_id.as_str(), "cell-w-2", "corrected to the relocated repo's CURRENT cell");
    assert_eq!(placement.group.as_str(), "pack-9", "the group moved to the target cell");
    assert_eq!(placement.region.as_str(), "eu-west", "region UNCHANGED — same-region move (the pin)");
}

/// **CDC: a stale clone URL for an UNREGISTERED repo is rejected with no route (no redirect target).**
#[test]
fn cdc_12_2_git_wire_rejects_an_unregistered_repo() {
    let reg = registry();
    let mut wire = GitWire::connect("cell-w-1");
    let err = wire
        .clone(&reg, "git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/ghost.git")
        .expect_err("an unregistered repo has no route");
    assert_eq!(err, GatewayReject::NoSuchTenant { tenant_id: TenantId::from_token("01J0ACME") });
}

/// **CDC: the clone-URL → ArtifactRef parse is the SAME grammar `myelin-git` mints (subs §2).** The
/// `<cell-endpoint>` is a routing host, NOT part of the repo identity — two clone URLs with DIFFERENT
/// endpoints but the same `tenant/repo` parse to the SAME [`ArtifactRef`] (the URL is not a node pin).
#[test]
fn cdc_12_2_clone_url_endpoint_is_not_part_of_repo_identity() {
    let from_w1 = GitWire::repo_ref_from_clone_url("git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/web.git");
    let from_w2 = GitWire::repo_ref_from_clone_url("git@cell.eu-west.cell-w-2.myelin.eu:01J0ACME/web.git");
    assert_eq!(from_w1, from_w2, "the cell-endpoint is a routing host, not part of the repo identity");
    assert_eq!(from_w1, ArtifactRef("myelin://01J0ACME/git/repo/web".into()));
}
