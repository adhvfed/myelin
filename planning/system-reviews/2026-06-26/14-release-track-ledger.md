# Release-Track Ledger — R0–R6 execution (plan: `planning/08-release/01-technical-release-plan.md`)

Date: 2026-07-06. Status: EXECUTING (R0). This ledger tracks execution of the R-track; the plan doc holds
the full rationale and source-finding citations — this file records what actually happened, per item, with
commits and proof. Phases R1/R2 largely re-enter ledger 13 (MR-009b W3b–W7) with the review HIGHs folded
into their waves; those waves keep reporting in ledger 13, and this ledger records only the fold-ins.

## Conventions

Same as ledgers 09–13: one builder per prompt (anti-duplication grep opens each), orchestrator runs the
full gate (`cargo build --workspace` + `cargo test --workspace` DB-free; `--features integration` on the
live docker stack where touched), **independent adversarial verifier (never the builder) on every
security item**, commit per item. Every R0 item's exit proof is an adversarial test that fails before the
fix and passes after — the proof column in the plan doc is the acceptance contract.

## R0 — stop-the-bleed (live security)

| # | Item | Source | Status | Commit(s) | Proof |
|---|---|---|---|---|---|
| R0.1 | Fail-closed Firecracker NIC (no egress NIC without applied+attested per-tap egress firewall) | ci #1, DELTA now-live HIGH | DONE | `21b5848` | `EnforcedEgress` record gates the NIC; mintable only by `nft` apply of a default-drop ruleset; fail-closed on hostname/apply-fail; `assert_enforced` honest; independent verifier CONFIRMED-SOUND; 113 tests. Follow-up: serde-hardening note. |
| R0.2 | Wire push evaluates merge-gate + per-repo branch-protection ruleset (kill `PushPolicy::default()`) | DELTA N1 HIGH | DONE | `f0df43e` | `evaluate_protected_ref_push` reuses `merge_gate`; ruleset from repo-owned `BranchProtectionConfig`; rejects delete/force/CI-not-green before ref-CAS. Verifier HOLD (fail-open on corrupt config) → fixed with `get_protection` fail-closed + regression test. CONFIRMED. |
| R0.3 | Per-repo object authz on wire routes (read+write) — seeds the R2 platform seam | DELTA N2 HIGH | DONE (seam) | `f0df43e` | `RepoAuthorizer` seam consulted in all 3 handlers; READ-deny 0-leak 404 / WRITE-deny 403; fixtures load-bearing. Verifier CONFIRMED-SOUND. **Latent until R2 (see R2.1 note):** prod `main.rs` injects only `AllowAllRepos` and does not `register_git_wire`. |
| R0.4 | Git crash reconciler: durable monotonic generation replaces reflog-length `update_seq` | git #1 HIGH | DONE | `c221b2e` | git-config `myelin.refgen.<hex>` counter, survives delete+recreate; write-path+reconcile switched; independent verifier CONFIRMED-SOUND; 432 tests |
| R0.5 | Wire HTTP body bounded at front door (stream+cap, 413) | DELTA N3 | DONE | `38d77cf` | `collect_bounded` frame-by-frame cap 100 MiB + Content-Length fast-reject + canonical 413; 7 tests; self-reviewed |
| R0.6 | Dev-login env guard (explicit flag AND non-prod build; loud audit) | fe-web auth bypass | DONE | `2209203` | `devLoginAllowed` requires `NODE_ENV!=production` AND `MYELIN_DEV_LOGIN=1`; refuses loud + fail-closed; unit-tested both directions; vitest+tsc+eslint green |
| R0.7 | Hygiene batch: shallow-push connectivity (N4), `digest_pinned` length, config Debug redaction, CLI token chmod | DELTA N4 + lows | DONE | `e5355d0` (A/B/C), `5f47dd8` (D) | A: CLI token atomic 0600-before-write. B: `digest_pinned` per-algo length (+4 downstream fixture crates padded). C: `S3Config`/`MyelinConfig` `Debug` redacts secrets. D: `history_connectivity_complete` full-ancestry walk, wired into the accept gate. |

R0 exit: all seven rows DONE with adversarial proofs; verifier sign-off recorded per security row.

**R0 COMPLETE (2026-07-06).** All 7 items landed across commits `21b5848` (R0.1), `f0df43e` (R0.2/R0.3),
`c221b2e` (R0.4), `38d77cf` (R0.5), `2209203` (R0.6), `e5355d0` (R0.7-A/B/C), `5f47dd8` (R0.7-D). Every
security item passed builder → independent adversarial verifier → commit; the one defect the process
caught (R0.2 fail-open on a corrupt branch-protection config) was fixed + regression-tested before commit.
Carried forward to R2 as **R2.1a**: wire R0.2/R0.3 live (inject a real `RepoAuthorizer`, `register_git_wire`
in production `main.rs`) — the gates are correct but latent until then. Workspace build clean; per-crate
gates green; the `drills_git_d9`/`DedupLedger::new` failure is a pre-existing MR-009b (W3) integration-gated
item, addressed in R1, not R0.

## R1 — MR-009b completion (executes in ledger 13; fold-ins tracked here)

**R1 grounding (2026-07-07).** Warm-up done: `DedupLedger` test-support dev-dep fixed (`5340c47`).
**Current scanner baseline (authoritative, via `cargo test -p myelin-lints production_graph_absence`):
`no-in-memory-durable-store` = 12** (+ 1 `no-structural-crypto-in-prod` residency_drill survivor, out of
scope). The 12: `OutboxStore` (W3b), `KmsEngine` (W5), and the W6/W7 cluster — `BusErasureLedger`,
`Registry` (W6d), `CellResolverRegistry`, `MisrouteAudit`, `PseudonymStore`, `PseudonymErasureLedger`,
`CostLedger`, `ErasureLedger`, `InMemoryPostPitLedger`, `FsBlobStore` (W7).

**W3b SCOPE CORRECTION — bigger than ledger-13's "13→12" line.** `OutboxStore` (`myelin-events/outbox.rs:230`)
is the in-memory event-emit seam, and unlike `DedupLedger` it has NO backend enum (the in-memory-ness IS the
struct). The scanner fires on the struct definition, so flipping it green requires gating the whole type
behind `test-support` — which requires that **no production code references it**. But `OutboxStore` is used
in PRODUCTION lib code across ~15 crates (flow `WfCtx`/engine/dogfood, git `RefStore`/code_projection, issues
write_path/reorder/import, notif escalation/router/reindex, storage coloc/olap_feed, substrate serve, knowledge,
search, refs-service, edge, chat[already re-pointed]). So W3b is a **platform-wide event-emit re-point**, not a
small wave. Architectural blocker: `myelin-identity-service` and `myelin-events` are DAG sinks that cannot depend
on `myelin-storage` (where `PgRelay::co_commit_in_tx` lives) → they cannot call the durable relay directly; the
durable co-commit primitive must be reachable from a low crate, or their emit must route through a caller that
owns both the state tx and the relay. Also unresolved: the in-process serving floor (`InProcessBus` + in-mem
`OutboxStore`, used by substrate `default_inproc`, flow `app`, notif `lib`, knowledge `lib`) — under MR-009b
doctrine (durable-by-default, in-memory test-only) this becomes a test/dev-only path. **W3b needs a design pass
before execution; sequencing under review (see R1 decision log).**

| Fold-in | Into wave | Status |
|---|---|---|
| Erasure ledger records COMPLETION time + restore-inside-window resurrection test | W6b | PENDING |
| git DSR receipts: real `holders_hit` reconciled against data-map | W6b/W6 | PENDING |
| SQL-interpolation shapes killed while touching crates (rls predicate_sql, block_tree, TRUNCATE classifier) | W6 | PENDING (block_tree/TRUNCATE are myelin-knowledge, out of spine scope per W6 grounding) |
| KMS ~90-site classification by independent verifier; boot fails LOUD | W5 | **DONE** (`c271932`) — 121 src sites audited: 1 prod root (edge main.rs, fail-loud), 110 cfg(test), 7 drill fns gated, 0 injection re-points |
| Region-scope sweep (identity PG `scope.region()`, fr-par partitions parameterized) | W7 | PENDING |

**W5 DONE (2026-07-15, `c271932`): scanner 12→11.** Builder→orchestrator-gate→independent adversarial
verifier (CONFIRMED-SOUND)→commit. Verifier defect **D1 fixed pre-commit** (rotate_kek KEK+DEK persist
was non-transactional → one PG tx + regression test). **W5 residual hardening follow-ups:**
(1) **D2** — concurrent first-mint of the same DekId can hand a ref out of the fast path while the
first minter's persist is in flight; a DB failure/crash in that window loses the key LOUDLY
(MEDIUM; fix = publish the in-memory entry only after persist commits). (2) `ensure_kek`/`destroy_*`
panic-on-durability-failure is task-down, not process-down — converting the infallible signatures to
`Result` is a ~50+180 call-site ripple wave. (3) `rotate_kek` fault-injection integration test.
(4) `MYELIN_CELL_ID` default "cell-dev" — confirm against the multi-cell boot spec at W6d.

**W6 grounding DONE (`180be21`, doc 15):** execution order W6a→W6b→W6c-events→W6c-cp→W6d, serialized
on the shared baseline file. **W3b design DONE (`6ce6702`, doc 16):** DedupLedger trait-seam pattern,
6 steps W3b.1–.6; identity is NOT a DAG sink (only myelin-events is); one real BUS-2 gap (identity
durable-tuple emit) fixed at W3b.3.

**W6a DONE (2026-07-15, `a621a07`): scanner 11→9.** Pseudonym stores durable-by-default
(pseudonym_map FORCE-RLS + NON-erasable erasure ledger, migrations 0020–0022). Verifier HOLD→fixed→
sound: the ledger's only write silently swallowed durable failure = a real resurrection path
(unrecorded erasure + pre-erasure PIT restore + replay miss); now fail-static panic mirroring
destroy_dek. Residuals: PrincipalId unconstrained-string PII grammar (platform-wide);
composition root must apply `pseudonym_durable_migrations()` (only integration tests do today);
region persisted from provider pin (W7 sweep owns).

**W3b.1 DONE (2026-07-15, merge `3583f59`): scanner-neutral.** OutboxStore role-struct +
`DurableOutboxBacking` trait (commit_staged + reads + composite drain_once); ~300 reference sites
unchanged; memory arm verifier-probed byte-equivalent. Staged debt for W3b.2 (in-line admissions):
surface the durable drain error; durable GC verb; duplicate-event_id commit-semantics CDC parity.

**W6b DONE (2026-07-15, `d7e418b`): scanner 9→7.** PostPit + restore ErasureLedgers durable
(migrations 0051/0052); §7.6 completion-offset window gate (R1 fold-in) + predicate_sql
parameterization fold-in. Verifier CONFIRMED-SOUND; two probe-confirmed latent gaps CLOSED
pre-commit: offset-0 `record_erased` fail-open (now test-support-gated) + `run_with_reerase`
trusted-not-structural post-PIT coverage (now a cross-ledger coverage assert + regression test).
**CostLedger NOT flipped (honest STOP):** its durable backing (0050, FORCE-RLS) is built + proven
live but unwired — flipping breaks production `myelin-flow::BudgetGate::new` → CI metering; needs a
BudgetGate durable redesign (queued as its own wave, "W6b2"). `reserve_settle.rs:283` stays,
SUPPLEMENTED. Residuals: Box::leak unit-label rebuild (bounded, unwired); cost-op infra faults
panic (no error variant); region from provider pin (W7).

**W6c-events DONE (2026-07-15, `ac4e923`): scanner 7→6.** BusErasureLedger durable via the
DAG-sink trait seam (`DurableBusErasure` in events, `DurableBusErasureBacking` + `bus_erasure_ledger`
0053 in storage, wired at EventsRuntime). Verifier HOLD→fixed→sound: the no-conflict INSERT arm
stored key_refs verbatim (unsorted+dup ⇒ memory-arm divergence + receipt double-count) — now
normalized in Rust pre-bind + adversarial regression input. Residuals: **boot-wiring wave must
apply migrations 0051–0053** (dedup's table IS boot-applied, these are not — asymmetry flagged);
key refs embed subject discriminators in a non-erasable table (opaque-id grammar concern, W6a).

**W6c-cp DONE (2026-07-15, `5dd13c1`): scanner 6→5.** CellResolverRegistry = boot-time projection
of the existing `cell.endpoint` authority (no net-new table); Memory arm test-support-gated.
Verifier HOLD→fixed→sound: the projection module shipped `integration`-gated with a provably false
justification — the correct-but-latent shape this track kills — now compiled unconditionally.
Residuals: W6d boot root must assert non-empty projection in multi-cell mode; real transport
factory = named transport floor; boot wiring lands with W6d/W3b.4.

**P-S12 minter floor — blast radius note (W3b.3 discovery):** the default `MonotonicMinter` resets
per store ⇒ two durable TupleStores mint colliding `event_id`s and `co_commit_in_tx`'s
`ON CONFLICT (event_id) DO NOTHING` silently DROPS the later event. Pre-existing named floor (the
production ULID source), but the W3b track makes the shared PG outbox the real emit path — the
ULID/unique-minter fix should land before or with W3b.4 (composition roots).

**W3b.3 DONE (2026-07-15, merge `50387a5`): BUS-2 exact for identity.** Tuple deltas +
`co_commit_in_tx` in ONE `with_tenant_tx` (both-or-neither, probe-proven under a forced post-delta
unique violation + kill-9 0-ghost/0-lost); event shape byte-identical; durable ctors drop the
OutboxStore param. **NAMED CONDITION → W3b.4: composition roots MUST wire a unique/ULID minter,
never the default `MonotonicMinter`** (collision → `ON CONFLICT DO NOTHING` silently drops events;
newly reachable via the shared PG outbox, probe-proven). Principal/revocation backings emit no
events — no analogous re-point exists.

**W3b.4 DONE (2026-07-15, merge `8745a50`): composition roots durable; in-proc floor test-only.**
Six service mains require DATABASE_URL (exit-1 pre-config — DevDefaults would silently supply the
dev DSN otherwise), migrate foundation, inject `OutboxStore::durable`; edge injects the new
`UlidMinter` (P-S12 stand-in — satisfies the W3b.3 condition; `TupleStore::with_pg` default also
re-pointed off the forbidden MonotonicMinter, verifier latent-gap). Verifier CONFIRMED-SOUND.
**MUST-FIX BEFORE the first production consumer / NATS wiring (probe-proven, documented in-code at
`PgRelay::drain_once_dead_letter`): the shared-table drain claim is unscoped** — one service's
relay can claim another's rows, publish to its process-local bus, and permanently stamp
published_at (loses nothing today: zero production consumers; git reconciler reads published rows).
Honest STOP: AppSpec::minimal + agent-service/ci spec builders construct the memory floor
EXPLICITLY (grep "W3b.6 debt"); ci-controlplane + ci-dispatch binaries boot it today (byte-identical
to pre-change). Residuals: migrate-as-owner/serve-as-app role split unaddressed platform-wide (the
app role cannot run the migrator — smokes/integration need the admin DSN); shared-backlog drain
coupling in dev; edge reconcile reads the full table (per-aggregate verb = future work).

**W6d DONE (2026-07-15, `241cab1`): scanner → 3 in-mem.** Registry whole-surface + MisrouteAudit
flipped; migrations 0035–0039 incl. the repo_placement residency trigger. Verifier findings CLOSED
pre-commit: tenant-delete drift vector (FK ON DELETE RESTRICT, probe-proven refused) + wrong-region
re-boot silent divergence (region-claim read-back assert at boot + adversarial test leg). Ledgered:
routing-path infra-fault panics — the Result-conversion wave MUST land before any serving binary
wires route(); placement_by_slug dup-slug (no UNIQUE); boot wiring must apply 0030–0039.

**W6b2 DONE (2026-07-15, `364d5df` merge `a7f6ba9`): scanner → 2 in-mem.** CostLedger role-struct +
BudgetGate injection; billing invariants proven on BOTH arms (verifier probe: 20 concurrent
double-settle rounds → 0 double charges); Box::leak gone; wallet stays per-run (P-ST-19).
**Remaining baseline: OutboxStore (W3b.5/.6) + FsBlobStore (W7.3).**

**W3b.2 DONE (2026-07-15, merge `20fe480`): scanner-neutral.** `PgOutboxBacking` over the frozen
outbox table; commit_staged_atomic (dup-event_id rejects whole commit = memory parity; one seq
discipline with co_commit_in_tx); drain_once_dead_letter (claim locks held across publishes; per-row
attempts/dead-letter; failed publish no longer aborts the pass); DrainReport.drain_errors surfaces
backing failure (W3b.1 debt resolved). Verifier CONFIRMED-SOUND (concurrent-drain no-double-publish,
crash-window 0-ghost/0-lost, EB-03 32-committer gap-free — all probed live); should-fix applied:
committed_rows/dead_rows ORDER BY (aggregate,seq) — event_id mint order provably ≠ commit order.
Residuals: no durable GC verb; NATS-bridge nested block_in_place unproven until W3b.4.

R1 exit: scanner `no-in-memory-durable-store` baseline **0**; unit tests DB-free; integration green; kill-9 drills.

**R1 EXIT REACHED (2026-07-15, merge `d2b8740`).** `no-in-memory-durable-store` = **0** (12→0 in one
session); the sole baseline survivor is the out-of-scope attestation structural floor. Closing waves:
**W3b.5+.6** (`2beaa92`) — THE OUTBOX FLIP (SI-007 closed; ci-controlplane + ci-dispatch binaries went
durable; kill-9 emit drill 0-lost/0-ghost; verifier probe proved the ratchet fires on un-gating) and
**W7.3** (`58f2def`, merge `d2b8740`) — FsBlobStore → S3 byte backing (knowledge/chat production
defaults re-pointed to injection; unwired fs floors gated with it; Backend::Real fail-loud). All 13
waves this session passed builder → orchestrator gate → independent adversarial verifier → commit;
~12 real defects fixed pre-commit. Unit tests DB-free (889 suites); integration green on the live
stack; kill-9 drills green.

**Carried out of R1 (the named follow-ups, in priority order):**
1. **Shared-outbox drain scoping** (W3b.4 MUST-FIX, in-code at `PgRelay::drain_once_dead_letter`) —
   BLOCKS the first production consumer / NATS wiring.
2. **Boot-migrations aggregate (W7.2)** — provider `all_durable_migrations()` at every main;
   includes the KNOWN EDGE DEFECT (identity tables never migrated at edge boot, doc 18 Part 5) +
   migrate-as-owner/serve-as-app role split.
3. **Region-scope sweep (W7.1)** — `provider.with_tenant_tx` hardcodes config.region (residency-bug
   shape, doc 18 Part 4).
4. **Result-conversion waves** — KMS/Registry/MisrouteAudit/cost infra-fault panics (routing-path
   reads must convert BEFORE any serving binary wires route()).
5. **W7.4 scanner blind-spot widening + W7.5 CT-004b CI slice** (doc 18 Parts 2–3) — the widened
   coverage obligations; the no-in-memory gate scope is still SPINE-only.
6. D2 KMS mint-visibility race; durable outbox GC verb; UlidMinter at remaining emit roots as they
   appear; PrincipalId opaque-id grammar; dedup-vs-0051-0053 boot-apply asymmetry (folds into #2).

## R2 — authz completion

| # | Item | Status | Commit(s) |
|---|---|---|---|
| R2.1 | Object-level authz at the edge, platform-wide (extends R0.3 seam; git_edge template first) | PENDING | — |
| R2.1a | **Wire R0.2/R0.3 LIVE** (carried from R0 verifier): production `main.rs` must (a) inject a real grant-backed `RepoAuthorizer` (not the `AllowAllRepos` default) and (b) `register_git_wire` in the production gateway. Until both, the R0.2 branch-protection gate and R0.3 per-repo authz are correct-but-latent. | **DONE** | `239547c` (merge; v2 `453d6af`, superseded reverted v1 `2138fd2`) |
| R2.2 | Identity `check()` fully-qualified object; EventMatcher + SSE scope likewise | **DONE** | `25bb2b7` (merge; `59b078c`) |
| R2.3 | Fail-static authz cache full-key comparison | **DONE** | `2154e38` (merge; `d3ebd3b`+`ff334e8`) |
| R2.4 | MCP HITL server-side verdict; batch partial-approval by approval-id | **DONE** | `d248644` (merge; `d6b9d1e`+`5f60037`) |
| R2.4-fu | Wire GovernedRouter + durable HitlVerdictStore into MCP prod main (currently GOVERNANCE_NOT_WIRED) | PENDING (tracked; MCP-serve composition root) | — |
| R2.4-h | MCP stdio lifecycle hardening: bounded frames, per-call wall clock, EOF/error run-token teardown | **DONE** | `4aa6aaf` |
| R2.1 | Object-level authz at the edge, platform-wide (extends R0.3 seam; git_edge template first) | **DONE** | `083831d` (merge; `fc06f54`) |
| R2.5 | Real OIDC login at edge; dev-login structurally dead in prod | **DONE** | `fb7fd1e` (merge; `a9e4e57`) |
| R2.6 | AllowAll removed from main.rs + lint | **DONE** | `3b1fda0` (merge; `e149dee`) + `75223a0` (followup: object-seam default fail-closed → scanner TRUE ZERO) |
| R2.7 | Search vector-path ACL parity | **DONE** | `f31e310` (merge; `6c56c42`) |

R2 exit: red-team campaign (subagent per subsystem: edge/wire/MCP/SSE/search reach-around) all-denied; AllowAll gone.

## **R2 COMPLETE — EXITED (2026-07-16, merge `e698b2d`).**

All 10 items merged, each builder → orchestrator gate → independent adversarial verifier (Fable) → merge,
every verifier CONFIRMED-SOUND. **Exit red-team ran (5 adversaries):** MCP / SSE / search PASS (latent-only);
edge + wire converged on ONE HIGH cross-subsystem blocker — the git protected-branch DIRECT-PUSH path was not
a real control (a plain writer landed code on protected `main` via self-certifiable CI + a gate honoring only
required_contexts + a wire path never checking ProtectedPush). **Blocker FIXED + verified + merged** (`e698b2d`):
report_checks refuses non-Service principals; the wire requires `RepoPermission::ProtectedPush` (admin) for a
protected-ref direct push; `evaluate_protected_ref_push` runs the FULL ruleset (approvals/CODEOWNERS/
conversations); multi-ref pushes abort atomically. Both red-team exploits are now DENIED as regression tests on
main (incl. a real `git push` refused under runsc); force/delete floor (R0.2) intact; legit flows work.
**Exit criteria met: every reach-around all-denied; AllowAll gone (no-permissive-authorizer-in-prod scanner at
TRUE ZERO).**

**Carried out of R2 (tracked, non-blocking):**
1. **#12 — MCP GovernedRouter not wired in prod** (`new_catalogue_only` → GOVERNANCE_NOT_WIRED; MCP tool
   EXECUTION is not live — fail-closed, not a vuln). Before wiring: inject `HitlVerdictStore::with_pg`; fix the
   latent gate findings (F1 approved-gate-not-single-use → per-effect idem ledger; run_id consult conjunct;
   durable-panic-not-caught-by-stdio-handler). Product-surface piece (R3-ish). The stdio transport itself is
   now bounded, reads a fresh clock per governed call, and tears down the run token on EOF/error (`4aa6aaf`);
   that does not make tool execution live.
2. **#13-residual — CI-producer relation:** the report_checks Service-kind floor is coarse (any in-tenant
   Service principal, not the specific ci_producer for that repo/run). Full `repo.report_checks`/`ci_producer`
   fragment relation is the R2+ follow-on; the realistic Human-developer vector is fully closed.
3. **#14 — latent residuals (non-prod-reachable):** SSE tenant-id charset injectivity (defense-in-depth before
   any per-resource stream on a shared stream name) + per-frame object filter (lands with the first per-resource
   publisher); search legacy 3-field sealed-segment deny-arm fail-open (close when at-rest seal-every-segment
   wiring SRCH-P06/P15 lands).

### R2 execution log (2026-07-15/16, session 2)

Same process as R0/R1: builder → orchestrator gate → independent adversarial verifier → commit/merge.
Grounding surveys done for all of R2.1a, R2.2, R2.3, R2.4, R2.7 before dispatch.

- **W7.2 boot-migrations aggregate DONE (`ef7400a`)** — pulled forward from the R1 carry list
  (#2) as a hard PREREQ for R2.1a: identity tuple/principal tables must exist at edge boot. Adds
  `all_durable_migrations()` in myelin-storage (built structurally FROM `durable_migration_groups()`,
  anti-drift), applied at all 9 durable service mains after `migrate_foundation()`. Closes the doc-18
  Part 5 LIVE DEFECT (identity 0010–0019 never migrated at edge — first principal write failed on a
  fresh DB) + the W6c dedup-vs-0051–0053 asymmetry. Fresh-schema red→green proof + `PrincipalStore::with_pg`
  first-write proof on live PG. Pre-existing `mr023_serve` outbox-pollution integration flake noted, out of scope.
- **R2.3 + R2.3b DONE (`2154e38`)** — core: FailStatic cache keyed by the real key, not a 64-bit
  DefaultHasher digest (kills cross-principal cached-ALLOW replay on hash collision). **TWO independent
  adversarial verifiers → CONFIRMED-SOUND**, aliasing test proven non-vacuous (fails on old code); both
  flagged the same residual → **R2.3b**: the two string-key builders (git live_check, refs resolve)
  flattened unconstrained OIDC-`sub`/SCIM-sourced segments with a `format!` delimiter join → distinct
  authz questions could serialize byte-identical. New `encode_authz_key()` length-prefixes each segment
  (`{len}:{seg}`) → injective; regression coverage incl. the background-refresh path; doc-rot fixed.
  Workspace 8080 passed.
- **R2.7 IN VERIFY (`6c56c42`, worktree)** — `AclFilter::admits(doc_id, acl_object)` mirrors the lexical
  `acl_clause` two-field match; `VectorRecord`/`knn_filtered`/`upsert_stamped` thread `acl_object`;
  persistence appends it with a legacy-missing-field fail-closed fallback; bonus: `projection_feeder` was
  silently `acl_object`-only, fixed. Red→green on the deny-set-both-directions leak tests. Adversarial
  verifier running (focus: NotIds deny arm + HNSW recall path).
- **R2.1a DONE (`239547c`)** — THE FLIP: git smart-HTTP clone/fetch/push now exists in prod, gated by the
  real Identity check. `CheckBackedRepoAuthorizer` over `StoreBackedCheck` via the doctrinal `GitCheckGate`
  (Read→pull, Write→push, fail-closed, revocation-consult byte-identical to `StoreBackedCheck::check`);
  `register_git_wire` mounted; `TupleRepoBootstrap` writes the creator→admin grant into the SAME store the
  checker reads (deny-by-default would otherwise strand every fresh repo). **Two parallel builds happened**
  (dedup): the shared-store **v2** (`453d6af`) superseded a two-store **v1** (`2138fd2`, reverted at
  `0a2b16d`) — v2 also adds a tenant-pin defence-in-depth + an R0.2 force-push-through-wire test. **Both
  independently adversarial-verified CONFIRMED-SOUND** (no false-ALLOW cross-user wire read/write; per-repo
  authz runs before any byte in all 3 handlers; cross-tenant blocked upstream by resolve_scope IDOR; R0.2
  fires through the wire; cross-store reads live-PG per check). Real-git gVisor oracle legs executed.
  **Verifier LOW carry-forwards → R2.1a-followup ledger row:** (#7) grant-first has no compensation on
  create-fail → orphan admin + slug-reuse = cross-user (narrow); (#1) durable-PG create-then-clone
  regression leg; (#2) `object_id_of` collapses namespaced slugs → **routed to R2.2** (not wire-reachable).
- **R2.1a-followup DONE (`90ad104`)** — #7 create_repo_as now compensates (exact-inverse `Remove` on
  create-fail so no orphan admin grant survives for slug-reuse cross-user; double-fail surfaces the orphan
  loudly; residual = crash between grant-commit and compensation needs a reconciler); #1 durable-PG
  create-then-clone regression test (two separate durable stores over one `rebac_tuple` table, grant visible
  to the wire check) passes live. Self-reviewed (LOW hardening); merged.
- **R2.2 DONE (`25bb2b7`)** — verifier CONFIRMED-SOUND. `myelin_refs::object_key` = the ONE canonical
  type-qualified key (`type:id`; bare refs are fixed points, URNs normalize, `#sub` roots, malformed→None
  fail-closed). `check_engine::object_id_of` delegates; `snapshot_view` canonicalizes stored keys on read
  (writers store verbatim → zero migration). Killed the cross-type leak (`no_cross_type_check` was ALLOW);
  fixed the R2.1a #2 namespaced-slug carry-forward; EventMatcher now gates type+tenant; SSE `sse_route_scoped`
  + registration-time panic for object-addressed routes. Live-PG proof. Residual (verifier obs, not a defect):
  bare-ref `#` stripping is a latent aliasing surface only if a future subsystem admits `#` in an id reaching
  both write+check — defended today by slug/id allowlists.
- **R2.4 DONE (`d248644`, +R2.4b `5f60037`)** — verifier CONFIRMED-SOUND. Caller-boolean dead; opaque
  OsRng gate-id + durable `waiting` verdict row; re-drive presents gateId, GovernedRouter admits only on a
  stored Approved verdict for the exact (tool+args) effect by a distinct **HUMAN** principal (R2.4b typed
  `MachineApproverRefused`, kind threaded — closes the two-machine-collusion MEDIUM). Step-6 + batch key
  per-effect, never bare tool name. New durable `hitl_gate_durable` store; **found+fixed a real boot-migration
  gap** — `agent_hitl_gate` was declared but never migrated (outside W7.2's 0010–0053 span) → now migration
  `0054` in the aggregate with a model-vs-boot DDL parity test. Fail-closed on store errors. **KNOWN LATENT
  (R2.4-fu, tracked):** the full GovernedRouter is not wired into the MCP prod main (`new_catalogue_only` →
  `GOVERNANCE_NOT_WIRED`), so MCP tool EXECUTION is not live (fail-closed, not a vuln); whoever wires the
  composition root MUST inject `HitlVerdictStore::with_pg`. Release must not claim MCP execution live.

- **R2.5 DONE (`fb7fd1e`)** — real OidcVerifier (already tested in oidc.rs) routed via `production_with_oidc`;
  `MyelinConfig` gains `Option<OidcSettings>` (issuer/audience + optional bootstrap JWKS and a required
  production `jwks_uri`); the follow-on now uses bounded, certificate-verified refresh with a
  serialized/rate-limited last-good cache;
  opt-in (absent→refuse-not-mock/boot-ok, partial→fail-loud); NO fail-open. Part B: frontend `loginDev`
  build-time dead in prod (verified the dev token is ABSENT from the `.output` deployable). Self-reviewed
  (fail-open surface clean; crypto pre-audited). SAML/SCIM/passkey/SSH stay refuse-not-mock.
- **R2.1 DONE (`083831d`)** — verifier CONFIRMED-SOUND. Closed the LIVE JSON-API object-authz bypass: the
  `repo_authz` field git_durable.rs carried was never called, so a git action-grant reached ANY repo's
  PR/blob/branch-protection. `RepoAuthorizer` extended to `RepoPermission {Pull,Push,ProtectedPush,
  ApproveUntrustedCi}`; `RepoObjectGuard` wraps all 16 object routes with the correct rung (merge/branch-
  protection = ProtectedPush/admin — a push grant is denied); `DRepoList` = leak-free `list_objects`
  prefilter. git_edge.rs confirmed dead. **Verifier surfaced a pre-existing residual (task #13, NOT an R2.1
  regression): report_checks is Push-gated → a writer can forge CI greens that feed the wire protected-ref
  push gate → bypass branch protection. Fix = a CI-producer relation. Prime R2-exit red-team target.**

- **R2.6 DONE (`3b1fda0` + followup `75223a0`)** — the action-level `AllowAll` is GONE from prod:
  `main.rs` wires `AuthenticatedActionPolicy` (deny-by-default over a 20-verb `MOUNTED_EDGE_ACTIONS`
  allowlist; denies unknown actions + empty principals; anti-drift test iterates `http_catalogue()`).
  `AllowAll` gated `#[cfg(test/test-support)]` (zero test-file edits — features already set). New
  `no-permissive-authorizer-in-prod` scanner (construction-shaped, edge-scoped, red-drill-confirmed).
  **Followup fail-closed the object-seam:** `DurableGitBackend::rooted` now defaults `DenyAllRepos`
  (was `AllowAllRepos`, always overridden by main.rs); the sole `AllowAllRepos` construction moved into
  the test-support `rooted_inmem_for_test` → the scanner is a **TRUE ZERO** (baseline 2→1, only the CI
  runner-attestation structural floor remains). Self-reviewed; workspace + lints ratchet green.

**ALL 10 R2 ITEMS MERGED** (R2.1, R2.1a, R2.1a-fu, R2.2, R2.3+b, R2.4+b, R2.5, R2.6+fu, R2.7, W7.2) — each
builder → orchestrator gate → independent adversarial verifier (Fable) → merge, every verifier
CONFIRMED-SOUND.

**R2 EXIT — red-team campaign RAN (5 adversaries, Fable×3 + Opus×2).** Results:
- **MCP — PASS** (no approval bypass; self-approval/caller-boolean/gate-forgery/per-effect-collision/store-fail-open
  all DENIED). MCP execution confirmed LATENT (GOVERNANCE_NOT_WIRED). Latent findings → #12: F1 (approved gate
  not single-use — re-applies the exact approved effect N times; add per-effect idem ledger before wiring), run_id
  consult conjunct, durable-panic-not-caught-by-stdio-handler.
- **SSE — PASS** (no cross/intra-tenant leak on the live surface; scope token-derived, registration-panic fires
  at build time, bounded-id validated). Latent → #14: tenant-id charset injectivity; per-frame filter deferred.
- **Search — PASS** (no prod-reachable leak, both directions, every serving path). Latent → #14: legacy 3-field
  sealed-segment deny-arm fail-open (not reachable — no serving path opens sealed segments today).
- **Edge + Wire — ONE HIGH RELEASE-BLOCKER, converged (task #13):** a plain in-tenant **writer** lands arbitrary
  code on protected `main` over the wire, defeating required checks + reviews + codeowners + the admin
  requirement. Three compounding defects: (1) self-certifiable CI (`git.checks.report` is an ordinary writer
  capability, `report_checks` copies greens verbatim); (2) `evaluate_protected_ref_push` honors only
  required_contexts, not the full ruleset (approvals/codeowners/conversations); (3) the direct-push wire path
  gates only `Write`, never `ProtectedPush`. Proven with 2 runnable exploits. Force/delete-on-protected floor
  (R0.2) HOLDS. **Everything else on edge+wire DENIED** (no-grant clone/push, cross-tenant, list leak, 0-leak
  404, object-grammar reach-around, create abuse — all secure).

**R2 EXIT STATUS: BLOCKED on #13.** Fix IN PROGRESS (Fable, worktree): (a) `git.checks.report` → a CI-producer
capability (not `writer`); (b) direct push to a protected ref requires `ProtectedPush`; (c) full-ruleset
evaluation on the direct-push gate. Acceptance = both red-team exploits flip to DENIED + legit flows
(admin/bypass push, CI-producer report, PR merge) still work. On merge: re-run a focused edge+wire red-team to
confirm the cluster is closed → **then R2 exits** (all-denied + AllowAll gone). Non-blocking latent residuals
tracked in #14; MCP wiring + its latent findings in #12.

**Sequencing decisions:** R2.2 (object-qualification of `check`/EventMatcher/SSE) must land AFTER R2.1a —
it changes tuple-store object keying while R2.1a writes the first production grant tuples; git's
type-prefixed refs (`repo:core`) survive today's reduction, so R2.1a builds on the current grammar and
R2.2 canonicalizes against the then-live wire tests. R2.4 (MCP HITL) deferred until after R2.1a lands to
avoid myelin-agent-service overlap; fix shape settled (persist HitlGate in the already-declared
`agent_hitl_gate` table, MCP re-drive presents a server-issued gate-id looked up server-side, step-6
gate + batch approval key per-effect not by bare tool name).

## R3 — Git/PR UX + first-run (OPENED 2026-07-16, design-first)

Entered at HEAD `6fd7e3a` (post-R2 exit). Design sketches precede all frontend code (VISION §3).
Design deliverable: `design-planning/09-r3-sketches/` — implementation-ready sketch pack (IA, flows,
full R-21 state sets) extending the frozen 08-design-system + 6c finalist-A (Instrument) direction;
orchestrator gates the pack against `design-planning/05-user-facing-surfaces/git.md` G-1..G-9 DoDs
before any build wave. Frontend is **SolidJS/SolidStart** (`frontend/apps/web`), not the React noted
in older design docs — the code wins.

| # | Item | Source | Status | Commit(s) | Proof |
|---|---|---|---|---|---|
| R3.0 | Design sketch pack (PR list, PR overview/context pane, PR diff, review/merge, repo browsing, first-run) | VISION §3 gate | DONE (2026-07-16) | (pack commit) | 5 surfaces × (sketch HTMLs + NOTES: routes, R-21 states, EXISTING/NEW data contract, keyboard/SR, component reuse); every surface gated in `design-planning/09-r3-sketches/_gate.md` — one rails violation found+fixed (04 ref-switcher outline), one cross-pack API conflict reconciled (canonical thread-store, object_key-keyed); all open questions decided at the gate |
| R3.1 | PR list + navigation front door (per-repo + cross-repo; edge `GET …/prs`) | ux-git crit 1 | DONE | `54bd8e0` | Durable PR title/body + leak-free per-repo (RepoObjectGuard Pull 0-leak 404) & cross-repo (visible_repos prefilter BEFORE bucket) list GETs; checks_summary rollup no N+1; bidirectional cursor; StatusPill→design-system; front door wired. Verifier HOLD→fixed (forged-cursor overflow ×3 + regression test; title 512B cap; merge bumps updated_at). Gates: cargo 81, web 16, DS 52, PW 21 |
| R3.2 | PR diff / files-changed (head_oid vs base_ref; side-by-side+unified; comment anchoring; G-7 keyboard/SR) | ux-git crit 2 | DONE (verify running) | `b015c8b` | Backend pr_diff (three-dot merge-base via libgit2; hunk-structured old_no/new_no; binary/LFS kind; per-file cap) + file_lines expand-context; edge GET …/prs/{n}/diff + …/file-lines/{oid} Pull-guarded 0-leak, restricted count-only, MR-014 paging. DiffViewer shared primitive→DS (split+unified, SR-as-text, roving j/k/F7/n/p/c/v grid); prs/[n]→prs/[n]/index.tsx + diff.tsx + PrHeader tabs; line-anchored composer→thread store; rebase-orphan honest-detach; W4 deep-link; commit/[oid] migrated onto DiffViewer; SSR hydration fix. Gates: cargo 100 ok, tsc, web 20, DS 60, PW 42. Diff-endpoint verifier HOLD→FIXED `bcde53a` (single-hunk file was uncapped → now bounded in line_cb, memory + wire, +regression test; all other attacks PASSED: authz 0-leak, cross-tenant, restricted count-only, file_lines no-500, three-dot, cursor overflow, line numbers). Floors: expand-context endpoint built but not wired to button (needs per-file blob oids; no dead affordance ships), auto-responsive deferred, restricted=0 by construction, mono (no word-emphasis/syntax per gate) |
| R3.3 | PR context pane + G-8 review/merge + G-9 checks + shared thread/comment store | ux-git crit 3, findings 5/9 | DONE — verifier HOLD, fix pending (R3-exit BLOCKER) | `3a99de0` (+ pending fix) | Canonical thread store (pr_threads.rs, object_key-keyed, view_for drops others' pending BY CONSTRUCTION, submit=one event); edge threads/reviews/commits/merge endpoints (read Pull / write Push, scanner clean); merge 409 carries fresh checks; backend-truth VERIFIED (R2 gate ingests changes_requested + required_approvals + conversation-resolution → honest blocked-reasons); AppShell contextPane 4th region + drawer; prs/[n].tsx rebuilt (checks degrade locally=finding5, check panel, discussion, commits-in-PR, batched review G-8, merge card+ConfirmDialog); Chip+PaneSection→DS. Carried my R3.4 verifier-fixes. Gates: cargo 100 suites/966 ok, tsc, DS 55, web 16, PW 31. Thread-store adversarial verifier running. Floors: N4 linked-refs resolver (honest-empty slots), N3 check→run refs (G-9 surface), visibility_label dropped, N7 live-SSE, git.review.submitted outbox=GIT-P16 |
| R3.4 | Repo browsing completeness (tree-at-path, nested blob, README, ref switcher, default_branch, 404 catch-all, error-state trio, binary fallback) | ux-git 4/6/8/10/12/13, firstrun 1 | DONE (verify running) | `e8200d8` | tree-at-path/nested-blob/refs/raw+download (guarded Pull 0-leak; is_safe_tree_path rejects traversal→clean Missing; bounded walk); gateway Seg::Rest catch-all; enriched RepoHomeVM (default_branch, README, latest_commit, counts); RefSwitcher; RepoErrorState trio; bidirectional commit pager; 404 catch-all + NotAvailable subsystems + honest rail. Built in worktree, merged w/ R3.1. Gates: cargo 81, tsc, web 16, DS 52, PW 26. Verifier PASS (no security bypass — path traversal/symlink escape contained via layered defense [no percent-decode + is_safe_tree_path + libgit2 object-graph walk], authz closed, README/raw XSS closed); 2 robustness defects fixed in-tree (non-commit-ref→500 now clean empty browse + regression test; protocol-relative README link rejected) — gate+commit folded into R3.3's (crate mid-edit) |
| R3.5 | First-run flow (login→tenant→repo→push→first CI as one path; onboarding empty states; real inbox; palette client-side nav) | ux-firstrun 2/3/5/6 | DONE | `513651d` (merge of `40c20f5`) | Login reworked (OIDC primary on --c-btn-primary-bg, dev seam gated on server flag, visible SSO-reason, 100dvh); unauth `GET /v1/auth/config`; repo.created/pushed wired onto the EXISTING SSE firehose at the gateway seam (no 2nd channel); ReposEmptyState onboarding (push-to-create + live waiting-for-push + dismissable checklist); inbox honesty (real empty state, no fake "2 unread"). Gates in worktree: cargo myelin-edge 86+12, lints clean, web 20, DS 52, PW 30. **FLOOR (names a real R2.5 gap): NO OIDC browser-start route exists — R2.5 landed only `POST /v1/auth/login` verification of an already-held id token; first-run login ships the honest "SSO unavailable" state. The authorization-code start (`/oidc/start → IdP 302`) is an unbuilt follow-on that blocks true self-serve login.** Merge reconciles AppShell (R3.5 inbox vs R3.3 pane) |
| R3.6 | A11y AA batch (palette focus ring, rail active=surface-hover, commit-link contrast, skeleton+aria-busy, inline-style hover, button-primary token, dvh) + fe-ds 7 findings | ux-a11y 1–7, fe-ds | DONE | `2be4342` | All 7 ux-a11y + 7 fe-ds findings fixed; Skeleton contributed to design-system (aria-busy + one polite region); +17 DS tests, +2 e2e; gates orchestrator-run: DS 49 pass, web 10 pass, tsc/lint clean, Playwright+axe 15 pass. Deferred (named): position.ts BOUNDED collision-hardening stands; Toast Undo accent-as-text flagged for a future pass |
| R3.7a | GT-004b PR review/merge UI (G-8: batched verdicts, merge action, gate_admitted stays authoritative) | ledger 11 follow-up | DONE (delivered by R3.3) | `3a99de0` | Batched review bar + verdict radiogroup (keyboard-reachable, submit=one event R-BATCH-1), merge card + ConfirmDialog (alertdialog, 409 re-verify, gate_admitted authoritative). Remaining: `git.review.submitted` outbox emit = named backend floor GIT-P16 (store produces the proven-idempotent batch; wire at the notif consumer path) — not UI-blocking, not an R3-exit blocker |
| R3.7b | Flow budget-reservation leak on retry-exhaustion (settle/refund; re-drive safe) | flow #1 HIGH | DONE | `6dc24ab` | Settle on both outcomes (failure bills zero → full refund), un-gated on fresh (idempotent re-drive reconciles crash windows), telemetry parity kept; +2 tests; verifier CONFIRMED-SOUND (reproduced pre-fix leak; 8 adversarial crash/concurrency attacks). Non-blocking follow-ups in commit msg: silent settle-error discard on failure path; latent P-ST-19 wallet-reseed credit; job.rs settle still fresh-gated |

**Backend endpoint gaps the R3 screens need** (from the 2026-07-16 frontend/API census — each becomes
part of its item's build wave): PR list GET (logic exists in `myelin-git/src/list_filter.rs`, unexposed);
PR head-vs-base diff; PR title/body in `PrVM`; PR discussion/comments (no store exists — scope decision
in R3.0); linked-refs-for-PR endpoint (myelin-refs crate exists, no edge surface); commits-in-PR;
tree-at-path + nested blob; branches/tags list + `default_branch` in RepoHomeVM; check→run refs in
`PrChecksVM`. PR-list MUST use the leak-free `list_objects` prefilter path (`repo_authz.rs`), not
post-filter.

**R3 carried follow-ups (non-blocking):** (a) blob/raw serving loads the full blob into memory before
the size cap and `raw_response` streams unbounded — inherent to libgit2 in-memory odb, pre-existing in
`read_file_at_ref`/`commit_detail`, NOT introduced by R3.4; a bounded-stream for very large blobs is
the named follow-on now that raw/download exists (both R3.4 verifiers). (b) R3.1: checks_summary `fail`
needs the per-commit check_status projection join; PR store PG home = GT-003b. (c) R3.7b flow follow-ups
(commit `6dc24ab` msg).

**R3-EXIT BLOCKER (FIXED `292fa3b`, re-verify running):** R3.3 thread-store verifier found the
review-batch write path enforced NO batch-author identity (H-1 verdict/identity forgery — a Push
holder force-submits another reviewer's draft as "A approved" with attacker text; H-2 draft
destruction). Read-side isolation, write=Push, IDOR, self-approval, agent-advisory, R-BATCH-1 all
HELD. Fixed: submit_review/discard_review take an actor and reject actor.display !=
batch.reviewer.display; add_pending_comment checks its author; edge threads ctx.principal; the
ownership check precedes submit's idempotency short-circuit; +regression test
(a_batch_is_not_submittable_discardable_or_appendable_by_a_non_author). Gate 100 suites ok.
**Independent re-verification CONFIRMED-SOUND** — H-1/H-2 closed, no bypass: pseudonym
(`{principal_id}@{tenant}.noreply`) is injective + tenant-scoped (no collision), ownership check
precedes idempotency (no leak oracle), identity never body-supplied, agent/service masquerade fails,
legitimate author + merge-gate feed intact. Lower-sev notes unchanged (key_stem non-injective floor;
double-submit TOCTOU is same-owner only).

R3 exit gate: the founder reviews and merges a real Myelin PR entirely inside Myelin (notification →
diff → merge), and the axe/Playwright a11y suite is green on the PR surfaces.

**R3 STATUS (2026-07-16): ENGINEERING COMPLETE + VERIFIED — founder acceptance is the R4 handoff.**
All 9 build items committed (R3.0 `88b626b`, R3.1 `54bd8e0`, R3.2 `b015c8b`+`bcde53a`, R3.3 `3a99de0`,
R3.4 `e8200d8`, R3.5 `513651d`, R3.6 `2be4342`, R3.7a in R3.3, R3.7b `6dc24ab`; blocker fix `292fa3b`).
Every security-load-bearing surface passed an independent adversarial verifier; every HOLD fixed +
regression-tested (R3.1 forged-cursor, R3.4 non-commit-ref-500 + README open-redirect, R3.3 review-batch
verdict-forgery, R3.2 single-hunk uncapped-diff). **Exit-gate a11y half MET:** the full Playwright+axe
suite is green (42 passed, 20 PR-surface/axe specs) — the notification→list→overview/context-pane→diff→
review→merge flow is exercised end-to-end in a real browser against the dev-edge contract. **Exit-gate
founder half = the handoff:** the founder reviewing+merging a real PR against the PRODUCTION edge is by
definition R4.1 (dogfood cutover — "the founder's daily push/pull/PR flow moves over"); it needs the
real myelin-edge binary + real repo data, which R4 stands up. R3 hands a verified, a11y-green PR product
to that first dogfood act. **Blocking real external use (tracked): the OIDC browser-start route (task
#11) — self-serve login ships the honest "SSO unavailable" state until it lands.**

## R4 — dogfood cutover (Tier D) — OPENED 2026-07-16 at HEAD `3ee0503`

Plan items (planning/08-release/01 §R4): R4.1 mirror Myelin-into-Myelin + founder daily flow · R4.2
CT-007 CI cutover (prereqs CT-004/CT-005 from ledger 12 still open) · R4.3 backup/restore drill on
real dogfood data · R4.4 finding-burndown loop in Myelin's own tracker.

**Census (scouted 2026-07-16, session open):** the R4.1 blocker cluster is AUTH BOOTSTRAP — nothing
can authenticate against the real `edge`: (1) `CellTokenAuthority::generate()` is EPHEMERAL per-boot
(main.rs ~122; the persisted load is the named P-527/MR-025 follow-on), no mint endpoint/CLI exists,
`POST /v1/auth/login` refuses by design, the dev-login seam is frontend-mock-only; (2) no
tenant/principal bootstrap surface anywhere; (3) the git wire is Bearer-only — vanilla `git push`
sends HTTP Basic; (4) no script boots the edge; `dev-stack.sh env` omits MYELIN_KMS_SEAL_KEY; (5)
frontend `MYELIN_EDGE_URL` defaults to the mock :8787; (6) CI inert (checks never run — merge needs
protection-without-required-checks or manual check-report); (7) wire push path needs `runsc` on PATH
(present on this host).

| Item | What | Status |
|---|---|---|
| R4.0 | Founder auth+bootstrap: durable KMS-sealed cell token root (P-527/MR-025), `edge bootstrap` operator subcommand (mint via DB-creds+seal-key trust boundary, NO mint HTTP endpoint), Basic→Bearer on the git wire only, `token_login_enabled` auth-config flag, web operator-token login, dogfood scripts+runbook | **DONE + VERIFIED** (backend `c6e6057` Fable-ACCEPT; web `c80a3e6`) |
| R4.1 | Cutover acceptance: mirror this repo into Myelin over the real wire; founder PR flow (push→PR→review→merge) against the production edge in a real browser | **DONE + PROVEN** (`82b8fe6` flow, `0325a22` F1/F3/F8/F9 fixes) — wire+API+browser all exercised on the real edge |
| R4.2 | CT-004 → CT-005 → CT-007 (CI backend, CI surfaces, GitHub-Actions cutover) per ledger 12 | **IN PROGRESS — CT-004's running root, operational reservation/settlement, claim/launch fences, exact-tenant runtime/fan-out, producer-authored PR head ordering, serialized run supersession, opt-in production runner boot, and durable CI→Git check projection are proven. CT-005a now serves repository-authorized durable run list/detail reads. Still required: live-log API/SSE, web, CLI/MCP, the live founder push→CI→surfaced-check pass, then CT-007.** |
| R4.3 | Backup/restore drill (repeating) on real dogfood data | **DONE + PASSING** (`scripts/backup-drill.sh`) |
| R4.4 | Finding-burndown in Myelin's own tracker (minimal issues subsystem) | **ENGINEERING COMPLETE (2026-07-19)** — atomic ReBAC bootstrap landed as an outbox/saga seam; `/v1/issues` mounted in the production edge main + CLI + web; **remaining: live founder dogfood pass (move the burndown out of this ledger)** |

R4 exit gate: 4 consecutive weeks where the founder never needed GitHub for daily work.

**R4.4 durable-store increment (2026-07-18; not the product-surface exit).** Issues now has a real
`PgIssueStore` whose create/list/view/close operations take scope only from a verified principal, run
through transaction-local `(tenant,region)` GUCs + FORCE RLS, bind every value, require an injected
non-permissive object-authorizer, bound keyset pages to 100, encrypt titles under the creator's
per-subject DEK, and make close durable + idempotent. The live PostgreSQL proof creates two tenants
through the runtime app role, shows cross-tenant view/close/list stay invisible even with a deliberately
permissive test authorizer, checks ciphertext-at-rest, and verifies close does not double-bump version.
The formerly model-only migrations are now production-applicable: concurrent indexes run as standalone
steps, the invalid expression primary key is a generated-column key, the shared `consumer_dedup` DDL is
byte-identical to the foundation table, and the Issues main invokes the real provider migrator.

**Honest R4.4 stop (2026-07-18, since RESOLVED — see next paragraph):** no `/v1/issues` route or CLI command was registered yet. A create must co-commit (or
compensate with a proven transaction seam) the new issue row and its `issue#parent_project` ReBAC tuple;
Identity currently owns that tuple transaction and exposes no connection-bound atomic bootstrap. Mounting
the route now would create either an orphan row or an authorization bypass, so the surface stays closed.
The platform-wide migrate-as-owner/serve-as-app gap now has a shared guarded bootstrap and is live in
Issues + Edge: production boot requires distinct credentials, performs DDL through the migration-only
pool, re-validates the NOBYPASSRLS runtime role, closes the privileged pool, and erases its DSN before
constructing stores or listeners. The remaining production roots still need that rollout.

**R4.4 surface increment (2026-07-19, recorded 2026-07-20 by code-wins reconciliation — the atomic-ReBAC
blocker above is CLOSED).** The bootstrap seam landed NOT as a connection-bound Identity tx but as an
honest outbox/saga: one Issues transaction inserts an INVISIBLE issue row + an `issue_authz_binding`
intent + an `issue.issue.authorization_requested` event (`pg_issue_store.rs`); an async reconciler
(`spawn_issue_authorization_reconciler`, wired in the production edge main to the real Identity
`TupleStore::write_tuples` via `issue_authz.rs`) idempotently writes the `issue#parent_project` tuple and
only then activates visibility — no orphan-row and no authz-bypass window (invisible until the tuple
exists). `/v1/issues` create/list/view/close are registered unconditionally in the production `run_edge`
(`issues_http.rs`, `main.rs`), the CLI ships `myelin issues list/create/view/close` (`dispatch.rs`), and
the web app has a founder issues surface (`routes/(app)/issues/`). Proofs at every layer:
`integration_issue_routes_pg.rs`, `integration_issue_authorization_reconciler.rs`, CLI grammar tests,
`tests/e2e/issues.spec.ts`. Remaining for R4.4 EXIT: a live founder dogfood pass — create the open
burndown items in Myelin's own tracker and stop hosting the burndown in this ledger.

**Production-hardening increment (2026-07-18; foundations are not activation).** The shared PostgreSQL
outbox now has a capability-minimal JetStream publisher adapter and an advisory-lock-elected drain pass.
The live NATS proof creates zero consumers; the live PostgreSQL proof shows one publisher at a time,
per-aggregate order, standby behavior, and broker outage rollback without consuming the permanent-failure
budget. Publisher-only service roots no longer claim shared rows locally. This does **not** make the relay
production-live: malformed-row quarantine, authoritative row/envelope validation, taxonomy/aggregate
reconciliation, TLS/JWT/ACL wiring, validation-only stream binding, a real long-running health/shutdown
lifecycle, and removal of CI Dispatch's private embedded relay are still blocking gates. In particular,
the current adapter must not be activated while it can provision a stream or route invalid envelopes to a
fallback subject.

The CI track now owns a strict versioned executable DAG contract in `myelin-ci-controlplane`: canonical
tenant-keyed CAS loading requires exact snapshot refs plus durable repo/commit provenance, enforces bounded
commands/images/matrices, rejects unknown authority fields and legacy/unversioned snapshots, and preserves
the DAG instead of silently flattening it. This is also **not execution activation**. Dispatch still has to
produce the versioned plan and provenance, and durable workflow ownership, DAG-wave execution, runtime
token/meter authority, per-check verdicts, authoritative attempts, and a tenant-scoped durable starter are
still required before a real job can start.

**R4.1 dogfood findings (2026-07-16, drove the REAL edge binary end-to-end — the first act no test covered).**
Boot + bootstrap + repo-create all worked first try (edge up on :8080, `token_login_enabled:true` served,
`edge bootstrap --tenant myelin --principal founder` minted a 326-char token, `whoami` 200 / no-token 401,
`POST /v1/git/repos` 201 durable). Then the real `git push` surfaced what the green oracle tests could not:
- **F1 (BLOCKER, auth):** the git-wire 401 carries NO `WWW-Authenticate: Basic` header, so a real `git
  push/clone` using a credential helper/manager/interactive prompt never sends its Basic creds — it fails
  auth after the unauthenticated probe. The oracle test passed only because it injects the header via
  `http.extraHeader` (preemptive on every request), bypassing git's challenge-response handshake entirely.
  Fix: emit `WWW-Authenticate: Basic realm="myelin"` on git-wire-route 401s ONLY (never the JSON API — a
  browser Basic popup there would wreck the web login UX). `curl -u` worked, proving the decode path is fine.
- **F2 (BUG):** `GET info/refs?service=git-upload-pack` on an EMPTY repo → HTTP 500 (receive-pack advertise
  is 200). Empty-repo upload-pack advertise must return an empty ref list (200), not crash.
- **F5 (ENV/runbook BLOCKER):** with preemptive Bearer, auth passes and the pack uploads, then receive-pack
  fails at `index-pack`: the sandboxed quarantine needs a staged gVisor git-rootfs at
  `~/.local/share/gvisor-assets/git-rootfs` (`MYELIN_GVISOR_GIT_ROOTFS`), which `dogfood.sh` never
  provisions. A base busybox rootfs + host `/usr/bin/git` + its libs ARE present, so it is stageable (the
  recipe is `crates/myelin-ci-sandbox/tests/git_wire_prod_exec_test.rs::stage_git_rootfs`). Fix: a staging
  script + `dogfood.sh` wiring `MYELIN_GVISOR_GIT_ROOTFS`.
- **F3 (papercut):** the repo list advertises `clone_url: ssh://git@myelin/...` but there is NO SSH server;
  the wire is HTTP-only. Misleading for dogfood — advertise the HTTP wire URL (or omit until SSH lands).
- **F4 (stale doc):** `dogfood.sh web` help still says the operator-token web login is "the SEPARATE
  frontend deliverable ... Until then use the CLI" — it landed (`c80a3e6`).
Continuing the drive PAST the push blockers surfaced the rest of the flow — and it WORKS end-to-end:
- **F5 FIXED** by `scripts/stage-git-rootfs.sh` (bakes host `git` + libs + git-core helpers + mount points
  into `~/.local/share/gvisor-assets/git-rootfs` from the base busybox rootfs) — after staging, the push
  cleared the gVisor `index-pack` quarantine. **F2 was a symptom of F5** (upload-pack advertise shells the
  sandbox; empty-repo 500 was the absent rootfs) — now 200. Both closed.
- **F6 (working policy, workflow friction):** the pseudonymous-commit floor (`enforce_pseudonymous_commit`,
  grammar `<pseudonym>@<tenant>.noreply`) REJECTS real-email history → you cannot bulk-mirror an existing
  GitHub repo's commits. Correct behavior; the dogfood workflow is `git config user.email
  <handle>@<tenant>.noreply` going forward (the policy checks grammar+tenant only, NOT ownership — attribution
  is the authenticated `pusher_pseudonym`, so any well-formed handle passes; `founder@myelin.noreply` works).
  Aligns with the plan's "GitHub stays read-only for a quarter". Document in the runbook.
- **F7 (solo-operator):** the default ruleset is `required_approvals: 1` and self-approval is deliberately
  excluded (`pr_store.rs:231` filters `reviewer != author`), so a solo founder can't merge. Fix for dogfood:
  `POST .../branch-protection {rulesets:[{ref_pattern:"refs/heads/main",required_approvals:0}]}`. Document.
- **F8 (UX BLOCKER):** `POST .../prs` without `head_oid` yields a PR that FAILS to merge ("invalid merge head:
  head_oid  is not a commit"); the API does NOT auto-resolve `head_oid` from `head_ref`. Passing it explicitly
  works. Fix: resolve head_oid from head_ref at open (and/or at merge). FIX NEEDED.
- **F9 (BUG):** a fresh repo's default `HEAD` symref is never pointed at `refs/heads/main`, so a real
  `git clone` warns "remote HEAD refers to nonexistent ref, unable to checkout" (refs+content ARE all
  present — origin/main had both commits + the merged README). Fix: set HEAD→default branch on first push
  (or repo create). FIX NEEDED.

**R4.1 CORE ACCEPTANCE PROVEN (2026-07-16):** against the REAL edge binary — bootstrap→token, repo create,
`git push main` + feature branch (pseudonymous, over the wire, THROUGH the gVisor sandbox), open PR #2 (with
head_oid), review, set branch protection (0 approvals), `merge` → main advanced durably (`merged:true`), then
a real `git clone` back recovered BOTH commits + the merged Vision section. The CLI/API + wire half of the
R3 exit gate is met on production infra. Remaining: the browser half (Playwright vs the real edge — R3's
suite already proved it vs the dev-edge contract) and the ergonomics fixes F1/F3/F8/F9 + F5 dogfood.sh wiring.

Burndown: F2 closed (symptom of F5). F4, F5 fixed (`82b8fe6`). F6/F7 = documented workflow (not code). **F1/F3/F8/F9
FIXED + re-verified end-to-end on the real edge (`0325a22`):** F1 credential-helper `git push` now works (401 carries
`WWW-Authenticate: Basic realm="Myelin"` on wire routes only; absent on the JSON API — verified live); F3 clone_url is
the honest HTTP wire path; F8 a PR opened with only head_ref auto-resolves head_oid and merges; F9 a fresh `git clone`
checks out main with no dangling-HEAD warning. This IS the R4.4 finding-burndown loop (recorded here until Myelin's own
tracker stands up).

**R4.1 BROWSER HALF PROVEN (2026-07-16):** started the SolidStart app (`MYELIN_EDGE_URL=http://127.0.0.1:8080`)
against the REAL edge; `/login` rendered the operator-token card from the real `/v1/auth/config`
(`token_login_enabled:true`, SSO honestly unavailable); POSTing the real bootstrap token to the `loginWithToken`
server action returned 302 + set the httpOnly `myelin_session` cookie + redirected to `/git/repos`; the authenticated
repos page rendered the REAL repositories (`dogfood2`, `myelin/myelin`) through the SSR gateway. The one previously
untested seam (real-edge response shapes → SSR gateway → rendered HTML) is confirmed — no dev-edge/real-edge contract
drift. **R4.1 is complete: the founder's push/pull/PR flow works on production infra via git CLI, the JSON API, AND
the browser.** Remaining R4 phases: R4.2 (CT-004 CI reconcile — decomposed above), R4.3 (backup/restore drill on the
now-real dogfood data), R4.4 (stand up Myelin's own issue tracker to host the burndown that lived here).

**R4.3 DONE + PASSING (2026-07-16):** `scripts/backup-drill.sh run` captures the LIVE dogfood data (a `pg_dump -Fc`
of the `myelin` DB + a tar of the on-disk git object tier under MYELIN_GIT_ROOT), RESTORES both into a CLEAN target
(a fresh `myelin_restore_drill` DB + a clean git root), and VERIFIES byte-identity: PG row-count parity across
principal/rebac_tuple/cell_token_root/outbox/kms_sealed_root/revocation (6/6 identical), and per-repo `git fsck` +
ref→oid + HEAD-symref parity (the same destructive-restore property `myelin_git::backup` asserts, now proven on
real data — dogfood2.git main+feat at the merged oid HEAD→main, myelin.git main+add-vision, all fsck-clean). Ran
twice green (KEEP=1 then default), cleanup + idempotency proven (DROP DATABASE IF EXISTS). Schedulable via
systemd-timer/cron (recipe in the script tail); full off-site DR (S3 bucket sync + 3-2-1 off-host) is the R5.3
ops-runbook extension. Tier-0 "our backups actually restore" is now a repeatable drill, not an assumption.

**CT-004 grounding (scouted 2026-07-16): RECONCILE + HARDEN, not green-field.** The census line "runs
no CI work" is true only at the `serve(AppSpec)` layer: scheduler (CI-P12 claim/reap/DRR), pipeline
body (CI-P15), metering (CI-P17), check emitter (CI-P18), result rollup (CI-P19), log pipeline
(CI-P20/21), fleet (CI-P14), dispatch trigger/resolve (CI-P10/11) all EXIST as tested modules — but
both CI mains register `consumers: Vec::new()`, so nothing subscribes, claims, or drives live. The
Git side (CheckStatusConsumer, merge gate/queue) is already live against a synthetic emitter. Named
gaps found: metering has NO durable PG impl (model-only); the `.myelin/ci.*` YAML/TOML text parser +
JSON-Schema validation is missing (`resolve_snapshot` takes pre-parsed `CiDefinition`); runner rides
an in-memory `JobLeaseStore` + `CountingFirehose`/`FsBlobStore` floors; check ingestion is bus-only
(no HTTP report endpoint — "only CI may git.checks.report" is an authz token, not a mounted route).
Decomposition (scout, deps in parens): **CT-004a** durable kill-9 harden + real PG metering — **DONE `57f42b6`**
(`CiCostEventStore` with_pg: settle_in_tx caller-tx co-commit + cost_events_for_run readback + deterministic
cost_id idempotency; owns ONLY the CI reporting projection, money-truth stays in storage CostLedger; dormant,
CT-004d attaches it. Kill-9 durability proven through the store 1/1 + p28 regression 4/4. **CT-004d PREREQ found:**
Storage `cost_event` [0050] vs CI `cost_event` [ci_0014] table-name collision in the single-binary composition —
harmless today, must reconcile [CI own DB / distinct name] before a live settle.) · **CT-004b** dispatch trigger consumer live (git.ref.updated→compile→dedup→stamp→resolve→
reserve_and_start) + the real ci.* parser (a) — **PARSER DONE `3e14285`** (`parse_ci_config` TOML+JSON,
deny_unknown_fields fail-closed, YAML deferred [serde_yaml archived]; structural-validation half, resolver
keeps DAG/digest/matrix; 17 tests + compose proof). **CONSUMER DONE `381a0e4`** (live `ci-dispatch.trigger`
EventHandler → read `.myelin/ci.*` at new_oid via myelin-git read backend → parse → resolve_snapshot →
persist the atomic reserve/start bundle; proven on live PG — envelope→durable ci_run + queued ci.check.updated
+ ci.run.started in one tx, idempotent [redelivery→0 dup]; trigger-match = type-family equality NOT
EventMatcher [authz run-object gate — documented seam]; config/resolve errors = fail-closed surfaced skips;
main.rs still boots the SHELL [no default-feature CAS BlobStore/git-read backing]; live pipeline EXECUTION =
CT-004d). **⚠ CLAIM CORRECTION (peer review 2026-07-16, findings 6/7/9 — see
`planning/system-reviews/2026-07-16-peer-review-burndown.md`):** "consumer DONE" = the consumer LOGIC is built +
proven in test; it is NOT registered in the prod ci-dispatch main (`main.rs:133` `consumers = Vec::new()` —
CAS-BlobStore-gated deploy floor, #6). And the exactly-once claim is OVERSTATED: the dedup mark commits before
the handler effect in a separate tx (#7) → a kill-9 window is AT-MOST-ONCE (MR-023b floor); the CT-004b
idempotency test derives deterministic event ids that prod's fresh-ULID emit does NOT (#9). #7/#10 are P1
burndown. Lesson: floor-confessions belong in the HEADLINE, not just a code comment. **★ CT-004m (NEXT — shared foundation blocker from BOTH a+b): CI durable-table migration/ownership
reconcile.** The 14 CI tables (ci_run…cost_event) are owned by `ci_controlplane_migrations()` via the CI
serve AppSpec; each service takes its own DATABASE_URL. (1) ci-DISPATCH writes ci_run but doesn't apply the
CI migrations → prod-per-service-DB lacks it; the production durable ci_run writer belongs in a shared
CI-schema owner (likely myelin-storage). (2) storage cost_event [0050] vs CI cost_event [ci_0014] collide in
the single-binary composition. Reconcile: a clear CI migration owner + schema/DB boundary (own schema OR
ci_-prefixed names) so dispatch+controlplane point at the same migrated CI tables in dev AND prod. Design-heavy
— ground the DB-per-service boundary first. Unblocks LIVE serve-wiring of CT-004a settle + CT-004b consumer +
CT-004c/d. — **DONE `8059a7b`**: confirmed ONE shared `myelin` DB for all services (per-service DB is
aspirational); CI dormant → rename-in-place clean; CI `cost_event`→`ci_cost_event` (collision-complete, only
that one collided); shared `ci_durable_migrations()` applied by BOTH CI mains (writer subset ci_run/check_attempt/
ci_cost_event from the same DDL/ids as the full set — subset==full test); CT-004a/b tests now hit REAL migrated
tables; new no-collision proof (both cost_event + ci_cost_event coexist, both stores write); real-boot sanity
(each CI main creates the tables in public, no swallow, boot-order coupling gone). **CT-004d FLOOR:** ci_cost_event
is FORCE-RLS but CiCostEventStore writes on a bare pool w/o the (tenant,region) GUC — live settle needs a
tenant-scoped tx (CT-004d). · **CT-004c** scheduler/lease loop live on `job_queue`
+ reaper/autoscaler background loops (a). **SCOUTED + SPLIT for risk (untrusted-exec isolation):** every piece
exists but is unit-only — no pool-backed store, no running loop; TWO parallel in-memory lease models
(controlplane `SchedulerState` + sandbox `JobLeaseStore`) are NOT connected. **CT-004c.1 DONE `c9c6766`** (`CiJobQueueStore`:
per-tenant enqueue/complete/heartbeat/cancel under `with_tenant_tx` RLS; region-scoped cross-tenant claim/reap
under `with_region_tx` [clears tenant GUC, no bleed] isolated in job_queue_region.rs with a NAMED tenant-predicate
exclusion [placement_durable precedent, honest — only the 2 cross-tenant queries]; JobQueueReaper spawned off the
serve runtime, minimal-impact; proven on live PG incl. the trust-tier gate [trusted-only claim NEVER leases
untrusted_fork/self_hosted], SKIP-LOCKED no-double-lease, reap-no-orphan/no-dup, kill-9, RLS under myelin_app;
lints 175/0, unit 400/0. Floor: region-scoped scheduler DB role for prod non-superuser claim/reap [region GUC
already set]). **CT-004c.2 DONE + VERIFIED `8757a4d`:** the durable-backed RunnerAgent claims from CiJobQueueStore →
`SandboxBackend::launch` executes untrusted code in a REAL runsc guest → job.done → settle. `LeaseStore` port
trait keeps `run_one` byte-for-byte; `DurableLeaseAdapter` forwards region+labels+allowed_tiers VERBATIM
(empty-tiers = ANY('{}') = claims nothing, fail-closed). The former production composition behind
`MYELIN_CI_RUNNER=1` has been removed: the flag now exits non-zero before database bootstrap until a real
durable `CostLedger` reserve/settle authority and live per-run-token verification are wired. The runner remains
an integration-proven component, not a production-activatable service. **Independent Fable adversarial verifier:
CONFIRMED-SOUND** (own probes: trusted-only runner hammered 20× never leases fork; empty-tiers claims nothing;
region-gated) — no path for untrusted code to reach a trusted runner / escape the gate / bypass the sandbox /
leak guest bytes. **ORCHESTRATOR CAUGHT:** builder reported 4/4 but the 3 security assertions were silently
DYING at per-pid schema-collision setup (UNPROVEN) — fixed test-schema uniqueness, now genuinely 4/4 stable.
**CT-004d must-fix (verifier MEDIUM, latent):** lease_ttl_secs=30 < job timeout(60) → a long real job could
lapse mid-run → re-execute (at-least-once by design; exactly-once WAKE holds) — raise TTL above max timeout OR
heartbeat during launch when real specs land. Sandbox prod-exec PROVEN (CT-002/003, 0 escapes).
· **CT-004d** pipeline body + metering bookends live (b,c). **d.1 DONE (dispatch→durable-JobSpec→resolve
bridge, real specs execute in runsc). d.2 SCOUTED (2026-07-17) — pure orchestration glue (untrusted-exec is
c.2, done+verified); 6 chunks:** (1) executor start-with-id [`DurableExecutor::start` mints its own run_id +
drops the consumer's pre-minted `wf_run_id`; add a caller run_id — foundational, no security surface, blocks
2/3] · (2) register+drive the ci.pipeline BODY in prod [`FlowDispatcher::register(CI_PIPELINE_WF_TYPE, body)`
+ tick, IN ci-controlplane same-process as CiRunnerLoop so job.done wakes the parked run — pattern:
`flow/dogfood.rs:165-206`, `flow/app.rs:151-200`] · (3) the start call at reserve→start · (4) durable
`CiRunStore::with_pg` [mirror CiJobSpecStore — the run-of-record, currently only written in the test path] ·
(5) durable JobRunner for the body's dispatch [sync-over-async → `CiJobSpecStore::co_persist_dispatch`;
SECURITY-ADJACENT: forwards trust-tier==spec-tier unchanged] · (6) #7b durable consumer DLQ [mirror
DurableDedup — a `consumer_dead_letter` table + backing]. **ARCHITECTURAL FLOOR found: no durable RunStore/
executor exists across processes (FlowExecutor is in-memory, per-process) — either drive the run in-process
with a durable-backed RunStore, or job.done rides the bus (sig.<tenant>.). A durable RunStore backing is an
implied floor; flag if chunks 2/3 hit a real fork.** **chunk 1 DONE `94d5a25`** (`DurableExecutor::start_with_id(spec, Option<RunId>)` + defaulted `start` — zero call-site churn; idem_key wins on redelivery; colliding run_id under a different idem_key → typed RunIdConflict fail-closed). **NEXT chunks:** 4 (durable ci_run writer) is really the TRUE co-commit for the CI consumer — it discharges #7's H1 floor [prod uses separate-tx+absorb; ride the ci_run row + events on the HandlerTx co-commit tx] → LOAD-BEARING, gets the full independent-adversarial-verify discipline. **chunk 4 DONE `9d0baee`** (durable CiRunStore, ci_run row co-committed with the mark on the HandlerTx tx; events stay absorb; independent Fable pass CONFIRMED-SOUND — events-lead-row benign [merge gate blocks on absent/queued], RLS write-forge REFUSED under myelin_app; 3 LOWs named). **chunk 6 (#7b DLQ) DONE `15c3033`** (durable consumer_dead_letter, PII-free). **★ CHUNKS 2+3+5 CULMINATION DONE `aa6738d` — VERIFIED (Fable CONFIRMED-SOUND, own live-PG fork probe: fork→trusted-runner UNREACHABLE): A PUSH RUNS A REAL runsc PIPELINE END-TO-END** — durable ci_run(queued) → parked ci.pipeline run (start_with_id w/ the pre-minted wf_run_id) → DurableJobRunner dispatches each stage (trust_tier stamped from the run, forwarded unchanged) → durable job_queue row → CiRunnerLoop claims + runs `echo` in a REAL runsc guest (exit 0) → CiPipelineReporter's job.done wakes the parked run → COMPLETES → ci.run.succeeded. **CT-004d.2 CORE COMPLETE (1/2/3/4/5/6); the dispatch→execute CI spine is integration-proven but NOT production-activated; `MYELIN_CI_RUNNER=1` is refused at startup.** Named hardening floors: the ci_run-poll STARTER must be per-tenant. **Tenant boundary hardened 2026-07-18:** `CiRunRecord` now reads the authoritative `tenant_id`, and `CiPipelineDriver::start_run` refuses a record whose tenant differs from the driver before registering a plan or creating any run/job state (live-PG RLS round-trip + unit mismatch/acceptance proofs green). The starter itself remains unwired, and main no longer constructs a synthetic driver. Durable RunStore (myelin-flow M2) also remains. Remaining CI track: **CT-004e VERIFIED CLOSED (2026-07-17, no build)** — the X-1 check/result seam is
complete by CONSTRUCTION + tested at each link: the CI producer (`ci_pipeline.rs`/`emit_check`) emits
`ci.check.updated` via the FROZEN `myelin_events::check_seam::check_updated_draft` (subject
`repo#commit-<oid>/check-<context>`); Git's `CheckStatusConsumer` decodes `OrderedCheck::check_status`
from the SAME frozen codec → the `check_status` projection the merge gate reads. Each link tested
(consumer `cdc_5_9_check_status_consumer`, merge gate `e2e_git_p21_merge_gate` +
`cdc_5_9_merge_gate_required_set`); the culmination proved CI emits `ci.run.succeeded` + `ci.check.updated`.
The full chain in ONE process = the cross-cell DELIVERY (a named deploy floor across the CI track, NOT a
seam gap); required-contexts config alignment is operational. **CT-004f** [log pipeline substrate — floors
exist, needs live firehose/BlobStore binding], then **CT-005** (surfaces) → **CT-007** (GitHub Actions cutover — the reward). ·
**CT-004e** check/result producers live → closes the X-1 seam to the merge gate (d) · **CT-004f** log
pipeline live substrate (a; SSE tail is CT-005) · **CT-004g** fleet/residency (c, optional split).

**CT-004e CODE-WINS CORRECTION (2026-07-23).** The 2026-07-17 “complete by construction”
paragraph immediately above is historical and **SUPERSEDED**. It proved compatible codecs and
in-process models, not a production Git consumer service or durable authority read by Edge; the
initial queued check also omitted frozen fields, and the human-readable aggregate was not a legal
structured-broker token for every canonical repository ref. The production closure is recorded in
the 2026-07-23 durable CI→Git check-projection increment below.

**CI launch-wire V2 increment (2026-07-18 late, `435bf7c` + `5cb3124`; recorded 2026-07-20 by code-wins
reconciliation — this landed AFTER the last ledger edit).** A staged V1→V2 migration of the CI plan
contract: `.myelin/ci.*` now parses to `CiPlanContract::V1` or `::V2` (`ci-dispatch/config.rs`,
`resolve.rs` — V2 = a resolved request that preserves authored stages and NAMES an execution profile,
e.g. `execution: { profile: linux-small-v1 }`, without granting runtime authority), resolving to
`VersionedResolvedSnapshot::V1|V2`. Execution stays fail-closed on V1 only: `run_plan.rs
load_resolved_run_plan` REFUSES V2 with `RunPlanError::LaunchAuthorityRequired` until durable launch
authority is materialized; legacy V1 call sites are byte-identical-preserved. Honest state: V2 is an
authored forward contract, NOT an active execution path; no V1-removal is scheduled.

**CI claim-bound completion authority (2026-07-21, `e594d34` + `00f9bf0`).** The three completion
blockers from the dormant CT-004d driver are closed without activating V2. `PgFlowExecutor` now exposes
a scope-verifying caller-transaction typed-signal seam; `CiPipelineReporter` uses it to verify the
co-persisted dispatch identity, consume the exact `(owner, epoch, fresh UUID nonce, durable stage)`
claim, and buffer canonical `CiJobDone` in one tenant-scoped PostgreSQL commit. The v2 receipt binds
tenant/region/run/job/idem/stage/verdict/ordered result refs/owner/epoch/nonce; invalid refs are refused
and roll the claim transition back (none are silently filtered). Runner authority travels as one typed
`CompletionClaim`. Historical NULL-stage rows are never silently quarantined: the least-privilege
region-scheduler capability counts `job_queue.stage IS NULL`, and the production activation seam refuses
while any non-terminal backlog remains. Migration immutability is probe-pinned: shipped `ci_0004a` and
`ci_0016a` DDL remain byte-identical; new nonce/stage and nonce-grant work is forward-only under
`ci_0004b`/`ci_0016b`. Proof: 361 control-plane unit tests; 121 sandbox unit tests; all-feature clippy;
732-file CI lint scan at zero; live migration-upgrade + constrained scheduler-boundary + claim/reap/
kill-9 suites; and the full live-PG culmination (both adversarial completion cases and a real `runsc`
guest pipeline) green. **Honest remaining floor:** start/signal are PG-durable, but body drive still
mirrors through the in-memory dispatcher; V2 launch and `MYELIN_CI_RUNNER=1` remain fail-closed until
`PgFlowDriveStore` lease/replay and the existing launch-authority floors are composed end to end.

**CI durable-drive boundary correction (2026-07-21, `ddcc006` + `63c2049`).** The default production
build no longer contains the process-local completion mirror or exports the restart-unsafe
`CiPipelineDriver`; that compatibility harness and its fixed-command builder are confined to the
`test-support` feature. `CiPipelineReporter` is PostgreSQL-only in production: its successful return
means the exact claim transition and typed `job.done` signal committed atomically, and a
tenant/region/partition-scoped `PgFlowWorker` is the only durable consumer path. The live culmination
now starts the pinned test body on PostgreSQL, drives it to a durable wait, destroys the worker,
reconstructs a fresh worker with the same definition identity, executes the claimed stage in a real
`runsc` guest, then consumes the durable signal and commits terminal workflow history plus the X-1
outbox events. That proof exposed and closed two masked outbox-conformance defects: run lifecycle
events now use their already-canonical run ref as subject, and the Refs parser recognizes the frozen
Event Bus `repo#commit-<oid>/check-<context>` and `repo#commit-<oid>/ci-result` spellings as canonical
CI check-family anchors. Proof: default control-plane tests 362/362, Refs tests 28/28, all-target/
all-feature clippy for both crates, and the two-case live culmination (adversarial claim authority +
real `runsc` restart/replay) green. **Honest remaining activation floor:** production still has no
restart-safe immutable CI body-input manifest carrying ordered/DAG semantics and `CheckFacts`; V1's
prepared plan is explicitly a DAG, not the legacy sequential `PipelineRun`, and V2 still lacks launch
authority. Do not compose the production worker fan-out or enable `MYELIN_CI_RUNNER=1` until those
contracts are durable and replay-deterministic.

**CI immutable drive-manifest foundation (2026-07-21, `d8b0925` + `42ad3a1` + `3c891b0`).** The
first activation prerequisite is now durable without pretending launch authority exists. PostgreSQL
attempt allocation is retry-safe: an exact retry for the same `current_run` reuses its issued
per-context attempt, while distinct runs remain strictly monotonic (including across process restart).
The new `ci_drive_manifest` table is tenant/region scoped, one-to-one with both `ci_run` and the
pre-minted workflow run, retains canonical bytes beside a domain-separated BLAKE3 digest, and is
structurally insert-only: the runtime role has no UPDATE/DELETE grant and a defensive trigger refuses
even owner mutations. `CiDriveManifestV1` is a strict server-authored contract for the exact DAG,
matrix identity, per-context attempts, workflow definition pin, provenance, resource/scheduling
grants, reserve handle, and stable token-mint authority; secret values and minted token JTIs are
absent. Validation rejects unknown/noncanonical wire fields, DAG cycles or missing dependencies,
unpinned images, unsafe workspace posture, and check-context drift. Merge waiter authority is
optional, so ordinary runs no longer need to fabricate a merge token/target. The PostgreSQL store
supports caller-transaction insertion for the future starter co-commit and exact replay verification;
a divergent uniqueness collision is loud. Proof: 367/367 control-plane unit tests, all-target/
all-feature clippy, live insert-only/RLS schema proof, live exact replay/divergence store proof, and
the constrained split-role full bootstrap. **Honest remaining activation floor:** the queued-run
starter does not yet allocate attempts + materialize policy-granted job templates + insert this
manifest + start the manifest-referenced workflow in one transaction. Its current snapshot-only input
is therefore not promoted, and production activation remains refused until that authority adapter and
the DAG-native body consume path land.

**CI manifest-backed V2 queued launch (2026-07-21, `226a1a9` + `7738b7d`).** The exact-cell starter
now accepts only canonical V2 launch requests and has an explicit asynchronous launch-authority
boundary. There is no permissive production adapter: the composition-root default refuses a fresh run
before allocating attempts or writing durable state. The authority can grant only runtime terms; job
identity, authored stage, matrix, dependencies, image, command, workspace, trust, check context, and
attempt are derived from the locked `ci_run` plus validated V2 plan. Failure semantics remain fixed at
`continue_on_error=false` until the authored contract can request them. The exact queued-row lock winner
alone invokes authority, whose external reserve/token implementation is contractually retry-safe by
run id. In one scoped PostgreSQL transaction the starter allocates distinct authored-context attempts,
builds/inserts the canonical immutable manifest, materializes the exact job ledger, starts/verifies the
workflow with `[drive-manifest digest ref, ci_run ref]`, and advances `queued -> running`; any later
failure rolls the whole set back. Replay is genuinely manifest-backed: it reads neither the source CAS
nor current policy/current composition pin, requires manifest+workflow as an atomic pair, validates the
frozen definition pin and attempt-ledger consistency, reconstructs immutable job facts directly from
the manifest, refuses missing rows, and preserves advanced lifecycle fields. Validation now pins the
V2 snapshot class, derived check contexts, bounded collections/resource ceilings, exact run-plan image
and argv semantics, and observable database causes. Live proof covers winner-only authority under two
concurrent starters, one manifest/workflow and one attempt allocation per context, unavailable-authority
zero-side-effect refusal, replay after source-CAS deletion and a bogus current pin, older-run replay
after check supersession without reallocating, orphan manifest/missing attempt/missing job refusal,
advanced job-state preservation, cross-cell RLS, and rollback of manifest/jobs/workflow/attempts/state.
Proof: 370/370 control-plane unit tests, all-target/all-feature clippy with warnings denied, the expanded
live PostgreSQL starter proof 1/1, and an independent focused audit with no remaining blocker from its
five findings. **Honest remaining activation floors:** compose a real policy-aware authority adapter,
emit the initial queued/in-progress check facts, make the durable workflow body consume this manifest as
a DAG, add exact-tenant poller/worker fan-out, and close the myelin-flow M2 durable `RunStore` floor.
`MYELIN_CI_RUNNER=1` remains startup-refused; this increment does not activate production execution.

**CI causal initial manifest checks (2026-07-21, `a43a36f`).** A fresh manifest-backed start now
co-emits one deterministic `ci.check.updated{in_progress}` fact per frozen authored context in the
same PostgreSQL transaction as the manifest, job ledger, workflow start, and `queued -> running`
transition; rollback leaves none and manifest replay emits none again. Dispatch and launch share the
versioned check-context mapping, so the earlier queued facts and these in-progress facts advance one
Git check lifecycle rather than creating `build`/`ci:build` siblings. Git's two-way trust projection
also maps member self-hosted execution to `trusted`, while forks remain `untrusted_fork`. Envelope
timestamps use the actual database wall clock; the immutable run start stays in the payload.
Asynchronous causality no longer relies on retaining the triggering outbox row: dispatch persists the
trigger event id, correlation root, canonical depth, and originating `caused_by` on `ci_run` through a
forward-only migration, and the starter derives byte-identical child provenance from that durable
snapshot. The store rejects depths outside the canonical `u32` range before SQL, and PostgreSQL pins
the same range. Proof: 653 event/control-plane/dispatch unit tests, warnings-denied all-feature clippy,
live migration/bootstrap, exact replay/RLS store, immutable manifest store, atomic starter rollback/
replay, and all six dispatch crash/replay cases; the independent re-audit confirmed the prior
causality blocker closed. **Honest remaining activation floors:** compose a real policy-aware launch
authority, make the durable workflow body consume the manifest as a DAG, add exact-tenant poller/
worker fan-out, and close the myelin-flow M2 durable `RunStore` floor. Production execution remains
startup-refused.

**CI immutable-input resolver and manifest-native DAG body (2026-07-21, `fd5e2b8` + `182bf3e`).**
`PgFlowWorker` now supports a bounded asynchronous immutable-input resolver under the exact fenced
drive lease. Retryable infrastructure failure releases the lease with no history, outbox, signal,
or run settlement; permanent identity/digest/schema refusal halts the run nondeterministically
without invoking the synchronous body. The production CI resolver strictly decodes the starter's
two canonical references, loads the tenant/region-scoped insert-only manifest by workflow/run/digest,
and reasserts the deployed workflow version and code hash. Registration pairs that resolver with the
new manifest-native body so a fresh V2 launch cannot accidentally use the legacy sequential fixture.
The body dispatches every ready DAG frontier, joins by Flow's deterministic deadline/token order,
drains already-dispatched siblings after a failure, suppresses descendants of a failed frontier, and
emits terminal checks using each manifest-frozen context attempt and a journaled completion clock.
The strongest live proof starts through the real PostgreSQL starter, injects a resolver outage with
zero commit, dispatches two roots, destroys the worker, delivers their completions out of order,
reconstructs a worker to launch the dependent, reconstructs again to terminalize, and observes exact
manifest job UUID targets plus per-context success facts. Full `myelin-flow` and
`myelin-ci-controlplane` all-target/all-feature suites and warnings-denied clippy are green. The full
gate also exposed and fixed a pre-existing parallel integration-test schema collision (`3ce4808`).
**Honest remaining activation floors:** supply a real policy-aware launch authority; translate each
exact manifest job into the durable queue/sandbox spec with an explicit retry-safe token mint (the
manifest authority handle is not a token JTI), exact replay readback, reserve/settle, and claim-bound
reporter identity; settle `ci_run` and terminal cost posture; attach Flow budget/remint hooks; and run
the exact-tenant region poller/worker composition. `MYELIN_CI_RUNNER=1` remains startup-refused.

**CI exact manifest dispatch and claimed completion identity (2026-07-21, `c932fe4` +
`fd88acc`).** Durable queue/spec replay now reads both rows back inside the insert transaction and
requires the complete requested tuple to match; an idempotency collision, job UUID collision, or
scheduling/spec drift rolls the transaction back instead of silently accepting `ON CONFLICT DO
NOTHING`. The manifest-native production adapter preserves each immutable `ci_job.job_id` through
`job_queue` and `ci_job_spec`, translates the exact image, command, environment, secret handles,
egress, resources, workspace provenance, trust tier, reserve, lane, labels, concurrency group, and
fair key, and accepts Flow only as the source of the engine-minted idempotency token. Per-job token
minting is an explicit adapter with no permissive default; its complete request is retry-stable and
the returned bounded JTI must differ from the manifest's authority handle. The completion reporter
now authenticates arbitrary manifest job UUIDs through the durable spec identity plus the live
queue claim CAS, rather than requiring the unrelated legacy `stage_job_id(idem_token)` derivation;
missing, cross-tenant, stale-lease, wrong-run, wrong-token, flipped-verdict, and replay forgeries stay
fail-closed. The live PostgreSQL proof starts from the real manifest starter, survives resolver
retry and worker reconstruction, verifies exact immutable root/dependent UUIDs in the durable queue,
reads every executable and scheduling field back from the persisted sandbox specs, and completes the
out-of-order DAG exactly once. Its drive clock is derived from PostgreSQL, removing a hard-coded
wall-clock deadline flake. The full control-plane all-target/all-feature suite, real-runsc
culmination/adversarial completion proof, and warnings-denied clippy are green. **Honest remaining
activation floors:** compose real policy-aware launch and token-authority adapters; attach Flow's
budget/remint hooks and the live reserve/settle bookend; atomically settle `ci_run` plus terminal cost
posture; run the exact-tenant region poller/worker composition; and close the myelin-flow M2 durable
`RunStore` floor. Production execution remains startup-refused.

**CI `linux-small-v1` launch-policy boundary (2026-07-22, `fd6f3bc4` + `de581870` + `19ee677b` +
`6d133138` + `239d64ab`).** The first real
`CiLaunchAuthorityMaterializer` now converts the customer-authored profile into fixed server-owned,
default-deny grants: zero egress, environment, or secret handles; 1 vCPU, 256 MiB RAM, 1 GiB disk,
128 PIDs, and a 600-second timeout; batch scheduling with frozen labels and project fair-share
identity. The adapter derives every job UUID from the locked run and validated plan, binds each
external request to tenant, region, run/workflow/project, stage and concrete name, trigger and trust,
source snapshot digest, workflow definition pin, policy revision, and exact limits, and refuses
malformed locked scope, invalid handles, or a reservation reused across jobs. Only retry-stable
operational budget reservation remains behind an explicit provider; token-authority references are
derived locally and bearer minting stays at the later live-claim boundary. There is no permissive
budget implementation. The no-authority starter constructor is now test-support-only:
production must call `new_with_authority`, and the dormant composition binds this real policy to an
explicit unavailable budget provider that fabricates no reservation handle. Queue claims now return
their
start and expiry from the same PostgreSQL `statement_timestamp()` used to write the lease. Token
requests must carry both instants and fail closed unless the identity, scope, ordering, and exact
runner-lease lifetime are valid before any issuer can mint; the live scheduler proof observes the
requested 30-second lifetime exactly. **Tier-P deviation boundary:**
the existing storage/Flow reserve contract names a future Commercial wallet, while the governing
P-track explicitly defers billing and Stripe. The code remains fail-closed instead of inventing that
wallet: launch authority now asks one explicit operational quota/cost provider to reserve the
complete job set atomically and retry-stably, with cardinality, handle-shape, and uniqueness checks.
This is a personal-production safety/metering seam, not a billing integration. Separately, each token
authority reference is a domain-separated, unambiguous BLAKE3 binding over every locked run, job,
source, workflow, policy, and limit field; it is persisted in the immutable manifest but is neither a
bearer nor a JTI. Nine focused tests pin exact batch replay, full-field token binding, malformed and
duplicate provider output, pre-call scope rejection, and dormant no-fabrication; the full
control-plane all-target/all-feature suite, production source guard, warnings-denied clippy, and L2
gate are green. An independent adversarial verifier reported CONFIRMED-SOUND with no HIGH/MEDIUM
findings.

The claim-time authority wrapper now locks the exact `job_queue` claim before `ci_run` in one
tenant-/region-scoped transaction, then reloads the immutable drive manifest and reconstructs the
complete token-authority request server-side. It refuses unless state, idempotency token, stage,
trust tier, runner owner, lease epoch, nonce, original claim window, run identity, and manifest
digest all agree and PostgreSQL still considers the claim live; only then is the raw Identity mint
seam reachable while both rows remain locked. Initial claim timestamps are now persisted separately
from heartbeat-extended `lease_expires` via forward-only nullable columns, so old/unmigrated rows
refuse mint and acknowledgement-loss retries retain one durable generation. The scheduler's
least-privilege role receives only those two additional column updates, and its live privilege audit
checks both required and excess grants. Live PostgreSQL proof admits the exact durable claim and
refuses forged handle/owner/time, reaped/null, and reclaimed generations before the raw minter; the
full control-plane all-target/all-feature suite and warnings-denied clippy are green. An independent
adversarial re-audit reported CONFIRMED-SOUND with no HIGH/MEDIUM findings.

**Concrete Identity generation + exact final launch fence (2026-07-23, `ef4c91f1`).** The claim-time
adapter now mints a real PASETO-backed Identity CI credential under S7. A production 6h10m scheduler
claim gets one deterministic token generation whose absolute deadline is
`min(claim_started_at + 300s, claim_expires_at)`: concurrent acknowledgement-loss retries return the
same credential while that window is live, and the same claim refuses rather than reminting after
the window. The final authorizer cryptographically pins the CI scheme, kind, purpose, tenant,
region, service principal, job, carrier/signed JTI, required capabilities, signed expiry, and S7
liveness. Its non-serializable launch context also carries workflow run, lease owner/epoch/nonce,
and both original claim timestamps.

Authorization is no longer a copied-fact check. After isolation/config derivation and successful
operational reservation, the last mutable action before sandbox spawn is one exact durable
`leased` → `running` CAS over every scheduler-generation field. Cancel-superseded and launch now
serialize on the same row: cancellation winning first refuses launch; launch winning first makes the
row ineligible for cancellation. A refused final authorization invokes the pre-spawn release
bookend, but the composed production hook zero-settles only an exact generation already proven
terminal without a completion receipt; a retryable, replacement, running, or reporter-completed
generation retains the deterministic reservation for its real owner. Heartbeat and dead-runner
recovery accept `running`, so a process death after the fence does not create an orphan. The live
PostgreSQL test races cancellation against the exact launch CAS, proves exactly one winner, refuses
CAS replay, and reaps an expired running generation. Real PASETO tests cover concurrent retry,
forgery, scope/capability/expiry divergence, S7 teardown, the full production lease lifetime, and a
refused durable fence.

The same verification uncovered that the gVisor process-pressure corpus had been mechanically
dishonest: it printed `CONTAINED` unconditionally, while rootless `runsc` actually admitted all 300
children because its OCI pids cgroup field was advisory. The runtime config now adds
`RLIMIT_NPROC` at the same server-owned PIDs ceiling, and the corpus counts admitted processes,
reports an escape above the ceiling, and isolates/reaps the pressure probe before later resource
checks. The real production-path rerun admitted 58 under the ceiling of 64 and completed all twelve
probes with zero escapes and zero did-not-run outcomes. Full control-plane and sandbox
all-target/all-feature suites, the agent-service suite, edge focused tests, workspace all-target
check, warnings-denied clippy, architecture lint, L2 erosion gate, and contract-coverage gate are
green.

**Code-wins deviation:** the first adapter revision wrapped `spawn_blocking` in a timeout, but Tokio
cannot cancel a blocking task; timing out could release the durable locks while signing/S7 mutation
continued detached. The concrete in-process mint is now awaited to completion instead. If a future
remote signer is introduced, its transport must provide a genuinely cancellable deadline before it
can replace this boundary; a cosmetic timeout is not an accepted gate.

**Concrete Tier-P operational reservation source (2026-07-23).** The PostgreSQL provider now admits
one complete ordered manifest batch into Storage's existing `cost_reservation` ledger under a
tenant/region transaction advisory lock and an explicit per-tenant outstanding-job ceiling. Every
handle binds the complete ordered batch plus every immutable per-job launch-authority field; exact
acknowledgement-loss retries return the same ordered handles even after lifecycle advancement, while
subset, superset, reorder, disjoint replacement, changed authority, duplicate jobs, exhausted
capacity, and partial durable state refuse. Same-database reservation runs on the starter's scoped
transaction, so reservation, manifest, attempts, job ledger, workflow start, and queued→running share
one commit. An injected post-reservation manifest failure leaves zero rows and a clean retry commits
the complete three-job start.

Tier P records operational capacity units—not customer prices—and terminal settlement now gates that
policy mechanically. A `ci-reserve:v1:` handle accepts only revision `tier-p-operational:v1`, CPU
wholesale equal to measured CPU-seconds, memory quantity and wholesale equal to rounded-up measured
GiB-seconds, and zero markup in both dimensions. Any field mismatch rolls the complete claim,
reservation, cost projection, accounting receipt, and workflow-signal transaction back. Live
PostgreSQL proofs cover concurrent exact retries, competing fresh batches at the ceiling, mid-batch
failure rollback/recovery, starter-level crash rollback/recovery, FORCE-RLS visibility, and terminal
pricing refusal/success; DB-free mutants cover every settlement-policy field. The focused
all-feature proofs, full control-plane suite, workspace all-target check, warnings-denied clippy,
architecture lint, erosion gate, and contract-coverage gate are green. Independent adversarial
review reported CONFIRMED-SOUND with no remaining HIGH/MEDIUM provider or terminal-policy finding.

**Single terminal settlement ownership (2026-07-23).** Successful completion now has one explicit
typed owner: generic Agent/Git execution settles through its sandbox hook, while durable CI defers
settlement to the reporter transaction that also consumes the claim, records the immutable receipt
and cost projection, and signals the workflow. `RunnerHooks` keeps its raw lifecycle closures
private, requires the owner at construction, and exposes only owner-aware completion and
pre-spawn-release methods; alternate backends therefore cannot call the raw settlement closure.
`TerminalReporter` implementations must declare their owner, and `RunnerAgent` refuses either
double-owned or unowned composition before queue claim, reservation, or launch. The production
runner loop treats that mismatch as a static configuration refusal and stops the lane.

Both Firecracker and gVisor prove that reporter-owned success does not invoke hook settlement, while
final-attribution refusal still invokes the pre-spawn bookend. The production implementation
distinguishes safe canceled-generation zero release from retry/replacement/running ownership instead
of treating every refusal as terminal. A runner-level rollback/reclaim/retry proof leaves the failed
claim retryable and records exactly one reporter settlement with zero hook settlements. The durable
PostgreSQL reporter advertises reporter ownership and its existing injected-failure proof rolls
claim, reservation, projection, receipt, and signal back together before a clean retry. The
exact-tenant router also rejects any factory output whose actual owner contradicts its
durable-accounting contract, including a test-support bypass. Full sandbox, control-plane, Agent,
and Edge all-target/all-feature suites, workspace all-target check, warnings-denied clippy,
architecture lint, erosion gate, and contract-coverage gate are green. Independent adversarial
review reported CONFIRMED-SOUND with no HIGH, MEDIUM, or remaining code LOW findings after its
fixture-honesty findings were corrected.

**Production Tier-P reservation binding (2026-07-23).** The dormant production starter factory now
constructs the proven PostgreSQL Tier-P reservation provider from the app-role pool and exact cell
region, and propagates construction refusal instead of retaining the placeholder provider. Its
server-owned outstanding-reservation ceiling is 1,024: deliberately distinct from the measured
64-job scheduler cap, because reservations include queued DAG jobs while that cap includes only
leased/running jobs. A mechanical invariant ties 1,024 to the largest valid run plan, so an otherwise
idle tenant can reserve one maximum-size run; further runs fail closed until earlier reservations
settle, while scheduler excess continues to queue at its independent 64-job concurrency boundary.

The live PostgreSQL starter proof now enters through the exact public production factory. An injected
failure after all three reservations but before the manifest leaves reservation, manifest, and
check-attempt counts at zero; an ordinary retry commits all three. A production-source gate pins the
PostgreSQL provider and dedicated ceiling, rejects both the obsolete unavailable provider and the
scheduler cap at that seam, and keeps runner activation refused before configuration, database
bootstrap, or factory construction. The dead placeholder implementation and its self-contained
fixture test were removed. Full row gates are green. Independent adversarial re-review reported
CONFIRMED-SOUND with no remaining HIGH or MEDIUM finding after catching and closing an initial
scheduler-cap semantic mismatch.

**Production durable Identity composition (2026-07-23).** The dormant production runner path now
constructs one cell-scoped Identity authority pair from a sealed durable cell root, durable S7
revocation state, the real PASETO signer/verifier, the locked durable-claim token issuer, and the
exact durable final-launch CAS authorizer. Both mint and launch are bound to the provider's exact
cell region. Cell IDs must be nonempty, canonical (no surrounding whitespace), control-free, and no
longer than 128 characters before any root lookup, preventing visually ambiguous IDs from creating
separate signing roots.

The live PostgreSQL proof mints through a first production authority construction, proves that a
wrong seal key cannot replace its root, reconstructs the authorities from the same root and S7
state, and uses the second construction to verify that token and win the one-shot durable launch
CAS. A cross-region request is refused before the raw minter runs. The production-source gate
requires every durable and cryptographic component, forbids ephemeral roots/revocation stores and
structural signers/verifiers, pins provider-region binding, and preserves startup refusal before
database access. Independent adversarial review initially found the whitespace split-root risk;
after the canonical-ID guard and both-sided regression coverage, re-review reported
CONFIRMED-SOUND with no HIGH, MEDIUM, or LOW findings.

**Production scoped runner lifecycle composition (2026-07-23).** `RunnerHooks` now carries the
complete resolved `JobSpec` through reserve, pre-spawn release, and successful-completion ownership
instead of reducing the money boundary to a caller-controlled meter string. The dormant production
hook factory binds one provider region, the immutable drive manifest, the durable launch template,
the exact live scheduler generation, Storage's Tier-P ledger, the real Identity final-launch
authorizer, and the enforced hardening profile. Reservation begin locks and verifies tenant, region,
workflow/job UUIDs, idempotency token, stage, trust tier, owner, epoch, nonce, original claim times,
database-clock liveness, manifest reservation, and every executable template field before moving the
exact reservation `reserved` → `inflight`. The final attribute hook repeats the complete
manifest/template and live-generation verification immediately before Identity verification and the
one-shot `leased` → `running` CAS.

The first adversarial review found two real HIGH defects in the initial composition: a stale worker
could zero-settle the deterministic reservation after a replacement worker launched, and a
same-job command/image mutation could retain a valid token and pass the launch CAS. Both are now
mechanically closed. Pre-spawn release row-locks the current queue disposition and zero-settles only
the exact generation already terminal with no completion receipt; live retry, replacement, running
or reporter-completed generations retain the reservation. The executable check compares the whole
`JobSpecTemplate` to the durable `ci_job_spec` record and independently to the immutable manifest
(image, command, environment, secret references, egress, resource limits, workspace, trust tier,
meter, CI-run and token authority), while queue identity separately pins idempotency/stage/trust.
A re-review then found a cancellation orphan if begin committed before handle acknowledgement or
cancellation committed immediately after a retry-preserving release check. Production
cancel-superseded is therefore a separate accounted coordinator: queue terminalization and exact
Tier-P zero settlement co-commit in one tenant transaction, and the old unaccounted queue-store verb
is structurally test-support-only. The live PostgreSQL proof covers exact begin retry, nonexistent
generation refusal, same-job command mutation refusal before both begin and CAS, cross-tenant
refusal, off-runtime execution, retryable Identity refusal with no settlement, stale
replacement-generation release refusal, launch-CAS acknowledgement-loss retention, cancellation
after retry retention, cancellation after begin-commit/handle-acknowledgement loss, atomic zero
settlement of both, cancellation acknowledgement retry, and reporter-owned success remaining
in-flight. The complete row is green under workspace all-target/all-feature check, warnings-denied
clippy, the full control-plane all-target/all-feature suite (including live PostgreSQL), architecture
lint, erosion budgets, contract coverage, and the production-source activation guard. Independent
adversarial re-review reported CONFIRMED-SOUND after the two HIGH and one MEDIUM findings were
closed, with no remaining finding. Production activation remains refused.

**Production exact-tenant workflow composition (2026-07-23).** One production factory now derives a
domain-separated, length-framed definition pin from the complete bytes of both the manifest workflow
body and its job-dispatch implementation. The dormant starter and each worker consume that same pin,
so a source change cannot conventionally leave the two sides on different workflow definitions.
Given one authoritative tenant and Flow partition, the factory creates the exact service principal
and `TenantScope`, immutable-manifest resolver, `PgFlowWorker`, durable CI-run finalizer, and
manifest-native workflow registration. Its reporter router derives the same exact tenant from the
claim and constructs the accounted reporter with the durable job accounting store, Tier-P
operational pricer, manifest, money ledger, cost projection, and completion receipt store. No
default/global tenant worker exists.

The production main now composes the region run-discovery starter poller and accounted reporter
router from that factory after constrained bootstrap. It deliberately creates no `CiRunnerLoop`,
worker fan-out, task, or spawn: `MYELIN_CI_RUNNER=1` still refuses before database access. The live
PostgreSQL proof uses the same production factory to register the source-derived definition, drives
its worker through real immutable-manifest dispatch and park, leases the resulting durable queue row,
reports completion through the production accounted router, and drives the same worker again so its
captured durable finalizer terminalizes the CI run and zero-settles the skipped reservation. The
existing injected-pricer refusal still proves that claim, accounting, projection, reservation, and
signal roll back as one transaction.

Independent adversarial review initially found two MEDIUM fixture-honesty defects: the first pin
covered the workflow body but not the dispatch implementation, and the live proof only constructed a
worker without driving its captured finalizer. Both were corrected before re-review, which reported
CONFIRMED-SOUND with no remaining finding. The complete suite also exposed a retry-poisoning test
fixture: a prior failed scheduler-boundary invocation left its reserved test tenants queued. Setup now
cleans only that test-owned namespace before asserting oldest-first discovery; the least-privilege
probe itself was retained unchanged and caught the contamination correctly. The full control-plane
all-target/all-feature suite, workspace all-target/all-feature check, warnings-denied clippy,
architecture lint, erosion budgets, contract coverage, production-source activation guard, and diff
check are green.

**Bounded active-workflow recovery fan-out (2026-07-23).** The constrained region scheduler can now
discover only nonterminal `ci.pipeline` rows from Flow's authoritative `workflow_run` table, returning
the persisted partition rather than deriving one from the run ID. Discovery is a 64-row maximum
keyset page ordered by the globally unique `(created_at, tenant_id, run_id)` tuple; it uses no
transparent offset and exposes only tenant, run identity, persisted partition, and cursor time. A
new partial concurrent index covers the exact region/type/state/keyset predicate. The scheduler role
gets SELECT on exactly the seven required `workflow_run` columns behind both a permissive and
restrictive empty-tenant/server-mapped-region RLS policy; the startup privilege probe treats every
additional Flow column or mutation capability as excess.

`CiProductionWorkflowPoller` keyset-cycles across at most 64 discovered rows per pass, deduplicates
the resulting exact `(tenant, partition)` scopes, derives bounded tenant-distinct worker IDs, and
drives each exact worker at most 64 times before yielding. Production constructs this poller from
the same dormant exact-tenant runtime factory but starts no task, spawn, or `CiRunnerLoop`. CI boot
now applies Flow's exact shared migrations before the CI-owned recovery index/policy follow-ons, so a
fresh cell has no service boot-order dependency; isolated bootstrap and rolling-upgrade proofs carry
the same real prerequisite instead of accidentally targeting `public.workflow_run`.

The live PostgreSQL recovery proof commits the CI finalizer first to simulate a crash between the
CI-run terminal transaction and the Flow commit. Even though `ci_run` is already failed, the
nonterminal Flow row remains discoverable; the production poller recreates the exact persisted
tenant/partition worker, deterministic replay finishes the Flow commit, and no second terminal
effect is invented. The scheduler boundary proof also covers wrong-region exclusion, persisted
partition routing, and two one-row keyset pages. Adversarial review initially found one HIGH and two
MEDIUM errors in the first draft: terminal `ci_run` state could hide a split-commit Flow run, the
cursor omitted tenant identity, and routing recomputed a partition using a helper whose contract
forbids that use. All three were replaced by authoritative Flow state, the tenant-inclusive keyset,
and the stored partition. Re-review then found the fresh-boot Flow prerequisite; production boot and
both isolated migration proofs now apply the exact shared Flow set first. Its apparent excess-grant
failure was a stale schema left by the earlier failed upgrade fixture, not product DDL; removing that
test-owned schema restored the unchanged fail-closed privilege probe. Final independent re-review:
CONFIRMED-SOUND with no remaining HIGH or MEDIUM finding. The full control-plane
all-target/all-feature suite, including every live PostgreSQL proof, workspace all-target/all-feature
check, warnings-denied clippy, production migration checksum audit, architecture lint, erosion
budgets, contract coverage, production-source activation guard, and diff check are green.

**Dormant full runner-host composition (2026-07-23).** The production root now retains one complete
but unstarted runner host: the queued-run starter, bounded active-workflow poller, accounted
cancel-superseded coordinator, and a real `CiRunnerLoop`. The loop receives the constrained regional
claim capability, tenant-scoped heartbeat/complete store, durable launch-template resolver, exact
claim-generation Identity minter, exact-tenant accounted reporter, scoped reservation/final-launch
hooks, production lease TTL, gVisor backend path, and durable PostgreSQL/S3 log backings. There is no
compatibility resolver, synthetic tenant, permissive token signer, unaccounted reporter, spawn, or
runner task.

The production launch policy and runner now share one `LINUX_SMALL_V1_RUNNER_LABELS` authority.
Adversarial review found the initial dormant composition advertised only `linux`, while every
production job required both `linux` and `linux-small-v1`; PostgreSQL's subset predicate therefore
made all real jobs unclaimable. The duplicated literals were replaced by the shared policy-owned
constant, and the production-source guard requires both manifest emission and runner advertisement
to consume it. Final independent re-review reported CONFIRMED-SOUND with no remaining HIGH or
MEDIUM finding. The full control-plane all-target/all-feature suite and the pre-database
activation-refusal/source guards, workspace all-target/all-feature check, warnings-denied clippy,
architecture lint, erosion budgets, contract coverage, and diff check are green.

**Canonical PR supersession authority (2026-07-23).** The validated PR event's repository and
positive number now derive one bounded canonical `pr:{repo}:{number}` identity. A forward-only
`ci_0001c` migration persists it as immutable `ci_run` authority without rewriting the shipped
create; both CI mains apply the same migration. New pull-request rows require the identity,
non-pull-request rows forbid it, exact redelivery verifies it, and legacy PR rows with no identity
remain readable but fail closed before operational capacity reservation. The production
`LinuxSmallV1LaunchAuthority` copies the persisted identity onto every immutable manifest job; push
jobs continue to carry none.

The live production-consumer proof co-commits a namespaced PR identity with the run and dedup mark.
The app-role PostgreSQL proof covers round-trip, exact replay, divergent collision, database
constraints, and RLS; the rolling-upgrade proof observes the column absent before applying the new
full set and present afterward. An initial per-job co-persist/cancel prototype was removed before
commit after adversarial review showed that job-local atomicity could not establish newest-head
ordering or finalize undispatched reservations and Flow. Supersession therefore remains explicitly
at the durable run-start boundary rather than being conventionally inferred during job dispatch.
Independent review first found the upgrade fixture accidentally included `ci_0001c` in its old
schema; after making the absent→present transition load-bearing, re-review reported
CONFIRMED-SOUND with no remaining HIGH or MEDIUM finding. Both CI crates' complete
all-target/all-feature suites (including live PostgreSQL and `runsc`), workspace check,
warnings-denied clippy, migration upgrade, production-source guard, architecture lint, erosion
budgets, contract coverage, and diff check are green.

**Producer-authored PR head ordering (2026-07-23).** Durable newest-head authority now comes from
Git's row-locked `git_pr.version`, read and emitted in the same PostgreSQL transaction as each PR
head-trigger event; timestamps, ULIDs, and consumer receipt order have no role. `git.pr.opened` and
`git.pr.synchronized` advance to schema v2 in Git's registered token lineage and require a positive
`head_generation`. CI installs the forward-only v1→v2 chain before handling: a legacy opened event
is deterministically rewritten to generation 1 (including an unknown conflicting v1 key), while a
legacy synchronized event without a generation is rejected rather than assigned invented ordering.
A current v2 omission, zero, negative, string, or fractional generation is permanent poison before
Git reads, capacity reservation, or run persistence.

Forward-only `ci_0001d` adds nullable `ci_run.pr_head_generation` without rewriting any applied
migration. New dispatchers require and replay-verify a positive value on every pull-request run and
forbid it on every other trigger; the database enforces the same boundary. `NULL` exists only for
rows written by the immediately preceding dispatcher during a mixed-version rollout: launch remains
available, but the run-supersession transaction must treat it as legacy-oldest, must never let it
cancel a positive generation, and must not invent an order between two legacy rows. The producer
wire adapter was extracted from the oversized PR store, bringing that module below the 3,000-line
soft ceiling and removing its shrink-only exception. Mechanical payoff: accepted current PR
head-trigger events lacking positive producer authority = 0; v1 opened variants yielding anything
other than generation 1 = 0; module-budget exceptions = 2→1. Live Git outbox, dispatch co-commit,
run-store/RLS, and absent→present migration proofs pass. Independent adversarial review twice
reported CONFIRMED-SOUND with no HIGH or MEDIUM finding, including after the extraction. This is the
ordering prerequisite only; serialized run cancellation, Flow finalization, and reservation
settlement remain the next coherent R4.2 row.

**Serialized PR run supersession (2026-07-23).** Producer insertion and every starter/canceller now
take one exact `(tenant, region, canonical PR group)` transaction lock. Every retained positive
generation, including terminal history, remains durable high-water authority: a delayed strictly
lower or rolling-upgrade `NULL` run is cancelled before CAS lookup or launch materialization, equal
positive generations remain unordered, and two legacy rows still do not invent an order. A positive
replacement becomes fully materialized before it cancels strictly older active peers in the same
transaction; any replacement or cancellation failure rolls back both sides.

Running-run cancellation now has one production owner at the run-start boundary. It terminates Flow,
closes queued/leased and never-dispatched manifest jobs, zero-settles their exact Storage reservations,
persists the CI cost projection and immutable skipped receipts, transitions `ci_run`, and co-commits
`ci.run.cancelled` plus cancelled per-context checks. A launched job remains the reporter's settlement
owner; its late exact report is a terminal Flow no-op, records actual usage once, and flips the
cancelled run/check set to settled only when the complete manifest receipt set exists. Existing
receipts are accepted only after immutable identity, queue disposition, deterministic completion
receipt, re-priced policy, Storage's ordered settlement log, billed/refunded outcome, and the exact CI
projection all agree. The obsolete job-local production cancellation coordinator is test-support
only.

The concurrency boundary is mechanical rather than conventional. Production manifest dispatch locks
and requires the exact live Flow row before co-persisting queue+spec, using the same
Flow→queue→accounting/reservation→`ci_run` order as cancellation and reporting. Thus dispatch-first is
observed and closed, while cancellation-first makes a stale body write zero queue/spec rows. Flow and
product terminal CAS outcomes must agree; a finalizer/canceller split rolls the replacement and Flow
termination back. Live PostgreSQL proofs cover terminal high-water, producer-lock blocking,
dispatch/cancel and final-launch/cancel outcomes, completed-before-cancel accounting, forged monetary
projection and skipped receipts, finalizer-winning rollback, launch-winning late report, exact replay,
legacy ordering, rollback coupling, RLS, and bounded polling.

The independent verifier initially returned HOLD with four HIGH findings (non-durable high-water,
producer insertion outside serialization, unfenced Flow dispatch, and split Flow/product
finalization) plus one MED monetary-verification finding. After each was made load-bearing and the
unrepresentative dispatch-lock test was replaced with the real production method, final re-review
reported **CONFIRMED-SOUND with no HIGH or MEDIUM finding**. Complete Flow, Controlplane, and Dispatch
all-target/all-feature suites (including live PostgreSQL, `runsc`, RustFS, migrations, production
source guards, and recovery drills), workspace check, warnings-denied clippy, architecture lint,
erosion budgets, contract coverage, and diff check are green. Mechanical payoff: accepted delayed
lower/legacy starts after a retained positive generation = 0; stale production dispatch rows written
after committed cancellation = 0; undispatched reservations left open after supersession = 0;
cancelled runs with complete receipts but `cost_settled=false` = 0; production supersession owners =
2→1.

**Coordinated dormant runner-host lifecycle (2026-07-23).** One `CiRunnerHost` now owns the concrete
queued-run starter, exact-tenant Flow recovery poller, and sandbox runner loop. Start is ordered and
fallible, one watch signal stops every lane, and shutdown retains and joins both Tokio tasks and the
runner OS thread. Intake checks the signal before every queued-run start, every exact workflow scope,
and every durable Flow drive rather than only between large batches. An already-running sandbox is
allowed to drain, but no subsequent job is claimed. Settlement-owner mismatch and terminal-report
failure are host failures: they stop peer intake and surface to the service instead of backing off
and claiming more work.

The drain deadline is the maximum admitted job timeout plus 60 seconds. Expiry publishes a typed
`DrainTimedOut` failure while the supervisor keeps ownership of every join. Production has two
observers: the ordinary lifecycle observer stops the service on the first host failure, while an
independent deadline observer survives service teardown and terminates the process if a stuck runner
crosses the deadline. Clean completion closes the deadline watch and joins that observer. The reaper
remains separately owned but consumes the same service shutdown signal.

The live PostgreSQL proof pauses the first run inside the real launch-authority transaction, signals
shutdown, releases the transaction, and observes `[running, queued]` with a batch limit of 64. Seven
host tests prove single-signal joining, peer-stop on lane failure, settlement-owner refusal,
terminal-report fail-stop, in-flight drain, no-lane invalid-config refusal, and retained join
ownership across a simulated service-receiver drop and drain timeout. The production-source guard
proves the two-observer topology while preserving pre-database activation refusal. Independent
adversarial review first returned HOLD because shutdown was checked only between batches, timeout
detached join ownership, and terminal-report failure continued intake. After those fixes, re-review
found one remaining HIGH: service teardown could drop the only failure receiver and then hang
awaiting the retained joins. The independent deadline observer and same-topology regression closed
it; final review reported **CONFIRMED-SOUND with no remaining HIGH or MEDIUM lifecycle finding**.
The complete Flow, Controlplane, and Dispatch all-target/all-feature suites (including live
PostgreSQL and `runsc`), workspace all-target/all-feature check, warnings-denied clippy,
architecture lint, erosion budgets, contract coverage, production-source guard, and diff check are
green. Mechanical payoff: newly started runs after shutdown = 0 beyond the one already inside its
authority transaction; new workflow drives after shutdown = 0 beyond the one already in flight;
detached host tasks/threads at deadline = 0; runner intake continuing after terminal-report failure
= 0; production runner-host lifecycle owners = 3→1.

**Fenced durable sandbox launch and crash recovery (2026-07-23).** Production Firecracker and
gVisor launches now spawn a trusted wrapper that is mechanically blocked before runtime exec. Only
after the exact `leased` → `running` CAS commits does the parent validate its retained PostgreSQL
session-lock ownership, write the gate byte while that ownership is still held, and release the
lock. No transaction, row lock, or pooled connection remains owned during guest execution. The
successful CAS installs a server-owned execution lease equal to the maximum admitted runtime plus
600 seconds; callers cannot select that recovery window. The reaper still recovers expired
pre-launch leases immediately, but an expired running generation is requeued only after it acquires
the exact generation's advisory lock. A paused live post-CAS continuation therefore cannot race a
replacement, while process or connection death releases ownership and makes the row recoverable.
Dirty pooled sessions carrying any advisory lock are refused before CAS because PostgreSQL session
locks are re-entrant.

A native watchdog forks before wrapper exec, closes every inherited descriptor except its exact
liveness, readiness, timer, and optional cgroup-kill capabilities, and moves into a process group
separate from the runtime. Runner death or liveness loss kills the complete runtime process group;
gVisor also inherits an exact `cgroup.kill` descriptor so sentry/gofer-shaped descendants that call
`setsid` cannot survive. Its independent `CLOCK_BOOTTIME` timer enforces the admitted runtime
deadline even when the runner thread or process is stopped. It accounts host-suspend time and acts
on resume; it does not claim to run while the whole host is suspended.

The live PostgreSQL production proof now injects both required crash windows. A credential minted
without a launch CAS expires, reaps, remints under a new scheduler generation, and the stale
generation cannot reserve or launch. A committed CAS retains visible `running` state without a
long transaction, blocks reaping while its session owner lives, reaps after simulated
process/connection death, refuses the stale generation, and executes the immutable command through
the real production gVisor backend under the fresh generation. The recovery creates exactly one
reservation and leaves terminal settlement with the reporter. Process-level regressions SIGKILL
and SIGSTOP a disposable runner and prove no delayed runtime effect; a real delegated-cgroup test
proves an out-of-process-group descendant's exact cgroup membership and absence after the ordinary
sandbox kill path.

Independent review first returned HOLD because the initial design retained a database transaction
through guest execution, then because the watchdog shared the runtime process group and the
cgroup-descendant proof was not load-bearing. After the transaction was replaced with the
commit-to-gate session fence, the watchdog moved to its own group, and the test asserted a real
detached descendant and exact cgroup membership, final review reported **CONFIRMED-SOUND for the
scoped process-crash and runner-pause boundary**. The complete Controlplane and Sandbox
all-target/all-feature suites (including live PostgreSQL, real Firecracker, real gVisor, and escape
drills), complete lint suite, workspace all-target/all-feature check, warnings-denied clippy,
architecture lint, erosion budgets, contract coverage, production-source guard, and diff check are
green. Mechanical payoff: runtimes execing before a committed exact launch CAS = 0; replacement
generations admitted while a live post-CAS owner is paused = 0; runtime process groups surviving
runner death or watchdog deadline = 0; gVisor descendants surviving outside the runtime process
group = 0; caller-selected execution-lease duration = 0.

Two bounded residuals remain explicit. If PostgreSQL applies COMMIT but its acknowledgement is lost,
execution remains refused and no duplicate is admitted, but recovery can wait for the fixed 6h10m
lease; this is a MED availability/recovery-latency residual, not a fencing-correctness failure. A
whole-host suspension spanning both watchdog deadline and remote lease expiry leaves resume
scheduling unordered; because the guest has no network or durable writable surface and its stale
terminal CAS is refused, the impact is brief stale contained compute, classified LOW and outside
this process-crash row.

**Complete running-root retry, recovery, and settlement (2026-07-23).** Terminal completion now
requires the exact scheduler generation to be `running`; possession of a valid leased generation is
not evidence that work executed. The live PostgreSQL proof begins with the production region claim,
locked Identity token mint, operational reservation begin, and exact launch CAS. It first proves a
leased completion has zero queue, signal, accounting, projection, or Storage effects. After launch,
an injected `ci_job_accounting` trigger faults only after Flow signalling, queue consumption,
Storage settlement, and the CI cost projection have all run in the same transaction. Every write
rolls back: the queue remains running, the reservation remains inflight, and no signal, completion
receipt, CI accounting/projection, or Storage cost event survives.

The failed running generation is then expired and recovered through the advisory-lock-aware
production reaper. Its stale completion is refused with zero effects. A second production scheduler
generation resolves through the manifest-bound Identity issuer, crosses the fenced launch
authorizer, executes immutable `/bin/false` in real gVisor through `RunnerAgent`, and reports through
the exact-tenant production reporter. The measured failure usage settles once; an exact
acknowledgement-loss replay is idempotent, while divergent usage is refused. The finalizer-to-Flow
crash window is replayed by the production poller and must leave the workflow exactly `completed`,
not merely outside its active states.

Historical culmination tests were made mechanically honest too. Their real gVisor launch now
defers the production exact-generation CAS until the child launch guard is armed. The claim-bound
test explicitly refuses the correct claim while leased, then attacks nonce, epoch, owner, verdict,
and typed-reference predicates only after the row is running. Independent review found and held on
the first ordering because the leased-state predicate masked three attacks; after reordering them
behind launch, final review reported **CONFIRMED-SOUND with no remaining HIGH or MEDIUM finding**.
The complete Controlplane all-target/all-feature suite (438 unit tests plus live PostgreSQL, real
gVisor, scheduler least-privilege, cancellation, recovery, and accounting integrations), workspace
all-target/all-feature check, warnings-denied clippy, architecture lint and its fixture matrix,
erosion budgets, contract coverage, production activation guard, rustfmt, and diff check are green.
Mechanical payoff: accepted completions before launch = 0; partial terminal commits after a late
accounting fault = 0; stale generation settlements after reaping = 0; duplicate terminal
signals/accounting/Storage events on exact retry = 0; recovered finalizer workflows left active = 0.

**Opt-in production runner activation (2026-07-23).** The former blanket refusal is removed only for
the exact `MYELIN_CI_RUNNER=1` value; unset / `0` remains dormant and malformed or non-UTF-8 values
still fail before database access. Activation now preflights an explicit absolute `MYELIN_RUNSC_BIN`
and `MYELIN_GVISOR_ROOTFS` before PostgreSQL bootstrap. That preflight identifies the runtime, checks
the immutable rootfs executables, and executes `/bin/false` as the non-root guest through the same
rootless runsc OCI staging, no-network posture, and delegated cgroup-v2 placement used by production.
The dogfood command exports the exact executor paths and starts the coordinated CI host in a
foreground-owned process.

Signal handling is armed and consumed into one process-wide shutdown latch before preflight or
bootstrap. The reaper, queued-run starter, exact-tenant Flow recovery poller, and sandbox runner all
subscribe to that same current state, so a termination received during bootstrap is visible before
their first intake instruction. The production-process proof seeds a claimable trusted queued row,
waits only for the early handler-armed marker, sends SIGTERM before the runner-host announcement, and
requires a clean exit with no runner start and the row still queued and unowned. A separate local boot
reached the real runner announcement with the production split roles and executor, then drained
cleanly on SIGINT.

The complete Controlplane all-target/all-feature suite (439 unit tests plus live PostgreSQL, real
gVisor, scheduler least-privilege, cancellation, recovery, accounting, and production-process
integrations), the complete Sandbox all-target/all-feature suite (including the real gVisor and
Firecracker production/escape paths), workspace all-target/all-feature check, warnings-denied clippy,
architecture lint and fixture matrix, erosion budgets, frontend-aware contract coverage, targeted
rustfmt, shell syntax, and diff check are green. Independent adversarial review first held MED on a
boot-time signal that was installed but not consumed before host start; after the shared latch and
seeded pre-host-signal proof, final review reported **CONFIRMED-SOUND with no remaining HIGH or MEDIUM
finding**. Mechanical payoff: opt-in boots with an unavailable executor = 0; queued jobs claimed after
a bootstrap-time termination request = 0; runner-host lanes outside the coordinated shutdown owner =
0.

**Honest remaining activation floor:** run the exact opt-in coordinated runner through a live founder
Myelin push → CI → check dogfood/cutover pass. No Commercial wallet, billing, or Stripe work is
admitted before the Tier-B go decision. The runner is now locally boot-proven, but that is not
evidence that a real founder push produced and surfaced its check.

**Durable CI→Git check-projection closure (2026-07-23).** CI Dispatch now emits the initial queued
check through the same frozen `CheckStatus` builder as terminal reporting, including tenant,
repository, commit, context, required flag, canonical run/details refs, attempt, trust tier,
timestamps, summary, and unsettled-cost state. Every check for one repository commit routes on the
fixed-width broker-safe `check:v1-<BLAKE3>` aggregate; the human-facing subject remains the canonical
ArtifactRef. Git owns forward-only `git_0014`/`git_0015` projection migrations with FORCE RLS and the
full `(tenant, region, repo, commit, provider, context)` authority key. The production
`git-check-projection` service consumes the structured JetStream subject, validates exact
tenant/region/subject/aggregate provenance, and co-commits the shared `(consumer,event_id)` dedup mark
with monotonic attempt supersession, DLQ, and quarantine.

Production Edge check cards, PR list summaries, merge 409 responses, merge admission, and protected
push policy read Git's own projection rather than CI or legacy PR-record green arrays. Missing or
unavailable projection truth fails closed; successful-but-unsettled work remains blocked. Merge
admission and projection updates share a transaction-scoped advisory lock over the exact
tenant/region/repository/commit authority. Admission rereads projection rows while holding the locked
PR row and persists either the blocked terminal result or the admitted merge intent in that same
transaction, defining the durable “facts frozen at admission” boundary even when the required row is
absent.

Mechanical evidence: the CI producer-builder → durable Git consumer integration proves rollback
before a missing projection, lossless redelivery, duplicate absorption, stale-attempt refusal,
CI-only provider and same-tenant canonical run-reference provenance, forged-provenance DLQ,
constrained-app-role write, and tenant/region RLS isolation. The production Dispatch/NATS proof
drives two distinct triggers for the same commit through the real durable reserve path and asserts
queued attempts `1 → 2`; its second queued fact supersedes a seeded settled success and flips the
Git merge gate back to blocked before PipelineStarter can emit `in_progress`. The run row, authority
high-water allocation, immutable per-run/context attempt issuance, queued outbox rows, and
trigger-dedup mark now share one PostgreSQL commit. A reserve-A/reserve-B-before-either-starts proof
holds those attempts at `1`/`2` through PipelineStarter and proves an older terminal A cannot
supersede newer queued B. Reserve-crash injection rolls the run, both attempt records, outbox rows,
and dedup mark back together. Live Git proofs cover same-commit cross-repository isolation,
missing/failing/fork/unsettled gate postures, and a dishonest legacy-green PR array that still blocks
at production merge admission. Full relevant Git and Edge suites, all-target/all-feature
warnings-denied clippy, architecture lint, shrink-only erosion gate, frontend-aware contract
coverage, shell syntax, targeted rustfmt, and diff checks pass. `scripts/dogfood.sh git-checks`
starts the consumer and `verify-check` performs a read-only exact-head/check/gate verification;
backup verification now includes `check_status`.

Protected direct pushes now hold the projection consumer's exact check-admission lock while reading
the head facts, evaluating branch protection, promoting objects, and performing the ref mutation;
a concurrent check update is serialized after that admission boundary rather than racing between a
completed read transaction and ref CAS. Projection admission uses a separately bounded runtime lane,
so its callback can mutate the ref/outbox through the ordinary store lane even when each pool is
limited to one connection. The hashed aggregate is a hard, fail-closed routing cutover, not a
fabricated backfill: keep the old Edge as the verification front door; deploy the new
`git-check-projection`, Dispatch, and coordinated CI/controlplane; drain every old Dispatch/CI
producer; start all three new services; rerun and verify every protected head; and only after every
exact-OID verification passes deploy the projection-only Edge. Personal production has no
pre-cutover production facts; any future existing-cell upgrade must abort if that rerun cannot be
completed. Independent adversarial re-review of immutable attempt issuance, reserve rollback,
bounded admission-lane composition, canonical detail refs, and the cutover order reports
**CONFIRMED-SOUND with no remaining HIGH or MEDIUM finding**.

**Honest remaining R4.2 floors:** complete CT-005's live-log API/SSE, web, and CLI/MCP surfaces; start
the three CI services and production Edge against the founder cell; push a real Myelin commit; and
observe the exact head's required check settle green through the verifier and browser. Only after the
complete surface and founder pass may CT-007 remove GitHub Actions. No completed increment claims that
founder act, starts the four-week R4 exit clock, or spends GitHub Actions.

**CT-005a production read-API increment (2026-07-24; not the complete CT-005 surface).** Production
Edge now mounts authenticated `GET /v1/ci/runs` and `GET /v1/ci/runs/{run}` routes over CI's durable
`ci_run` / `ci_job` / `log_anchor` authority. The verified principal supplies tenant/region; run
visibility is inherited from the parent Git repository's exact Pull decision rather than a second CI
ACL. Lists intersect the bounded visible-repository set before querying and use an opaque,
tenant/region/filter/visibility-bound `(created_at,run_id)` keyset cursor authenticated by a
domain-separated, cell-seal-root-derived keyed MAC held in zeroizing memory; timestamps are
semantically validated before PostgreSQL. Detail authorization resolves only the canonical parent
repo, then materializes run/jobs/steps in one tenant-scoped repeatable-read snapshot bound to that
exact authorized `repo_ref`, and returns the same 404 for absent and denied runs. Edge applies the
complete CI schema before dropping migration authority and refuses runtime handoff unless the new
forward-only `ci_run_surface_repo_created` index is valid and ready.

The live PostgreSQL proof covers hidden-repository exclusion before pagination, an insert between
pages, visibility/filter cursor staleness, DAG/job/step-anchor detail, a parent mutation committed
between detail statements while the authorized repeatable-read snapshot stays stable, FORCE-RLS
tenant isolation, and index readiness. The cursor unit gate separately rejects retained-tag mutation,
recomputation under a non-cell key, and shape-valid impossible timestamps before database access. A
second live proof invokes the production handlers and action policy, proves that parent visibility is
conjoined before pagination, and proves denied and absent details have the identical 404 envelope.
Separate gates pin Git Pull inheritance, strict query grammar, production route/action registration,
split-credential migration ordering, and the shrink-only module budget. Mechanical payoff: offset
cursors accepted = 0; hidden-parent runs entering a list page = 0; stale/tampered cursors accepted = 0;
hidden details returned after parent movement = 0; runtime starts with the run-list index unready = 0.
This increment does **not** serve log bytes or live tail, does not add the CI web route, does not wire
CLI/MCP, does not claim the founder acceptance pass, and does not open CT-007.

**CT-005a bootstrap identity follow-on (2026-07-24; prior LOW closed).** The reusable split-credential
bootstrap now has an exact index-readiness probe over the schema-local relation, owning table,
ordinary table/index relation kinds, access method, PostgreSQL-rendered ordered keys (including
direction and non-default null order), partial predicate, and PostgreSQL's
valid/ready/live/not-`indcheckxmin` planner-usability state. Edge binds the CI run-list gate to `ci_run`,
`(tenant_id,region,repo_ref,created_at DESC,run_id DESC)`, and `repo_ref IS NOT NULL`; a same-name
relation with a wrong schema, relation kind, access method, table, key/order/null order, predicate,
or planner-usability state refuses runtime handoff. The live split-credential proof admits the exact shape
and rejects that physical near-miss matrix; the full live CI bootstrap applies the real migration
then verifies the same canonical spec Edge consumes. The live CI store proof independently pins the
migration's actual catalogue identity. The earlier name-only readiness caveat no longer blocks
completion of CT-005.

L3 bootstrap-hardening footprint: **10 files, +477 / -14 lines** for the reusable exact probe and live matrix.

L3 row footprint: **23 files, +2,257 / -48 lines** for the bounded production read surface and its proofs.

## R5–R6

| Phase | Status |
|---|---|
| R5 production ops | NOT STARTED |
| R6 graduation gate | NOT STARTED |

## Decision log

- 2026-07-16: **R4 opened** at HEAD `3ee0503`. R4.0 (founder auth bootstrap) inserted ahead of the
  planned R4.1–R4.4: the census showed no authentication path exists against the real edge, so the
  cutover is structurally impossible without it. Design decision: operator-trust mint (a `bootstrap`
  subcommand sharing the edge composition root — possession of DATABASE_URL + MYELIN_KMS_SEAL_KEY IS
  the mint capability), explicitly NOT a network mint endpoint; web login = paste-token verified
  against real `whoami`, gated on a new `token_login_enabled` edge flag; git-wire-only Basic→Bearer
  (password = token, GitHub-PAT shape) so vanilla `git push` works.
- 2026-07-16: **R3 opened** at HEAD `6fd7e3a`. Design-first per VISION §3: sketch pack
  (`design-planning/09-r3-sketches/`) gates all frontend items; R3.6 (a11y fixes to existing chrome)
  and R3.7b (flow budget leak, backend) run in parallel un-gated. Census confirmed the frontend is
  SolidStart (older design docs said React — code wins) and enumerated the backend endpoint gaps
  (recorded in the R3 section above).
- 2026-07-06: Ledger opened; R0 execution begins at HEAD `2f38fce`.
- 2026-07-06: **R0 complete** (7/7, HEAD `5f47dd8`). Builder/verifier/commit process throughout; R2.1a
  carried forward (wire the R0.2/R0.3 gates live). **Next: R1** — MR-009b W3b–W7 (ledger 13) with the
  review HIGHs folded into their waves (see the R1 fold-in table above). R1 exit = scanner
  `no-in-memory-durable-store` baseline 0; the pre-existing `DedupLedger::new` integration breakage is an
  R1 item. R3 (Git/PR UX) can run in parallel with R1/R2 when opened.
