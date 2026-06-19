# Issue Tracker — 06 · Required Shared-System Changes (for Phase-5 reconciliation)

> See [`00-overview.md`](./00-overview.md) for the role. This is the **explicit, itemized list** of what Issues
> needs from the shared systems (Identity / Bus / Refs / Search / Notif / Agents / Storage / GDPR / Workflow /
> Tenancy) **that isn't already in the Phase-3 contracts** — the Phase-5 reconciliation backlog. Each item names
> the **owner**, the **ask**, the **nature** (new / confirmation / reconciliation), and whether Issues can ship
> a **floor without it**. Most are *confirmations* or small *reconciliations* of already-frozen seams (Phase 3
> reversed no ADR; the pattern is additive sharpening). Cross-referenced as CR-* throughout the architecture.

---

## The change requests

| # | Owner | Ask | Nature | Floor without it? |
|---|---|---|---|---|
| **CR-1** | **Identity** | Confirm `list_objects` `Filter{set_expr, zookie}` is **consumer-composable over the `issue.id` column** (push-down, facet-expressible) so the query planner can conjoin the ACL filter into a Tier-1 board scan without an N+1 or a post-filter. This is contract S-10 *confirmed for Issues* (Search has it over `doc_id`, Refs over `source`). | Confirmation of Id §8.2 | No — the leak-free board/list scan depends on push-down. |
| **CR-2** | **Identity** | Confirm the **field/transition ABAC caveat at the edge** (`field.view` "visible iff `issue.severity < X`"; transition "approver-role signed off") is evaluatable at `check`-time with issue context and **kept off the hot `list_objects` path** (Id §9). Issues needs the caveat-context shape (which issue attributes are passable). | Reconciliation (Id §9 caveat context) | Partial — field-level hiding degrades to all-or-nothing without it. |
| **CR-3** | **Refs** | Reconcile **"the human key is the ArtifactRef id"** with REF-3 "display keys are render-time." Resolution leaned: the **full key (`ENG-1421`) is the stable public id in the URN**; *short* forms (`#1421`) are the render-time projection. Confirm Refs treats `ENG-1421` as the canonical `<id>` segment, not a render-time alias. | Reconciliation (REF-3 vs §7 of doc 01) | No — the ArtifactRef id grammar must be agreed before keys are minted. |
| **CR-4** | **Bus / Refs** | Confirm the **`initiative` type token** addition to the Bus §6.2 token table (a ranked `issue`-family type, alongside the seeded `issue/epic/sprint/field/comment/relation`). | New token (Bus §6.2 extension — explicitly sanctioned: "each subsystem owns its complete list") | Yes — but the ArtifactRef for an initiative needs the token. |
| **CR-5** | **Bus** | Confirm the **`arm_trigger` condition** accepts an `EventMatcher` over `issue.*` events that references the **`issue_relation` projection state** ("all `blocked_by` edges resolved"), i.e. a relational condition, not just a single-event match. The stateful Trigger flagship depends on it (contract 3.3). | Confirmation/reconciliation of Bus §3.6/§4.5 (matcher expressiveness) | Partial — a simpler "last blocker's `transitioned` event" matcher is a floor, but loses precision. |
| **CR-6** | **Workflow** | Confirm the **SLA timer re-arm on pause/resume** pattern: Issues disarms + re-arms a `wf_timer` (the precomputed `fire_at`) without polluting the wheel with calendar logic. Confirm a bare SLA timer emits `sla.deadline.reached` (durable-workflow §4.2 names this) **and** that Issues can cancel/re-arm cheaply. | Confirmation of Workflow §4.2 (re-arm semantics) | No — pause/resume correctness depends on cheap re-arm. |
| **CR-7** | **GDPR / Legal** | Resolve the **free-text PII erasure residual (GD-6)**: the lawful-basis posture for third-party free-text mentions of a subject that cannot be crypto-shredded (they live in content others own). Issues ships the floor (anonymise + redaction-tombstone + crypto-shred-own + agent-scan); the **residual is [OPEN — LEGAL]** and needs a ratified posture. | New (legal posture) | Yes — the floor ships; the residual is documented honestly pending review. |
| **CR-8** | **GDPR / Legal** | Classify **worklog / productivity / estimate-vs-actual field sensitivity** (works-council / labour-law — GD-13): are these special-category or works-council-consultable in EU jurisdictions? Drives whether they are `#[personal_data]`-tagged with a restricted `data_role`. | New (legal classification) | Yes — fields ship; the classification gates their indexing/analytics use. |
| **CR-9** | **Knowledge** | Co-design the **ADF→`myelin-content` converter fidelity** (import, sketch 09): Knowledge owns the content taxonomy; Issues needs the lossy-node map (which ADF nodes have no `myelin-content` analogue) for the import reconciliation report. | New (co-design; Knowledge leads ADR-05) | Yes — import ships with a coarser converter + a fuller "lossy" flag list. |
| **CR-10** | **Knowledge** | Confirm **primitive parity** on `myelin-query` (ADR-06): the field-type enum, the view-model, and the AST grammar are shared; Issues owns its AST→store compiler + cost-bounding. Confirm the `order_key`/LexoRank encoding (base, jitter) is identical so a row dragged in a Knowledge db and an issue dragged in a backlog use the same family. | Confirmation (ADR-06 co-ownership) | Partial — Issues can ship its own compiler regardless, but encoding drift would block a future shared CRDT. |
| **CR-11** | **Search** | Confirm Issues' `IndexSpec` (`declare_indexable`) supports the **Tier-3 escalation** shape: a board/list query that exceeds the OLTP cost budget compiles to a Search query (struct + FT fields), ACL-pre-filtered. Confirm the **projection feeder** can read a frequency signal (which custom facet is filtered often) to drive measured promotion. | Confirmation of Search §5.1/§5.3 + a feeder signal | No (for Tier-3) — without it, a cold ad-hoc query has no safe valve and would hit OLTP. |
| **Yes** for the feeder signal floor (the GIN index serves until promoted). | | | | |
| **CR-12** | **Notif** | Confirm the **"My Work" scoped view** is a `list_inbox(principal, filter)` over the **one inbox** (C-9) — the assigned/blocked/needs-approval/overdue groups are `reason`/`subject` filters, with shared read-state (mark once, consistent everywhere). Confirm `define_notif_rule` lets Issues map its Signal classes (SLA at-risk, unblocked, approval-requested) to inbox reasons/priorities. | Confirmation of Notif §1.3/§3.1 | No — "My Work" must not become a second store. |
| **CR-13** | **Notif** | Confirm the **escalation chain** config shape (co-design with `oncall_now`/`page`, Notif §3.7): an SLA breach starts a durable escalation workflow; Issues needs the chain-definition shape it passes to Notif. | Co-design (Notif §3.7) | Yes — a single-step "page on-call" floor ships; multi-step chains follow. |
| **CR-14** | **Agents** | Confirm the **forecast agent ToolDef** (`issue.forecast`, compute-only, reads OLAP) + the **at-risk threshold config** mechanism (per-initiative or per-scheme). Confirm the triage/SLA-draft agents register as gated tools with `requires_approval` defaults Issues sets. | Confirmation of Agent §6/§7.1 (ToolDef shape) + a threshold config | Yes — agents are mock-now (ADR-08); the ToolDefs ship; the LLM runtime is P6. |
| **CR-15** | **Tenancy / Control plane** | Confirm the **cross-cell portfolio rollup** seam (an initiative whose child epics span cells) rides the PII-free pointer bridge (Tenancy §10). Issues' rollup walk needs the bridge to carry `subject`/`type`/`correlation_id` for a remote child, resolving the child's progress locally per-viewer. | Confirmation of Tenancy §10 (the named multi-cell floor) | Yes — single-cell rollup is the complete v1; cross-cell is the named floor. |
| **CR-16** | **Storage** | Confirm the **OLAP read store** (Storage §3.4) accepts Issues' `issue.*` + `sla.*` + `cycle.*` event stream for CFD/cycle-time/velocity/SLA-compliance, fed by the consumer template, reindex-from-source only. Confirm the **OLAP store honours the restriction flag** (no analytics for a restricted subject). | Confirmation of Storage §3.4 + restriction-flag propagation | No — reports depend on OLAP; partial without restriction-flag honouring (a compliance gap). |

---

## Summary by owner

- **Identity:** CR-1 (push-down `list_objects` over `issue.id` — *blocking*), CR-2 (ABAC caveat context).
- **Bus / Refs:** CR-3 (key=ArtifactRef-id reconciliation — *blocking*), CR-4 (`initiative` token), CR-5
  (relational trigger matcher).
- **Workflow:** CR-6 (SLA timer re-arm — *blocking*).
- **GDPR / Legal:** CR-7 (free-text PII residual GD-6 — *legal*), CR-8 (worklog sensitivity GD-13 — *legal*).
- **Knowledge:** CR-9 (ADF converter co-design), CR-10 (`myelin-query` + `order_key` primitive parity).
- **Search:** CR-11 (Tier-3 escalation + feeder signal — *partially blocking*).
- **Notif:** CR-12 (My Work scoped view — *blocking*), CR-13 (escalation chain shape).
- **Agents:** CR-14 (forecast ToolDef + threshold config).
- **Tenancy:** CR-15 (cross-cell rollup bridge — *named floor*).
- **Storage:** CR-16 (OLAP stream + restriction-flag honouring — *partially blocking*).

**The five *blocking* items** (cannot ship even a floor without them): CR-1 (`list_objects` push-down), CR-3
(key=ArtifactRef-id), CR-6 (SLA timer re-arm), CR-11 (Tier-3 escalation valve), CR-12 (My Work = one inbox).
None requires a new shared *system* — each is a confirmation or small reconciliation of an already-frozen
Phase-3 seam. **No ADR reversal is requested.**

Continue to [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md).
