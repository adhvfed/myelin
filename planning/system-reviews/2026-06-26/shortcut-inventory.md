# Shortcut Inventory — make-it-real census synthesis (MR-003)

Date: 2026-06-26. Status: AUTHORITATIVE (consumed by every later build prompt). Author: MR-003.
Inputs: `census/mr-001-substrate-findings.md` (57 findings), `census/mr-002-git-findings.md` (11 findings).
Ranking context: `06-make-it-real-master-plan.md` (dogfood order Git → Actions/CI → chat → issues → docs),
`09-spine-prompt-ledger.md` (the MR-NNN spine set these feed).

`file:symbol` references are preserved verbatim from the census docs (verified there). No code was changed.

---

## Executive summary

- **68 raw findings → 66 deduped canonical shortcuts.** Two merges: the RLS pooled-connection bleed
  (`F-storage-2` ≡ `F-tenancy-3`) and the sandbox no-op exec (`F-sandbox-1` ≡ `F-sandbox-2`, same defect on
  both backends). Nothing dropped.
- **By severity: 17 CRITICAL · 26 HIGH · 18 MEDIUM · 5 LOW.** Three of the LOW/honest entries
  (`F-cp-12`, `F-cp-27`, `F-tenancy-2`) are *mostly real* — residual shortcut only; they also appear in
  Section C as "genuinely real — do not re-stub."
- **The two load-bearing themes** (root causes behind most CRITICAL/HIGH): (1) **the production graph runs on
  mock crypto** — every default verifier/signer in `identity_app_spec` parses/emits a plaintext pipe-delimited
  string, so any principal/tenant/grant-set is forgeable with no signature to defeat; and (2) **load-bearing
  state lives in `HashMap`/`BTreeMap`/`Mutex` in one process** — the "durable store / real broker / real
  backup / real KMS" is a deferred floor, the real backings that exist are `#[cfg(feature="integration")]` and
  compiled out of every default `cargo test`, and the tests assert serde/same-process shape, so a stub *is*
  what the green gate certifies.

### The 17 CRITICALs that gate the spine (and their fixing MR)

| SI | shortcut | fixed by |
|---|---|---|
| SI-001 | prod assembly wires `Structural*` + ephemeral KMS as the **default** | MR-012 (P-528) + MR-004 scanner |
| SI-002 | machine/capability token verify is mock crypto | MR-011 (P-527) + MR-012 |
| SI-003 | token mint is a plaintext string formatter | MR-011 (P-527) + MR-012 |
| SI-004 | human/SSO credential verify is mock crypto | MR-010 (P-526) + MR-012 |
| SI-005 | RLS session-GUC pooled-connection cross-tenant bleed | MR-013 (P-531) |
| SI-006 | KMS keys in-process; backup omits KEKs/root → key loss on restart | **GAP (near-term leg)** |
| SI-007 | transactional outbox is in-memory `HashMap` | **GAP** |
| SI-008 | relay publishes to in-process fake bus | **GAP** |
| SI-009 | no events `serve`/assembly (NATS→outbox→relay unwired) | **GAP** |
| SI-010 | control-plane DDL is dead; `MigrationRunner::run` executes no SQL | **GAP (foundational)** |
| SI-011 | placement registry is in-memory `BTreeMap` | **GAP** |
| SI-012 | git ref store is in-memory; `open` loads nothing | Git track (no MR yet) |
| SI-013 | no production `WireExecutor` (clone/push has no backing) | Git track (no MR yet) |
| SI-014 | backup is a modeled WAL offset, no bytes | Git track (no MR yet) |
| SI-015 | restore is modeled, no `pg_restore`/WAL replay | Git track (no MR yet) |
| SI-016 | sandbox `launch()` runs no command (both backends) | CI track (no MR yet) |
| SI-017 | AG-D4 escape corpus runs via drill harness, not `launch()` | CI track (no MR yet) |

### Ledger-coverage gaps (the headline for the orchestrator)

**Five CRITICALs have NO spine MR** (detail in Section B). The spine durable-persistence prompts (MR-007/008)
are scoped to the **identity** stores (principal/tuple/revocation/expiry) only — they silently exclude the
**events bus** and the **control-plane registry**, both of which are spine-substrate organs the dogfood leans
on from day one:

- **SI-007 / SI-008 / SI-009** — events outbox + relay + serve (the silent-data-loss floor) → author an
  events durable-persistence MR (P-522/523) + an events-serve MR (P-539) in W2/W3.
- **SI-010 / SI-011** — control-plane DDL execution + placement registry durability → author a control-plane
  durable-persistence MR; **SI-010 is foundational** — until `MigrationRunner::run` actually executes SQL,
  MR-007/008/009 cannot create the tables they bind to.
- **SI-006** — the KMS root/KEKs must survive a restart. Full HSM is Tier-4 (P-524/525), but a minimal
  durable-root slice is a hard prerequisite for **MR-009**'s kill-9/restart verify to mean anything (today
  `KmsEngine::new()` mints a fresh random root per process, so every encrypted column is unrecoverable after
  restart even once MR-007/008 land).

The Git (SI-012/013/014/015) and CI/sandbox (SI-016/017) CRITICALs are **correctly deferred to the subsystem
and long-pole tracks** per the master plan — but those tracks are "decomposed after the spine," so **no MR-NNN
exists for them yet**: git ref-store durability, the git server binary, real backup/restore, and the sandbox
production exec/escape path all still need prompts authored.

---

## Section A — Ranked shortcut inventory

Ranked by **severity** first, then **dogfood proximity** (master-plan order). Tie-break within a severity:
spine/auth → spine/tenant-isolation → spine/persistence → git (Tier 1) → actions-ci (Tier 3) →
chat-issues-docs / gdpr → multi-cell (deferred). Auth leads the spine because the census names the auth bypass
the single most dangerous defect and it is the first boundary the dogfood hits.

| SI-NNN | sev | shortcut (one line) | canonical location `file:symbol` | source findings | fixed by MR-NNN (P-NNN) | dogfood surface |
|---|---|---|---|---|---|---|
| SI-001 | CRITICAL | Prod assembly wires `Structural*` verifiers/signers + a fresh ephemeral KMS as the DEFAULT graph | `myelin-identity-service/src/lib.rs:identity_app_spec` (1534); `StoreBackedCheck::with_index` (540–550) | F-identity-7 | MR-012 (P-528) + MR-004 scanner; KMS leg → MR-007/008 share one KMS / Tier-4 P-524 | spine/auth |
| SI-002 | CRITICAL | Machine/capability token verification is mock crypto (parses a 6-field pipe string) | `myelin-identity-service/src/machine_auth.rs:StructuralTokenVerifier` (262–339) | F-identity-2 | MR-011 (P-527) + MR-012 (P-528) | spine/auth |
| SI-003 | CRITICAL | Token minting is a plaintext string formatter, not a signer | `myelin-identity-service/src/mint.rs:StructuralTokenSigner` (164–189) | F-identity-3 | MR-011 (P-527) + MR-012 (P-528) | spine/auth |
| SI-004 | CRITICAL | Human/SSO credential verification is mock crypto (splits `material` on `\|`) | `myelin-identity-service/src/authenticate.rs:StructuralVerifier` (146–184) | F-identity-1 | MR-010 (P-526) + MR-012 (P-528) | spine/auth |
| SI-005 | CRITICAL | RLS GUC is SESSION-scoped on a pooled conn with no reset-on-release + a bare `pool()` hatch → cross-tenant bleed | `myelin-storage/src/pg.rs:set_session_scope_in_region` (413); `PgStore::pool` (150–152) | F-storage-2, F-tenancy-3 | MR-013 (P-531) | spine/tenant-isolation |
| SI-006 | CRITICAL | KMS holds all key material in-process; `backup_snapshot` omits KEKs/root → total key loss on restart/restore | `myelin-storage/src/kms.rs:KmsEngine` (470–478); `KmsEngine::backup_snapshot` (685–698) | F-storage-7 | Tier-4 P-524/525; **near-term root-persistence leg = GAP** (gates MR-009) | spine/persistence |
| SI-007 | CRITICAL | Transactional outbox is an in-memory `HashMap`, not a DB table | `myelin-events/src/outbox.rs:OutboxStore` (229), `Inner` (199) | F-events-1 | P-522/523 — **GAP (MR-007/008 are identity-only)** | spine/persistence |
| SI-008 | CRITICAL | Relay publishes to an in-process fake bus; "delivery" is a sync method call | `myelin-events/src/relay.rs:InProcessBus` (138) | F-events-3 | P-522/523 + P-539 — **GAP** | spine/persistence |
| SI-009 | CRITICAL | No events production assembly/`serve` wiring NATS→outbox→relay | `myelin-events/src/lib.rs` (no `serve` fn) | F-events-11 | P-539 + P-522/523 — **GAP** | spine/persistence |
| SI-010 | CRITICAL | Control-plane DDL is dead string constants; `MigrationRunner::run` never executes SQL | `myelin-control-plane/src/lib.rs:control_plane_migrations` (322–386); `myelin-substrate/src/migrations.rs:MigrationRunner::run` (108–141) | F-cp-2 | P-522/523 + P-531 — **GAP (foundational to all persistence)** | spine/persistence |
| SI-011 | CRITICAL | Placement registry is an in-memory `BTreeMap`; restart loses all tenant→cell routing | `myelin-control-plane/src/registry.rs:Registry` (110–127) | F-cp-1 | P-522/523 — **GAP (cp)** | spine/persistence |
| SI-012 | CRITICAL | Git ref store (every repo's entry point) is in-memory `Mutex<BTreeMap>`; `open` loads nothing | `myelin-git/src/receive_pack.rs:RefStore` (537); `RefStore::open` (560–575) | F-git-1 | P-522/523 + GIT-P20 — **Git track (no MR yet)** | git |
| SI-013 | CRITICAL | No production `WireExecutor`; clone/push/upload-pack byte-serving has no backing | `myelin-git/src/core.rs:WireExecutor` (268, all impls `#[cfg(test)]`) | F-git-3 | GIT-P13 (depends P-544) — **Git track (no MR yet)** | git |
| SI-014 | CRITICAL | Backup is a modeled WAL offset + metadata-only tier — no bytes | `myelin-storage/src/backup.rs:ContinuousArchiver` (259–389), `ObjectTierBackup` (398–477) | F-storage-4 | P-529/530 — **Git Tier-1 track (no MR yet)** | git |
| SI-015 | CRITICAL | Restore is a modeled `restore_to_offset` over in-memory rows — no `pg_restore`/WAL replay | `myelin-storage/src/restore.rs:restore_to_offset` (360–415) | F-storage-5 | P-529/530 — **Git Tier-1 track (no MR yet)** | git |
| SI-016 | CRITICAL | Sandbox production `launch()` runs no command (FC boots `init=/bin/true`; gVisor probes `runsc --version`) | `myelin-ci-sandbox/src/firecracker.rs:FirecrackerBackend::launch` (327); `gvisor.rs:spawn_real_runsc` (227) | F-sandbox-1, F-sandbox-2 | P-544 — **CI track (no MR yet)** | actions-ci |
| SI-017 | CRITICAL | AG-D4 escape corpus runs through bespoke drill harnesses, NOT production `launch()` | `myelin-ci-sandbox/src/firecracker.rs:drill_config_json` (414); `gvisor.rs:gvisor_drill_config_json` (352) | F-sandbox-4 | P-545 (depends P-544) — **CI track (no MR yet)** | actions-ci |
| SI-018 | HIGH | S1 principal store (identity system-of-record) is an in-memory `HashMap` | `myelin-identity-service/src/principal_store.rs:Inner` (249–271); `PrincipalStore::new` (295) | F-identity-5 | MR-007 (P-522) + MR-009 (P-523) | spine/persistence |
| SI-019 | HIGH | S3 relation-tuple store (ReBAC authz graph) is an in-memory `HashMap` | `myelin-identity-service/src/tuple_store.rs:Inner` (200–211) | F-identity-6 | MR-007 (P-522) + MR-009 | spine/persistence |
| SI-020 | HIGH | Revocation/S7 denylist in-memory; the "durable mirror" + "crash" are same-process map copies | `myelin-identity-service/src/machine_auth.rs:S7Denylist` (347–372); `revocation.rs:Inner` (145–162) | F-identity-4 | MR-008 (P-522) + MR-009 | spine/persistence |
| SI-021 | HIGH | OLTP "pool" (system of record) is an in-memory permit counter, not a DB | `myelin-storage/src/oltp.rs:OltpPool` (128–218) | F-storage-1 | MR-007 (P-522) + MR-009 | spine/persistence |
| SI-022 | HIGH | All real backings (PG/S3/Valkey/RLS) behind `--features integration`; default build+CI 100% in-memory | `myelin-storage/Cargo.toml` `default=[]`; `src/backend.rs:blob_store/cache` (33–56) | F-storage-3 | MR-007 + MR-013 + MR-009 (put real backings on default gate) | spine/persistence |
| SI-023 | HIGH | Consumer-dedup ledger is an in-memory `HashSet`, not a DB table | `myelin-events/src/dedup.rs:DedupLedger` (87) | F-events-2 | P-522/523 — **GAP (events)** | spine/persistence |
| SI-024 | HIGH | Consumer cursor/lag/in-flight all in-memory; no broker cursor exists | `myelin-events/src/consumer.rs:Consumer` (370) | F-events-5 | P-522/523 + P-539 — **GAP** | spine/persistence |
| SI-025 | HIGH | Real NATS JetStream client compiled out of every default build/test | `myelin-events/src/nats.rs:NatsJetStreamBus` (37) | F-events-4 | P-522/523 + P-539 — **GAP** | spine/persistence |
| SI-026 | HIGH | `place()` stickiness is process-lifetime, not durable | `myelin-control-plane/src/place.rs:PlacementService::place` (236–284) | F-cp-4 | P-522/523 — **GAP (cp)** | spine/persistence |
| SI-027 | HIGH | Provisioning gate runs real restore-verify/KMS logic over modeled inputs; persists nothing | `myelin-control-plane/src/provision.rs:provision_cell`/`decommission_tenant` (223–329) | F-cp-5 | P-522/523 — **GAP (cp)** | spine/persistence |
| SI-028 | HIGH | Gateway misroute decision real, but audit is a volatile `Arc<Mutex<Vec>>` + dead `cross_tenant_reads` counter | `myelin-control-plane/src/placement_of.rs:CellGateway::route` (358–405); `MisrouteAudit` (222–260) | F-cp-6 | P-531 (decision) + P-522/523 (durable audit) | spine/persistence+isolation |
| SI-029 | HIGH | Object `oid→ContentHash` index is in-memory, never rebuilt on open (bytes durable, retrieval not) | `myelin-git/src/pack_tier.rs:PackObjectDb::new` (145–156); `receive_pack.rs:InMemoryObjectDb` (427) | F-git-2 | P-522/523 + GIT-P11/P20 — **Git track** | git |
| SI-030 | HIGH | No runnable git server — no binary, no bound listener; all assembly is in `tests/` | `myelin-git/src/front_door.rs:FrontDoor` (295+) (no `main.rs`/`[[bin]]`) | F-git-4 | MR-015 (product API) partial; E0.6/E1.2 — **Git/API track** | git |
| SI-031 | HIGH | No exit/stdout/stderr capture, no timeout-kill, no limit enforcement; metering settles on a no-op boot | `myelin-ci-sandbox/src/firecracker.rs` (289–297); `gvisor.rs` (195–201) | F-sandbox-3 | P-544/545 — **CI track** | actions-ci |
| SI-032 | HIGH | CI runner's production dispatch inherits the no-op exec | `myelin-ci-sandbox/src/runner.rs:612` (`backend.launch`) | F-sandbox-5 | P-544/545 — **CI track** | actions-ci |
| SI-033 | HIGH | `residency_verify` trusts a declared `region` label — never probes where data lives | `myelin-control-plane/src/residency_verify.rs:residency_verify_over` (469–517) | F-cp-11 | P-531 (MR-013 partial) + Storage P-ST-07 (store-probe leg uncovered) | isolation/residency |
| SI-034 | HIGH | `mirror_allowed` is advisory — counts `Deny` verdicts, not blocked egress; trusts declared region | `myelin-control-plane/src/mirror_allowed.rs:mirror_allowed` (250–297) | F-cp-16 | P-531 + contract 10.5; enforcement deferred to `myelin-git` | isolation/residency |
| SI-035 | HIGH | Runner-claim region pin trusts a declared `runner_region` (no allowlist/attestation/backend) | `myelin-control-plane/src/runner_claim_pin.rs:admit_claim` (185–200) | F-cp-24 | P-531 + CI-R3 — **CI/isolation track** | actions-ci/residency |
| SI-036 | HIGH | Restore-verify gate verifies a fresh in-memory `RestoreTarget`, never a restored DB | `myelin-storage/src/restore_verify.rs:RestoreVerifyGate::run` (440–527) | F-storage-6 | P-530 — **Git Tier-1 track** | git |
| SI-037 | HIGH | Firehose (resume-cursor live transport) is an in-memory `HashMap` of ring buffers | `myelin-events/src/firehose.rs:Firehose` (618) | F-events-8 | P-522/523 + P-539 — **GAP (chat/issues live transport)** | chat-issues-docs |
| SI-038 | HIGH | Crypto-shred / `PersonalDataHolder` runs against an in-memory `BTreeSet` "KMS" | `myelin-events/src/holder.rs:InMemoryShredder` (140) | F-events-6 | P-522/523 (real KMS binding) — **GDPR track** | gdpr/erasure |
| SI-039 | HIGH | Post-restore re-erasure ledger is in-memory; "restore" is a hand-built resurrection | `myelin-events/src/reerase.rs:BusErasureLedger` (102) | F-events-7 | P-522/523 — **GDPR track** | gdpr/erasure |
| SI-040 | HIGH | Crypto-shred is an in-memory `BTreeMap::remove` (`destroy_kek`), no HSM/backup reach | `myelin-control-plane/src/migration.rs` (441–443) → `myelin-storage/src/kms.rs:KmsEngine::destroy_kek` (664) | F-cp-22 | P-522/523 + Storage 11.3 — **GDPR/Tier-4** | gdpr/erasure |
| SI-041 | HIGH | Repo placement in-memory map; relocation flips a pointer without moving bytes | `myelin-control-plane/src/placement_of_repo.rs:register_repo`/`relocate_repo` (233–326) | F-cp-8 | P-522/523 + P-CP-22; residency → P-531 — **Git/cp track** | git |
| SI-042 | HIGH | Live migration "moves" zero bytes — `clone()`s an in-process struct | `myelin-control-plane/src/migration.rs:LiveMigration::migrate_tenant` (351–455) | F-cp-18 | P-522/523 + M5 CP-D7 — **multi-cell (deferred)** | multi-cell |
| SI-043 | HIGH | Cross-cell DSR fan-out operates on in-memory eraser maps — no store touched | `myelin-control-plane/src/multi_cell.rs:CrossCellDsrFanOut::fan_out` (201–233) | F-cp-26 | P-522/523 + P-531 + P-CP-20 — **multi-cell GDPR (deferred)** | multi-cell |
| SI-044 | MEDIUM | RLS guard module is a type/string model; `resolve` hardcodes the verdict, `predicate_sql` never runs | `myelin-storage/src/rls.rs:TenantScope::resolve` (110–123); `TenantQuery::predicate_sql` (210–217) | F-storage-8 | MR-013 (P-531) | spine/tenant-isolation |
| SI-045 | MEDIUM | `four_layer` write boundary is `Region==Region`; `out_of_region_writes_admitted` is a dead counter | `myelin-control-plane/src/four_layer.rs:check_write` (166–179) | F-cp-13 | P-531 + Storage RLS twin (P-522/523) | isolation |
| SI-046 | MEDIUM | Region-immutability "layer 1" enforced only by absence of a setter; `pub region` fields freely settable | `myelin-control-plane/src/registry.rs` (21–27); `schema.rs:83,118` | F-cp-9 | P-531 + P-522/523 | isolation |
| SI-047 | MEDIUM | `schema.rs` is plain Rust structs, not DB-mapped tables (struct/DDL drift uncaught) | `myelin-control-plane/src/schema.rs` (1–191) | F-cp-3 | P-522/523 + P-531 | spine/persistence |
| SI-048 | MEDIUM | `discover()` "JOIN" is two map lookups + an O(n) slug scan (fail-static cache is real) | `myelin-control-plane/src/discover.rs:Registry::discover` (138–156) | F-cp-7 | P-522/523 | spine/persistence |
| SI-049 | MEDIUM | "Durable workflow" decorative — migration steps run synchronously outside the run; no resume | `myelin-control-plane/src/migration.rs:migrate_tenant` (386–443); `myelin-flow/src/executor.rs` (252) | F-cp-19 | P-522/523 + contract 9.1 | multi-cell |
| SI-050 | MEDIUM | Migration "0-loss numbers" are structurally fixed, not measured from the target | `myelin-control-plane/src/migration.rs` (410–453) | F-cp-20 | P-522/523 + CP-D7 | multi-cell |
| SI-051 | MEDIUM | `relocate_repo_durably` claims copy→reindex→shred but only flips a registry row | `myelin-control-plane/src/migration.rs:relocate_repo_durably` (498–529) | F-cp-21 | P-522/523 | multi-cell/git |
| SI-052 | MEDIUM | Cross-cell bridge resolves via an in-memory resolver registry, no reachability probe; dead counter | `myelin-control-plane/src/cross_cell_bridge.rs:CrossCellBridge::resolve` (326–349) | F-cp-23 | P-531 + P-CP-19 | multi-cell |
| SI-053 | MEDIUM | Cross-cell zookie consistency is arithmetic over two caller-supplied timestamps | `myelin-control-plane/src/multi_cell.rs:CrossCellZookieReader::read_through` (320–341) | F-cp-25 | P-531 + P-CP-20 | multi-cell |
| SI-054 | MEDIUM | `CellFleet` bulkhead in-memory counters; cross-cell impact 0 by loop construction; "RED" is a formula | `myelin-control-plane/src/bulkhead.rs:CellBulkhead.offer` (215–238); `CellFleet.run_surge` (360–415) | F-cp-14 | P-531 + CP-D5 | multi-cell |
| SI-055 | MEDIUM | `ControlPlane` outage is an `AtomicBool`; degrade-not-cascade is one path checking a bool | `myelin-control-plane/src/cp_outage.rs:ControlPlane.hard_down`/`is_down` (171–183) | F-cp-15 | P-531 | multi-cell |
| SI-056 | MEDIUM | `retention.rs` ships hardcoded "measured" constants; no retention enforced anywhere | `myelin-events/src/retention.rs:StreamClass::tuning` (95) | F-events-9 | P-522/523 | chat-issues-docs |
| SI-057 | MEDIUM | Cross-cell propagation is in-process fan-out with a hardcoded-zero PII tripwire | `myelin-events/src/crosscell_propagation.rs:CrossCellPropagator` (187) | F-events-10 | P-522/523 + cross-cell epic | multi-cell |
| SI-058 | MEDIUM | "Tenant-isolation primitive" is value types only — zero enforcement/state/RLS | `myelin-tenancy/src/lib.rs` (1–695) | F-tenancy-1 | MR-013 (P-531) — enforcement lives in storage | spine/tenant-isolation |
| SI-059 | MEDIUM | Web UI is server-rendered HTML strings; e2e drives STATIC files, degrades to `partial` | `myelin-git/src/web.rs` (render fns: `ForkTrustBadge::render` 204, `CheckRowView::render` 276) | F-git-web-1 | E0.7 SolidJS (MR-016..019) / E1.3 — view-model source ONLY | git/ui |
| SI-060 | MEDIUM | API/CLI surface is a route/command CATALOGUE, not a served API or dispatching binary | `myelin-git/src/api.rs:http_catalogue()` (120); `CliCommand`/`handler()` (202,255) | F-git-api-1 | MR-015 (product API) + MR-020 (CLI); E0.6/E0.9 | git/api |
| SI-061 | MEDIUM | Git crypto-shred "reach" verifies only the blob DEK; reflog/bitmap/pack reach is a hardcoded enum | `myelin-storage/src/git_shred.rs:GitCryptoShredReach::shred_git_structures` (248–277) | F-storage-9 | P-532/533; E1.1 Git track | gdpr/erasure |
| SI-062 | LOW | `partition_key` tier-invariance is a struct-shape tautology (no RLS) | `myelin-control-plane/src/isolation.rs:PartitionKey::for_tier` (180–183) | F-cp-10 | P-531 | isolation |
| SI-063 | LOW | `DependencyBreaker`/`Scope::Cell` fault injector is decorative in the D4/D5 drills | `tests/cp_d4_blast_radius_drill.rs`; `tests/cp_d5_cell_bulkhead_surge_drill.rs` | F-cp-17 | testing-strategy T-3 | testing |
| SI-064 | LOW | (mostly REAL) `SignedAttestation` keyed-BLAKE3 MAC is genuine crypto; residual = in-process key | `myelin-control-plane/src/residency_verify.rs:ResidencySigningKey::mac` (320–323) | F-cp-12 | Storage P-ST-04 (key provenance) — see Section C | isolation |
| SI-065 | LOW | (honest floor) `CrossCellPointer` is FROZEN-NOT-LIVE — bridge type with no resolution path, by design | `myelin-tenancy/src/lib.rs:CrossCellPointer` (385–441) | F-tenancy-2 | P-531 / P-CP-19 — see Section C | multi-cell |
| SI-066 | LOW | (mostly REAL) Self-host "bootstrap" is an in-memory one-row registry; parity is genuinely tested | `myelin-control-plane/src/self_host.rs:DegenerateControlPlane` (102–301) | F-cp-27 | P-CP-13/P-CP-23 (durability rides SI-011) — see Section C | self-host |

---

## Section B — Ledger-coverage gaps

Definition: a gap is a CRITICAL/HIGH shortcut for which **no current spine MR (MR-001..021) is responsible.**
Split into (B1) spine-substrate organs the spine ledger *should* cover but doesn't — these may require the
orchestrator to author extra prompts — and (B2) shortcuts correctly routed to subsystem/CI tracks that are
not yet decomposed into MRs.

### B1 — Spine gaps (author extra prompts; sequence in W2/W3 alongside MR-007/008)

The root cause: **MR-007 ("principal + tuple") and MR-008 ("revocation + expiry") are scoped to the identity
crate only.** They are the entire spine's durable-persistence coverage, yet the events bus and the
control-plane registry are equally load-bearing spine substrate.

| gap | SIs | severity | why it's a spine gap | recommended sequencing |
|---|---|---|---|---|
| **Migration runner executes no SQL** | SI-010 | CRITICAL | `MigrationRunner::run` only lints DDL then `applied.push(id)`; nothing opens a connection. **Foundational** — MR-007/008/009 cannot create the tables they bind to until this is real. | Author first, as a W2 prerequisite to MR-007 (or fold into MR-007's scope). |
| **Events bus durability** | SI-007, SI-008, SI-009, SI-023, SI-024, SI-025, SI-037 | 3×CRITICAL + 4×HIGH | Outbox/dedup/relay/consumer/NATS/firehose are all in-memory; the real `NatsJetStreamBus` is compiled out by default. The outbox is the silent-data-loss floor every subsystem rides. No MR owns it. | Author an events durable-persistence MR (P-522/523) in W2 + an events-`serve` MR (P-539) in W3, mirroring MR-007/009. |
| **Control-plane registry durability** | SI-011, SI-026, SI-027, SI-028 | 1×CRITICAL + 3×HIGH | Placement registry, stickiness, provisioning activation, and the misroute audit are all in-memory `BTreeMap`/`Vec`; restart loses all tenant→cell routing. No MR owns it. | Author a control-plane durable-persistence MR (P-522/523) in W2/W3. |
| **KMS root/KEK persistence (near-term slice)** | SI-006 | CRITICAL | Full HSM-KMS is correctly Tier-4 (P-524/525), but `KmsEngine::new()` mints a fresh random root per process and `backup_snapshot` omits the KEKs/root — so **MR-009's kill-9/restart verify is hollow** (every encrypted column is unrecoverable after restart even once the stores are durable). | Author a minimal durable-KMS-root slice in W3, before/with MR-009. Keep HSM/Shamir in Tier-4. |

Also note **SI-022** (HIGH): MR-007/MR-013 must move the real PG/S3/Valkey backings onto the *default* gate —
today they are `--features integration` and never run in `cargo test`, so MR-009's verify must explicitly run
the integration path or the durability proof is against the in-memory model again.

### B2 — Correctly deferred to subsystem/CI tracks (no MR authored yet)

These are *not* spine gaps — the master plan routes them to the Git Tier-1, CI long-pole, GDPR, and multi-cell
tracks, which are "decomposed after the spine." The orchestrator must author those tracks; flagged here so the
daily-driver-gating ones aren't forgotten.

- **Git track (E1.1/E1.2; P-529/530, GIT-P11/P13/P20):** SI-012 (ref store, C), SI-013 (wire executor, C),
  SI-014 (backup, C), SI-015 (restore, C), SI-029 (oid index, H), SI-030 (git server binary, H),
  SI-036 (restore-verify, H), SI-041 (repo placement, H). **Git ref-store durability (SI-012), the git server
  binary (SI-030), and real backup/restore (SI-014/015) are the daily-driver-gating criticals with no prompt.**
- **CI/sandbox long-pole track (Tier 3; P-544/545):** SI-016 (no-op exec, C), SI-017 (escape corpus, C),
  SI-031 (no capture/timeout, H), SI-032 (runner inherits, H), SI-035 (runner-claim pin, H).
- **GDPR/erasure track:** SI-038, SI-039, SI-040 (H), SI-061 (M) — all need the real KMS binding.
- **Multi-cell (deferred per master plan):** SI-042, SI-043 (H) + the MEDIUM multi-cell tail
  (SI-049/050/052/053/054/055/057).

---

## Section C — Duplicate-risk map (extend, don't fork)

Surfaces where a build agent could FORK a second implementation instead of extending the existing one. For
each: the canonical thing (`file:symbol`) and the rule. **Step 1 of every build prompt is the anti-duplication
grep + ledger-vs-commit cross-check (ledger §Conventions); this table is its index.**

### Seams that already exist — inject the real impl behind them, do not add a parallel one

| surface | canonical `file:symbol` | rule |
|---|---|---|
| Token/credential verifiers + signer | `machine_auth.rs:CapabilityAuthenticator::with_verifier` (403–409); `authenticate.rs:HumanSsoAuthenticator::with_verifier` (288–290); `mint.rs` signer seam | Implement real OIDC/SAML/WebAuthn/PASETO/biscuit/DPoP crypto as a new verifier/signer injected through the **existing `with_verifier`/`with_signer`/`with_kms` seams**. Replace the `Structural*` defaults in the prod graph; keep `Structural*` as `#[cfg(test)]`. Do NOT create a second authenticator. |
| Production composition root | `myelin-identity-service/src/lib.rs:identity_app_spec` (1534); `StoreBackedCheck::with_index` (540–550) | One identity assembly. Swap the stubs it constructs for durable stores + real KMS behind the same `AppSpec`. Do NOT author a parallel `serve`/spec. |
| KMS | `myelin-storage/src/kms.rs:KmsEngine` (crypto core is genuinely real) | One KMS. Make `KmsEngine` durable/HSM-backed behind its existing API; `identity_app_spec` must **share** one KmsEngine, not mint a fresh per-process root. Do NOT write a second key manager. |
| Git ref store | `myelin-git/src/receive_pack.rs:RefStore` / `RefStore::open` (560–575) | Make `RefStore` durable (reftable-on-OLTP) behind its existing `open`/CAS API. Do NOT fork a second ref store — the durability swap goes here. |
| Git object DB | `myelin-git/src/pack_tier.rs:PackObjectDb` + `PackTierMigration` (write-through is real) | Hydrate the `oid_index` from the pack tier on `open`; extend the existing `PackObjectDb`. Do NOT add a third `QuarantineMigration`/object DB beside `InMemoryObjectDb`/`PackTierMigration`. |
| Events bus / broker client | `myelin-events/src/nats.rs:NatsJetStreamBus` (real, integration-gated) | Wire the existing `NatsJetStreamBus` into the relay/outbox `serve` path. Do NOT write a new broker client; the in-memory `InProcessBus` stays a test double. |
| Outbox / dedup store | `myelin-events/src/outbox.rs:OutboxStore`; `dedup.rs:DedupLedger` (the frozen `OUTBOX_MIGRATION`/`CONSUMER_DEDUP_MIGRATION` DDL is the schema) | Bind these to Postgres behind their existing API + the named migration shape. Do NOT define a second outbox/dedup table. |
| Tenant RLS scoping | `myelin-storage/src/pg.rs:set_session_scope_in_region` (413); `PgStore` | Fix scoping IN PLACE (`SET LOCAL`/`set_config(...,true)` + `after_release`/`DISCARD ALL`) and remove the bare `PgStore::pool` hatch. Enforcement lives HERE, not in `myelin-tenancy` (which is, by design, glue-type vocabulary only — do not build a parallel tenant guard there). |
| Control-plane self-host | `myelin-control-plane/src/self_host.rs:DegenerateControlPlane` (delegates to shared `Registry`/`PlacementService`/`CellGateway`) | Already non-forking by design (parity is drill-tested). Keep it **delegating** to the shared organs when they're made durable; do NOT give it its own stores. |
| Product API / CLI grammar | `myelin-git/src/api.rs:http_catalogue()` / `CliCommand` (the route/command catalogue + the `Id.check`-gated-write invariant) | MR-014/015 (edge API) and MR-020 (CLI) **reuse this grammar**; expose it via the edge `serve`. Do NOT author a parallel route table or CLI verb set. |
| Git web view-models | `myelin-git/src/web.rs` render functions (`ForkTrustBadge::render`, `CheckRowView::render`, …) | Use `web.rs` as a **view-model source only**. The real interactive surface is the E0.7 SolidJS app (MR-016..019). Do NOT harden `web.rs` into "the UI", and do NOT re-derive its view-models from scratch in Solid — port them. |

### Genuinely real — do NOT re-stub (MR-002 counter-list + the honest substrate organs)

These are load-bearing and correct today; a build agent must extend, never replace or re-stub them:

- **`GixCore` read backend over `git2`/libgit2** — `myelin-git/src/gix_backend.rs:56-160` (`read_blob`/`diff_blobs`/`blame`); `core.rs:296` (`ReadBackend`). The one genuinely runnable git organ. (gix-preferred swap is the OQ-1/GIT-P33 floor.)
- **`PgCheckStatusProjection`** — `myelin-git/src/check_status_store.rs:51,105-138` — a real sqlx/Postgres projection with in-transaction `event_id` idempotency (note the asymmetry: check-status is on real Postgres while the *ref* store, SI-012, is not).
- **`FsBlobStore` + pack-tier write-through for object bytes** — `myelin-storage/src/blob.rs:362,437` (real content-addressed `fs::write`), consumed by `pack_tier.rs:PackTierMigration`. Byte durability is real; only the oid index (SI-029) is volatile.
- **Sandbox hardening profile + config builders** — `firecracker.rs:FcMachineConfig`, `gvisor.rs:OciConfig`, `hardening.rs` (read-only root / no-NIC / caps-dropped / nnp / seccomp / pids). The *recipe* is real; P-544 adds **executing a job under it** — extend, do not rewrite the config builders.
- **Storage crypto core** — RustCrypto AES-256-GCM wrap/unwrap, real crypto-shred, BLAKE3 content-addressing, fail-static ladder (MR-001 §B). The rot is in persistence/backup/RLS, not the crypto primitives.
- **`SignedAttestation` keyed-BLAKE3 MAC** (SI-064) — `residency_verify.rs:ResidencySigningKey::mac` — real tamper-evident crypto; only residual is in-process key provenance (Storage P-ST-04).
- **The `myelin-tenancy` type discipline** — opaque `TenantId` (no `From<String>`), no `Region` setter, PII-free `CrossCellPointer` (SI-065). Good and intentional; keep the vocabulary, add enforcement in storage (not here).

> Note (compile-time guard, keep): `myelin-storage/src/rls.rs` carries a genuine `compile_fail` doctest that
> prevents minting a `TenantScope` from a path — that guard is real and valuable (SI-044 is only about the
> runtime `resolve`/`predicate_sql` being a model). Preserve the compile-time guard when fixing SI-044.
