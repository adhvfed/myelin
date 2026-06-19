# Issue Tracker — 07 · Drills Owed & Open Questions

> See [`00-overview.md`](./00-overview.md) for the role. This doc names the **quantified PROVE-IT drill for each
> failable property** (Phase 5 executes; T-4 scorecard) and the **open questions** handed to Phase 5. A drill is
> a *named, quantified, falsifiable* test of a property the architecture claims — per VISION §3 ("name the drill
> for each failable property") and the doctrine's PROVE-IT discipline. Floors are restated so Phase 5 knows what
> ships partial.

---

## 1. The drills owed (quantified PROVE-IT)

| # | Property (could fail) | The drill (quantified) | Ties to |
|---|---|---|---|
| **D1** | **Co-equal-view consistency** — the board and roadmap never drift | A chained-mutation E2E: edit an issue's date/scope **on the board** ⇒ the roadmap reflects the **same `issue` row** with zero drift, and vice versa; assert both views read the same row id. | [05 §1](./05-hard-problems.md); T-6 |
| **D2** | **Flexible-field query latency** — the JQL trap doesn't recur | A large-custom-field tenant (50+ custom fields, 1M+ issues) board query returns **under the <1s keyboard budget**; a **cold ad-hoc query escalates to Search**, never an unbounded OLTP scan (assert the planner never emits a full JSONB scan). | [02 §3](./02-internals-and-algorithms.md); T-8 |
| **D3** | **Permission-leak (confidential)** — zero leak | A cross-tenant + confidential-issue **IDOR drill**: a confidential issue and a cross-tenant issue must **not** appear in any board / `list_objects` / search / backlink / context-pane result for an unauthorised viewer, **including under zookie staleness**. Zero leak. | Id §5; deep-dive §8.4; T-5; D4 (Id) |
| **D4** | **Human-key correctness** — no dup, monotonic, gaps benign | A **create-storm** (an import + an incident burst on one hot prefix, N workers): assert **no duplicate key**, monotonic per prefix, gaps-only (never reuse), and the per-prefix isolation (a busy `ENG` doesn't slow `OPS`). | [02 §4](./02-internals-and-algorithms.md); sketch 04 |
| **D5** | **Concurrent-reorder** — no silent clobber | **N humans + an agent** re-ranking the **same** backlog region: assert zero silent clobber, bounded re-base churn, order converges, and the rebalance never reorders the *displayed* order. | [02 §5](./02-internals-and-algorithms.md); sketch 06 |
| **D6** | **SLA breach durability + calendar correctness** | (a) A breach **fires after a process restart** (the SC-11 rider). (b) A **business-calendar arithmetic corpus**: DST transition, multi-day span, holiday boundary, mid-window pause/resume — assert the computed `fire_at` matches the expected wall-clock to the second. | [02 §6.2](./02-internals-and-algorithms.md); sketch 07 |
| **D7** | **Trigger fires-once-after-restart** | Arm "Remind me when unblocked"; resolve the last blocker **across a restart**; assert the trigger fires **exactly once** into the one inbox; after `stale_after` with no resolution, the stale nudge fires exactly once and the trigger goes stale. | [03 §10](./03-events-contracts-and-glue.md); sketch 08 |
| **D8** | **Rollup freshness + reindex parity** | (a) **Rollup freshness under an import-storm**: a 10k-issue import triggers a *bounded* number of ancestor recomputes (debounce coalescing), and the initiative progress is correct within the debounce window. (b) **Reindex-from-source rollup/edge parity**: `replay` rebuilds the rollup aggregate + the Refs edge projection **drift-free** vs the live state. | [02 §6.1](./02-internals-and-algorithms.md); sketch 05; T-5 reindex-from-cold parity |
| **D9** | **Import round-trip + resume + fairness** | (a) **`export→import→export` round-trips** over a corpus (the canonical interchange oracle). (b) A **large import resumes after a crash** with no duplicate creates (the ID map). (c) The import **doesn't starve other tenants** (the protected human lane + X-3 caps — a concurrent interactive tenant's latency stays within budget). | [01 §8](./01-tech-and-data-model.md); sketch 09; X-3 |
| **D10** | **Editor round-trip** — one render path | `render(parse(md)) === md` over a corpus for issue bodies + comments (the `myelin-content` round-trip gate; read mode and edit mode use the identical parser). | ADR-05/KN-4; T-5/§8b.2 |
| **D11** | **Erasure reaches every holder** | Erase a subject; assert their PII is gone from **every** holder: the `issue` row free-text (per-subject DEK shred), the change-log deltas, comments, attachment blobs, the OLAP read store, the Search index (incl. any embeddings), the Refs projection — and the **post-restore re-erasure** (GD-14) catches a restore. | [03 §7](./03-events-contracts-and-glue.md); GDPR §4; T-5 |
| **D12** | **Workflow guard correctness** — governed transitions hold | The "can't mark Done while CI red on the linked PR" + "can't close while `blocked_by` an open issue" guards: assert the transition is **blocked** with a pre-assembled reason, and that an agent hitting a governed transition is **HITL-gated** (the tool withheld, no mutation) until approval. | [02 §2](./02-internals-and-algorithms.md); flow B2/C1 |
| **D13** | **Frontend switch-test + measured UX gates** | The **switch-test** (T-7): can a Jira/Linear user complete the core loop (create → triage → plan → board → done) without a manual? + measured **contrast/latency** gates (T-8) on the primary screens (S1/S3/S5/S6/S9/S10/S13/S17/S19), incl. the empty/loading/error/permission/erased/agent-pending states. | [`../design/`](../design/); T-7/T-8 |

---

## 2. The named floors (restated; E-3 gap-report seeds, status "claimed")

| Floor | What ships v1 | Named follow-on (promotion trigger) |
|---|---|---|
| **Issue hierarchy** | tree `parent` | constrained-DAG portfolios (opt-in per `type_scheme`; cross-team multi-parent need) |
| **Rollup** | read-time for small subtrees | materialise-on-measured-large (KN-3 measured-promotion) |
| **Forecast** | linear `remaining ÷ velocity` | Monte-Carlo agent (reads OLAP) |
| **Ranking** | LexoRank + server-arbitrated CAS | move-CRDT (Yrs list / Fugue) on *measured* concurrent-reorder pain (R-5) |
| **Sync** | optimistic UI + resume-cursor over the firehose | offline/local-first (out of v1 scope unless promoted) |
| **Storage** | PG hybrid (typed core + JSONB + projection feeder), sharded by tenant | distributed-SQL on *measured* single-shard outgrowth |
| **SLA** | full business-calendar logic over SC-11 | history-compaction for very-long `time_to_resolution` SLAs (continue-as-new) |
| **Import** | canonical core + Jira/Linear/GitHub/CSV adapters | permission-scheme mapping (lossy/legal-review leg) |
| **Free-text PII erasure** | anonymise-actor + redaction-tombstone + crypto-shred-own + agent-scan | the third-party-mention residual is **[OPEN — LEGAL]** (GD-6) — documented, not claimed solved |
| **Multi-cell** | single-cell is complete | cross-cell portfolio rollup over the PII-free pointer bridge (Tenancy §10) |
| **Forecast/triage/SLA agents** | mock runtime (ADR-08), ToolDefs registered | the LLM runtime (P6, post safety drills) |

---

## 3. Open questions handed to Phase 5

1. **`list_objects` push-down shape over `issue.id`** (CR-1) — the exact `Filter{set_expr}` encoding the planner
   conjoins into a Tier-1 board scan without N+1. *Blocking; resolver: Identity.*
2. **The ABAC caveat context** (CR-2) — which issue attributes are passable to a `field.view` caveat at
   `check`-time, and the perf envelope of edge-evaluated field-hiding at board scale. *Resolver: Identity.*
3. **Key = ArtifactRef-id vs REF-3** (CR-3) — confirm `ENG-1421` is the canonical `<id>` segment, not a
   render-time alias. *Blocking; resolver: Refs.*
4. **The relational trigger matcher** (CR-5) — can an `EventMatcher` express "all `blocked_by` edges resolved"
   (a projection-state condition), or must Issues maintain a derived "is-blocked" flag the matcher watches?
   *Resolver: Bus.*
5. **The projection-feeder promotion threshold + cost model** (CR-11) — the measured frequency that promotes a
   custom facet to a generated index, and the AST→store cost model's OLTP↔Search escalation threshold. *Resolver:
   Issues + Search; the D2 drill calibrates it.*
6. **The reconnect/resync protocol + per-view subscription scope bounding** (sketch 08; co-design with Chat's
   connection tier / KN-1) — how a huge board's event stream is bounded without losing ops on reconnect.
   *Resolver: Issues + Knowledge + Chat.*
7. **The canonical interchange schema + the lossy mapping tables** (CR-9) — the round-trip oracle's exact shape;
   the link-type / status-category / permission-scheme mapping tables; ADF→`myelin-content` fidelity. *Resolver:
   Issues + Knowledge; permission-scheme leg → Legal.*
8. **Free-text PII erasure completeness + the documented residual** (CR-7 / GD-6) — the ratified lawful-basis
   posture for third-party free-text mentions. *Resolver: GDPR + Legal.*
9. **Worklog / productivity-field sensitivity** (CR-8 / GD-13) — works-council / labour-law classification.
   *Resolver: GDPR + Legal.*
10. **Cross-cell portfolio rollup** (CR-15) — the rollup walk over a child in another cell via the PII-free
    pointer bridge; the per-viewer remote resolution. *Resolver: Issues + Tenancy (the named multi-cell floor).*
11. **Debounce-window + affected-ancestor fan-out policy** (sketch 05) — the per-tenant-tunable debounce window
    and the in-flight cap that bounds a leaf-under-a-50-team-initiative recompute. *Resolver: Issues; the D8
    drill calibrates it.*
12. **The full armable-condition catalogue + the Trigger management surface** (sketch 08) — the complete set of
    conditions a user can arm and where they manage armed/resolved/stale triggers (S16). *Resolver: Issues +
    design.*

---

## 4. How this subsystem proves it earned its place

The Issue Tracker's job is to make the engineer's board, the PM's roadmap, and the corporate governance surface
**one product over one model** — and to be the subsystem the rest of Myelin coordinates around without ever
growing a private back-channel. The architecture earns that on three falsifiable claims, each with a drill:
**(1) co-equal-view consistency** (D1 — the board and roadmap are the same rows, structurally); **(2) the
flexible-field query never recreates the JQL trap** (D2 — typed core + bounded planner + Search valve); and
**(3) zero permission leak** (D3 — the confidential exclusion is by-construction, not a post-filter). Everything
else — keys, ranking, rollup, SLA, triggers, sync, import — is built on the shared substrate with named floors
and measured promotions, so the subsystem ships honest-partial and grows on evidence, never on speculation.
