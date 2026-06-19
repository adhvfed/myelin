# Knowledge Platform — 07 · Drills Owed & Open Questions

> See [`00-overview.md`](./00-overview.md) for framing. Per the PROVE-IT mandate (EI-01 P3; T-2/T-5): each
> property that can fail names the **quantified drill** that proves it. Phase 6 owns execution + the exact
> thresholds; this is Knowledge's **obligation register** (a capability is "proven" only when a drill emits a
> green artifact — until then "claimed"). The headline drill — **reconnect-loses-zero-ops** — is **Knowledge's**
> (KN-1, the collab transport is Knowledge's deliverable over the frozen firehose protocol, contract 3.5).

---

## 1. The drills owed (quantified)

| # | Property / failure mode | Drill (quantified gate) | Telemetry read | Source |
|---|---|---|---|---|
| **KD-1** | **Reconnect loses zero ops** (KN-1 — the headline; Knowledge owns it) | Kill a collab client mid-edit + sever the connection during a sustained multi-author edit; on reconnect (`firehose::resume(scope=doc:<id>, last_seq=cursor)`) assert **zero ops lost, zero duplicate effects** (the `UNIQUE(op_id)` idempotent apply). Re-run **across an `engine_promote` boundary** (CAS→CRDT migration). Gate: **0 lost, 0 duplicate.** | op-log apply lag, op dedup hit-rate, resume-gap size, `resync_required` rate | KN-1, T-5; contract 3.5 |
| **KD-2** | **Editor round-trip** (KN-4 — the correctness bar regardless of engine) | `render(parse(md)) === md` over a markdown-subset corpus (incl. the three structured nodes `U+FFFC`-anchored × nesting in bold/lists/tables, code, IME/paste edge cases). Gate: **100% round-trip; 0 corpus regressions.** | corpus pass rate | KN-4, T-5; contract 13.1 |
| **KD-3** | **CAS floor: no silent overwrite** | Two clients edit the same block concurrently; assert the loser is **rejected with the current state to reconcile**, never silently overwritten; assert different blocks edit in parallel with no false conflict. Gate: **0 silent overwrites; cross-block edits independent.** | CAS conflict rate (the CRDT-promotion trigger metric) | KN-1 floor (EI-04 §2.1) |
| **KD-4** | **Erasure reaches every holder** (the hardest GDPR surface) | Erase a subject; assert structured PII (person props, mentions, attribution) purged/pseudonymised (pseudonym-map shred), free-text under a **per-subject DEK** crypto-shredded (key destroyed → unrecoverable in op-log/snapshots/backups), embeddings purged, backlinks tombstoned. Gate: **0 recoverable structured PII incl. vectors; residual covered by the platform posture 10.9.** | holder erase receipts, vector-tombstone lag, key-shred count (bounded: one key per subject — CR-I) | contract 10.1/10.9/11.4, X-7 |
| **KD-5** | **Permission-filtered reads — zero leak (incl. count-leak)** | A confidential page / overridden sub-page / row-restricted db / field-hidden column must **never** appear in any view / backlink / search / embed / RAG result for an unauthorized viewer — incl. an aggregate `COUNT` (the `SetExpr` conjoin is *inside* the query). Gate: **0 leaked artifacts; 0 count-leak.** | zero-escape counters | ADR-03/SC-1, T-2; contract 4.3/6.1 |
| **KD-6** | **Reindex-from-cold parity** | Wipe Knowledge's derived state (the Refs edge projection / Search index for Knowledge); `replay(scope)` (block-granular `*.snapshot`); assert the rebuilt state **matches live**. Gate: **cold == live; rebuild uses the live consumer path only.** | reindex parity hash | contract 2.6/6.4, T-5 |
| **KD-7** | **No silent data loss (outbox)** | Crash the Knowledge service *between* the block/row commit and relay-publish; assert the event is still delivered (outbox survived) and never delivered without the state change. Gate: **0 ghost, 0 lost.** | outbox depth+age | contract 2.2/2.3 |
| **KD-8** | **Hot-document thundering herd** (the OQ-K shed budget) | An all-hands doc with thousands of concurrent readers/editors; assert per-doc op in-flight cap + read-fanout bound + **active-editor lane reservation** (viewers shed before editors, agents before humans) hold the op fan-out within budget, other tenants unaffected. Include a **concurrent-same-gap LexoRank insert storm** (no key-collision reorder; bounded rebalance). Gate: **hot-doc latency within budget; other tenants unaffected; 0 reorder.** | per-tenant in-flight, op fan-out, rebalance cost | OQ-K/contract 1.11; X-3 |
| **KD-9** | **Flexible-DB query latency at scale** (TE-17 measured-promotion trigger) | Filter/sort/group a large multi-tenant database via JSONB + derived-projection + the `SetExpr` conjoin; assert read-time latency within budget; **measure** when a facet crosses the >5% promotion threshold. Gate: **query p99 within budget; promotion trigger measured.** | db-query latency, facet-execution frequency | TE-17, KN-3; contract 6.3 |
| **KD-10** | **Read-time rollup latency** (TE-18 measured-promotion trigger) | A rollup over a large related set, computed at read time (permission-filtered); assert latency within budget; **measure** when a rollup needs incremental materialisation. Gate: **rollup p99 within budget; promotion trigger measured.** | rollup latency | TE-18, KN-3; contract 13.3 |
| **KD-11** | **Agent edits are governed + attributed (the four uniform guarantees)** | An agent edits a doc via `EffectApi` → collab apply; assert the edit is attributed "suggested by agent", a consequential edit (publish/confidential) is **HITL-withheld** (returns `Denied`, does NOT mutate) until approval, a double-click is one approval (per-effect `idem_key`), denied effects return ordinary tool errors, the run passed reserve/settle. Gate: **0 ungoverned agent mutation; 0 mutation before approval; 0 double-apply.** | gate-state, denial counter, idem-key dedup | ADR-08, X-6/OQ-F; contract 8.2/9.1 |
| **KD-12** | **Agent-trace holder erasure** (AG-7) | Erase a subject; assert their content-addressed agent traces are crypto-shredded/purged, attribution falls back to the pseudonym. Gate: **0 recoverable PII in traces; attribution intact.** | trace holder receipts | contract 8.8/10.1 |
| **KD-13** | **Cross-tenant IDOR** | Attempt to read a page/db/row across tenants via path-tenant spoofing; assert zero cross-tenant read (tenant from token, the `tenant-predicate` lint catches a tenant-less query at compile). Gate: **0 cross-tenant read.** | per-tenant counters | EI-02 §1, contract 1.6/12.1 |

**Knowledge inherits the substrate/Id/Bus/Search drills** (30× agent-surge, Id-hiccup/fail-static, restore +
cross-seam, causal-loop tripwire) by standing on `serve(AppSpec)`. KD-1, KD-2, KD-3, KD-4 are the
**Knowledge-specific** headline gates.

---

## 2. Open questions for Phase 6

> The nine **design-shaped** questions Stage-1 ([`../sketches/00-findings.md`](../sketches/00-findings.md) §6)
> handed forward were committed in the Phase-4 architecture (CR-A…CR-I) and are now **folded into 01–05** against
> the frozen shapes (the deltas, [00 §0](./00-overview.md)). The table below is the residual **Phase-6** set:
> measured promotion thresholds, legal ratifications, and cross-subsystem consolidations. (Mirrors
> [06 §10](./06-reconciliation-compliance.md) R6-*.)

| # | Question | Lean / default-to-beat | Owner |
|---|---|---|---|
| KQ-1 | **The CRDT promotion timing.** When does the first true concurrent-edit conflict (R5) cross the threshold that triggers Yrs? | Measure CAS conflict rate (KD-3); promote when same-block concurrent edits exceed a measured threshold. Transport (KN-1) + editor (KN-4) are CRDT-ready from day one; the online `engine_promote` migration is built then. | Knowledge P6 (measured) |
| KQ-4 | **The flexible-DB materialisation promotion trigger** (TE-17/TE-18). | The >5% view-execution facet-promotion threshold is frozen (contract 6.3); the read-time *latency* trigger for rollup materialisation is measured by KD-9/KD-10. | Knowledge P6 (measured) |
| KQ-5 | **Field-level ABAC predicate catalogue per database.** | The mechanism (the frozen `CaveatContext` on `view_field`, off the hot path) is decided; the per-db predicate catalogue is co-designed with Id's role-bundle catalogue. | Knowledge + Id P6 |
| KQ-6 | **Synced-block editable-in-place engine** (Δ3). | v1 = the `sync_block` node + a read-projection engine (floor); editable-in-place multi-home is the named CRDT-era follow-on (reference-counted erasure + most-restrictive-of-sites permission). | Knowledge P6+ |
| KQ-7 | **Cross-cell collab for multi-cell tenants** (OQ-I). | Single-cell, residency-pinned (floor); the PII-free `CrossCellPointer` frame is frozen (12.6); true cross-cell op fan-out is owned by control-plane / multi-cell tenancy. | Control-plane P6 |
| KQ-8 | **The free-text PII erasure residual ratification** (`[OPEN — LEGAL]`). | Subsumed into the ONE platform posture (contract 10.9, X-7); counsel/DPO ratify the residual lawful basis in **one statement**, not a Knowledge-specific write-up; the structural floor ships regardless. | Legal/DPO |
| KQ-9 | **The KB-comments → Chat-threading consolidation** (OQ-L). | v1 = KB-native store over the shared `#sub`/content/refs scheme; promote onto the Chat threading primitive + the firehose transport on the real-time-presence trigger (a merge, not a rewrite). | Knowledge + Chat P6 |
| KQ-10 | **Block-vs-page search index size at world scale.** | Both, page default + significant-block; measure index size and prune block-level to useful jump targets if it grows. | Knowledge + Search P6 (measured) |

---

## 3. The gap-report seed (E-3 — the durable floor list)

Per E-3, the floors this subsystem ships, with claimed/proven status (dated 2026-06-19; all **claimed** until
the KD-* drills emit green artifacts in Phase 6):

- **CAS floor (no merge)** → Yrs CRDT (KN-1; trigger KQ-1). Claimed.
- **Read-time formula/rollup** → per-rollup materialisation (KN-3; trigger KQ-4). Claimed.
- **JSONB + derived projection** → per-facet materialisation (TE-17; frozen >5% threshold). Claimed.
- **`sync_block` = read-projection floor** → editable-in-place multi-home on the CRDT (KQ-6). Claimed.
- **Offline = read + queued light-edit** → full offline-first with the CRDT. Claimed.
- **Single-cell collab** → cross-cell op fan-out (KQ-7; the frozen pointer-bridge frame). Claimed.
- **KB-native comments (one scheme, two stores)** → Chat-threading consolidation (KQ-9). Claimed.
- **Free-text PII residual** → the platform erasure posture 10.9 (`[OPEN — LEGAL]`, KQ-8). Claimed; counsel
  ratifies; the structural floor ships regardless.

---

This completes the Knowledge Platform Phase-5-B architecture (rewritten against the reconciled shared layer).
Index: [`../README.md`](../README.md).
