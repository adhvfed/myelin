# Infra & deployment decisions — the committed real-backend stack (binding)

> Status: **CONFIRMED (binding user steer).** This document **supersedes the "default-to-beat"
> placeholders** in the refined shared-systems docs wherever they named a *candidate* backend
> rather than a *committed* one — in particular the storage doc's "S3-compatible object store
> (MinIO or Ceph)" line (`storage.md` §3.2 / §11) and any "MinIO/Ceph" parity references. Those
> were defaults-to-beat (EI-02 §8 — propose, then measure); the data layer is now **pinned** to
> the concrete open, self-hostable stack below. Where a doc still says "MinIO/Ceph", read
> **RustFS**; where it says "a managed Postgres/cache/bus", read the Scaleway mapping in §3.

This is the data-layer consolidation: code talks to **STANDARD interfaces** so dev↔prod is a
**CONFIG SWAP, not a code change**. The stack is all open + self-hostable, with **no
US-hyperscaler managed services**.

---

## 1. The committed stack (dev = the docker-compose stack)

| Tier | Backend (committed) | Licence | Role | Client crate |
|---|---|---|---|---|
| OLTP + outbox + ReBAC tuple store + audit | **Postgres 16** | PostgreSQL | the relational substrate | **sqlx with RUNTIME-checked queries** (`sqlx::query` / `query_as`, **not** the `query!` compile-time macros) or `tokio-postgres` |
| Object store | **RustFS** (`rustfs/rustfs`) | Apache-2.0 | S3-compatible blob storage — **PREFERRED over MinIO (AGPL)** | **aws-sdk-s3** with a custom endpoint + path-style addressing (`force_path_style`) + static dev creds |
| Cache | **Valkey 8** | BSD | the cache tier (Redis fork) | **fred** (or `redis-rs`) |
| Durable bus | **NATS 2.10 JetStream** | Apache-2.0 | the durable event bus | **async-nats** (JetStream API), self-hosted container |

**Why these.** RustFS is Apache-2.0 (MinIO is AGPL — avoided); Valkey is the BSD Redis fork (no
licence drift); NATS JetStream is Apache-2.0 and self-hostable as a single container. Postgres
carries the OLTP **and** the outbox **and** the ReBAC tuple store **and** the audit log so there
is one transactional boundary for the emit-iff-committed seam.

**The sqlx runtime-query discipline (load-bearing).** Queries use `sqlx::query` /
`sqlx::query_as` (runtime-checked), **never** the `query!` / `query_as!` compile-time macros.
This keeps **`cargo build --workspace` (default features) DB-FREE** — the build never needs a
live database or an offline `.sqlx` cache. Only the integration tests need a live DB. The real
clients (sqlx, aws-sdk-s3, fred, async-nats) are **optional deps behind a cargo feature named
`integration`**; the default in-memory/mock backends remain for unit tests.

**The dev stack** is `docker-compose.dev.yml` (Postgres / RustFS / Valkey / NATS). Bring it up
with the health-gated:

```
docker compose -f docker-compose.dev.yml up -d --wait
```

The `--wait` flag blocks until every healthcheck passes. The dev env-var contract
(`DATABASE_URL`, `S3_*`, `REDIS_URL`, `NATS_URL`, `MYELIN_REGION`) is the same surface prod
points at — see `scripts/dev-stack.sh env`.

---

## 2. The trait surfaces the real backends sit BEHIND (no trait forked)

The real backends are implemented **behind the existing frozen trait surfaces** (EI-01 §7
coherence — implement behind the trait, never redefine or fork it):

| Seam | Trait | Owning crate | Default impl (unit tests) | Real impl (`integration`) |
|---|---|---|---|---|
| Object store | **`BlobStore`** | `myelin-gdpr` (re-exported via `myelin-storage::blob`) | in-memory | `S3BlobStore` (aws-sdk-s3 → RustFS) |
| Durable bus | **`BusTransport`** | `myelin-events` | in-memory | `NatsJetStreamBus` (async-nats JetStream) |
| OLTP + outbox + ReBAC tuple store | the substrate client | `myelin-substrate` / `myelin-identity` | in-memory | `PgStore` / `PgRelay` (sqlx → Postgres) |
| Cache | **`Cache`** | `myelin-storage::cache` (created at Stage 1 — it did not yet exist; **noted here**) | `InMemoryCache` | `ValkeyCache` (fred → Valkey) |

The `Cache` trait was the one seam with **no pre-existing trait**; a minimal `Cache` trait was
created in `myelin-storage::cache` and the Valkey backing (`ValkeyCache`) sits behind it. Every
other seam already had its trait — the real backend rode the frozen surface.

---

## 3. Prod mapping — Scaleway (FR), `fr-par`

Prod is the SAME code, a different config. No US-hyperscaler managed service is used.

| Tier | Prod (Scaleway, FR) | Endpoint via |
|---|---|---|
| OLTP / outbox / ReBAC / audit | **Scaleway Managed PostgreSQL** | `DATABASE_URL` |
| Object store | **Scaleway Object Storage** (S3 API; aws-sdk-s3 path-style) | `S3_ENDPOINT` / `S3_REGION` / `S3_ACCESS_KEY` / `S3_SECRET_KEY` / `S3_BUCKET` |
| Cache | **Scaleway Managed Redis** | `REDIS_URL` |
| Durable bus | **NATS JetStream container** on Scaleway compute | `NATS_URL` |
| Durable-workflow engine | **container** on Scaleway compute | (its own config) |

**Residency pin.** `MYELIN_REGION=fr-par` in prod — the **residency-pin lint's prod
instantiation**. A blank region is rejected; the dev default is also `fr-par` so the residency
posture is identical dev↔prod.

**The dev↔prod config-swap principle.** The code is written ONCE against the standard interfaces
(sqlx/Postgres, aws-sdk-s3/S3, fred/Redis, async-nats/JetStream). Moving from the docker-compose
dev stack to Scaleway is **purely** repointing `DATABASE_URL` / `S3_*` / `REDIS_URL` / `NATS_URL`
/ `MYELIN_REGION` — there is no code branch on environment, no `#[cfg(prod)]` backend, no second
client. This is what makes the integration suite a real prod-parity gate: the same
`--features integration` tests that pass against docker-compose pass against Scaleway by env swap.

---

## 4. The testing-policy change — every backend prompt ships a REAL integration test

**The policy (binding).** Every DB / storage / cache / bus prompt ships a **real integration
test** against the live stack, run `--features integration`. The scorecard row for that drill is
**RED-until-proven**: it can only read PASS once its `--features integration` test emits a dated
green artifact against the live docker-compose stack. A DB-free run **cannot** flip a row green —
the proof command fails to connect without the stack, so the honest verdict is RED.

**The mechanism (committed).**

- The four retrofitted foundational drills are infra scorecard rows
  (`myelin-harness::scorecard`, `Band::Infra`), each proven by
  `cargo test -p myelin-storage --features integration --test stage3_drills <drill>`:
  - **STOR-D-OUTBOX** — outbox no-loss under crash (real PG → real NATS JetStream): 0 lost / 0
    ghost across a crash between broker-publish and the `published_at` commit; restart re-claims
    + re-publishes; exactly-once-in-effect via `Nats-Msg-Id = event_id` dedup.
  - **STOR-D-RESTORE** — restore-verify cross-seam (real PG ⟷ real RustFS ⟷ bus offset): every
    restored row's blob is present AND re-hashes to its content-address; no bus offset past the
    restored rows.
  - **STOR-D-RLS** — (tenant, region) RLS isolation, **DB-enforced** via the NOBYPASSRLS
    `myelin_app` role: a predicate-less `SELECT` returns only the acting tenant's rows
    (cross-tenant leak = 0); the deliberately predicate-less probe lives in the test, so
    production `pg.rs` keeps every tenant-store query tenant-bound (the tenant-predicate IDOR
    lint stays fully live).
  - **ID-D-REBAC** — ReBAC check/list_objects no-leak / no-N+1 (real PG tuple store): `check`
    fail-closed allow/deny correctness; `list_objects` returns EXACTLY the visible set (no leak)
    in ONE reverse-index query, not one check per candidate.

- The gate is committed two ways (an uncommitted gate is no gate, EI-01 §5):
  - **`scripts/integration-test.sh`** — `up -d --wait` → `cargo test --workspace --features
    integration` → the infra scorecard runner; optionally `--down` / `--nuke`. LOUD: no `|| true`.
  - **`.github/workflows/integration.yml`** — the committed CI band-boundary integration gate,
    runs the script and uploads `testing/scorecards/infra.md`. The DB-free `ci.yml` stays green +
    DB-free; only this workflow needs Docker + a live stack.

- The **ratchet** (`myelin-harness::scorecard`, the `tests/scorecard_ratchet.rs` family): you
  cannot drop a row (the frozen `infra_required_rows()` set re-reds the gate on a missing id), and
  you cannot flip a row green without proof (`RowResult::pass` panics on an empty proof line).

---

## 5. The two remaining TRUE floors + their containerized smokes

Two drills genuinely need **more than Docker** to be a full gate. They stay **RED with their
floor NAMED** (the deferral is visible, never invisible — EI-01 §1). Each ships a
**CONTAINERIZED SMOKE** now so it is **not zero-coverage**:

| Floor (still open) | Why Docker is not enough | Containerized smoke (PROVEN now) |
|---|---|---|
| **Real-kernel SANDBOX-ESCAPE** (the sandbox-tenant isolation gate) | needs a real isolation kernel — **gVisor / Firecracker microVM** — not a Docker container; container escape resistance is a kernel property | **`SANDBOX-SMOKE`** — a HARDENED-container smoke that launches `alpine` with the production isolation flags and probes from inside: **egress-deny** (`--network=none` → an outbound request fails), **read-only-root** (`--read-only` → a write to `/` fails), **dropped caps** (`--cap-drop=ALL` → a cap-gated `chown` is refused). Asserts the hardening *posture* works. |
| **WORLD-SCALE 30× LOAD** (the surge/headroom gate) | needs **real hardware** — a multi-node cluster — not a single dev box | **`LOAD-10X-SMOKE`** — drives the `myelin-harness` `LoadGenerator` at **10×** against the LIVE PG outbox → NATS JetStream path; asserts every issued request's event is committed, the outbox drains to 0 (no loss under the 10× containerized burst), and every committed event is delivered exactly-once. |

Both smokes carry a `floor:` note on their scorecard row, so the rendered
`testing/scorecards/infra.md` prints the two genuine floors as **open, dated deferrals** even
though the smokes pass — a proven smoke never silently claims its floor closed. The full
real-kernel SANDBOX-ESCAPE gate (gVisor/microVM) and the WORLD-SCALE 30× LOAD drill (real
hardware) stay RED until run on the real substrate.

---

## 6. What this supersedes (explicit)

- **`storage.md` §3.2 / §11 "MinIO or Ceph" object-store placeholder** → **RustFS** (Apache-2.0),
  preferred over AGPL MinIO. The "managed cell ↔ self-hosted cell parity" line (`storage.md`
  §11) now reads: the same Storage artifacts (Postgres + **RustFS** + KMS + backup machinery) run
  a managed Scaleway cell and a self-hosted cell.
- Any "default-to-beat" backend *candidate* in the refined docs (Postgres/cache/bus/object) is
  now a **committed** choice with the Scaleway `fr-par` prod mapping in §3. The *tuning*
  defaults-to-beat (W, RPO, promotion thresholds — the measured-not-predicted knobs) are
  untouched; this doc pins the **backends and the deployment target**, not the perf knobs.
