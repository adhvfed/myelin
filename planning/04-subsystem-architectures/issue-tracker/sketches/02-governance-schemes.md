# Sketch 02 — Governance: baked-in vs opt-in schemes (PR-3)

> Exploration note. Weighs Phase-2 §11 Q2 / deep-dive §12.3-#2 / PR-3: "how much governance is baked-in vs
> opt-in scheme" — the "Linear-fast by default, Jira-powerful on demand, **without a fork in the product**"
> decision (deep-dive §4.4; design-language P4 progressive disclosure). Leans; commit in `00-findings.md`.

## The question

An org must be able to start Linear-simple (one sensible workflow, no required fields, no SLAs, no permission
schemes) and turn governance **on incrementally** — adding workflow schemes, field schemes, permission schemes,
SLA policies — **without data migration** (Phase-2 §1.1). The fork PR-3 names: how much is *baked into the
schema* (always present, always enforced) vs *layered as optional configuration* (absent until opted into)?

Get it wrong two ways:
- **Too baked-in** → every issue carries Jira's full machinery from day one → slow, ugly, the configuration
  sprawl we exist to beat (deep-dive §0).
- **Too opt-in / too dynamic** → turning governance on later requires migrating existing data, or the schema
  can't express what a regulated enterprise needs → we fork into "Myelin Lite" and "Myelin Enterprise."

## What MUST be baked-in (non-negotiable invariants)

These cannot be opt-in because cross-cutting correctness depends on them platform-wide:

- **State *categories* are a fixed, closed set** (`unstarted / started / completed / cancelled`) over unlimited
  *named* states (Phase-2 §1.2; deep-dive §3.3). This is baked-in because **cross-project reporting, boards,
  burndown, and "is this done?" logic all read the category**, never the name. Jira's historical lack of this
  is the cited failure. An org renames/adds states freely; it can never invent a category. (HOUSE invariant
  with a PROVEN payoff: heterogeneous custom workflows still roll up.)
- **Every state change is recorded** (the change-log / version history) — baked-in because it is the audit +
  GDPR basis (deep-dive §3.9, §8). Not optional.
- **One human-key per issue** (TE-14) — baked-in (every issue is addressable).
- **`parent` containment + the lateral relation set** (`blocks/blocked_by/depends_on/relates/closes`) — the
  *vocabulary* is baked-in (frozen by REF-1/event-bus as the lifecycle `rel` set); which relations an org
  *uses* is emergent.
- **Permission is always ReBAC-enforced** — but the *default* is the inherited project-read (identity §5
  `issue.view = parent_project->read - confidential + confidential_grant`). "No permission scheme" doesn't mean
  "no permissions"; it means "the default inheritance, no field/transition overlays."

## Candidate A — Schema-on-read everything (maximally dynamic; Notion-leaning)

Every governance concept is a row in a config table interpreted at runtime: workflows, fields, permissions, SLAs
are all data; the `issue` table is a thin property bag. Nothing is "baked into" the issue shape.

- **For:** turning anything on is a config write, never a migration — the "no data migration" requirement is
  trivially met.
- **Against:** a *fully* schema-on-read issue is the JQL-performance-trap (TE-17, sketch 03) at its worst — even
  `state` and `assignee` are property-bag lookups, so every board query is a JSONB scan. And a fully-dynamic
  workflow interpreter on the hot transition path is slow.
- **Against:** loses the strong-typing the Rust state machine wants (Phase-2 §3 "strong types for the workflow
  state machine and the scheme algebra").

## Candidate B — Hardcoded tiers (Lite / Standard / Enterprise) — the explicit fork

Three product tiers with different baked-in schemas; "upgrade" migrates data.

- **Against (disqualifying):** this IS the fork PR-3 forbids. Upgrading from Lite to Enterprise is a migration;
  the startup that grows into a regulated enterprise (design-language P4 — "the same product") hits a wall.
  Rejected outright.

## Candidate C — Layered optional schemes over a typed core (the Phase-2 thesis, made concrete)

The `issue` table has a **typed core** (the always-present spine: id, key, type, state+category, priority,
assignee, reporter, timestamps, parent, project) as **first-class typed columns / generated columns** — these
are baked-in, fast, indexed. Everything beyond the core is a **flexible field in a JSONB property bag** (sketch
03). Governance is **layered scheme objects** *assigned* per (type × team/project), evaluated as configuration:

| Scheme | What it layers | Default (Linear-simple) | When opted in |
|---|---|---|---|
| **workflow scheme** | named states + transitions + guards + post-actions, mapped to the fixed categories | one 3-state default (`Todo→In Progress→Done` + `Cancelled`) | custom states, transition guards (CI-gate, approvals), required-fields-on-transition |
| **field scheme** | which flexible fields exist + scoping (global/team/type) + transition-scoped required-ness | none required; ad-hoc fields allowed | typed, validated, required fields for governance/reporting |
| **permission scheme** | the field/transition/browse/confidential overlays *on top of* the default ReBAC inheritance | default project-read inheritance | field-level, transition-level, confidential overlays (identity §5 + ABAC edges) |
| **SLA policy** | time targets + business calendar + pause/escalation | none | support/ITSM orgs (sketch 07) |
| **type scheme** | which types exist + their hierarchy rank + parenting rules (sketch 01) | the default ranked set | custom types, custom ranks |

**The "no data migration" guarantee comes from two design rules:**
1. **Schemes are interpreted, not compiled into the row.** An issue doesn't store "which workflow am I" as baked
   structure; it stores `state` (a name) + `category`. Assigning a new workflow scheme to its (type, project) is
   a config write; the issue's existing `state`/`category` remain valid (every workflow must include a mapping
   for the categories, so an existing `category=started` issue lands in the new workflow's started-state set).
2. **Adding a flexible field is a config write, never DDL** (sketch 03 — JSONB + derived projection, not a
   column per field). So "turn on a custom field" never migrates the table.

- **For:** Linear-simple is the *empty configuration* — no schemes assigned → the typed core + the one default
  workflow → fast, clean, keyboard-first. Jira-powerful is *more schemes assigned*. Same product, same tables,
  same code path. **No fork.**
- **For:** the hot path (board/list/transition) reads typed-core columns + the assigned-workflow interpreter
  (small, cached per (type,project)) — fast. The slow, flexible part (custom fields) is off the hot path (sketch
  03).
- **For:** matches the seeded ReBAC exactly — permission *overlays* (field/transition/confidential) are the
  identity §5 sub-object permissions + ABAC edges; "no permission scheme" = "no overlays, just inheritance."
- **For:** strong-typed in Rust where it matters (the core + the state-machine *interpreter*); data-driven where
  it must flex (the schemes as config the interpreter runs). This is Phase-2 §3's "data-driven interpreter, not
  codegen, so schemes are config."
- **Against / cost:** the scheme *algebra* (assignment resolution: which workflow applies to this (type, team,
  project)? precedence when a team override and a project default disagree?) is real complexity — must be
  designed carefully (a deterministic precedence: type×project-specific > type-default > project-default >
  org-default). Mitigated: it is *config resolution*, cached, off the per-write hot path.

## The workflow-engine representation (sub-decision)

Given Candidate C, how is the state machine represented? Two options:
- **Codegen per workflow** (a compiled Rust enum/FSM per org workflow) — rejected: schemes are user-authored at
  runtime; you cannot recompile the binary per tenant.
- **Data-driven interpreter** (Phase-2 §3 direction): a workflow is a `{states[], transitions[], guards[],
  post_actions[]}` config row; the engine is one interpreter that loads the assigned scheme and evaluates. Guards
  are expressed in the **shared safe query-AST `EventMatcher` core** (event-bus §4.5 / ADR-07) — so "guard: linked
  PR CI is green" and "guard: approver-role signed off" are AST predicates, *not* arbitrary code (no UDFs, no
  Jira-Groovy-scripting footgun). **Lean: data-driven interpreter, guards as bounded AST predicates.**

## Leaning

**Candidate C** — typed-core + layered optional schemes, schemes interpreted (data-driven) not baked, the fixed
state-*category* set as the one mandatory invariant, guards as safe-AST predicates. This is the only candidate
that delivers "Linear-fast by default, Jira-powerful on demand, no fork, no migration." It is the Phase-2 thesis
(§1.1) turned into a concrete storage+interpretation rule, and it composes cleanly with sketch 03's flexible-field
storage (the schemes that *aren't* the typed core live in the same JSONB the custom fields live in).

## Hands forward

- The scheme-assignment precedence algebra (deterministic resolution) — architecture.
- Exactly which core fields are typed columns vs generated columns vs JSONB (sketch 03 owns the storage line).
- The workflow-scheme editor UX (S13 — design/wireframes): a state-machine graph editor + AST guard builder +
  unreachable-state validation.
