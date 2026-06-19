# 06 — Reconciliation Compliance: How Git Hosting Implements the Frozen Contracts

> Phase 5-B. This replaces the Phase-4 first-pass `06-shared-system-change-requests.md`. That file asked the
> shared systems for things; reconciliation **answered** every ask (it froze the contracts). This file is the
> inverse map: **how this subsystem now IMPLEMENTS the frozen reconciled contracts**, contract by contract,
> plus the **residual requests carried to Phase 6**. Build-to surface:
> [`contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md);
> rationale: [`00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md).
> Date: 2026-06-19.

---

## 1. The Git punch-list items (recon Part 4) — implementation map

The reconciliation Part-4 per-system punch list names exactly five things Git "must now build to". Each is
implemented as follows:

| Punch-list item (recon §4 "Git") | Where implemented | Contract |
|---|---|---|
| **`check_status` projection + supersession + branch-protection `required`-set + fork-endorsement (X-1)** | `02 §6.1` (the `run_attempt`-monotonic supersession consumer), `02 §6.2` (the gate reading Git's own `required_contexts` policy), `02 §6.3` (the `approve_untrusted_ci` fork-endorsement + `untrusted_fork`-is-neutral rule), `01 §4.3` (`check_status` + `ruleset.required_contexts` schema) | 5.9, 4.9 |
| **Pseudonymous commits (GIT-1)** | `02 §2` (push-policy enforces the pseudonym), `01 §4` (`author_pseudonym`/`reviewer_pseudonym`/`pusher_pseudonym`, never name/email), `03 §6` (the pseudonym-map shred is DSR step 1) | 4.8 |
| **Content-anchored line-range fingerprints (OQ-D)** | `02 §5` (the BLAKE3-fingerprint 4-state resolver), `01 §4.4` (the stored `anchor_fingerprint` + `anchor_state`), `03 §2` (the `#L<a>-L<b>` `#sub` mint) | 5.7 |
| **Object-backing seam (STOR-5)** | `01 §4.1` + `02 §4` (packs behind `BlobStore`, repos relocatable / never node-pinned; the object-backed swap is a `BlobStore`-impl change) | 11.2 / Storage §3.5 |
| **(implicit) the CDN clone class + trust-scoped caches + mirror gate (recon §8/§10)** | `02 §1.4` (CDN bundle-URI), `02 §6.3`/`02 §7` (trust-scoped fork cache), `04 §2.3` (the mirror residency gate) | 11.2 C3/C4, 10.5 |

---

## 2. The frozen cross-system contracts Git CONSUMES — conformance

### 2.1 The X-1 `CheckStatus` seam (contract 5.9) — CONSUMER + GATE

- **Consumes `ci.check.updated`** carrying the frozen `CheckStatus{repo, commit_oid, context, state,
  required, run, run_attempt, trust_tier, details_ref, summary, cost_settled, ...}`. Idempotent on
  `event_id`; applies **monotonic `run_attempt` supersession** (`>=` supersedes, `<` is dropped as stale
  re-delivery) into the `check_status` projection keyed `(commit_oid, context)` (`02 §6.1`).
- **Git owns the `required`-set policy** (`ruleset.required_contexts`) — CI's `required` flag on the fact is
  advisory; Git decides which contexts gate this `base_ref` (`02 §6.2`).
- **Git reads `trust_tier` off the fact, never recomputes it** (`02 §6.3`). An `untrusted_fork` success is
  **neutral for gating** until endorsed via `check(subject, approve_untrusted_ci, repo)` or re-run trusted.
- **Git never synchronously calls CI** — it reads its own projection (acyclic dependency, EI-02 §3).
- **The merge queue** is a durable workflow per target ref waking on the rollup **`ci.result` signal** via
  the `SCHEDULE_AND_RUN_JOB` long-park idiom (`02 §6.4`), idempotent on the `merge_attempt_id` `idem_token`.
- `cost_settled` gates "is this check final" (the reserve/settle bookend is visible to the gate).

### 2.2 The unified `#sub` grammar (contract 5.7) — Git mints `comment-`/`thread-`/`L<a>-L<b>`

- Git mints **stable opaque** `#comment-<id>` / `#thread-<id>` (immutable across edits) and the
  **content-anchored** `#L<a>-L<b>` (BLAKE3 fingerprint). Git is the **owner's sub-anchor resolver** the Refs
  4-step ladder calls; it returns `LIVE/MOVED/OUTDATED/GONE` and Refs maps to projection/flag/tombstone. The
  `check-<context>`/`step-<n>` kinds are CI's; Git renders a `details_ref` as a link, never reads CI's DB
  (`03 §2`).

### 2.3 `list_objects` `SetExpr` push-down (contract 4.3, OQ-E)

- Git's query compiler lowers the `SetExpr` to a SQL JOIN over Git's own id columns (`repo.id` / `pr.id`)
  against Identity's per-tenant authz reverse index — one query, no N+1, no post-filter; for repo/PR lists,
  the context-pane pre-filter, and Search code-search (`03 §5.3`, `02 §10`).

### 2.4 The Git ReBAC fragment (contract 4.9) — frozen, incl. `approve_untrusted_ci`

- Declared at build time (`03 §5.2`): ref-glob-scoped relations, CODEOWNERS-as-relations, `protected_push`,
  the **`approve_untrusted_ci`** relation (X-1), and a `watcher` relation per watchable type (Notif
  read-fanout). Id owns the engine; Git never invents object ids.

### 2.5 `myelin-content` (contract 13.1) — Git consumes the strict subset

- PR/review/comment bodies use the frozen `myelin-content` markdown-subset + the three structured inline
  nodes (`mention`/`artifact_ref`/`embed`) which are the **producers of `refs.edge.created`** (so
  `Closes <ISSUEKEY>` / `@alice` / embeds produce edges uniformly — `03 §1.1`/Refs §4.1). Git authors no node
  type Knowledge did not freeze.

### 2.6 The ONE erasure posture (contract 10.9, X-7) — instantiated by reference

- Git states the residual **by reference** to `00-reconciliation §X-7`, not as a fifth restatement (`03 §6.2`,
  `05 §HP-7`). Git's mechanism half (pseudonymous-by-default + per-subject DEK shred) + its residual levers
  (history-rewrite audited op + the lawful-basis limit) are the platform posture's Git instantiation.

### 2.7 ArtifactRef id grammar / REF-3 (contract 5.1)

- Git's `<id>` segment is the **stable mintable canonical key** (commit sha, PR number, repo id) — the stored
  link — never a render-time display form (REF-3: display keys are render-time). Git needs no `<PROJECTKEY>-
  <seqno>` reconciliation; its sha / PR-number is already its stable key (`03 §2`).

### 2.8 `requires_approval` defaults + the four uniform sandbox guarantees (contracts 8.1/8.4, X-6)

- The frozen defaults: `git.merge` = **yes**, `git.open_pr` = **no** (`03 §7`). Any code-executing Git tool
  (history-rewrite, SCIP indexing) inherits the four uniform guarantees by construction (reserve/settle
  cost gate, per-run token attribution, HITL withhold, isolation floor + the real-kernel escape drill) —
  Git does not re-implement them; `ToolHands::exec` is the CI runner's `kind=agent` job.

### 2.9 Storage seams (contracts 11.2/11.4/11.8) + GDPR/Tenancy (10.5/10.6, 12.2)

- **Within-EU CDN clone/bundle class** (11.2 C3) for clone-storm (`02 §1.4`); **trust-tier/branch-scoped
  cache namespaces** (11.2 C4 — a fork write cannot reach the trusted cache scope, `02 §6.3`/`02 §7`);
  **per-subject DEK** for PR/review/comment bodies + crypto-shred reach into reflogs/bitmaps/pack backups
  (11.4, `03 §6`); **outbound push-mirror residency gate** (10.5 `transfer_allowed`, `04 §2.3`);
  **history-rewrite as an audited op with fork/mirror/clone-cache invalidation fan-out** (10.6, `02 §8`/`03
  §6.2`); **repo-granular `placement_of` + relocatable repos** (12.2, `02 §4`/`02 §10`); **cell-local
  cross-cell `resolve`** (5.2 / OQ-I, `03 §3`).

### 2.10 Durable workflow + reserve/settle (contracts 9.1/9.2/9.4, 11.7)

- The merge queue + maintenance ops are durable workflows (`02 §6.4`/`02 §8`); `SCHEDULE_AND_RUN_JOB` +
  per-effect `idem_key` (OQ-F) for the `ci.result` wait and batch HITL; reserve/settle fronts every
  spend-bearing activity (`03 §8`).

---

## 3. Residual requests carried to Phase 6 (the genuinely-not-yet-built)

These are not contract gaps (the contracts are frozen) — they are **named follow-ons / spikes** the Phase-6
roadmap must schedule:

| # | Residual | Owner | Tracked as |
|---|---|---|---|
| R-1 | **Object-backed pack/delta management over `BlobStore`** + the smart-transport read path from object-tier blobs (the STOR-5 *implementation*; the seam is frozen, the impl is the Git P6 deliverable). | Git P6 + Storage | GF-1 / OQ-4 |
| R-2 | **gitoxide server-side capability matrix spike** — which wire/maintenance ops can move off `ShellGitCore` to a `gix` server (gating any migration). | Git P6 | OQ-1 |
| R-3 | **SCIP/LSIF "find usages"** code-intelligence index input from CI (the Search 6.5 follow-on). | Git P6 + Search + CI | GF-3 |
| R-4 | **Speculative/parallel merge-queue batching** — the measured promotion trigger from single-lane. | Git P6 (measured) | GF-8 / OQ-5 |
| R-5 | **SHA-256-default flip trigger** — the measured stock-client + tooling compatibility bar (post-Git-3.0). | Git P6 (measured) | GF-2b / OQ-9 |
| R-6 | **Patch-id-chain anchor carry-over** across a multi-commit rebase (hardening `rebased→MOVED`). | Git P6 | GF-5 |
| R-7 | **`[OPEN — LEGAL]`** — the Art. 17 reach into immutable git bytes + the residual lawful-basis limit, ratified by counsel/DPO as the ONE platform statement (contract 10.9). | Legal/DPO | recon §X-7 / GF-7 |
| R-8 | **Pseudonym enforcement mode** — client-cooperative (sha-stable) vs server-side rewrite-at-push (guaranteed, sha-shifting) as the per-repo default (the *property* is decided; the enforcement default is the call). | Git P6 + Id | OQ-10 |
| R-9 | **External MCP endpoint** — `exposed_over_mcp` flags are set; the external server + threat model is the platform's shared MCP work. | P6 + Legal | GF-9 |

No residual is a contract change. Every contract Git depends on is **frozen** in the Phase-5 index; these are
implementation depth + measured-promotion + legal-ratification items.
