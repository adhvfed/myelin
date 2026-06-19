# Git hosting — Stage-1 Findings (what I learned · what I commit · what I hand forward)

> Phase: `04-subsystem-architectures/git-hosting`, Stage 1 (design & sketch). The synthesis of the
> exploration notes (01–09) + the design sketches (`design/`). This is the committed direction the
> Stage-2 architecture builds; open questions are explicitly handed to it. Grounded in cited prior art;
> two facts re-verified against the live web (TE-8). Date: 2026-06-19.

## What I learned (the load-bearing discoveries)

1. **TE-8 re-verification flipped two "open" calls to "decided" (web-checked 2026-06):**
   - **gitoxide (`gix`) still has NO server-side `upload-pack`/`receive-pack`** — only client transport.
     A pure-`gix` server is *not viable for v1*. → shell-out to canonical `git` for wire serving.
   - **reftable is production-ready** (Git 2.48–2.51; GitLab uses it for all new repos; Git 3.0 late-2026
     makes it the default). → reftable is the ref-store on-disk format *now*, not a bet.
2. **The doctrine already pre-decided more of my "hard problems" than Phase 1 implied** — the Phase-3
   docs froze the per-ref ordering aggregate (`git/ref/<repo>:<ref>`, bus §2.3), the seeded Git ReBAC
   namespace (Id §5), the GD-1 git-history reconciliation (gdpr §7), the `BlobStore`/STOR-5 relocatability
   seam (storage §3.5), and the code-search v1 grade (Search §4.4). My job is to *implement to* these,
   not re-open them. This narrowed the genuinely-open space to: storage/replication mechanism, the
   git-core engine mix, diff-anchoring, fork/merge-queue/web-edit scope, and the SHA call.
3. **The cleanest linearization point is the ref-store DB transaction, not a bespoke per-repo consensus
   group.** Because the git subsystem owns the ref lock and the outbox row commits in the same
   transaction (BUS-2), "outbox order == ref-update order by construction" — I get linearizable
   protected-ref merges from a DB CAS, and durability from pack replication, *separately*.
4. **Pseudonymous-by-default is a push-path concern, and it dissolves most of the erasure horror.** The
   "developers expect their real name in git log" tension resolves cleanly: store the **pseudonym**,
   **project the display name at render time** (Refs/Notif). The only true residual is PII in *file
   content* / legacy history — and that is the (disruptive, documented) history-rewrite path + a
   `[OPEN — LEGAL]` lawful-basis limit, not an engineering gap I can close.

## Committed direction per hard problem

| Hard problem | Committed direction | Floor / follow-on | Sketch |
|---|---|---|---|
| **World-scale storage / replication (TE-24)** | Local-disk packs **behind the `BlobStore` trait** (STOR-5 relocatability); **DB-backed transactional ref store** as the linearization point; **primary + quorum-ack WAL-streamed replicas** for pack HA; placement via Tenancy control plane keyed `(tenant,region,repo_id)`, never node-pinned. | **Floor:** object-store-backed packs (Mononoke/JGit-DFS-class) designed-not-built; follow-on = the DFS pack layer + cache + object-aware GC, on **measured** node pressure. | 01 |
| **Git-core build-vs-embed (TE-8)** | **Layered `GitCore` trait** (strategy pattern): **canonical `git`** shelled-out+sandboxed+streamed for **wire serving + maintenance** (only complete server-side option, re-verified); **`gix` preferred / `libgit2` fallback** in-process for **read/diff/blame/projection**; whole service in **Rust** (no language divergence). | **Follow-on:** migrate wire-serving rows to `gix` server-side when it ships + passes protocol-compat & escape drills. | 02 |
| **Push path / per-ref ordering / outbox** | **In-process receive-pack** + embedded Rust policy engine using git **quarantine**; one ref-store DB txn = {migrate objects via BlobStore, **CAS ref tip**, insert `git.ref.updated` outbox row}; **per-ref aggregate** + `UNIQUE(aggregate,seq)`. **No native post-receive emit** (it's a dual write). | reject-before-ref-moves; one event per accepted move. | 03 |
| **Ref store + SHA (TE-23)** | **reftable** serving format + **DB authoritative ref index** (CAS+outbox in one txn). **SHA-1 + `sha1dc` default**, **SHA-256 opt-in per repo**, **hash-agnostic data model**. | **Follow-on:** SHA-256-default + interop/migration, on ecosystem maturity (post-Git-3.0). | 04 |
| **Authz / merge gate / wire auth** | Extend the **seeded Git ReBAC namespace** (`reader/writer/maintainer/admin`); **CODEOWNERS evaluated in the merge gate** (diff-dependent, not tuples); **merge gate = ReBAC ∩ ruleset ∩ live check state**; agents ride the **same gate** ("agent-needs-human" = ruleset predicate on `actor.kind`); wire auth via **Id.authenticate then Id.check per op** at a residency+backpressure front door. | — | 05 |
| **Code projection / code-search v1 (TE-27)** | We **own the projection**: per-blob **path/symbol/literal/trigram** + commit/PR/comment FT, `acl_object_type=repo`, **incremental on push**, **default-branch (+ configured) only**, `replay` for reindex-from-source. We own the **tokenizer + diff-driven update**; Search owns the index. | **Floors:** SCIP/LSIF symbol nav (CI-produced) and code embeddings = named follow-ons, demand-triggered. | 06 |
| **Diff/comment anchoring (TE-22)** | Anchor on **blob-SHA + path + line + side + hunk_context + commit_sha**; **remap by in-process blob-diff (gix) on every head/base move**; relocate when the line survives, mark **outdated** (+ "show in original context") when it doesn't; **comment ids are stable opaque mints** (the `#sub` ref never changes). | **Floor:** fuzzy hunk-context fallback; precise op-transform position tracking = follow-on if measured insufficient. | 07 |
| **Monorepo (TE-25)** | Support **large-but-normal** via partial/sparse/shallow + **mandatory** commit-graph/bitmaps/MIDX (canonical-git maintenance). **No Mononoke-class system v1.** Thresholds benchmarked, not guessed. | hyperscale = the object-backed follow-on (sketch 01). | 08 |
| **Forks / merge-queue / web-edit (TE-26)** | Forks = **independent copies in v1** (clean erasure/residency). Merge queue = **simple single-lane on `myelin-flow`** + auto-merge-when-green. Web edit = **single-file edit→commit + suggestions + shared rich editor for comments** (code-file editing is a *separate* code surface, not the rich editor). | **Floors:** shared-object-storage forks; speculative multi-lane merge queue; in-UI 3-way conflict resolution — all named follow-ons, measure-triggered. | 08 |
| **Erasure / pseudonymity / history (GIT-1/GD-1)** | **Pseudonymous-by-default commit identity DECIDED, gates the data model** — commit bytes carry a stable opaque pseudonym; person↔pseudonym map in Id (erasable); **display name = render-time projection**. Enforced on the **push path**. **History-rewrite** = the audited, hash-changing, rate-limited path for PII-in-content/legacy, with full distributed-erasure reach (replicas/reflogs/bitmaps/backups/bundles/CDN; foreign mirrors policy-gated). | **`[OPEN — LEGAL]` floor:** the Art. 17 residual reach into immutable bytes / lawful-basis limit — named gap-report item, DPO follow-on. | 09 |

## The glue contracts I implement (the build-to surface — confirmed)

- **`serve(AppSpec)`** (Rust harness) — three-surface topology, outbox relay, consumers, holders.
- **`OutboxTx::emit(draft, cause)`** — the only emit path; per-ref ordered `git.*` events (taxonomy in
  sketch 08); `git.ref.updated` aggregate = the ref.
- **`project(ref, viewer) → {title,state,icon,render_hint,sub_anchor?}`** — repo/PR/commit/comment/blob
  projections, per-viewer, pre-permission-checked (the PR context pane + Search text + Notif humanise
  consume it). **`replay(scope, since)`** emitting `git.*.snapshot` (sub-artifact-granular) for reindex.
- **One `Principal` via `Id.authenticate` + `Id.check`/`list_objects`/`list_subjects`**; the **Git ReBAC
  namespace fragment** (sketch 05).
- **`ToolDef` registrations:** `git.open_pr`, `git.submit_review`, `git.comment`, `git.resolve_thread`,
  `git.suggest_change`, `git.merge` (sensitive/HITL-gateable), `git.read_diff`, `git.read_file`,
  `git.search_code`. **`reserve/settle`** fronts any spend-bearing agent run touching git.
- **`PersonalDataHolder`** (we are H1) — `locate/export/rectify/restrict/erase`; restriction flag honoured
  (no index/agent/analytics/notify for a restricted subject).
- **`declare_indexable(IndexSpec)`** — the code projection (sketch 06).
- Stable **`#sub` ArtifactRef** grammar, scope-complete (sketch 08).

## PROVE-IT — the quantified drills I owe (each a Phase-5 scorecard item)

| Property | Drill (quantified gate) |
|---|---|
| Per-ref ordering at push QPS | Burst N force-pushes to one ref; assert `git.ref.updated` delivered in push order; **0 reorder, 0 lost/ghost** (bus §2.3). |
| Linearizable protected-ref merge | Concurrent merge + force-push to `main`; assert one wins, no split-brain; **0 divergent tips** across replicas. |
| Outbox never loses/dupes on a busy server | Crash between ref-move and ack; assert exactly-one event (idempotent dedup); **0 lost, 0 ghost**. |
| Diff-anchor across rewrite | Force-push a rebase; assert each comment relocates **or** marks outdated, **never mispoints**; corpus-based. |
| Code-projection reindex-from-cold parity | Wipe Search; `replay` the repo; assert index == incremental-built index (**parity**, sketch 06). |
| Erasure-reaches-every-holder (we are H1) | Seed a subject into commits+PRs+comments+LFS; erase; assert `locate` returns **0 recoverable PII** (pseudonym map gone, free-text shredded, refs tombstoned, search purged) — the residual is *only* PII-in-content (history-rewrite path). |
| Pseudonymous-commit property | Push a commit; assert the stored author bytes contain **only the opaque pseudonym**, display name resolves at render. |
| History-rewrite distributed reach | Rewrite to remove content; assert reach into replicas/reflogs/bitmaps/backups/bundles; **0 resurrected** from any of them. |
| Cross-tenant IDOR | Attempt cross-tenant repo/PR access; **0 cross-tenant read** (token tenant, never path). |
| Sandbox-escape (shelled git / push policy) | The serving-tier process hardening drill — git serving is platform code, but resource-capped + isolated; assert no host bypass. (CI owns the *untrusted-code* escape drill; ours is the serving-process hardening gate.) |
| 30× agent/clone surge | Agent/CI clone-storm; assert the **human push lane holds**, agent lane sheds (429+Retry-After), other tenants unaffected. |
| Frontend switch-test | Drive the real PR review UI in a browser; a team could move to it without hitting a wall the old tool didn't have (T-7). |

## Named floors carried to the gap report (E-3)

Object-backed packs (designed-not-built) · shared-object-storage forks · speculative merge queue ·
in-UI conflict resolution · SCIP/LSIF symbol nav + code embeddings · SHA-256-default flip ·
**pseudonymous-commit residual limit (`[OPEN — LEGAL]`)** · op-transform precise diff-anchoring.

## Open questions handed to the Stage-2 architecture

1. **Replication mechanism detail** — exact quorum-ack protocol + fencing/generation-number scheme for
   pack-replica durability; failover window; how reftable + the DB ref index stay consistent on recovery
   (DB is the tiebreaker — formalize). (sketch 01/04)
2. **Pseudonym enforcement mode** — client-cooperative (CLI/UI authors as pseudonym, sha-stable) vs
   server-side rewrite-at-push (guaranteed, sha-shifting) as a per-repo policy. The *property* is
   decided; the *enforcement default* + the stock-client story is the architecture call. (sketch 09)
3. **Push-policy sandboxing** — the exact isolation/resource-cap profile for the serving-tier
   receive-pack process (it's platform code, distinct from the CI/agent untrusted sandbox, but still
   needs hardening); reconcile with AG-2/CI-1. (sketch 02/03)
4. **The Git↔CI checks/commit-status contract** — the most load-bearing cross-subsystem seam; jointly
   designed with the CI P4 agent (required-checks shape, fork-trust-tier signals that gate merges).
5. **`commit`/`blob` ArtifactRef scoping** — whether `<type>/<id>` packs `repo:sha` or uses a `#sub`;
   finalize the scope-complete grammar with Refs. (sketch 08)
6. **Clone-storm / hot-repo mitigation detail** — clone-bundle CDN-within-EU, replica fan-out counts,
   per-repo replica policy; the residency-vs-latency trade (no non-EU replicas). (Phase-1 §3.4)
7. **LFS scope** — LFS batch protocol vs partial-clone-native large files vs both; the v1 call. (Phase-1 §3.3)
8. **Benchmark the monorepo thresholds** — repo size / file count / push QPS where stock-git-on-a-shard
   stops working (explicitly *not* guessed — Phase-1 §3.2).

## Cross-references
- Exploration: sketches 01–09 (this folder). Design: `design/{information-architecture,user-flows,
  wireframes}.md`.
- Phase-3 build-to surface: `contract-index.md` (2.1/2.3/4.x/5.x/6.x/8.x/9.x/10.x/11.2),
  `event-bus.md` §2.3/§6, `identity-and-access.md` §5/§8, `storage.md` §3.5/§5, `gdpr-and-audit.md` §7,
  `search-and-indexing.md` §4.4.
- Doctrine: EI-04 §1/§3 (erasure, world-scale git), EI-02 §1/§4 (tenant, outbox); GIT-1, STOR-5.
- Phase-2 `subsystems/git-hosting.md`; Phase-1 `subsystem-deep-dives/git-hosting.md`.

[Re-verified web sources: GitoxideLabs/gitoxide discussion #1299 (no server-side serving);
git-scm.com/docs/BreakingChanges + gitlab.com epics/12503 (reftable production + Git-3.0 default).]
