# Phase 3 — Identity & Access (ReBAC / Zanzibar-style)

> Phase: `03-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md). Doctrine
> bound: [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md)
> §2/§10, [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1.
> Directives bound: [`integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
> Phase-3 Identity (ID-1…ID-4), GD-3, X-1…X-5. Spine bound: ADR-03, ADR-17 (also ADR-08, ADR-11,
> ADR-12, ADR-13, ADR-16). Resolves **AG-1** (one principal kind or two) and **AG-2** (delegation
> algebra). Springboard: [`shared-systems-overview.md`](../02-holistic-architecture/shared-systems-overview.md) §1.
>
> **Status convention.** *DECIDED* = committed for Phase 4/5; *FLOOR* = a partial answer shipped with a
> named follow-on; *[OPEN → P4/P5/LEGAL]* = handed forward. Every capability that can fail names the
> **drill** that proves it (Phase 5 owns execution; this doc enumerates the obligation).
>
> **Prior art this doc builds on (cited inline):** Zanzibar (Pang et al., *Google's Consistent, Global
> Authorization System*, USENIX ATC 2019) for the tuple/check/expand/zookie core; **SpiceDB**
> (authzed.com) and **OpenFGA** (CNCF, OpenFGA modeling language) as the open, EU-self-hostable
> implementations; **Leopard** (Zanzibar §3.2.1) for the set-flattened index behind `list-objects`;
> OAuth 2.0 (RFC 6749), OAuth 2.1, OIDC Core 1.0, SAML 2.0, SCIM 2.0 (RFC 7642/7643/7644), WebAuthn
> Level 2 / FIDO2, PASETO/JWT (RFC 7519) with **biscuit**-style attenuable tokens (Geoffroy Couprie et
> al.) for capability tokens, JWKS (RFC 7517), DPoP (RFC 9449), the **macaroon** caveat model (Birgisson
> et al., Google, NDSS 2014) for delegation attenuation, and **NIST SP 800-162** (ABAC) for the edge
> predicates. Caching/staleness leans on Zanzibar §4 (Spanner snapshot reads + zookie) and the
> bounded-staleness pattern (ADR-17).

---

## 1. Purpose & responsibilities

Identity & Access (`Id`) is the **dependency root of the platform** (EI-02 §3: "identity depends on
nothing"). It answers exactly two questions for every other system, and owns the data behind them:

1. **Who is this?** — `authenticate(credential) → Principal`. Resolve any entrypoint's credential
   (SSO session, passkey, SSH key, API/agent/CI token) to one polymorphic `Principal` carrying its
   `tenant` and `region`.
2. **May this principal do this?** — `check(subject, permission, object) → Decision` and its
   set-valued siblings `list_objects` / `list_subjects`. One **ReBAC (Zanzibar-style) engine**
   evaluates humans, agents, and services *identically* (ADR-03, EI-02 §2).

It owns, end to end:

- **The Principal model** — `Human | Agent | Service` as one polymorphic record (AG-1 resolved in §3),
  plus agent delegation/on-behalf-of (AG-2 resolved in §7) and the **pseudonym-indirection table**
  that is the lever for git/bus/audit erasure (ADR-12.4; EI-04 §1).
- **Authentication surfaces** — SSO (SAML 2.0 / OIDC), SCIM 2.0 provisioning/deprovisioning, MFA &
  passkeys (WebAuthn/FIDO2), SSH keys for the git wire, scoped API tokens, short-lived CI job tokens,
  and **per-run auto-revoked agent tokens** (ID-2). (§4.)
- **The authorization hierarchy** — `org → team → project → resource → artifact → sub-artifact`,
  expressed as relationship tuples with inheritance (§5–§6).
- **The ReBAC tuple store** — schema, per-subsystem namespaces/relations, the `check` / `list_objects`
  / `list_subjects` algorithms, **consistency tokens (zookies)** for read-your-writes, and the cache
  hierarchy that makes them fast and `list_objects` the leak-free pre-filter Search/Refs consume (§8).
- **RBAC-as-authoring-face compiled to tuples**, with **ABAC predicates at the edges** (§9).
- **The fail-static availability cache** (ADR-17, ID-1): bounded-staleness "actor active / coarse
  grants" so an Id-dependency hiccup degrades, not cascades (§10).
- **Principal lifecycle** — onboarding/offboarding, ownership transfer, break-glass, tenant
  decommission, and the **disabled-user → zero-access-in-N-min** revocation guarantee (§11, §13).

**What Id is NOT.** It is not a session store for subsystem UI state, not the audit log (it *emits to*
the tamper-evident audit log via the outbox; GDPR/Audit owns it, ADR-12.9), and it does not store
artifact content — only tuples *about* artifacts whose IDs are minted by their owning subsystem. **No
subsystem reads Id's stores; everything goes through the contracts in §12** (ADR-01, ADR-13).

**Three platform invariants Id inherits and must never break:** residency-pinned + per-tenant
envelope-encrypted + crypto-shred-capable + `PersonalDataHolder` on every store (ADR-11/12);
tenant+region in the partition key of every table, tuple, cache key, and queue (EI-02 §1; ID-3); the
transactional **outbox is the only emit path** (EI-02 §4).

---

## 2. The store map (stateful-component register — X-4)

Id is two cooperating planes behind one stateless gateway-fronted service (EI-02 §9; X-2). Per X-4,
every stateful component is named with a shared-state/sharding/blast-radius note:

| # | Component | Engine (ADR-14) | Holds | Shard key | Blast radius | Crypto-shred unit |
|---|---|---|---|---|---|---|
| S1 | **Principal/Auth DB** | Postgres-class | principals, orgs/teams/projects, credentials, tokens, SSO/SCIM links, agent-identity records | `(tenant, region)` | one tenant (RLS) | per-tenant DEK; per-subject sub-key for profile PII |
| S2 | **Pseudonym map** | Postgres-class (own schema, tightest RLS) | `real_identity ↔ per-tenant pseudonym`; the **erasure lever** | `(tenant, region)` | one tenant | **per-subject key** (shred = erase the person from immutable history) |
| S3 | **ReBAC tuple store** | SpiceDB-class (Zanzibar) | `object#relation@subject` tuples; the hot authz path | `(tenant, region)` + object-id hash | one tenant; one shard of one tenant | per-tenant DEK |
| S4 | **`list_objects` index (Leopard-class)** | flattened set index (own store or SpiceDB internal) | denormalized reachable-set index for fast set queries | `(tenant, region)` + type | derived — rebuildable from S3 | derived (inherits S3) |
| S5 | **Authz read-replica** (ID-4) | Postgres / tuple-store replica | the authn/authz **hot-path replica** (first real replica need, EI-02 §8) | follows S1/S3 | read-only; stale-tolerant | inherits |
| S6 | **Fail-static cache** (ADR-17) | Redis/Valkey-class (NEVER source of truth — STOR-3) | bounded-staleness `{actor_active, coarse_grants}` snapshots + decision cache | `(tenant, region, subject)` | one cell; staleness-bounded | ephemeral; key TTL ≤ revocation SLA |
| S7 | **Revocation list / token denylist** | Redis/Valkey + PG mirror | revoked `jti`s, suspended principals, per-run agent token TTLs | `(tenant, region)` | one cell | ephemeral |

**Derived stores (S4, S6) are reindex-from-source primitives** (SEARCH-1 analogue, EI-04 §5.3): both
are rebuildable by replaying S3 / S1 through the live consumer path — no bespoke recovery code. S4's
rebuild is the standard Leopard incremental-from-watch build (Zanzibar §3.2.1). Everything else is the
system of record and is gated by the restore-verification drill (ADR-18; STOR-4).

---

## 3. The Principal model — AG-1 resolved: **one kind, three faces**

### Decision (resolves AG-1)
**One polymorphic `Principal` record with a `kind` discriminant `Human | Agent | Service` — not two
parallel models, not a `Service`/`Agent` merge.** This is the literal embodiment of EI-02 §2 ("an
agent is a principal with `kind = agent`, **not a special case in the permission code**") and ADR-08.1.
The discriminant changes *governance metadata and credential type*, never the authorization code path:
`check(subject, …)` does not branch on `kind`.

**Why one kind, not "Service==Agent" and not "Agent special":** the three faces differ on three axes
only, and those axes are *data*, not *code paths*:

| Axis | Human | Service | Agent |
|---|---|---|---|
| Credential type | passkey / SSO session / SSH key / PAT | long-lived scoped token / mTLS / SSH deploy key | **per-run, short-lived, auto-revoked** token (ID-2) |
| Lifecycle | SCIM/HR-driven; durable | admin-created; durable | **dispatch-minted, teardown-revoked; ephemeral** |
| Governance | MFA, AI-Act N/A | rotation policy, scope ceiling | **owner + on_behalf_of + RunBudget + DelegationPolicy + AI-Act labelling** (ADR-08.6) |
| Authz path | identical | identical | identical |

A `Service` is a durable non-human automation identity (a webhook poster, a deploy bot, the CI control
plane); an `Agent` is a `Service` that additionally carries the agent-governance envelope (owner,
delegation, budget, runtime_ref) and is **minted per run**. Modeling them as one `kind`-tagged record
with an optional `agent_governance` sub-record (null for Service) keeps the permission code uniform
while keeping the governance fields type-safe and non-optional *where they apply*. We reject two
separate tables because that is exactly the "parallel agent-permission system that diverges" failure
EI-02 §2 names; we reject collapsing Agent into Service because the agent governance fields
(per-run TTL, delegation intersection, AI-Act label) are mandatory for `kind=agent` and meaningless
for `kind=service`, and the discriminant is what makes that a *compile-time* guarantee.

### Schema (S1, illustrative)

```sql
-- Every table: (tenant, region) first, RLS-enforced, no cross-tenant query path (EI-02 §1; ID-3).
CREATE TABLE principal (
  tenant            uuid        NOT NULL,
  region            text        NOT NULL,
  principal_id      uuid        NOT NULL,           -- stable, opaque, attribution-safe (EI-04 §1)
  kind              principal_kind NOT NULL,        -- 'human' | 'agent' | 'service'
  display_handle    text        NOT NULL,           -- @alice / ~deploybot / #agent-triage (render-time only)
  data_role         data_role   NOT NULL,           -- 'tenant-content' | 'platform-operational' (ADR-12.5)
  status            principal_status NOT NULL,      -- active | suspended | retired | erased
  profile_ref       uuid,                           -- → erasable profile record (PII isolated, ADR-12.4)
  created_at        timestamptz NOT NULL,
  PRIMARY KEY (tenant, principal_id)
);

-- Agent governance: present iff kind='agent' (enforced by trigger/check constraint).
CREATE TABLE agent_governance (
  tenant            uuid NOT NULL,
  principal_id      uuid NOT NULL,
  owner_principal   uuid NOT NULL,                  -- the human/team accountable for this agent
  on_behalf_of      uuid,                           -- the human whose session caused this run (caused-by)
  runtime_ref       text NOT NULL,                  -- mock | llm:<adapter> — the strategy swap (ADR-08.2)
  delegation_policy jsonb NOT NULL,                 -- the attenuation caveat set (§7)
  run_budget        jsonb NOT NULL,                 -- RunBudget (ADR-08.6)
  ai_act_label      text  NOT NULL,                 -- always-labelled-as-agent (ADR-08.6)
  run_id            uuid,                            -- the run this per-run identity belongs to
  token_ttl         interval NOT NULL,              -- token life == run life (ID-2)
  PRIMARY KEY (tenant, principal_id),
  FOREIGN KEY (tenant, principal_id) REFERENCES principal(tenant, principal_id)
);
```

The `principal_id` is **opaque and stable** so events/git/audit attribute by it while the erasable
`profile_ref` (name, email, avatar) lives in a separately keyed record — this is the
GDPR-erasure-vs-immutability split (EI-04 §1; ADR-12.4): *delete the identity, not the fact.*

### The org → team → project → resource hierarchy
Stored as principals (`org`, `team`, `project` are *objects* in the ReBAC namespace, §5) plus
membership tuples (§6). The hierarchy is **not** a column tree in S1; it is relationship tuples in S3
so that inheritance is a graph walk the same engine answers for every subsystem (ADR-03). S1 holds the
*authoring* records (an org has a name, a billing tenant, a region); S3 holds the *authorization*
edges. This is the "RBAC face / ReBAC core" boundary (§9).

---

## 4. Authentication surfaces

All surfaces resolve to the **same `Principal`** and inject a **trusted identity header** at the
stateless gateway (EI-02 §9; X-2) — the public/internal split is a security boundary, and **tenant is
taken from the verified credential, never the URL path** (ID-3, EI-02 §1). Internal services trust the
header *only* because it crossed the gateway.

| Surface | Standard / prior art | Principal kinds | Notes |
|---|---|---|---|
| **SSO — SAML 2.0** | SAML 2.0 Web Browser SSO | Human | IdP-initiated + SP-initiated; per-tenant IdP metadata; assertions mapped to `Principal` + group→team tuples. |
| **SSO — OIDC** | OIDC Core 1.0 (auth-code + PKCE, RFC 7636) | Human | Preferred over SAML for new tenants; `id_token` (RFC 7519) + JWKS (RFC 7517) rotation. |
| **SCIM 2.0** | RFC 7642/7643/7644 | Human, Service | Provisioning/**deprovisioning** is the revocation backbone: a SCIM `DELETE`/`active:false` is the disabled-user trigger (§13 drill). |
| **MFA / Passkeys** | WebAuthn L2 / FIDO2; TOTP (RFC 6238) fallback | Human | Passkeys are the default 2FA; phishing-resistant. Step-up for break-glass + consequential actions. |
| **SSH keys (git wire)** | SSH public-key auth | Human, Service | Key → principal mapping in S1; the git smart-transport authenticates here, then `check` per ref (§5). |
| **Scoped API tokens (PAT)** | OAuth 2.1 bearer; **biscuit/PASETO** attenuable tokens | Human, Service | Scopes are *capability caveats* (attenuate-only, §7); `jti` in the denylist (S7) for revocation. |
| **CI job tokens** | OIDC-workload-style short-lived token | Service | Minted per CI run by the CI control plane *through Id*; TTL ≤ run length; auto-revoked on completion (mirrors agent tokens). Audience-bound to the run. |
| **Per-run AGENT tokens** | short-lived + macaroon-attenuated (ID-2) | Agent | **Minted at dispatch; token life == run life; revoked on teardown idempotently even on crash** (ID-2, ADR-08). The token *carries* the delegation caveat (§7) so the effective policy travels with it. Any shared platform token is **scrubbed from the child environment** (anti-leak, ID-2). |

**Token format decision (DECIDED).** Capability tokens (PAT, CI, agent) are **attenuable bearer
tokens** — a signed envelope (`PASETO v4` / JWT for compat) whose *authority* is a **macaroon/biscuit
caveat chain** (Birgisson et al. 2014; biscuit). Rationale: delegation (§7) is *attenuation*, and
macaroons make attenuation a client-side, offline, monotone operation — a parent token can mint a
strictly-narrower child without a round-trip to Id, which is exactly what per-run agent identity needs
at dispatch. **DPoP (RFC 9449) sender-constraining** is applied to long-lived PATs to blunt token
theft. Revocation is **denylist (S7) + short TTL**, never long-lived-and-hope: the TTL is the
fail-static staleness ceiling (§10).

**Floor named:** v1 ships OIDC + SAML + SCIM + passkeys + SSH + the three token types. **Follow-on:**
hardware-attested device binding and full WebAuthn passkey *sync* governance are a P5/P6 follow-on; SAML
SLO (single-logout) is best-effort in v1 (deprovision via SCIM is the authoritative revocation path).

---

## 5. The ReBAC namespace & relation model (per-subsystem)

We adopt the **Zanzibar namespace-configuration model** (Zanzibar §2.3) as implemented by SpiceDB's
schema DSL / OpenFGA's modeling language: each object *type* declares **relations** (direct edges) and
**permissions** (computed usersets — unions/intersections/exclusions and tuple-to-userset rewrites for
inheritance). A tuple is `⟨object#relation@subject⟩` where the subject may itself be a *userset*
(`object#relation`), giving group-membership and hierarchy for free (Zanzibar §2.1–2.2).

### Core hierarchy namespaces (Id-owned)
```
definition org {
  relation admin:    user
  relation member:   user | team#member
  permission administer = admin
  permission view       = admin + member
}
definition team {
  relation parent_org: org
  relation maintainer: user
  relation member:     user | team#member          // nested teams
  permission view      = member + maintainer + parent_org->administer
}
definition project {
  relation parent_team: team
  relation owner:       user
  relation contributor: user | team#member
  permission admin      = owner + parent_team->maintainer
  permission write      = admin + contributor
  permission read       = write + parent_team->view   // inheritance via tuple-to-userset rewrite
}
```
`parent_team->view` is the **tuple-to-userset rewrite** (Zanzibar's "computed_userset"/"tupleset")
that makes `org→team→project→…→artifact` inheritance a single rewrite rule rather than materialized
copies — this is the mechanism the whole hierarchy reduces to.

### Per-subsystem namespaces (subsystems *declare*, Id *owns the engine*)
Subsystems contribute their namespace fragments at build time (compiled into one cell schema). Each
artifact ID is minted by its owning subsystem; Id never invents object IDs.

**Git** — `repo`, `branch`/`ref`, `pull_request`, `pr_comment`:
```
definition repo {
  relation parent_project: project
  relation reader: user | team#member
  relation writer: user | team#member
  relation admin:  user
  permission pull         = reader + writer + admin + parent_project->read
  permission push         = writer + admin + parent_project->write
  permission administer   = admin + parent_project->admin
  permission protected_push = admin            // branch protection as a tighter permission
}
definition pull_request {
  relation parent_repo: repo
  relation author:      user
  relation reviewer:    user | team#member
  permission view    = parent_repo->pull
  permission review  = reviewer + parent_repo->push
  permission merge   = parent_repo->protected_push
}
```

**CI** — `pipeline`, `run`, `secret`, `runner_pool`: `run.view = parent_repo->pull`;
`run.trigger = parent_repo->push`; **`secret.read` is *not* inherited** — it is a direct, narrow
relation (`secret.reader: user | service`) so secrets never leak via project-read inheritance
(secrets-resolved-inside-the-boundary, CI-1).

**Issues** — `issue`, `field`, `transition`, plus the `confidential` overlay:
```
definition issue {
  relation parent_project: project
  relation assignee: user | team#member
  relation watcher:  user
  relation confidential_grant: user | team#member   // explicit grant for confidential issues
  permission view       = (parent_project->read - confidential) + confidential_grant
  permission transition = assignee + parent_project->write
}
```
**Field-level and transition-level visibility** are permissions on `field`/`transition` sub-objects
(`field.view`), and **ABAC predicates** (e.g. "field visible only if `issue.severity < X`") attach at
the edge (§9) — kept off the hot `list_objects` path. The `- confidential` exclusion is Zanzibar's
*exclusion* userset (set difference), which is why a confidential issue disappears from a normal
project-reader's `list_objects` *by construction*, not by a post-filter.

**Knowledge** — `space`, `page`, `block`, `database_row`: page-tree inheritance with **overrides** is
the canonical Zanzibar pattern — `page.read = parent_page->read + direct_reader - direct_block`. A
sub-page can *narrow* (override) inherited access via a `direct_block` exclusion relation. This is the
"page-tree inheritance with overrides" requirement (knowledge deep-dive) reduced to one rewrite + one
exclusion.

**Chat** — `channel`, `message`, `unfurl`: `channel.read = member + parent_project->read`;
`message.view = parent_channel->read`. The **per-viewer permission-aware unfurl** is *not* a chat
concern — chat asks Refs, Refs asks Id `check(viewer, view, target)` per unfurl target, so an unfurl of
a confidential issue degrades to a tombstone for a viewer who lacks `issue.view` (the §8 / §12 contract;
this is why unfurls cannot leak).

**Design rule:** every cross-subsystem visibility need (PR review, CI secret, confidential issue,
page-tree override, chat unfurl) reduces to **union + intersection + exclusion + tuple-to-userset
rewrite** over tuples — the four Zanzibar userset operators. No subsystem gets a bespoke check path.

---

## 6. Tuple schema, storage & sharding (S3)

### Tuple wire/storage shape (DECIDED)
Following Zanzibar §2.1 / SpiceDB:

```
RelationTuple {
  tenant         TenantId          // partition key prefix — no cross-tenant tuple, ever (ID-3)
  region         Region
  object         { namespace, id } // e.g. issue:9f3...   (id minted by owning subsystem)
  relation       string            // e.g. "reviewer"
  subject        Subject           // user:alice | team:eng#member (userset) | agent:run-7
  caveat         optional CaveatRef // the ABAC edge predicate (§9), evaluated at check time
  zookie         CommitToken       // the consistency token at write (§8)
  expires_at     optional ts       // native TTL for per-run agent grants (auto-expiring tuples)
}
```

`expires_at` is a deliberate extension beyond stock Zanzibar: **per-run agent grants are
auto-expiring tuples** whose TTL == run life (ID-2), so even if teardown-revoke fails, the grant
self-destructs inside the staleness window — defence in depth for the "revoke on crash" obligation.

### Storage & sharding
- **Engine:** a SpiceDB-class Zanzibar store (ADR-14), self-hostable/EU-deployable (ADR-11). Backed by
  Postgres-class storage in v1 (SpiceDB's `postgres` datastore) — **measure before sharding** (ID-4,
  EI-02 §8): the first scaling move is the **dedicated authz read-replica (S5)**, not sharding.
- **Partition key:** `(tenant, region)` then object-id hash. There is **no cross-tenant tuple and no
  cross-tenant query path** (EI-02 §1). Cross-tenant *references* (a public OSS repo referenced from
  another tenant) are handled by a narrow, explicitly visibility-gated `public` relation, never by a
  cross-tenant tuple read — this closes the public-ref PII side-channel ([OPEN → P4] gating policy, but
  the mechanism — a `public` userset, not a cross-tenant join — is DECIDED here).
- **Hot-tuple fan-in** (a popular repo/team with thousands of members): handled by the **Leopard index
  (S4)** set-flattening (Zanzibar §3.2.1) and the check-cache (§8), not by widening the tuple table.

### Tuples are event-sourced
Authz state changes are **a consequence of subsystem events** (`iam.role_granted`,
`git.repo.member_added`, SCIM group sync) consumed through the bus, and Id's own writes emit
`iam.tuple_written` via the **outbox** (BUS-2; the only emit path). This makes authz state auditable
and **reindex-from-source** rebuildable (the tuple store can be rebuilt by replaying its source events),
satisfying X-1/SEARCH-1-class recoverability.

---

## 7. The delegation / on-behalf-of algebra — AG-2 resolved

### Decision (resolves AG-2)
An agent's **effective authority is the monotone intersection**
`effective = agent.policy ∩ delegation ∩ tenant.policy`, computed as **attenuation, never
amplification** (macaroon/biscuit caveat semantics, Birgisson et al. 2014). Concretely, for a
candidate effect `check(agent_run, perm, object)` the `EffectApi` (ADR-08.3) requires **all four** to
hold:

1. **`agent.policy`** — the agent identity's own ceiling (a capability set on `agent_governance`).
2. **`delegation`** — the caveat chain carried in the per-run token: the *delegating human's* grant,
   attenuated to this run's scope (e.g. "only repo X, only `write`, only for 1h"). The delegating human
   must themselves hold the permission — **you cannot delegate authority you do not have** (this is the
   `∩ delegating_principal.authority` term, enforced by re-checking the delegator at mint time and
   carrying a `caused_by` to them).
3. **`tenant.policy`** — tenant-level guardrails (ABAC predicates, residency, agent-allow lists,
   AI-Act constraints) — a *deny-overrides* ceiling the tenant admin sets.
4. **`object` ReBAC `check`** — the ordinary tuple check, run **as the agent principal**, so the agent
   appears in `list_subjects` and audit exactly like a human.

### Why intersection, why caveats
Intersection (not union, not "agent inherits owner's full rights") is the least-privilege guarantee
EI-02 §2 demands: *"an agent can do things no human role can"* is the named failure, and intersection
makes it structurally impossible — an agent can never exceed *either* its own ceiling *or* the
delegator's grant *or* the tenant policy. Implementing `delegation` as a **macaroon caveat chain** means
the per-run token is *self-describing and offline-attenuable*: dispatch mints a child token by *adding*
caveats (monotone narrowing), no Id round-trip, and the narrowing cannot be undone client-side. This is
the exact property the per-run agent identity (ID-2) needs and the reason we chose attenuable tokens in
§4.

### Where it runs
The intersection is **not** a fourth store — it is composed at `check` time: tenant policy + agent
policy are caveats/usersets in S3; the delegation caveat rides the token; the object check is the
ordinary engine call. The `delegation(agent, trigger_actor) → effective_policy` contract (§12) returns
the *composed* decision so the Agent Fabric never re-implements the algebra. A **denied effect returns
an ordinary `Denied` tool error** to the agent loop (AG-5) — no privileged fallback.

**Drill owed (§13):** an adversarial delegation test — an agent must be unable to perform any effect
outside `agent.policy ∩ delegation ∩ tenant.policy`, including via a delegator who later lost the right.

---

## 8. The algorithms: check, list-objects, list-subjects, zookies, caching

### 8.1 `check(subject, permission, object, zookie?) → Decision`
The depth-bounded userset-rewrite evaluation from Zanzibar §2.4.2:
1. Resolve `object`'s namespace config → the permission's userset expression (union/intersect/exclude/
   TTU-rewrite).
2. Recursively evaluate, expanding `tuple-to-userset` rewrites by reading tuples (`object#relation@*`)
   and following userset subjects (`team#member`) — **memoized per request** and **bounded in depth**
   (a configured ceiling; cycles are impossible because the schema is a DAG of rewrites, but member
   graphs can be deep — bound + cache).
3. Apply any **caveat** (ABAC predicate, §9) with the request context; a caveat that needs missing
   context returns `CONDITIONAL` (the caller must supply it) — never silently allow.
4. Evaluate at the **snapshot named by the zookie** (§8.4) for read-your-writes; absent a zookie, at
   `min_latency` bounded-staleness (Zanzibar §4.3 "default consistency").

**Fail-closed on genuine uncertainty** (ADR-03): if the engine cannot resolve (corrupt config, missing
tuple it must read, predicate error), `check` **denies**. (Availability hiccups are different — §10.)

### 8.2 `list_objects(subject, permission, type, zookie?) → {ids | filter}` — the leak-free pre-filter
This is **the single most load-bearing inter-system contract** (ADR-03 §Consequences): Search and Refs
**pre-filter** with it instead of calling `check` per result (no N+1, no leak). Two return modes:

- **`ids` mode** — the enumerated reachable set, served from the **Leopard-class flattened index (S4)**
  (Zanzibar §3.2.1): the index materializes, per `(subject, permission, type)`, the set of reachable
  object IDs by incrementally consuming tuple-change events, so a set query is an index read, not a
  graph walk. Used when the set is small/bounded (a user's starred repos).
- **`filter` mode** — a *predicate pushed down to the caller's store* (a `tenant = ? AND id IN
  (reachable-set-or-subquery)` the search engine / Postgres applies). Used when the set is large
  (every issue a user can read across a big tenant): Id returns a **compiled filter** (a set
  expression + a zookie) that Search compiles into its index query (the `list_objects↔index`
  integration, ADR-03; exact push-down vs pre-fetch is [OPEN → P4 Search], but the *contract* — Id
  returns a zookie-stamped filter, Search composes it — is DECIDED).

**No-leak guarantee:** because the query AST is *permission-aware by construction* (ADR-07) it *always*
composes `list_objects(viewer, read, type)`; there is no query path that returns objects the viewer
can't see. A leak here is both a security and a GDPR breach (SC-1) — hence the cross-tenant-IDOR drill
and a zero-escape leak drill (§13).

### 8.3 `list_subjects(object, permission, zookie?) → subjects`
The Zanzibar **Expand** API (§2.4.5): returns the full userset tree for "who can do `permission` on
`object`" — powering the admin **permission inspector** ("who can see this / why", §12) and the
ReBAC **explain** (the inverse, "why can/can't P see O", a walk of the rewrite tree).

### 8.4 Consistency tokens (zookies) — read-your-writes
We adopt Zanzibar's **zookie** (§2.4.4, §3.2.5) wholesale: every tuple write returns an opaque
**`CommitToken` (zookie)** encoding the commit timestamp; a subsequent `check`/`list_objects` passing
that zookie is evaluated at a snapshot **≥ that write**, guaranteeing read-your-writes and preventing
the **"new enemy" problem** (Zanzibar §2.4.4: an object re-shared after an ACL change must not be read
at a stale snapshot that still grants the old, removed access). The flow:

- A subsystem that mutates an artifact's ACL (grant/revoke) gets a zookie back and **stamps it on the
  artifact's content version / the emitted event**.
- Any later authz read about that artifact passes the stamped zookie → Id evaluates at ≥ that point.
- Search/Refs carry the zookie through `list_objects` so a freshly-revoked grant cannot be read stale.

**This is the hard interplay with the fail-static cache (§10):** zookie reads demand freshness;
fail-static serves staleness. The reconciliation: **zookie-stamped reads bypass the fail-static cache**
(they require the named snapshot or they wait/deny); only *un-zookied, default-consistency* reads are
served from the bounded-staleness cache during a hiccup. Security-sensitive transitions (revocation,
confidential re-classification) always carry a zookie, so they are never served stale.

### 8.5 Caching (the hot path)
Three layers, Zanzibar §3.2.4-style:
- **Decision cache (S6, per-cell Redis/Valkey):** memoized `check` results keyed by
  `(tenant, subject, permission, object, snapshot-bucket)`, TTL ≤ staleness bound. **Never source of
  truth** (STOR-3).
- **Subproblem/userset cache:** memoized intermediate usersets (`team:eng#member`) shared across many
  checks — the dominant win for big teams (one team-membership expansion serves thousands of object
  checks).
- **Leopard set index (S4):** the `list_objects` flattened reachable-set, incrementally maintained.

Cache **invalidation is event-driven**: a tuple write (with its zookie) publishes an invalidation so
caches don't serve past the new snapshot for zookie-stamped reads. **Hot-spot mitigation** (a viral
repo): request-coalescing + the userset cache + the subproblem cache (Zanzibar's answer to hot
checks). Bounded prefetch + bounded pools everywhere (X-3, ADR-16).

---

## 9. RBAC face, ABAC edges (the three-layer model)

"**ReBAC core, RBAC face, ABAC at the edge**" (ADR-03), made concrete:

- **RBAC is the authoring UX** — admins assign *roles* (`org-admin`, `repo-maintainer`,
  `issue-triager`). A role is a named bundle that **compiles to tuples** at assignment time: granting
  `repo-maintainer@alice` writes `repo:R#writer@alice` (+ whatever the role bundles). Roles never
  evaluate at check time; they are a *write-time projection* into the tuple store. This keeps the
  authoring model familiar while the engine stays pure ReBAC. Role definitions are versioned and
  tenant-customizable.
- **ReBAC is the engine** (§5–§8) — every check is tuple evaluation.
- **ABAC predicates at the edges** (NIST SP 800-162) — **caveats** on tuples (SpiceDB caveats /
  OpenFGA conditions; Zanzibar has no native caveats, this is the SpiceDB extension we adopt). Used
  only where relationships are a poor fit: "field visible iff `issue.severity < X`", "merge allowed
  iff `ci.status == green`", "access only from tenant IP range / during business hours". Caveats are
  **kept off the hot `list_objects` path** (they evaluate at `check` with request context; a
  `list_objects` over a caveated relation returns `CONDITIONAL` members the caller resolves with
  context) so the bulk pre-filter stays fast. Predicates reuse the **safe, non-Turing-complete query
  AST predicate core** (ADR-07; AG-7) — one DoS-hardened evaluation engine, no second predicate
  language.

---

## 10. The fail-static availability cache (ADR-17 / ID-1)

### Decision
Distinguish the two axes (ADR-17): **authorization correctness stays fail-closed** (deny when genuinely
unsure, §8.1); **availability fails *static*** — on an Id-dependency hiccup, already-authenticated
traffic survives on a **bounded-staleness cached answer** (S6).

### What the cache holds (coarse, bounded)
Two cached artifacts per active principal, refreshed continuously in steady state:
1. **`actor_active`** — is this principal still active/un-suspended? (the cheap, high-value answer EI-02
   §10 names).
2. **`coarse_grants`** — a compact, *coarse* grant snapshot (project-level read/write the principal
   held at snapshot time), **not** the full fine-grained tuple set. Coarse-but-static keeps
   already-authenticated traffic flowing without inventing access.

### The staleness bound (GD-3 / L-1)
The staleness window **W ≤ the deprovision/revocation SLA**, and **W must contain the short-lived
agent/CI token TTL** (so a revoked agent's token expires *inside* the window regardless). Proposed
concrete bound (DPO ratifies — L-1): **W = 5 minutes**, agent/CI token TTL ≤ run length and ≤ W,
revocation SLA target = "disabled user → zero access within N=5 min" (the §13 drill threshold). The
fail-static window is the *residual GDPR-revocation exposure window* and is **DPO-ratified, written, and
dated** — not a silent default.

### The interplay with zookies (the subtle part)
- **Default-consistency reads** (no zookie) → served from S6 during a hiccup (fail-static).
- **Zookie-stamped reads** (post-revocation, post-reclassification, read-your-writes) → **bypass S6**;
  they require the named snapshot. During a hiccup these **fail closed or wait**, never serve stale —
  because these are exactly the reads where staleness would re-grant removed access (the "new enemy"
  problem, §8.4). So fail-static never weakens a *just-revoked* grant; it only keeps *unchanged*
  authority flowing.
- **Liveness ≠ readiness** (X-2): a *dead* critical dependency → not-ready → shed traffic; fail-static
  covers the *transient hiccup* where the dependency is degraded, not gone. The cache TTL self-expires,
  so a prolonged outage degrades to fail-closed as entries age past W.

**Drill owed (§13):** the **Id-hiccup / fail-static** drill — break the Id-dependency, assert
authenticated traffic survives within W and that a revoked principal is denied within N min.

---

## 11. Lifecycle & revocation

- **Onboarding:** SSO/SCIM creates the `Principal`; group memberships sync to tuples (§6).
- **Offboarding / disable:** SCIM `active:false` (or admin disable) → `status:suspended` in S1 →
  `jti` denylist (S7) → tuple revocations → cache invalidation. The **N-minute revocation guarantee**
  (§13) is the union of: token TTL ≤ W, denylist propagation, and cache-entry expiry ≤ W.
- **Ownership transfer / orphaned artifacts:** retiring a principal transfers `owner` tuples to a
  designated successor (admin or team) so artifacts don't become unreachable.
- **Break-glass:** a step-up-MFA, time-boxed, **fully audited** elevation (emits a distinct
  `iam.break_glass` event via the outbox) — never a silent backdoor.
- **Per-run agent/CI token revocation (ID-2):** revoke on teardown **idempotently even on crash**;
  belt-and-suspenders with auto-expiring tuples (§6) and TTL ≤ run life. The teardown-revoke is
  idempotent so a double-teardown or a crash-then-retry is safe.
- **Tenant decommission:** crypto-shred the tenant DEK (S1/S3) + pseudonym keys (S2) → all Id data for
  the tenant is unrecoverable; emits the offboarding receipt (ADR-12; co-ordinated by the DSR
  orchestrator).
- **Erasure (PersonalDataHolder):** Id's `erase(subject)` deletes the **pseudonym mapping (S2)** — after
  which git/bus/audit history holds only the opaque pseudonym, completing the
  erasure-vs-immutability split (EI-04 §1; ADR-12.4) — purges the erasable profile record, and
  crypto-shreds the per-subject key. `locate/export/rectify/restrict/erase` are implemented per the
  `PersonalDataHolder` contract (ADR-12).

---

## 12. Contracts / APIs exposed to other systems (the glue — stable & foundational)

This is the surface other Phase-3 systems and Phase-4 subsystems build on. Field **names AND units**
are reconciled here per X-5. `myelin-identity` (Rust, ADR-01) carries the types + the authz client;
cross-language services consume the same contract over the internal RPC surface (ADR-02).

| Contract | Signature (illustrative) | Consumed by | Semantics |
|---|---|---|---|
| **authenticate** | `authenticate(credential) → Principal{tenant, region, principal_id, kind, data_role, status}` | every gateway/entrypoint | resolves any credential; tenant from credential, never path (ID-3). |
| **check** | `check(subject, permission, object, zookie?) → {Allow \| Deny \| Conditional(ctx_needed)}` | every write path; `EffectApi` | the per-action gate; fail-closed on uncertainty (§8.1). |
| **list_objects** | `list_objects(subject, permission, type, zookie?) → {ids \| Filter{set_expr, zookie}}` | **Search, Refs** | the leak-free pre-filter (§8.2); the most load-bearing contract. |
| **list_subjects** | `list_subjects(object, permission, zookie?) → SubjectTree` | admin permission-inspector | "who can see this / why" (§8.3). |
| **explain** | `explain(subject, permission, object) → RewriteTrace` | `myelin policy show` | the ReBAC "why" — a walk of the rewrite tree. |
| **delegation** | `delegation(agent, trigger_actor) → EffectivePolicy` | Agent Fabric `EffectApi` | the composed `agent.policy ∩ delegation ∩ tenant.policy` (§7). |
| **write_tuples** | `write_tuples([Δtuple], precondition?) → zookie` | subsystems (via their own writes) + role-compile | atomic tuple write; returns the zookie to stamp (§8.4). Emitted via outbox (BUS-2). |
| **mint_token** | `mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` | Agent Fabric / CI dispatch | per-run attenuated token; TTL == run life (ID-2). |
| **revoke** | `revoke(token_jti \| principal_id)` (idempotent) | teardown, offboarding | denylist + cache invalidation; idempotent even on crash (ID-2). |
| **resolve_pseudonym / erase** | `resolve_pseudonym(subject, tenant)`, PersonalDataHolder `erase(subject)` | Git, Audit, GDPR DSR | the erasure lever (ADR-12.4). |
| **telemetry** | `auth_decision_latency`, `cache_hit_ratio`, `staleness_age`, `revocation_lag`, `tuple_write_lag` | Phase-5 drills (X-1) | the survival signals the drills read. |

**Stability promise:** these signatures are the *foundational contract* other Phase-3 docs read. The
**envelope/`ArtifactRef` shape, the `Decision` enum, and the `list_objects` `Filter` shape** are
frozen here (X-5); the *predicate language* inside caveats and the *exact filter push-down encoding*
are the named [OPEN → P4] items (§14). Subsystems **never implement their own auth** (ADR-13.3).

---

## 13. Failure modes + the drills owed (PROVE-IT)

Per the honesty rule, each property that can fail names the **quantified drill** that proves it (Phase 5
executes; this is the obligation register, ties to T-2/T-5):

| # | Failure mode | Drill (quantified gate) | Owner |
|---|---|---|---|
| D1 | **Disabled user retains access** | **disabled-user → zero-access within N=5 min**: SCIM-disable a user, assert every surface (UI/API/git wire/agent path) denies within W; cache + token TTL + denylist all inside W. | P5 (T-5) |
| D2 | **Id-dependency hiccup cascades** (ADR-17) | **Id-hiccup / fail-static**: break the Id dependency; assert already-authenticated traffic survives within W on coarse cache; assert a just-revoked principal is still denied (zookie reads bypass cache). | P5 (T-5) |
| D3 | **Cross-tenant access (IDOR)** | **cross-tenant IDOR**: attempt to read/check/list across tenants via path-tenant spoofing; assert zero cross-tenant tuples readable, zero leak (ID-3, EI-02 §1). | P5 (T-5) |
| D4 | **Permission-filtered read leaks** | **zero-escape leak drill**: confidential issue / overridden page / private channel must not appear in any `list_objects`/search/refs result for an unauthorized viewer, including under zookie staleness. | P5 |
| D5 | **Agent exceeds delegated authority** | **delegation-intersection drill** (adversarial, §7): agent cannot perform any effect outside `agent.policy ∩ delegation ∩ tenant.policy`, incl. via a delegator who lost the right; denied → ordinary `Denied` (AG-5). | P5 |
| D6 | **Per-run agent token outlives the run** | **token-TTL/crash-revoke drill**: kill a run mid-flight; assert the token is revoked (teardown) AND auto-expires (tuple `expires_at`) within run-life ≤ W (ID-2). | P5 |
| D7 | **New-enemy / stale re-grant** | **zookie consistency drill**: revoke access, immediately re-read with the post-revoke zookie; assert no stale allow (Zanzibar §2.4.4). | P5 |
| D8 | **Restore resurrects revoked/erased authority** | **restore-verify + cross-seam** (ADR-18): restore S1/S3 to a consistent point; assert no resurrected grants past an erasure; **post-restore re-erasure** runs (GD-14). | P5 |
| D9 | **Authz store down → whole platform down** | covered by D2 (fail-static) + liveness≠readiness shedding (X-2). | P5 |
| D10 | **Agent surge starves human authz** | the **30× agent-surge** drill (ADR-16): the authz hot path's human lane holds while the agent lane sheds (`429 + Retry-After`); bounded pools/prefetch (X-3). | P5 (T-5) |

---

## 14. Scaling & sharding in the cell topology

- **In-cell, tenant-partitioned.** Authz is the **highest-QPS shared system** (every action checks). It
  scales **inside a cell** (ADR-11) via the cache hierarchy (§8.5) + the Leopard set index (S4); the
  tuple store is tenant-partitioned, `(tenant, region)`-keyed.
- **Measure before you shard (ID-4, EI-02 §8).** The committed first scaling move is the **dedicated
  authz read-replica (S5)** for the authn/authz hot path — the doctrine's named "likely first real
  replica need." Sharding the tuple store is deferred until a hot tenant is *measured* to outgrow a
  single Postgres-backed shard; premature sharding is its own outage (EI-02 §8).
- **No cross-cell authz on personal-data hot paths** (ADR-11 §Consequences): a principal's authority is
  evaluated in the cell that holds the object.
- **Multi-cell tenants** (a 10,000-person org spanning cells) make "a principal spanning cells" the
  hard open case: the candidate direction is a **home-cell-authoritative + cross-cell read-through with
  zookie-bounded staleness** model (the principal's identity is authoritative in its home cell; another
  cell reads its coarse grants through a zookie-stamped, residency-respecting channel), but this is
  **[OPEN → P4/P5]** (SC-2/SC-3) and must not violate the no-cross-region-personal-data rule.
- **Bounded everything** (X-3, ADR-16): bounded connection pools (fast-fail on saturation, statement
  timeouts), bounded check-cache, per-tenant in-flight caps, protected human lane on the authz path.

---

## 15. Open questions for Phase 4 / Phase 5 / Legal

- **[OPEN → P4 Search]** The exact `list_objects ↔ search index` integration: filter push-down (Id
  returns a compiled set predicate the index applies) vs pre-fetch (Id returns enumerated ids). The
  *contract* (zookie-stamped `Filter`) is frozen here; the encoding is Search's call.
- **[OPEN → P4]** Cross-tenant reference visibility *policy* (the `public` userset mechanism is decided;
  *when* a public ref is shown, and the PII-side-channel gating, is a product/legal call — ties to
  ADR-13 §Deferred and `gdpr-eu-sovereignty.md §3.1`).
- **[OPEN → P4]** Final role-bundle catalogue per subsystem (the RBAC face's named roles) — subsystems
  declare; Id compiles. Field/transition-level caveat predicates per subsystem (Issues leads).
- **[OPEN → P4/P5]** Multi-cell principal authority (home-cell-authoritative + cross-cell read-through)
  — the deepest unknown (§14; SC-2/SC-3).
- **[OPEN → P5]** All drill thresholds (the N in "N-minute revocation", the surge multiplier, the
  staleness window's measured headroom) — this doc proposes N=5min / W=5min as the default-to-beat.
- **[OPEN → LEGAL / DPO]** Ratify W (fail-static staleness ≤ revocation SLA — L-1/GD-3). EU AI Act
  classification of the agent-governance/delegation labelling (L-4). Audit-log retention carve-out for
  authz decisions (GD-5).
- **[OPEN → P4 Agent Fabric]** The exact shape of the `EffectApi`↔`delegation()` call (single composed
  decision vs decomposed terms) co-designed with ADR-08's `Agent::handle` finalization (AG-3) — the
  *algebra* (§7) is decided; the call ergonomics are joint.

---

## 16. Cross-references
- Spine: ADR-03 (ReBAC model), ADR-17 (fail-static), ADR-08 (agent fabric / delegation), ADR-11
  (cells), ADR-12 (GDPR/PersonalDataHolder), ADR-13 (three glue contracts), ADR-16 (backpressure).
- Directives: ID-1…ID-4, GD-3, X-1…X-5, AG-2/AG-5 (delegation/denial), CI-1 (secrets-in-boundary).
- Doctrine: EI-02 §1 (tenant-first/IDOR), §2 (one principal), §9 (three-surface topology), §10
  (fail-static); EI-04 §1 (erasure-vs-immutability).
- Sibling Phase-3 docs that consume this: **Search** (`list_objects` filter), **Reference Graph**
  (backlink filtering + projection-API permission checks), **Event Bus** (authz events via outbox,
  consumer authz), **Agent Fabric** (`delegation`/`EffectApi`), **GDPR/Audit** (pseudonym lever, DSR
  fan-out), **Storage** (KMS/crypto-shred per store).
- Prior art: Zanzibar (USENIX ATC 2019); SpiceDB; OpenFGA; macaroons (NDSS 2014); biscuit; SCIM
  (RFC 7642/3/4); OIDC Core 1.0; SAML 2.0; WebAuthn L2; OAuth 2.1; DPoP (RFC 9449); NIST SP 800-162.
```
