# WS-I — Sovereignty-as-UX

> Workstream I (see [`README.md`](./README.md)). The EU-sovereign/GDPR promise must be *felt* in the
> product, not buried in settings (design-language P9). No external method covers this — it is novel and
> under-evidenced (Phase-1 README §5.7), so it is handled via service-blueprint + heuristic and flagged
> for a deferred regulated-buyer review. Phase-1 methods #8 (service blueprint), #15 (HAX "convey
> consequences"), #19 (heuristics).

---

## R-19 — Sovereignty-as-UX: residency / GDPR / DSR / audit legibility patterns

**Questions answered.** How are data residency, lawful basis, who/what can see a thing, agent scope, and
audit trails made **first-class legible surfaces**? How are "Where does this data live?", "Who processed
this?", and "Show me everything about this subject" answerable *in the UI* (P9)? How does the DSR
orchestrator UI (locate/export/rectify/restrict/erase across all holders, with deadline tracking and
verifiable receipts) work for the DPO (P13) — and operable by tenants (Art. 28)? How do residency/
visibility cues sit near the data without violating calm (P8) — the sketch-funnel Axis 6 trade-off?

**Phase-1 methodology.** #8 service blueprinting (blueprint the DSR/erasure flow + the residency console:
frontstage screens → backstage DSR orchestrator / audit log / tombstoning → the DPO actor); #15 HAX
("convey consequences" of an erasure); #19 heuristics (P9 sovereignty-as-UX heuristic).

**Inputs.** design-language P9 (sovereignty as UX), §7.6 (the GDPR/data-rights console, data-map/RoPA &
residency console, audit-log explorer, tenant/cell & residency settings, agent governance console), §5.3
(the tombstone/erased state); `system-overview.md` §8.3 (DSR fan-out); R-04 (the DPO DSR cross-surface
flow); the gdpr-and-audit architecture (`05-refined/.../gdpr-and-audit.md` — the DSR orchestrator,
crypto-shred, data map, restriction flag — to *surface*, not redesign); `competitive-landscape.md` §6.2
(what EU-sovereign must mean).

**Deliverable.** `design-planning/04-research/sovereignty/sovereignty-as-ux.md`. The sovereignty-as-UX
pattern set: the residency/visibility cue patterns (where they sit near data — the scope indicator's
region/residency cue, per-artifact visibility chip); the DSR console blueprint (locate/export/rectify/
restrict/erase across holders; deadline tracking; verifiable receipts; the data-subject view AND the DPO
view); the data-map/RoPA & residency console; the audit-log explorer with provenance/correlation
threading; the agent governance/kill-switch surface; and the erased/tombstoned UX (the GDPR-aware
degraded state). Plus the Axis-6 trade-off articulation (always-on cues ↔ on-demand consoles). Tag
PROVEN (where a GDPR/EN-301-549 requirement backs it) vs HOUSE STYLE.

**Sequencing & dependencies.** Seq #17. Depends on R-04 (the DSR flow). Feeds the rubric D9, sketch-funnel
Axis 6, and Phase 6 (the corporate/governance approachable surface can be a sovereignty console).

**User-dependency.** none for the patterns/blueprint; the **regulated-buyer (P13/P14) review is
deferred-until-users** (a DPO/procurement review substitutes for user testing — carried from README
§5.7).

**Effort.** M.

**Acceptance criteria.** Residency/visibility cues are placed concretely near data; the DSR console is
blueprinted from both the data-subject and DPO sides; the erased/tombstoned UX is specified; the
audit-log explorer surfaces provenance/correlation; the patterns surface the existing gdpr-and-audit
mechanics rather than inventing them; the Axis-6 trade-off is articulated; the deferred regulated-buyer
review is recorded; sovereignty-as-UX is honestly tagged under-evidenced where it is HOUSE STYLE.
