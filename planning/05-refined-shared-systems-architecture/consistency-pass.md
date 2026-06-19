# Phase 5 — Final Consistency Pass (the drift verification)

> Phase: `05-refined-shared-systems-architecture`. Purpose: a **final consistency verification** that each
> rewritten subsystem's `06-reconciliation-compliance.md` actually implements the frozen contracts — without
> drift from the refined shared docs or the reconciliation decisions. Inputs:
> [`contract-index.md`](./contract-index.md), [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md),
> the 11 refined shared-system docs, each subsystem's `06-reconciliation-compliance.md` + `00-overview.md`,
> and the [`testing-strategy/`](./testing-strategy/) folder. Date: 2026-06-19.
>
> **Verdict up front: CLEAN.** No contradiction was found between a refined shared doc and a subsystem
> rewrite, and no reconciliation decision was left unabsorbed by a subsystem that owns or consumes it. The
> residual open items are the named floors and the `[OPEN — LEGAL]` posture ratifications carried into Phase 6
> (§4), none of which reverses a frozen contract.

---

## 0. Method

For each of the six load-bearing frozen contracts the prompt singles out — **CheckStatus (5.9), `list_objects`
`SetExpr` Filter (4.3), the `myelin-content` taxonomy (13.1/13.2), the `myelin-query` `order_key` parity
(13.3), the unified `#sub` grammar (5.7), and the ONE erasure posture (10.9)** — plus the cross-cutting
seams (ReBAC fragments 4.9, `requires_approval` defaults 8.1, `SCHEDULE_AND_RUN_JOB`/per-effect `idem_key`
9.x, the firehose resume-cursor protocol 3.5, the `humanise` sole templating surface 7.3), I checked:

1. **Ownership matches.** The subsystem that the contract names as **owner/producer** claims ownership; the
   subsystems named as **consumers** claim consumption — and no consumer claims ownership of a producer's
   responsibility.
2. **Shape matches.** The struct/grammar/encoding the subsystem says it implements is **byte-for-byte** the
   frozen one (field names, vocabulary, encoding constants, key/units).
3. **No restatement / no fork.** For the by-reference posture (10.9) and the shared crates (13.x), the
   subsystem points at the one definition rather than authoring a second.
4. **The testing strategy covers it.** A consumer-driven contract suite + gate exists for the contract
   (testing-strategy doc 02 §3), so the no-drift property is mechanically enforced, not just asserted on paper.

---

## 1. Per-contract conformance (the six the prompt names)

### 1.1 CheckStatus (contract 5.9, X-1) — CLEAN

| Subsystem | Role claimed | Verified against frozen shape |
|---|---|---|
| **CI** | **Producer** | Emits `ci.check.updated` carrying the frozen `CheckStatus{repo, commit_oid, context, state, required, run, run_attempt, trust_tier, details_ref, summary, cost_settled, ...}`; supplies the monotonic `run_attempt`; stamps `trust_tier` from provenance + the `!is_untrusted_fork` edge; sets `details_ref = #step-<n>`; emits the rollup `ci.result` signal. Explicitly states CI does **not** own the projection, decide `required`, recompute trust, endorse forks, or merge. (06 §1, Δ1–Δ3.) |
| **Git** | **Gate / consumer** | Owns the `check_status` projection keyed `(commit_oid, context)`; applies `run_attempt >=` monotonic supersession (`<` dropped as stale); owns the `required`-set branch-protection policy; reads `trust_tier` off the fact, never recomputes; `untrusted_fork` success = neutral until `approve_untrusted_ci` endorsement or trusted re-run; merge queue is a durable workflow waking on `ci.result`. (06 §2.1.) |
| **Issues** | Consumer (read-only) | "Can't mark Done while CI red" reads `CheckStatus{state, trust_tier}` via `project(PR_ref)`; **never recomputes trust**. (06 §2.7.) |
| **Chat** | Consumer (unfurl only) | Consumes `ci.check.updated` to bust a PR/commit unfurl card; explicitly does **not** own the gate/projection/supersession. (06 §1.) |

The producer/gate split is exact; no consumer over-claims. Keying, supersession-on-`run_attempt`-not-clock,
and the fork-neutral rule are stated identically on both halves. Testing: SEAM-CHK-1/2/3 (doc 02 §3.4).

### 1.2 `list_objects` `SetExpr` Filter (contract 4.3, OQ-E) — CLEAN

All five consumers lower the frozen `SetExpr` (All/None/Ids/NotIds/`InRelation{relation, via_column}`/Union/
Intersect/Difference/`TupleSet`) to a SQL predicate/JOIN against the per-tenant authz reverse index over
**their own id column**, with the exact `ColRef` named: Git `repo.id`/`pr.id` (06 §2.3); CI `ci_run.run_id`
(06 §1, row 4.3); Issues `issue.id` with the verbatim JOIN `... JOIN authz_visible av ON av.object_id =
issue.id ...` (06 §2.1); Knowledge `db_row.id` via `InRelation{row_reader}` (06 §3); Chat `message.id` /
candidate-id (06 §1). Each states **no N+1, no post-filter** and that the `search-requires-acl-filter` lint
holds. Field-level hiding is correctly pushed to the `CaveatContext` at `check`-time **off** the hot path
(Issues 06 §2.2, Knowledge 06 §3; Chat 06 §1 explicitly notes it does not need the field caveat). Testing:
SEAM-LO-1/2/3 (doc 02 §3.5).

### 1.3 `myelin-content` taxonomy + ADF map (contracts 13.1/13.2, X-2) — CLEAN

- **Knowledge leads + freezes** the full v1 Block set + the inline markdown-subset + the three structured
  nodes; owns the ADF→content lossy import map, recording every lossy node in the import report. (06 §1.)
- **Chat consumes the strict subset** verbatim — `paragraph, heading(1..3), bullet_list, ordered_list,
  task_list, blockquote, code_block, callout, table, divider, image` + the three inline nodes — and
  **excludes `db_view, sync_block, toggle`**, per-message CAS (no CRDT). (06 §1.) Matches the frozen subset.
- **Issues consumes the same block subset** + the three inline nodes, excludes `db_view/sync_block/toggle`
  from inline authoring, single-author CAS. (06 §2.4.) Matches.
- The three inline ref nodes (`mention`/`artifact_ref`/`embed`) are named as the **uniform** producers of
  `refs.edge.created` in all three. The WASM one-editor render path + `render(parse(md)) === md` is asserted in
  all three. Testing: SEAM-MDC-1/2/3 (doc 02 §3.3/3.5b).

### 1.4 `myelin-query` `order_key` parity (contract 13.3, X-3) — CLEAN

Issues (06 §2.5) and Knowledge (06 §1, row 13.3) both link the **frozen shared crate** (field-type enum,
`ViewSpec`, `QueryAst`, `order_key`) and own only their compiler/executor. Both state the **byte-identical**
LexoRank: base-62 `0-9 A-Z a-z`, lexicographic compare, midpoint bisection, 2-char jitter, 48-char rebalance
trigger, `created_at`+ULID tiebreak (Knowledge additionally notes first key `"U"`). The "an issue dragged in a
backlog and a row dragged in a Knowledge db produce byte-identical keys" property is stated on both sides.
`rollup`/`formula` read-time-computed (KN-3) on both. No second implementation. The `QueryAst` is confirmed as
the sole `EventMatcher` core, no per-subsystem CEL (CI 06 row 3.4; Issues 06 §2.10).

### 1.5 Unified `#sub` grammar + tombstone ladder (contract 5.7, X-4) — CLEAN

Each owner mints exactly its frozen kinds with stable opaque ids: Git `comment-`/`thread-`/content-anchored
`L<a>-L<b>` (BLAKE3 fingerprint, 4-state resolver) (06 §2.2); CI `step-<n>`/`check-<context>` (06 row 5.7);
Issues `comment-`/`b`/`field-`/`row-` (06 §2.6); Knowledge `b`/`h`/`row-`/`field-`/`comment-`/`thread-`
(06 §2); Chat `message-`/`thread-` (06 §1). Refs stores the full sub-URN + stripped root; the one 4-step
ladder (permission → root → sub-resolve {live/moved/outdated/gone} → erased) and "a tombstone always carries
the root" are stated consistently. The stability obligation is correctly each owner's. Testing: SEAM-SUB-1/2/3
(doc 02 §3.6).

> **Resolved nuance (not a drift):** the frozen vocabulary lists the heading kind as `h<opaqueid>` (no
> hyphen). Knowledge's compliance doc calls this out explicitly — "`h<opaqueid>` (heading, **hyphen dropped**
> vs Phase 4)" — so the rewrite *matches the frozen grammar* and flags the change from its own Phase-4 draft.
> Consistent.

### 1.6 The ONE erasure posture (contract 10.9, X-7) — CLEAN

All five subsystems instantiate the platform posture **by reference** and explicitly do **not** restate a
fifth residual: Git (06 §2.6, "by reference to `00-reconciliation §X-7`"); CI (06 row 10.9, "does not restate
a CI-local residual"); Issues (06 §2.13, Δ13); Knowledge (06 §8, "the GD-6 write-up is **subsumed** … points
at 10.9"); Chat (06 row 10.9, "writes no fifth chat-specific residual statement"). Each ships the structural
floor now (per-subject DEK crypto-shred + pseudonym-map shred + `restrict`) and flags the third-party
free-text residual as `[OPEN — LEGAL]`. This is exactly the X-7 "one posture, instantiated per subsystem by
reference" mandate — no five-way divergence. Testing: GDPR gates A.1–A.5 + the git-history erasure posture
test (doc 03 Part A).

---

## 2. The cross-cutting seams (also checked) — CLEAN

| Seam | Frozen authority | Conformance |
|---|---|---|
| **ReBAC fragments (4.9)** | Git ref-glob+CODEOWNERS+`approve_untrusted_ci`; CI `ci_project/environment/secret/run` + `read & !is_untrusted_fork`; Issues `issue`+field/transition caveats; KN page-tree inherit + `row_reader` + field caveat; Chat `channel.read = member + parent_project->read`. | Each subsystem declares **verbatim** its frozen fragment + a `watcher` relation per watchable type; all state Id owns the engine and "never invents object ids." CLEAN. |
| **`requires_approval` defaults (8.1)** | The frozen table (CI deploy/secret=yes; Git merge=yes/open_pr=no; Issues forecast/triage=no/SLA-transition=caveat-gated; KN publish/confidential=yes; Chat post=no; cross-subsystem inherits the target's default). | Each subsystem's compliance doc reproduces its row of the frozen table; Chat + Knowledge both state the cross-subsystem "inherits the target's default" rule. CLEAN. Testing: SEAM-TOOL-1. |
| **Four uniform sandbox guarantees + one runner (8.4, X-6)** | `ToolHands::exec` = CI's `kind=agent` job; the real-kernel escape drill gates both kinds; cost gate + per-run token + HITL withhold + isolation floor inherited. | CI owns the runner + the escape drill (06 row 8.4); Git/Issues/Knowledge/Chat each state they **re-implement none** and inherit by construction. CLEAN. Testing: B.1 escape drill leads by non-negotiability. |
| **`SCHEDULE_AND_RUN_JOB` + per-effect `idem_key` (9.1/9.2/9.4, OQ-F)** | Long-park-completed-by-signal; `idem_key = card_id` / `card_id:<effect_idx>`. | CI uses the idiom for pipeline dispatch + `job.done` (06 row 9.x); Git for the `ci.result` merge-queue wait (06 §2.1/§2.10); Chat + Knowledge for batch/partial HITL cards (06 §1 / §5); Issues for SLA escalation HITL (06 §2.9). CLEAN. |
| **Firehose resume-cursor protocol (3.5, OQ-J)** | One `subscribe(stream, scope, cursor?)`/`resume(stream, scope, last_seq)`; per-`(stream,scope)` `seq`; bounded scope never `*`; `resync_required` → `*.snapshot`. | Knowledge owns the protocol over the collab tier (`scope=doc:<id>`, 06 §1); CI rides it for logs (`scope=run:<id>/job:<id>`, 06 row 3.5); Chat rides it for the connection tier (`scope=channel:<id>`, 06 §1). All three state zero-loss-on-reconnect as the gate + the bounded-scope rule. CLEAN. |
| **`humanise` sole templating surface (7.3, OQ-L)** | One ICU-MessageFormat registry; no second template engine; `ArtifactRef`-paired, backend-humanised. | CI status summaries (06 row 7.3), Issues SLA strings (06 §2.8), Knowledge living-doc/status strings (06 §6), Chat card/agent-message strings (06 row 7.3) all register into the one surface; each states "no second/private string map." CheckStatus.summary is a HumanisedRef. CLEAN. |
| **ArtifactRef id grammar / REF-3 (5.1)** | Issues `<PROJECTKEY>-<seqno>` is the stored canonical key; `#1421` is render-time. Git's sha/PR-number is already its stable key. | Issues mints `<PROJECTKEY>-<seqno>` as the stored `<id>`, `#1421` render-time (06 §1); Git states it needs no reconciliation, its sha/PR-number is already canonical (06 §2.7). CLEAN. |
| **Cross-cell PII-free bridge (12.6, OQ-I)** | `CrossCellPointer{subject (opaque), type, correlation_id, home_cell}`; resolution always cell-local; single-home-cell is v1. | Issues portfolio rollup (06 §2.14), Knowledge cross-cell collab (06 §7), Chat cross-org channels (06 §1) all ride the frozen frame, designed-not-built, cell-local resolution. CLEAN. |

---

## 3. Structural confirmations

- **The rewrite-in-place is real.** All five `00-overview.md` files carry a "Changes vs the Phase-4 first pass
  (reconciliation deltas absorbed)" section, and the Phase-4 "06 — shared-system change requests" file is
  replaced in every subsystem by `06-reconciliation-compliance.md` (the inverse map: how the subsystem now
  *implements* the frozen contracts). This matches VISION §5 ("rewrites all of the `04` documents").
- **Every Phase-4 blocking ask is closed.** Issues names its five Phase-4 blocking CRs (CR-1, CR-3, CR-6,
  CR-11, CR-12) as all granted/frozen (06 §0 + §2); Chat maps all twelve CHG-Cn to frozen contracts (06 §1);
  CI lists its twelve deltas Δ1–Δ12 (06 §2). No subsystem carries an open *contract* ask.
- **The testing strategy mechanically enforces no-drift.** The contract-coverage scanner (gate CDC-G0) fails
  the workspace if any contract-index row lacks provider+consumer coverage; CDC-G1 makes a unit mismatch a
  compile error. So the consistency verified here on paper is also a committed gate, not a one-time review.

---

## 4. Residual open questions carried into Phase 6 (none reverses a frozen contract)

These are the honesty-register items every subsystem and the reconciliation doc agree on — **named floors** and
**`[OPEN — LEGAL]` ratifications**, all shipping a structural floor now:

**`[OPEN — LEGAL]` (flagged to counsel/DPO; structural floor ships regardless):**
- The ONE free-text/immutable-content erasure residual lawful basis (X-7/10.9, L-2) — ratified once as the
  platform statement (Git R-7, Issues R-1, Knowledge R6-6, Chat R-C5, CI R-4).
- Worklog/productivity/estimate special-category classification + works-council trigger (OQ-H/10.2) —
  Issues R-2 (fields already tagged `behavioural`/restricted-by-default, per-individual rollups off).
- Build-data-as-LLM-training basis + CD-as-PaaS scope (OQ-H) — CI R-5 (foreclosed/flagged by default).
- Art. 17 reach into immutable git bytes (Git R-7); audit-log retention carve-out (GD-5); fail-static
  staleness-bound ratification (L-1).
- Implicit agent auto-dispatch (L-3) — Chat R-C10 (explicit-first is v1).

**Named floors → named follow-ons (measured-or-promoted in Phase 6):**
- CAS-floor → CRDT for collaborative editing (KN-1; Knowledge R6-1, Issues R-3).
- Node-backed git → object-backed pack/delta over `BlobStore` (STOR-5; Git R-1).
- Read-time rollups → materialised-when-measured (KN-3; Knowledge R6-2, Issues R-4).
- KB-native comments → Chat-threading consolidation, over the shared `#sub`/content/refs scheme (OQ-L;
  Knowledge R6-7, Chat R-C8).
- Single-home-cell → multi-cell cross-cell op fan-out (OQ-I; Issues R-7, Knowledge R6-5, Chat R-C9).
- SCIP/LSIF "find usages" code-intelligence index input (Search 6.5; Git R-3, CI R-3).
- Per-surface shed-budget numbers + firehose retention window — v1 floors tuned by the drills (OQ-K; Chat
  R-C1/R-C2, Knowledge §7 row 1.11).

**Measured-not-predicted thresholds:** the projection-feeder promotion threshold (OQ-C), the `order_key`
48-char rebalance trigger (X-3), the column-store promotion (BUS-6), the CRDT-promotion CAS-conflict rate
(KQ-1), and the gVisor/microVM second-backend promotion trigger (CI R-6) — all promoted on measurement.

**Genuinely-open spikes (build-phase, not contract gaps):** the full adversarial escape-drill corpus +
green-attestation format (CI R-1); the gitoxide server-side capability matrix (Git R-2); the pseudonym
enforcement-mode default (Git R-8); the resource-second → Commercial credit mapping (CI R-2); the
canvas pin/embed joint Chat↔Knowledge mechanism (Chat R-C7); the external MCP endpoint + threat model
(Git R-9, platform-shared).

---

## 5. Conclusion

The Phase-5 layer is **internally consistent**. Every rewritten subsystem implements the frozen contracts it
owns or consumes with the correct producer/consumer split, the byte-identical shapes, and the by-reference
erasure posture; no refined shared doc contradicts a subsystem rewrite; no reconciliation decision was left
unabsorbed by a subsystem that touches it; and the no-drift property is backed by committed consumer-driven
contract gates rather than paper assertion. The only items carried forward are the named floors and the
`[OPEN — LEGAL]` ratifications in §4 — none of which reverses a frozen contract — making this the clean handoff
to Phase 6 (roadmaps).

## 6. Cross-references
- [`README.md`](./README.md) — the Phase-5 index & executive summary.
- [`contract-index.md`](./contract-index.md) — the frozen build-to surface.
- [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) — the rationale for every frozen shape.
- [`testing-strategy/02-parts-contracts-and-mock-agents.md`](./testing-strategy/02-parts-contracts-and-mock-agents.md)
  — the consumer-driven contract gates that mechanically enforce this consistency.
