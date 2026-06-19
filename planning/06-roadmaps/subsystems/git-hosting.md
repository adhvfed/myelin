# Git Hosting — Subsystem Roadmap (Phase 6)

> Phase: `06-roadmaps/subsystems`. The detailed, sequenced build roadmap for the **git-hosting** subsystem.
> Slots into the master sequencing bands M0..M6 ([`../00-master-sequencing.md`](../00-master-sequencing.md)) —
> it refines the work *inside* the bands and must not contradict the band ordering or the gate invariant.
> Frozen architecture (this roadmap sequences, it does not redesign):
> [`../../04-subsystem-architectures/git-hosting/architecture/`](../../04-subsystem-architectures/git-hosting/architecture/)
> (00..07) + [`../../04-subsystem-architectures/git-hosting/design/`](../../04-subsystem-architectures/git-hosting/design/).
> Build-to contracts: [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md).
> Drills: [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (GIT-D1..GIT-D11 + the shared families). Binding doctrine: EI-01 (order-by-non-negotiability, prove-it,
> the ratchet, name-your-floors) + EI-04 (world-scale git, erasure-vs-immutability). Plain-text identifiers
> (no backticks-as-emphasis). Markdown only; no commits. Date: 2026-06-19.

---

## 0. Where git-hosting lives in the master sequence

Git hosting is a **producer subsystem** (master §2 M3, §3.2). It is on the **critical path**: it produces the
commits CI checks, the refs the merge gate guards, and the artifacts Issues/Chat reference. The two hardest
single seams that touch git both sit on the critical spine:

- **AG-D4 / CI-T1** (the real-kernel sandbox-escape GATE) is an **upstream blocker** — it must be green in M2
  before git's own code-executing tools (sandboxed receive-pack policy, history-rewrite, SCIP indexing) run.
- **X-1 / contract 5.9** (the Git↔CI CheckStatus seam) is **split producer/consumer across M4/M3**: git builds
  the **consumer + gate + projection in M3**, against the seam frozen in M2; CI builds the **producer in M4**;
  the seam is proven end-to-end (GIT-D10 / CI-D8) at the M4 exit.

The bulk of git's build is **M3** (the producer band). But git **participates earlier** — it freezes its
ReBAC fragment and event tokens in M1/M2 so dependents compile, and it consumes the M0..M2 substrate. Its
**world-scale / hard-problem follow-ons** (object-backed packs, cross-cell replication, the SHA-256 flip,
speculative merge-queue) are explicitly scheduled into **M5**, and the switch test lands in **M6**.

Per master §2 ("within a band, the per-system roadmaps parallelise the work"), the milestones below are the
git-internal decomposition of the M3 producer work plus its M1/M2 pre-work and its M5 follow-ons. The band a
milestone belongs to is named on every milestone. **The gate invariant binds**: no git milestone is "done"
over a red earlier-band gate (master §4).

---

## 1. The non-negotiability order applied to git (what kills us first, inside git)

Following EI-01 §2 and master §1, the git work is internally ordered by what is catastrophic, not by feature
size:

1. **Silent data loss on push** — a ref that moved without its event, or an event without the ref move, or a
   lost push under crash. This is the Tier-1 floor *for git*: the receive-pack → ref-CAS → outbox emit must be
   one transaction (BUS-2). Proven by **GIT-D9** before any feature reads the event stream.
2. **The erasure-vs-immutability decision** — pseudonymous-by-default commit identity is a **commit-time
   prerequisite that gates the data model** (EI-04 §1; arch 00 §1.1, 05 §HP-7). It must be decided and
   enforced **before the first commit is stored**, or it is nearly impossible to bolt on. This is sequenced
   into the **first** git data-model milestone (M3-G1), not deferred.
3. **Cross-tenant / authz leak on the wire and in lists** — tenant from the token never the URL (ID-3);
   leak-free PR/repo lists via the SetExpr push-down. Proven by **GIT-D8** and **GIT-D11**.
4. **The merge gate correctness (poisoned-pipeline defence)** — a fork must never green its own required gate;
   supersession is monotonic on run_attempt not wall-clock. Proven by **GIT-D10**.
5. Then the breadth (code review, code search, web edit), then the world-scale hardening (object-backed packs,
   replication failover, clone-storm shed), then the switch-test polish.

Sandbox escape (Tier 2) is **not** owned by git — it is the shared AG-D4 gate (master M2). Git inherits it by
construction (the four uniform sandbox guarantees, contract 8.4 / X-6) and **must not run code-executing tools
until AG-D4 is green**.

---

## 2. Upstream dependencies (what must exist + be green before git work starts)

Git is a thin shell over the substrate (arch 00 §2 "every box is a thin shell over myelin-substrate"). The
dependency table below names, per git milestone, the contracts that must already be implemented. The critical
ones are starred.

| Upstream (contract) | Owner / band | Why git needs it | Blocks git milestone |
|---|---|---|---|
| serve(AppSpec), three-surface, liveness≠readiness (1.1–1.3) | substrate / M0 | every git service boots from it | all |
| OutboxTx::emit + outbox table per-ref aggregate + EventHandler + consumer_dedup (2.2–2.5) | Bus / M0 | the receive-pack → outbox path; the only emit path (no-raw-publish) | M3-G1 (★) |
| EventEnvelope frozen (2.1) | Bus / M0 | the git.* event shapes align to it | M3-G1 (★) |
| The 12 lints incl. tenant-predicate, no-raw-publish, no-cross-db, residency-pin, search-requires-acl-filter (1.6) | substrate / M0 | git compiles against the ratchet | all |
| ResilientClient + FailStatic (1.9/1.10) | substrate / M0 | git→Id, git→CI-projection calls degrade not cascade | M3-G2 |
| **Identity: authenticate (machine-identity: SSH/deploy-key/PAT/per-job) (4.1)** | Id / M1 | the SSH + smart-HTTP front door resolves a Principal | M3-G2 (★) |
| **Identity: check + CaveatContext (4.2)** | Id / M1 | per-action push/merge/review gate | M3-G2 (★) |
| **Identity: list_objects SetExpr push-down (4.3)** | Id / M1 | leak-free, no-N+1 repo/PR lists + code-search pre-filter | M3-G5 (★) |
| Identity: write_tuples/zookie (4.6/4.10) | Id / M1 | the Git ReBAC fragment compiles + read-your-writes | M3-G2 |
| Identity: mint_run_token (4.7) | Id / M1 | per-run token for code-executing git tools | M3-G6 |
| **Identity: resolve_pseudonym/erase + pseudonym grammar `<pseudonym>@<tenant>.noreply` (4.8)** | Id / M1 | pseudonymous-by-default commits + DSR step 1 | M3-G1 (★) |
| Identity: ReBAC engine (4.9) accepting the Git fragment | Id / M1 | ref-glob + CODEOWNERS-as-relations + approve_untrusted_ci | M3-G2 |
| Tenancy: (tenant, region) partition (12.1); discover/placement_of repo-granular relocatable (12.2); residency_verify (12.4) | Tenancy / M1 | repo→cell placement honouring residency; reject-if-leaving-region at the front door | M3-G2 (★) |
| **Storage: OLTP tier + RLS + encrypted columns + the outbox (11.1)** | Storage / M1 | git's control-plane DB (PR/review/repo/ruleset rows) | M3-G1 (★) |
| Storage: BlobStore content-addressed, fs-backed floor (11.2) | Storage / M1 | the pack tier (local-NVMe floor), LFS, bundles | M3-G1 (★) |
| Storage: KMS hierarchy + per-subject DEK (11.3/11.4) | Storage / M1 | crypto-shred for PR/review/comment bodies + reflogs/bitmaps/pack backups | M3-G1, M3-G7 |
| **Storage: backup/restore + restore-verify, RPO ≤ 5min / RTO ≤ 1h-tenant (11.5)** | Storage / M1 | the silent-data-loss floor; git does not write real data over a red STOR-D1 | M3-G1 (★) |
| GDPR: PersonalDataHolder trait + classify-derive + erasure ledger + the ONE posture (10.1/10.2/10.8/10.9) | GDPR / M1 | git auto-registers as holder H1; tags author_pseudonym etc. | M3-G1, M3-G7 |
| **Refs: ArtifactRef parse/format (5.1); resolve/project (5.2/5.6); the #sub grammar + 4-step tombstone ladder (5.7)** | Refs / M2 | git mints commit/pr/comment/thread/L<a>-L<b> sub-refs + the content-anchored resolver | M3-G3, M3-G4 (★) |
| Refs: edges from content nodes (5.4); typed-edge mirror (5.5) | Refs / M2 | Closes/PR-link trailers produce edges | M3-G3 |
| Search: declare_indexable + query conjoining the list_objects Filter (6.1/6.3/6.5) | Search / M2 | the git.* code projection + code search | M3-G5 |
| Notif: humanise (7.3); define_notif_rule (7.6); inbox (7.1) | Notif / M2 | review-requests / PR-status notifications | M3-G8 |
| **Agent fabric: ToolSurface + EffectApi + ToolHands::exec unified sandbox (8.1/8.2/8.4); AG-D4 GREEN** | Agent / M2 | git's code-executing tools (history-rewrite, SCIP) + agent authors/reviewers | M3-G6 (★) |
| **Durable workflow: DurableExecutor + WfCtx + SCHEDULE_AND_RUN_JOB + durable signal (9.1/9.2/9.4); timer wheel (9.3)** | Workflow / M2 | the merge queue (parks on ci.result for hours), maintenance ops | M3-G4 (★) |
| Reserve/settle cost gate (11.7) | Agent+Commercial / M1-M2 | fronts every code-executing git tool run | M3-G6 |
| **CI: CheckStatus producer (ci.check.updated + ci.result rollup) (5.9)** | CI / M4 | closes the X-1 seam end-to-end | M4 co-gate (★) |

**The seam frozen-but-not-live note:** the X-1 CheckStatus seam (5.9) is *declared* in M2 (master §2 M2) and
the Git side (consumer/projection/gate) is built in M3, but it only goes **live end-to-end** when CI's producer
lands in M4. Git in M3 ships the gate + projection table + supersession against a **synthetic ci.check.updated
emitter** (a drill fixture), and the real producer wiring is the M4 co-gate. This is a named seam-floor (§5).

---

## 3. The milestones (each mapped to a master band, with the work)

Nine git milestones. M3-G1..M3-G8 are the M3 producer band; M5-G9 is the M5 world-scale follow-on; M6-G10 is
the dogfood switch test. A small slice of **pre-work lands in M1/M2** (the ReBAC fragment freeze + the event
token registration + the design-system pass) so dependents compile — called out in §3.0.

### 3.0 — Pre-work in M1/M2 (the freeze-so-dependents-compile slice)

**Band: M1 (alongside Identity) + M2 (alongside the reactive layer).** No git *features* here — only the
contract fragments other systems compile against, plus the design-system pass that VISION §3 requires before
any frontend code.

- **M1:** freeze and submit the **Git ReBAC namespace fragment** (4.9: ref-glob-scoped relations,
  CODEOWNERS-as-relations, protected_push, **approve_untrusted_ci**, watcher-per-watchable) into the one cell
  schema Identity compiles. Register the **git.\* event tokens** (git.ref.updated, git.pr.\*, git.review.\*)
  in the Bus taxonomy seed (2.9). Declare git's **PersonalDataHolder H1** intent + the `#[personal_data]` tags
  on the (still-skeletal) schema so the no-untagged-personal-data lint is green from the first migration.
- **M2:** confirm the **#sub grammar mints** git owns (comment-/thread-/L<a>-L<b>) are registered with Refs
  (5.7) and that git's **declare_indexable** projection spec is registered with Search (6.3). The X-1
  **CheckStatus consumer contract** is declared (the projection-table schema + the run_attempt supersession
  rule are written against the M2-frozen 5.9 shape, ready to build in M3).
- **M2 (design, pre-frontend — OQ-12):** the visual/token-level design-system pass over the present IA/flows/
  wireframes (design/), **including the new X-1 affordances** (fork-trust badge, checks panel, merge-queue
  affordances; arch 04 §2.2). VISION §3: no frontend code without a reviewed design sketch behind it. This is
  a **decision-shaped** surface only where fork-trust UX is concerned (EI-01 §8) — sketch + sign-off.

**Entry dependency:** M0 green (the lints + outbox + envelope exist to register against).
**Exit gate:** the Git ReBAC fragment compiles in the cell schema; git.\* tokens registered; the design pass
is reviewed-and-signed-off. (No drill — these are compile-time + sign-off gates, not runtime properties.)

---

### M3-G1 — The git object store + receive-pack + the silent-data-loss floor + pseudonymous commits

**Band: M3.** The keystone. This milestone makes git **store data without losing it** and **without baking
erasable PII into immutable bytes** — the two Tier-1/erasure non-negotiables, before any feature reads the
stream.

**Work:**
- The git-core embed via the layered **GitCore seam** (arch 01 §2, 02 §2): sandboxed canonical `git` for wire
  serving + maintenance; `gix` (libgit2 fallback) in-process for read/diff/blame. (The TE-8 Stage-1 position;
  the gix-ward per-op migration is OQ-1, a named M5+ spike, not in scope here.)
- **The receive-pack path (arch 02 §2):** sandboxed `git receive-pack` ingests the pack into a **quarantine**;
  in-process Rust evaluates branch-protection / secret-scan / size / **pseudonymity** rules — *reject before
  the ref moves*; then **our** code does the ref CAS + the outbox insert **in one DB transaction** (BUS-2).
- The **reftable-on-OLTP ref store** (arch 02 §3): the ref-update transaction is the linearisation point; the
  aggregate for git.ref.updated is the **ref** (per-ref ordering at push QPS).
- Pack/delta storage on the **local-NVMe floor behind the BlobStore trait** (GF-1), repos **relocatable, never
  node-pinned** (STOR-5). Commit-graph + reachability bitmaps + MIDX maintenance.
- **Pseudonymous-by-default commit identities (GIT-1, the data-model gate, EI-04 §1):** the schema stores
  `author_pseudonym`/`reviewer_pseudonym`/`pusher_pseudonym` (never name/email); the person↔pseudonym map is
  Identity's erasable record (4.8). Push policy enforces the pseudonym. **This is decided and enforced here,
  before the data model is fixed** — it cannot be bolted on later.
- Repo / fork-network / quarantine schema; the control-plane DB (one DB per service, RLS, per-tenant
  envelope-encrypted, per-subject DEK for free-text bodies); auto-registered as PersonalDataHolder H1.
- The git.ref.updated / git.\* event taxonomy emitted via the outbox only.

**Floor-then-full:** ships the **local-disk pack floor (GF-1)** + **single-cell primary+quorum replication
floor (GF-2)** + **pseudonymous-by-default (GF-7 mechanism)**. Follow-ons named in §5.

**Upstream deps:** outbox (2.2–2.5), envelope (2.1), Storage OLTP+BlobStore+KMS+restore-verify (11.1–11.5),
Identity pseudonym (4.8), GDPR holder (10.1/10.9). **★ STOR-D1 must be green** — git does not write real data
over a red restore-verify (master M1→M2 gate).

**Done gate (quantified):**
- **GIT-D9** (CI): crash serving tier mid-push (after policy, before/after commit) → git.ref.updated emitted
  **iff** the ref move committed; **0 ghost, 0 lost**; quarantine objects discarded on abort.
- **GIT-D1** (SCHED): burst force-pushes + rapid pushes to one hot ref (1×/10×/30×) → git.ref.updated in
  **push order per ref**; refs fan out parallel; **0 lost/ghost**; outbox order == ref-update order.
- **GIT-D2** (SCHED, partial here, completed at M3-G7): erasing a commit-author subject leaves
  pseudonymous-by-default residual == the ONE platform posture (the GIT-1 half is asserted here).

---

### M3-G2 — The front door (SSH + smart-HTTP v2), authz, residency, and cross-tenant isolation

**Band: M3.** Makes the wire **safe**: every request authenticates to a Principal, the tenant comes from the
token, every action is checked, and no route leaves the region.

**Work (arch 00 §2 (A), 02 §1):**
- The stateless **front door / router**: SSH + smart-HTTP protocol-v2; `Id.authenticate` resolving every
  machine-identity (SSH pubkey / deploy-key / PAT / per-job token, 4.1) → Principal; the per-action `Id.check`
  with CaveatContext (4.2); `discover`/`placement_of(repo)` → cell + backend node (12.2); **reject any route
  that would leave the region** (ADR-11, residency-pin lint); streams packs without full buffering;
  liveness≠readiness (readiness gates on backend reachability, liveness does not).
- The **Git ReBAC fragment** wired live (4.9): ref-glob relations, CODEOWNERS-as-relations, protected_push,
  approve_untrusted_ci. FailStatic bound on the Id dependency (4.11) so an Id hiccup degrades, not cascades.
- The protected-human-lane **shed order** (ADR-16, the OQ-K per-surface budget floor: speculative → batch/CI →
  agent → human-last) at the front door, with `429 + Retry-After`.

**Floor-then-full:** the per-surface shed **budget floor** (OQ-K) is a named-floor tuned by GIT-D6 (the
clone-storm drill lands in M5-G9). The CDN clone/bundle accelerated-clone path (11.2 C3) ships its **bundle-URI
floor** here; the full within-EU CDN class hardens in M5.

**Upstream deps:** Identity 4.1/4.2/4.6/4.9/4.11; Tenancy 12.1/12.2/12.4; ResilientClient/FailStatic.

**Done gate (quantified):**
- **GIT-D8** (CI): cross-tenant repo access via a token whose tenant ≠ the URL-path tenant → **tenant from
  the token**; **0 cross-tenant read**; rejected at the front door; tenant-predicate lint green.

---

### M3-G3 — Pull/merge requests, reviews, inline threads, and the reference-graph edges

**Band: M3.** The hosting-layer domain entities and the producer side of the reference graph.

**Work (arch 00 §1.1, 03 §1.1):**
- The **Pull Request lifecycle**, Reviews, inline comment **threads**, branch-protection rulesets, CODEOWNERS
  resolver — all on the control-plane OLTP.
- PR/review/comment **bodies use the frozen myelin-content markdown-subset** + the three structured inline
  nodes (mention/artifact_ref/embed, 13.1) which **produce refs.edge.created uniformly** (Closes <ISSUEKEY> /
  @alice / embeds → edges, 5.4). Single-author CAS over the content subset; `render(parse(md)) === md`.
- `project(ref, viewer)` (5.6) for git artifacts (PR/commit/review) — the only way Refs/Search/Notif read
  git's artifacts, per-viewer permission-checked.
- The typed-edge mirror (5.5): PR-link / commit-trailer lifecycle edges (closes/relates) into the Refs
  projection.
- ArtifactRef id grammar (5.1, REF-3): git's stored canonical key is the sha / PR-number (already stable); the
  `#1421`-style display is render-time only.

**Upstream deps:** Refs 5.1/5.2/5.4/5.5/5.6; myelin-content 13.1; Identity check.

**Done gate (quantified):** (no git-specific drill is exclusive to this milestone; it is proven by the
reference-graph leak drills it feeds, exercised at M3-G5 / M5 E2E-1). Internal CI proof: the three inline ref
nodes each emit exactly one refs.edge.created; `render(parse(md)) === md` 100% on PR/comment bodies (the
KN-D2-class round-trip applied to git content).

---

### M3-G4 — The merge gate, the CheckStatus projection, and the merge queue (the X-1 consumer half)

**Band: M3 (the consumer half of the critical-path X-1 seam).** Git owns "what is allowed to land."

**Work (arch 02 §6, 03 §1.1):**
- The **check_status projection table** keyed `(commit_oid, context)` — the X-1 consumer (5.9): consumes
  `ci.check.updated`, applies **monotonic run_attempt supersession** (`>=` supersedes, `<` dropped as stale
  re-delivery — the bus is at-least-once so the drop is mandatory), idempotent on event_id, holds exactly one
  current row per key.
- The **merge gate**: Git owns the **required-set policy** (`ruleset.required_contexts` — CI reports facts,
  Git decides which contexts gate this base_ref). Git **reads trust_tier off the fact, never recomputes it**.
- The **fork / trust-tier gate (the poisoned-pipeline defence, arch 02 §6.3):** an `untrusted_fork` success is
  **neutral for gating** until a maintainer endorses via `check(subject, approve_untrusted_ci, repo)` OR the
  context is re-run trusted. Fork-PR cache writes confined to the `fork:<pr_id>` scope (11.2 C4) — a fork
  cannot reach the trusted cache or the trusted gate.
- The **merge queue** as a **durable workflow** per target ref (9.1/9.2/9.4): parks on the rollup **ci.result
  signal** via SCHEDULE_AND_RUN_JOB (holds no runtime while CI runs for hours); idempotent on the
  merge_attempt_id idem_token. Single-lane serialised (GF-8 floor).
- Git **never synchronously calls CI** (no-cross-sync-cycle lint) — it reads its own projection.
- The **content-anchored line-range resolver** (5.7, OQ-D): mint `#L<a>-L<b>` storing a BLAKE3 fingerprint +
  context window + mint-time blob oid; resolve through the 4-state ladder (exact LIVE / rebased MOVED /
  partial OUTDATED / tombstone GONE). Git is the owner's sub-anchor resolver the Refs ladder calls.

**Floor-then-full:** single-lane merge queue (GF-8 — speculative/parallel batching is OQ-5, M5). Diff-anchor
per-pair fingerprint remap (GF-5 — patch-id-chain carry-over is R-6, M5). **The CI producer is not live until
M4** — built here against a synthetic ci.check.updated fixture (the seam-floor, §5).

**Upstream deps:** Workflow 9.1/9.2/9.4 + timer wheel 9.3; the X-1 seam frozen (5.9); Refs #sub ladder (5.7);
Storage trust-scoped cache (11.2 C4); approve_untrusted_ci ReBAC relation (4.9). **The producer side (CI 5.9)
is the M4 co-dependency.**

**Done gate (quantified):**
- **GIT-D10** (CI, against the synthetic producer here; **re-confirmed end-to-end with the real CI producer at
  the M4 exit**): (a) out-of-order/dup ci.check.updated → run_attempt-monotonic supersession holds the correct
  current row, drops stale lower attempts; (b) a fork PR self-greens → **neutral for gating** (merge blocked);
  (c) a maintainer endorses via approve_untrusted_ci → gate flips green; (d) a doubly-delivered ci.result →
  the merge workflow wakes **exactly once**; **0 double-merge** (merge-count == 1).
- **GIT-D7** (CI): force-push/rebase a PR with open inline threads → anchors resolve
  **LIVE/MOVED/OUTDATED/GONE** correctly; **0 mis-anchored**; never silently wrong; "view in original context"
  renders.

---

### M3-G5 — The code projection for search + leak-free fast lists at scale

**Band: M3.** Git owns *what* to index; Search owns the index (no cross-DB).

**Work (arch 02 §9, 03 §5.3):**
- The **code-projection emitter** (6.3/6.5): per changed blob, emit {path, language, symbols (camel/snake
  split), literals, commit message, text}; incremental update on push. Search builds trigram indices
  (symbol/path/literal/trigram-grade v1, GF-3).
- The **list_objects SetExpr push-down** wired for repo/PR lists and the code-search pre-filter (4.3, OQ-E):
  lower the `Ids | Filter{set_expr, zookie}` to a **SQL JOIN over git's own id column** (repo.id / pr.id)
  against Identity's per-tenant authz reverse index — **no N+1, no post-filter**. Always conjoined before
  scoring (search-requires-acl-filter lint).

**Floor-then-full:** trigram/lexical code search v1 (GF-3); the AST-aware "find usages" via CI-produced
SCIP/LSIF (R-3) is the named M5+ follow-on.

**Upstream deps:** Search 6.1/6.3/6.5; Identity list_objects 4.3 (★).

**Done gate (quantified):**
- **GIT-D11** (SCHED): a viewer with partial repo/PR visibility lists a 100k-PR tenant → the SetExpr JOIN
  returns **only visible rows (0 leak)**, in **one query** (no N+1, no post-filter); a just-revoked grant is
  reflected within the zookie bound.
- Feeds the shared SRCH-D1/D3 (confidential code never in any result incl. counts/IDF) — git's code projection
  asserted leak-free there.

---

### M3-G6 — Code-executing git tools (history-rewrite, SCIP indexing) + agent authors/reviewers

**Band: M3.** Anything in git that runs code rides the **unified sandbox** — **gated on AG-D4 being green**.

**Work (arch 00 §4.8, 03 §7):**
- Register git's **ToolDefs** with the frozen requires_approval defaults (8.1, X-6): `git.merge` = **yes**,
  `git.open_pr` = **no**. Code-executing tools (history-rewrite, SCIP indexing) go through `EffectApi::apply`
  (plan-then-apply) and `ToolHands::exec` (= the CI runner's kind=agent job, 8.4) — inheriting the **four
  uniform guarantees** (reserve/settle cost gate, per-run attenuated token, HITL withhold, isolation floor +
  the real-kernel escape drill) **by construction, never re-implemented**.
- Agents as **first-class authors/reviewers** (legible, bounded): an agent can open a PR, comment, review — via
  EffectApi, with mock runtimes during development (`--use-mock`, VISION §3).
- The **history-rewrite erasure path** as an audited, rate-limited tenant op (10.6) with fork/mirror/
  clone-cache invalidation fan-out (built here as the tool; its erasure semantics complete at M3-G7).

**Hard upstream gate:** **AG-D4 / CI-T1 must be green (M2 exit).** Until then, no code-executing git tool runs
(master M2→M3 gate). Reserve/settle (11.7) fronts every run.

**Upstream deps:** Agent 8.1/8.2/8.4; Workflow (a run is a durable workflow); reserve/settle 11.7; Id
mint_run_token 4.7. **★ AG-D4 green.**

**Done gate (quantified):** inherits **AG-D1/D2/D3/D5** (no write outside EffectApi; effect outside the ∩
denied; HITL-gated tool withheld → 0 mutation pre-approval) — git's tools are asserted to honour them. No
git-exclusive drill; the sandbox property is the shared AG-D4 (re-run on the git tool image).

---

### M3-G7 — Erasure-reaches-every-holder (the GDPR git-history obligation, completed)

**Band: M3.** Git is the **hardest holder in the platform** (H1). This milestone completes the erasure
mechanism begun at M3-G1.

**Work (arch 05 §HP-7, 03 §6):**
- DSR fan-out over git: **pseudonym-map delete** (Id, step 1) ⇒ immutable bytes hold only the opaque
  pseudonym; **per-subject DEK crypto-shred** for PR/review/comment **bodies + titles** (11.4) reaching live +
  backups by construction; reflogs / bitmaps / pack-tier backups shreddable via the per-tenant blob DEK; the
  search code index purge+reindex; refs tombstone; cache/CDN invalidation (H9).
- The **history-rewrite path** (10.6) as the supported disruptive op for PII-in-content (the rare case a body
  must be expunged), with the understood changed-hash consequence + the invalidation fan-out.
- The **residual is instantiated by reference** to the ONE platform posture (10.9 / X-7), **not** restated as a
  Git-local statement. The `[OPEN — LEGAL]` Art. 17 ratification is R-7 (Legal/DPO, parallel).

**Floor-then-full:** the structural floor (pseudonymous-by-default + per-subject DEK shred + history-rewrite)
ships here regardless; the lawful-basis residual is one ratified statement (R-7, parallel-legal, not a code
gate).

**Upstream deps:** GDPR DSR orchestrator (10.1/10.4); Storage crypto-shred (11.4); Id erase (4.8); the
erasure ledger (10.8); reindex-from-source (2.6).

**Done gate (quantified):**
- **GIT-D2** (SCHED): erase a subject who authored commits/PRs/comments + uploaded LFS → **every holder hit**
  (pseudonym map, per-subject DEK bodies live+backups, reflogs/bitmaps/pack backups, search index, refs,
  cache/CDN); the residual is **exactly the ONE platform-posture residual (10.9), nothing more**; crypto-shred
  reaches backups.
- **GIT-D3** (SCHED): wipe the Search code index + Refs edges + the check_status projection; reindex/replay →
  cold rebuild **byte-matches** live (one code path, no drift); the check_status projection rebuilds from CI's
  ci.check.updated re-emit; **no cross-DB read**.

---

### M3-G8 — Notifications, web UI, and the M3 producer-band exit

**Band: M3.** The user-facing surface + the notification wiring that closes the M3 band.

**Work (arch 04, 00 §2 (D)):**
- Which git events are notifiable + their targets via Signals (define_notif_rule 7.6: review-requested /
  PR-status / mention); the summary template keys resolved through **humanise** (7.3) per-viewer (the ONE
  templating surface — confidential subject → humanised tombstone, title never leaks). Review-requests appear
  as a `filter` over the ONE inbox (7.1), never a second store.
- The **Web UI**: repo browse, code view, PR/review/inline-thread, the checks panel + fork-trust badge +
  merge-queue affordances (the X-1 design pass from §3.0), single-file **web edit + commit** (GF-6 floor — no
  3-way conflict editor in v1). Built against the reviewed design sketches (VISION §3); driven in a browser
  before "done" (the switch-test rehearsal, full switch test at M6).
- The myelin CLI git surface + the HTTP/RPC + agent-tool API (arch 04).

**Floor-then-full:** single-file web edit (GF-6 — in-browser conflict resolution is OQ-8, M5+).

**Upstream deps:** Notif 7.1/7.3/7.6; the design-system pass (§3.0); Refs resolve for unfurls.

**Done gate (quantified):** **NOTIF-D4-class** assertion on git subjects (confidential PR/commit subject →
humanised tombstone, title never leaks). The M3 band exit (master §2 M3) is the **aggregate** of GIT-D9, GIT-D8,
GIT-D11, GIT-D7, GIT-D2 — all green ⇒ M3 done for git, M4 may start.

---

### M5-G9 — World-scale hardening + the floor follow-ons (the hard-problem band)

**Band: M5.** With all five subsystems on one substrate and the deterministic correctness drills green, the
named git floors are promoted and git is proven **as a whole under world-scale load** (master §2 M5, §5).

**Work — the named floor follow-ons (each named in its M3 band, scheduled here):**
- **Object-backed git packs (GF-1 → R-1 / OQ-4):** authoritative pack bytes move from node-local NVMe to the
  object store behind BlobStore — delta/pack management, sharding, replication, the smart-transport read path
  from object-tier blobs, the within-EU CDN clone/bundle class (11.2). **This is the explicit local-disk →
  object-backed transition EI-04 §3 insisted be sequenced, not bolted on** — early choices (repo-granular
  relocatable placement, 12.2) did not pin repos to one node, so this is a BlobStore-impl swap + a transport
  path, not a data-model rewrite. The quorum-ack protocol + fencing (update_seq) + object pack layout is OQ-4.
- **Cross-cell / multi-region replication (GF-2):** cross-cell active replica sets within-EU; geo
  read-replicas; the single-cell floor's primary+quorum lifts to multi-cell (rides the OQ-I cross-cell bridge).
- **Speculative/parallel merge-queue batching (GF-8 → OQ-5):** promote from single-lane once the promotion
  trigger is **measured**.
- **SHA-256 default flip (GF-2b → OQ-9):** flip new-repo default from SHA-1+sha1dc to SHA-256 once the
  stock-client/tooling bar is met (post-Git-3.0) — a default-change, not a migration (hash-agnostic model).
- **Patch-id-chain anchor carry-over (GF-5 → R-6):** a thread follows a rebased hunk through a multi-commit
  rebase.
- **SCIP/LSIF "find usages" (GF-3 → R-3):** AST-aware code intelligence fed by CI-produced SCIP indices.
- **gitoxide server-side migration (OQ-1 → R-2):** per-op, gated on the capability-matrix spike + a
  protocol-compat + sandbox-escape re-drill — *iff* it clears.
- **In-browser conflict resolution (GF-6 → OQ-8)** and **External MCP endpoint (GF-9 → R-9, P6+Legal).**

**Work — world-scale hardening (the F6 surge family + the scheduled scale drills):**
- The monorepo ceiling benchmark; concurrent-merge linearizability under failover; the clone-storm shed; the
  cross-tenant fairness; prod-scale benchmarks (100k-PR list, monorepo ceiling); online-migration-under-load;
  restore-verify at cell scale.

**Git's contribution to the four whole-system E2E scenarios (master §2 M5, testing-strategy §2):**
- **E2E-1 PR context pane** (Git+CI+Issues+Knowledge+Refs+Search+Id+Notif) — git is the PR host + the
  reference producer.
- **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (the agent-native flagship) — git hosts the fix-PR;
  the `git.merge` HITL approval + the X-1/D-10 gate + `git.pr.merged` closing the issue via the Closes trailer.
- **E2E-3 Spec-to-ship traceability** — git provides the commit→PR→merge lineage (cold-reindex == live).

**Upstream deps:** M4 green (all five subsystems exist; the deterministic correctness drills green); the
floors in place to promote; Storage object-store BlobStore swap (11.2); the cross-cell bridge (12.6).

**Done gate (quantified):**
- **GIT-D4** (SCHED): grow a synthetic monorepo until partial-clone/sparse/bitmaps degrade → **documented v1
  ceiling** (measured, not guessed); clone/fetch p99 held below it.
- **GIT-D5** (SCHED): concurrent merges + force-push to one protected base_ref + DB-replica failover + node
  recovery mid-merge → **linearizable on the ref CAS; no split-brain; 0 lost merge**; update_seq monotonic +
  the fence honoured.
- **GIT-D6** (SCHED): 30× agent/CI clone surge on a hot repo → **human fetch p99 held**; agent/CI lane sheds
  (429 + Retry-After); **0 cross-tenant starvation**; CDN hit-rate.
- **E2E-1, E2E-2, E2E-3** green (their git slices); **STOR-D2 at cell scale** re-confirmed.

---

### M6-G10 — Dogfood: Myelin hosts its own repositories (the switch test)

**Band: M6.** The cheapest, most honest load generator is the platform's own development (master §2 M6).

**Work:**
- Migrate the Myelin monorepo onto Myelin git hosting; the build/test/lint/mutation pipeline becomes a Myelin
  CI pipeline (the gates run on the platform's own git commits).
- The roadmap + gap report live as Myelin issues + Knowledge; the every-incident-adds-a-drill loop files a
  Myelin issue + a reproducing git drill.

**Done gate (quantified):**
- **Git OQ-12 switch test** (SCHED): driven in a browser — could a GitHub user move to Myelin git hosting
  **without hitting a wall the old tool didn't have** (EI-01 §4)? Measured contrast + latency budgets +
  `render(parse(md)) === md` + overlays against the real anchor (design-language §8b).
- The Myelin self-hosting CI graph is green on the platform's own git commits; **no later-band git gate is
  red** (the truth-up pass confirms every PROVEN git row rests on a dated green artifact).

---

## 4. The contracts git must implement, by milestone

From the contract index (§4.2 above is the dependency list; this is git's **own implementation** obligations).

| Contract | What git implements | By milestone |
|---|---|---|
| 2.2/2.3 OutboxTx::emit + per-ref aggregate | the receive-pack → ref-CAS → outbox emit-in-one-tx | M3-G1 |
| 2.9 git.\* event tokens | git.ref.updated / git.pr.\* / git.review.\* taxonomy | M1 (register), M3-G1/G3 (emit) |
| 2.6 reindex-from-source / replay | replay for the check_status projection + code index + refs edges | M3-G7 (proven GIT-D3) |
| 4.9 Git ReBAC fragment | ref-glob + CODEOWNERS-as-relations + protected_push + approve_untrusted_ci + watcher | M1 (freeze), M3-G2 (live) |
| 4.3 list_objects consumer | SetExpr → SQL JOIN over repo.id/pr.id (no N+1) | M3-G5 |
| 5.1 ArtifactRef id grammar | commit/<repo>:<sha>, pr/<repo>:<n> stable canonical keys | M3-G3 |
| 5.2/5.6 resolve/project | project(ref, viewer) for git artifacts; the owner sub-anchor resolver | M3-G3 |
| 5.4/5.5 edges + typed-edge mirror | content inline nodes → refs.edge.created; PR/trailer lifecycle edges | M3-G3 |
| 5.7 #sub grammar + content-anchored line-range | mint comment-/thread-/L<a>-L<b> + the BLAKE3 4-state resolver | M3-G4 |
| **5.9 the X-1 CheckStatus consumer + gate** | check_status projection + run_attempt supersession + required-set policy + fork-endorsement + the merge-queue ci.result wait | M3-G4 (consumer), M4 (end-to-end with CI producer) |
| 6.3/6.5 declare_indexable + code projection | the git.\* code projection (path/symbols/literals/commit-msg) | M2 (register), M3-G5 (emit) |
| 7.3/7.6 humanise + define_notif_rule | git notification rules + summary template keys | M3-G8 |
| 8.1/8.4 ToolDefs + ToolHands::exec | git ToolDefs (merge=yes, open_pr=no) on the unified sandbox | M3-G6 |
| 9.1/9.2/9.4 durable workflow | the merge-queue workflow (parks on ci.result); maintenance ops | M3-G4 |
| 10.1/10.4 PersonalDataHolder + DSR | locate/export/rectify/restrict/erase over git+metadata | M3-G1 (register), M3-G7 (complete) |
| 10.6 history-rewrite audited op | the audited, rate-limited rewrite + invalidation fan-out | M3-G6/G7 |
| 10.9 the ONE erasure posture (by reference) | instantiate, never restate; pseudonymous-by-default + per-subject DEK | M3-G1/G7 |
| 11.2 BlobStore (pack tier) | local-NVMe floor (GF-1) → object-backed (M5) | M3-G1 (floor), M5-G9 (full) |
| 11.2 C3/C4 CDN class + trust-scoped cache | bundle-URI floor + fork:<pr_id> cache scope | M3-G2/G4 (floor), M5-G9 (full) |
| 11.4 per-subject DEK crypto-shred | bodies/titles + reflogs/bitmaps/pack-backup reach | M3-G1/G7 |
| 12.2/12.4 placement_of + residency_verify | repo→cell placement honouring residency; reject-if-leaving-region | M3-G2 |

---

## 5. The floors register (name the floor, name the follow-on — VISION §3, EI-04 §4)

Each floor ships **named**, tracked in the gap report with claimed/proven status; the follow-on band is fixed.
A floor masquerading as done is the only failure.

| # | Floor (what ships) | Ship band | Follow-on (the full answer) | Follow-on band | Trigger |
|---|---|---|---|---|---|
| GF-1 | **Local-disk packs** behind BlobStore; repos relocatable, never node-pinned | M3-G1 | Object-store-backed pack/delta + smart-transport + CDN class (R-1/OQ-4) | M5-G9 | the single-node ceiling measured (GIT-D4); EI-04 §3 |
| GF-2 | **Single-cell** primary + quorum-ack WAL replica set | M3-G1 | Cross-cell active replica sets within-EU; geo read-replicas | M5-G9 | cross-cell demand; the multi-cell bridge live |
| GF-2b | **SHA-1 + sha1dc** default / SHA-256 opt-in (hash-agnostic) | M3-G1 | Flip default to SHA-256 (OQ-9/R-5) | M5-G9 (measured) | post-Git-3.0 client/tooling bar met |
| GF-3 | **Trigram/lexical** code search (symbol/path/literal) | M3-G5 | AST-aware "find usages" via CI SCIP/LSIF (R-3) | M5-G9 | demand-triggered |
| GF-4 | **Large-but-normal monorepo** (partial-clone/sparse/bitmaps); no virtual FS | M3-G1 | Mononoke-class backend | M5+ (measured) | a tenant exceeds the GIT-D4 ceiling |
| GF-5 | **Per-pair fingerprint** diff-anchor (4-state) | M3-G4 | Patch-id-chain carry-over across multi-commit rebase (R-6) | M5-G9 | rebase carry-over demand |
| GF-6 | **Single-file web edit + commit**; no 3-way conflict editor | M3-G8 | In-browser conflict resolution (OQ-8) | M5+ (measured) | demand-triggered |
| GF-7 | **Pseudonymous-by-default + per-subject DEK shred + history-rewrite** | M3-G1/G7 | Art. 17-into-immutable-bytes lawful-basis residual = the ONE posture's `[OPEN — LEGAL]` (R-7) | parallel (Legal/DPO) | a body must be expunged; ratification |
| GF-8 | **Single-lane** serialised merge queue | M3-G4 | Speculative/parallel batched merge queue (OQ-5/R-4) | M5-G9 (measured) | throughput ceiling measured |
| GF-9 | **exposed_over_mcp flags set**; no external endpoint | M3-G6 | Platform MCP server + threat model (R-9) | M5+ / Legal | shared MCP work |
| Seam-floor | **X-1 gate built against a synthetic ci.check.updated emitter** | M3-G4 | Real CI producer wired; GIT-D10/CI-D8 end-to-end | M4 (co-gate) | CI's 5.9 producer lands |

**Two named spikes the roadmap schedules (not floors — investigations):** OQ-1 (gitoxide server-side capability
matrix, gates any gix-ward wire migration, M5) and OQ-10/R-8 (pseudonym enforcement mode: client-cooperative
sha-stable vs server-side rewrite-at-push — the *property* is decided, the default is the call, M3-G1 → Id).

---

## 6. The honest first-runnable / first-useful / production-hardened progression

- **First runnable (end of M3-G2):** you can `git clone`/`git push` over SSH or smart-HTTP-v2 against a
  single-cell, single-node deployment; pushes are authenticated, tenant-isolated, region-pinned, and **never
  lose an event** (GIT-D9 green). Commits are pseudonymous-by-default. No PR UI yet, no CI gate, no search —
  but the **substrate-correct git server exists and is drilled** (the data-loss + cross-tenant floors are
  green). This is a *floor named as a floor*: it stores code safely, it is not yet a product.

- **First useful (end of M3-G8 — the M3 band exit):** the full producer subsystem — PRs, reviews, inline
  threads (content-anchored, GIT-D7 green), the merge gate + single-lane merge queue gating on CheckStatus
  (GIT-D10 green against the synthetic producer), code search (GIT-D11 green), erasure-reaches-every-holder
  (GIT-D2 green), reindex-from-cold parity (GIT-D3 green), agent authors/reviewers, and a browser-driven Web
  UI. A team could host real repositories and review code. **Still floors:** local-disk packs, single-cell,
  single-lane queue, the X-1 seam only fully live once **CI lands in M4** (GIT-D10/CI-D8 end-to-end at the M4
  exit).

- **Production-hardened (end of M5-G9):** object-backed relocatable packs, cross-cell replication with proven
  failover (GIT-D5: linearizable, no split-brain, 0 lost merge), the clone-storm shed proven (GIT-D6: human
  lane holds, agents shed), the monorepo ceiling measured (GIT-D4), the speculative merge queue, and git's
  slices of the three whole-system E2E scenarios green. Restore-verify holds at cell scale. This is the
  world-scale git the platform promised.

- **The done-bar (M6-G10):** Myelin hosts its own repositories; the switch test passes driven in a browser; the
  self-hosting CI graph is green on the platform's own git commits; no earlier git gate is red.

---

## 7. Digest

**Milestones (each mapped to a master band):**
- **Pre-work (M1/M2):** freeze the Git ReBAC fragment + git.\* event tokens + holder tags; the design-system
  pass incl. the X-1 fork-trust/checks-panel/merge-queue affordances (pre-frontend, OQ-12).
- **M3-G1 (M3):** git object store + receive-pack + the silent-data-loss floor (one-tx ref-CAS+outbox) +
  pseudonymous-by-default commits (the data-model gate). Gate: GIT-D9, GIT-D1.
- **M3-G2 (M3):** SSH + smart-HTTP front door, authenticate/check, residency reject, ReBAC live, shed order.
  Gate: GIT-D8.
- **M3-G3 (M3):** PRs/reviews/inline-threads + the reference-graph edges + project(). Gate: render==md, edges.
- **M3-G4 (M3):** the merge gate + check_status projection (X-1 consumer) + fork-endorsement + the
  ci.result-waiting merge queue + content-anchored line ranges. Gate: GIT-D10 (synthetic), GIT-D7.
- **M3-G5 (M3):** the code projection for search + leak-free fast lists (SetExpr push-down). Gate: GIT-D11.
- **M3-G6 (M3):** code-executing git tools + agent authors/reviewers on the unified sandbox. Gate: AG-D4 green
  (upstream) + AG-D1/D2/D3/D5 honoured.
- **M3-G7 (M3):** erasure-reaches-every-holder + history-rewrite + reindex-from-cold parity. Gate: GIT-D2,
  GIT-D3.
- **M3-G8 (M3, band exit):** notifications + Web UI + CLI/API. Gate: NOTIF-D4-class; the M3 git exit aggregate.
- **M5-G9 (M5):** object-backed packs, cross-cell replication, speculative queue, SHA-256 flip, SCIP, the F6
  surge family + the E2E slices. Gate: GIT-D4, GIT-D5, GIT-D6, E2E-1/2/3, STOR-D2 at scale.
- **M6-G10 (M6):** dogfood + the switch test. Gate: Git OQ-12 switch test; self-hosting CI graph green.

**Floors + follow-ons (band → band):**
- Local-disk packs (M3) → object-backed packs (M5). Single-cell (M3) → cross-cell (M5). SHA-1+sha1dc (M3) →
  SHA-256 flip (M5). Trigram search (M3) → SCIP find-usages (M5). Per-pair anchor (M3) → patch-id-chain (M5).
  Single-lane queue (M3) → speculative queue (M5). Single-file web edit (M3) → in-browser conflict (M5+).
  Pseudonymous-by-default + DEK shred + history-rewrite (M3) → the `[OPEN — LEGAL]` lawful-basis residual
  (parallel/Legal). **X-1 gate vs synthetic producer (M3) → real CI producer end-to-end (M4).**

**Critical upstream dependencies (must exist + be green before the named git milestone):**
- **Outbox + envelope + restore-verify STOR-D1 (M0/M1)** → M3-G1 cannot write real data until restore-verify
  is green (the silent-data-loss floor).
- **Identity authenticate/check/list_objects/resolve_pseudonym + the ReBAC engine (M1)** → M3-G2/G5 (the wire,
  authz, leak-free lists) + M3-G1 (pseudonymous commits).
- **Tenancy partition + placement_of + residency_verify (M1)** → M3-G2 (region-pinned placement).
- **Refs #sub grammar + 4-step tombstone ladder + resolve/project (M2)** → M3-G3/G4 (line-range anchors,
  unfurls).
- **Durable workflow + SCHEDULE_AND_RUN_JOB + durable signal + timer wheel (M2)** → M3-G4 (the merge queue
  parks on ci.result for hours).
- **AG-D4 / CI-T1 sandbox-escape GATE green (M2)** → M3-G6 (no code-executing git tool runs until it is green).
- **CI's CheckStatus producer (5.9, M4)** → the X-1 seam goes live end-to-end; GIT-D10/CI-D8 is the M4 exit
  co-gate (git built the consumer in M3 against a synthetic producer).
