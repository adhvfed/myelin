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
| R2.1 | Object-level authz at the edge, platform-wide (extends R0.3 seam; git_edge template first) | **DONE** | `083831d` (merge; `fc06f54`) |
| R2.5 | Real OIDC login at edge; dev-login structurally dead in prod | **DONE** | `fb7fd1e` (merge; `a9e4e57`) |
| R2.6 | AllowAll removed from main.rs + lint | **DONE** | `3b1fda0` (merge; `e149dee`) + `75223a0` (followup: object-seam default fail-closed → scanner TRUE ZERO) |
| R2.7 | Search vector-path ACL parity | **DONE** | `f31e310` (merge; `6c56c42`) |

R2 exit: red-team campaign (subagent per subsystem: edge/wire/MCP/SSE/search reach-around) all-denied; AllowAll gone.

### R2 execution log (2026-07-15, session 2)

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
  `MyelinConfig` gains `Option<OidcSettings>` (issuer/audience + static JWKS, no jwks_uri fetch — tracked);
  opt-in (absent→refuse-not-mock/boot-ok, partial→fail-loud); NO fail-open. Part B: frontend `loginDev`
  build-time dead in prod (verified the dev token is ABSENT from the `.output` deployable). Self-reviewed
  (fail-open surface clean; crypto pre-audited). Residual: jwks_uri fetch/rotation; SAML/SCIM/passkey/SSH
  stay refuse-not-mock.
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

**R2 EXIT — red-team campaign IN PROGRESS.** 5 adversaries (edge JSON API, git wire, MCP, SSE, search) each
hunting an intra-tenant object reach-around; the edge+wire adversaries tasked to prove/refute #13
(report_checks CI-forgery → protected-branch bypass) end-to-end. On return: fix all confirmed reach-arounds,
re-verify, then R2 exits (all-denied + AllowAll gone — already true). **Tracked/deferred:** R2.4-fu (#12,
MCP GovernedRouter prod wiring — product-surface, MCP execution not live); #13 report_checks CI-producer
relation (likely fixed as part of the exit).

**Sequencing decisions:** R2.2 (object-qualification of `check`/EventMatcher/SSE) must land AFTER R2.1a —
it changes tuple-store object keying while R2.1a writes the first production grant tuples; git's
type-prefixed refs (`repo:core`) survive today's reduction, so R2.1a builds on the current grammar and
R2.2 canonicalizes against the then-live wire tests. R2.4 (MCP HITL) deferred until after R2.1a lands to
avoid myelin-agent-service overlap; fix shape settled (persist HitlGate in the already-declared
`agent_hitl_gate` table, MCP re-drive presents a server-issued gate-id looked up server-side, step-6
gate + batch approval key per-effect not by bare tool name).

## R3–R6

Tracked here at phase granularity once entered; R3 rows will be added when its design sketches start
(design-before-frontend, VISION §3 — R3.1–R3.7), R4–R6 when their phases open.

| Phase | Status |
|---|---|
| R3 Git/PR UX + first-run | NOT STARTED |
| R4 dogfood cutover (Tier D) | NOT STARTED |
| R5 production ops | NOT STARTED |
| R6 graduation gate | NOT STARTED |

## Decision log

- 2026-07-06: Ledger opened; R0 execution begins at HEAD `2f38fce`.
- 2026-07-06: **R0 complete** (7/7, HEAD `5f47dd8`). Builder/verifier/commit process throughout; R2.1a
  carried forward (wire the R0.2/R0.3 gates live). **Next: R1** — MR-009b W3b–W7 (ledger 13) with the
  review HIGHs folded into their waves (see the R1 fold-in table above). R1 exit = scanner
  `no-in-memory-durable-store` baseline 0; the pre-existing `DedupLedger::new` integration breakage is an
  R1 item. R3 (Git/PR UX) can run in parallel with R1/R2 when opened.
