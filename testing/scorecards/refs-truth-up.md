# Reference-graph truth-up pass — every PROVEN Refs row rests on a dated green artifact (REF-P28 / P-513, REF-M6)

Run date: 2026-06-26

The code wins over the docs (EI-01 §1): each Refs PROVEN row below names its DATED green artifact (the
`cargo test` target that emits it + the proof-source file on disk), not a doc claim. The pass is GREEN iff
EVERY row rests on a dated artifact WHOSE SOURCE EXISTS ON DISK — the gate invariant holds end-to-end (no
earlier-band Refs gate is red).

Rendered by `myelin_refs_service::dogfood::run_refs_truth_up_scorecard` over the FROZEN `proven_refs_rows`
set; the committed test
`tests/ref_p28_dogfood_drill.rs::the_truth_up_pass_confirms_every_proven_refs_row_is_dated` asserts 0
claimed-not-proven rows (the gate the CI must not swallow). A row that names a vanished artifact is surfaced
as CLAIMED-NOT-PROVEN, never trusted on faith.

| Gate / drill | §face | Dated artifact | Proof command |
|---|---|---|---|
| `REF-D1` — the resolve chokepoint: per-viewer gate; a denied target tombstones (root-only), 0 title/count/backlink leak | 5.2 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_2_resolve` |
| `REF-D2` — the leak-free backlink read: list_objects SetExpr lowered into the per-tenant authz reverse index, 0 cross-tenant leak | 5.3 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_3_backlinks` |
| `REF-D3` — the tombstone / graceful-degradation ladder: a tombstone always carries the root | 5.7 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_7_sub_ladder` |
| `REF-D4` — the bounded cycle-safe lineage traverse: per-viewer prune, 0 leak | 5.3 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_3_traverse` |
| `REF-D5` — the event-sourced edge inverse index: deterministic edge_id, idempotent rebuild | 5.4 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_4_edge_builder` |
| `REF-D6` — the TE-7 typed-edge mirror: typed table = source of truth, Refs = rebuildable projection | 5.5 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_5_mirror` |
| `REF-D7` — the PersonalDataHolder structural-erasure surface: reaches edges + cache, 0 recoverable PII | 10.1 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test integration_ref_p15_holder_erase` |
| `REF-D8` — reindex-from-source: the rebuilt index byte-matches the live projection (parity) | 5.8 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test cdc_5_8_reindex` |
| `REF-D9` — reindex-from-source parity AT SCALE: byte-match live over the five-producer corpus | 5.8 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test ref_d4_reindex_parity_at_scale` |
| `REF-D10` — the 30× backlink surge: DRR-fair shed within budget; restore + re-erase at backup scale, 0 recoverable PII | 12.6 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test ref_d10_surge_drill --test ref_d5_restore_reerase_at_backup_scale` |
| `E2E-1` — the PR context pane: per-viewer unfurl; mid-flight ci.check.updated live-update; denied issue tombstones, 0 leak | 5.2 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test e2e_wedge_ref_p27` |
| `E2E-3` — spec-to-ship traceability: lineage traverse depth-16 per-viewer (0 leak) → wipe → reindex → byte-match | 5.3 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test e2e_wedge_ref_p27` |
| `E2E-4` — the DSAR fan-out: holder fan-out reaches edges + cache, unfurls degrade to tombstones, 0 recoverable PII | 10.1 | [2026-06-26] PROVEN | `cargo test -p myelin-refs-service --test e2e_wedge_ref_p27` |

**TRUTH-UP: GREEN** — 13 PROVEN Refs rows, 0 claimed-not-proven; the gate invariant holds end-to-end (no
earlier-band Refs gate is red). The engine is fixed at M2 and hardened through M5 — REF-P28 promotes
nothing; it exercises the production-hardened reference graph on real (self-)tenant data.

**Named floor (REF-P29):** the reference-graph **switch-test surfaces driven in a browser** are the M6
follow-on (REF-P29) — the switch-test verdict (a Refs surface is done only when someone could move to it
without hitting a wall the old tool didn't have) is reached by *driving* the real surface in a browser
(EI-01 §4), not by this in-process drill. The world-scale 30× FLEET-hardware load drill (REF-D10 at true
multi-box fleet scale) remains the one legitimate remaining infra floor — the single-box SCALED surge runs
green here.

## The reference graph over Myelin's own work (live)

`myelin_refs_service::dogfood::run_refs_over_myelins_own_work` drives the production Refs surface over the
Myelin self-tenant (`tenant=myelin`, `region=fr-par`) across three faces — REUSING the SAME resolve
chokepoint / traverse / reindex / holder engine (EI-01 §7, never a second implementation):

- **the PR context pane** on the Myelin monorepo's PRs (commits ↔ issues ↔ CI checks ↔ Knowledge docs ↔
  chat threads unfurl per-viewer through the one graph);
- **the spec-to-ship lineage** on Myelin's roadmap / gap-report / scorecard living as Myelin issues + a
  Myelin Knowledge space (the full lineage traverse + reindex-from-source parity);
- **the structural-erasure holder fan-out** over a Myelin team member's own data (0 recoverable PII).

All three faces green, 0 leak. Wired as the Myelin CI job `REF-P28-dogfood` in
`myelin_harness::self_hosting_ci::self_hosting_jobs` — the dogfood loop is live (it re-runs on every Myelin
commit and reds the self-hosting CI gate on any regression).

## The every-incident-adds-a-drill loop on Myelin's own tracker (live)

A Refs incident (`RefsIncident`) files a PII-free Myelin **issue** draft AND a reproducing-**drill** ticket
— both reference-linked (the moat thesis: the issue points at the drill that reproduces it). The committed
test `tests/ref_p28_dogfood_drill.rs::a_refs_incident_files_an_issue_and_joins_the_permanent_drill_suite`
registers the repro into the harness `DrillRegistry` via the T-3 `register_drill` hook and proves it re-runs
green forever (a regression would re-red it loudly).
