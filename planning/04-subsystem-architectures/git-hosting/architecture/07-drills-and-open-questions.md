# 07 — Drills Owed & Open Questions

> PROVE-IT (T-1/T-4): a property does not exist until a failure-injection drill forces the failure and
> observability watches the system survive. Each failable property below names its **quantified gate** and
> the **survival signal** (the telemetry set, contract 1.8). Then the open questions for Phase 6. Updated for
> the X-1 check seam + the OQ-D anchor resolver. Date: 2026-06-19.

---

## 1. Drills owed (quantified)

| # | Drill | Property proven | Quantified gate | Survival signal |
|---|---|---|---|---|
| **D-1** | **Per-ref ordering at push QPS** (= Bus D-9). Burst force-pushes + rapid pushes to one hot ref (1×/10×/30×), parallel pushes across many refs. | `git.ref.updated` delivered in **push order per ref**; refs fan out in parallel; outbox order == ref-update order. | per-ref order preserved at target QPS; zero lost/ghost events; cross-ref throughput scales. | per-aggregate publish latency; outbox depth; consumer lag. |
| **D-2** | **Erasure-reaches-every-holder (git subject).** Erase a subject who authored commits/PRs/comments + uploaded LFS. | DSR fan-out hits: pseudonym map (Id), per-subject DEK (bodies, live+backups), reflogs/bitmaps/pack backups, search index (purge+reindex), refs (tombstone), cache/CDN (H9). **The residual is EXACTLY the platform-posture residual (contract 10.9) — nothing more.** | every holder hit; residual scoped to the ONE-posture residual; crypto-shred reaches backups. | DSR receipt set; erasure-ledger entry; reindex completion. |
| **D-3** | **Reindex-from-cold parity** (Search code index + Refs edges + the `check_status` projection). Wipe each; `replay`/`reindex` re-emits the source events. | the cold rebuild matches steady-state (one code path, no drift); the `check_status` projection rebuilds from CI's `ci.check.updated` re-emit. | byte-for-byte parity of rebuilt index vs. steady-state; no cross-DB read. | replay throughput; index doc count parity; supersession re-applied identically. |
| **D-4** | **Monorepo ceiling benchmark.** Grow a synthetic monorepo (file count, history depth, push QPS) until partial-clone/sparse/bitmaps degrade. | establishes the v1 monorepo ceiling (GF-4) — measured not guessed. | documented ceiling numbers; clone/fetch latency held below the ceiling. | clone/fetch p99; bitmap/commit-graph freshness; serving CPU/IO. |
| **D-5** | **Linearizable protected-ref merge / no split-brain.** Concurrent merges + a force-push to one protected `base_ref`; inject a DB-replica (ref-authority) failover + a serving-node recovery mid-merge. | protected-ref merges are linearizable on the DB ref CAS; no split-brain; no lost merge; recovery reconciles to the DB ref index. | zero conflicting tips; merge order linearized on the ref row; failover loses zero committed merges; `update_seq` monotonic + the fence honoured. | DB failover events; `update_seq` monotonicity; merge-queue state; recovery reconcile log. |
| **D-6** | **Clone-storm / hot-repo shed (protected human lane).** 30× agent/CI clone surge on a hot repo. | the human interactive lane holds; agent/CI lane sheds (`429 + Retry-After`); per-tenant fairness (the OQ-K budget floor). | human fetch p99 held; agent/CI shed as designed; zero cross-tenant starvation. | shed counts; per-tenant in-flight; bundle-URI/CDN hit rate. |
| **D-7** | **Diff-anchor correctness across rewrite (the OQ-D four states).** Force-push/rebase a PR with open inline threads. | anchors resolve to the correct state: **LIVE** (unchanged), **MOVED** (rebased, fingerprint found shifted), **OUTDATED** (partial), **GONE** (tombstone). Never silently wrong. | zero mis-anchored comments; each thread's resolved state matches the ground-truth diff; "view in original context" renders. | per-anchor state distribution; fingerprint-match rate; outdated/gone counts. |
| **D-8** | **Cross-tenant IDOR on the git wire.** Cross-tenant repo access via a token whose tenant ≠ the URL-path tenant. | tenant comes from the token, never the path (ID-3); zero cross-tenant read. | zero cross-tenant read; rejected at the front door. | authz deny counts; tenant-predicate lint (build-time). |
| **D-9** | **Outbox-at-push-throughput, emit-iff-committed.** Crash the serving tier mid-push (after policy, before/after commit). | a `git.ref.updated` is emitted **iff** the ref move committed (BUS-2); no ghost, no lost event. | exactly-once-effective per push; quarantine objects discarded on abort. | outbox depth; dedup ledger; relay claim rate. |
| **D-10** | **The X-1 check-seam correctness drill (NEW).** (a) deliver `ci.check.updated` out of order + duplicated for one `(commit_oid, context)`; (b) a fork PR greens its own check; (c) a maintainer endorses via `approve_untrusted_ci`; (d) wake the merge queue on a doubly-delivered `ci.result`. | **(a)** the `run_attempt`-monotonic supersession holds the correct current row, drops stale lower attempts; **(b)** the `untrusted_fork` success is **neutral for gating** (the merge is blocked); **(c)** endorsement (or re-run-trusted) flips the gate green; **(d)** the doubly-delivered `ci.result` wakes the workflow **exactly once** (idempotent on `idem_token`); no double-merge. | exactly one current row per key; fork cannot self-green; endorsement is a plain `check`; one wake per signal; zero double-merge. | check_status row churn; dropped-stale count; gate-state transitions; workflow signal dedup; merge count == 1. |
| **D-11** | **`list_objects` leak-free + fast at scale (NEW).** A viewer with partial repo/PR visibility lists a 100k-PR tenant. | the `SetExpr` `Filter` JOIN returns **only** visible rows (no leak), in **one query** (no N+1, no post-filter); a just-revoked grant is reflected (the zookie watermark). | zero invisible-row leak; one SQL query; revoke reflected within the zookie bound. | query count per list; rows-scanned vs. returned; authz reverse-index lag. |

D-1, D-3, D-6, D-8, D-11 are shared/inherited drills instantiated on git surfaces; D-2, D-4, D-5, D-7, D-9,
D-10 are git-specific. All feed the Phase-5 testing strategy (the named-drill set).

---

## 2. Open questions for Phase 6

| # | Question | Owner / resolver |
|---|---|---|
| **OQ-1** | **gitoxide server-side capability matrix** against the current release — which wire/maintenance ops can move off `ShellGitCore` to a `gix` server (gating any migration). v1 serves the wire with canonical `git`. | Git P6 spike |
| **OQ-2** | *(resolved by reconciliation — folded into the ONE platform erasure posture, contract 10.9 / recon §X-7; the residual Art. 17 ratification is R-7 `[OPEN — LEGAL]`.)* | Legal/DPO |
| **OQ-3** | *(RESOLVED — the Git↔CI check seam is now frozen contract 5.9 / recon §X-1. Implemented per `02 §6`, `03 §1.1`. The drill is D-10.)* | — (was Git+CI P4; now frozen) |
| **OQ-4** | **The replica quorum-ack protocol + failover window** (pack durability), **plus** the object-backed pack/delta layout over `BlobStore` (pack chunking, delta-base selection, the smart-transport read path from object-tier blobs) — the GF-1/GF-2 implementation. Includes the fencing scheme (`update_seq`) + reftable/DB-ref-index recovery consistency. | Git P6 + Storage |
| **OQ-5** | **Merge-queue speculative batching** — the measured promotion trigger from single-lane (GF-8). | Git P6 (measured) |
| **OQ-6** | **Multi-tenancy isolation level for git** (row-level vs schema vs cell) + how residency partitioning maps onto repo placement groups — within ADR-11's spectrum + the frozen `placement_of(repo)` (12.2). | Git P6 + Tenancy |
| **OQ-7** | **CODEOWNERS-as-relations + ref-glob-scoped relations at push QPS** — whether the frozen fragment (4.9) answers `list_subjects(pr, review)` / `check(push_protected)` at push QPS off the authz reverse index, or a materialised resolver cache is needed. | Id P6 + Git P6 |
| **OQ-8** | **In-UI conflict resolution scope** beyond GF-6 single-file web edit. | Git P6 (measured) |
| **OQ-9** | **SHA-256-default flip trigger (GF-2b)** — the measured stock-client + tooling-compatibility bar (post-Git-3.0). | Git P6 (measured) |
| **OQ-10** | **Pseudonym enforcement mode** — client-cooperative (sha-stable) vs server-side rewrite-at-push (guaranteed, sha-shifting) as the per-repo default. The *property* (pseudonymous-by-default) is decided; the enforcement default + stock-client story is the call. | Git P6 + Id |
| **OQ-11** | **LFS scope** — LFS batch protocol vs partial-clone-native large files vs both — the v1 call. | Git P6 |
| **OQ-12** | **Design fidelity to pixels.** The IA/flows/wireframes (`../design/`, with happy/empty/loading/error/permission/erased/agent-pending states for every primary screen) are **present**; the remaining follow-on is the visual/token-level design-system pass (incl. the **new X-1 fork-trust + checks-panel + merge-queue affordances**, `04 §2.2`) before frontend build. | Git P6-design (pre-frontend) |

---

## 3. Honesty notes (uncertainty, assumptions, deferrals)

- **The Git↔CI seam is no longer an open question** — it is frozen contract 5.9. The remaining risk is
  *implementation* (the D-10 drill), not design. The two load-bearing invariants to not regress:
  `run_attempt`-monotonic supersession (clocks are not authority) and `untrusted_fork`-is-neutral-until-
  endorsed (a fork must never self-green).
- **gitoxide server-side maturity** is the single biggest *implementation* uncertainty (OQ-1) — v1 uses
  canonical `git`; the `GitCore` seam makes the eventual gix-ward swap per-op, not a subsystem bet.
- **The erasure residual is one platform statement, not a Git-local one** (contract 10.9) — the engineering
  job is to *minimise PII in immutable history* (pseudonymous-by-default) so the legal question rarely bites;
  the residual ratification is R-7 `[OPEN — LEGAL]`, not claimed solved.
- **Monorepo + replication numbers** (D-4, D-5) are deliberately **not guessed** (Phase-1 §3.2); they are
  benchmark deliverables.
- **The Stage-1 design folder is present and consumed** (`../design/`) — every primary screen has
  happy/empty/loading/error/permission/erased/agent-pending states; only the visual/token pass (OQ-12),
  now including the X-1 affordances, remains for the P6-design build.
