# Phase 7 — The Prompt Ledger: overview, template, numbering, and coverage contract

> Phase: `07-prompts`. **The single entry point** to the Myelin build prompts. This document defines how the
> whole prompt ledger works: the template every prompt follows, the global numbering + interleaving scheme,
> the coverage contract that ties every roadmap milestone to at least one prompt, and the repo/workspace
> conventions every prompt assumes. It is the spec the per-system prompt files and the consolidated ledger
> index (`01-ledger-index.md`, Phase 7-B) build to.
> Canonical brief: [`../../VISION.md`](../../VISION.md) §7 (convert the roadmaps into ONE sequence of prompts,
> target 400k–700k tokens, each fed to a coding agent with clean context, each self-contained, each commits
> when done) — never contradicted. Binding doctrine:
> [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md)
> (§1 code-wins-over-docs + name-your-floors; §2 order-by-non-negotiability + the gate invariant; §3
> prove-it-or-it-isn't-real with a quantified drill + observability-is-part-of-the-pass; §5 the ratchet /
> committed gates). Frozen sequence (this phase OPERATIONALIZES, it does not redesign):
> [`../06-roadmaps/00-master-sequencing.md`](../06-roadmaps/00-master-sequencing.md) (the M0..M6 bands + the
> gate invariant), [`../06-roadmaps/README.md`](../06-roadmaps/README.md) (the consolidated timeline + the
> critical path + the ordered drill-gate sequence), the per-system roadmaps under
> [`../06-roadmaps/shared/`](../06-roadmaps/shared/) + [`../06-roadmaps/subsystems/`](../06-roadmaps/subsystems/),
> the reconciled architecture under [`../05-refined-shared-systems-architecture/`](../05-refined-shared-systems-architecture/)
> (the 11 refined shared docs + `00-reconciliation-decisions.md` + `contract-index.md`) and the rewritten
> subsystems under `../04-subsystem-architectures/<slug>/architecture/`. Spine: ADR-01..ADR-20
> ([`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)).
> Date: 2026-06-19.
>
> **What this is.** Every Myelin coding prompt is fed to a coding agent with **clean context** — it knows
> nothing but what the prompt tells it to read. So a prompt must be **self-contained**: it names the exact
> canon docs to read, the exact deliverable to build and where, the contracts to implement, the quantified
> gate/drills that prove it real, the tests owed, and the closing instruction to commit. This document fixes
> the shape of that prompt, the order prompts run in, and the rule that guarantees no roadmap milestone is
> ever dropped. Identifiers are plain text (no backticks-as-emphasis). Markdown only; no git commits by this
> document or its author.

---

## 1. The frame — what Phase 7 produces and how the pieces nest

Phase 7's job (VISION §7) is to **convert the roadmaps into ONE ordered sequence of prompts** that
operationalize every roadmap milestone into a coding task (research only where a real unknown blocks the
code). The architecture is frozen (Phase 5); the build order is frozen (Phase 6). Phase 7 adds the **executable
unit of work**: a clean-context, independently-committable prompt that a single agent runs to completion and
commits, after which the next prompt runs (VISION §8 — sequential, one agent at a time).

Three layers of document, nested like Phase 6:

1. **This overview** (`00-ledger-overview.md`) — the template, the numbering grammar, the interleaving rule,
   the coverage contract, and the workspace conventions. The spec every prompt and the index build to.

2. **The per-system prompt files** (Phase 7-A) — one file per system under `shared/<system>.md` and
   `subsystems/<system>.md` (mirroring the Phase-6 roadmap layout), each carrying that system's prompts in
   band order. A system yields roughly 8–20 prompts (§4). These are authored against each system's Phase-6
   roadmap: every roadmap milestone in that file maps to one or more prompts here.

3. **The consolidated ledger index** (`01-ledger-index.md`, Phase 7-B) — the reconciliation layer. It
   **interleaves** all per-system prompts into the single global execution order (§3), assigns each its stable
   global id, carries the coverage matrix (every roadmap milestone → its prompt ids, §5), and is the document
   the execution phase reads top-to-bottom. The index is the source of truth for *order*; the per-system files
   are the source of truth for *content*.

**The gate invariant binds the whole ledger (EI-01 §2):** a prompt may not be executed (and certainly not
called done) while an earlier-band prompt's gate is red. The ledger order *is* the gate order — running the
prompts in ledger order, each to a green gate, is what enforces the master-sequencing band invariant at the
build layer. A prompt's DEPENDS-ON edges make this concrete per-prompt; the band column makes it concrete
per-band.

---

## 2. The prompt template (every prompt follows this exact shape)

Every prompt in the ledger is a self-contained Markdown block with the fields below, in this order. The
executing agent has clean context: it reads **only** what the CANON DOCS field names, builds **only** what the
DELIVERABLE field names, and stops at the gate the GATE/DRILLS field quantifies. A field is never left
implicit — silence is a defect (EI-01 §1, name-your-floors). The template:

---

### P-<NNN> — <short imperative title>

- **BAND.** One of M0 / M1 / M2 / M3 / M4 / M5 / M6 (the master-sequencing band this prompt's work lives in).
- **ROADMAP MILESTONE.** The exact per-system milestone id this prompt implements (e.g. `ID-M1`, `B-M0`,
  `KN-M3a`, `M4-C1`, `FLOW-M2.1`) **+** the path to its roadmap file
  (`../06-roadmaps/shared/<system>.md` or `../06-roadmaps/subsystems/<system>.md`). One prompt implements one
  milestone or a named slice of one; the coverage matrix (§5) is keyed on this field.
- **DEPENDS-ON.** The list of prior prompt ids (`P-012, P-031, …`) that **must be merged before this prompt
  starts** — never "should", always "must". Empty only for the root prompt(s) of M0. These edges are the
  build-layer realization of the Phase-6 cross-system dependency DAG (`06-roadmaps/README.md` §5); the index
  verifies they are acyclic and that every depended-on id precedes this one in the global order.
- **CANON DOCS (read these first, in full, before writing any code).** The precise, clickable paths the agent
  must read. Required members:
  - **VISION + doctrine anchors:** `../../VISION.md` (always) and the specific `external-insights/` section(s)
    whose discipline this prompt carries (e.g. EI-01 §3 for a drill prompt; EI-04 §1 for erasure-vs-immutability).
  - **The architecture site:** the exact refined-shared doc
    (`../05-refined-shared-systems-architecture/<system>.md`) and/or the subsystem architecture folder
    (`../04-subsystem-architectures/<slug>/architecture/…`) that specifies what is being built — to the
    section if the doc is large.
  - **The contracts:** the specific rows of
    `../05-refined-shared-systems-architecture/contract-index.md` this prompt implements or calls (by cluster +
    number, e.g. "4.3 `list_objects`, 4.6 `write_tuples`"), and `00-reconciliation-decisions.md` where the
    shape's *rationale* (X-1..X-7 / OQ-A..OQ-L) is needed.
  - **The roadmap milestone:** the Phase-6 roadmap file + section for the milestone in the ROADMAP MILESTONE
    field (so the agent reads the floor-then-full progression and the gate in context).
  - **The drill/test source:** the relevant rows of
    `../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` for
    the drills this prompt must green, and `…/testing-strategy/README.md` for the strategy.
  Reading is scoped: name sections, not whole 25k-token files, wherever the architecture doc supports it.
- **DELIVERABLE (what to build + exactly where in the repo).** The concrete artifacts: which crate(s) under
  the Cargo workspace (§6) gain which modules/types/functions, which migrations, which fixtures, which
  binaries. Paths are repo-relative and explicit (e.g. "in crate `myelin-identity`, add `list_objects.rs`
  implementing the `SetExpr` push-down and the S8 reverse-index `EventHandler`"). If the work ships a **floor**
  (VISION §3, EI-04 §4), the field **names the floor and names the follow-on prompt/band** — a floor that
  masquerades as done is the only failure.
- **CONTRACTS TO IMPLEMENT.** The contract-index rows this prompt makes real (owned) or wires up (consumed),
  each by its number + name. Owned contracts ship with the provider side; consumed contracts ship the call
  site. The contract signatures are stable (ADR-01) — the prompt implements to the frozen shape, it does not
  redesign it; a needed shape change is escalated, not silently diverged (EI-01 §1, code-wins-over-docs: if the
  code must diverge, write down why and fix the doc).
- **GATE / DRILLS (quantified; must be green to call this done).** The named catalogue drills + lint(s) that
  must emit a **dated green artifact** for this prompt to be done, each with its **quantified threshold** and
  the **telemetry signal** (contract 1.8) that is the green artifact (EI-01 §3: a target you cannot measure is
  not a gate; observability is part of the pass condition). Examples: "ID-D3 → 0 cross-tenant tuples readable,
  cross-tenant-count signal = 0, CI"; "the `tenant-predicate` lint green with both fixtures". A prompt whose
  band-boundary drill is a **permanent gate** (AG-D4/CI-T1, STOR-D1/STOR-D2) says so. **Never weaken a
  threshold or invert an assertion to make a check pass** (EI-01 §3) — a red gate is information; record a
  dated "claimed, not proven" / "needs human verification" note instead.
- **TESTS (required).** The test obligations: (a) **unit tests** for the logic built, and (b) the **relevant
  contract test (CDC) and/or drill test** — the provider+consumer CDC pair for any contract row this prompt
  owns or consumes (the contract-coverage scanner fails the build if a row lacks both, §6), and the drill
  harness scenario for each GATE/DRILLS row. Where the mandatory-core mutation gate applies (cargo-mutants,
  §6), the prompt states the mutation-score floor for the core module it touches. Tests that **chain mutations**
  end-to-end are preferred over single-handler tests where the property is a sequence property (EI-01 §4).
- **DEFINITION OF DONE.** A prompt is **done** when, and only when, **all** hold: the DELIVERABLE exists and
  compiles in the workspace; the CONTRACTS TO IMPLEMENT are implemented/wired to the frozen shape; **every
  GATE/DRILLS row emits its dated green artifact** (PROVEN, not CLAIMED — EI-01 §3); the TESTS exist and pass
  (unit + CDC + drill), the contract-coverage scanner passes, and all committed lints are green; any **floor**
  is named in writing with its follow-on; any **untested-but-named** surface is honestly recorded (yes/no/
  partial — EI-01 §4, silent skipping is the failure); and the work is **committed** (below). A red gate does
  **not** become green by editing the threshold — it becomes a dated scorecard row and, if it blocks, an
  escalation. "Looks done" is never done.
- **COMMIT (the closing instruction — do this when, and only when, DEFINITION OF DONE holds).** Stage the work
  and commit with the message header `P-<NNN> <BAND>: <short title>` and a body that lists: the contracts
  implemented, the drills greened (with their measured numbers), any floor named + its follow-on, and any
  untested-but-named surface. Branch first if on the default branch; do not push unless the orchestrator asks.
  End the commit message with the workspace's required `Co-Authored-By:` trailer. One prompt = one commit
  (squash incidental fixups) so the ledger order maps one-to-one onto the commit history (the dogfood loop in
  M6 reads this).

---

**The "definition of done" rule, stated once for the whole ledger:** *A prompt is done only when its
DEFINITION OF DONE field's conjunction is true — deliverable built to the frozen contract, every quantified
gate emitting a dated green artifact, tests (unit + contract + drill) passing, lints + coverage scanner green,
floors and untested surfaces named in writing, and the work committed. Nothing is marked done over a red gate;
a threshold is never weakened to manufacture green; the code wins over the docs (if reality forces a
deviation, write it down and fix the doc, then proceed).* This rule is binding on every prompt and is the
build-layer expression of EI-01 §1/§2/§3/§5.

---

## 3. Numbering + interleaving (the single global execution order)

### 3.1 The id grammar

Every prompt has a **stable global id** of the form **`P-<NNN>`** — a capital P, a hyphen, and a
zero-padded three-digit ordinal (`P-001`, `P-002`, … `P-137`). Properties:

- **Stable.** Once assigned in the index, an id never changes — DEPENDS-ON edges, commit headers, and the
  coverage matrix all reference it. New prompts discovered mid-build (EI-01: every incident adds a drill;
  gaps surface) are appended with the next free ordinal and slotted into order by their DEPENDS-ON edges, never
  by renumbering existing ids. The ordinal encodes *assignment order*, and (because of how the index is built,
  §3.2) it is also **monotonic in band** — but DEPENDS-ON, not the number, is the authoritative ordering
  constraint within a band.
- **Three digits.** The target is 400k–700k tokens at ~1500–4000 tokens of *executing-agent work* per prompt
  (§4) across 16 systems — on the order of 120–200 prompts. Three digits leaves headroom for appended prompts.
- **Per-system tag is in the ROADMAP MILESTONE field, not the id.** The global id is system-agnostic so the
  execution order is one flat sequence; the system a prompt belongs to is read from its milestone tag.

### 3.2 How per-system prompt files interleave into one order (Phase 7-B)

The per-system files (7-A) are authored **independently, in each system's own band order**. Phase 7-B's index
**interleaves** them into one global sequence by the following deterministic procedure:

1. **Primary sort: by band, M0 → M6.** All M0 prompts precede all M1 prompts, etc. The band column is the
   coarse key; the master-sequencing gate invariant means no band's prompts run before the prior band's exit
   gate is green.
2. **Within a band: topological order by the cross-system dependency DAG** (`06-roadmaps/README.md` §5 + each
   prompt's DEPENDS-ON). A prompt comes after every prompt it depends on. Where the DAG leaves two prompts
   unordered (no path between them), they may run in either order — but the index fixes one concrete order so
   the sequence is total and reproducible. The within-band tie-break, in priority order: (a) the
   order-by-non-negotiability tier (EI-01 §2 / master §1 Tiers 0–6 — the harness and the data-loss/RCE/lint
   keystones first), then (b) the critical-path spine (`06-roadmaps/README.md` §4) ahead of its branches, then
   (c) lower system index (the §2.1/§2.2 roadmap-index numbering) for full determinism.
3. **Cross-band split contracts keep their declaration order.** Where a contract is split across bands (the
   X-1 Git↔CI check seam: declared M2, consumer half Git M3, producer half CI M4 — `06-roadmaps/README.md`
   §7.1), each half is a separate prompt in its own band; the later half DEPENDS-ON the earlier. The index never
   collapses a cross-band seam into one prompt.
4. **The two permanent gates get re-confirmation prompts in every band that re-runs them.** AG-D4/CI-T1 (every
   backend/image/kernel change) and STOR-D1/STOR-D2 (every store-touching change) appear as explicit
   re-confirm prompts at the M4 boundary (CI prod image) and wherever a band adds a store — each a real prompt
   with a real gate, not an assumed background check (EI-01 §5, an uncommitted gate is no gate).
5. **Assign `P-<NNN>` in the resulting total order.** The ordinal is handed out front-to-back over the
   interleaved sequence, so reading `P-001 … P-NNN` top-to-bottom *is* the execution order.

The result: one flat, totally-ordered, reproducible sequence whose order encodes the band gates, the
dependency DAG, and the order-by-non-negotiability thesis — exactly what the execution phase (VISION §8) runs
one agent at a time.

---

## 4. Granularity (one prompt = one clean-context, independently-committable unit)

The unit of a prompt is **a clean-context-sized, independently-committable piece of work**: small enough that
a single agent can hold it with no prior context and finish it in one sitting ending in one commit, large
enough to be a coherent, separately-gateable deliverable. Calibration:

- **Token budget per prompt: ~1500–4000 tokens** of executing-agent work (the prompt body + the bounded code/
  test it asks for). Below ~1500 the work is a fragment that should fold into its neighbour; above ~4000 the
  agent's context fills with the work itself and the clean-context property is lost — split it.
- **A system yields ~8–20 prompts**, depending on size: a small shared crate (Notifications, Refs) lands near
  the low end; Identity, Storage, the Event bus, Git, Knowledge, CI near the high end. Total across all 16
  systems lands in the VISION §7 band of **400k–700k tokens**.
- **One prompt implements one roadmap milestone, or one named slice of a large one.** A milestone like Identity
  `M1` (the whole Id surface) is too large for one prompt — it splits into a handful (authenticate; check +
  CaveatContext; `list_objects` + S8 reverse index; write_tuples/zookie; delegation + mint_run_token; the
  ReBAC engine + core hierarchy; fail-static + pseudonym shred), each independently gateable. A small milestone
  (an M0 lint with its red+green fixtures) is one prompt. The coverage matrix (§5) records the
  milestone → prompt(s) mapping either way.
- **The seam of a split is a contract or a drill,** never an arbitrary line count: split where a sub-deliverable
  has its own green gate. Each resulting prompt's DEFINITION OF DONE stands on its own.

**The compounding-payoff check (EI-01 closing):** if late-band prompts are *larger* and harder than early ones,
the substrate prompts under-built something — that is a signal to add an intermediate prompt, not to bloat the
late one. Well-ordered, each new surface is a projection of capabilities earlier prompts already shipped, so
prompts trend *smaller* up the bands.

---

## 5. The coverage contract (every roadmap milestone maps to ≥1 prompt)

The binding guarantee of the ledger: **every roadmap milestone across all 16 systems maps to at least one
prompt.** Nothing in Phase 6 is silently dropped on the way to Phase 7.

- **The unit of coverage is the per-system roadmap milestone** — every milestone named in every
  `06-roadmaps/shared/*.md` and `06-roadmaps/subsystems/*.md` (the system-prefix + band ids: `SUB-M0`, `B-M0`,
  `S-M0`, `ID-M1`, `GA-M1`, `CP-M1`, `R-M2`, `N-M2.x`, `FLOW-M2.1`, `KN-M3a`, `M4-C1`, …) **and** the master
  sequencing's per-band work rows.
- **The coverage matrix lives in `01-ledger-index.md`** (Phase 7-B): a table of every milestone → the prompt
  id(s) that implement it. The index **verifies** the mapping is total — a milestone with zero prompts is a
  coverage failure the index must surface and resolve before Phase 7 is done (the same discipline as the
  contract-coverage scanner: an unmapped milestone is a hole, made loud, never swallowed — EI-01 §5).
- **Many-to-one is expected both ways.** One milestone → several prompts (a large milestone split, §4); several
  milestones → one prompt only when they are genuinely one committable unit (rare; prefer splitting). A prompt
  always names exactly one *primary* milestone in its ROADMAP MILESTONE field; secondary milestones it also
  advances are listed in the coverage matrix, not the prompt header.
- **The gate side of coverage:** every band-boundary drill in the ordered gate sequence
  (`06-roadmaps/README.md` §6) must be greened by some prompt's GATE/DRILLS field. The index cross-checks that
  the drill catalogue's proof obligations each appear as a gate on at least one prompt — a drill no prompt
  greens is as much a hole as a milestone no prompt implements.
- **Floors are covered by their follow-on prompts.** Each named floor in the master sequencing §5 / per-system
  roadmaps must have both a prompt that ships the floor (in its floor band) and a prompt that ships the
  follow-on (in its follow-on band); the coverage matrix links the pair so the gap is visible, never invisible
  (EI-04 §4).

---

## 6. The repo / workspace conventions every prompt assumes

Prompts are written against a fixed substrate so the DELIVERABLE field can name paths without re-explaining the
layout each time. The conventions, from ADR-01 and the Phase-5/6 substrate:

- **The Cargo workspace + glue crates (ADR-01).** Myelin is one Cargo workspace. The cross-system contracts
  live in **glue crates** that are compile-time contract carriers — `myelin-events`, `myelin-identity`,
  `myelin-refs`, `myelin-agent`, `myelin-gdpr`, `myelin-content`, `myelin-query`, `myelin-tenancy` (plus the
  substrate crates `myelin-client` for `ResilientClient`, the harness crate for `serve(AppSpec)`, and each
  system's own implementation crate). A change to a glue contract breaks **every consumer's build now**, never
  silently in prod — so a prompt that implements a contract implements it *to the frozen signature* in its glue
  crate, and a prompt that needs a shape change is a whole-workspace contract PR, escalated and written down,
  not a local divergence. The M0 prompts that lay down the workspace + the eight glue-crate skeletons are the
  first prompts in the ledger (master §2 M0; substrate roadmap `SUB-M0`).
- **The substrate bootstrap harness (contract 1.1, `serve(AppSpec)`).** Every service prompt assumes the
  harness exists from M0: `serve(AppSpec)` does boot → migrate → outbox relay → consumers → three ports
  (public / internal / metrics-health, 1.2) → graceful drain, with liveness ≠ readiness (1.3), forward-only
  online migrations (1.5), `PersonalDataHolder` auto-registration on every store opened (1.4),
  `ResilientClient` (1.9) and `FailStatic<T>` (1.10) primitives, and the protected-human-lane shed order
  (1.11). A subsystem prompt's DELIVERABLE is "an `AppSpec` + the handlers/consumers the harness wires", not a
  hand-rolled `main`. The cross-language harness shim (1.7) is the frozen contract a non-Rust tier (Chat's
  connection tier, TE-21) must satisfy — a prompt building such a tier builds *to that shim*.
- **The committed lints as gates (contract 1.6, the ratchet — EI-01 §5).** The twelve architecture lints
  (`no-cross-db`, `no-raw-publish`, `tenant-predicate`, `no-host-exec`, `forward-only-migration`,
  `no-cross-sync-cycle`, `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`,
  `no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`) are committed CI gates from M0, each
  with a red-fixture (proves it rejects) + a green-fixture (proves it admits), wired loud-never-swallowed (no
  `... || true`). Every prompt's DEFINITION OF DONE requires all committed lints green; a prompt whose
  milestone *adds* a lint ships both fixtures as part of its DELIVERABLE. An uncommitted lint is no lint — the
  prompt wires it into CI, it does not leave it on disk.
- **The contract-coverage scanner + the failure-injection harness (M0).** The scanner fails the workspace if
  any contract-index row lacks a provider + consumer CDC pair — so every contract prompt's TESTS field includes
  the CDC pair. The failure-injection harness (the 1×/10×/30× load generator with mixed principal kinds, the
  scoped-reversible dependency-break injector, the telemetry-assertion library reading contract 1.8) is the
  Tier-0 unit of proof built by the first M0 prompts; every later prompt's drill is *a scenario on that
  harness*, asserting against the named survival signals. The thresholds file (one versioned file of every
  default-to-beat) is where a GATE/DRILLS number is read from and where a red gate becomes a dated
  "claimed-not-proven" row.
- **The mutation gate (cargo-mutants).** The repo `.gitignore` is pre-seeded for `cargo-mutants` (VISION §4 —
  the expected quality bar). Mandatory-core modules carry a mutation-score floor; a prompt touching a core
  module states that floor in its TESTS field, and M6 runs the mutation gate as a Myelin CI job on every Myelin
  commit (the dogfood loop). Test stacks for non-Rust tiers (Chat's connection tier, if it diverges) satisfy
  the equivalent gate via the 1.7 shim's test obligations.

---

## 7. Digest

**The prompt template fields (in order):** `P-<NNN>` id + short imperative **title**; **BAND** (M0..M6);
**ROADMAP MILESTONE** (the per-system milestone id + its roadmap file path); **DEPENDS-ON** (prior prompt ids
that must be merged first); **CANON DOCS** (VISION + the exact `external-insights/` section; the exact 04/05
architecture site; the precise `05/contract-index.md` rows + `00-reconciliation-decisions.md`; the 06 roadmap
milestone; the testing-strategy drill rows); **DELIVERABLE** (what to build + exact repo paths/crates; floors
named with follow-ons); **CONTRACTS TO IMPLEMENT** (the contract-index rows, owned/consumed, to the frozen
shape); **GATE / DRILLS** (the named catalogue drills + lints, each quantified, with its telemetry green
artifact — never weakened to pass); **TESTS** (unit + the contract CDC pair + the drill scenario; mutation
floor on core); **DEFINITION OF DONE** (deliverable built + every gate green-and-dated + tests pass + lints +
coverage scanner green + floors/untested-surfaces named + committed); **COMMIT** (commit when and only when DoD
holds, header `P-<NNN> <BAND>: <title>`, body listing contracts/drills/floors, with the Co-Authored-By
trailer).

**The numbering scheme:** stable global id **`P-<NNN>`** (capital P, hyphen, zero-padded three-digit ordinal),
system-agnostic, assigned once and never renumbered. The single execution order is built (Phase 7-B,
`01-ledger-index.md`) by interleaving the per-system files: **primary sort by band M0→M6**, **within a band
topological by the cross-system DAG + DEPENDS-ON**, tie-broken by (a) order-by-non-negotiability tier, (b)
critical-path-spine-before-branches, (c) lower system index — then the ordinal is handed out front-to-back over
the resulting total order, so `P-001 … P-NNN` read top-to-bottom *is* the run order. Cross-band split contracts
(X-1) keep declaration order as separate per-band prompts; the two permanent gates (AG-D4/CI-T1, STOR-D1/D2)
get explicit re-confirm prompts in each band that re-runs them.

**The coverage contract:** every roadmap milestone across all 16 systems → ≥1 prompt; the index's coverage
matrix verifies the mapping is total and surfaces any unmapped milestone, unmapped band-boundary drill, or
unpaired floor as a loud hole. **Granularity:** one prompt = one clean-context, independently-committable unit,
~1500–4000 tokens, ~8–20 prompts per system, total 400k–700k tokens; split a milestone where a sub-deliverable
has its own green gate. **Workspace conventions assumed:** the ADR-01 Cargo workspace + the eight glue crates
(compile-time contract carriers), the `serve(AppSpec)` substrate harness (three ports, holder auto-registration,
resilient-client, fail-static, shed order), the twelve committed lints + the contract-coverage scanner + the
failure-injection harness + the cargo-mutants mutation gate — all from M0, all the gates every prompt's
DEFINITION OF DONE leans on.
