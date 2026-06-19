# 07 — Drills Owed & Open Questions

> PROVE-IT (T-1/T-4): a property does not exist until a failure-injection drill forces the failure and
> observability watches the system survive. Each failable property below names its **quantified gate**
> and the **survival signal** the drill reads (X-1). Then the open questions for Phase 5. Date:
> 2026-06-19.

---

## 1. Drills owed (quantified)

| # | Drill | Property proven | Quantified gate | Survival signal (X-1) |
|---|---|---|---|---|
| **D-1** | **Per-ref ordering at push QPS** (= Bus D-9). Burst force-pushes + rapid pushes to one hot ref under load (1×/10×/30×), parallel pushes across many refs. | `git.ref.updated` delivered in **push order per ref**; refs fan out in parallel; outbox order == ref-update order. | **per-ref order preserved at target QPS**; zero lost/ghost events; cross-ref throughput scales. | per-aggregate publish latency; outbox depth; consumer lag. |
| **D-2** | **Erasure-reaches-every-holder (git subject).** Erase a subject who authored commits/PRs/comments + uploaded LFS. | DSR fan-out hits: pseudonym map (Id), per-subject DEK (bodies, live+backups), reflogs/bitmaps/pack backups, search index (purge+reindex), refs (tombstone). **The residual is EXACTLY the immutable non-pseudonymised content bytes — nothing more.** | every holder hit; residual scoped + documented (GD-1); crypto-shred reaches backups. | DSR receipt set; erasure-ledger entry; reindex completion. |
| **D-3** | **Reindex-from-cold parity** (Search code index + Refs edges). Wipe the code index; `replay(repo, since=0)` re-emits `git.*.snapshot`. | the cold rebuild via `replay` matches steady-state indexing (one code path, no drift). | **byte-for-byte parity** of the rebuilt index vs. steady-state; no cross-DB read. | replay throughput; index doc count parity; consumer lag drain. |
| **D-4** | **Monorepo ceiling benchmark.** Grow a synthetic monorepo (file count, history depth, push QPS) until partial-clone/sparse/bitmaps degrade. | establishes the v1 monorepo ceiling (GF-4) — the "out of scope, use Mononoke" line, **measured not guessed**. | documented ceiling numbers; clone/fetch latency budget held below the ceiling. | clone/fetch p99; bitmap/commit-graph freshness; serving CPU/IO. |
| **D-5** | **Linearizable protected-ref merge / no split-brain.** Concurrent merges + a force-push to one protected `base_ref`; inject a DB-replica (ref-authority) failover and a serving-node recovery mid-merge. | protected-ref merges are **linearizable** on the DB ref CAS; no split-brain; no lost merge; recovery reconciles to the DB ref index. | zero conflicting tips; merge order linearized on the ref row; failover loses zero committed merges; `update_seq` monotonic + the fence honoured. | DB failover events; ref `update_seq` monotonicity; merge-queue state; recovery reconcile log. |
| **D-6** | **Clone-storm / hot-repo shed (protected human lane).** 30× agent/CI clone surge on a hot repo. | the human interactive lane holds; agent/CI lane sheds (`429 + Retry-After`); other tenants unaffected (per-tenant fairness). | human fetch p99 held; agent/CI shed as designed; zero cross-tenant starvation. | shed counts; per-tenant in-flight; bundle-URI hit rate. |
| **D-7** | **Diff-anchor correctness across rewrite.** Force-push/rebase a PR with open inline threads. | anchors remap correctly in unchanged hunks; **mark `outdated` (never silently wrong)** in changed hunks; "view in original context" renders. | zero mis-anchored comments; outdated set is exactly the changed-hunk threads. | anchor remap outcomes; outdated count. |
| **D-8** | **Cross-tenant IDOR on the git wire.** Attempt a cross-tenant repo access via a token whose tenant ≠ the URL-path tenant. | tenant comes from the token, never the path (ID-3); zero cross-tenant read. | **zero cross-tenant read**; the request is rejected at the front door. | authz deny counts; tenant-predicate lint (build-time). |
| **D-9** | **Outbox-at-push-throughput, emit-iff-committed.** Crash the serving tier mid-push (after policy, before/after commit). | a `git.ref.updated` is emitted **iff** the ref move committed (BUS-2); no ghost event, no lost event. | exactly-once-effective per push; quarantine objects discarded on abort. | outbox depth; dedup ledger; relay claim rate. |

D-1, D-3, D-6, D-8 are shared/inherited drills (the subsystem instantiates them on git surfaces); D-2,
D-4, D-5, D-7, D-9 are git-specific. All feed the Phase-5 testing strategy (T-5 named-drill set).

---

## 2. Open questions for Phase 5

| # | Question | Owner / resolver |
|---|---|---|
| **OQ-1** | **gitoxide server-side capability matrix** against the current release. Stage-1 web-verified that `gix` has NO server-side `upload-pack`/`receive-pack` *today* (so v1 serves the wire with canonical `git`); the spike tracks which wire/maintenance ops can later move off `ShellGitCore` to a `gix` server, gating any migration. | Git P4 spike (gates the gix-ward migration) |
| **OQ-2** | **The Art. 17 residual reach into immutable commit bytes** vs. the documented-lawful-basis limit (`[OPEN — LEGAL]`, GD-1/L-2). | Legal/DPO + Git P4 |
| **OQ-3** | **The Git↔CI checks/commit-status contract** — the exact `check_status` shape, fork/trust-tier signals that gate merges, and required-vs-optional semantics. The most load-bearing cross-subsystem seam; **jointly designed with CI in P4**. | Git P4 + CI P4 |
| **OQ-4** | **The replica quorum-ack protocol + failover window** (pack durability), **plus** the object-backed pack/delta layout over `BlobStore` (pack chunking, delta-base selection, the smart-transport read path from object-tier blobs) — the GF-1/GF-2 implementation detail. Includes the fencing/generation-number scheme (`update_seq`) and how the reftable + DB ref index stay consistent on recovery (the DB is the tiebreaker — `02 §4.2`). | Git P4 + Storage |
| **OQ-5** | **Merge-queue speculative batching** — when base-ref merge throughput justifies promoting from single-lane (GF-8); the measured promotion trigger. | Git P4 (measured) |
| **OQ-6** | **Multi-tenancy isolation level for git** (row-level vs schema vs cell) and how residency partitioning maps onto repo placement groups — within ADR-11's spectrum. | Git P4 + Tenancy |
| **OQ-7** | **CODEOWNERS-as-relations efficiency** — whether ref-glob-scoped relations (CR-ID-1) are answerable at push QPS, or a materialised resolver cache is needed. | Id P5 + Git P4 |
| **OQ-8** | **In-UI conflict resolution scope** beyond GF-6 single-file web edit. | Git P4 (measured) |
| **OQ-9** | **SHA-256-default flip trigger (GF-2b).** The measured stock-client + tooling-compatibility bar at which the new-repo *default* flips from SHA-1+`sha1dc` to SHA-256 (post-Git-3.0). The hash-agnostic model makes this a default-change; the trigger is the open call. | Git P4 (measured) |
| **OQ-10** | **Pseudonym enforcement mode** (Stage-1 sketch 09 open item): client-cooperative (CLI/UI authors as the pseudonym, sha-stable) vs server-side rewrite-at-push (guaranteed, sha-shifting) as the per-repo default. The *property* (pseudonymous-by-default) is decided; the enforcement default + stock-client story is the call. | Git P4 + Id |
| **OQ-11** | **LFS scope** (Stage-1 sketch open item): LFS batch protocol vs partial-clone-native large files vs both — the v1 call. | Git P4 |
| **OQ-12** | **Design fidelity to pixels.** The IA/flows/wireframes (`../design/`, with happy/empty/loading/error/permission/erased/agent-pending states for every primary screen) are **present**; the remaining follow-on is the visual/token-level design-system pass on those wireframes before frontend build (VISION §3 "no frontend code without a design sketch" is satisfied at structural fidelity; pixel/token fidelity is the P4-design build). | Git P4-design (pre-frontend) |

---

## 3. Honesty notes (uncertainty, assumptions, deferrals)

- **gitoxide server-side maturity** is the single biggest uncertainty (OQ-1) — Stage-1 web-verified that
  `gix` cannot serve the wire today, so v1 uses canonical `git`; the whole `GitCore` seam exists precisely
  to make the eventual gix-ward swap per-op rather than betting the subsystem on gitoxide.
- **The GD-1 legal residual** (OQ-2) is genuinely unresolved at the engineering layer; the engineering
  job is to *minimise PII in immutable history* so the legal question rarely bites — not to claim it
  solved.
- **Monorepo + replication numbers** (D-4, D-5) are deliberately **not guessed** (Phase-1 §3.2 refused to
  guess thresholds); they are benchmark deliverables.
- **The Stage-1 design folder is present and consumed** (`../design/{information-architecture,user-flows,
  wireframes}.md`) — every primary screen has happy/empty/loading/error/permission/erased/agent-pending
  states (satisfying VISION §3 "no frontend code without a design sketch"); only the visual/token pass
  (OQ-12) remains for the P4-design build.
