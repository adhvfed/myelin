# Doctrine Integration — `external-insights/04-hard-problems.md`

> Phase: `02b-doctrine-integration` (inserted between Phase 2 holistic-architecture and Phase 3
> shared-systems-architecture). Canonical brief: [`VISION.md`](../../../VISION.md). Doctrine
> source: [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md)
> ("the genuinely unsolved or expensive problems, named honestly"). Planning this maps against:
> Phase-1 [`open-questions-and-risks.md`](../../01-research/open-questions-and-risks.md);
> Phase-2 [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
> (ADR-04/05/06/08/09/10/11/12/13/14); subsystem docs
> [`knowledge-platform.md`](../../02-holistic-architecture/subsystems/knowledge-platform.md) and
> [`git-hosting.md`](../../02-holistic-architecture/subsystems/git-hosting.md).
>
> **Purpose.** Classify every insight in the doctrine doc against committed Myelin planning, and
> route each non-CONFIRMS item to the phase where it binds. The doctrine's own framing — "name
> your floors; the gap between ambitious design and shipped floor is normal, the gap being
> *invisible* is the failure" (`README.md` honesty rule; doc §4) — is itself one of the deltas.
>
> **Headline.** This doc overwhelmingly **CONFIRMS** our spine — the erasure-vs-immutability
> tension, the CRDT ladder, the storage tiers, reindex-from-source, and the deferral discipline
> are all already in ADR-04/05/10/12/13. The **real deltas** are four sharpenings that the
> existing planning states too softly: (1) the **git-history half of erasure is genuinely
> unsolved** and is *not* covered by the event-log/`PersonalDataHolder` answer — our planning
> currently lets ADR-12 §4 imply it is; (2) the collaborative-editing **resume-cursor durable
> transport must be built *first*, before the CRDT**, and is itself a floor that silently loses
> the reconnect gap if mistaken for done; (3) the knowledge data-model picks
> (**markdown-subset inline string**; **read-time-only rollups**) should be *named defaults*,
> not left fully open; (4) **reindex-from-source** must be elevated to a *first-class resilience
> primitive* with a precise definition (the index never reads owner DBs; owners re-emit through
> the live consumer). Plus the deferral discipline (§4) which binds in Phase 8/Phase 5 as
> execution + scorecard law.

---

## 1. How to read this

- **Classification** per the phase brief: **CONFIRMS** (already committed — cite the ADR/doc),
  **SHARPENS** (we have it vaguer; the insight tightens it), **RESOLVES-OPEN** (answers a
  TE-/SC-/AG-/GD-/PR- open question with a concrete *default-to-beat*), **CONFLICTS** (rare),
  **NEW** (net-new).
- **Binds at** uses the brief's vocabulary: VISION (canonical) · Phase-2 back-patch
  (new ADR / design-language augmentation) · Phase 3 (shared system) · Phase 4 (subsystem) ·
  Phase 5 (testing) · Phase 6 (roadmap sequencing) · Phase 8 (execution discipline) ·
  Legal/DPO · Commercial.
- A **default-to-beat** is the concrete answer we hand downstream that the resolving agent must
  either adopt or write down why it deviated (VISION §3; doctrine README "deviate only with a
  reason written down").

---

## 2. Integration table — every insight in the doc

### §1 — GDPR erasure vs. immutability

| # | Insight (doc §1) | Class | Open-Q | Integration ACTION + default-to-beat | Binds at |
|---|---|---|---|---|---|
| 1.1 | **Event-log half: separate identity from action; tombstone/pseudonymise; soft-delete over hard-delete; tightest-policy-wins retention + legal-hold.** | CONFIRMS | GD-1, GD-3 | ADR-12 §1/§4 (references-not-payloads + pseudonymous identities + `PersonalDataHolder`) and ADR-04 §4 (references-not-payloads, bounded retention, crypto-shred, tombstones) already commit exactly this. **No change** — validation. The one *additive*: "tightest-policy-wins retention + legal-hold awareness" is a phrasing P3's retention engine should adopt. | — (CONFIRMS); legal-hold phrasing → Phase 3 (GDPR/Audit) |
| 1.2 | **Git-history half is genuinely unsolved and usually overlooked — author name/email are baked into the commit hash; you cannot tombstone without rewriting history and changing every downstream hash. The event-log answer does NOT cover this; pretending it does is the trap.** | **SHARPENS** (the headline delta) | GD-1, GD-2 | **This is the most important correction in the whole doc.** Our planning *names* the problem (git-hosting §7.5; ADR-12 §4) but **softens it**: ADR-12 §4 ("Keep personal data out of immutable structures… minimises the tension so it 'rarely bites'") reads as if the spine *solves* erasure; git-hosting §7.5 leans on "references-not-payloads + pseudonymous-identity + crypto-shred… *without* rewriting immutable history" as the answer. The doctrine says that combination only solves *the part you pseudonymised in the first place* — it does **nothing** for the immutable bytes if author identity was *not* pseudonymised at commit time. **ACTION:** (a) **Phase-2 back-patch**: add a sentence to ADR-12 §4 and a `[OPEN — LEGAL]` carve-out making explicit that pseudonymous-by-default is a *commit-time prerequisite*, not a retroactive fix, and that for non-pseudonymised history the only levers are history-rewrite (changed hashes, disruptive) or a documented lawful-basis limit — **none free**. (b) **Sharpen the open question** into a named first-class design item (below). **Default-to-beat for git subsystem (P4) + Legal:** *pseudonymous commit identities by default* (commit to a stable opaque author id; person mapping lives out-of-band in the erasable store) — so the immutable bytes never contain erasable PII. Beat it only by writing down why. The history-rewrite path and "different lawful basis with documented limits" are the named alternatives, each with its cost stated. **The decision must be made before the git data model is fixed (P4) — it is nearly impossible to bolt on later.** | Phase-2 back-patch (ADR-12) + Legal/DPO + Phase 4 (Git) data-model gate |
| 1.3 | **Per-tenant / customer-managed keys as the substrate for crypto-shredding (a v2 capability worth designing seams for now).** | CONFIRMS | GD-4 | ADR-12 §3 (per-tenant envelope encryption, optionally per-subject; crypto-shred first-class) + ADR-10/14 ("every tier per-tenant envelope encryption + crypto-shred"). The "v2 / design seams now" framing matches ADR-12 §3's "optionally per-subject where feasible." | — (CONFIRMS); seam-now → Phase 3 (KMS hierarchy, GD-4) |
| 1.4 | **Residency as region-pinning; one tenant's region is immutable; no cross-region query path.** | CONFIRMS | SC-2/SC-3 | ADR-11 §1/§2 (cell = unit of sovereignty; region binding immutable-by-default, enforced at the data layer) + §"cross-region operations in hot paths handling personal data prohibited by construction." Exact match. | — (CONFIRMS) |
| 1.5 | **Tamper-evident eDiscovery exports.** | SHARPENS | GD-5 | ADR-12 §9 commits "one tamper-evident audit log"; eDiscovery *export* (vs the audit log itself) is implied but not named. **ACTION:** P3 GDPR/Audit must add a **tamper-evident export** capability (eDiscovery/legal-hold path) alongside the DSR export receipts (ADR-12 §2). Small addition. | Phase 3 (GDPR/Audit) |
| 1.6 | **EU AI Act angle for any agent that processes personal data.** | CONFIRMS | GD-9, AG-8 | ADR-08 §6 (agents always labelled; suggest-by-default; HITL; audit) + ADR-12 §9 + ADR-15 `[OPEN — LEGAL]` GD-9/AG-8. Design-safe minimums already mandated now. | — (CONFIRMS); GD-9/AG-8 stay Legal |
| 1.7 | **Treat the erasure-vs-immutability reconciliation as a first-class design item with its own write-up, not a checkbox.** | **SHARPENS / NEW (as a deliverable)** | GD-1/2/6 | We have the *constraints* (ADR-12) and the *open question* (GD-1/2, git-hosting §9) but **no dedicated reconciliation document** is mandated. **ACTION:** make "Erasure vs. Immutability reconciliation" a **named Phase-3 deliverable** (a standalone write-up co-owned by GDPR/Audit + the Git P4 agent + Legal), covering: pseudonymous-by-default commit identities, the history-rewrite path and its blast radius (forks/mirrors/CDN/changed hashes), crypto-shred reach into reflogs/bitmaps/backups, and the documented residual limit. This is the doctrine's explicit instruction ("its own write-up"). | Phase 3 (named deliverable) + Phase 4 (Git) + Legal/DPO |

### §2 — Real-time collaborative editing (Knowledge)

| # | Insight (doc §2) | Class | Open-Q | Integration ACTION + default-to-beat | Binds at |
|---|---|---|---|---|---|
| 2.1 | **A legitimate v1 floor: per-block optimistic compare-and-swap (guard each write on the block's last-modified token; precondition miss → reject loser, return server state). Guarantees no *silent* overwrite but does NOT merge. Ship it *named as a floor*, with advisory soft-locks + version snapshot/restore.** | **RESOLVES-OPEN** (gives TE-15 a concrete floor) | TE-15, TE-16 | Our planning leans **straight to CRDT (Yrs)** as the leading candidate (knowledge §3, ADR-14) and treats it as the thing to prototype — it does **not** name a shippable *floor below* the CRDT. The doctrine supplies one. **ACTION:** establish the **CAS floor as the Phase-4 Knowledge default-to-beat for v1 collaboration**: per-block optimistic compare-and-swap on a last-modified token + advisory soft-locks + version snapshot/restore, **explicitly named as a floor that does not merge**. The CRDT is the named next step (2.2), not v1's bar. This *de-risks the schedule*: Knowledge can ship correct-but-non-merging concurrency without the CRDT being on the critical path. **Default-to-beat (P4 Knowledge):** CAS-floor v1 → CRDT v2, both over `myelin-content` (ADR-05). | Phase 4 (Knowledge) + Phase 6 (sequencing: floor before CRDT) |
| 2.2 | **The real answer is a CRDT (Automerge/Yjs-class) — a NAMED, SCHEDULED subsystem, not a "someday"; the first true concurrent-edit conflict is its trigger.** | CONFIRMS (+ SHARPENS the trigger) | TE-15 | CONFIRMS knowledge §3/§9 + ADR-14 (CRDT/Yrs leading candidate, prototype in P4). **SHARPENS** by giving the *trigger condition* ("first true concurrent-edit conflict") — adopt that as the explicit promotion criterion in the Phase-6 roadmap so "CRDT later" has a concrete trip-wire, not a vague "v2." | Phase 6 (roadmap: named promotion trigger) |
| 2.3 | **Build the durable, resume-cursor real-time transport FIRST (dropped connection loses nothing; ops apply idempotently) — the CRDT slots into that transport. A relay *without* resume cursors is itself a floor that silently loses the gap on reconnect — don't mistake it for done.** | **SHARPENS (high-value sequencing delta)** | TE-15, TE-11/SC-9 (firehose), AG-…idempotency | This is a **sequencing correction our planning does not state.** Knowledge §2/§7.3/§8.3 commit a "firehose transport" for the collab op-stream and "ops apply idempotently" (ADR-04 idempotency) — but **nowhere do we say the resume-cursor durable transport is built FIRST, as the foundation the CRDT (and even the CAS floor's op delivery) sit on**, nor that a resume-cursor-less relay is a *named floor* that silently loses the reconnect gap. **ACTION:** (a) **Phase-6 roadmap**: order the Knowledge collab work as **resume-cursor durable transport → CAS floor → CRDT** — the transport is item 0, not a sub-detail of the CRDT. (b) **Phase-3 firehose design** must include **per-connection resume cursors with idempotent op application** as a *required property of the collab firehose*, not an optional nicety (sharpens ADR-04 §5 + knowledge §8.3). (c) **Phase-5 testing**: a *drill* (reconnect-after-drop loses zero ops) is the green artifact that promotes the transport from "claimed" to "proven" (doctrine §4). **Default-to-beat (P3 + P6):** the collab firehose ships with resume cursors + idempotent apply *before* any merge engine; a cursor-less relay is explicitly a floor. | Phase 6 (sequencing) + Phase 3 (firehose transport) + Phase 5 (reconnect drill) |
| 2.4 | **Store inline content as a markdown-subset STRING (not an inline-range JSON model) — survives copy/paste, export, diff, reference-extraction; keeps saved content human-readable.** | **RESOLVES-OPEN (concrete default for the content model)** | TE-16, TE-4/ADR-05 | ADR-05 commits a shared block/inline taxonomy but leaves the **inline representation open** ("exact block taxonomy completeness… → P4"); knowledge §1/§3 say "one shared content model" without fixing inline storage. The doctrine gives a concrete, durable default. **ACTION:** record **markdown-subset string for inline content** as the Phase-2 `myelin-content` (ADR-05) **directional default**, to be ratified when Knowledge leads the taxonomy in P4 — with the explicit caveat that the `mention(Principal)`/`artifact_ref(ArtifactRef)`/`embed(ArtifactRef)` nodes (ADR-05's load-bearing inline nodes) remain *structured* nodes, not collapsed into the string (so reference-extraction stays reliable). **Default-to-beat (ADR-05 → P4 Knowledge):** inline = markdown-subset string + structured ref/mention/embed nodes; beat it only with a written reason. | Phase-2 back-patch (ADR-05 directional note) + Phase 4 (Knowledge taxonomy) |
| 2.5 | **Model in-document databases as a property bag per row, with rollups and formulas computed at READ TIME, never stored.** | **RESOLVES-OPEN (default for TE-17/TE-18)** | TE-17, TE-18 | Knowledge §3 already leans **JSONB property-bag** (matches "property bag per row") — that half CONFIRMS TE-17. The **read-time-never-stored** rule for rollups/formulas is the part our planning leaves *open*: knowledge §3/§9 lean "async incremental dataflow off the bus" with "eventual consistency" — i.e. it contemplates *materialising* rollups. The doctrine's default is the opposite: **compute at read time, never store**. This is a genuine tension worth surfacing (not a CONFLICT — both are admissible; the doctrine names the simpler, drift-free floor). **ACTION:** hand P4 Knowledge a **two-rung default-to-beat for TE-18**: *rung 1 = read-time-computed rollups/formulas (never stored), the doctrine's floor; rung 2 = async incremental materialised dataflow, only when read-time recompute is measured too slow* (mirrors "don't add the column-store before volume is measured", §5). This *inverts our current lean* (which jumps to materialised dataflow) into floor-first. Also adopt the doctrine's caution: **relation columns need careful, initially best-effort bidirectional consistency** (matches knowledge §3 "two-way relations" + TE-7). | Phase 4 (Knowledge, TE-17/TE-18) + handed via ADR-06 note |

### §3 — World-scale git storage

| # | Insight (doc §3) | Class | Open-Q | Integration ACTION + default-to-beat | Binds at |
|---|---|---|---|---|---|
| 3.1 | **Git at world scale is the single heaviest subsystem to scale; sequenced last for good reason.** | CONFIRMS | SC-8, TE-24 | git-hosting §1/§9 + ADR-14 (heaviest; TE-24 flagged). Matches "sequenced last." **ACTION (light):** make the "sequence git storage last" guidance explicit in the **Phase-6 roadmap ordering** so it is a deliberate decision, not an accident. | Phase 6 (sequencing) |
| 3.2 | **Authoritative bytes (objects, packs) want to live in an object store, not a node's local disk — a deep project (delta/pack mgmt, sharding, replication, smart-transport). Plan the local-disk → object-backed transition as explicit, sequenced work; don't pin repos to a single node forever.** | **SHARPENS (sequencing as first-class work)** | TE-24 | git-hosting §3 + ADR-10/14 already flag object-store-backed packs (Mononoke/JGit-DFS) as an *option* for TE-24. The doctrine SHARPENS by insisting the **local-disk-to-object-backed migration is sequenced work planned up front**, and that early choices must **not pin a repo to one node forever** (a forward-compat constraint on the v1 data model). **ACTION:** (a) **Phase-4 Git**: add a constraint that the v1 storage/replication design (TE-24) must keep an **object-backing migration seam** — repo placement must be relocatable, not node-pinned. (b) **Phase-6**: name "local-disk → object-store-backed packs" as an explicit sequenced milestone, not an emergent rewrite. **Default-to-beat (P4 Git):** start node-backed if needed, but the data model must not foreclose object-backing; the transition is planned, not discovered. | Phase 4 (Git, TE-24) + Phase 6 (sequencing) |
| 3.3 | **Build on proven structures — content-addressed objects, Merkle history, pack/delta storage — rather than inventing.** | CONFIRMS | TE-8, VISION §5.4 | git-hosting §3 + VISION §5.4 ("Merkle structures and pack/delta storage for git… name it and explain the choice"). Exact match. | — (CONFIRMS) |

### §4 — The deferral discipline (shipping floors without lying)

| # | Insight (doc §4) | Class | Open-Q | Integration ACTION + default-to-beat | Binds at |
|---|---|---|---|---|---|
| 4.1 | **Every hard problem ships as a floor before the full answer — correct and necessary.** | CONFIRMS | VISION §3 ("quality over plan-adherence"; "honesty about uncertainty") | VISION §3 + README honesty rule. The *principle* is canonical; what's missing is the *mechanism* (4.2–4.4). | — (CONFIRMS, principle) |
| 4.2 | **Name the floor AND name the follow-on.** ("CAS floor; full CRDT is the named next step.") | **NEW (as binding execution discipline)** | — | VISION §3 says "write down the deviation"; the doctrine makes it **structured**: every floor must name its follow-on. **ACTION:** adopt as **Phase-8 execution discipline** and a **Phase-6 roadmap convention** — every roadmap item that is a floor carries an explicit, linked follow-on. | Phase 8 (execution) + Phase 6 (roadmap convention) |
| 4.3 | **A skeleton/spike is not done — name the half that's missing. The scorecard is SOURCE-VERIFIED, not doc-verified: a capability is "proven" only when a drill produced a green artifact; until then it is "claimed."** | **NEW (high-value; binds the testing strategy)** | — | This is the strongest net-new contribution to *process*: a **claimed/proven distinction backed by a green-artifact drill**. Our planning has `cargo-mutants`/golden tests (ADR-08) but **no claimed-vs-proven scorecard discipline**. **ACTION:** (a) **Phase-5 testing strategy** must define the **source-verified scorecard**: each load-bearing capability is "proven" only when a drill emits a green artifact (e.g. the reconnect-loses-zero-ops drill (2.3); the sandbox-escape kernel drill (5.1); the erasure-reaches-search drill). (b) **Phase-8**: agents report capabilities as *claimed* until the drill is green. | Phase 5 (scorecard definition) + Phase 8 (reporting discipline) |
| 4.4 | **Track floors somewhere durable (a gap report) so the next worker sees the real state. The gap between ambitious design and shipped floor is normal; the gap being *invisible* is the failure.** | **NEW (binds execution)** | — | We have ADR-15's open-questions carry-forward, but **no living "gap report" of shipped floors**. **ACTION:** **Phase-8** maintains a durable **gap report** (shipped floors + their follow-ons + claimed/proven status), seeded from the floors named across this analysis (CAS floor, single-region, pseudonymous-commit limit, read-time-rollup floor, node-backed git). It is the inter-agent handoff artifact the doctrine demands. | Phase 8 (gap report) |

### §5 — A few more that bite

| # | Insight (doc §5) | Class | Open-Q | Integration ACTION + default-to-beat | Binds at |
|---|---|---|---|---|---|
| 5.1 | **Untrusted code execution is a permanent, never-"done" security surface — one escape is catastrophic; a property not drilled on a real kernel is a claim.** | CONFIRMS (+ SHARPENS into a drill) | TE-28, AG-…sandbox | ADR-15/TE-28 (microVM isolation, dedicated security track) + agent-fabric doctrine. CONFIRMS the surface; the doctrine SHARPENS by demanding a **real-kernel escape drill** as the proof (ties to 4.3). **ACTION:** **Phase-5** adds the sandbox-escape kernel drill to the source-verified scorecard; **Phase-4 CI** owns it as a never-done surface. (Note: this insight is *primarily* owned by the agent-fabric doctrine doc (`03-…`); recorded here only as it appears in §5. Defer detailed routing to that doc's integration.) | Phase 4 (CI) + Phase 5 (escape drill) — primary owner is the `03` doctrine doc |
| 5.2 | **Event volume: an append-everything log can outgrow a general-purpose DB; keep a SEAM for a column-store/time-series engine for the highest-volume streams, but don't add it before the volume is *measured*.** | CONFIRMS | TE-13, SC-5/SC-9 | ADR-10/14: log/firehose tier (wide-column Cassandra/Scylla candidate), OLAP ClickHouse off the bus, firehose split (ADR-04 §5). The **"seam now, engine only when measured"** discipline matches ADR-10's directional posture and §"don't add before measured." CONFIRMS; the measure-first phrasing is a useful guard to carry into P3 sizing. | — (CONFIRMS); measure-first → Phase 3 (Bus sizing) |
| 5.3 | **Search and the reference graph are easy to under-budget; rebuild-from-source (the index never reads source DBs — it asks each owner to re-emit through the live consumer) is what makes them recoverable and drift-free. Treat reindex-from-source as a FIRST-CLASS resilience primitive, not an afterthought.** | **SHARPENS (elevate to first-class primitive)** | TE-1, SC-1/SC-7, GD-3 | Our spine has the *ingredients* — Search/Refs are fed by the bus (ADR-04/13/14), "no subsystem reads another subsystem's DB" (ADR-01/13), Refs built from `ref.created` events — but **nowhere is "reindex-from-source" named as a first-class resilience primitive with the precise mechanism**: *the index never reads owner databases; on rebuild it asks each owner to **re-emit through the live consumer path** (the same path used for steady-state), so recovery uses one code path and cannot drift.* This also reinforces the no-cross-DB law (ADR-13) and gives erasure a recovery story (GD-3: a corrupted index is rebuilt, not patched). **ACTION:** (a) **Phase-2 back-patch / Phase-3**: name **reindex-from-source** as a required capability of **Search** and **Reference Graph** (and any bus-fed read model — OLAP, Notifications), with the exact rule "owners re-emit through the live consumer; the index never reads source DBs." (b) **Phase-5 testing**: a **reindex-from-cold drill** (drop the index, rebuild from source, assert parity) is a source-verified scorecard item. **Default-to-beat (P3 Search/Refs):** every derived store is reconstructible from owner re-emission via the live consumer; no bespoke recovery path. | Phase 3 (Search + Refs + read models) + Phase 5 (reindex drill) + Phase-2 back-patch (note in ADR-13) |

---

## 3. Genuine conflicts

**None that override a committed decision.** The closest is **2.5 (read-time-never-stored
rollups)** vs. our current Knowledge lean toward **async materialised dataflow** (knowledge §3/§9).
This is **not a CONFLICT** — ADR-06 explicitly leaves the formula/rollup engine (TE-18) open to
P4 — but it is a **real tension worth flagging honestly**: our planning currently jumps to the
*harder, materialised* answer, while the doctrine prescribes a *simpler, drift-free read-time
floor first*. Recommended resolution: invert to **floor-first** (read-time → materialised only
when measured too slow), consistent with the doctrine's own "don't add it before the volume is
measured" discipline (§5.2). Recorded as a SHARPENS, routed to P4 Knowledge.

---

## 4. Prioritised deltas (the 5–8 that matter)

1. **[SHARPEN — highest] Git-history erasure is genuinely unsolved and NOT covered by the
   event-log / `PersonalDataHolder` answer (1.2, 1.7).** Back-patch ADR-12 §4 so it stops
   implying the spine *solves* erasure; the pseudonymous-commit-identity default is a
   **commit-time prerequisite**, not a retrofit. Make "erasure vs. immutability reconciliation"
   a **named Phase-3 deliverable** co-owned with Legal/DPO, and **gate the Git P4 data model on
   the pseudonymous-by-default decision** (impossible to bolt on later). *Default-to-beat:
   pseudonymous commit identities by default.* **Binds: Phase-2 back-patch (ADR-12) + Legal/DPO
   + Phase 4 (Git).**

2. **[SHARPEN — sequencing] Build the resume-cursor durable collab transport FIRST; a
   cursor-less relay is a named floor that silently loses the reconnect gap (2.3).** Order
   Knowledge collab as **resume-cursor transport → CAS floor → CRDT**; require resume cursors +
   idempotent apply as a property of the collab firehose; prove it with a reconnect-loses-zero
   drill. **Binds: Phase 6 (sequencing) + Phase 3 (firehose) + Phase 5 (drill).**

3. **[RESOLVES-OPEN] CAS floor as the named v1 collaboration default below the CRDT (2.1).**
   Per-block optimistic compare-and-swap + soft-locks + snapshot/restore, *named as a floor that
   does not merge*; CRDT is the scheduled next step with an explicit trigger ("first true
   concurrent-edit conflict"). De-risks the Knowledge schedule. **Binds: Phase 4 (Knowledge) +
   Phase 6.**

4. **[SHARPEN] Reindex-from-source as a first-class resilience primitive (5.3).** Name it for
   Search, Refs, and every bus-fed read model: *the index never reads owner DBs; owners re-emit
   through the live consumer*. Reinforces the no-cross-DB law and gives erasure/corruption a
   single recovery path. Prove with a reindex-from-cold drill. **Binds: Phase 3 (Search/Refs) +
   Phase 5 + ADR-13 note.**

5. **[RESOLVES-OPEN] Knowledge data-model defaults: markdown-subset inline string + read-time-
   only rollups (2.4, 2.5).** Record both as directional defaults (markdown-subset string with
   structured ref/mention/embed nodes preserved; rollups/formulas computed at read time, never
   stored — materialise only when measured too slow). Inverts our current materialised-first
   lean to floor-first. **Binds: Phase-2 back-patch (ADR-05/06 notes) + Phase 4 (Knowledge).**

6. **[NEW] The deferral discipline: claimed-vs-proven, source-verified scorecard, named
   floor + follow-on, durable gap report (4.2–4.4).** A capability is "proven" only when a drill
   emits a green artifact; every floor names its follow-on; a living gap report carries the real
   state between agents. **Binds: Phase 5 (scorecard) + Phase 8 (execution + gap report) +
   Phase 6 (roadmap convention).**

7. **[SHARPEN] World-scale git storage object-backing as explicit, sequenced work (3.2).** v1
   git storage must keep an object-backing migration seam (repos relocatable, never node-pinned);
   the local-disk → object-store transition is a planned milestone, not an emergent rewrite.
   **Binds: Phase 4 (Git, TE-24) + Phase 6.**

8. **[SHARPEN] Tamper-evident eDiscovery export + the erasure-reconciliation write-up (1.5,
   1.7).** Add a tamper-evident export (legal-hold/eDiscovery) capability to GDPR/Audit and make
   the erasure-vs-immutability reconciliation its own document (folds into delta 1). **Binds:
   Phase 3 (GDPR/Audit) + Legal/DPO.**

---

## 5. Cross-references

- Doctrine source: [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md)
  and [`external-insights/README.md`](../../../external-insights/README.md) (honesty rule §"name your floors").
- Spine touched: ADR-04 (firehose/idempotency), ADR-05 (content model / inline string), ADR-06
  (rollup engine open), ADR-10/14 (storage tiers / object-backing), ADR-12 (erasure spine),
  ADR-13 (no-cross-DB law / reindex-from-source) in
  [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md).
- Open questions resolved/sharpened: GD-1/GD-2 (git erasure), TE-15/16/17/18 (collab + data
  model), TE-24 (git storage), SC-1/SC-7 (refs/search), plus the new process items (§4).
- Companion subsystem docs: [`knowledge-platform.md`](../../02-holistic-architecture/subsystems/knowledge-platform.md),
  [`git-hosting.md`](../../02-holistic-architecture/subsystems/git-hosting.md).
