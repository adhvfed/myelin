# Issue Tracker — 07 · Drills Owed & Open Questions (for Phase 6)

> See [`00-overview.md`](./00-overview.md) for the role. This doc names the **quantified PROVE-IT drill for each
> failable property** (the build/test phases execute; the source-verified scorecard) and the **open questions**
> now handed to **Phase 6** (the Phase-5 reconciliation answered every Phase-4 open question — they are
> cross-referenced as resolved). A drill is a *named, quantified, falsifiable* test of a property the architecture
> claims (VISION §3; the PROVE-IT discipline). Floors are restated so the next agent knows what ships partial.

---

## 1. The drills owed (quantified PROVE-IT)

| # | Property (could fail) | The drill (quantified) | Ties to |
|---|---|---|---|
| **D1** | **Co-equal-view consistency** — the board and roadmap never drift | A chained-mutation E2E: edit an issue's date/scope **on the board** ⇒ the roadmap reflects the **same `issue` row** with zero drift, and vice versa; assert both views read the same row id (the same `ViewSpec` over the same table). | [05 §1](./05-hard-problems.md) |
| **D2** | **Flexible-field query latency** — the JQL trap doesn't recur | A large-custom-field tenant (50+ custom fields, 1M+ issues) board query returns **under the <1s keyboard budget** with the `SetExpr` JOIN conjoined; a **cold ad-hoc query escalates to Search** (same `Filter`), never an unbounded OLTP scan (assert the planner never emits a full JSONB scan). | [02 §3](./02-internals-and-algorithms.md) |
| **D3** | **Permission-leak (confidential)** — zero leak | A cross-tenant + confidential-issue **IDOR drill**: a confidential issue and a cross-tenant issue must **not** appear in any board / `list_objects` `SetExpr` JOIN / search / backlink / context-pane result for an unauthorised viewer, **including under zookie staleness** (the reverse-index revision watermark, contract 4.10). Zero leak. | contract 4.3/4.10; [03 §6](./03-events-contracts-and-glue.md) |
| **D4** | **Human-key correctness** — no dup, monotonic, gaps benign | A **create-storm** (an import + an incident burst on one hot prefix, N workers): assert **no duplicate key**, monotonic per prefix, gaps-only (never reuse), per-prefix isolation (a busy `ENG` doesn't slow `OPS`), and the key is the stored canonical `<id>` (not a render alias). | [02 §4](./02-internals-and-algorithms.md) |
| **D5** | **Concurrent-reorder** — no silent clobber | **N humans + an agent** re-ranking the **same** backlog region: assert zero silent clobber, bounded re-base churn, order converges with the frozen `order_key` (2-char jitter prevents same-key collision), and the 48-char rebalance never reorders the *displayed* order. | [02 §5](./02-internals-and-algorithms.md); contract 13.3 |
| **D6** | **SLA breach durability + calendar correctness** | (a) A breach **fires after a process restart** (the SC-11 rider). (b) A **business-calendar arithmetic corpus**: DST transition, multi-day span, holiday boundary, mid-window pause/resume (cheap disarm/re-arm) — assert the computed `fire_at` matches the expected wall-clock to the second; (c) breach starts the frozen escalation chain (contract 7.5). | [02 §6.2](./02-internals-and-algorithms.md); contracts 7.5/9.3 |
| **D7** | **Trigger fires-once-after-restart** | Arm "Remind me when unblocked" (the frozen `QueryAst` condition); resolve the last blocker **across a restart**; assert the trigger fires **exactly once** into the one inbox; after `stale_after` with no resolution, the stale nudge fires exactly once and the trigger goes stale. | [03 §10](./03-events-contracts-and-glue.md); contract 3.3 |
| **D8** | **Rollup freshness + reindex parity** | (a) **Rollup freshness under an import-storm**: a 10k-issue import triggers a *bounded* number of ancestor recomputes (debounce coalescing), and the initiative progress is correct within the debounce window. (b) **Reindex-from-source rollup/edge parity**: `replay` rebuilds the rollup aggregate + the Refs edge projection **drift-free** vs the live state. | [02 §6.1](./02-internals-and-algorithms.md); contract 2.6/5.5 |
| **D9** | **Import round-trip + resume + fairness** | (a) **`export→import→export` round-trips** over a corpus (the canonical interchange oracle), with the frozen ADF lossy-map nodes named in the report (never silent). (b) A **large import resumes after a crash** with no duplicate creates (the ID map). (c) The import **doesn't starve other tenants** (the protected human lane + per-surface shed budget — a concurrent interactive tenant's latency stays within budget). | [01 §8](./01-tech-and-data-model.md); [05 §10](./05-hard-problems.md); contract 13.2 |
| **D10** | **Editor round-trip** — one render path | `render(parse(md)) === md` over a corpus for issue bodies + comments (the `myelin-content` round-trip gate; the consumed block subset; read mode and edit mode use the identical WASM parser). | contract 13.1 |
| **D11** | **Erasure reaches every holder** | Erase a subject; assert their PII is gone from **every** holder: the `issue` row free-text (per-subject DEK shred), the change-log deltas, comments, attachment blobs, the OLAP read store (+ restriction-flag), the Search index (incl. embeddings), the Refs projection — and the **post-restore re-erasure** (GD-14, contract 10.8) catches a restore. The third-party residual is the documented `[OPEN — LEGAL]` limit (contract 10.9). | [03 §7](./03-events-contracts-and-glue.md); contracts 10.1/10.9 |
| **D12** | **Workflow guard correctness** — governed transitions hold | The "can't mark Done while CI red on the linked PR" (reads the frozen `CheckStatus` + trust posture, contract 5.9) + "can't close while `blocked_by` an open issue" guards: assert the transition is **blocked** with a pre-assembled reason, and that an agent hitting a governed transition is **HITL-gated** per the frozen `requires_approval` default (the tool withheld, no mutation) until approval. | [02 §2](./02-internals-and-algorithms.md); contracts 5.9/8.1 |
| **D13** | **Real-time sync — reconnect loses zero ops** | A board subscribed at `scope = board:<id>` drops its connection mid-edit-storm; on `resume(stream, scope, last_seq)` the backfill `(last_seq, now]` then live loses **zero ops**; a `last_seq` older than the retention window yields `resync_required` → a `*.snapshot` replay (not a silent gap). | [02 §7](./02-internals-and-algorithms.md); contract 3.5 |
| **D14** | **Frontend switch-test + measured UX gates** | The **switch-test**: can a Jira/Linear user complete the core loop (create → triage → plan → board → done) without a manual? + measured **contrast/latency** gates on the primary screens (S1/S3/S5/S6/S9/S10/S13/S17/S19), incl. the empty/loading/error/permission/erased/agent-pending states. | [`../design/`](../design/) |

---

## 2. The named floors (restated; gap-report seeds, status "claimed" until a green drill artifact)

| Floor | What ships v1 | Named follow-on (promotion trigger) |
|---|---|---|
| **Issue hierarchy** | tree `parent` | constrained-DAG portfolios (opt-in per `type_scheme`) |
| **Rollup** | read-time for small subtrees | materialise-on-measured-large (KN-3) |
| **Forecast** | linear `remaining ÷ velocity` (mock agent) | Monte-Carlo agent (reads OLAP) |
| **Ranking** | frozen `order_key` + server-arbitrated CAS | move-CRDT (Yrs list / Fugue) on *measured* concurrent-reorder pain |
| **Sync** | optimistic UI + the frozen resume-cursor protocol | offline/local-first (out of v1 scope unless promoted) |
| **Storage** | PG hybrid (typed core + JSONB + projection feeder), sharded by tenant | distributed-SQL on *measured* single-shard outgrowth |
| **SLA** | full business-calendar logic over the SC-11 wheel + the frozen escalation chain | history-compaction for very-long `time_to_resolution` SLAs |
| **Import** | canonical core + Jira/Linear/GitHub/CSV adapters + the frozen ADF lossy-map | permission-scheme mapping (lossy/legal-review leg) |
| **Free-text PII erasure** | per-subject DEK + pseudonym-map shred + `restrict` (the platform structural floor, contract 10.9) | the third-party-mention residual basis is **[OPEN — LEGAL]** — documented, not claimed solved |
| **Multi-cell** | single-cell complete | cross-cell portfolio rollup over the `CrossCellPointer` bridge (contract 12.6) |
| **Agent runtime** | mock (ADR-08), ToolDefs registered | the real-LLM runtime (post the real-kernel escape drill, contract 8.4) |

---

## 3. Open questions handed to Phase 6

> The Phase-4 open questions were all resolved by the Phase-5 reconciliation: the `list_objects` push-down shape
> (→ OQ-E `SetExpr`), the ABAC caveat context (→ `CaveatContext`), key=ArtifactRef-id (→ frozen
> `<PROJECTKEY>-<seqno>`), the relational trigger matcher (→ frozen `QueryAst`), the reconnect/resync protocol (→
> OQ-J), the ADF map (→ frozen contract 13.2), and the two `[OPEN — LEGAL]` items (→ X-7/OQ-H, structural floor
> ships). What remains for Phase 6 (roadmaps/build) is **calibration, legal ratification, and measured-promotion
> thresholds** — not contract shape.

1. **The projection-feeder promotion threshold calibration** — the measured frequency that promotes a custom
   facet to a generated index (the contract-6.3 default-to-beat is `> 5%` of view executions). The D2 drill
   calibrates the exact value + the OLTP↔Search escalation cost threshold. *Resolver: Issues + Search (measured).*
2. **The debounce-window + affected-ancestor fan-out policy** — the per-tenant-tunable debounce window and the
   per-surface in-flight cap (OQ-K floor) that bounds a leaf-under-a-50-team-initiative recompute. The D8 drill
   calibrates it. *Resolver: Issues (measured).*
3. **The third-party free-text PII residual lawful basis** — counsel/DPO ratify the documented limit (the one
   platform statement, contract 10.9). The structural floor ships regardless. *Resolver: GDPR + Counsel.*
4. **Worklog/productivity special-category vs elevated classification + the works-council consultation trigger
   per jurisdiction** (contract 10.2, OQ-H). The fields ship with the frozen restricted-by-default tags; counsel
   ratifies the category. *Resolver: GDPR + Counsel + works-council.*
5. **Cross-cell portfolio rollup activation** — the multi-cell rollup walk over the `CrossCellPointer` bridge with
   per-viewer cell-local resolution (contract 12.6). Single-cell is the complete v1; this is the named
   multi-cell floor's build. *Resolver: Issues + Tenancy (when multi-cell lands).*
6. **The full armable-condition catalogue + the Trigger management surface (S16)** — the complete set of
   `QueryAst` conditions a user can arm and the manage-armed/resolved/stale UX. *Resolver: Issues + design.*
7. **The move-CRDT promotion** — whether *measured* concurrent-reorder pain crosses the threshold to promote the
   frozen `order_key` CAS floor to the Yrs move-CRDT. *Resolver: Issues (measured).*

---

## 4. How this subsystem proves it earned its place

The Issue Tracker's job is to make the engineer's board, the PM's roadmap, and the corporate governance surface
**one product over one model** — and to be the subsystem the rest of Myelin coordinates around without ever
growing a private back-channel. The architecture earns that on three falsifiable claims, each with a drill, all
now built to the **frozen** shared contracts: **(1) co-equal-view consistency** (D1 — the board and roadmap are
the same rows, structurally); **(2) the flexible-field query never recreates the JQL trap** (D2 — typed core +
bounded planner lowering the `SetExpr` push-down + the Search valve); and **(3) zero permission leak** (D3 — the
confidential exclusion is by-construction via the `SetExpr` JOIN, not a post-filter). Everything else — keys,
ranking, rollup, SLA, triggers, sync, import, erasure — is built on the shared substrate with named floors and
measured promotions, so the subsystem ships honest-partial and grows on evidence, never on speculation.
