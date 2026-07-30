# Myelin dev data-layer stack (Stage 1)

Myelin code talks to STANDARD interfaces (Postgres, S3, the Redis protocol, NATS) so moving from
the local dev stack to production is a CONFIG SWAP, not a code change. This document describes the
dev stack, the env-var contract, and the dev↔prod (Scaleway, fr-par) endpoint mapping.

All four backends are open and self-hostable (no US-hyperscaler managed services):

| Tier         | Dev backend (docker-compose)      | Prod backend (Scaleway, FR)                 | Client crate            |
|--------------|-----------------------------------|---------------------------------------------|-------------------------|
| OLTP / outbox / ReBAC / audit | Postgres 16        | Scaleway Managed PostgreSQL                 | `sqlx` (runtime queries)|
| Object store | RustFS (Apache-2.0, S3-compatible)| Scaleway Object Storage                     | `aws-sdk-s3`            |
| Cache        | Valkey 8 (BSD Redis fork)         | Scaleway Managed Redis                      | `fred`                 |
| Durable bus  | NATS 2.10 + JetStream (Apache-2.0)| NATS container on Scaleway compute          | `async-nats`           |

RustFS is preferred over MinIO (MinIO is AGPL). NATS + the durable-workflow engine run as
containers on Scaleway compute in prod.

## Bring it up

```sh
docker compose -f docker-compose.dev.yml up -d --wait   # blocks until all healthchecks pass
# or:
./scripts/dev-stack.sh up
```

`--wait` blocks until every service's healthcheck reports healthy. Helpers:

```sh
./scripts/dev-stack.sh up      # up + --wait + ps
./scripts/dev-stack.sh wait    # re-run the health gate without recreating
./scripts/dev-stack.sh ps      # show health
./scripts/dev-stack.sh logs    # follow logs (optionally a service name)
./scripts/dev-stack.sh down    # stop + remove containers (keeps volumes)
./scripts/dev-stack.sh nuke    # stop + remove containers AND volumes (data loss)
./scripts/dev-stack.sh env     # print the eval-able dev env-var contract
```

## Ports (stable localhost)

Host ports are offset where they collide with a commonly-running local service, so the dev stack
does not fight a host-native Postgres/Redis.

| Service  | Container port | Host port | URL                                   |
|----------|----------------|-----------|---------------------------------------|
| Postgres | 5432           | **5433**  | `postgres://…@localhost:5433/myelin`  |
| RustFS   | 9000           | **9000**  | `http://localhost:9000`               |
| Valkey   | 6379           | **6380**  | `redis://localhost:6380`              |
| NATS     | 4222 / 8222    | 4222/8222 | `nats://localhost:4222` (mon: 8222)   |

## Postgres RLS-ready init

`scripts/pg-init/*.sql` run once, in filename order, on first cluster init.
`00-rls-conventions.sql` establishes the `(tenant, region)` Row-Level-Security conventions:

- **`myelin_admin`** — the migration/owner role (`POSTGRES_USER`).
- **`myelin_app`** — the runtime application role. It is `NOSUPERUSER NOBYPASSRLS`: a superuser or
  a `BYPASSRLS` role silently ignores every policy, so the runtime role must be neither. The app
  connects as `myelin_app`; migrations run as `myelin_admin`.
- **`myelin_ci_region_scheduler` / `myelin_ci_scheduler_fr_par`** — a `NOLOGIN` least-privilege
  scheduler capability and its constrained dev login. The login is mapped to `fr-par` in a private,
  admin-owned table and receives only queue `SELECT`, updates to the three lease columns, and
  fairness reads. It cannot assume the capability role or use a client-selected region to widen its
  server-owned region boundary.
- **`myelin_ci_definition_fence`** (from `scripts/pg-init/01-ci-definition-fence.sql`) — the ONLY
  role in the stack that carries `BYPASSRLS`, and the one exception to the rule above. The
  `ci.pipeline` definition cutover must ask a database-wide question ("is any non-terminal run still
  pinned to the superseded version?") while `workflow_run` is FORCE-RLS and no tenant scope is set;
  without bypass authority that question answers `false` instead of raising, which would drain the
  old definition while live runs still depend on it. The role is `NOLOGIN` (never connectable),
  `NOINHERIT`, owns only the `myelin_ci_security` schema and the single boolean probe function in
  it, and — after `ci_0020h` — holds `SELECT` on exactly three non-payload `workflow_run` columns.
  `myelin_admin` may adopt it (`SET TRUE`) but does not inherit it. See
  [ci-runner-deployment.md](ci-runner-deployment.md)'s definition-fence provisioning section.
- **Session GUCs** `myelin.tenant_id` / `myelin.region` — the app sets these per transaction
  (`SELECT set_config('myelin.tenant_id', $1, true)`); RLS policies reference
  `current_setting('myelin.tenant_id', true)`.
- **`myelin_make_tenant_scoped(regclass)`** — the convention helper every tenant-scoped migration
  calls once per table. It does `ENABLE` + `FORCE ROW LEVEL SECURITY` (FORCE so even the table
  owner is subject to policies) and installs the standard `(tenant_id, region)` isolation policy.

The integration test `crates/myelin-storage/tests/integration_backends.rs::postgres_rls_isolates_tenants`
proves this end-to-end: a session set to tenant A sees only tenant A's rows.

## The env-var contract (the dev↔prod CONFIG SWAP)

`crates/myelin-config` (`MyelinConfig::from_env`) reads these. Dev defaults point at this
compose stack; prod supplies every var via the environment.

| Var             | Meaning                                | Dev default                                                 |
|-----------------|----------------------------------------|-------------------------------------------------------------|
| `DATABASE_URL`  | Postgres runtime OLTP + outbox + ReBAC | `postgres://myelin_app:myelin_app_pw@localhost:5433/myelin` |
| `DATABASE_MIGRATION_URL` | Postgres migration-only credential | `postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin` |
| `MYELIN_CI_SCHEDULER_DATABASE_URL` | CI region claim/reap credential | `postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:5433/myelin` |
| `S3_ENDPOINT`   | S3-compatible object-store endpoint    | `http://localhost:9000`                                     |
| `S3_REGION`     | S3 region label                        | `fr-par`                                                    |
| `S3_ACCESS_KEY` | S3 access key id                       | `myelin_dev_access`                                         |
| `S3_SECRET_KEY` | S3 secret access key                   | `myelin_dev_secret`                                         |
| `S3_BUCKET`     | default object-store bucket            | `myelin-dev`                                                |
| `REDIS_URL`     | Valkey/Redis cache URL                 | `redis://localhost:6380`                                    |
| `NATS_URL`      | NATS JetStream bus URL                 | `nats://localhost:4222`                                     |
| `MYELIN_REGION` | data-residency region pin              | `fr-par`                                                    |

`MyelinConfig::from_env(Mode::DevDefaults)` falls back to the dev default for any absent var.
`Mode::RequireEnv` (prod) fails fast (`ConfigError::Missing`) on any absent endpoint var — never a
silent fallback to a dev endpoint. `MYELIN_REGION` defaults to `fr-par` in BOTH modes (the
residency pin; the residency-pin lint's prod instantiation pins `fr-par`).

Object-store addressing uses `force_path_style = true` (`http://endpoint/bucket/key`), correct for
both RustFS and Scaleway Object Storage.

## Scaleway (fr-par) prod endpoint mapping

Same vars, prod values — the swap is config only:

| Var             | Scaleway (fr-par) value (shape)                                        |
|-----------------|-----------------------------------------------------------------------|
| `DATABASE_URL`  | `postgres://<runtime-user>:<pw>@<id>.pg.fr-par.scw.cloud:<port>/myelin?sslmode=require` |
| `DATABASE_MIGRATION_URL` | `postgres://<migration-user>:<pw>@<id>.pg.fr-par.scw.cloud:<port>/myelin?sslmode=require` |
| `MYELIN_CI_SCHEDULER_DATABASE_URL` | `postgres://<fr-par-scheduler-user>:<pw>@<id>.pg.fr-par.scw.cloud:<port>/myelin?sslmode=require` |
| `S3_ENDPOINT`   | `https://s3.fr-par.scw.cloud`                                          |
| `S3_REGION`     | `fr-par`                                                               |
| `S3_ACCESS_KEY` | Scaleway IAM access key                                                |
| `S3_SECRET_KEY` | Scaleway IAM secret key                                                |
| `S3_BUCKET`     | `myelin-prod` (your provisioned bucket)                               |
| `REDIS_URL`     | `rediss://<user>:<pw>@<id>.mdb.fr-par.scw.cloud:<port>` (TLS)         |
| `NATS_URL`      | `nats://<nats-host>:4222` (NATS container on Scaleway compute)         |
| `MYELIN_REGION` | `fr-par` (residency pinned)                                            |

Residency is pinned to `MYELIN_REGION=fr-par` in prod. Scaleway Managed PostgreSQL supplies distinct
runtime, migration, and constrained CI scheduler credentials via `DATABASE_URL`,
`DATABASE_MIGRATION_URL`, and `MYELIN_CI_SCHEDULER_DATABASE_URL`;
Scaleway Object Storage → `S3_*`; Scaleway Managed Redis → `REDIS_URL`; the NATS JetStream +
durable-workflow containers run on Scaleway compute → `NATS_URL`.

## Real-backend tests (the `integration` cargo feature)

The default `cargo build --workspace` is DB-free: the real clients (`sqlx`, `aws-sdk-s3`, `fred`,
`async-nats`) are optional deps pulled ONLY by `--features integration`. The unit/floor backends
(in-memory bus, fs BlobStore, `InMemoryCache`) remain for unit tests. Run the live-backend tests
against the stack:

```sh
docker compose -f docker-compose.dev.yml up -d --wait
cargo test -p myelin-storage  --features integration --test integration_backends -- --test-threads=1
cargo test -p myelin-storage  --features integration --test integration_cache
cargo test -p myelin-events   --features integration --test integration_nats
cargo test -p myelin-identity --features integration --test integration_rebac
```
