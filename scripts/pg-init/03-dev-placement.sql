-- Bind the Fed-managed development tenants AND the self-host dogfood tenant
-- to this local cell. This runs after the edge has applied the placement
-- schema and is safe to repeat when a Fed data volume survives a restart.
-- The self-host tenant must be placed like any other: an unplaced tenant's
-- events are out of scope for every cell-bound consumer (agent triggers,
-- most visibly), which is how the myelin dogfood tenant's CI events ended up
-- as quarantine noise before this row existed.
\set ON_ERROR_STOP on

BEGIN;

INSERT INTO cell (
  cell_id, region, status, isolation_kind, tenants_max, write_qps_max,
  storage_bytes_max, utilisation, version, endpoint
) VALUES (
  :'fed_cell', 'fr-par', 'Active', 'Pool', 1000, 5000,
  1099511627776, 0, 1, :'edge_endpoint'
)
ON CONFLICT (cell_id) DO UPDATE SET
  status = EXCLUDED.status,
  endpoint = EXCLUDED.endpoint;

INSERT INTO tenant_placement (
  tenant_id, region, home_cell, isolation_tier, slug, status, member_cells
) VALUES
  (:'development_tenant', 'fr-par', :'fed_cell', 'Pool', :'development_tenant', 'Active', ARRAY[:'fed_cell']),
  (:'integration_tenant', 'fr-par', :'fed_cell', 'Pool', :'integration_tenant', 'Active', ARRAY[:'fed_cell']),
  (:'selfhost_tenant', 'fr-par', :'fed_cell', 'Pool', :'selfhost_tenant', 'Active', ARRAY[:'fed_cell'])
ON CONFLICT (tenant_id) DO UPDATE SET
  home_cell = EXCLUDED.home_cell,
  isolation_tier = EXCLUDED.isolation_tier,
  slug = EXCLUDED.slug,
  status = EXCLUDED.status,
  member_cells = EXCLUDED.member_cells;

INSERT INTO local_tenant (cell_id, tenant_id, isolation_tier, active) VALUES
  (:'fed_cell', :'development_tenant', 'Pool', true),
  (:'fed_cell', :'integration_tenant', 'Pool', true),
  (:'fed_cell', :'selfhost_tenant', 'Pool', true)
ON CONFLICT (cell_id, tenant_id) DO UPDATE SET
  isolation_tier = EXCLUDED.isolation_tier,
  active = EXCLUDED.active;

-- Local hosted-agent credit is explicit development fixture data. The immutable
-- ledger entry has a stable identity, so restarting Fed never mints more credit.
INSERT INTO agent_wallet (tenant_id, region, balance_micro) VALUES
  (:'development_tenant', 'fr-par', 10000000),
  (:'integration_tenant', 'fr-par', 10000000)
ON CONFLICT (tenant_id, region) DO NOTHING;

INSERT INTO agent_wallet_ledger
  (tenant_id, region, entry_id, kind, amount_micro, run_id) VALUES
  (:'development_tenant', 'fr-par', md5(:'development_tenant' || ':hosted-dev-credit')::uuid,
   'topup', 10000000, NULL),
  (:'integration_tenant', 'fr-par', md5(:'integration_tenant' || ':hosted-dev-credit')::uuid,
   'topup', 10000000, NULL)
ON CONFLICT (tenant_id, entry_id) DO NOTHING;

COMMIT;
