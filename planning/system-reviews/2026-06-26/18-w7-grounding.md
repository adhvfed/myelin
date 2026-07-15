# W7 Grounding — blob flip, CI slice, blind-spots, region sweep, boot migrations (R1 exit)

Date: 2026-07-15. Read-only grounding (Opus). W7's own scanner contribution = ONE removal
(`blob.rs:362`); the ledger-13 "2→0" assumes W3b.6 + W6d + W6b2 land first. W7 also WIDENS coverage
(blind-spots + CI scope) with flip/reclassify obligations, plus two scanner-neutral sweeps.

## Part 1 — FsBlobStore (SMALL, bounded — not a platform re-point)

**S3 is the durable backing and it already exists always-compiled:** `S3BlobStore`
(`myelin-storage/src/s3blob.rs:41`, real aws_sdk_s3, custom endpoint + path-style = RustFS dev /
Scaleway prod) implements the frozen `BlobStore` trait; selection seam built:
`SubstrateProvider::blob_store(rt)` → `backend::blob_store(Backend::Real, …)` (`provider.rs:193`,
`backend.rs:33`). The "git byte backing" is NOT a second store — gitpack rides the same trait.
Drill assumptions (content-address blake3, re-hash-on-read refusal, per-tenant keyspace,
overwrite-as-heal) are trait-level, backing-agnostic. **Only 2 production defaults construct
FsBlobStore:** knowledge `store.rs:170` and chat `store/mod.rs:717–782` (cold segments) — everything
else is cfg(test). Flip = gate FsBlobStore test-support + re-point those two to the provider-selected
backing (`Arc<dyn BlobStore>`; both holders are generic `B: BlobStore`) + baseline −1.

## Part 2 — CT-004b CI slice (THE HEAVIEST — a mini-MR-009b for CI)

CT-004 built the durable scheduler/metering as SQL strings + integration tests ONLY; production
types are in-memory (`SchedulerState` `scheduler.rs:337` = Vec/BTreeMap; metering INSERT is a bind
string). CI deps (sqlx/tokio/config/aws) are ALL `optional = true` behind `integration` — needs the
W1 treatment. Widening the scanner scope to ci-controlplane/dispatch/sandbox would fire TODAY on:
`ArtifactStore` (`surfacing.rs:1044`), `JobLeaseStore` (`ci-sandbox/runner.rs:171`), CI-local
`DedupLedger` (`ci-dispatch/dispatch.rs:179`); `SchedulerState` is invisible (State suffix) —
name it explicitly. Supply-chain-sensitive; a misjudged "default the durable scheduler" that leaves
in-memory SchedulerState as system-of-record silently loses lease/claim state on restart.

## Part 3 — Blind-spot widening (4 reclassify + 1 document; NO new persistence)

| type | shape | fires? | resolution |
|---|---|---|---|
| PlacementService (`place.rs:186`) | atomics only, no collection | NO | add to named list = documentation |
| Consumer (`events/consumer.rs:370`) | pending/dead_letters/inflight maps | YES | reclassify: ephemeral broker runtime state; durable cursor = consumer_dedup (W3) |
| Firehose (`events/firehose.rs:618`) | windows/subscribers, Rc/RefCell !Send | YES | reclassify: in-process live-tail floor; real transport = P-S12 |
| OltpPool (`storage/oltp.rs:137`) | permit semaphore state | YES | reclassify: concurrency limiter, not durable data |
| InMemoryShredder (`events/holder.rs:140`) | live/unreachable BTreeSets | YES (InMem prefix dodges the Mem exclusion) | gate/exclude: real shred = KMS destroy_dek (W5) |

Risk is JUDGMENT: a wrong exclusion hides a durable holder forever — each needs the Wave-0
admit + adversarial-twin fixture discipline.

## Part 4 — Region sweep (scanner-NEUTRAL, parallelizable) — ROOT CAUSE FOUND

`tenant_tx::with_tenant_tx(pool, tenant, region, op)` is correct (explicit region GUC), but
**`SubstrateProvider::with_tenant_tx` hardcodes `&self.config.region`** (`provider.rs:174–183`) for
every write; all durable backings inherit it (pseudonym_durable `:146,349`, reserve_settle_durable
`:149`, identity_durable via provider). A de-fra tenant's write on a fr-par-pinned provider persists
under region='fr-par' — the residency bug W6a/b/c flagged. Fix = a `with_scope_tx(tenant, region, op)`
provider path + thread `scope.region()` through the backings. Hardcoded fr-par outside tests: ONE
constant (`myelin-agent-service/src/dogfood.rs:72` self-region default) to review.

## Part 5 — Boot migrations (scanner-NEUTRAL) — CONFIRMED GAP + a LIVE DEFECT

`foundation_migrations()` = 0000/0001 only. Nothing boot-applies identity 0010–0019, pseudonym
0020–0022, placement 0030–0034, kms 0040–0042 (edge applies ONLY these), cost 0050, erasure
0051–0053. **LIVE DEFECT: `myelin-edge/src/main.rs:63–92` constructs `PrincipalStore::with_pg` +
`RevocationStore::with_pg` but never migrates their tables and never calls migrate_foundation —
first principal write fails at runtime on a fresh DB.** Home for the fix: a provider aggregate
`all_durable_migrations()` (numeric id order; PgMigrator is idempotent + advisory-locked +
version-recorded; FK/trigger deps already numerically ordered; free ids 0022–0029 unused? — note
0022 is used by pseudonym; free: 0023–0029, 0035–0039, 0060+ for CI) applied at every service main —
lands naturally with the W3b.4 composition-root wave.

## Sub-wave split + order

- **W7.1 region sweep** + **W7.2 boot-migrations aggregate**: scanner-neutral, run in parallel
  anytime (W7.2 ideally WITH/right after W3b.4).
- Baseline-serialized trio: **W7.3 blob flip** (−1, mechanical) → **W7.4 blind-spot widening**
  (scanner constants + fixtures) → **W7.5 CT-004b** (heaviest last, on a settled scanner + settled
  composition roots). Riskiest: W7.5; second: W7.4 (permanent judgment calls).

## Out of scope

`residency_drill.rs:444` (attestation track); registry/placement (W6d), outbox (W3b.6), CostLedger
(W6b2); knowledge block_tree/TRUNCATE shapes; git holders_hit; mTLS region pin; non-CI subsystem
stores beyond the two blob re-point sites (census §B2 subsystem tracks).
