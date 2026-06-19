# Sketch 09 — Import fidelity from Jira/Linear (PR-8)

> Exploration note. Weighs Phase-2 §11 Q11 / deep-dive §10: high-fidelity import is the **adoption gate** and a
> credibility signal for the "leave Atlassian cloud cleanly" sovereignty pitch. Leans; commit in `00-findings.md`.

## Why it's a hard problem (not just throughput)

Deep-dive §10.4: it's a **correctness/migration-engineering** problem more than a scale one — ID remapping, link
integrity, history fidelity, user matching, lossy rich-text/permission conversion. A company won't leave Jira
unless import is trustworthy, *and* the convert must be lossless enough that the switch-test passes (T-7).

## The mapping problems, ranked by difficulty

| Source concept | Maps to | Difficulty / hazard |
|---|---|---|
| Issues, types, priorities, labels | `issue` rows, type-scheme types (sketch 01/02) | easy; type/status name mapping |
| Statuses → workflow | named states + the **fixed category** mapping (sketch 02) | medium — must map every source status to a category; unmapped → `unstarted` + flag |
| Hierarchy (epics/sub-tasks/initiatives) | `issue_relation parent` (ranked types, sketch 01) | medium — Jira "Epic Link" → `parent`; Linear Projects/Initiatives → ranked-type parents |
| Issue links | `issue_relation` rel set (`blocks/depends_on/relates/…`) | medium — **a mapping table** (Jira link types ≠ ours; semantics differ, deep-dive §10.2) |
| Rich text (Jira ADF / wiki-markup; Linear markdown) | `myelin-content` AST + markdown-subset inline (ADR-05/KN-2) | **hard, lossy** — ADF→content is the messiest; flag lossy nodes |
| Custom fields | field-scheme typed fields + the JSONB tail (sketch 03) | hard — type coercion; unmapped → JSON bag (deep-dive §10.2) |
| JQL saved filters | the shared query AST (ADR-07) | medium — JQL→AST compile; some JQL has no clean AST analogue → flag |
| Permission schemes | ReBAC tuples (identity §5) | **hard, lossy** — Jira permission schemes → ReBAC; deep-dive §10.3 calls it lossy; needs review |
| People (reporter/assignee/watcher/author) | identity principals | **hard** — user matching/merging; deactivated/erased source users → pseudonymous placeholder |
| History/change-log | the change-log (deep-dive §3.9) | configurable depth — full history is expensive + PII-laden (deep-dive §10.2) |
| Sprints/cycles, versions/releases | `cycle` object (sketch 01), milestones | medium |
| Attachments | shared `BlobStore` (STOR-1), residency-pinned | volume + residency |

## The engineering shape (deep-dive §10.4 — the known-good pattern)

### Candidate A — Two-pass, ID-remapped, idempotent, dry-run-first (the cited best practice)
1. **Pass 1: create all entities**, recording a **source-ID ↔ Myelin-ID map** (persisted — also enables
   incremental re-sync + rollback).
2. **Pass 2: wire links/parents/relations** against the map (avoids forward-reference problems — you can't link
   to an issue that isn't created yet).
3. **Idempotent + resumable**: a large import (hundreds of thousands of issues + attachments) must resume after
   failure without duplicating — keyed on the source-ID map (re-running skips already-created).
4. **Dry-run + mapping preview + reconciliation report** (what mapped / what was lossy / what was dropped) —
   *essential for trust* (deep-dive §10.4). The import wizard (S17) shows this before committing.
5. **Rate-limit-aware backfill** against the source API.

- **For:** this is the proven migration pattern; every leg is named in the research. The source-ID map is the
  load-bearing artifact (idempotency, resume, rollback, re-sync, and the export-symmetry check).
- **For (events):** import emits the **same `issue.*` events** as normal creates — so Search/Refs/OLAP index the
  imported data through the *one* live consumer path (no special import-indexing path; reindex-from-source works
  on imported data for free). Per-tenant in-flight caps (X-3) keep a giant import from starving other tenants
  (reference-graph §6.1 names the "giant import" as the fairness case).

### Candidate B — Streaming one-pass with deferred link resolution
- **Against:** still needs the ID map + a deferred-link queue → it *is* two-pass with extra state. No win.

## Symmetry with export / portability (the round-trip)

Deep-dive §10.4 + §8.6: the **same canonical interchange format** used for GDPR/portability export should
**round-trip** with import. So:
- Define **one canonical interchange schema** (issues + schemes + relations + history + people-map + attachments
  manifest). Export (`myelin export --format canonical`, deep-dive §11) produces it; import consumes it.
- This gives the anti-lock-in promise (design-language §7.6 "exit") *and* a test oracle: **export→import→export
  must round-trip** (a PROVE-IT drill — import-fidelity round-trip).
- The Jira/Linear/GitHub importers are **adapters** that normalise the source into the canonical interchange,
  then the canonical importer runs once. (One importer core, N source adapters — abstract at the third source.)

## The lossy-by-honesty rule

Where a mapping is lossy (ADF rich text, Jira permission schemes, JQL with no AST analogue), the **reconciliation
report names it explicitly** (deep-dive §10.4) rather than silently dropping — the "name your floors" discipline
(VISION §5.4) applied to import. The user decides whether the residual loss is acceptable before committing.

## Leaning

**Candidate A — two-pass, ID-remapped (persisted source-ID↔Myelin-ID map), idempotent+resumable, dry-run +
reconciliation-report-first**, with source adapters (Jira/Linear/GitHub/CSV) normalising into **one canonical
interchange format that round-trips with the portability export**. Import emits the normal `issue.*` events
(one indexing path; per-tenant capped). Lossy mappings are named in the reconciliation report, never silently
dropped.

## Hands forward

- The canonical interchange schema definition (the round-trip oracle) — architecture.
- The link-type + status-category + permission-scheme mapping tables — architecture (the lossy ones flagged for
  legal/user review on permissions).
- ADF→`myelin-content` converter fidelity (co-design with Knowledge, who owns the content taxonomy) —
  architecture.
- PROVE-IT: export→import→export round-trip drill; large-import resume-after-crash drill; import-doesn't-starve-
  other-tenants drill (X-3) — findings §drills.
