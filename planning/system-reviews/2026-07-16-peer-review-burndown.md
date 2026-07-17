# R4.4 finding-burndown — peer adversarial review (2026-07-16)

Source: an independent Fable session (on Adrian's Mac) ran **three adversarial review agents** over the
R3–R4 commit range `787665e..HEAD` with instructions to falsify commit-message claims. Full report
(with the "claims that HELD" list — do NOT re-verify those) is at `~/myelin-review-findings-2026-07-16.md`
on this host. 21 verified findings below, triaged. This IS the R4.4 finding-burndown loop (lives here
until Myelin's own issue tracker stands up).

**Process note the reviewers flagged (taken):** the gaps cluster where a headline/ledger line says
DONE/live/exactly-once while the honest caveat lives only in a code comment (findings 6/7/9 are the
poster children). Fix forward: promote floor-confessions into the commit headline + ledger line itself
so the ledger alone never overstates. See the ledger correction committed alongside this file.

## Status legend
DONE · IN-PROGRESS · TODO(pri) · WONTFIX(reason) · POLICY(needs founder call)

| # | Sev | Area | Finding (file:line) | Status |
|---|-----|------|---------------------|--------|
| 1 | MED-HIGH sec | git | merge-gate `counting_approvals` (pr_store.rs) not per-reviewer-deduped nor agent-excluded → a lone reviewer or an agent satisfies required_approvals≥2 | **DONE `fe966e8`** (dedup by reviewer + `!is_agent` + regression test) |
| 7 | HIGH | events | dedup mark commits BEFORE handler effect in a SEPARATE tx (consumer.rs:546) → kill-9 between → redelivery deduped → push arms nothing = AT-MOST-ONCE despite exactly-once claims (MR-023b floor named in dedup.rs:92-100) | **DONE `f0d3f8f` — mechanism (0886f20) + all 3 supplementary-verifier HOLDs fixed + INDEPENDENT re-pass CONFIRMED-SOUND (held the commit for the independent agent this time — banked lesson).** H1 livelock closed via commit_staged_absorb (ON CONFLICT DO NOTHING + payload-equality; every reserve-bundle field proven byte-identical across re-derivations); H2 panic-leak closed via native sqlx::Transaction rollback-on-drop + catch_unwind; H3 check-id seeds evt:<run_id>:<subject>. **NEW MED (fix-forward, BEFORE CT-004d.2 goes live — see #7b below):** the H2 panic path acks but its replayable DLQ is a volatile in-memory Vec → panicked effect lost after redeploy; fix = durable DLQ or don't-ack-panic-DL.
| 7b | MED→before-live | events | (introduced by H2's fix) panic path acks the broker while its replayable dead-letter set is a volatile/unbounded/undrained in-memory Vec → a panicked handler's effect is lost after a redeploy | **DONE `15c3033`** (durable consumer_dead_letter table + DurableDeadLetter sink mirroring DurableDedup; H2 panic path persists on a fresh conn, survives restart; orchestrator closed the PII residual — panic detail to the loud log, PII-free constant in the durable table). LOWs recorded (soft read, InMemory-default not test-gated). |
| 6 | HIGH claim | ci-dispatch | prod `main.rs:133` `consumers = Vec::new()` — the "live" trigger consumer is NOT registered in prod (CAS BlobStore backing integration-gated); 381a0e4 headline + ledger overstated | **DONE** — the "CAS is integration-gated" premise was STALE (aws-sdk-s3 is NON-OPTIONAL via myelin-storage since MR-009b W1; `cargo tree -i aws-sdk-s3` confirms it in the default graph). `main.rs` now calls new testable `build_dispatch_consumers` which assembles the 4 real backings (CoCommitReserveStore durable ci_run co-commit + S3BlobStore CAS + DurableGitConfigReader over MYELIN_GIT_ROOT + durable DedupLedger) and registers 1 bound consumer, fail-loud. Boot diagnostic surfaces the git-root (review LOW). Verified: default+integration build clean, clippy clean, 62 unit + 6/6 live-PG integration (new finding6 registration test + proof-4 co-commit) green, independent adversarial review found no defect. Cross-service NATS delivery remains the named deploy floor. |
| 10 | MED-HIGH (once live) | ci-dispatch | `resolve.rs:297-314` `expand_matrix` materializes the full axis cross-product UNBOUNDED; no cap on config size/jobs/axes/values → a tenant OOMs the dispatch consumer on one push | **DONE `f6ccb3c`** (saturating instance-count cap `MAX_TOTAL_MATRIX_INSTANCES=1024`, refused fail-closed BEFORE materialization; test: 10^8 → MatrixTooLarge, 3×4 still resolves). Follow-on P3: also cap raw config bytes + job count at parse. |
| 2 | MED | git | JSON durable stores do read-modify-write with NO isolation (pr_threads.rs, pr_store.rs `put`, `next_pr_number` git_durable.rs:880) — concurrent writers clobber (lost comment; PR-number collision) | **TODO(P2)** — the PG-home migration (GT-003b) is the real fix; interim: a per-repo write lock or CAS. Doc conflates write-atomic with rmw-isolated. |
| 9 | MED | ci-dispatch | `integration_ci_ct004b_trigger_consumer.rs:188` derives event ids deterministically so "redelivery adds no dup events" passes — but prod `OutboxReserveStore` mints FRESH ULIDs → prod WOULD duplicate. Test double stronger than prod; no restart leg. | **TODO(P2)** — make the test mirror prod ULID minting (or fix #8 so it's moot) + add a reopen leg. |
| 8 | MED | ci-dispatch | duplicate run-started/check-updated events under documented dedup fail-open (fresh ULID per emit) — ci_run protected by deterministic run_id, co-emitted events are not | **DONE `0886f20`** (folded into #7: `OutboxReserveStore::persist` derives ci.run.started + each ci.check.updated id via deterministic_uuid + emit_with_id → outbox ON CONFLICT dedups all bundle members; no fresh-ULID escapee). |
| 15 | MED | web | PR diff "Load remaining files" never reads `search.cursor` (prs/[n]/diff.tsx:60) → >50-file PRs can't page | **DONE `02c01a0`** (root-caused + real-browser e2e). |
| 16 | MED | web | split-view commenting on old-side (deleted) lines silently no-ops (DiffViewer.tsx:490; composer side check diff.tsx:243) | **DONE `02c01a0`** (root-caused + real-browser e2e). |
| 18 | MED-LOW | web | in-progress review batches orphan on reload (prs/[n]/index.tsx:350) — draft in local signal, never rehydrated from threads().reviews → "Start a review" double-creates | **DONE `02c01a0`** (root-caused + real-browser e2e). |
| 3 | LOW-MED | git | `DurablePrStore::list` silently skips unparseable records (pr_store.rs:620) → number REUSE via next_pr_number → live PR overwritten | **DONE `64c434b`** (filename-authoritative `max_pr_number` for allocation; list() stays a tolerant view; test: corrupt #3.json still bumps to #4). |
| 5 | LOW | edge | absent-repo 404 leaks the on-disk rootfs path (durable.rs:1842) for granted-but-missing repos — layout leak, not existence oracle | **DONE `e1db444`** (NotFound names the logical slug, not path.display()). |
| 11 | LOW-MED | storage | stores migrated BETWEEN the colliding cost_event migrations (pre-CT-004m) have no repair path — money ledger to a wrong-shaped table | **TODO(P3)** — boot-time column-shape assertion on cost_event (dev-only ~7h window, accepted risk). |
| 12 | LOW | ci-dispatch | `CI_TRIGGER_SUBJECTS` contains an EMPTY SubjectPattern (consumer.rs:109) under a "not empty" doc → a router iterating subjects() treats it as match-all | **DONE `93862eb`** (`ci_trigger_subjects()` OnceLock → bounded `myelin://`; test now covers the SubjectPattern form, not just _STRS). |
| 13 | LOW | ci-controlplane | cost_id FNV-1a + ON CONFLICT DO NOTHING, no conflict verification (cost_store.rs:313) → a re-delivered settle with different amounts silently drops a unit | **TODO(P3)** — verify-on-conflict or a stronger key; it keys the billing table. |
| 17 | LOW-MED latent | web | expand-context is an unwired pipeline (DiffViewer.tsx:51/68/177; getFileLines 0 call sites) | **TODO(P3)** — wire onExpandContext→buildRows or remove until CT wires it. |
| 19 | LOW-MED | web | uncontrolled composers desync after posts (prs/[n]/index.tsx:442/539/578: onInput, no value bind) → duplicate post | **DONE `02c01a0`** (root-caused + real-browser e2e). |
| 4 | LOW | edge | duplicate unreachable `MergeAttempt::RefRefused` match arm (git_durable.rs:3257); myelin-edge CI doesn't deny warnings | **DONE `a5a38c6`** (duplicate arm removed). Second half — `-D warnings` in CI for myelin-edge — deferred to the CI track (no CI yet). |
| 20 | LOW | web | dev-contract.mjs:626 cross-repo `repo:"myelin/myelin"` but real edge emits the bare slug → cross-repo rows 404 vs harness, invisible in e2e | **TODO(P3)** — fixtures-mirror-contract check. |
| 21a | LOW | web | cross-repo buckets: no pager, count chip shows page size not total (silent truncation) | **TODO(P3)** |
| 21b | LOW | web | prs/index.tsx:86 renders raw err.message into a hidden DOM text node (no XSS, but against the range's rule) | **TODO(P3)** |
| 21c | LOW→pre-external | web | login form actions lack origin/CSRF checks (sameSite=lax doesn't stop cross-site POST logging the victim in as attacker) — fine for single-founder dogfood, **fix before external users** | **DONE** — `sameOriginVerdict` (pure, `csrf.ts`) + `assertSameOrigin` guard on the state-changing actions (`loginDev`/`loginWithToken`/`logout`); reject → `/login?error=csrf`. Fail-closed on absent/opaque origin. Verified: 5 unit + typecheck + lint + 8 login e2e (incl. real end-to-end token login, no fail-closed regression) |
| 21d | minor a11y | web | review verdict panel (prs/[n]/index.tsx:458) ad-hoc role="dialog", no focus move/trap/Esc, unlike the DS Dialog | **TODO(P3)** — use the DS Dialog. |
| 14 | POLICY | flow | R3.7b residuals: crash-after-settle-then-rerun-fails → billed-success/surfaced-failure divergence; failure-bills-zero = deliberate refund leak (always-failing tenant never billed) | **POLICY** — founder call on the billing policy; recorded. Core R3.7b fix HELD under re-attack. |

## Finding #7 design analysis (grounded 2026-07-16 — for the dedicated chunk)

Current flow (`consumer.rs` ~540): `mark_handled` (INSERT..ON CONFLICT, COMMITS immediately) → run
`handler.handle` → on `Retry` REVERT the mark, on `Done` ack. The dedup mark and the handler's effect
(e.g. the ci-dispatch reserve bundle, committed later in its own tx) are SEPARATE transactions. Kill-9
between the mark-commit and the effect-commit → redelivery sees the mark → `Deduplicated` → ack → the
effect is LOST forever. This is the MR-023b floor `dedup.rs:92-100` explicitly names ("the in-the-SAME-
transaction-as-the-handler's-state-write co-commit requires the consumer runtime to thread a transaction
INTO the handler — a runtime change beyond this seam"). Fail-direction on DB-unreachable is already
correct (report FRESH → at-least-once, never lost).

Two fixes:
- **Option A (the named MR-023b fix): thread a tx into `handle`** so the dedup INSERT + the handler's
  state writes co-commit. CORRECT exactly-once-with-effect. Cost: the `EventHandler::handle` contract
  gains a tx/connection param; every handler runs its writes on it. Invasive (all consumers), touches
  the frozen consumer contract — architecturally weighty; the "right" fix.
- **Option B (mark-AFTER-effect + idempotent effect): move `mark_handled` to AFTER `Done`.** Then a
  kill-9 between effect-commit and mark-commit → redelivery RE-RUNS the handler → the effect must be
  IDEMPOTENT. Less framework churn, but pushes idempotency onto every handler + REQUIRES fixing #8
  (the co-emitted `ci.run.started`/`ci.check.updated` mint FRESH ULIDs → would duplicate on re-run;
  need deterministic ids). The CI `ci_run` row is already idempotent (deterministic run_id), so with #8
  fixed, Option B makes the CI path effectively-once.

**Recommendation:** Option A is the correct, contract-honest fix the codebase already names. Scope it as
its own chunk with an adversarial verifier on the kill-9 co-commit invariant (a probe that crashes
between mark and effect and asserts the redelivery re-runs). Do #8 (deterministic co-emitted event ids)
alongside either option — it's independently correct. Sequence BEFORE CT-004d.2 registers the consumer
in prod (so the exactly-once claim is true when it goes live).

## Codex peer-review option (from the reviewer)
GPT 4.6 "Sol" via Codex CLI found real P1s in Fable-written fixes on ovim. NOT installed on this host.
To use: `npm i -g @openai/codex` + login, OR route through Adrian's wrapper at
`/Users/adrian/talk-to-codex/codex-session` (`start NAME`, `send NAME "msg"`). Pattern: give scope
(commit range) + intent + pointed questions; it verifies empirically and blocks on real things.
**Deferred — needs Adrian to install/enable.**

## Next burndown actions (in loop priority order)
1. #7 at-most-once dedup window (P1, framework) · #10 matrix/config resource caps (P1, DoS) · #6 finish
   the prod consumer registration (with CT-004d.2) · #21c CSRF before Tier-B.
2. P2 cluster: #2 rmw isolation, #12 empty subject, #15/#16/#18/#19 web PR-review UX, #3/#8/#9.
3. P3 polish + #14 policy call.
