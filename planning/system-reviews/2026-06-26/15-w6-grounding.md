# W6 Grounding — per-holder map + builder-prompt skeletons (R1 / MR-009b)

Date: 2026-07-15. Produced by a read-only grounding pass (Opus subagent) over the 9 remaining
W6a–W6d holders in the `no-in-memory-durable-store` baseline. Feeds the ledger-13 W6 execution.
Authoritative baseline: `crates/myelin-lints/tests/production_graph_absence.rs` (BASELINE const) —
the **single shared file every sub-wave edits**, which forces sub-wave serialization.

Established flip pattern (Wave 2): `myelin-identity-service/src/principal_store.rs` — backend enum
with `#[cfg(any(test, feature="test-support"))] Memory(...)` arm + always-compiled `Pg`/`with_pg`;
durable-backing style: `myelin-storage/src/identity_durable.rs` / `placement_durable.rs`
(`X_MIGRATION` DDL consts + `x_durable_migrations()`). **Migration id ranges in use:** identity
`0010–0019`, placement `0030–0034`, kms `0040–0042`; free: `0020–0029`, `0035–0039`, `0050+`.

## W6a — identity (`myelin-identity-service`, reaches storage; exact W2 mirror). 2 entries.

**`PseudonymStore`** (`pseudonym_store.rs:191`): `inner: Arc<Mutex<Inner>>` + kms + holder; `Inner`
= 3 partitioned HashMaps (by_subject/by_pseudonym/sealed). NO durable backing exists (no
`pseudonym_map` table). Build: `pseudonym_map` table (tenant, region, principal_id PK,
pseudonym_render, real_id_key_ref, nonce, ciphertext) + reverse index + RLS; migrations
`0020_pseudonym_map`/`0021_pseudonym_map_rls`; `PseudonymBackend{Memory,Pg}` + `with_pg`.
Subtleties: persist ciphertext only (keys stay in KMS); `shred_row` must DELETE row+sealed+reverse
on Pg; `crypto_shredded_resolve_fails_loud_but_pseudonym_survives` must hold on Pg; write real
`scope.region()`. Difficulty: standard.

**`PseudonymErasureLedger`** (`pseudonym_erase.rs:268`): `Arc<Mutex<LedgerByPartition>>` where the
alias `type LedgerByPartition = BTreeMap<(String,String), BTreeMap<String, ErasureLedgerEntry>>`
hid the collection. Build: `identity_pseudonym_erasure_ledger` table (tenant, region, subject PK,
dek_class, erased_at); migration `0022`. **CRITICAL: PII-free and NON-shred-erasable** — must
survive the crypto-shred it records AND restore; NO RLS/crypto-shred lever on this table.
Idempotent upsert (`ON CONFLICT ... DO UPDATE SET erased_at`); partition-isolation test must hold
on Pg. Difficulty: standard/mechanical.

## W6b — storage (in-crate, no DAG hop; carries the erasure-completion fold-in). 3 entries.

**`CostLedger`** (`reserve_settle.rs:283`): plain `#[derive(Default)]` struct, NOT an Arc handle —
`reservations: HashMap`, `cost_events: Vec`, `inflight_interrupt_count`; **`&mut self` API is the
trap** (reserve/settle/begin/cancel_unstarted). Recommend converting to the cloneable `&self`
interior-mutability handle shape to match the pattern (changes caller mutability). Build:
`cost_reservation` + `cost_event` tables (migrations `0050+`). Preserve the 4 invariants
(never-interrupt-in-flight; one-cost-event-per-unit; settle-capped-at-reserved; idempotent
double-settle → SQL re-read of `recorded_outcome`). Difficulty: careful.

**`ErasureLedger`** (`restore_verify.rs:175`): bare `BTreeSet<TenantId>`, **no timestamp**.
Embedded in `RestoreVerifyGate::run` step 4. Build: `restore_erasure_ledger` table (tenant,
**erased_at completion timestamp**); migration `0051`. **R1 FOLD-IN LANDS HERE:** completion time +
restore-inside-window resurrection test (§7.6 backup-window-vs-erasure-SLA residual; connects to
`InMemoryPostPitLedger.ErasureRecord.completed_at_offset` which records WAL offset, not wall
clock). Difficulty: careful (touches the permanent gate).

**`InMemoryPostPitLedger`** (`reerase.rs:156`): `records: Vec<ErasureRecord>` — **already behind
`trait PostRestoreErasureLedger`** (`erasures_completed_after(pit)`); `ReErasePass::run` takes
`&dyn` → a durable impl drops in with zero caller change. Build: `post_pit_erasure_ledger` table +
`DurablePostPitLedger` impl; migration `0052`; gate the in-memory type test-support. Easiest W6b
holder — do first within the wave. Difficulty: mechanical/standard.

**Also in W6b (SQL-interpolation fold-in):** `rls.rs:210` `TenantQuery::predicate_sql` renders
tenant/region as interpolated literals — convert away from interpolation. (`block_tree`/TRUNCATE
shapes are in `myelin-knowledge` — out of spine scope. git DSR `holders_hit` reconciliation is
`myelin-git/src/holder.rs:210` — out-of-spine adjacent fold-in.)

## W6c — split into two independent items (different crates, opposite DAG reach).

**W6c-events `BusErasureLedger`** (`myelin-events/src/reerase.rs:102`): `entries:
Arc<Mutex<BTreeMap<String, ErasedSubject>>>`; consumers `BusHolder::erase_and_record` +
`re_erase_after_restore` (production lib code). **DAG BLOCKER: myelin-events is a §2.9 sink**
(deps: tenancy/identity/serde only; tokio/config/nats optional behind `integration`) — cannot name
PgPool. **Use the DedupLedger trait-seam pattern** (production_graph_absence.rs:80–89): trait in
`myelin-events`, impl + `bus_erasure_ledger` table in `myelin-storage/src/events_durable.rs`
(continue its migration sequence), wire at `events_serve::EventsRuntime`. PII-free +
non-shred-erasable; idempotent `key_refs` merge → `ON CONFLICT ... DO UPDATE` array-merge.
Difficulty: careful. Direct sibling of the W3b OutboxStore sink problem.

**W6c-cp `CellResolverRegistry`** (`myelin-control-plane/src/cross_cell_bridge.rs:244`):
`resolvers: HashMap<CellId, Arc<dyn CellLocalResolver>>` — **live trait-object handles, not
durable data**. Design decision required: (a) net-new `cell_resolver_endpoint` table +
boot-rebuild, or (b) **project from the existing `cell.endpoint` column**
(`placement_durable.rs:77`) and gate the in-memory registry test-support — (b) may be the honest
answer since `cell.endpoint` already durably holds each cell's PII-free routing endpoint.
CP-D8 "0 PII across bridge" tests must stay green. Difficulty: careful (design decision first).

## W6d — control-plane placement (hardest; land Registry + MisrouteAudit together). 2 entries.

**`Registry`** (`registry.rs:111`): FIVE in-memory collections (cells, placements,
provisioning_log, local_tenants, repo_placements) + rich `&mut self` surface; the pervasive
always-compiled CP API and `DegenerateControlPlane` self-host's REAL system-of-record. Partial
durable backing exists: `registry_durable.rs::DurablePlacementRegistry` (`PlacementBackend{
Memory(Registry), Pg(PgPlacement)}`) covers cell + tenant_placement + misroute_audit ONLY — and
its Memory arm wraps the whole canonical Registry, which is why the scanner still fires. Build:
`repo_placement` (NOT rebuildable — the load-bearing gap), `cell_provisioning`, `local_tenant`
tables (migrations `0036–0038`); convert the canonical `Registry` to a role-struct over a
whole-surface `PlacementBackend{Memory[test-support], Pg}`; re-point self-host/boot to Pg.
Preserve region-immutability + repo→tenant region derivation (residency pin cannot drift; the
tenant_placement invariant is already a real DB trigger). Difficulty: careful — hardest W6 holder.

**`MisrouteAudit`** (`placement_of.rs:223`): `records: Arc<Mutex<Vec<...>>>`; **durable backing
fully exists** (`DurableMisrouteAuditBacking` + `misroute_audit` table, migration 0034, bound via
`DurablePlacementRegistry::record_misroute`). Just add the backend enum + wire `CellGateway` to
the durable sink. Difficulty: standard. Must land with Registry (shared self-host boot re-point).

## Execution order (serialized on the shared baseline file)

**W6a → W6b → W6c-events → W6c-cp → W6d.** W6a first (exact W2 mirror, validates wave machinery
cheaply, −2). W6b second (in-crate, carries fold-ins, −3; PostPit first as warm-up). W6c-events
third (DAG-sink trait seam, −1). W6c-cp fourth (design decision, −1). W6d last (highest blast
radius, −2). Region-scope sweep stays W7, but every W6 durable write persists real
`scope.region()` (no hardcoded fr-par) for forward-compat.

Full builder-prompt skeletons (goal/files/pattern/migrations/gates per sub-wave) are in the
grounding agent's report, summarized above; gates per sub-wave: baseline −N + count-asserts,
DB-free workspace build+test, `--features integration` live-PG durable proof, per-crate isolated
clippy on touched crates, kill-9/restart proof where the holder is a system-of-record (W6d).
