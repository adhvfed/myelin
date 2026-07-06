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

| Fold-in | Into wave | Status |
|---|---|---|
| Erasure ledger records COMPLETION time + restore-inside-window resurrection test | W6b | PENDING |
| git DSR receipts: real `holders_hit` reconciled against data-map | W6b/W6 | PENDING |
| SQL-interpolation shapes killed while touching crates (rls predicate_sql, block_tree, TRUNCATE classifier) | W6 | PENDING |
| KMS ~90-site classification by independent verifier; boot fails LOUD | W5 | PENDING |
| Region-scope sweep (identity PG `scope.region()`, fr-par partitions parameterized) | W7 | PENDING |

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
