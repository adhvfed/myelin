# Git Hosting & Code Review — Detailed Architecture: Overview

> Phase: `04-subsystem-architectures/git-hosting/architecture` — **rewritten in Phase 5-B** against the
> RECONCILED shared layer. Canonical brief: [`VISION.md`](../../../../VISION.md) (never contradicted).
> Binding doctrine: [`external-insights/02-platform-substrate.md`](../../../../external-insights/02-platform-substrate.md),
> [`external-insights/04-hard-problems.md`](../../../../external-insights/04-hard-problems.md),
> [`external-insights/05-ux-and-design.md`](../../../../external-insights/05-ux-and-design.md).
> **Build-to surface (FROZEN):** [`05-refined-shared-systems-architecture/contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md)
> + the reconciliation rationale [`00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (X-1..X-7, OQ-A..OQ-L; Part 4 per-system punch list).
> Design record (PRESERVED, not rewritten): [`../design/`](../design/) + [`../sketches/`](../sketches/).
> Date: 2026-06-19.

---

## 0. Document map

| Doc | Covers |
|---|---|
| `00-overview.md` (this) | **Changes vs the Phase-4 first pass** (§0.1); role; owns-vs-delegates; component map; floors register; inherited non-negotiables. |
| [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) | Language/DB choice (carried forward + confirmed); git-core embed (TE-8); SHA (TE-23); the full data model — git object tier, reftable-on-OLTP, hosting OLTP — **now with the frozen `check_status` projection, content-anchored line ranges, and `<PROJECTKEY>-<seqno>`-style ref-grammar conformance.** |
| [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) | Smart-transport; sandboxed `receive-pack` + in-process Rust policy; ref-store CAS; replication (TE-24); GC/repack; diff-anchoring as the **content-fingerprint resolver (OQ-D)**; **the merge gate + merge queue implementing the X-1 CheckStatus consumer + run_attempt supersession + fork-endorsement + the `ci.result` durable-signal wait**; forks; monorepo; code projection. |
| [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) | The complete `git.*` taxonomy + consumed events (incl. **`ci.check.updated`/`ci.result`**); every glue contract against the **frozen** shapes: ArtifactRef + the unified `#sub` grammar, `project`, `replay`, outbox envelope, Identity `check`/`list_objects` **SetExpr push-down** + the frozen ReBAC fragment (`approve_untrusted_ci`), `PersonalDataHolder`, `ToolDef`s with the **frozen `requires_approval` defaults**, reserve/settle. |
| [`04-views-cli-and-api.md`](./04-views-cli-and-api.md) | Views (IA + flows + states, consuming `design/`); the two CLI surfaces; the HTTP/RPC + agent-tool API. |
| [`05-hard-problems.md`](./05-hard-problems.md) | Each subsystem hard problem resolved, cited prior art, named floors. |
| [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md) | **How this subsystem now IMPLEMENTS the frozen reconciled contracts** (X-1 CheckStatus consumer + merge gate; the `#sub` grammar; the `list_objects` Filter; the ONE erasure posture; REF-3 vs human key; trust-scoped cache; CDN clone class; mirror gate) + any RESIDUAL request for Phase 6. |
| [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md) | Quantified drills owed (D-1…D-11) + open questions for Phase 6. |

---

## 0.1 Changes vs the Phase-4 first pass (the reconciliation deltas absorbed)

The Phase-4 first pass was sound and most of it carries forward unchanged. Reconciliation (Phase 5) froze
several contracts that the first pass had left as open requests or sketched loosely. This rewrite conforms
to the **frozen** shapes. The deltas, each with its driver:

| # | What changed | Driver (frozen contract) |
|---|---|---|
| Δ1 | **The Git↔CI check seam is now a frozen contract, not an open question (was OQ-3).** The first pass had a thin `check_status` table keyed `(repo, commit_oid, check_name)` with a free `conclusion`. It is **replaced** by the frozen **`CheckStatus` fact** (contract 5.9) keyed `(commit_oid, context)`, carrying `state`/`required`/`run`/`run_attempt`/`trust_tier`/`details_ref`/`summary`/`cost_settled`. Git is the **consumer + gate**; CI is the producer. Git owns the `check_status` projection table, the **monotonic `run_attempt` supersession** rule, the **branch-protection `required`-set policy** (Git decides which contexts gate), and the **fork-endorsement** flow. Git **never calls CI synchronously**; it reads its own projection and **reads `trust_tier` off the fact** (never recomputes trust). | X-1 / OQ-A, contract 5.9 |
| Δ2 | **The merge queue now waits on the rollup `ci.result` durable signal**, distinct from the per-context `ci.check.updated` events, via `SCHEDULE_AND_RUN_JOB` long-park + `wait_for_signal(idem_key)`. The first pass said "woken by `ci.result`/`approval` signals" generically; now the exact two-channel split (events drive the projection/UI, the single `ci.result` rollup drives the queue resume) and the per-effect `idem_key` are pinned. | X-1 + OQ-F, contracts 9.1/9.2/9.4 |
| Δ3 | **Untrusted-fork CI is `neutral` for gating until endorsed.** A check whose `trust_tier = untrusted_fork` is recorded but **cannot satisfy a `required` context by itself**; the gate treats it as neutral until a maintainer `check(subject, approve_untrusted_ci, repo)` endorses it OR the context is re-run under `trust_tier = trusted`. This poisoned-pipeline defence was implicit in the first pass; it is now an explicit, frozen flow riding the `approve_untrusted_ci` ReBAC relation. | X-1, contract 4.9/5.9 |
| Δ4 | **Content-anchored line ranges are now the frozen `#sub` resolver (was a loosely-described `outdated` fallback).** Git mints `#L<a>-L<b>` storing a **BLAKE3 content fingerprint** of the anchored lines + a context window + the mint-time blob oid, and resolves through the unified 4-state ladder **exact / rebased(moved) / partial(outdated) / tombstone(content_gone)**. The first pass had `anchor_blob_oid` + an `outdated` boolean; the rewrite adds the fingerprint and aligns the resolution states to the one Refs ladder (contract 5.7). | X-4 / OQ-D, contract 5.7 |
| Δ5 | **The `list_objects` push-down is the frozen `SetExpr`, lowered to a SQL JOIN over Git's own id column.** The first pass referenced `list_objects` as a pre-filter; the rewrite pins the **`Ids \| Filter{set_expr, zookie}`** result, the `via_column` lowering (`repo.id` / `pr.id`), and the JOIN against Identity's per-tenant authz reverse index — no N+1, no post-filter. | OQ-E, contract 4.3 |
| Δ6 | **The ONE platform erasure posture is now instantiated by reference, not restated.** The first pass authored a Git-local HP-7 erasure write-up. The rewrite keeps the Git-specific *mechanism* (pseudonymous-commit-by-default, per-subject DEK shred, history-rewrite path) but states the *residual lawful-basis posture* by reference to `00-reconciliation §X-7` / contract 10.9 (pseudonymous-by-default + history-rewrite + lawful-basis residual), not as a fifth restatement. | X-7 / OQ-G, contract 10.9 |
| Δ7 | **REF-3 vs human-key reconciled (ArtifactRef id grammar).** The Git ArtifactRef id segment is the **stable mintable key the subsystem owns**: `pr/<repo>:<n>`, `commit/<repo>:<sha>` — the stored canonical key, never a render-time display form. (Issues' `<PROJECTKEY>-<seqno>` is the parallel decision; Git's commit sha / PR number is *already* its stable canonical key.) Pinned to align with REF-3 (display keys are render-time, never the stored link). | REF-3, contract 5.1 |
| Δ8 | **`requires_approval` defaults are now the frozen table** (X-6): `git.merge` = **yes**, `open_pr` = **no**; the four uniform sandbox guarantees (cost gate / per-run-token attribution / HITL withhold / isolation floor+drill) are inherited by construction for any Git tool that executes code (history-rewrite, SCIP indexing). | X-6, contracts 8.1/8.4 |
| Δ9 | **New storage seams pinned:** the within-EU **CDN clone/bundle blob class** (11.2 C3) for clone-storm; the **trust-tier/branch-scoped cache namespaces** (11.2 C4 — a fork write cannot reach the trusted cache scope); **per-subject DEK** for PR/review/comment bodies + the crypto-shred reach into reflogs/bitmaps/pack backups (11.4); the **outbound push-mirror residency gate** (10.5) and **history-rewrite as an audited op with fork/mirror/clone-cache invalidation fan-out** (10.6). The first pass requested these; they are now frozen and consumed. | recon §8/§9/§10, contracts 11.2/11.4/10.5/10.6 |
| Δ10 | **File renames:** the old `06-shared-system-change-requests.md` becomes **`06-reconciliation-compliance.md`** (how this subsystem implements the frozen contracts + the residual Phase-6 asks). |

No ADR was reversed; no Phase-4 *decision* was overturned. The language/DB choice (Rust + Postgres + the
object tier), the git-core embed call (canonical `git` for the wire, `gix`/`libgit2` in-process for reads),
the SHA call (hash-agnostic, SHA-1+`sha1dc` default / SHA-256 opt-in), and the replication call (the DB
ref-store transaction is the linearisation point) **all stand** — reconciliation did not force a change to
any of them (recon §0: "no ADR is reversed").

---

## 1. Role & responsibilities

Git hosting is the **system of record for source code and its history** and the gravitational centre of
Myelin's engineering side (Phase-1 §1, Phase-2 §1). Its differentiator is **not the git server** — every
competitor has a competent one — but that this one sits on Myelin's unified
identity/permission/event/reference fabric and is **agent-native** (agents are first-class
authors/reviewers, legible and bounded), all **EU-sovereign and GDPR-by-construction**.

### 1.1 What it OWNS (core competency)

- **The git object store + serving core**: blobs/trees/commits/tags, refs, packfiles + delta compression,
  reachability acceleration (commit-graph, reachability bitmaps, multi-pack-index), GC/repack,
  partial-clone/sparse/shallow serving, and the **git wire protocol** (smart-HTTP protocol-v2 + SSH).
- **Per-ref ordering at push QPS** — the aggregate for `git.ref.updated` is the **ref**, not the repo
  (contract 2.3); the ref-update transaction is the linearisation point.
- **Hosting-layer domain entities** not in git itself: Repository (visibility/default-branch/tenant
  binding), Fork/network, the **Pull Request** lifecycle, **Reviews + inline comment threads** (with
  diff-anchoring), **Branch-protection rulesets**, CODEOWNERS, deploy-key bindings.
- **The merge gate — *the place "what is allowed to land" is decided*** — and, per X-1, the **consumer
  side of the Git↔CI check seam**: Git owns the `check_status` projection table (keyed `(commit_oid,
  context)`), the `run_attempt` supersession rule, **which contexts are `required`** (branch-protection
  policy — Git decides, CI only reports facts), the **fork-endorsement** gate, the merge queue, and the
  agent-vs-human merge policy. Git **never synchronously calls CI**; it reads its own projection.
- **The indexable CODE PROJECTION** (path/symbols/literals/commit-message per blob/ref) emitted for Search
  code-search v1 (contract 6.3/6.5) — git hosting owns *what* to index.
- **`git`-namespace `ArtifactRef`s** down to sub-artifact granularity (the frozen unified `#sub` grammar,
  contract 5.7), including the **content-anchored `#L<a>-L<b>` line range**, and the complete `git.*` event
  taxonomy under the Bus §6 grammar.
- **Its erasure obligations** as a `PersonalDataHolder` (holder H1) — the hardest in the platform —
  including **pseudonymous-commit-by-default (GIT-1)** as a commit-time prerequisite, and the
  **history-rewrite** path. The *residual lawful-basis* is the ONE platform posture (contract 10.9 / recon
  §X-7), instantiated here by reference, not restated.
- **The pack tier kept object-backable/relocatable** (STOR-5 / Storage §3.5): repos are never node-pinned.

### 1.2 What it DELEGATES to shared systems (ADR-13; no cross-DB, ADR-01)

| Concern | Delegated to | Git hosting still owns |
|---|---|---|
| Who a principal is; SSH-key/token/OAuth auth; org/team model; **the pseudonym map**; **`approve_untrusted_ci`** | **Identity** (`authenticate`/`check`/`list_objects`/`resolve_pseudonym`/`erase`) | the Git ReBAC fragment (ref-glob relations + CODEOWNERS-as-relations + `approve_untrusted_ci`); the SSH/HTTPS front door |
| **Producing `CheckStatus` facts** (`ci.check.updated`) + the `ci.result` rollup signal + stamping `trust_tier`/`run_attempt`/`details_ref` | **CI** (the producer of contract 5.9) | the `check_status` projection table, the supersession rule, the `required`-set policy, the fork-endorsement gate, the merge queue |
| Emitting/consuming events | **Event Bus** (`OutboxTx::emit`) | the receive-pack → outbox path; per-ref ordering |
| Commit↔issue↔doc↔run edges, backlinks, unfurls, the `#sub` resolver ladder | **Reference Graph** (`ArtifactRef`/`resolve`/`backlinks`) | producing edges from trailers/PR links; `project`; minting stable `#sub` ids + the line-range content fingerprint |
| Code/PR/comment index + query | **Search** (`query`/`declare_indexable`) | the code projection + incremental update on push |
| Durable bytes (LFS, packs-as-blobs, bundles, backups, **CDN clone class**, **trust-scoped caches**) | **Storage** (`BlobStore`, KMS) | the LFS batch protocol, pack/delta management, residency tags |
| Notification delivery + humanisation | **Notifications** (`humanise`/inbox) | which events are notifiable + targets (via Signals); the `summary` template keys |
| Agent authors/reviewers; trigger dispatch; plan-then-apply; the sandbox | **Agent Fabric** (`ToolSurface`/`EffectApi`/`ToolHands`) | its `ToolDef`s + the events that drive triggers |
| Long-running / human-gated flows (merge-queue waits, auto-merge-when-green, HITL gate) | **Durable Workflow** (`DurableExecutor`/signals/timers) | the merge-queue state-machine semantics |
| DSR fan-out, KMS/crypto-shred, the tamper-evident audit log, **the history-rewrite audited op + invalidation fan-out**, **the mirror residency gate** | **GDPR/Audit + Tenancy** (`PersonalDataHolder` orchestration / `transfer_allowed`) | implementing `locate/export/rectify/restrict/erase` over git+metadata; initiating the history-rewrite op |
| Cell placement, region-pinning, discovery | **Tenancy/control plane** (`discover`/`placement_of`/`residency_verify`) | repo→cell placement honouring residency; rejecting any route that leaves region |

**Hard rule (ADR-01/13):** no subsystem reads git hosting's DB; git hosting reads no other subsystem's DB.
All cross-subsystem reads go through `ArtifactRef` resolution + the owning subsystem's `project` API,
permission-filtered per viewer. The Git↔CI seam is no exception: CI **emits** the `CheckStatus` fact over
the bus; Git **reads its own projection** of it.

---

## 2. Internal component architecture

The subsystem keeps the Phase-2 four-tier shape — a **stateless front door**, a **stateful serving tier**,
a **metadata control plane**, and an **async event/index path** — over the reconciled substrate
(`serve(AppSpec)`, the outbox, the three-surface topology, the resilient client). Every box is a thin shell
over `myelin-substrate` (ADR-01).

```
            ┌──────────────────────── CLIENTS ────────────────────────┐
            │  git wire (SSH / smart-HTTP v2) · Web UI · myelin CLI ·  │
            │  internal RPC · MCP (external agents, later)             │
            └───────────────┬─────────────────────────────────────────┘
                            ▼
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ (A) GIT FRONT DOOR / ROUTER  (stateless, per-cell, region-pinned)         │
 │   Id.authenticate → Principal · Id.check per-action gate · discover/       │
 │   placement_of(repo) → backend node(s) · residency reject-if-leaving-region│
 │   · streams packs (no full buffering) · ADR-16 protected-human-lane shed   │
 │   (per-surface budgets, OQ-K) · SSH + smart-HTTP-v2 · liveness≠readiness    │
 └───────────────┬───────────────────────────────────┬──────────────────────┘
                 ▼ (git transactions)                 ▼ (PR/review/API/UI/RPC)
 ┌───────────────────────────────────┐  ┌─────────────────────────────────────┐
 │ (B) REPO SERVING TIER (stateful)  │  │ (C) HOSTING CONTROL PLANE (OLTP)    │
 │  git-core engine: upload-pack /   │  │  PR / review / comment-thread /     │
 │  receive-pack · pack/delta · GC/  │  │  repo / fork / ruleset / merge-queue│
 │  repack · commit-graph + bitmaps  │  │  rows · branch-protection evaluator │
 │  + MIDX · reftable-on-OLTP ref    │  │  + the required-set policy · CODE-  │
 │  store · partial-clone / sparse   │  │  OWNERS resolver · DIFF-ANCHOR svc  │
 │  · IN-PROCESS receive-pack policy │  │  · the CHECK_STATUS PROJECTION +    │
 │  engine (pre-receive) + outbox    │  │  supersession + fork-endorsement ·  │
 │  emit (post-receive, same tx)     │  │  CODE-PROJECTION emitter · project()│
 └───────┬───────────────┬───────────┘  └───────┬───────────────────┬─────────┘
         │ packs/LFS/     │ outbox events        │ outbox events     │ reads
         ▼ bundles (Blob) ▼                      ▼                   │ (project/
 ┌──────────────┐   ┌────────────────────────────────────────┐       │  resolve)
 │ STORAGE      │   │        EVENT BUS (JetStream-class)      │◄──────┘
 │ BlobStore +  │   │  git.ref.updated · git.pr.* · git.review│  consumes:
 │ KMS · CDN    │   │  .* (envelope + outbox + per-ref order) │  ci.check.updated,
 │ clone class ·│   └──┬──────┬──────┬──────┬──────┬──────────┘  ci.result (X-1)
 │ trust-scoped │      ▼      ▼      ▼      ▼      ▼
 │ caches       │    REFS  SEARCH  AGENTS  NOTIF  CI / OLAP / AUDIT
 └──────────────┘                          ▲ ci.result signal → merge-queue workflow
```

- **(A) Front door.** SSH + smart-HTTP-v2. Authenticates a `Principal` via Id (SSH pubkey/deploy-key/PAT/
  OAuth — contract 4.1 machine-identity resolution), runs the per-action `Id.check`, resolves
  `placement_of(repo) → cell + backend node(s)` (contract 12.2, repo-granular + relocatable), **rejects
  any route that would leave the region** (ADR-11), and *streams* the transaction without buffering whole
  packs. The protected-human-lane shed order (ADR-16, the OQ-K per-surface budget floor: speculative →
  batch/CI → agent → human-last) lives here. Stateless; horizontal; liveness must not check deps,
  readiness gates on backend reachability.
- **(B) Serving tier (stateful).** The git core via the layered **`GitCore`** seam: **wire serving
  (`upload-pack`/`receive-pack`/`ls-refs`) + maintenance run as sandboxed canonical `git`** in v1 (the
  Stage-1-verified TE-8 position — `gix` has no server-side serving), while **`gix` (libgit2 fallback) runs
  in-process for read/diff/blame + the code projection**. Pack/delta storage, reachability acceleration,
  GC/repack, the **reftable-on-OLTP ref store**, partial-clone/sparse serving live here. **Push policy runs
  in-process in Rust**: a sandboxed `git receive-pack` ingests the pack into a quarantine, *our* Rust
  evaluates branch protection / secret-scan / size / agent / pseudonymity rules — *reject before the ref
  moves* — and *our* code does the ref CAS + the outbox insert **in the same DB transaction** (BUS-2).
- **(C) Control plane (OLTP).** Postgres for everything that is not a git object: PR/review/comment/repo/
  fork/ruleset/merge-queue rows; the branch-protection evaluator **and its `required`-set policy**; the
  CODEOWNERS resolver; the **diff-anchor service**; the **`check_status` projection** (the X-1 consumer:
  applies `run_attempt` supersession, holds exactly one current row per `(commit_oid, context)`, evaluates
  trust posture, runs the fork-endorsement gate); the **code-projection emitter**; and the
  **`project(ref, viewer)`** API. One DB per service, RLS tenant-scoped, per-tenant envelope-encrypted (with
  per-subject DEKs for free-text bodies), forward-only migrations, auto-registered as a `PersonalDataHolder`.
- **(D) Async path.** Off the bus: Search code-projection indexing, Refs edge creation, Notification
  routing, the OLAP feed, the agent trigger/dispatch tier, the audit consumer, **and the `ci.check.updated`
  consumer feeding the `check_status` projection**. All idempotent on `event_id`. Never in the synchronous
  push/PR write path. The **merge-queue durable workflow** waits on the rollup `ci.result` signal (it holds
  no runtime while CI runs).

**Two-transport discipline (ADR-04).** Durable control/domain events (`git.ref.updated`, `git.pr.*`,
`git.review.*`) ride the durable bus. Git hosting has no per-line firehose of its own (unlike CI logs or
chat presence); its high-volume path is **streaming clone/fetch byte transfer**, which stays on the git
wire/object tier and **never** touches the durable bus.

---

## 3. The floors register (named partials + their follow-ons — VISION §3, gap report E-3)

Each dated 2026-06-19, named with its follow-on owner. (Reconciliation added no new floors here; it froze
the contracts these floors build against.)

| # | Floor (what v1 ships) | Follow-on | Owner |
|---|---|---|---|
| GF-1 | **Object-backed packs** — v1 runs packs on local NVMe behind the `BlobStore` trait; repos are **relocatable, never node-pinned** (STOR-5 / Storage §3.5 DECIDED). | Object-store-backed pack/delta management + smart-transport over `BlobStore`. | Git P6 / Storage |
| GF-2 | **Replication** — the **DB ref-store transaction is the linearisation point** (per-ref CAS); pack durability via **primary + quorum-ack WAL-streamed replica set**. Single-cell in v1. | Cross-cell active replica sets; geo read-replicas within-EU; object-store-backed pack relocation. | Git P6 / control plane |
| GF-2b | **SHA-256** — default new repos are **SHA-1 + `sha1dc`**; SHA-256 is **opt-in per repo**; the data model is **hash-agnostic**. | Flip the default to SHA-256 once the client/tooling ecosystem matures (post-Git-3.0); audited `migrate --to sha256`. | Git P6 (measured) |
| GF-3 | **Code search** — symbol/path/literal/trigram-grade projection (Search §4.4). | AST-aware "find usages" via **CI-produced SCIP/LSIF** (Search contract 6.5 follow-on, named). | Git P6 + Search + CI |
| GF-4 | **Monorepo** — large-but-normal monorepos via partial-clone/sparse/commit-graph/bitmaps; **not** a Mononoke-class virtual FS. | A Mononoke-class backend if a tenant exceeds the benchmarked ceiling (`05 §HP-3`). | Git P6 (measured) |
| GF-5 | **Diff-anchoring** — content-fingerprint blob-diff remap with the four-state ladder; `partial→outdated` fallback (OQ-D). | Rebase-aware "changes since you last reviewed" via patch-id chains (the `rebased→moved` carry-over hardened). | Git P6 |
| GF-6 | **In-UI web editing** — view + single-file web edit + commit; **no** 3-way merge conflict editor in v1. | In-browser conflict resolution for simple cases. | Git P6 (measured) |
| GF-7 | **Git-history erasure** — pseudonymous-commit-by-default (GIT-1) makes erasure usually a pseudonym-map delete; **history-rewrite** (the audited op, contract 10.6) is the supported disruptive path for PII-in-content. | The Art. 17 reach into immutable commit bytes is the ONE platform posture's **`[OPEN — LEGAL]`** residual (contract 10.9 / recon §X-7). | Git P6 + Legal/DPO |
| GF-8 | **Merge queue** — single-lane serialised durable workflow in v1 (correctness first). | Speculative/parallel batched merge-queue (GitHub-merge-queue-class). | Git P6 (measured) |
| GF-9 | **External MCP** — `exposed_over_mcp` flags set; external endpoint deferred to the platform's shared MCP work. | Platform MCP server + threat model. | P6 + Legal |

---

## 4. The non-negotiables this subsystem inherits (substrate, never re-litigated)

1. `(tenant, region)` is the first column / partition key of every table and every git object placement;
   tenant comes from the verified token, never the URL (ID-3).
2. Every store is residency-pinned, per-tenant envelope-encrypted (per-subject DEK for free-text bodies),
   crypto-shred-capable, and a `PersonalDataHolder` (auto-registered by `serve`, contract 1.4).
3. No cross-DB reads (`no-cross-db` lint); interaction via contracts only.
4. The transactional **outbox is the only emit path** (`no-raw-publish` lint); no fire-and-forget.
5. Causality is nested + derived correct-by-construction (`correlation_id`/`causation_id`/`depth`).
6. Reindex-from-source is the only recovery path for the derived stores fed by git (Search code index, Refs
   edges, **and the `check_status` projection itself**, which rebuilds by `replay` of CI's `ci.check.updated`).
7. The three-surface topology (public gateway / internal RPC / metrics-health); public↔internal is a
   security boundary; liveness ≠ readiness.
8. Every code-executing tool inherits the **four uniform sandbox guarantees** (X-6): reserve/settle cost
   gate, per-run attenuated token, HITL withhold, isolation floor + the real-kernel escape drill.

See [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) for the concrete implementation
of every glue contract and [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md) for the
contract-by-contract conformance map.
