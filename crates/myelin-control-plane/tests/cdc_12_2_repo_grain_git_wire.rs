use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellStatus, GatewayReject, IsolationKind, PlacementStatus,
    Registry, RepoPlacement, StorageGroup, TenantPlacement,
};
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

struct GitWire {
    gateway: CellGateway,
}

impl GitWire {
    fn connect(cell_id: &str) -> GitWire {
        GitWire {
            gateway: CellGateway::new(CellId::from_token(cell_id)),
        }
    }

    fn repo_ref_from_clone_url(clone_url: &str) -> ArtifactRef {
        let after_host = clone_url
            .split_once(':')
            .expect("clone url has a host:path split")
            .1;
        let path = after_host.strip_suffix(".git").unwrap_or(after_host);
        let (tenant, repo) = path.split_once('/').expect("clone url path is tenant/repo");
        ArtifactRef(format!("myelin://{tenant}/git/repo/{repo}"))
    }

    fn clone(
        &mut self,
        registry: &Registry,
        clone_url: &str,
    ) -> Result<(RepoPlacement, u32), GatewayReject> {
        let repo = Self::repo_ref_from_clone_url(clone_url);
        let mut redirects = 0u32;
        loop {
            match self.gateway.route_repo(registry, &repo) {
                Ok(placement) => return Ok((placement, redirects)),
                Err(GatewayReject::Misroute(m)) if redirects == 0 => {
                    redirects += 1;
                    self.gateway = CellGateway::new(m.correct_cell.clone());
                }
                Err(other) => return Err(other),
            }
        }
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
    reg.register_repo(
        &ArtifactRef("myelin://01J0ACME/git/repo/web".into()),
        StorageGroup::from_token("pack-0"),
    )
    .expect("repo registered on the home cell");
    reg
}

#[test]
fn cdc_12_2_git_wire_resolves_a_repo_to_its_cell() {
    let reg = registry();
    let mut wire = GitWire::connect("cell-w-1");
    let (placement, redirects) = wire
        .clone(&reg, "git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/web.git")
        .expect("the git wire resolves the repo to its cell");
    assert_eq!(
        placement.cell_id.as_str(),
        "cell-w-1",
        "PROVIDER: the repo's current cell"
    );
    assert_eq!(
        placement.group.as_str(),
        "pack-0",
        "PROVIDER: the repo-storage group"
    );
    assert_eq!(
        placement.region.as_str(),
        "eu-west",
        "PROVIDER: the repo region = the TENANT region (pin)"
    );
    assert_eq!(placement.status, PlacementStatus::Active);
    assert_eq!(
        redirects, 0,
        "a correct clone URL routes directly - no redirect"
    );
}

#[test]
fn cdc_12_2_git_wire_redirects_a_relocated_repo() {
    let mut reg = registry();
    reg.relocate_repo(
        &ArtifactRef("myelin://01J0ACME/git/repo/web".into()),
        CellId::from_token("cell-w-2"),
        StorageGroup::from_token("pack-9"),
    )
    .expect("same-region relocation");

    let mut wire = GitWire::connect("cell-w-1");
    let (placement, redirects) = wire
        .clone(&reg, "git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/web.git")
        .expect("the git wire follows the misroute redirect to the current cell");
    assert_eq!(
        redirects, 1,
        "the git wire re-discovered ONCE after the relocation"
    );
    assert_eq!(
        placement.cell_id.as_str(),
        "cell-w-2",
        "corrected to the relocated repo's CURRENT cell"
    );
    assert_eq!(
        placement.group.as_str(),
        "pack-9",
        "the group moved to the target cell"
    );
    assert_eq!(
        placement.region.as_str(),
        "eu-west",
        "region UNCHANGED - same-region move (the pin)"
    );
}

#[test]
fn cdc_12_2_git_wire_rejects_an_unregistered_repo() {
    let reg = registry();
    let mut wire = GitWire::connect("cell-w-1");
    let err = wire
        .clone(
            &reg,
            "git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/ghost.git",
        )
        .expect_err("an unregistered repo has no route");
    assert_eq!(
        err,
        GatewayReject::NoSuchTenant {
            tenant_id: TenantId::from_token("01J0ACME")
        }
    );
}

#[test]
fn cdc_12_2_clone_url_endpoint_is_not_part_of_repo_identity() {
    let from_w1 =
        GitWire::repo_ref_from_clone_url("git@cell.eu-west.cell-w-1.myelin.eu:01J0ACME/web.git");
    let from_w2 =
        GitWire::repo_ref_from_clone_url("git@cell.eu-west.cell-w-2.myelin.eu:01J0ACME/web.git");
    assert_eq!(
        from_w1, from_w2,
        "the cell-endpoint is a routing host, not part of the repo identity"
    );
    assert_eq!(
        from_w1,
        ArtifactRef("myelin://01J0ACME/git/repo/web".into())
    );
}
