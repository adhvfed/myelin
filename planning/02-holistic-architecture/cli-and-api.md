# Phase 2 — Platform CLI & API Conventions (the unifying surface)

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth; never contradicted). Phase-2 spine:
> [`architecture-decisions.md`](./architecture-decisions.md) (the ADR register) and
> [`system-overview.md`](./system-overview.md) (the holistic narrative). Phase-1 foundation:
> [`01-research/technical-structuring.md`](../01-research/technical-structuring.md) (§3 glue
> contracts, §3.1 `ArtifactRef`), [`01-research/agent-native-design.md`](../01-research/agent-native-design.md)
> (§4.2 the tool/skill surface, MCP exposure), [`01-research/use-cases.md`](../01-research/use-cases.md).

## 0. Scope & altitude

This document defines the **platform-wide CLI and API conventions** and the **cross-cutting
command/endpoint surface** — the unifying design that every subsystem's own CLI/API plugs into.
Individual subsystem commands (`myelin repo`, `myelin issue`, `myelin ci`, `myelin doc`,
`myelin chat`) are specified in their Phase-4 subsystem docs; **this doc is the grammar, the
conventions, the global flags, the output contracts, the cross-cutting verbs (auth, context,
search, refs, inbox, agents, admin/GDPR, audit), and the API/MCP strategy they all obey.**

It is a *direct projection of the spine*. There is no CLI/API decision here that isn't grounded
in an ADR: the `ArtifactRef` grammar (ADR-13) is the CLI's noun grammar; the event envelope
(ADR-13/ADR-04) is the webhook payload; the `Principal` + ReBAC engine (ADR-03/ADR-13)
authorizes every CLI/API/MCP call identically; the `ToolDef`/`ToolSurface` registry (ADR-08) is
the MCP-exposed tool catalogue; the cell/region topology (ADR-11) is how endpoints are routed;
the query AST (ADR-07) is the search/filter syntax; `PersonalDataHolder`/DSR (ADR-12) is the
admin GDPR surface.

Altitude is Phase 2: **conventions and the shape of the surface**, with illustrative
invocations. Concrete endpoint schemas, the full command tree, and wire details are Phase 3/4.
Open questions are tagged `[OPEN → Pn]`.

---

## 1. Design philosophy — one surface, three consumers

### 1.1 The thesis

Myelin has **one API surface** with **three first-class consumers — humans (Web UI/CLI),
scripts/CI, and agents (internal runtimes + external MCP)** — and they are the *same* surface,
not three parallel ones (`system-overview.md §2` layering; VISION §3 agent-native). The CLI is a
**thin, faithful mirror of the API**: every CLI command is a typed API call wearing a
human-friendly coat, and every API call is reachable from the CLI. This is a non-negotiable
because the platform is agent-native: an agent that can drive the CLI/API can do anything a human
can, *gated by the same `Principal` and the same ReBAC engine* (ADR-03, ADR-08) — there is no
"admin-only backdoor API" and no "CLI-only convenience" that escapes authz or audit.

Five principles, each tracing to a non-negotiable:

1. **Noun-verb, mirrors the `ArtifactRef` grammar (ADR-13).** The CLI's nouns *are* the
   subsystem/type axes of `myelin://<tenant>/<subsystem>/<type>/<id>`. `myelin issue create`,
   `myelin repo pr merge`, `myelin doc publish` — the command path reconstructs an `ArtifactRef`.
2. **Human-first *and* machine-first output (VISION §3).** Every command renders rich human
   output by default and exact, stable JSON under `--json`/`--output json`. The JSON is the
   *same envelope shape* the API returns. Agents and scripts never scrape human text.
3. **One identity, checked everywhere (ADR-13).** Every CLI/API/MCP call resolves to a
   `Principal` and is authorized by the one ReBAC engine; every call is audited (ADR-12).
4. **Context-aware, residency-aware (ADR-11).** The CLI carries a *tenant + region + project*
   context; calls route to the tenant's cell. Cross-region personal-data operations are
   impossible by construction — the CLI cannot even address another region's data for a tenant
   pinned elsewhere.
5. **GDPR/audit are first-class verbs, not hidden ops (ADR-12).** `myelin gdpr dsr`,
   `myelin audit query`, `myelin admin …` are part of the catalogue, governed and audited like
   everything else.

### 1.2 Why a CLI at all (and why it's the agent's body too)

The CLI is (a) the **ergonomic surface for engineers** (the persona who lives in a terminal —
`personas.md`), (b) the **scripting surface for CI and automation**, and (c) **the canonical
worked example of "the API is fully agent-drivable"**: the same typed operations the CLI exposes
are the `ToolDef`s in the `ToolSurface` (ADR-08 §4) and the MCP tools exposed to external agents.
Building the CLI well *is* building the agent tool catalogue well — they are the same operations
with different front-ends (one catalogue, many front-ends; `agent-native-design.md §4.2`).

---

## 2. CLI grammar & structure

### 2.1 The command shape

```
myelin [global-flags] <noun> [sub-noun …] <verb> [args] [flags]
```

- **`<noun>`** is a subsystem or a cross-cutting domain. Subsystem nouns mirror `ArtifactRef`
  subsystems: `repo` (git), `ci`, `issue`, `doc` (knowledge), `chat`. Cross-cutting nouns:
  `auth`, `context`, `search`, `ref`, `inbox`, `agent`, `trigger`, `admin`, `gdpr`, `audit`,
  `config`, `api` (raw escape hatch), `tool` (catalogue introspection).
- **`<sub-noun>`** nests where the artifact tree does, mirroring `#sub` granularity (ADR-13):
  `myelin repo pr …`, `myelin repo pr comment …`, `myelin issue field …`.
- **`<verb>`** is a small, predictable, *consistent-across-nouns* vocabulary:

| Verb | Meaning | Notes |
|---|---|---|
| `list` | enumerate (paginated, filterable via query AST) | always cursor-paginated (§4.3) |
| `get` / `show` | fetch one by id or `ArtifactRef` | `show` = rich human view; `get` = data |
| `create` | make one | returns the new `ArtifactRef` |
| `update` / `edit` | mutate fields | partial; honors field-level authz (ADR-03) |
| `delete` | remove (soft by default) | GDPR-aware; `--erase` routes to DSR (ADR-12) |
| `<domain verbs>` | subsystem-specific transitions | e.g. `pr merge`, `issue transition`, `ci trigger`, `doc publish` |

Every subsystem reuses `list/get/create/update/delete` identically; only domain verbs differ.
This consistency is what makes the surface learnable *and* lets an agent generalize across
subsystems from one tool's schema (`agent-native-design.md §4.2`).

### 2.2 Addressing: ids, `ArtifactRef`s, and shorthands

Anything that takes a target accepts **three equivalent forms** (ADR-13 §3.1):

- **Full `ArtifactRef`**: `myelin://acme-eu/issue/issue/ISSUE-412` (fully qualified, tenant +
  region-routable; what events and APIs carry).
- **Context-relative short id**: `ISSUE-412`, `pr/88`, `DOC-77#sec3` — resolved against the
  current context's tenant/project (§3).
- **`#sub` sub-artifact**: `pr/88#comment-12`, `DOC-77#block-9`, `RUN-991#step-3` — the same
  sub-granularity the Reference Graph and projection APIs resolve.

The CLI prints the canonical `ArtifactRef` for anything it creates, so output is always
re-addressable (and pipe-able into `myelin ref`, `myelin chat post`, etc.).

### 2.3 Global flags (apply to every command)

| Flag | Purpose | ADR |
|---|---|---|
| `--tenant <t>` / `--project <p>` | override the active context for this call | ADR-11 |
| `--region <r>` | assert region (safety: fails loudly on mismatch, never silently re-routes) | ADR-11 |
| `--output {human,json,yaml,table}` (alias `--json`) | output format; default `human` (TTY) / `json` (non-TTY) | VISION §3 |
| `--as <principal>` | act *as* (delegation/on-behalf-of); requires delegation grant, fully audited | ADR-08, ADR-12 |
| `--quiet` / `--verbose` / `--debug` | verbosity; `--debug` shows the underlying API call + correlation_id | |
| `--no-input` | never prompt; fail instead (for scripts/agents) | VISION §3 |
| `--profile <name>` | select a stored credential/context profile (§3) | |
| `--dry-run` | for mutating commands, return the *plan* (the effects) without applying — mirrors plan-then-apply (ADR-08) | ADR-08 |
| `--idempotency-key <k>` | dedupe a mutating call (maps to `event_id` dedup, ADR-04) | ADR-04 |
| `--wait` / `--watch` | block until an async op completes / stream live updates off the bus | ADR-04 |

`--dry-run` deserves emphasis: because the platform is plan-then-apply for agents (ADR-08), the
CLI exposes the *same* "show me the proposed effects" affordance to humans and scripts. A
mutating command with `--dry-run --json` returns the exact `effects[]` that would be applied —
the human and agent safety models are unified.

### 2.4 Output contract (human + machine)

- **Human (`--output human`, default on a TTY):** color, tables, unfurled `ArtifactRef`s
  (showing title/status, permission-filtered per the calling `Principal`), progress, paging.
- **Machine (`--json`, default off a TTY):** the **common response envelope** (§4.2) — the
  *identical* shape the REST/GraphQL API returns. Stable, versioned, never contains ANSI. Errors
  also serialize as the structured error envelope (§4.4), so scripts/agents branch on
  `error.code`, never on text.
- **`--output table`/`yaml`:** convenience projections for humans/ops.

Non-TTY defaulting to JSON (the "agent/CI default") means `myelin issue list | jq …` and an
agent calling the same command both get structured data with zero extra flags — the
machine-first default the agent-native mandate requires.

---

## 3. Auth, context & config (the CLI's state)

### 3.1 Authentication (ADR-03, ADR-13)

```
myelin auth login [--tenant acme-eu] [--sso] [--device]   # browser/device-code OIDC by default
myelin auth login --token $MYELIN_TOKEN                    # CI/agent: PAT or short-lived token
myelin auth status                                         # who am I, which principal, scopes, region
myelin auth logout
myelin auth token create --scope "issue:write repo:read" --ttl 1h   # mint scoped, short-lived token
```

- **Humans** authenticate via the org's SSO (OIDC; SSO/SCIM/MFA owned by Identity, ADR-14) →
  the CLI stores a short-lived, refreshable session bound to the active context.
- **CI/agents** use scoped, short-lived **tokens** (`MYELIN_TOKEN` env var or `--token`). Tokens
  carry a `Principal` (Human-acting-as / Service / Agent), scopes (compiled to ReBAC), a tenant,
  and a TTL. There is **no long-lived god token**; least-privilege per the agent-safety mandate
  (ADR-08 §6).
- Every authenticated identity is a **`Principal`** authorized by the **one** ReBAC engine
  (ADR-13). The CLI never makes a local authz decision; the server is authoritative.

### 3.2 Context (tenant / region / project) — ADR-11

The CLI keeps an **active context**: `(tenant, region, project)`. The tenant determines the cell;
the cell determines the region; the region is **immutable for that tenant** (ADR-11) — the CLI
surfaces it but cannot change a tenant's residency.

```
myelin context list                          # all contexts I can access
myelin context use acme-eu/platform-team      # switch tenant+project (region inferred from cell)
myelin context current                        # show active (tenant, region, project, principal)
myelin context use --project mobile-app        # switch project within current tenant
```

Switching context re-points the CLI at the right cell's endpoint (resolved via the global
control plane's tenant→cell directory, ADR-11). A command run against the wrong region with an
explicit `--region` **fails loudly** rather than silently crossing a residency boundary — the
CLI inherits ADR-11's "misrouting personal data is impossible, not discouraged."

### 3.3 Config & profiles

```
myelin config get|set <key> [value]          # e.g. defaults.output=json, defaults.project=...
myelin config list
```

- A **config file** (`~/.config/myelin/config.toml`) stores profiles (named `(endpoint,
  tenant, project, credential-ref)` bundles), defaults (e.g. always-JSON), and the cell endpoint
  cache. Credentials live in the OS keyring, not the config file.
- **Precedence (highest→lowest):** explicit flag → env var (`MYELIN_TENANT`, `MYELIN_TOKEN`,
  `MYELIN_OUTPUT`, …) → active profile → config defaults. This makes the CLI fully
  scriptable/agent-drivable: an agent sets env vars and never touches interactive state.

---

## 4. The API surface strategy

### 4.1 Position: REST-first JSON, GraphQL for read-aggregation, gRPC internal, git-wire native

Consistent with the spine (one API surface, many consumers; `system-overview.md §2`), Phase 2
commits the following **directional** API strategy (concrete schemas → P3/P4):

| Surface | Role | Rationale / ADR |
|---|---|---|
| **REST/JSON over HTTPS** | The **primary public surface** — resource-oriented, mirrors the CLI noun-verb grammar 1:1, mirrors `ArtifactRef` addressing | Simplest-correct, universally consumable by scripts/agents/webhooks; the CLI is a thin client over it (§1.1). [DECIDED — direction] |
| **GraphQL** | The **read-aggregation surface** for the Web UI's composite views (e.g. the PR context pane, `system-overview.md §8.1`) — fetch a PR + its refs + projections in one round-trip, each permission-filtered | The UI's cross-subsystem panes are aggregation-heavy; GraphQL avoids N round-trips. **Mutations stay REST** (clearer authz/audit/idempotency boundary). [DECIDED — direction; surface boundary → P4 design] |
| **gRPC** | **Internal subsystem↔shared-system** calls (projection APIs, `list-objects` authz filter, ref resolution) and high-throughput paths | Typed, fast, codegen across the Rust workspace (ADR-01/02). Not a public surface. [DECIDED — direction] |
| **Git wire (SSH/HTTPS)** | Native git transport for clone/fetch/push | A first-class client of the platform, authorized by the same `Principal` (ADR-13); owned by Git subsystem (P4). |
| **MCP** | The **agent/tool surface** — the `ToolSurface` registry exposed as MCP tools (§6) | One catalogue, two front-ends (ADR-08 §4.2). |
| **Webhooks** | Outbound event delivery to external systems | The event envelope on the wire (§4.5). |

The **CLI ↔ REST mirror is the contract**: a command and its REST endpoint share a name, a
verb→method mapping (`list`→GET collection, `get`→GET item, `create`→POST, `update`→PATCH,
`delete`→DELETE, domain verbs→POST action sub-resources), and the *same envelope*. This is why
`--debug` can print the exact equivalent `curl`.

> **[OPEN → P4]** The exact REST/GraphQL boundary (which composite reads are GraphQL-only) and
> whether GraphQL is also offered publicly or UI-internal-only. The CLI does **not** depend on
> GraphQL — it is REST-only — so this stays a UI/Phase-4 decision.

### 4.2 The common response envelope & `ArtifactRef`

Every API/CLI-JSON response shares one envelope, so a script/agent parses *one* shape across all
five subsystems (the unification payoff at the API layer):

```jsonc
{
  "data": { /* the resource, or array for collections */ },
  "ref": "myelin://acme-eu/issue/issue/ISSUE-412",   // canonical ArtifactRef of the primary resource (ADR-13)
  "meta": {
    "request_id": "...",            // correlates to audit + logs
    "correlation_id": "...",        // ties into the event causation/correlation chain (ADR-04/13)
    "tenant": "acme-eu",
    "region": "eu-west",            // residency, surfaced explicitly (ADR-11)
    "api_version": "2026-06-01",
    "page": { "next_cursor": "...", "has_more": true }   // present on collections
  },
  "effects": [ /* present on --dry-run / plan-then-apply mutations (ADR-08) */ ],
  "warnings": [ /* non-fatal, e.g. "field X hidden by permission" */ ]
}
```

- **`ref`** is the load-bearing field: every resource carries its canonical `ArtifactRef`
  (ADR-13), so any response can be piped into `myelin ref`, `myelin chat post`, an embed, or an
  agent tool call without re-deriving addresses. Embedded `ArtifactRef`s inside `data` (mentions,
  relations, `embed` nodes from the shared content model, ADR-05) are first-class and resolvable.
- **`meta.region`** makes residency *visible* on every response (ADR-11/12).
- **`effects`** unifies the human/agent safety model: a `--dry-run` mutation and an agent's
  proposed `AgentDecision` carry the same effect list shape (ADR-08).

### 4.3 Pagination, filtering, sorting (the query AST surface)

- **Cursor-based pagination everywhere** (`next_cursor` in `meta.page`) — never offset, so it's
  stable under writes at world scale (ADR-11). `list` commands accept `--limit`, `--cursor`,
  `--all` (auto-follow cursors).
- **Filtering/sorting is the shared query AST (ADR-07).** A `list`/`search` accepts a
  human-friendly filter expression *and* a raw AST (`--filter-ast @file.json`) so agents emit the
  exact same validated AST the UI's saved views emit. The AST is **permission-aware by
  construction** (ADR-07/ADR-03): a `list`/`search` can *never* return artifacts the caller can't
  see, because it composes with the authz `list-objects` filter server-side — no post-filtering,
  no leak (`system-overview.md §5.2`).

```
myelin issue list --filter 'status != done AND assignee = @me AND label in (bug, p0)' --sort -updated
myelin search 'auth refactor' --type issue,doc,pr --filter-ast @saved-view.json --json
```

### 4.4 Errors

One structured error envelope; scripts/agents branch on `error.code`, never on prose:

```jsonc
{ "error": {
    "code": "permission_denied",          // stable, machine-branchable
    "message": "You cannot transition ISSUE-412 to 'done'.",
    "target": "myelin://acme-eu/issue/issue/ISSUE-412",
    "request_id": "...", "correlation_id": "...",
    "retryable": false,
    "details": [ /* e.g. which permission, which field */ ]
}}
```

Canonical codes (cross-cutting): `unauthenticated`, `permission_denied` (ReBAC, ADR-03),
`not_found` (or permission-masked-as-not-found, to avoid existence leaks — SC-1),
`validation_failed`, `conflict`, `rate_limited`/`budget_exceeded` (agent governance, ADR-08),
`region_mismatch` (ADR-11), `gdpr_restricted` (data under restriction, ADR-12). CLI exit codes
map to code classes so shell scripts branch cleanly.

### 4.5 Webhooks (the event envelope on the wire)

Outbound webhooks **carry the canonical event envelope** (ADR-13 §2 / ADR-04) — the *same*
envelope consumers see internally, so an external integration and an internal trigger consume
identical events:

```
myelin trigger webhook create --on 'ci.pipeline.failed AND repo = acme/api' \
  --url https://example.com/hook --secret-ref kms://...
```

- **References-not-payloads (ADR-04 §4 / ADR-12):** webhook payloads carry `ArtifactRef`s +
  envelope metadata, **not** personal-data bodies; the receiver resolves refs via the API
  (permission-checked), keeping the GDPR surface minimal. `contains_personal_data` and
  `visibility` envelope flags are honored.
- **At-least-once + `event_id` (ADR-04):** webhook delivery is at-least-once; receivers dedupe on
  `event_id`, and deliveries are signed. The matcher is the query-AST `EventMatcher` (ADR-07).
- **[OPEN → P3]** exact event taxonomy (dotted names) and the `EventMatcher` predicate dialect.

### 4.6 Versioning

- **Date-based API versioning** (`api_version: "2026-06-01"`, sent via header or
  `myelin config set api.version`) for the public REST surface, with a deprecation window —
  agents and scripts pin a version and aren't broken by evolution.
- **The envelope/`ArtifactRef`/event-envelope contracts are versioned independently** with
  `schema_ver` (ADR-13 §2) and evolve additively; a breaking change is a single workspace-wide PR
  (ADR-01), surfaced to every consumer at build time, not silently in prod.
- **The CLI** carries its own semver but negotiates the API version it targets; `myelin version`
  prints both.

---

## 5. The cross-cutting command surface

These are the **platform-level** verbs that exist *because of the shared systems* — they are the
CLI face of Identity, Refs, Search, Notifications, the Agent Fabric, and GDPR/Audit. Subsystem
verbs (`repo`, `issue`, `ci`, `doc`, `chat`) live in Phase-4 docs.

### 5.1 `auth` / `context` — covered in §3.

### 5.2 `search` — the unified, permission-aware search (ADR-03, ADR-07, Search shared system)

```
myelin search 'flaky test main' --type pr,issue,run --in acme/api --json
myelin search --semantic 'how do we rotate KMS keys' --type doc        # vector/semantic (ADR-14)
myelin search --filter-ast @view.json --all
```

One command spans all subsystems; results are **pre-filtered by `list-objects`** (ADR-03) — a
user "never finds what they cannot access" (SC-1). Supports full-text, structured (query AST),
and semantic/vector modes (ADR-14). Output rows carry `ArtifactRef`s, pipe-able onward.

### 5.3 `ref` — the Reference Graph (ADR-13, Reference Graph shared system)

```
myelin ref show ISSUE-412                  # what this artifact references + backlinks (permission-filtered)
myelin ref backlinks DOC-77#sec3           # everything pointing at this doc section
myelin ref create --from ISSUE-412 --to pr/88 --kind implements   # explicit edge (usually emitted from content)
myelin ref graph PR-88 --depth 2 --json    # neighborhood, for agents/visualization
```

Edges are normally **emitted from content** (mention/`artifact_ref` nodes, ADR-05) via
`ref.created` events; `ref create` is the explicit form. Backlinks are **permission-filtered at
read time** (ADR-13); tombstoned targets degrade gracefully (erasure-aware, ADR-12).

### 5.4 `inbox` — Notifications (ADR-12, Notifications shared system)

```
myelin inbox list --unread --json          # the one prioritized "what needs ME" feed across all subsystems
myelin inbox show <notif-id>
myelin inbox read <notif-id> | myelin inbox read --all
myelin inbox snooze <notif-id> --until tomorrow
myelin inbox watch                         # stream new notifications live (off the bus, --watch)
myelin inbox prefs set --channel email --on 'mention OR review_requested'
```

One cross-subsystem inbox (ADR-12) with storm-control/dedup; the `watch` form streams via the
durable bus. Notification prefs are themselves a query-AST `EventMatcher` (ADR-07).

### 5.5 `agent` / `trigger` / `tool` — the Agent Fabric surface (ADR-08, ADR-09)

This is where the agent-native mandate surfaces in the CLI. It manages **agent identities,
triggers (the one trigger/automation/agent engine, ADR-08 §5), and the tool catalogue.**

```
# Agent identities (first-class Principals, kind=Agent — ADR-08)
myelin agent list
myelin agent show triage-bot                       # runtime_ref, delegation policy, budgets, audit trail
myelin agent create triage-bot --runtime mock --delegation @policy.json --budget '50 effects/run'
myelin agent set triage-bot --runtime mock|llm     # the strategy-pattern swap is a config change (ADR-08 §4)!

# Triggers (EventMatcher → target under run_as principal + RunBudget + DelegationPolicy + HITL gates)
myelin trigger list
myelin trigger create --on 'ci.pipeline.failed AND branch = main' \
   --run-as triage-bot --budget '50 effects/run' --gate 'open_pr:human-approve' \
   --action agent:triage-bot
myelin trigger create --on 'issue.transitioned to=in_review' --action automation:notify-reviewers   # automation, same engine
myelin trigger disable <id>
myelin trigger runs <id> --json                    # run history: effects proposed/applied, gates, audit

# Tool catalogue (the ToolSurface — one catalogue, two front-ends; ADR-08 §4.2)
myelin tool list [--for triage-bot]                # tools this principal may call (permission-scoped)
myelin tool show issue.create                      # name + JSON-schema input + required caps + effect kind + side-effecting flag
myelin tool describe --mcp                          # emit the catalogue as an MCP tool manifest (§6)
```

Three things this makes concrete from the spine:

- **The mock→real swap is `myelin agent set ... --runtime`** — a config/implementation swap, not
  a rewrite (ADR-08 §4; VISION §3). The CLI proves the strategy-pattern boundary is real.
- **Automations and agents are one engine** (ADR-08 §5): `myelin trigger` creates
  subscriptions, durable automations, *and* agent triggers — same command, different `--action`.
- **Plan-then-apply is visible:** `myelin trigger runs` shows the *proposed* effects, which were
  HITL-gated, and which were applied — the same `effects[]` shape as `--dry-run` (ADR-08).
  HITL gates surface as Chat approval cards (ADR-09; `system-overview.md §8.2`), and can also be
  resolved from the CLI: `myelin agent approve <run-id>` / `myelin agent reject <run-id>`.

### 5.6 `admin` / `gdpr` / `audit` — governance & compliance (ADR-12, ADR-11)

```
# Admin (tenant/org/membership/SSO — Identity, ADR-03/11)
myelin admin org show
myelin admin member add alice@acme.eu --role engineer --team platform
myelin admin tenant residency                        # show region binding (immutable, ADR-11)

# GDPR / DSR (the PersonalDataHolder/DSR spine — ADR-12; operable by Myelin AND by tenants, Art. 28)
myelin gdpr dsr create --subject alice@acme.eu --type access     # locate/export across ALL holders
myelin gdpr dsr create --subject alice@acme.eu --type erase      # crypto-shred fan-out + tombstone (ADR-12)
myelin gdpr dsr status <dsr-id>                                   # deadline tracked, per-holder progress
myelin gdpr dsr receipt <dsr-id> --json                          # verifiable deletion/export receipt
myelin gdpr datamap export                                       # the GENERATED data-map/RoPA (ADR-12 §6)
myelin gdpr consent show --subject ...                           # consent/lawful-basis registry

# Audit (the one tamper-evident log of every human AND agent action — ADR-12 §9)
myelin audit query --actor triage-bot --since 24h --json
myelin audit query --subject ISSUE-412 --correlation <id>        # full provenance of a workflow
myelin audit verify <range>                                       # tamper-evidence check
```

- **`gdpr dsr`** fans the request to **every** `PersonalDataHolder` — all 5 subsystems + search +
  refs + bus history + agent memory + backups (ADR-12 §1) — producing one inventory + a
  verifiable receipt; `erase` uses crypto-shred + tombstoning (`system-overview.md §8.3`). It is
  operable **by tenants for their own data subjects** (Art. 28 assistance, ADR-12 §2).
- **`audit query`** spans humans *and* agents uniformly (agents are Principals; every effect is
  attributed — ADR-08/ADR-12). `--correlation` reconstructs an entire multi-subsystem workflow.
- **`datamap export`** emits the *generated* (not hand-curated) classification registry (ADR-12
  §6) — drift-proof RoPA/DPIA input.

### 5.7 `api` and `tool` as escape hatches

```
myelin api GET /v1/issues/ISSUE-412 --json           # raw API call, auth/context applied — for new endpoints
myelin api POST /v1/repos/acme/api/pulls --data @pr.json
```

`myelin api` is the raw, authenticated, context-aware passthrough so anything the API can do is
CLI-reachable even before a typed command exists — guaranteeing the "CLI mirrors API" invariant
holds *by construction*, not by hand-maintaining parity.

---

## 6. MCP — the agent/tool exposure path (ADR-08 §4.2)

The platform's tool catalogue (`ToolSurface`) is **defined once and exposed two ways** (ADR-08 §4;
`agent-native-design.md §4.2`):

1. **Internally** to our own runtimes (`MockAgentRuntime` now, `LlmAgentRuntime` later) — agents
   call typed `ToolDef`s, the `EffectApi` validates each against permissions ∩ delegation ∩
   tenant policy ∩ budget ∩ HITL gates, then applies (plan-then-apply, ADR-08 §3).
2. **Over MCP** to external/third-party agents later — the *same* registry, projected as MCP
   tools (`name` + JSON-schema input + description + `invoke`), **governed by the same ReBAC and
   the same effect-validation pipeline** (ADR-03/ADR-08). An external MCP agent is just another
   `Principal` (kind `Agent`) with a delegation policy and a budget — no privileged side door.

```
myelin tool describe --mcp > manifest.json     # the MCP tool manifest for the calling principal's catalogue
myelin mcp serve --as triage-bot               # (later) expose this principal's permitted ToolSurface over MCP
```

Key properties (all inherited from the spine, not new decisions):

- **One catalogue → CLI commands, REST endpoints, internal `ToolDef`s, and MCP tools are the same
  operations.** Building a good typed CLI *is* building the agent tool catalogue.
- **Every tool carries its required capabilities and a side-effecting flag** (ADR-08 §4), so a
  non-side-effecting tool (read/search) and a consequential one (open PR, transition issue) are
  distinguishable, and consequential actions are **suggest-by-default / human-confirm** (ADR-08
  §6; GDPR Art. 22 + AI Act).
- **[OPEN → P3]** Exact MCP wire conformance — Phase 1 flagged confidence in the *shape* but not
  verified wire specifics (`agent-native-design.md §4.2`, §6 #6). Carried forward; the
  registry-and-governance design does not depend on the exact wire.

---

## 7. End-to-end usage examples (crossing subsystems from the CLI)

Each example exercises multiple subsystems + shared systems, all through the one surface, all
under one `Principal`, all audited.

### 7.1 Open a PR, link an issue, trigger CI, post to chat — the developer flow

```bash
# context: I'm on acme-eu / platform-team, region eu-west (immutable, ADR-11)
myelin context use acme-eu/platform-team

# 1. open a PR (Git) — returns its ArtifactRef
PR=$(myelin repo pr create --repo acme/api --head fix/login --base main \
      --title "Fix login race" --json | jq -r .ref)
# => myelin://acme-eu/git/pr/88

# 2. link it to the issue it fixes (Reference Graph, ADR-13) — emitted as ref.created
myelin ref create --from ISSUE-412 --to "$PR" --kind fixes

# 3. CI auto-triggers on the push (ADR-04 event); or trigger explicitly and wait:
RUN=$(myelin ci trigger --repo acme/api --ref fix/login --json | jq -r .ref)
myelin ci status "$RUN" --wait                       # blocks on the run via the bus (--wait)

# 4. announce in chat with live unfurls of all three artifacts (shared content model, ADR-05)
myelin chat post --channel "#platform" \
  --text "Opened $PR for ISSUE-412, CI: $RUN — review please @alice"
```

**Shared systems exercised:** Identity (one `Principal` authorizes every step), Event Bus (PR
push → CI trigger; `--wait` streams run status), Reference Graph (the `fixes` edge + the chat
unfurls), Search (the artifacts become findable), Notifications (the `@alice` mention →
@alice's inbox), Storage (PR/run state). No subsystem touched another's DB — `myelin chat post`'s
unfurls resolve via each subsystem's projection API per the calling viewer (`system-overview.md
§8.1`). This is the §8.1 PR-context-pane wedge, driven from the terminal.

### 7.2 The agent-native flagship, set up and observed from the CLI (ADR-08, `system-overview.md §8.2`)

```bash
# 1. create the mock triage agent (a first-class Principal) and a trigger
myelin agent create triage-bot --runtime mock \
  --delegation @triage-delegation.json --budget '50 effects/run'
myelin trigger create --on 'ci.pipeline.failed AND branch = main' \
  --run-as triage-bot --action agent:triage-bot \
  --gate 'open_pr:human-approve'              # consequential action is HITL-gated (ADR-08 §6)

# 2. CI goes red on main → the agent wakes, PROPOSES effects (issue.create, ref.create×2, chat.post)
#    the EffectApi validates & applies the safe ones; the PR-open is gated.
myelin trigger runs --run-as triage-bot --json
# => shows: effects proposed, which applied, the HITL gate "open_pr" PENDING

# 3. approve the gated PR-open from the CLI (or from the Chat approval card — ADR-09)
myelin agent approve <run-id>                  # durable-workflow signal; PR #88 opens

# 4. full provenance — one correlation_id across CI→issue→refs→chat→PR
myelin audit query --correlation <id> --json

# 5. LATER: swap mock → real with zero platform change (the strategy-pattern payoff, ADR-08 §4)
myelin agent set triage-bot --runtime llm
```

**Shared systems exercised:** Bus (trigger), Agent Fabric (plan-then-apply), Identity (effect
validation + delegation), Refs (edges), Durable-workflow (the HITL gate that can wait days,
ADR-09), Chat/Notifications (approval card), Audit (provenance). The mock and real runtimes run
**identical platform code** — the CLI's `--runtime` flag *is* the swap (ADR-08).

### 7.3 A DSAR fan-out for a departed contributor (ADR-12, `system-overview.md §8.3`)

```bash
# tenant DPO runs this for their own data subject (Art. 28 assistance, ADR-12 §2)
DSR=$(myelin gdpr dsr create --subject bob@former.eu --type access --json | jq -r .data.id)
myelin gdpr dsr status "$DSR"          # per-holder progress: git, issues, knowledge, chat, ci,
                                       #   search, refs, bus-history, agent-memory — ALL holders (ADR-12 §1)
myelin gdpr dsr receipt "$DSR" --json  # the assembled "everywhere this subject appears" inventory

myelin gdpr dsr create --subject bob@former.eu --type erase   # crypto-shred + tombstone fan-out
```

**Shared systems exercised:** GDPR/Audit (DSR orchestrator + KMS crypto-shred), Identity (subject
→ pseudonym resolution), and **every** subsystem + Search + Refs + Bus history + Agent memory as
`PersonalDataHolder`s (ADR-12). Erasure-vs-immutability is minimized by construction (pseudonyms +
references-not-payloads + crypto-shred — `system-overview.md §8.3`). The same shared layer that
powers the wedge (7.1) powers compliance.

---

## 8. How the CLI/API embody the agent-native mandate (summary)

| Property | Mechanism | ADR |
|---|---|---|
| Same surface for humans, scripts, agents | CLI mirrors REST; non-TTY defaults to JSON; one envelope | §1.1, §4.2 |
| Structured, stable, parse-once output | common envelope + structured errors; branch on `error.code`, never text | §4.2/4.4 |
| Agents act exactly like humans, gated identically | one `Principal`, one ReBAC engine on every call | ADR-03/13 |
| Same tool catalogue everywhere | `ToolSurface` → CLI commands = REST endpoints = `ToolDef`s = MCP tools | ADR-08 §4.2, §6 |
| Plan-then-apply is a first-class affordance | `--dry-run` / `effects[]` for humans; `EffectApi` for agents — same shape | ADR-08, §2.3/4.2 |
| Consequential actions are safe-by-default | `--gate`/HITL, side-effecting flag, budgets, idempotency-key | ADR-08 §6, §5.5 |
| Fully scriptable (no interactive state needed) | env-var/flag precedence, `--no-input`, scoped tokens | §3.3 |
| Residency-safe by construction | context routes to the tenant's cell; `region_mismatch` fails loudly | ADR-11 |
| Everything audited | every CLI/API/MCP call is a recorded action of a Principal | ADR-12 §9 |

---

## 9. Open questions carried forward

| # | Question | Resolver |
|---|---|---|
| CA-1 | Exact REST↔GraphQL boundary; is GraphQL public or UI-internal-only? CLI stays REST-only regardless. | [OPEN → P4] |
| CA-2 | The canonical event taxonomy (dotted names) the webhook/`trigger --on` surface filters on. | [OPEN → P3] (ADR-13) |
| CA-3 | The `EventMatcher`/filter predicate dialect exposed in `--filter`/`--on` (CEL/JSONLogic/custom) and its human↔AST renderer. | [OPEN → P3] (ADR-07, AG-7) |
| CA-4 | MCP wire-spec conformance (shape confident; wire specifics unverified). | [OPEN → P3] (ADR-08) |
| CA-5 | Token/credential model details: PAT vs short-lived OIDC-exchanged tokens, scope→ReBAC compilation, `--as` delegation algebra. | [OPEN → P3] (ADR-03 AG-2) |
| CA-6 | Endpoint/cell discovery for the CLI when a tenant spans cells (multi-cell tenants — the deepest open item). | [OPEN → P3] (ADR-11 SC-2/3) |
| CA-7 | Where a domain verb belongs (subsystem CLI noun vs cross-cutting) for seams like Git↔CI checks and Issues↔Knowledge databases. | [OPEN → P4] (per subsystem doc) |
| CA-8 | Streaming/long-poll mechanics for `--watch`/`inbox watch`/`ci status --wait` over the bus + firehose split. | [OPEN → P3] (ADR-04) |

## 10. Cross-references

- [`architecture-decisions.md`](./architecture-decisions.md) — ADR-03 (Principal/ReBAC), ADR-04
  (events/webhooks), ADR-05 (content/unfurls), ADR-07 (query AST = filter syntax), ADR-08
  (agent fabric/tools/MCP/plan-then-apply), ADR-09 (HITL), ADR-11 (cells/residency/context),
  ADR-12 (GDPR/DSR/audit verbs), ADR-13 (`ArtifactRef` = noun grammar + envelope = webhook).
- [`system-overview.md`](./system-overview.md) — §2 (one API surface), §8.1/8.2/8.3 (the
  walkthroughs §7 here drives from the CLI).
- [`01-research/agent-native-design.md`](../01-research/agent-native-design.md) — §4.2 (the
  tool/skill surface and MCP exposure this CLI/API projects).
- [`01-research/technical-structuring.md`](../01-research/technical-structuring.md) — §3.1
  (`ArtifactRef` grammar), §3 (glue contracts).
- **Subsystem CLI sections** (Phase 4): `repo`, `ci`, `issue`, `doc`, `chat` verbs are defined in
  each subsystem's architecture doc, obeying *these* conventions.
