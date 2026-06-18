# Phase 2 — Cross-Document Consistency Review

> Phase: `02-holistic-architecture`. A cross-document consistency pass over the Phase-2 spine
> ([`architecture-decisions.md`](./architecture-decisions.md)), the holistic narrative
> ([`system-overview.md`](./system-overview.md)), the shared-systems overview
> ([`shared-systems-overview.md`](./shared-systems-overview.md)), the design language
> ([`design-language.md`](./design-language.md)), the CLI/API conventions
> ([`cli-and-api.md`](./cli-and-api.md)), and the five subsystem docs under
> [`subsystems/`](./subsystems/). Checks for: subsystems assuming tech the spine rejected, views
> missing from the catalogue, CLI grammar mismatches, and any ADR contradicted downstream.

---

## 0. Headline

**The Phase-2 corpus is substantially consistent.** Every subsystem doc explicitly ratifies the
ADRs it touches, cites them inline, and (Knowledge §10, Chat §10, Git §10, CI §10, Issues §12)
carries an explicit "does not diverge from the spine" note. No subsystem assumes a technology the
spine rejected; no ADR is contradicted. The issues found are **one mechanical file defect**, a
small set of **CLI noun-grammar mismatches** between the platform convention and the subsystem
illustrations, a few **view-catalogue gaps**, and several **terminology/naming inconsistencies**
that should be normalized before Phase 3/4 consume these docs. None is a design contradiction; all
are editorial or naming-alignment fixes. The consolidated open-questions list (§3) is the real
handoff payload.

---

## 1. Defects & misalignments found (with recommended resolution)

### C-1 — `design-language.md` has stray closing tags (MECHANICAL DEFECT — fix now)
**What conflicts.** The file ends at lines 771–772 with literal `</content>` and `</invoke>`
tags — an artifact of how the file was written. The document's actual last content section is §10
Cross-references; the closing fence (` ``` `) that normally ends the doc is absent and these XML-ish
tags are appended instead.
**Why it matters.** The file is otherwise complete and correct, but the trailing tags are not valid
Markdown and will render as visible junk. A Phase-4 agent diffing/parsing the catalogue could trip.
**Recommended resolution.** Delete lines 771–772 (`</content>` / `</invoke>`). No content change.
**Severity: low (cosmetic), but trivially fixable — do it before commit.**

### C-2 — CLI noun grammar: `repo`/`pr` nesting mismatch between the convention and Git's doc
**What conflicts.** [`cli-and-api.md` §2.1](./cli-and-api.md) defines the noun for git as **`repo`**
and explicitly nests PRs under it (`myelin repo pr …`, `myelin repo pr comment …`), and §7.1 uses
`myelin repo pr create`. The **Git subsystem doc §5.2** instead exposes a top-level **`myelin pr …`**
noun (`myelin pr create|list|view|merge|review`), and the **CI doc §6.1** also uses bare `myelin pr`
indirectly. The design-language §7.7 CLI summary likewise lists `myelin pr create`.
**Why it matters.** This is a genuine grammar inconsistency: is the PR verb `myelin pr create` or
`myelin repo pr create`? The convention doc treats the command path as reconstructing an
`ArtifactRef` (`…/git/pr/<id>`), which argues for nesting under the git noun; the subsystem docs
favour the flatter, ergonomic `myelin pr`.
**Recommended resolution.** Phase 2 should **not** force this now — it is exactly the seam
`cli-and-api.md` §9 **CA-7** defers ("where a domain verb belongs … is per-subsystem"). But the docs
should be made *non-contradictory in wording*: have the Git subsystem doc note that `myelin pr` is
an **alias/shorthand** for the canonical `myelin repo pr` (consistent with §2.2's "three equivalent
addressing forms" philosophy), so both spellings are documented as the same operation. Flag CA-7 as
the resolver. **Severity: low; resolve naming in P4, harmonize wording now.**

### C-3 — Git subsystem `ArtifactRef` subsystem segment uses `git`; one Chat example uses `issues`
**What conflicts.** The canonical `ArtifactRef` is `myelin://<tenant>/<subsystem>/<type>/<id>`
(ADR-13). The CLI convention (§2.2) and most docs use the subsystem segment **`issue`** (singular,
e.g. `myelin://acme-eu/issue/issue/ISSUE-412`) and **`git`** (e.g. `myelin://acme-eu/git/pr/88`).
But **`chat.md` §5 CLI** writes `myelin://acme/issues/issue/ABC-123` (plural **`issues`**) in
`myelin chat ref`, and `knowledge-platform.md` uses **`kb`** as the subsystem segment
(`myelin://acme/kb/page/PAGE-7c2`) while the CLI doc / design language use **`doc`** as the CLI noun
for knowledge.
**Why it matters.** The subsystem segment of `ArtifactRef` is platform law (ADR-13) and is the noun
grammar of the CLI (cli-and-api §1.1). `issue` vs `issues`, and `kb` vs `doc`, are the *same axis*
and must be one canonical token, or refs won't resolve / route consistently.
**Recommended resolution.** Phase 3 owns the canonical subsystem/type token table as part of the
event taxonomy + `ArtifactRef` grammar (ADR-13 §Deferred, CA-2). For Phase 2, **flag the
inconsistency and pick provisional canonical tokens** to stop the drift: recommend singular
`git` / `ci` / `issue` / `knowledge` (or `kb`) / `chat` for the subsystem segment, and align the
CLI noun (`repo`/`ci`/`issue`/`doc`/`chat`) as a *separate, documented* human-facing alias map. The
mismatch between the CLI noun (`doc`) and the ref segment (`kb`/`knowledge`) is the most visible;
normalize it in the Phase-3 taxonomy. **Severity: medium — it touches platform law; resolve the
canonical table in P3, note provisional tokens now.**

### C-4 — Notifications "inbox" CLI noun: `inbox` vs `notify`
**What conflicts.** [`cli-and-api.md` §5.4](./cli-and-api.md) puts the Notifications surface under
the noun **`inbox`** (`myelin inbox list|show|read|snooze|watch|prefs`). The
**shared-systems-overview §5.5/§11** puts it under **`notify`** (`myelin notify prefs`, `notify
test`, plus `oncall`). Both refer to the same shared system.
**Why it matters.** Two CLI nouns for one system. A user/agent won't know whether to type
`myelin inbox prefs` or `myelin notify prefs`.
**Recommended resolution.** Adopt **both, deliberately split by intent**: `myelin inbox …` for the
*per-user "what needs me" feed* (list/read/snooze/watch), and `myelin notify …` for
*preferences/admin/on-call* (`prefs`, `test`, `oncall`). Document the split in the Phase-3 CLI
consolidation. (This is consistent with §5.4 already exposing `inbox prefs` *and* §11 listing
`notify prefs` — they should be one or cross-referenced.) **Severity: low; harmonize in P3 CLI pass.**

### C-5 — GDPR/DSR CLI verb: `gdpr dsr` vs `dsr`
**What conflicts.** [`cli-and-api.md` §5.6](./cli-and-api.md) uses **`myelin gdpr dsr create|status|
receipt`** (nested under a `gdpr` noun, alongside `gdpr datamap`, `gdpr consent`). The
**shared-systems-overview §8.5/§11** uses a top-level **`myelin dsr submit|status|receipt`** and
**`myelin datamap`**, **`myelin audit`**, **`myelin retention`**, **`myelin subprocessor`** without
the `gdpr` prefix. Knowledge §5 adds subsystem-local `myelin kb export-subject` / `kb erase-subject`.
**Why it matters.** Inconsistent nesting of the compliance verbs across two platform-level docs.
**Recommended resolution.** Standardize on the **`gdpr` parent noun** from cli-and-api §5.6
(`gdpr dsr`, `gdpr datamap`, `gdpr consent`, `gdpr retention`, `gdpr subprocessor`) as it groups the
compliance surface coherently and matches §5.6's "GDPR/audit are first-class verbs"; keep `audit` as
its own top-level noun (it spans humans+agents broadly, not only GDPR). The subsystem-local
`kb export-subject` is fine as a convenience that delegates into the same DSR orchestrator. Resolve
in the Phase-3 CLI consolidation. **Severity: low.**

### C-6 — Agent runtime-swap CLI verb: `agent set --runtime` vs `agent runtime set`
**What conflicts.** [`cli-and-api.md` §5.5](./cli-and-api.md) uses **`myelin agent set <id>
--runtime mock|llm`**. The **shared-systems-overview §6.5/§11** uses **`myelin agent runtime set
<id> <runtime_ref>`**. Both express the mock→real strategy-pattern swap (ADR-08).
**Why it matters.** The runtime swap is called out in *both* docs as a load-bearing demonstration of
the strategy pattern; it should have one spelling.
**Recommended resolution.** Pick one in the Phase-3 CLI pass; recommend **`myelin agent set <id>
--runtime <ref>`** (consistent with the `set`-with-flags pattern used elsewhere, e.g.
`agent budget show|set`). **Severity: low.**

### C-7 — CI "trigger" lives under both `myelin ci trigger` and `myelin trigger`
**What conflicts.** The platform convention (cli-and-api §5.5) makes **`myelin trigger`** the
cross-cutting noun for the *one* trigger/automation/agent engine (ADR-08 §5). The **CI doc §5**
exposes **`myelin ci trigger create|list|pause`** as a CI-namespaced surface, and the Git doc §5.2
uses `myelin subscription add`. Issues §5 uses bare `myelin trigger`.
**Why it matters.** ADR-08 §5 is explicit that automations and agents are *one* engine — so a
per-subsystem `ci trigger` / `subscription` could imply parallel engines.
**Recommended resolution.** Clarify (already *almost* stated) that `myelin trigger` is the
**canonical engine surface** and subsystem-namespaced forms (`myelin ci trigger`,
`myelin subscription`) are **scoped conveniences that create the same `Trigger` objects** with the
subsystem pre-bound as the event source — *not* separate engines. Add one sentence to the CLI
convention doc and to each subsystem's trigger section. (Git's `subscription` term is also a
naming drift from `trigger`/`webhook` — recommend renaming to `myelin trigger`/`myelin webhook` for
consistency.) **Severity: low–medium (conceptual clarity around a non-negotiable); harmonize wording.**

### C-8 — View-catalogue gaps: a few subsystem screens not reflected in design-language §7
**What conflicts (minor omissions, not contradictions).** The design-language §7 catalogue is meant
to be the *complete* Phase-4 checklist, but a few screens named in subsystem docs are thin or
absent:
- **Git §4.3 "Erasure / redaction admin"** (the destructive history-rewrite + crypto-shred tool for
  secret/PII incidents) is **not** in design-language §7.1 — it appears only in the shared GDPR
  surfaces (§7.6) generically. It is a git-specific, high-stakes screen and should be named.
- **CI §4 view #11 "Triggers management view"** and **#9 "Caches & artifacts browser"** map only
  loosely to design-language §7.2 (which lists "Environments & deployments" and "Usage/quota" but
  not a dedicated triggers or caches/artifacts browser). Minor — fold in.
- **Issues §4 S13 "Workflow/scheme editor"** and **S14 "SLA policy editor"** are present in
  design-language §7.3 ("Workflow/SLA/field-scheme admin") but collapsed into one line; the
  state-machine graph editor and SLA business-calendar editor are deep surfaces worth itemizing.
- **Chat §4 #14 "Incident/canvas view"** is correctly carried as `[UNCERTAIN/DEFER]` in *both* docs
  (design-language §7.5 and chat §4) — **consistent**, no action.
**Why it matters.** §7 is the Phase-4 sketching checklist; a screen omitted there could be missed.
**Recommended resolution.** In the Phase-4 design-sketch pass (or a light design-language touch-up),
**add the Git erasure/redaction admin screen, the CI triggers + caches/artifacts browser, and
itemize the Issues workflow-scheme and SLA-policy editors** into §7. No conflict — these are
catalogue completeness fixes. **Severity: low; the §7 legend already says each subsystem owns
producing any missing sketch, so the safety net exists.**

### C-9 — "Notifications inbox" vs Issues "My Work hub" vs Chat "Activity inbox" overlap
**What conflicts (acknowledged overlap, needs an owner).** Design-language §5.8 defines **one**
unified notifications inbox ("what needs me"). Issues §4 S10 defines a **"My Work hub"** and notes it
"overlaps the notifications inbox"; Chat §4 #7 defines an **"Activity/Mentions inbox"** feeding the
unified inbox; design-language §7.6 also lists the unified inbox.
**Why it matters.** Three inbox-like surfaces risk fragmenting the "one prioritised inbox" promise
(P8) — the exact failure (notification overload) the platform claims to fix.
**Recommended resolution.** Confirm the **Notifications shared inbox (§5.8) is the one canonical
cross-subsystem inbox**; the Issues "My Work hub" and Chat "Activity inbox" are **scoped views/feeds
into it**, not separate inboxes. State this explicitly in the Phase-4 design sketches so they
compose rather than compete. **Severity: low–medium (UX coherence); already flagged as overlap by
the subsystem docs themselves.**

### C-10 — Knowledge collaboration concurrency: spine says "subsystem-owned", consistently applied
**Checked, no conflict — recorded for confidence.** ADR-05 keeps the collaboration/concurrency
engine subsystem-owned ("share the AST, not the editor"). Knowledge §2/§3/§9 owns CRDT-vs-OT (TE-15);
Issues §3 explicitly states issue bodies are *single-author-at-a-time*, **not** the CRDT engine;
Chat §2/§3 owns its own fan-out. All three correctly consume `myelin-content` (ADR-05) while owning
concurrency. **No action — this is the spine working as intended.**

### C-11 — Frontend stack: design-language recommends TS/React; subsystem docs say "open, TS baseline"
**Checked, no conflict — recorded.** design-language §8 *recommends* TS/React-class with a shared
component library and a WASM-Rust escape hatch; every subsystem doc says "frontend open per VISION
§4; TS/React-class baseline, not mandated." These agree (ADR-02 says the same). The §8.4 divergence
escape hatch (Chat virtualization, Git diff canvas) is consistently flagged in chat §9 and git §9.
**No action.**

### C-12 — CI "CD scope" decision is a Phase-2 position not echoed in the spine ADRs
**What conflicts (mild).** CI §1.1 commits a **Phase-2 position**: "CD = deployment mechanics, not a
hosted PaaS runtime," citing commercial PR-5. This is a real scoping decision but lives *only* in the
CI subsystem doc — it is not reflected in the spine (architecture-decisions.md) or system-overview.
**Why it matters.** It's a meaningful platform-scope commitment (no hosted PaaS in v1) that other
docs (and the roadmap phases) should be able to find in the spine, not buried in one subsystem doc.
**Recommended resolution.** Either (a) accept it as a subsystem-local scope call (it is genuinely
CI-specific) and cross-reference it from ADR-15's commercial backlog, or (b) add a one-line note to
the spine that CD scope = mechanics-not-PaaS for v1 (PR-5, carried). Recommend (a) — keep the spine
lean; ensure ADR-15's PR-5 line points at CI §1.1. **Severity: low.**

---

## 2. Things explicitly checked and found CONSISTENT

- **No subsystem assumes a rejected technology.** Every subsystem defaults to Rust (ADR-02); the
  only flagged divergence (Chat connection tier, BEAM/Elixir, TE-21) is correctly carried as *open*,
  not decided, and is consistent across chat.md §3, ADR-02, and ADR-14.
- **The three glue contracts (ADR-13)** are implemented and cited by all five subsystems; all five
  state "no subsystem reads another's DB" and resolve cross-subsystem reads via the projection API.
- **The firehose/control split (ADR-04)** is consistently applied: CI logs (CI §2.4), chat
  presence/typing/read-state (chat §2.8), and knowledge collab op-streams (knowledge §2) all ride
  the firehose, never the durable bus — matching ADR-04.5 and system-overview §7.2.
- **Plan-then-apply + one trigger engine + HITL-in-chat (ADR-08/09)** are described identically in
  the agent flagship walkthrough across system-overview §8.2, cli-and-api §7.2, git §6.2, ci §6.2,
  issues §6.2, knowledge §6.2, and chat §6.2 — the same `correlation_id`, the same gated `open_pr`.
- **The two named seams** (Git↔CI checks contract; Issues↔Knowledge shared field/view primitive) are
  named identically in system-overview §4, ADR-06, git §9, ci §10, issues §9, and knowledge §7.3.
- **`PersonalDataHolder` + crypto-shred + pseudonyms (ADR-12)** are implemented by every subsystem
  with the same technique (references-not-payloads + crypto-shred + tombstone), and all flag the
  free-text-PII residual as `[OPEN — LEGAL]` GD-6 honestly.
- **The shared field/view/query primitive (ADR-06/07)** is consumed (not re-implemented) identically
  by Issues (§1.4) and Knowledge (§3), with engines/execution kept subsystem-owned in both.
- **The common response envelope + `ArtifactRef`-as-noun (cli-and-api §4.2)** is consistent with the
  event envelope (ADR-13.2) and the addressing contract; webhooks carry the same envelope (§4.5).

---

## 3. Consolidated open questions carried into Phase 3 / Phase 4 / Legal

This is the merged, de-duplicated backlog (from ADR-15, shared-systems-overview §12, cli-and-api §9,
and each subsystem's open-questions section). It is the real handoff payload of this review.

### → Phase 3 (shared systems)
- **Identity/Authz (ADR-03):** the ReBAC tuple schema + per-subsystem relation namespaces;
  consistency-token/caching strategy; the **delegation/on-behalf-of algebra** (AG-2); **`Agent` vs
  `Service`** as one principal kind or two (AG-1); scope→ReBAC compilation (CA-5); cross-tenant
  reference visibility gating; multi-cell principal authority; cheap *cached, per-viewer*
  `check`/`list-objects` for chat unfurls; ReBAC expressing ref-pattern-scoped + field-/transition-/
  browse-/confidential-issue relations efficiently.
- **Event Bus (ADR-04):** durable-bus + firehose **transport selection**; partitioning/sharding
  (incl. per-ref ordering at git-push QPS); the **`EventMatcher` predicate language** (CEL/JSONLogic/
  custom, AG-7); replay/compaction; retention windows; the **canonical event taxonomy + dotted
  names** (TE-10, CA-2) and the canonical `ArtifactRef` subsystem/type token table (resolves C-3);
  cross-cell propagation; cheap predicate matching at firehose ingress.
- **Refs (ADR-13):** **Refs-owns-hierarchy/relations vs subsystem-local materialized tree** (TE-7);
  cross-tenant edge gating; hot-artifact backlink scale; block-granular (`#block-id`) sub-artifact
  refs + tombstoning.
- **Search (ADR-03/07/10):** engine selection; the `list-objects`↔index integration mechanism
  (filter push-down vs pre-fetch); vector/embedding approach + its erasure; multilingual analyzer
  set; block-vs-page index granularity. *(Code-search v1 scope → P4.)*
- **Notifications (ADR-12):** priority/ranking model; storm-control/dedup algorithm; on-call/
  escalation data model (with the durable-workflow engine); delivery-provider sovereignty posture.
- **Agents (ADR-08/09):** the **`Agent::handle` signature** / streaming / context management (AG-3,
  provisional); adversarial loop/runaway + load-governance validation (AG-4/AG-5, with P5 testing);
  the HITL approval-card data model + signal round-trip (joint Chat↔Fabric↔Workflow).
- **Storage (ADR-10):** per-store engines; sharding internals; **per-subject vs per-tenant
  crypto-shred granularity** (GD-4); HYOK limits on what search/agents can do over data they can't
  decrypt; backup-window-vs-erasure-SLA residual.
- **GDPR/Audit (ADR-12):** DSR orchestrator design; KMS key hierarchy; retention engine; consent +
  sub-processor registries; post-restore re-erasure (GD-14); multi-cell DSR fan-out.
- **Substrates:** durable-workflow **build-vs-adopt** (TE-20); OLAP feed/schema.
- **Tenancy (ADR-11):** cell sizing; tenant→cell assignment; **multi-cell tenants** + cross-cell
  collaboration/latency (SC-2/SC-3, the deepest unknown); the control plane holding zero in-region
  personal data; CLI endpoint/cell discovery for multi-cell tenants (CA-6).
- **CLI/API (cli-and-api §9):** MCP wire-spec conformance (CA-4); token/credential model details
  (CA-5); streaming/long-poll mechanics for `--watch`/`tail`/`--wait` (CA-8); the
  `--filter`/`--on` predicate dialect + human↔AST renderer (CA-3). **Plus the naming harmonizations
  from §1 (C-4/C-5/C-6/C-7).**

### → Phase 4 (subsystems)
- **Git:** storage/replication backend (TE-24); git-core build-vs-embed + reftable maturity (TE-8);
  monorepo ambition (TE-25); diff/comment anchoring across rewrites (TE-22); SHA-1-vs-256 (TE-23);
  forks/merge-queue/web-edit scope (TE-26); code-search v1 scope (TE-27); push-policy execution
  locus; multi-tenancy isolation level for git; the Git↔CI checks contract (joint with CI).
- **CI:** isolation model (TE-28); runner ownership/EU infra (TE-29); config grammar; component/
  action registry supply-chain (TE-30); CI↔agent substrate unification depth (TE-31); metering unit
  (TE-32); scheduler internals; multi-region execution boundaries; local execution.
- **Issues:** Epic/Initiative as type-vs-level (PR-2); governance baked-in-vs-opt-in (PR-3);
  flexible-field storage/query engine (TE-17); human-readable monotonic keys (TE-14); drag-reorder
  ranking (TE-19); rollup/forecast engine (TE-18); SLA business-calendar engine; real-time sync
  engine; import fidelity (PR-8); offline/local-first scope.
- **Knowledge:** CRDT-vs-OT + granularity (TE-15); block-tree storage (TE-16); flexible-DB query
  model (TE-17); formula/rollup engine (TE-18); permission granularity (page/row/field); folders-vs-
  pure-pages; offline depth; search granularity + vector-in-v1; synced blocks/transclusion;
  comments reuse-Chat-vs-native; embed liveness; multi-region collab locality; shared templating.
- **Chat:** connection-tier transport + language (TE-21); message-store substrate + tiering;
  write-vs-read-fanout boundary; unfurl live-vs-snapshot + cheap per-viewer resolution; erasure
  mechanism specifics; group-DM-vs-private-channel; threads UX; agent presence/streaming semantics;
  agent loop/abuse chat-side mechanism; canvas-vs-Knowledge boundary; cross-org/federated channels.
- **Design (per subsystem):** concrete token values; block-taxonomy completeness + extension; unfurl
  live-vs-snapshot edge cases; collaboration-concurrency UX; designer-persona depth (canvas);
  offline scope; mobile/native-app scope; frontend-stack divergences; persona-adaptive vocabulary.
  **Plus the view-catalogue completeness fixes from C-8 and the inbox-overlap resolution from C-9.**

### → Legal / DPO (before binding)
Art. 17 erasure scope into immutable git history (GD-1/GD-2); audit-log retention carve-out (GD-5);
free-text PII completeness (GD-6); Schrems-III / EU–US DPF stability (GD-7); CLOUD-Act exposure of
"EU sovereign" hyperscaler partnerships (GD-8); EU AI Act final classification (GD-9); Gaia-X/EUCS/
NIS2/DORA/eIDAS-2/Data-Act applicability (GD-10); controller/processor classification per category
(GD-11); worklog/productivity special-category data (GD-13); GDPR-vs-LLM erasure of
decision-influencing data (AG-8); EU-sovereign real-LLM sub-processor (AG-9).

### Commercial (outside engineering phases)
Segment priority/WTP (PR-1); **CD scope = mechanics-not-PaaS for v1** (PR-5, owned in CI §1.1 — see
C-12); CI config format (PR-6); pricing/packaging/GTM/certification (PR-9); the narrowing
agent-native gap (PR-10).

---

## 4. Recommended pre-commit actions (smallest set)

1. **C-1 — delete the stray `</content>`/`</invoke>` lines in `design-language.md`** (mechanical;
   do now).
2. **C-3 — add a one-line note** flagging the `ArtifactRef` subsystem-segment token inconsistency
   (`issue`/`issues`, `kb`/`knowledge`) as resolved by the Phase-3 canonical taxonomy (CA-2), so no
   downstream agent treats a provisional spelling as canonical.
3. **C-7 — add one clarifying sentence** (CLI convention doc + each subsystem trigger section) that
   subsystem-namespaced trigger commands create objects in the **one** shared trigger engine
   (ADR-08 §5), not parallel engines.
4. The remaining items (C-2, C-4, C-5, C-6, C-8, C-9, C-12) are **naming/catalogue harmonizations
   best done in the Phase-3 CLI consolidation and the Phase-4 design-sketch pass** — they are noted
   here so those phases resolve them deliberately rather than rediscovering them.

---

## 5. Cross-references
- [`architecture-decisions.md`](./architecture-decisions.md) — the ADRs every consistency check is
  measured against (esp. ADR-04 firehose, ADR-05/06 share-boundaries, ADR-08 one-engine, ADR-13
  glue + addressing).
- [`README.md`](./README.md) — the Phase-2 index/executive summary this review accompanies.
- [`shared-systems-overview.md`](./shared-systems-overview.md) §11–§12 and
  [`cli-and-api.md`](./cli-and-api.md) §9 — the CLI/open-question sources reconciled here.
