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

**P-S12 minter floor — blast radius note (W3b.3 discovery):** the default `MonotonicMinter` resets
per store ⇒ two durable TupleStores mint colliding `event_id`s and `co_commit_in_tx`'s
`ON CONFLICT (event_id) DO NOTHING` silently DROPS the later event. Pre-existing named floor (the
production ULID source), but the W3b track makes the shared PG outbox the real emit path — the
ULID/unique-minter fix should land before or with W3b.4 (composition roots).

**W3b.2 DONE (2026-07-15, merge `20fe480`): scanner-neutral.** `PgOutboxBacking` over the frozen
outbox table; commit_staged_atomic (dup-event_id rejects whole commit = memory parity; one seq
discipline with co_commit_in_tx); drain_once_dead_letter (claim locks held across publishes; per-row
attempts/dead-letter; failed publish no longer aborts the pass); DrainReport.drain_errors surfaces
backing failure (W3b.1 debt resolved). Verifier CONFIRMED-SOUND (concurrent-drain no-double-publish,
crash-window 0-ghost/0-lost, EB-03 32-committer gap-free — all probed live); should-fix applied:
committed_rows/dead_rows ORDER BY (aggregate,seq) — event_id mint order provably ≠ commit order.
Residuals: no durable GC verb; NATS-bridge nested block_in_place unproven until W3b.4.

R1 exit: scanner `no-in-memory-durable-store` baseline **0**; unit tests DB-free; integration green; kill-9 drills.

## R2 — authz completion

| # | Item | Status | Commit(s) |
|---|---|---|---|
| R2.1 | Object-level authz at the edge, platform-wide (extends R0.3 seam; git_edge template first) | PENDING | — |
| R2.1a | **Wire R0.2/R0.3 LIVE** (carried from R0 verifier): production `main.rs` must (a) inject a real grant-backed `RepoAuthorizer` (not the `AllowAllRepos` default) and (b) `register_git_wire` in the production gateway. Until both, the R0.2 branch-protection gate and R0.3 per-repo authz are correct-but-latent. | PENDING | — |
| R2.2 | Identity `check()` fully-qualified object; EventMatcher + SSE scope likewise | PENDING | — |
| R2.3 | Fail-static authz cache full-key comparison | PENDING | — |
| R2.4 | MCP HITL server-side verdict; batch partial-approval by approval-id | PENDING | — |
| R2.5 | Real OIDC login at edge; dev-login structurally dead in prod | PENDING | — |
| R2.6 | AllowAll removed from main.rs + lint | PENDING | — |
| R2.7 | Search vector-path ACL parity | PENDING | — |

R2 exit: red-team campaign (subagent per subsystem: edge/wire/MCP/SSE/search reach-around) all-denied; AllowAll gone.

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
