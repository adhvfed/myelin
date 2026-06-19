# Knowledge Platform — 07 · Drills Owed & Open Questions

> See [`00-overview.md`](./00-overview.md) for framing. Per the PROVE-IT mandate (EI-01 P3; T-2/T-5): each
> property that can fail names the **quantified drill** that proves it. Phase 5 owns execution + the exact
> thresholds; this is Knowledge's **obligation register** (each a Phase-5 scorecard item, T-4: a capability is
> "proven" only when a drill emits a green artifact — until then "claimed"). The headline drill — the
> reconnect-loses-zero-ops drill — is **Knowledge's** (KN-1, Phase-3 handoff README §5).

---

## 1. The drills owed (quantified)

| # | Property / failure mode | Drill (quantified gate) | Telemetry read | Directive/source |
|---|---|---|---|---|
| **KD-1** | **Reconnect loses zero ops** (KN-1 — the headline; Knowledge owns it) | Kill a collab client mid-edit + sever the firehose connection during a sustained multi-author edit; on reconnect (resume from `op_seq` cursor) assert **zero ops lost, zero duplicate effects** (the `UNIQUE(op_id)` idempotent apply). Gate: **0 lost, 0 duplicate.** | op-log apply lag, op dedup hit-rate, resume-gap size | KN-1, T-5 (EI-04 §2.3) |
| **KD-2** | **Editor round-trip** (KN-4 — the correctness bar regardless of engine) | `render(parse(md)) === md` over a markdown-subset corpus (incl. mentions/refs/embeds, nested lists, code, tables, IME/paste edge cases). Gate: **100% round-trip; 0 corpus regressions.** | corpus pass rate | KN-4, T-5 (EI-05 §2; DL §8b.2) |
| **KD-3** | **CAS floor: no silent overwrite** | Two clients edit the same block concurrently; assert the loser is **rejected with the current state to reconcile**, never silently overwritten; assert different blocks edit in parallel with no false conflict. Gate: **0 silent overwrites; cross-block edits independent.** | CAS conflict rate (the CRDT promotion trigger) | KN-1 floor (EI-04 §2.1) |
| **KD-4** | **Erasure reaches every holder** (the hardest GDPR surface) | Erase a subject; assert structured PII (person props, mentions, attribution) is purged/pseudonymised, free-text PII under a per-subject DEK is **crypto-shredded (key destroyed → unrecoverable in op-log/snapshots/backups)**, embeddings purged, backlinks tombstoned. Gate: **0 recoverable structured PII incl. vectors; free-text covered by the documented residual.** | holder erase receipts, vector-tombstone lag | KN-3/GD-4, T-5 (EI-04 §1) |
| **KD-5** | **Permission-filtered reads — zero leak** | A confidential page / overridden sub-page / row-restricted db / field-hidden column must **never** appear in any view / backlink / search / embed / RAG result for an unauthorized viewer (incl. counts). Gate: **0 leaked artifacts; 0 count-leak.** | zero-escape counters | ADR-03/SC-1, T-2 ([02 §5](./02-internals-and-algorithms.md)) |
| **KD-6** | **Reindex-from-cold parity** | Wipe Knowledge's derived state (the Refs edge projection / Search index for Knowledge); `replay(scope)`; assert the rebuilt state **matches live** (same blocks, edges, ACL behaviour). Gate: **cold == live; rebuild uses the live consumer path only.** | reindex parity hash | SEARCH-1/REF-4, T-5 (EI-04 §5.3) |
| **KD-7** | **No silent data loss (outbox)** | Crash the Knowledge service *between* the block/row commit and relay-publish; assert the event is still delivered (outbox survived) and never delivered without the state change. Gate: **0 ghost, 0 lost.** | outbox depth+age | BUS-2, substrate D-1 |
| **KD-8** | **Hot-document thundering herd** | An all-hands doc with thousands of concurrent readers/editors; assert connection multiplexing + awareness throttling + read-replica hold the op fan-out within budget, per-tenant caps protect others. Gate: **hot-doc latency within budget; other tenants unaffected.** | per-tenant in-flight, op fan-out | deep-dive §5.9; ADR-16 |
| **KD-9** | **Flexible-DB query latency at scale** (TE-17 measured-promotion trigger) | Filter/sort/group a large multi-tenant database via the JSONB + derived-projection path; assert read-time latency within budget; **measure** when a facet needs materialisation. Gate: **query p99 within budget; promotion trigger measured.** | db-query latency, facet-materialisation flags | TE-17, KN-3 ([05 §3](./05-hard-problems.md)) |
| **KD-10** | **Read-time rollup latency** (TE-18 measured-promotion trigger) | A rollup over a large related set, computed at read time; assert latency within budget; **measure** when a rollup needs incremental materialisation. Gate: **rollup p99 within budget; promotion trigger measured.** | rollup latency | TE-18, KN-3 ([05 §4](./05-hard-problems.md)) |
| **KD-11** | **Agent edits are governed + attributed** | An agent edits a doc via `EffectApi` → collab apply; assert the edit is attributed "suggested by agent", a consequential edit is **HITL-withheld** (returns error, does NOT mutate) until approval, denied effects return ordinary tool errors. Gate: **0 ungoverned agent mutation; 0 mutation before approval.** | gate-state, denial counter | ADR-08, agent-fabric D-5 ([02 §9](./02-internals-and-algorithms.md)) |
| **KD-12** | **Agent-trace holder erasure** (AG-7) | Erase a subject; assert their content-addressed agent traces are crypto-shredded/purged, attribution falls back to the pseudonym. Gate: **0 recoverable PII in traces; attribution intact.** | trace holder receipts | AG-7, agent-fabric D-10 ([03 §5.2](./03-events-contracts-and-glue.md)) |
| **KD-13** | **Cross-tenant IDOR** | Attempt to read a page/db/row across tenants via path-tenant spoofing; assert zero cross-tenant read (tenant from token, the `tenant-predicate` lint catches a tenant-less query at compile). Gate: **0 cross-tenant read.** | per-tenant counters | EI-02 §1/ID-3, substrate D-7 |

**Knowledge inherits the substrate/Id/Bus/Search drills** (30× agent-surge, Id-hiccup/fail-static, restore +
cross-seam, causal-loop tripwire) by standing on `serve(AppSpec)` — they assert against Knowledge's surfaces
too. KD-1, KD-2, KD-3, KD-4 are the **Knowledge-specific** headline gates.

---

## 2. Open questions for Phase 5

> The nine **design-shaped** questions Stage-1 ([`../sketches/00-findings.md`](../sketches/00-findings.md) §6)
> handed forward are **closed** in [`08-committed-resolutions.md`](./08-committed-resolutions.md) (CR-A…CR-I) —
> they were architecture decisions for me to make now, not Phase-5 measurements. The table below is the
> residual **Phase-5** set: measured promotion thresholds and cross-subsystem consolidations. Where an `08`
> resolution names a floor, that floor's *threshold* (not its mechanism) appears here.

| # | Question | Lean / default-to-beat | Owner |
|---|---|---|---|
| KQ-1 | **The CRDT promotion timing.** When does the first true concurrent-edit conflict (R5) cross the threshold that triggers the Yrs CRDT? | Measure CAS conflict rate; promote when same-block concurrent edits exceed a measured threshold. The transport (KN-1) and editor (KN-4) are CRDT-ready from day one. | Knowledge P5 (measured) |
| KQ-2 | **Comments: reuse the Chat threading primitive or stay KB-native?** (deep-dive Q12) | v1 ships KB-native comment threads; the shared-primitive consolidation is a cross-subsystem follow-on. | Knowledge + Chat P5 |
| KQ-3 | **Templating as a shared capability** (with issue + CI templates, deep-dive §2.5). | v1 ships KB page/db templates; a shared templating capability is a flagged cross-subsystem follow-on. | Knowledge + Issues + CI P5 |
| KQ-4 | **The flexible-DB materialisation promotion trigger** (TE-17/TE-18) — the measured latency threshold that promotes a JSONB facet / read-time rollup to a materialised projection. | Read-time + derived-projection floor; promote a *specific* facet/rollup only when KD-9/KD-10 measure it too slow. | Knowledge P5 (measured) |
| KQ-5 | **Field-level ABAC predicate catalogue per database** ([05 §5](./05-hard-problems.md)). | The mechanism (a caveat on `field.view`, off the hot path) is decided; the per-db predicate catalogue is a P5 detail co-designed with Id's role-bundle catalogue (Id §15). | Knowledge + Id P5 |
| KQ-6 | **Synced-blocks/transclusion design** ([05 §7](./05-hard-problems.md)) — deferred to post-v1, designed against the CRDT. | v1 = embeds only; transclusion is a named follow-on requiring reference-counted erasure + DAG tree model. | Knowledge P5+ |
| KQ-7 | **Cross-cell collab for multi-cell tenants** ([05 §9](./05-hard-problems.md); SC-2/SC-3). | Single-cell, residency-pinned (floor); cross-cell op propagation inherits the bus cross-cell pointer bridge (event-bus §7.4). | Control-plane / multi-cell tenancy P5 |
| KQ-8 | **The free-text PII erasure residual write-up** (GD-6 `[OPEN → LEGAL]`, [05 §6](./05-hard-problems.md)). | Structured reliable + per-subject crypto-shred; free-text = tooling + documented residual — a named co-owned Knowledge/Legal/DPO deliverable. | Knowledge + Legal/DPO |
| KQ-9 | **Agent `requires_approval` defaults + approver set per Knowledge tool** ([03 §5.1](./03-events-contracts-and-glue.md)). | Gated by default for consequential/irreversible (publish, confidential/PII edits); approver = `list_subjects(object, manage)`. | Knowledge + Agent Fabric P5 |
| KQ-10 | **Block-vs-page search index size at world scale** ([02 §6](./02-internals-and-algorithms.md)). | Both, page default + significant-block; measure index size and prune block-level to useful jump targets if it grows. | Knowledge + Search P5 (measured) |

---

## 3. The gap-report seed (E-3 — the durable floor list)

Per E-3, the floors this subsystem ships, with claimed/proven status (dated 2026-06-19; all **claimed**
until the KD-* drills emit green artifacts in Phase 5):

- **CAS floor (no merge)** → CRDT (KN-1). Claimed.
- **Read-time formula/rollup** → per-rollup materialisation (KN-3). Claimed.
- **JSONB + derived projection** → per-facet materialisation (TE-17). Claimed.
- **Synced blocks deferred** → transclusion on the CRDT. Claimed.
- **Offline = read + queued light-edit** → full offline-first with the CRDT. Claimed.
- **Single-cell collab** → cross-cell (SC-2/SC-3). Claimed.
- **Free-text PII = tooling + documented residual** (GD-6). Claimed; co-owned with Legal.

---

This completes the Knowledge Platform Phase-4 architecture. Index: [`../README.md`](../README.md).
