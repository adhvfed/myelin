# Phase 6 — Roadmap: Cross-Artifact Reference Graph (`myelin-refs`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **reference-graph** shared system.
> Slots into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§2 bands, §3 critical-path/DAG, §4 gate
> invariant, §5 name-your-floors). Frozen architecture (this roadmap SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/reference-graph.md`](../../05-refined-shared-systems-architecture/reference-graph.md)
> (the refined Refs architecture) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §5 (the contracts Refs owns) + §4/§2/§12/§13 (the contracts Refs consumes). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (REF-D1..REF-D10) + architecture §7 (the ten carried-forward drills D-1..D-10). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (order-by-non-negotiability; name-your-floors; the committed gates; prove-it-or-it-isn't-real) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1
> (erasure vs immutability), §5 (Search + the reference graph are easy to under-budget; reindex-from-source is
> the resilience primitive). Spine: ADR-13 (the three glue contracts + the Reference-Graph clause + TE-7),
> ADR-14 (PG-by-default), ADR-03 (`list_objects`), ADR-04 (events authoritative), ADR-05 (the
> `mention`/`artifact_ref`/`embed` nodes produce edges), ADR-06 (relation field type → typed tables), ADR-11/12
> (cells/holder). Directives: REF-1..REF-4, X-1, X-4, ID-3, GD-3. Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** Refs is **a thin, event-sourced projection
> over the platform's edge facts** (architecture §1.3). It owns **one contract crate** (`myelin-refs`: the
> `ArtifactRef` value type + the edge/backlink/traverse client + the `#sub` grammar + the tombstone ladder) and
> is otherwise **overwhelmingly a consumer** — it composes `myelin-identity` (`list_objects`/`check`/zookie),
> `myelin-events` (the outbox, the consumer template, reindex-from-source), `myelin-content` (the three
> structured inline ref nodes that *produce* edges), `myelin-query` (no direct dep, but the `QueryAst` anchors
> the predicate vocabulary it never re-invents), and each subsystem's `project(ref, viewer)`. It holds **only
> derived, reconstructible state**: the `edge` inverse index (R1) and the projection cache (R2) are both
> rebuildable from the log by reindex-from-source; the TE-7 typed relation tables are **not Refs' components**
> (they belong to Issues/Knowledge — Refs holds only the rebuildable projection of them). Three consequences
> for the roadmap: (1) Refs **cannot start its core build until its upstreams are frozen and green** — the
> `list_objects` `SetExpr` push-down (4.3), the durable outbox + consumer template (2.x), the
> `EventEnvelope`/`ArtifactRef` token table (2.1 / Bus §6.2), the `myelin-content` three inline ref nodes
> (13.1), the KMS/per-subject-DEK hierarchy (11.3/11.4). (2) Its two cardinal invariants — **a viewer must
> never find a reference from an artifact they cannot read** (F1, the backlink leak) and **no edge is ever lost
> or ghosted across a producer crash** (F5, the dual-write hazard) — are not Refs-local features; they are
> properties of how Refs composes upstream contracts (the `list_objects` pre-filter; the outbox), so they are
> drilled the moment the composition exists, never deferred. (3) Refs is **the cell-local chokepoint that makes
> every unfurl/embed/backlink non-leaking** — a confidential artifact degrades to a tombstone *here*, once, for
> every subsystem; the rest is engine integration.

---

## 0. Where Refs lands in the master bands (the one-paragraph map)

Refs' **core build is M2** (the reactive shared layer — the band that builds "the connective tissue every
subsystem projects onto", master §2 M2). Nothing of Refs' engine ships before M2 because every Refs read path
calls `list_objects` (M1), consumes the outbox (M0), reads `project(ref, viewer)` (M2, declared by each
subsystem), and is residency-pinned + crypto-shred-capable (M1). But Refs is **named and seeded in M0 and M1**:
its `ArtifactRef` value type + the `parse`/`format` ambiguity-rejection ship as part of the **M0** glue-crate
skeleton (`myelin-refs` is one of the eight compile-time contract carriers, master §2 M0 / ADR-01) so the
names/units anchor everything; its `PersonalDataHolder` auto-registration is part of the **M1** GDPR holder
floor (the holder list must be exhaustive before any real data, contract 10.1). Refs' **producer-fed edges
light up incrementally across M3/M4** as each subsystem ships the `mention`/`artifact_ref`/`embed` content
nodes (M3 Git/KN; M4 CI/Issues/Chat) and its typed-lifecycle events (Issues `issue_relation`, KN `db_relation`/
`page_parent`). Refs' **world-scale hardening + the one floor follow-on (cross-cell backlink fan-out build) are
M5** (the 30× surge family, the hot-artifact reach index R4 if measured-triggered, the cross-cell fan-out built
when multi-cell goes live). Refs participates in the **M5 whole-system E2E wedge** (E2E-1 the PR pane — Refs is
the spine of it; E2E-3 reindex-parity + lineage traversal; E2E-4 DSAR tombstone degradation) and in the **M6
dogfood**.

The honest progression: **first runnable** = M2 (the leak-free edge index + resolve/backlinks/traverse on a
single tenant, with the `#sub` ladder); **first useful** = late M3/M4 (real producer edges — git commit/PR
links, KN embeds, issue relations, chat unfurls — traversable per-viewer, the PR context pane lights up);
**production-hardened** = M5 (30× surge holds, the viral-PR hot-fanout paged within budget, reindex-from-cold
byte-parity, cross-cell resolution proven cell-local).

---

## 1. The contracts Refs owns / consumes, mapped to the milestone they land in

From contract-index §5 (owned by Refs), §4/§2/§12/§5.9/§13 (consumed). "Lands" = the milestone by which the
contract must be implemented or callable for Refs' gate to be green.

### 1.1 Owned by Refs (contract-index §5)

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 5.1 | `ArtifactRef` — `myelin://<tenant>/<subsystem>/<type>/<id>[#sub]`; `parse`/`format` reject ambiguity; Issues key `<PROJECTKEY>-<seqno>` frozen, `#1421` render-time | **M0** value type + parse/format; **M2** resolve wiring | the value type is a glue-crate carrier (M0, ADR-01) so every system links the same names early; the resolver lands M2. |
| 5.2 | `resolve(ref, viewer, mode) → Projection \| Tombstone` — live per-viewer unfurl/embed; denied → tombstone; `Display` = the Notif humanisation projection; cross-cell resolution **always cell-local** | **M2** (single-cell); **M5** cross-cell build | the chokepoint that makes unfurls non-leaking. Cross-cell *semantics* frozen now; the cross-cell fan-out *build* is the M5 named floor. |
| 5.3 | `backlinks/edges/traverse` — leak-free inverse via `list_objects`; bounded cycle-safe recursive-CTE walk (depth 16) | **M2** | the crux — conjoins the OQ-E `SetExpr` `Filter` over `source_root`. |
| 5.4 | `refs.edge.created` / `refs.edge.removed` — emitted by producers via outbox; the `mention`/`artifact_ref`/`embed` content nodes are the producers; **no standalone edge-write API** | **M2** consumer + the M0 grammar; **per-producer M3/M4** | Refs *consumes* these; the producers (the three inline nodes) ship per subsystem (KN/Git M3; Issues/Chat M4). |
| 5.5 | TE-7 typed-edge mirror — lifecycle edges (`closes/blocks/blocked_by/depends_on/parent/assigns/relates`) dual-homed; the typed table is truth, Refs is the rebuildable projection + fixes inverse pairing | **M2** vocabulary + mirror discipline; **per-owner M3/M4** | Refs fixes the `rel` vocabulary + inverse pairing in M2; Issues/KN own the rows + emit typed events (Issues M4, KN M3). |
| 5.6 | `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}` — REQUIRED on every subsystem; the only way Refs reads another subsystem's artifact | **M2** shape frozen; **per-subsystem M3/M4** | Refs *consumes* this; each subsystem *implements* it (the `sub_anchor` resolver returns the frozen `live/moved/outdated/gone` state). |
| 5.7 | Unified `#sub` grammar (frozen) + the one 4-step tombstone ladder; Git line-ranges **content-anchored** (BLAKE3 + 3-way context match → exact/rebased/partial/tombstone) | **M2** grammar + ladder; **per-subsystem mint M3/M4** | Refs owns the grammar + the ladder (M2); each subsystem owns the **stable mint** (a block id survives moves, a message/comment id is immutable, a Git range carries the fingerprint) — its P6 deliverable, asserted by REF-D9. |
| 5.8 | `reindex(scope)` (Refs) — reindex-from-source for the edge index + projection cache; never reads owner DBs | **M2** | depends on Bus 2.6 sub-artifact-granular `*.snapshot` replay (M2) + each owner's `replay` (M3/M4). |
| (10.1) | `PersonalDataHolder{locate/export/rectify/restrict/erase}` — Refs is a holder (small, structural surface: opaque ids + cache titles, never third-party free-text bodies) | **M1** registration; **M2** real erase mechanism | auto-registered by the harness (1.4) in M1 so the holder list is exhaustive; the real R2-cache-purge + `*.erased`-tombstone erase runs once the index exists (M2). |
| (1.8) | telemetry signal set — `backlink_read_latency`, `resolve_cache_hit_ratio`, `index_lag`, `hot_artifact_fanout`, `tombstone_count`, `reindex_parity` | **M2** | every Refs drill asserts against this; no signal = failed drill (T-1/T-3). |

### 1.2 Consumed by Refs — the upstream dependencies that must exist first (contract-index §4/§2/§12/§5.9/§13)

| # | Consumed contract | From | Must be green by | Why Refs blocks on it |
|---|---|---|---|---|
| 4.3 | `list_objects(...) → Ids \| Filter{set_expr, zookie}` with the frozen `SetExpr` algebra (lowered over `source_root`) | **Id (M1)** | **M1** | the leak-free pre-filter — the single most load-bearing dependency. Refs cannot do a backlink read safely until this is frozen + green. |
| 4.2 | `check(subject, perm, object, zookie?, caveat?)` | **Id (M1)** | **M1** | step 1 of the resolve ladder (denied → tombstone, never leak) + the per-viewer projection gate. |
| 4.10 | `Consistency`/zookie + the authz reverse-index revision watermark | **Id (M1)** | **M1** | no-stale-grant via backlinks ("new enemy", REF-D6) + zookie-stamped reads bypass fail-static. |
| 4.8 | `resolve_pseudonym(subject)` + `erase(subject)` + the frozen `<pseudonym>@<tenant>.noreply` grammar | **Id (M1)** | **M1** | `origin_actor` is a stable opaque pseudonym so erasing the person needs **no edge mutation** (the common case); Id's pseudonym shred makes the id unresolvable (REF-D5). |
| 2.1 | `EventEnvelope` — the names/units anchor + the `ArtifactRef` subject + causality fields | **Bus (M0)** | **M0** | every `refs.edge.*` carries it; `OutboxTx::emit(draft, cause)` sets the causal depth the loop guard reads (AG-6). |
| 2.2/2.3 | `OutboxTx::emit` + the `outbox` table (per-**ref** aggregate ordering, `UNIQUE(aggregate, seq)`) | **Bus (M0)** | **M0** | the **only** sanctioned emit path — edges are born iff their content/relation commits (REF-D7, the no-ghost floor). |
| 2.4/2.5 | `EventHandler` consumer template + `consumer_dedup` ledger | **Bus (M0)** | **M0** | `refs-edge-builder` + `refs-projection-invalidator` are ordinary template consumers; idempotent on `event_id`. |
| 2.6 | `reindex(scope)` re-emit + `*.snapshot`/`*.erased`, **sub-artifact-granular** | **Bus + every subsystem (M2 seam; per-owner M3/M4)** | **M2** seam | rebuild + erasure; the only recovery path (REF-4). |
| 2.9 | event taxonomy + the `<subsystem>/<type>` token table (incl. `ci.check.updated`/`ci.result`, type token `initiative`) | **Bus (M0 grammar; tokens land with producers)** | **M0** grammar | Refs is the **validator and primary consumer** of the token table, not a second authority (contract §14). |
| 13.1 | `myelin-content` taxonomy — the three structured inline ref nodes (`mention`/`artifact_ref`/`embed`), byte-identical across Chat/Issues/KN | **`myelin-content` (M2 frozen); per-producer M3/M4** | **M2** frozen | these nodes are the **producers** of `refs.edge.created` — extraction is structured-node-driven, not regex over prose (the reliability guarantee). |
| 5.6 | `project(ref, viewer)` + the `sub_anchor` resolver | **each subsystem (M2 shape; M3/M4 owners)** | **M2** shape; per-owner later | Refs fetches the per-viewer projection (NOT the owner DB) + resolves the `#sub` ladder state. |
| 5.5 | the typed relation tables, read **only via their events** (Issues `issue_relation`; KN `db_relation`/`page_parent`) | **Issues + Knowledge (M4 / M3)** | **M3 (KN)/M4 (Issues)** | the TE-7 lifecycle-edge truth; Refs projects, never owns; on drift a scoped reindex reconverges to the typed table (which wins, REF-D4). |
| 12.6 | cross-cell PII-free pointer bridge — `CrossCellPointer{subject, type, correlation_id, home_cell}`; resolution always cell-local | **control plane (M1 frame; M5 live)** | **M5** | cross-cell backlink/embed resolution rides it (designed-and-extends until M5). |
| 11.3/11.4 | KMS hierarchy (per-tenant DEK + per-subject DEK backstop) + crypto-shred | **Storage/GDPR (M1)** | **M1** | the edge index + R2 cache are per-tenant envelope-encrypted, crypto-shred-capable; the projection cache may hold a name in a title. |
| 1.1/1.2/1.8 | `serve(AppSpec)` + three-surface + telemetry | **substrate (M0)** | **M0** | the service shell. |
| 1.6 | the lints `tenant-predicate`, `no-raw-publish`, `no-cross-db`, `no-cross-sync-cycle` (the four Refs leans on) | **substrate/CI (M0)** | **M0** | compile-time: no cross-tenant edge query, no edge escaping the outbox, no cross-DB read of an owner, no synchronous cross-subsystem call cycle. |
| 1.9/1.10 | `ResilientClient` (for `project` calls) + `FailStatic<T>` | **substrate (M0)** | **M0** | Refs calls owners' `project` through the resilient client; fail-static under an Id hiccup. |
| 1.11 | protected-human-lane shed order + per-surface shed budgets (OQ-K) | **harness + Refs budget (M0 harness; M5 tuned)** | **M2** mechanism; **M5** numbers | the backlink-read + ref-creation surfaces are shed lanes; REF-D10 proves it. |

**The critical upstream dependency, stated plainly:** Refs' entire correctness story is downstream of
**Identity 4.3 (`list_objects` `SetExpr` push-down)** and the **Event-Bus outbox (2.2/2.3)**. The first makes
the backlink read leak-free by construction (you see a backlink iff you can see the artifact that made the
reference — REF-1); without it frozen + drilled green in M1, Refs cannot begin M2 — there is no leak-free
traverse to build. The second makes an edge exist iff its content commits — the no-ghost/no-loss floor (F5);
without the outbox green in M0, every `refs.edge.created` is a dual-write hazard. The third hard dependency is
the **`myelin-content` three inline ref nodes (13.1)** in M2: edges are *produced* by those structured nodes,
so until they are frozen byte-identical (X-2), the producer is non-uniform and extraction is unreliable.

---

## 2. The sequenced milestones (Refs' slice of each band)

Each milestone below states **the work**, the **floor-then-full progression** (each floor named with its
scheduled follow-on), the **upstream dependencies** (what must be green first), and the **quantified
gates/drills** that call it done. Drill thresholds carry the Q32 defaults-to-beat; Phase 6 measures the final
numbers (EI-02 §8): depth ceiling 16 (traversal), hot-fanout read budget (R4 promotion trigger), surge 30×,
index-lag alarm, R2 cache TTL ≤ revocation SLA.

---

### R-M0 — The `ArtifactRef` value type + the Refs ratchet (inside master band M0)

**Master band:** M0 (substrate, harness, committed gates — the glue-crate skeleton).

**The work (Refs' contribution to M0 — the contract-carrier crate + the lints it leans on, not Refs' engine):**
- **Ship the `myelin-refs` glue crate as a compile-time contract carrier** (ADR-01, master §2 M0): the
  `ArtifactRef` value type + `parse(&str) → Result<ArtifactRef>` / `format(&ArtifactRef) → String` with the
  **ambiguity-rejection** (REF-3): scope is explicit and total (`tenant`/`subsystem`/`type`/`id` all required);
  a scope-less / short-hash ref (`#42`, `@alice`, `~general`, a 7-char prefix) is **rejected, never guessed**.
  The Issues key grammar `<PROJECTKEY>-<seqno>` is frozen as the stored canonical `<id>` (C-3); `#1421` is the
  render-time display projection (never stored, never resolved as a scope). The **frozen `#sub` grammar
  vocabulary** (`comment-`/`thread-`/`message-`/`b`/`h`/`row-`/`field-`/`L<a>-L<b>`/`check-`/`step-`) ships as
  the parse/format target now so every later producer mints into the same self-describing grammar (C-1/C-6).
  This crate is the names anchor: a change to it breaks every consumer's build *now*, never silently in prod.
- **Validate (do not author) the `<subsystem>`/`<type>` token table** owned by Bus §6.2 (2.9): Refs is the
  validator and primary consumer, not a second authority. The token set + the `initiative` type token + the
  `ci.check.updated`/`ci.result` tokens (X-1) are registered here as the parse vocabulary.
- **Lean on the four committed lints** (contract 1.6, the M0 ratchet — Refs does not own them but its later
  code is structurally bound by them): `tenant-predicate` (no cross-tenant edge query compiles — every `edge`
  query carries the `tenant` predicate, ID-3), `no-raw-publish` (no edge escapes the outbox — there is no
  standalone edge-write API, 5.4), `no-cross-db` (Refs never reads an owner's DB — only `project`/events),
  `no-cross-sync-cycle` (Git never synchronously calls CI to ask "is it green"; every cross-subsystem edge is
  an async event/projection — the acyclicity rule, master §3.2). Each ships with its red+green fixture in M0.

**Floor-then-full:** none — this is the contract crate + the ratchet, not a feature. (The value type is
complete at M0; the *resolver* over it is the M2 floor's follow-on. Named so the value type is not mistaken for
the working graph.)

**Upstream dependencies:** the Cargo workspace + glue-crate skeleton (M0 substrate); the `EventEnvelope` (2.1)
+ the `ArtifactRef` token table (Bus §6.2) frozen as the anchor; the lint framework + the contract-coverage
scanner (M0 substrate).

**Gate (Refs' piece of the M0→M1 boundary — the "all 12 lints green w/ fixtures" + "contract-coverage scanner
passes" clauses):**
- The `myelin-refs` crate compiles and is linked by every consumer (a change to `ArtifactRef` breaks the
  workspace build — the ADR-01 property). `parse`/`format` round-trip + ambiguity-rejection unit + property
  tests green (a fuzz corpus of malformed/short-hash/ambiguous URNs is rejected, never guessed). — **CI.**
- The four lints Refs leans on (`tenant-predicate`, `no-raw-publish`, `no-cross-db`, `no-cross-sync-cycle`)
  green with both fixtures, wired into CI, loud, never `|| true`. — **CI.**
- The contract-coverage scanner passes on the `myelin-refs` contract rows (5.1 has a provider+consumer CDC
  stub). — **CI.**

---

### R-M1 — Refs as a holder + the edge-index encryption floor (inside master band M1)

**Master band:** M1 (Identity + storage durability + tenancy).

**The work (Refs' contribution to the M1 data-loss/holder floor — still no engine yet):**
- **Register Refs as `PersonalDataHolder`** via harness auto-registration (contract 1.4) so the H1–H18 holder
  list is **exhaustive before any real tenant data exists** (10.1). At M1 the holder is a stub — it has no edge
  index to purge yet — but it is on the list, so the M5 DSAR fan-out cannot silently miss it. Refs' erasure
  surface is **small and structural by design**: it holds only pseudonymous opaque ids (`origin_actor`) and
  cache titles, **never third-party free-text bodies**, so its residual is the one platform posture
  instantiated *by reference* (10.9 / X-7) — Refs adds **no new `[OPEN — LEGAL]` residual**.
- **Pin the per-tenant DEK for the (future) `edge` index + R2 cache into the KMS hierarchy** (11.3): per-cell
  root → per-tenant KEK → per-tenant DEK as the tenant-decommission crypto-shred unit; the per-subject DEK
  (11.4) backstops a name that lands in a cached title. No index exists yet; this reserves the key class so
  M2's index is encrypted-from-birth.
- **Confirm the residency-pin** applies to the (future) per-tenant `edge` table + R2 cache: all Refs state is
  cell-local, `(tenant, region)`-partitioned, **no cross-tenant query path** (EI-02 §1; ID-3). The
  `residency-pin` + `tenant-predicate` lints (M0) already enforce it structurally.

**Floor-then-full:**
- **Floor: per-tenant DEK** (the crypto-shred + backup-backstop unit). **Follow-on: the structural erasure
  surface** (R2-cache PII purge + reliance on Id's pseudonym-map shred for `origin_actor` + `*.erased`
  tombstoning of content-erased targets) lands in **R-M2** once the index exists. Named so the DEK is not
  mistaken for the whole erasure answer.

**Upstream dependencies (must be green to do this work):**
- **M1 Identity** must exist (Refs' holder + future read path are meaningless without it) — but Refs' M1 work
  only needs the **holder harness (1.4)** + the **KMS hierarchy (11.3/11.4)**, both M1 storage/GDPR.
- The **M1 exit gate itself** — STOR-D1/STOR-D2 (restore-verify, the silent-data-loss floor), ID-D3
  (cross-tenant 0), ID-D2 (fail-static), ID-D1 (disabled-user N=5min), CP-D2/CP-D3 (misroute + residency-pin)
  — **must be green before Refs' M2 core build starts.** Refs inherits these; it does not re-prove them, but it
  cannot build the edge index over a red STOR-D1.

**Gate (Refs' piece of the M1→M2 boundary — inherited platform gates Refs depends on, plus Refs' holder
check):**
- Refs appears in the harness-generated holder registry (a structural check, not a drill) — **0 stores
  unregistered** (the contract-coverage scanner confirms 10.1 coverage). — **CI (structural).**
- The per-tenant Refs DEK is a destroyable key in the KMS hierarchy (proven later by REF-D5 in M2/M5; at M1 the
  check is structural: the key class exists and `destroy` is callable). — **CI (structural).**

---

### R-M2 — The Refs core: leak-free edge index + per-viewer resolution + the `#sub` ladder (master band M2)

**Master band:** M2 (the reactive shared layer + the safety drills). **This is Refs' primary build milestone**
— the band whose thesis is "build the connective tissue every subsystem projects onto — the reference graph,
search, the one inbox, the agent fabric, and the durable workflow engine" (master §2 M2). Contract-index 5.1–5.8
land here.

**The work (the full Refs engine, single-cell, single-tenant-correct):**
- **The `edge` inverse-index schema + the two consumers** (architecture §3.2, §4.3): the `edge` table
  (`(tenant, region)` first, RLS, per-tenant DEK) as the **materialised projection** of
  `refs.edge.created`/`refs.edge.removed` — `edge_id = hash(tenant, source, target, rel)` (deterministic →
  idempotent rebuild), the `source`/`target` full sub-URNs **and** the `#sub`-stripped `source_root`/
  `target_root` columns (the hot inbound index + the C-4 filter column), the `rel`/`rel_class` (reference vs
  lifecycle) seam, `origin_actor` as a **pseudonymous** Principal ref (erasure-safe), the `zookie` at
  edge-write time, the indexes `edge_inbound (tenant, target_root)`, `edge_outbound (tenant, source_root)`,
  `edge_by_rel (tenant, target_root, rel) WHERE rel_class='lifecycle'`. Two ordinary `EventHandler` consumers
  (2.4): **`refs-edge-builder`** (whitelists `refs.edge.>` + the typed-lifecycle subjects `issue.relation.>` /
  `knowledge.page.>` — one of the explicitly reviewed firehose-class infra consumers, BUS-4; upsert on
  `created`, delete on `removed`, tombstone on `*.erased`; ack-after-apply, idempotent on `event_id`) +
  **`refs-projection-invalidator`** (busts R2 on `*.updated`/`*.erased`). **Steady-state ingestion and cold
  rebuild are the same code path** (so they cannot drift — REF-D4).
- **Edge extraction → emit (the producer seam, exercised here with the M2-available producers)** (architecture
  §4.1): the three structured inline nodes of `myelin-content` (`mention`/`artifact_ref`/`embed`, 13.1) emit
  one `refs.edge.created` per node in the **same transaction** that writes content, via `OutboxTx::emit(draft,
  cause = Some(content_event))` (causality: correlation root carries, causation = the content event, depth +1).
  At M2 the producers are exercised with a synthetic/test content writer + the first real ones land in M3; the
  **seam + the loop-guard depth stamp** are built and drilled now.
- **The per-viewer resolution service — the chokepoint** (architecture §4.2, contract 5.2): `resolve(ref,
  viewer, mode)` → (1) parse + validate; (2) `Id.check(viewer, view, ref)` → **denied returns a tombstone, never
  a leak** (the chokepoint that makes unfurls non-leaking — a confidential issue degrades to a placeholder);
  (3) projection via R2 cache hit, else the owner's `project(ref, viewer)` through the **resilient client**
  (Refs never reads the owner's DB — `no-cross-db`); (4) the caller subscribes to `*.updated`/`*.erased` so the
  rendered ref stays live. **Per-viewer correctness without per-viewer caching:** the per-viewer check (step 2)
  gates a viewer-independent, ref-keyed cache (step 3) — shared without leaking because no content returns until
  the check passes.
- **The permission-filtered backlink read — the crux** (architecture §4.4, contract 5.3): `backlinks(target,
  viewer)` calls `Id.list_objects(viewer, view, type, zookie?)` → handles **both frozen shapes** and **lowers
  the `SetExpr` over its own `source_root` id column** before the row scan — `Ids/NotIds` → `source_root IN/NOT
  IN (…)`; `InRelation`/`TupleSet` → a **JOIN against the per-tenant authz reverse index** (`JOIN authz_visible
  av ON av.object_id = edge.source_root AND av.subject = :viewer AND av.relation = view`); `Union`/`Intersect`/
  `Difference` → `AND`/`OR`/`EXCEPT`; `All` → no predicate; `None` → `WHERE false`. **One query, no N+1, no
  post-filter** — Refs never loops `check` per inbound edge (the leak-prone, slow anti-pattern). The **zookie is
  carried** so a just-revoked grant can't read stale (the JOIN reads the authz reverse index at-or-after the
  zookie's revision watermark, bypassing Id's fail-static cache — the "new enemy" defense). Always paginated
  (hot-artifact safety).
- **The recursive-CTE traversal** (architecture §4.5, contract 5.3): `traverse(root, rels, depth, viewer)`
  walks the `edge` adjacency list with a `WITH RECURSIVE` filtered by `rel`/`rel_class`, a **visited-set cycle
  guard** (path-array / SQL:2023 `CYCLE`), a **depth ceiling (default 16)**, a statement timeout, and **one**
  `list_objects` post-filter over the collected node set (not per-hop) where a hop into an unreadable artifact
  **prunes that branch** (the traversal is not a side-channel). A request exceeding the budget returns a
  **partial result + a "truncated" marker**, never an unbounded scan; a dependency cycle is surfaced as a
  **diagnostic**, not a hang.
- **The unified `#sub` resolution ladder — frozen, one ladder for all three content shapes** (architecture
  §4.6, contract 5.7): `resolve(ref, viewer, mode)` for any `#sub` runs (1) permission → Deny ⇒ `Tombstone{denied}`;
  (2) root resolve → No ⇒ `Tombstone{root_gone}`; (3) sub-resolve via the owner's `project` sub-anchor resolver
  → `LIVE → Projection` / `MOVED → Projection + flag moved` / `OUTDATED → Projection(partial) + flag outdated`
  / `GONE → Tombstone{sub_gone, root}`; (4) ERASED → `Tombstone{erased}`. **A tombstone always carries the
  root** (an embed degrades to "this referenced *<parent>* (the specific part is no longer available)" rather
  than vanishing). Git line-ranges resolve content-anchored (exact→LIVE / rebased→MOVED / partial→OUTDATED /
  content_gone→GONE via BLAKE3 fingerprint + 3-way context match); KN block/heading/row anchors and Chat
  message/thread anchors return the same `live/moved/outdated/gone` shape. The `check-<context>`/`step-<n>`
  kinds (CI's check seam + jump-to-failure, C-6) resolve through the same ladder.
- **The TE-7 typed-edge mirror discipline** (architecture §3.3, contract 5.5): Refs fixes the `rel` vocabulary
  (`closes/blocks/blocked_by/depends_on/parent/assigns/relates`), the `rel_class='lifecycle'` mirror
  discipline, and the **inverse pairing** (`blocks`↔`blocked_by`, `parent`↔`child`); it consumes the typed
  lifecycle events and projects `lifecycle`-class edges so cross-subsystem traversal is one Refs query. At M2
  the discipline + the consumer are built; the *typed tables* are owned by Issues/KN and arrive M3/M4 (so the
  lifecycle producers are exercised with synthetic events at M2, real ones M3/M4).
- **Erasure as a real (small) holder** (architecture §4.6 tail, contract 10.1): `locate(subject)` →
  edges/cache entries naming the subject; `erase(subject)` → purge R2 cache PII + rely on Id's pseudonym shred
  for `origin_actor` (the edge keeps the opaque id; the human becomes unresolvable) + tombstone content-erased
  targets via the `*.erased` consumer. **No erasure backdoor** — driven by `*.erased` through the same live
  consumer path. `restrict` suppression keeps a restricted subject's references out of indexing/agent-use/
  analytics.
- **Reindex-from-source** (architecture §4.7, contract 5.8): `reindex(scope)` calls the Bus re-emit protocol
  (2.6) → each owner's `replay(scope, since)` emits `*.snapshot` (content nodes + typed relations,
  **sub-artifact-granular**) → `refs-edge-builder` ingests idempotently → the rebuilt `edge` index
  **byte-matches** the live index. One code path for steady-state and recovery; **no "load the edge table from
  an owner's DB" backdoor** (REF-4). On a Refs↔typed-table TE-7 drift, a scoped reindex reconverges Refs to the
  typed table (which always wins).
- **The projection cache (R2)** (architecture §3.6): bounded, invalidatable, event-busted per `ArtifactRef`
  (title/state/icon/render hint), keyed `(tenant, ref)`, TTL + `*.updated`/`*.erased` invalidation. A
  `PersonalDataHolder` (may hold a name in a title), **never a source of truth**; on miss/erasure it
  re-resolves. Residency-pinned, crypto-shred-able (Valkey-class).
- **Telemetry** (architecture §5.1, contract 1.8): `backlink_read_latency`, `resolve_cache_hit_ratio`,
  `index_lag`, `hot_artifact_fanout`, `tombstone_count` (+ ladder-state distribution), `reindex_parity`, the
  filter-mode split (`Ids` vs `Filter`/`TupleSet`), per-tenant in-flight + shed counts, causal-depth on
  edge-creation. Every drill asserts against these (observability is part of the pass condition).

**Floor-then-full progressions (each named with its scheduled follow-on):**
- **Floor: read-time recursive-CTE + `list_objects` filter + pagination + (M5) read replica** for the
  hot-artifact backlink case (the "viral PR / referenced-by-50,000"). **Follow-on: the Leopard-style flattened
  reach index (R4)** — derived/rebuildable from R1, incrementally maintained from `refs.edge.*`, gated by the
  same `list_objects` filter; promotion trigger = **measured hot-fanout exceeding the read budget (R5), not
  predicted** (architecture §6.3). **Scheduled: M5** (or whenever the measured trigger fires). Named so
  "we page them, we don't materialise them" is not mistaken for the final hot-path answer.
- **Floor: in-cell graph build + cell-local cross-cell *resolution semantics* (frozen now, C-5)** — a
  cross-cell `target` resolves **in the home cell** (the home cell renders + permission-checks; only the
  already-filtered projection or a tombstone crosses, over the frozen `CrossCellPointer`). **Follow-on: the
  cross-cell backlink *fan-out build*** for multi-cell tenants (ISS cross-cell portfolio rollup, KN cross-cell
  collab, CHAT cross-org channels). **Scheduled: M5** (when multi-cell goes live, 12.6). The §5 contracts are
  cell-agnostic, so the fan-out build extends **without a rewrite**.
- **Floor: the `#sub` grammar + the one tombstone ladder (Refs-owned, frozen now).** **Follow-on: each
  subsystem's stable `#sub` mint** — a block id survives moves, a message/comment id is immutable, a Git
  line-range carries the BLAKE3 fingerprint. **Scheduled: per-producer M3/M4**, asserted by REF-D9. Named so
  the grammar is not mistaken for a working sub-anchor on every subsystem.

**Upstream dependencies (all must be green before R-M2 starts):**
- **M1 fully green** — Identity 4.3 (`SetExpr` push-down, the crux dependency), 4.2 (`check`), 4.10 (zookie +
  revision watermark), 4.8 (pseudonym shred); KMS 11.3/11.4; restore-verify STOR-D1/D2; residency CP-D2/D3.
- **M0 green** — the outbox + consumer template (2.x), the `EventEnvelope` + `ArtifactRef` token anchor, the
  four Refs lints, the failure-injection harness (the unit of proof for every Refs drill), the `myelin-refs`
  value type (R-M0).
- **M2 siblings, frozen, that Refs composes:** the frozen `myelin-content` three inline ref nodes (13.1, X-2 —
  the edge producers); the Bus reindex-from-source re-emit + sub-artifact-granular `*.snapshot` (2.6); the
  `project(ref, viewer)` + `sub_anchor` resolver shape (5.6) each subsystem implements. **Note: Refs consumes
  the durable bus, NOT the firehose live tier** — OQ-J does not touch Refs (the firehose carries CI logs /
  presence / collab op-streams; Refs rides the durable outbox). Noted so no one wires Refs onto the firehose.

**Gate (the Refs rows of the M2→M3 boundary — CI-tier deterministic correctness drills that must emit green
telemetry to call R-M2 done; the master §2 M2 exit gate cites REF-D1/REF-D2/REF-D8 explicitly):**
- **REF-D1 (F1, the cardinal sin) — backlink leak / zero-escape:** a confidential artifact referencing a
  public one is **absent** from backlinks/traverse for an unauthorized viewer — **incl. filter-mode and under
  zookie staleness**. Gate: **0 unauthorized backlinks/traverse**. Green artifact: the zero-escape counter at
  0. — **CI.**
- **REF-D2 (F2) — cross-tenant edge / IDOR:** a cross-tenant edge read via path spoof / a crafted cross-tenant
  URN → **0 cross-tenant edge readable**; the `tenant`-predicate is enforced (lint catches a tenant-less query
  at compile). — **CI.**
- **REF-D6 (F8) — stale re-grant via backlinks ("new enemy"):** revoke access, immediately re-read backlinks
  with the **post-revoke zookie** → **no stale allow** (the zookie bypasses fail-static + honours the
  reverse-index revision watermark). — **CI.**
- **REF-D7 (F5) — edge loss / no-ghost (the dual-write hazard):** crash a producer between the content/relation
  commit and the relay publish → the edge is **still delivered** (outbox), **never an edge without its
  content**. Gate: **0 ghost, 0 lost**. Green artifact: outbox emit-iff-committed. — **CI.**
- **REF-D8 (traversal bound) — cycle / unbounded walk:** a cycle + a 1000-deep chain → the CTE **terminates**
  (visited-set + depth ceiling 16), the cycle is surfaced as a **diagnostic** (not a hang), the statement
  timeout is respected. — **CI.**
- **REF-D9 (sub-tombstone, the unified ladder):** delete a doc block / PR comment / chat message / make a Git
  line-range outdated that others embed → each degrades through the **frozen `live/moved/outdated/gone` ladder**
  to the **correct state** (`moved`/`outdated`/`sub_gone`) with the **root carried** — **0 dangling embed, 0
  hard 404, no leak**. (At M2 exercised against the available producers + synthetic ones; re-run on each real
  producer in M3/M4.) — **CI.**
- **REF-D4 (F4) — reindex-from-cold parity (small-corpus CI variant):** wipe the `edge` index, `reindex(scope)`
  → the rebuilt index **byte-matches** live; a TE-7 drift reconverges to the typed table (typed wins). Green
  artifact: the reindex-parity hash. — **CI variant gates the band; full-scale REF-D4 is SCHED at M5.**
- **REF-D5 (erasure, CI variant) — erasure leaves no dangling/leaking edge:** erase a subject + a referenced
  artifact → references tombstone, the person is unresolvable, **0 recoverable PII** in edge/cache, **no 500 on
  resolve**. — **CI variant gates the band; the full backup-level proof joins the M5 DSAR fan-out (E2E-4).**

(REF-D3 hot-fanout and REF-D10 surge are scheduled/scale drills that land in **M5**, see R-M5.)

**This milestone is part of the master M2 exit gate** (the §2 list cites REF-D1 / REF-D2 / REF-D8 as the Refs
rows of the M2→M3 boundary). **M3 does not start over a red REF-D1.**

---

### R-M3 — Producer edges light up: git links + knowledge embeds + the first lifecycle edges (master band M3)

**Master band:** M3 (the producer subsystems — Git hosting + Knowledge platform).

**The work (Refs consumes the first real producers; the engine is unchanged, the *edges* arrive):**
- **Git-produced reference edges + sub-anchors** (Git subsystem deliverables Refs consumes): commit-trailer /
  PR-link / `Closes <issue>` references emit `refs.edge.created` via the three content nodes; Git implements
  `project(ref, viewer)` for repo/PR/commit unfurls and the `sub_anchor` resolver for **content-anchored line
  ranges** (`#L<a>-L<b>` — BLAKE3 fingerprint + 3-way context match → exact/rebased/partial/tombstone, the
  REF-D9 / GIT-D7 anchor) and PR review-thread `comment-`/`thread-` anchors. The Git ReBAC fragment (4.9) flows
  through `list_objects` so the PR/repo backlink lists are leak-free (the GIT-D11 `SetExpr` JOIN).
- **Knowledge-produced edges + sub-anchors:** KN block/heading/row embeds emit `refs.edge.created`; KN
  implements `project(ref, viewer)` for page/block/row unfurls + the `sub_anchor` resolver for `b<id>`/`h<id>`/
  `row-`/`field-` anchors (stable across edits → LIVE; edited → OUTDATED; deleted → GONE). KN's `page_parent`
  typed-lifecycle events (page-tree `parent` edges, TE-7) land here — the **first real lifecycle mirror** Refs
  projects.
- **Sub-artifact-granular `replay`** (architecture §4.7 ask, contract 2.6): Git `replay` per-blob/ref; KN
  `replay` page-subtree at **block granularity** — so a scoped `reindex` re-emits the right grain and the
  content-anchored line-range / block anchors re-derive (never a stale raw line number / positional index).
- **The Git↔CI `CheckStatus` seam — Refs' grammar half** (contract 5.9, C-6): the `check-<context>` /
  `step-<n>` `#sub` kinds (frozen in R-M0) are now *used* — Git's `check_status` projection + `details_ref`
  (`#step-<n>`, jump-to-failure) resolve through the same Refs ladder as every other sub-anchor. Refs declared
  the grammar; the producer (CI) lands its half in M4 — at M3 Git ships the consumer/projection awaiting CI.

**Floor-then-full:**
- **Floor: in-cell single-home-cell graph build.** **Follow-on: cross-cell backlink fan-out** — unchanged
  (R-M2 floor), still **M5**. (No M3 change; the producers are all in one cell.)
- **Floor: Git pseudonymous-by-default commit author as `origin_actor`** (the immutable bytes never bake in
  erasable PII — the erasure-vs-immutability answer, decided *before* the git data model fixes, EI-04 §1).
  **Follow-on: the audited history-rewrite erasure path** (10.6) — **M5 / on-demand.** This is a Git deliverable
  Refs *depends on* (so `origin_actor` is erasure-safe); named here because it gates Refs' clean erasure surface
  (REF-D5 / GIT-D2).

**Upstream dependencies:**
- **M2 green** (the Refs core — the edge index, resolution, the backlink read, the `#sub` ladder, REF-D1/D2/D7/
  D8 green).
- **M3 producers' deliverables Refs consumes:** Git's three content nodes + `project(ref, viewer)` + the
  content-anchored line-range `sub_anchor` resolver + per-blob/ref `replay` + pseudonymous commit authors; KN's
  three content nodes + block/page/row `project` + the `sub_anchor` resolver + page-subtree `replay` + the
  `page_parent` typed-lifecycle events. Refs blocks on these producers existing — but Refs' *engine* does not
  change, only the edges it ingests. **AG-D4 green** (the sandbox-escape gate) is a band precondition but is not
  a Refs dependency directly — Refs runs no untrusted code.

**Gate (Refs' rows within / supporting the M3→M4 boundary):**
- **REF-D1 / REF-D2 re-confirmed green on the real Git + KN edge corpora** (the leak + IDOR invariants must
  hold on production-shaped edges, not just the M2 synthetic corpus) — **CI.** The gate-invariant ratchet: the
  M2 drills re-run on each new producer corpus.
- **REF-D9 green on Git content-anchored line-ranges + KN block/row anchors** (the `live/moved/outdated/gone`
  ladder on the **real** sub-anchor shapes — a force-pushed PR line-range resolves MOVED/OUTDATED/GONE; an
  edited/deleted KN block resolves OUTDATED/GONE; the root is always carried). **0 dangling embed, 0 hard 404,
  no leak.** This is the Refs half of **GIT-D7** (force-push/rebase a PR with open inline threads → anchors
  resolve LIVE/MOVED/OUTDATED/GONE; 0 mis-anchored) — **CI.**
- **REF-D4 reindex-parity green on a Git + KN corpus** (cold == live incl. content-anchored line-ranges +
  block-granular sub-artifacts + the KN `page_parent` lifecycle mirror reconverging to the typed table),
  small-to-moderate scale — **CI/SCHED.**
- The Refs half of **E2E-1 (PR context pane)** is the spine of that scenario and its behaviour is proven here:
  a PR description's `Closes ENG-1421` + `embed` of a KN doc + `@mention` resolve per-viewer (the issue
  projection, the doc embed, backlinks via depth-16 `traverse`), and a confidential linked issue unfurls to a
  **tombstone carrying the root, title never present** (the 4-step ladder step 1) — **0 leak incl.
  count/backlink leak.** (E2E-1 runs at M5, but the Refs resolution + tombstone behaviour it depends on is
  proven here.) — **CI.**

---

### R-M4 — Consumer-subsystem edges: CI check seam + issue relations + chat unfurls (master band M4)

**Master band:** M4 (the consumer subsystems — CI + Issues + Chat).

**The work (Refs consumes the remaining producers; the engine is unchanged):**
- **The Git↔CI `CheckStatus` seam — CI's producer half closes (X-1)** (contract 5.9): CI emits
  `ci.check.updated` per `(commit_oid, context)` with `run_attempt` monotonic supersession; the
  `details_ref = #step-<n>` jump-to-failure anchor now resolves through the Refs ladder (the grammar Refs froze
  in R-M0, the consumer Git built in M3). Refs' role is the **sub-anchor resolution** of `check-<context>` /
  `step-<n>` — the seam itself (out-of-order supersession, fork-success-neutral, the merge-queue wake) is the
  Git+CI X-1 deliverable (GIT-D10/CI-D8); Refs proves only that the check/step anchors resolve correctly through
  the one ladder.
- **Issues lifecycle edges — the second TE-7 mirror** (contract 5.5): Issues' `issue_relation` typed events
  (`closes`/`blocks`/`blocked_by`/`depends_on`/`relates`/`parent`/`assigns`) land here; Refs projects them as
  `lifecycle`-class edges with the frozen inverse pairing, so the spec-to-ship lineage (`initiative` → child
  issues → PRs → commits → CI → deploy → chat decision) is **one Refs `traverse`**, not a five-way fan-out.
  Issues implements `project(ref, viewer)` for the `<PROJECTKEY>-<seqno>` key + `field-`/`row-` sub-anchors.
- **Chat unfurls — the maximal consumer** (Chat subsystem deliverable Refs serves): Chat's
  `mention`/`artifact_ref`/`embed` nodes produce edges; Chat *consumes* `resolve` for every unfurl (commit /
  issue / doc / CI run) through the 4-step tombstone ladder + the shared per-ref cache busting on `*.updated`;
  `message-`/`thread-` sub-anchors mint stably (immutable → LIVE; deleted → GONE). The Chat ReBAC fragment
  (`channel.read = member + parent_project->read`, 4.9) flows through `list_objects` so a search/backlink as a
  non-member returns 0.
- **Cross-subsystem traversal is now complete:** all five producers emit the structured inline nodes uniformly
  (X-2) + Issues/KN own both typed-relation tables, so mention/ref/lifecycle edges are dependable across
  Git/CI/KN/Issues/Chat — the full reference graph is populated.

**Floor-then-full:**
- **Floor: in-cell single-home-cell graph build** — still **M5** for the cross-cell fan-out (unchanged).
- No new Refs floor in M4 — the engine is fixed at M2; M4 only adds the remaining producer edges + the second
  lifecycle mirror.

**Upstream dependencies:**
- **M3 green** (Git + KN edges traversable; the `#sub` ladder green on real sub-anchors).
- **M4 producers' deliverables Refs consumes:** CI's `ci.check.updated` producer half + the `details_ref`
  step anchor (11.8 sealed log segments); Issues' three content nodes + `issue_relation` typed events +
  `project` + `<PROJECTKEY>-<seqno>` / `field-`/`row-` sub-anchors; Chat's three content nodes + `message-`/
  `thread-` sub-anchors + the channel ReBAC fragment. **AG-D4 re-confirmed green** is a band precondition (not
  a Refs dependency directly).

**Gate (Refs' rows within the M4→M5 boundary):**
- **REF-D1 / REF-D2 green on the full five-producer corpus** — the leak + IDOR invariants hold across Issues +
  CI check/step anchors + Chat unfurls (the most adversarial corpus: confidential issues, private channels,
  fork-scoped CI). **0 leak, 0 cross-tenant edge.** — **CI.**
- **REF-D9 green on CI `check-`/`step-` + Issues `field-`/`row-` + Chat `message-`/`thread-` anchors** — every
  `#sub` kind resolves through the one ladder to the correct state with the root carried (this is the Refs half
  of the X-1 `details_ref` resolution + the Chat unfurl tombstone — supports CHAT-D5 confidential-unfurl →
  tombstone, 0 title leak). — **CI.**
- **The lifecycle-mirror reconvergence check (TE-7):** an out-of-band edit to an `issue_relation` row → a scoped
  `reindex` reconverges Refs to the typed table (typed wins) — proves REF-D4's TE-7 half on the **second** real
  mirror. — **CI.** (Supports ISS-D6 typed-relation correctness.)
- The Refs half of **E2E-1** lights up end-to-end (the PR pane unfurls *every* connected artifact — issue, doc,
  CI checks, chat thread — per-viewer, leak-free, live, with the mid-flight check-update + confidential-issue
  tombstone). Proven in-context here; the full E2E-1 run is M5. — **CI.**

---

### R-M5 — World-scale hardening + the cross-cell floor follow-on + the E2E wedge (master band M5)

**Master band:** M5 (world-scale hardening + floor follow-ons + the four whole-system E2E scenarios).

**The work — the world-scale / hard-problem work, explicitly scheduled here (not deferred silently):**
- **The 30× agent ref-creation + backlink-read surge** (REF-D10, F6 family): the protected-human-lane shed
  order (1.11) tuned to Refs' two surfaces — a human's interactive backlink/traverse read holds the protected
  lane; agent ref-creation + backlink-read sheds with `429 + Retry-After`; per-tenant in-flight caps keep one
  tenant's agent storm off another's humans (the per-tenant bulkhead). The per-surface shed-budget *numbers*
  (OQ-K) are set here from **measurement**, not predicted.
- **The hot-artifact backlink scale — the "viral PR / referenced-by-50,000" case** (REF-D3, the named M2 floor's
  follow-on, architecture §6.3): the read-time CTE + `list_objects` filter + pagination + **read replica**
  (ID-4-class, the doctrine's named first scaling move) is the built floor; the **Leopard-style flattened reach
  index (R4)** — derived/rebuildable from R1, incrementally maintained from `refs.edge.*`, gated by the same
  `list_objects` filter — is promoted **only when measured hot-fanout exceeds the read budget (R5)**, not
  predicted. Gate: paginated p99 within budget under concurrent permission-filtered reads; R4 serves
  post-promotion; the hot-fanout telemetry fires. The *property* (paginated, leak-free) is fixed in M2; the
  *index* is measured here.
- **The cross-cell backlink fan-out build — the named M2 floor's follow-on** (the deepest remaining Refs
  unknown, architecture §6.5, contract 12.6 / OQ-I): when multi-cell goes live (master §2 M5), the cross-cell
  *resolution semantics* (already frozen cell-local in M2 — the home cell renders + permission-checks; only the
  projection or a tombstone crosses, over the frozen `CrossCellPointer`) get their *fan-out build* (ISS
  cross-cell portfolio rollup, KN cross-cell collab, CHAT cross-org channels). The §5 contracts are
  cell-agnostic so the build **extends without a rewrite**; the FLOOR drills GA-D8 / CP-D7 / CP-D8 (cross-cell
  erasure receipt set / cell→cell migration 0 loss / cross-cell ref PII-free bridge) are now owed and run. Until
  multi-cell goes live the single-cell path is complete and the design is the named floor.
- **Sharding `edge` if measured** (architecture §6.2): the shard key is already `(tenant, region)` +
  `target_root` hash, so a measured hot tenant outgrowing one shard is a **re-home, not a redesign** — measured
  here, not before.
- **Restore + cross-seam + re-erase at scale** (REF-D5 at backup scale, F3): restore the `edge` index with
  OLTP/blob/offsets to a consistent point → **no resurrected edges past an erasure** (post-restore re-erasure
  runs from the erasure ledger, 10.8); references stay tombstoned, the person stays unresolvable. Folds into
  the M5 DSAR fan-out (E2E-4).

**Floor-then-full (the M5 follow-ons whose floors were named earlier):**
- **Floor (M2): read-time CTE + pagination + replica for hot backlinks.** **Follow-on: the Leopard-style
  flattened reach index (R4)** — **promoted here, measured-trigger** (REF-D3).
- **Floor (M2): cell-local cross-cell resolution semantics (frozen).** **Follow-on: the cross-cell backlink
  fan-out build** — **here, when multi-cell goes live** (GA-D8/CP-D7/CP-D8 owed).
- **Floor (M3, Git deliverable Refs depends on): pseudonymous-by-default commit authors.** **Follow-on: the
  audited history-rewrite erasure path** (10.6) — **here / on-demand** (when a body must be expunged).
- **Floor (M2): each subsystem's `#sub` mint** — all five real mints proven by M4; M5 re-runs REF-D9 at scale.

**Upstream dependencies:**
- **M4 green** (all five producer corpora traversable; the deterministic correctness drills green).
- **M5 platform pieces Refs consumes:** the multi-cell bridge live (12.6) for cross-cell fan-out; restore-verify
  at cell scale (STOR-D2); the full DSR fan-out (10.4) for the backup-level erasure proof; the read replica
  (M5 storage) for the hot-fanout floor.

**Gate (the Refs rows of the M5→M6 boundary — the scale/surge drills + the whole-system wedge):**
- **REF-D10 (30× surge)** — human backlink-read lane holds (interactive latency within budget), agent
  ref-creation + read lane sheds (`429+Retry-After` honoured), other tenants unaffected. Green artifact:
  shed-counts + read p99. — **SCHED.** (Part of the master M5 F6 surge family.)
- **REF-D3 (hot-fanout)** — "referenced-by-50,000" under concurrent permission-filtered reads → paginated p99
  within budget; the hot-fanout telemetry fires; R4 serves post-promotion. — **SCHED.**
- **REF-D4 (reindex-parity at full scale)** — wipe the `edge` index, `reindex` → byte-matches live across the
  full five-producer corpus incl. both TE-7 lifecycle mirrors. Green artifact: the reindex-parity hash. —
  **SCHED.**
- **REF-D5 (erasure at backup scale)** — erase a subject + a referenced artifact → references tombstone, the
  person unresolvable, **0 recoverable PII in edge/cache/backups**, no 500 on resolve. — **SCHED** (folded into
  E2E-4).
- **GA-D8 / CP-D7 / CP-D8 (the cross-cell FLOOR drills, now owed)** — cross-cell erasure receipt set;
  cell→cell migration 0 loss; the cross-cell ref **PII-free** bridge (only the projection/tombstone crosses,
  never raw rows/PII). — **SCHED.**
- **The whole-system E2E scenarios Refs crosses are green:** **E2E-1** (the PR context pane — Refs is the spine:
  every connected artifact resolves per-viewer, the confidential issue → tombstone carrying the root, 0 title/
  count/backlink leak, the live check-update lands within the freshness budget); **E2E-3** (spec-to-ship —
  `traverse(spec_doc, viewer)` walks the **entire lineage** depth-16 cycle-safe per-viewer, and the wiped Refs
  edge index `reindex`es to **byte-match live**, F4 / REF-D4 at scale); **E2E-4** (DSAR fan-out — Refs' edges +
  cache return **0 recoverable PII**, unfurls degrade to tombstones, the holder-coverage receipt includes Refs).
  Each emits its named green artifact. — **SCHED.**

---

### R-M6 — Dogfooding: the reference graph over Myelin's own work (master band M6)

**Master band:** M6 (Myelin hosts itself).

**The work:**
- The reference graph runs over Myelin's own work: the PR context pane on the Myelin monorepo's PRs (commits ↔
  issues ↔ CI checks ↔ KN docs ↔ chat threads), the spec-to-ship lineage on the roadmap/gap-report/scorecard
  living as Myelin issues + a Myelin Knowledge space (the every-incident-adds-a-drill loop files a Myelin issue
  + a reproducing drill, both reference-linked). The builders drive real cross-artifact navigation in a browser
  — "jump from a failing test to the line of code to the issue to the conversation in four keystrokes" (the
  moat thesis, architecture §1) — reached by *driving the real UI*, not reading the feature list.
- The reference-graph contribution to the per-subsystem **switch tests** (folded into the L5 done-bars): does a
  GitHub/Jira/Linear/Notion user's cross-artifact navigation work — unfurls live, backlinks complete,
  tombstones graceful — without hitting a wall the old tool didn't have? Measured against latency budgets
  (backlink read / unfurl within the keyboard/no-spinner-flash budgets, design-language §8b).

**Floor-then-full:** none new — M6 promotes nothing; it exercises the production-hardened reference graph on
real (self-)tenant data.

**Upstream dependencies:** **M5 green** — you do not put real team data (the builders' own work) onto a Refs
tier whose restore + re-erase + DSAR tombstone fan-out + cross-cell resolution are not green (Tier-1/Tier-6 of
the thesis: the team's data is real tenant data).

**Gate (Refs' piece of the M6 done-bar):**
- Refs is green on the **self-hosting CI graph** (the Refs drills run as Myelin CI jobs on Myelin's own commits
  — the dogfood loop).
- The reference-graph switch-test surfaces pass when driven in a browser (measured latency; the four-keystroke
  cross-artifact jump works).
- **No earlier-band Refs gate is red** (the truth-up pass: every Refs PROVEN row rests on a dated green artifact
  — REF-D1..D10 + the E2E rows — never a doc claim; code-wins-over-docs, EI-01 §1).

---

## 3. The honest progression — first runnable / first useful / production-hardened

- **First runnable (end of R-M2 / master M2):** a single-tenant, single-cell reference graph that ingests
  edges off the bus (the outbox-fed `refs-edge-builder`), resolves a ref to a per-viewer projection or a
  tombstone, answers `backlinks`/`traverse` **permission-filtered by `list_objects`** with **zero leak proven**
  (REF-D1/D2/D6 green), survives a producer crash with **no ghost edge** (REF-D7), bounds every traversal
  (REF-D8), degrades every `#sub` through the one ladder (REF-D9), and rebuilds byte-for-byte from the log
  (REF-D4 CI variant). It is *correct* before it is *broad* or *fast* — the leak invariant and the no-ghost
  floor are non-negotiable and land first. The producers are synthetic/the first real ones; the cross-cell path
  is resolution-only (semantics frozen, fan-out not built).
- **First useful (end of R-M3 → R-M4 / master M3–M4):** real edges — git commit/PR links + content-anchored
  line-ranges (Git), KN block/row embeds + `page_parent` lifecycle, the CI check/step anchors, issue
  `closes`/`blocks` relations, chat unfurls of everything — all traversable per-viewer, leak-free, live. The PR
  context pane lights up: every connected artifact unfurls per-viewer, the confidential issue degrades to a
  tombstone, a force-pushed line-range resolves MOVED/OUTDATED/GONE. A developer can actually *jump across
  subsystems* from one artifact to its whole causal neighbourhood. Hot backlinks are paged (not yet R4-indexed);
  cross-cell is single-cell.
- **Production-hardened (end of R-M5 / master M5):** the 30× surge holds with the protected human lane
  (REF-D10), the viral-PR hot-fanout is paged within budget with R4 promoted by measurement (REF-D3),
  reindex-from-cold is byte-parity at full scale across both TE-7 mirrors (REF-D4), restore + re-erase is green
  at backup scale (REF-D5), the cross-cell backlink fan-out is built-and-extends over the PII-free bridge
  (GA-D8/CP-D7/CP-D8), and Refs passes E2E-1 (as its spine) / E2E-3 / E2E-4. Only here is the reference graph
  "done" enough to carry the builders' own data (M6).

---

## 4. Digest

**Milestones (Refs' slice of each master band):**
- **R-M0 (band M0):** ship the `myelin-refs` glue crate — the `ArtifactRef` value type + `parse`/`format`
  ambiguity-rejection + the frozen `#sub` grammar vocabulary + the Issues `<PROJECTKEY>-<seqno>` key; validate
  (not author) the token table; lean on the four lints (`tenant-predicate`, `no-raw-publish`, `no-cross-db`,
  `no-cross-sync-cycle`) with red+green fixtures. The names anchor + the ratchet, before the engine they guard.
- **R-M1 (band M1):** register Refs as an exhaustive-list `PersonalDataHolder` (small structural surface: opaque
  ids + cache titles, never third-party free-text — no new `[OPEN — LEGAL]` residual); pin the per-tenant DEK
  into the KMS hierarchy; confirm residency-pin. No engine yet — the holder + encryption floor.
- **R-M2 (band M2, the primary build):** the full engine — the `edge` inverse index + the two consumers
  (`refs-edge-builder`, `refs-projection-invalidator`, steady-state == cold-rebuild path); the edge-extraction
  emit seam (the three content nodes via the outbox); **the per-viewer resolution chokepoint (denied →
  tombstone, never leak)**; **the permission-filtered backlink read (the crux: lower the `SetExpr`/`Ids` ACL
  filter over `source_root` before the row scan, zookie-carried, no N+1)**; the bounded cycle-safe recursive-CTE
  traverse (depth 16); **the unified 4-step `#sub` tombstone ladder (live/moved/outdated/gone, root always
  carried)**; the TE-7 mirror discipline; the small structural erasure holder; reindex-from-source; the R2
  projection cache; telemetry.
- **R-M3 (band M3):** Git edges (commit/PR links + content-anchored line-ranges + pseudonymous authors) + KN
  edges (block/row embeds + `page_parent` lifecycle — the first real TE-7 mirror); sub-artifact-granular
  `replay`; the `CheckStatus` grammar half awaiting CI.
- **R-M4 (band M4):** the CI `CheckStatus` producer half closes the X-1 seam (check/step anchors resolve);
  Issues `issue_relation` lifecycle edges (the second TE-7 mirror); Chat unfurls of everything; cross-subsystem
  traversal complete across all five producers.
- **R-M5 (band M5, world-scale):** the 30× surge + protected lane (REF-D10); the hot-artifact reach index R4
  (REF-D3, measured-trigger); **the cross-cell backlink fan-out build (the named floor follow-on, when
  multi-cell goes live — GA-D8/CP-D7/CP-D8 owed)**; reindex-parity + restore + re-erase at scale (REF-D4/D5);
  E2E-1 (as its spine) / E2E-3 / E2E-4.
- **R-M6 (band M6):** the reference graph over Myelin's own work; the four-keystroke cross-artifact jump in a
  browser; green on the self-hosting CI graph.

**Floors + follow-ons (name-your-floors):**
- per-tenant DEK (R-M1) → **the structural erasure surface** (R2-purge + Id pseudonym-shred + `*.erased`
  tombstoning) (R-M2, the primary mechanism).
- read-time CTE + pagination + replica for hot backlinks (R-M2) → **the Leopard-style flattened reach index
  (R4)** (R-M5, promoted at measured hot-fanout > read budget, REF-D3).
- cell-local cross-cell **resolution semantics** (frozen, R-M2) → **the cross-cell backlink fan-out build**
  (R-M5, when multi-cell goes live, over the PII-free `CrossCellPointer` bridge — the deepest remaining Refs
  unknown).
- the `#sub` grammar + the one tombstone ladder (Refs-owned, R-M2) → **each subsystem's stable `#sub` mint**
  (per-producer R-M3/R-M4, asserted by REF-D9; a block id survives moves, a message/comment id immutable, a Git
  range carries the BLAKE3 fingerprint).
- Git pseudonymous-by-default commit authors (R-M3, Git deliverable Refs depends on) → **the audited
  history-rewrite erasure path** (10.6, R-M5 / on-demand).
- single-home-cell graph (R-M2..R-M4) → **cross-cell fan-out** (R-M5, designed-and-extends — same floor as the
  cross-cell row above, stated as the build).

**The critical upstream dependencies (what must exist first):**
1. **Identity 4.3 — `list_objects` `SetExpr` push-down (M1)** — the single most load-bearing dependency; Refs'
   entire backlink/traverse correctness is downstream of it (you see a backlink iff you can see the artifact
   that made the reference). No core build (R-M2) without it frozen + green.
2. **The Event-Bus outbox + consumer template 2.2/2.3/2.4 (M0)** — edges are born **iff** their content/relation
   commits (the no-ghost/no-loss floor, F5 / REF-D7); the only sanctioned emit path (no standalone edge-write
   API). No edge ingestion without the outbox green.
3. **The `myelin-content` three inline ref nodes 13.1 (M2 frozen, X-2)** — the **producers** of every reference
   edge; structured-node extraction (not regex over prose) is the reliability guarantee. Byte-identical across
   Git/Issues/KN/Chat so the producer is uniform.
4. **`project(ref, viewer)` 5.6 + the per-owner `sub_anchor` resolver (M2 shape; per-producer M3/M4)** — Refs
   reads the per-viewer projection + resolves the `#sub` ladder, never an owner DB (`no-cross-db`).
5. **The TE-7 typed relation tables + their lifecycle events 5.5 (KN M3, Issues M4)** — the lifecycle-edge truth
   Refs projects (typed table wins on drift). Refs fixes the vocabulary + inverse pairing; the owners own the
   rows.
6. **The cross-cell PII-free pointer bridge 12.6 (M1 frame; M5 live)** — the cross-cell fan-out build's
   substrate; the resolution semantics are frozen cell-local in M2, the build rides the bridge in M5.
7. **KMS hierarchy 11.3/11.4 + restore-verify STOR-D1/D2 + Id pseudonym shred 4.8 (M1)** — index encryption,
   crypto-shred, erasure-safe `origin_actor`, and the silent-data-loss floor Refs builds over.

**The two cardinal invariants drilled earliest, never deferred:** F1 backlink-leak / zero-escape (REF-D1, the
cardinal sin — a backlink leak is simultaneously a security and a competitive/PII side-channel) and the
no-ghost/no-loss edge floor (REF-D7, F5 — an edge must never exist without its content, nor be lost across a
producer crash) — both proven the moment the composition exists in R-M2 and re-run as a ratchet on every new
producer corpus (R-M3, R-M4) and at scale (R-M5).
