# Phase 5 — Identity & Access (refined, canonical)

> Phase: `05-refined-shared-systems-architecture`. Canonical brief: [`VISION.md`](../../VISION.md)
> (single source of truth). Binding doctrine:
> [`external-insights/02-platform-substrate.md`](../../external-insights/02-platform-substrate.md) §1/§2/§9/§10,
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1.
> **Reconciliation spine (binding):** [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md)
> (resolves X-1..X-7, OQ-A..OQ-L) + [`contract-index.md`](./contract-index.md) (the frozen build-to surface,
> §4 Identity & access). Phase-3 base this refines:
> [`../03-shared-systems-architecture/identity-and-access.md`](../03-shared-systems-architecture/identity-and-access.md).
> Spine: ADR-03 (ReBAC), ADR-17 (fail-static), ADR-08 (agent fabric), ADR-11 (cells), ADR-12 (GDPR),
> ADR-13 (three glue contracts), ADR-16 (backpressure). Date: 2026-06-19.
>
> **What this doc is.** The refined, canonical "Identity & Access" shared-system architecture Phase 6/7
> build on. It carries the Phase-3 design forward as the base and **applies the Phase-5 reconciliation
> decisions + the §1 Identity change requests**. Where a thing is unchanged from Phase 3 it says so and
> cites it rather than restating. Code identifiers are plain text (no backticks needed for meaning).
>
> **Nothing is reversed.** Every change below is one of: **CONFIRM** (the Phase-3 seam was right, ratified),
> **SHARPEN** (the contract stood; its open encoding is now frozen concrete), or **NEW** (a sub-shape named
> for the first time). Identity led no Phase-3 ADR reversal and leads none here.

---

## 0. Changes vs Phase 3 (the complete list)

Every change to Identity & Access in Phase 5, each tagged and traced to its reconciliation source:

| # | Change | Kind | Source | Contract |
|---|---|---|---|---|
| C1 | **`list_objects` returns `Ids{ids,zookie}` OR `Filter{set_expr,zookie}`; `SetExpr` is a frozen consumer-composable set algebra** (All/None/Ids/NotIds/InRelation{relation,via_column}/Union/Intersect/Difference/TupleSet{index}) lowered to a SQL predicate/JOIN over the consumer's **own id column** via a **per-tenant authz reverse index**. No N+1, no post-filter. | **SHARPEN → frozen** | OQ-E / S-10 | 4.3 |
| C2 | **The per-tenant authz reverse index is now a named first-class store** (S8) — a materialised `(subject, relation, object_id)` projection of S3 tuples, kept fresh off the bus, the JOIN target the five subsystems push down against; honours the zookie revision watermark. | **SHARPEN** (it was the Leopard S4 idea; now its consumer-JOIN shape + watermark are frozen) | OQ-E | 4.3, 4.4, 4.10 |
| C3 | **`CaveatContext{object, field?, transition?, attrs}`** on `check` for field-level / transition-level ABAC, evaluated at `check`-time on already-filtered rows, **off the hot `list_objects` path**. | **SHARPEN → frozen** | OQ-E / ISS-KN field hiding | 4.2 |
| C4 | **Per-subsystem ReBAC namespace fragments frozen** — Git (ref-glob + CODEOWNERS-as-relations + `approve_untrusted_ci`); CI (`ci_project/environment/secret/run` + `read & !is_untrusted_fork`); Issues (`issue` + field/transition caveats); Knowledge (page-tree inherit-with-overrides + row + field caveat); Chat (`channel.read = member + parent_project->read`); a `watcher` relation per watchable type. | **CONFIRM → fragments frozen** | CR §1, X-1 | 4.9 |
| C5 | **Pseudonym grammar pinned `<pseudonym>@<tenant>.noreply`**; Git commits pseudonymous-by-default (GIT-1); `resolve_pseudonym`/`erase` is DSR step-1. | **CONFIRM + NEW grammar pin** | CR §1, X-7 | 4.8 |
| C6 | **Machine-identity resolution pinned** — SSH-pubkey / repo-scoped deploy-key machine principal / PAT / per-job token → Principal; **self-hosted runner token scoped to one tenant's SelfHosted jobs** (cannot mint cross-tenant); per-job token re-mintable mid-workflow on resume. | **SHARPEN** | CR §1 / S-11 | 4.1, 4.7 |
| C7 | **`approve_untrusted_ci` permission served as a namespace relation** so Git's fork-endorsement gate (X-1) is an ordinary `check`, not bespoke logic; the `trust_tier` is **stamped by CI from run provenance + the `read & !is_untrusted_fork` ABAC edge**, Git reads it, Identity never recomputes trust. | **CONFIRM** (the relation) + alignment to X-1 | X-1 / OQ-A | 4.9, 5.9 |
| C8 | **`list_subjects(object, watcher)` performant at 50k-member density**, served by the same authz reverse index (S8); the read-fanout half of the fanout boundary. | **SHARPEN** (density pinned) | CR §1 | 4.4 |
| C9 | **`mint_run_token` callable mid-workflow on resume** for multi-day HITL (a days-later approval re-mints a fresh attenuated token). | **CONFIRM** | CR §1 / S-11 | 4.7 |
| C10 | **Zookie from `write_tuples` stamped on the object** (`page.acl_zookie`, Chat membership) — the new-enemy guard; the reverse index honours the revision watermark so a just-revoked grant cannot read stale. | **CONFIRM** (mechanism was decided; consumer surfaces named) | CR §1 | 4.6, 4.10 |
| C11 | **Fail-static bound** `static_max ≤ revocation SLA` and `≥ agent-token TTL`; W = 5 min default-to-beat. **`[OPEN — LEGAL]`: DPO ratifies the bound (L-1).** | **CONFIRM** (`[OPEN — LEGAL]` ratification carried) | recon §1, L-1 | 4.11 |
| C12 | **Residual free-text/immutable-PII erasure handled by reference**, not restated — Identity owns the pseudonym-map shred half (DSR step-1); the rest is the platform posture in 00-reconciliation §X-7. | **NEW (by reference)** | X-7 / OQ-G | 10.9 |

Everything else in the Phase-3 doc — the one-polymorphic-Principal model (§3, AG-1), the delegation
intersection algebra (§7, AG-2), the Zanzibar `check` evaluation (§8.1), zookie semantics (§8.4), the
three-layer RBAC-face/ReBAC-core/ABAC-edge model (§9), the fail-static availability cache (§10), the
lifecycle/revocation flows (§11), and all ten drills (§13) — is **CONFIRMED unchanged** and is carried
forward by citation below.

---

## 1. Purpose & responsibilities — CONFIRMED (Phase 3 §1)

Identity & Access (Id) is the **dependency root of the platform** (EI-02 §3: "identity depends on nothing").
It answers exactly two questions and owns the data behind them: **who is this?**
(`authenticate(credential) → Principal`) and **may this principal do this?**
(`check` + the set-valued `list_objects` / `list_subjects`). One ReBAC (Zanzibar-style) engine evaluates
humans, agents, and services **identically** (ADR-03, EI-02 §2). Carried verbatim from Phase 3 §1.

**The three platform invariants Id inherits and never breaks** (Phase 3 §1, CONFIRMED): residency-pinned +
per-tenant envelope-encrypted + crypto-shred-capable + `PersonalDataHolder` on every store; `(tenant,
region)` in the partition key of every table, tuple, cache key, queue, **and now the authz reverse index**
(C2); the transactional outbox is the only emit path.

**What Id is NOT** (Phase 3 §1, CONFIRMED): not a subsystem-UI session store; not the audit log (it emits to
it via the outbox); does not store artifact content — only tuples about artifacts whose ids are minted by
their owning subsystem. **No subsystem reads Id's stores; everything goes through the contracts in §11.**

**Phase-5 sharpening of the second question.** The leak-free pre-filter `list_objects` is now frozen with a
concrete consumer-composable push-down (C1), and a `CaveatContext` rider lets `check` do field/transition
hiding off that hot path (C3). These are the two highest-fan-in shapes Identity exposes; they were "→ P4
open" in Phase 3 and are now frozen.

---

## 2. The store map — SHARPENED (one new derived store: S8)

Phase 3 §2 named S1–S7. Phase 5 promotes the `list_objects` index from "Leopard-class set index (S4)" to a
**concrete consumer-JOIN-shaped authz reverse index (S8)** because the OQ-E push-down (C1/C2) requires the
index to be JOIN-able from each subsystem's own query planner against the subsystem's own id column. S4 (the
flattened reachable-set, materialise/`Ids` path) and S8 (the per-tenant `(subject, relation, object_id)`
reverse index, JOIN/`Filter` path) are **two faces of the same Leopard-class derivation** of S3; both are
reindex-from-source primitives rebuilt by replaying S3 through the live consumer (no bespoke recovery code).

| # | Component | Engine (ADR-14) | Holds | Shard key | Blast radius | Crypto-shred unit | vs P3 |
|---|---|---|---|---|---|---|---|
| S1 | Principal/Auth DB | Postgres-class | principals, orgs/teams/projects, credentials, tokens, SSO/SCIM links, agent-identity records | `(tenant, region)` | one tenant (RLS) | per-tenant DEK; per-subject sub-key for profile PII | CONFIRMED |
| S2 | Pseudonym map | Postgres-class (tightest RLS) | `real_identity ↔ per-tenant pseudonym`; **the erasure lever** | `(tenant, region)` | one tenant | **per-subject key** | CONFIRMED; grammar pinned (C5) |
| S3 | ReBAC tuple store | SpiceDB-class (Zanzibar) | `object#relation@subject` tuples; the hot authz path | `(tenant, region)` + object-id hash | one tenant; one shard | per-tenant DEK | CONFIRMED |
| S4 | `list_objects` reachable-set index (Leopard-class) | flattened set index | denormalised reachable-set, the `Ids` materialise path | `(tenant, region)` + type | derived — rebuildable from S3 | derived (inherits S3) | CONFIRMED |
| **S8** | **Authz reverse index (the JOIN target)** | **Postgres-class co-located projection (own store / read replica)** | **per-tenant `(subject, relation, object_id)` projection of S3, + a `revision_watermark` column; the consumer JOINs against `authz_visible`** | **`(tenant, region)` + object-type** | **derived — rebuildable from S3; per-tenant only, no cross-tenant query path** | **derived (inherits S3)** | **NEW (C2)** |
| S5 | Authz read-replica (ID-4) | Postgres / tuple-store replica | the authn/authz hot-path replica (first real replica need, EI-02 §8) | follows S1/S3/S8 | read-only; stale-tolerant | inherits | CONFIRMED |
| S6 | Fail-static cache (ADR-17) | Redis/Valkey-class (NEVER source of truth) | bounded-staleness `{actor_active, coarse_grants}` + decision cache | `(tenant, region, subject)` | one cell; staleness-bounded | ephemeral; TTL ≤ revocation SLA | CONFIRMED |
| S7 | Revocation list / token denylist | Redis/Valkey + PG mirror | revoked `jti`s, suspended principals, per-run agent token TTLs | `(tenant, region)` | one cell | ephemeral | CONFIRMED |

**Why S8 is its own row and not just S4.** S4 answers "give me the ids" (small, bounded sets — a materialise).
S8 answers "let my query planner do the conjoin" (large/unbounded sets — a JOIN target the consumer's own SQL
references). They serve the two return modes of the same contract (4.3). S8 is **per-tenant** (EI-02 §1: no
cross-tenant query path), is itself a `PersonalDataHolder` (its tuples reference subjects), and is the
dedicated read replica the doctrine names as the likely first scaling need (ID-4). For
`can_derive_plaintext_index()=false` (HYOK) tenants S8 still works — **it indexes tuples, not content**.

---

## 3. The Principal model — CONFIRMED (Phase 3 §3, AG-1) + machine-identity pinned

**One polymorphic `Principal` record with a `kind` discriminant `Human | Agent | Service`** — not two
parallel models, not a Service/Agent merge. The discriminant changes governance metadata and credential
type, never the authorization code path: `check(subject, …)` does not branch on `kind`. This is carried
**unchanged** from Phase 3 §3 (AG-1 resolved); the schema (`principal` + `agent_governance`), the
three-axis face table, and the org→team→project hierarchy-as-tuples are all unchanged — see Phase 3 §3.

**Phase-5 pin (C6): machine identity resolves to the same Principal.** Every machine credential resolves to a
Principal of `kind` Human or Service exactly as Phase 3 §4 specified; Phase 5 freezes the four shapes the
subsystems asked for (CR §1):

- **SSH public key** → the principal mapped in S1 (Git smart-transport authenticates here, then `check` per
  ref). Human or Service.
- **Deploy key** → a **repo-scoped machine principal** (`kind = service`) — a Service whose authority ceiling
  is one repo, expressed as a narrow tuple, never a project-wide grant.
- **PAT** → a Human or Service with capability caveats (attenuate-only, §6 delegation algebra).
- **Per-job CI / runner token** → a `kind = service` Principal minted per run through Id; **a self-hosted
  runner token is scoped to one tenant's SelfHosted jobs** (it cannot mint or act cross-tenant — the
  no-global-pool property at the identity layer, ties to Tenancy 12.4); the per-job token is **attenuated and
  re-mintable mid-workflow on resume** (4.7, S-11).

`principal_id` stays opaque and stable so events/git/audit attribute by it while the erasable `profile_ref`
lives separately — the GDPR-erasure-vs-immutability split (EI-04 §1; ADR-12.4). Unchanged from Phase 3 §3.

---

## 4. Authentication surfaces — CONFIRMED (Phase 3 §4) + machine-identity scope (C6)

All surfaces resolve to the **same Principal** and inject a trusted identity header at the stateless gateway;
**tenant is taken from the verified credential, never the URL path** (ID-3, EI-02 §1). The full surface table
(SAML 2.0, OIDC, SCIM 2.0, WebAuthn/FIDO2 passkeys, SSH, scoped PAT, CI job tokens, per-run agent tokens) is
**unchanged from Phase 3 §4** and not restated here.

**Token format decision — CONFIRMED (Phase 3 §4).** Capability tokens (PAT, CI, agent) are **attenuable
bearer tokens** (PASETO v4 / JWT envelope) whose authority is a **macaroon/biscuit caveat chain** (Birgisson
et al. 2014; biscuit), so delegation is offline, client-side, monotone attenuation; **DPoP (RFC 9449)**
sender-constrains long-lived PATs; revocation is **denylist (S7) + short TTL** where the TTL is the
fail-static staleness ceiling (§10). Unchanged.

**Phase-5 pins (C6):** the self-hosted-runner token scope (one tenant's SelfHosted jobs), the deploy-key
repo-scoped machine principal, and the per-job-token mid-resume re-mint are now frozen on 4.1/4.7. No new
auth surface; the existing ones are made precise where CI/Git needed bytes.

**Floor named (Phase 3 §4, CONFIRMED):** v1 ships OIDC + SAML + SCIM + passkeys + SSH + the three token
types; hardware-attested device binding and full passkey-sync governance are a P5/P6 follow-on; SAML SLO is
best-effort (SCIM deprovision is the authoritative revocation path).

---

## 5. The ReBAC namespace & relation model — CONFIRMED + fragments frozen (C4, C7)

We adopt the **Zanzibar namespace-configuration model** (Zanzibar §2.3; SpiceDB schema DSL / OpenFGA modeling
language): each object type declares **relations** (direct edges) and **permissions** (computed usersets —
union / intersection / exclusion + tuple-to-userset rewrites for inheritance). A tuple is
`⟨object#relation@subject⟩`; the subject may be a userset. The core hierarchy namespaces (org / team /
project, with `parent_team->view` as the tuple-to-userset inheritance rewrite) are **unchanged from Phase 3
§5**.

**Phase-5: the per-subsystem fragments are now FROZEN (C4).** Subsystems declare their namespace fragments at
build time; Id owns the engine and **never invents object ids**. The frozen fragments (contract 4.9):

- **Git** — `repo`, `branch`/`ref`, `pull_request`, `pr_comment`, with **ref-glob-scoped relations**
  (branch-protection as a tighter `protected_push` permission) and **CODEOWNERS expressed as relations**
  (a CODEOWNERS path-glob compiles to reviewer-requirement tuples, not a bespoke check), **plus the new
  `approve_untrusted_ci` relation** (C7) that the fork-endorsement gate reads. `pull_request.merge =
  parent_repo->protected_push` is unchanged from Phase 3 §5.
- **CI** — `ci_project`, `environment`, `secret`, `run`. `run.view = parent_repo->pull`;
  `run.trigger = parent_repo->push`; **`secret.read` is NOT inherited** (a direct narrow relation, so secrets
  never leak via project-read inheritance — CI-1). **Plus the `read & !is_untrusted_fork` ABAC edge** (C7):
  CI stamps `trust_tier` from run provenance using this edge; a fork run is `untrusted_fork`. Unchanged
  intent from Phase 3 §5; the fork edge + secret-non-inheritance now frozen.
- **Issues** — `issue`, `field`, `transition`, plus the `confidential` overlay. Field-level and
  transition-level visibility are permissions on `field`/`transition` sub-objects with **ABAC caveats**
  (e.g. "field visible iff `issue.severity < X`"; "transition needs an approver edge"), kept off the hot
  `list_objects` path via the `CaveatContext` (C3, §8.6). The `- confidential` exclusion userset is why a
  confidential issue disappears from a normal project-reader's `list_objects` **by construction**, not by a
  post-filter. Unchanged from Phase 3 §5; the caveat shape now frozen.
- **Knowledge** — `space`, `page`, `block`, `database_row`. Page-tree inheritance **with overrides**
  (`page.read = parent_page->read + direct_reader - direct_block`) lets a sub-page narrow inherited access;
  **row-level** ACL pushes down via `list_objects` (C1); **field-level** column hiding is a `check`-time
  caveat (C3). Unchanged from Phase 3 §5.
- **Chat** — `channel`, `message`, `unfurl`. `channel.read = member + parent_project->read`;
  `message.view = parent_channel->read`. The per-viewer permission-aware unfurl is a Refs concern (Refs asks
  Id `check(viewer, view, target)` per unfurl target), so an unfurl of a confidential issue degrades to a
  tombstone for a viewer lacking `issue.view` — unfurls cannot leak. Unchanged from Phase 3 §5.
- **Cross-cutting: a `watcher` relation per watchable type** (C8) — every watchable type declares
  `watcher: user`, so Notif's read-fanout (`list_subjects(object, watcher)`) is served by the same engine +
  reverse index (S8). Frozen.

**Design rule (Phase 3 §5, CONFIRMED):** every cross-subsystem visibility need reduces to **union +
intersection + exclusion + tuple-to-userset rewrite** over tuples — the four Zanzibar userset operators. No
subsystem gets a bespoke check path. The `approve_untrusted_ci` endorsement, the `!is_untrusted_fork` edge,
the confidential exclusion, the page-tree override, and the chat unfurl are all instances of these four.

---

## 6. Tuple schema, storage, sharding & the delegation algebra — CONFIRMED (Phase 3 §6, §7)

**Tuple shape, event-sourcing, and sharding** are unchanged from Phase 3 §6: `RelationTuple {tenant, region,
object, relation, subject, caveat?, zookie, expires_at?}`; `(tenant, region)` then object-id hash partition;
**no cross-tenant tuple and no cross-tenant query path**; per-run agent grants are **auto-expiring tuples**
(`expires_at` == run life) as defence-in-depth for revoke-on-crash; authz state changes are event-sourced and
Id emits `iam.tuple_written` via the **outbox** (the only emit path), so the tuple store is
reindex-from-source rebuildable. **The new consumer of `iam.tuple_written` is S8** (C2): the authz reverse
index is fed by exactly these tuple-write events, carrying the write's zookie as the revision watermark.

**The delegation / on-behalf-of algebra — CONFIRMED (Phase 3 §7, AG-2).** An agent's effective authority is
the **monotone intersection** `effective = agent.policy ∩ delegation ∩ tenant.policy`, computed as
attenuation never amplification (macaroon/biscuit caveats). The four conjuncts (agent ceiling; the delegating
human's grant carried as the per-run token's caveat chain; tenant guardrails; the ordinary object `check` run
as the agent principal) and the "you cannot delegate authority you do not have" re-check at mint are all
unchanged from Phase 3 §7. The `delegation(agent, trigger_actor) → EffectivePolicy` contract returns the
composed decision so the Agent Fabric never re-implements the algebra. Unchanged; this is the security floor
that makes "an agent can do things no human role can" structurally impossible (EI-02 §2).

---

## 7. `list_objects` — SHARPENED → frozen (C1, C2): the platform's most load-bearing contract

This is the single most-repeated ask across all five subsystems (OQ-E / S-10) and the most load-bearing
inter-system contract (ADR-03). Phase 3 left the push-down encoding "→ P4 open"; Phase 5 freezes it.

### 7.1 The return shape (frozen — matches contract 4.3)

```
list_objects(subject, permission, type, zookie?) → ListObjectsResult

ListObjectsResult =
  | Ids    { ids: Vec<ObjectId>, zookie: Zookie }    // small sets: materialise (default under a cardinality cap; the S4 path)
  | Filter { set_expr: SetExpr, zookie: Zookie }     // large/unbounded: push down (the S8 path)

SetExpr =                                  // a tenant-scoped, monotone set algebra over the object-id space
  | All                                    // subject sees every object of this type in the tenant (e.g. admin)
  | None                                   // subject sees nothing (deny) — consumer adds `WHERE false`
  | Ids(Vec<ObjectId>)                     // an explicit allow-set, inlined when small
  | NotIds(Vec<ObjectId>)                  // an explicit deny-set over an otherwise-visible space
  | InRelation { relation: RelName, via_column: ColRef }   // objects where this id is the object of <relation> for subject
  | Union([SetExpr]) | Intersect([SetExpr]) | Difference(SetExpr, SetExpr)
  | TupleSet { index: AuthzIndexRef }      // a server-materialised tuple set the consumer JOINs against (the big-result path)

ColRef = { table: "<consumer table>", column: "<the id column>" }   // names the consumer's OWN id column
```

The `Filter` is **not an opaque blob** — it is structured and consumer-composable: the consumer's
`myelin-query` compiler (the same compiler that lowers saved-view ASTs) lowers `set_expr` into a SQL predicate
over its own `via_column`/`ColRef` and ANDs it into the board/list/search query.

### 7.2 The no-N+1, no-post-filter lowering (frozen)

- **`Ids` / `NotIds`** → `WHERE <id_col> IN (...)` / `NOT IN (...)`, inlined under the cardinality cap.
- **`InRelation { relation, via_column }`** and **`TupleSet { index }`** → a **JOIN against S8**, the
  per-tenant, residency-pinned authz reverse index (the `(subject, relation, object_id)` projection of S3):

  ```sql
  ... JOIN authz_visible av
        ON av.object_id = <consumer table>.<id column>
       AND av.subject   = $subject
       AND av.relation  = $relation
  ```

  This is the SpiceDB/Zanzibar "reverse index / LookupResources" pattern (Leopard, Zanzibar §3.2.1) realised
  as a **co-located JOIN target**, so the consumer's own query planner does the conjoin — **one query, no
  N+1, no post-filter** (the SC-1 leak-and-slowness fix; ADR-03's pre-filter-not-post-filter mandate).
- **`Union` / `Intersect` / `Difference`** → the boolean composition compiled to `AND` / `OR` / `EXCEPT`.

### 7.3 The five id columns this serves (frozen mapping)

Each subsystem names its **own** `via_column` and JOINs against `authz_visible` keyed by **that** object type:

| Consumer | `type` | `via_column` (the consumer's own id column) | the conjoin |
|---|---|---|---|
| Git | `pr` / `repo` | `pr.id` / `repo.id` | board/list of PRs/repos the viewer may read |
| CI | `run` | `run.id` | the runs list, ACL-filtered |
| Issues (was *blocking*) | `issue` | `issue.id` | the board/backlog scan; the Tier-3 escalation valve compiles the board query to Search with the **same** `Filter` conjoined (now unblocked) |
| Knowledge | `database_row` | `db_row.id` | a db view, row-level ACL pushed down (field-level hiding is the off-hot-path caveat, §8.6) |
| Chat | `channel` / `message` | `channel.id` / `message.id` | the ambient channel list; `list_subjects` serves the read-fanout side |

### 7.4 Consistency (CONFIRMED + watermark, C10)

The returned `zookie` bounds staleness; a security-sensitive scan passes the zookie so the read does not use
the fail-static cache (4.10). **Read-your-writes**: a just-revoked grant (`write_tuples` returned a newer
zookie, stamped on the object) is reflected because the JOIN reads S8 at-or-after the zookie's revision — S8
carries a `revision_watermark`, and a scan requiring a fresher revision **waits or falls back to per-row
`check`** rather than serving stale. This is the new-enemy guard (Zanzibar §2.4.4) realised through the index.

### 7.5 `list_subjects` at density (C8)

`list_subjects(object, permission, zookie?) → SubjectTree` (the Zanzibar Expand API) + `explain(...) →
RewriteTrace` are unchanged in semantics (Phase 3 §8.3). Phase 5 pins that the **read-fanout case**
(`list_subjects(channel, watcher)` over a 50k-member channel) is served by the **same S8 reverse index**, so
Notif's ambient-unread fanout does not degrade. The `watcher` relation (C4) is what makes this an ordinary
expand, not a bespoke scan.

---

## 8. The algorithms — CONFIRMED (Phase 3 §8) + the `CaveatContext` rider (C3)

`check` evaluation (depth-bounded userset-rewrite, memoised-per-request, fail-closed on genuine uncertainty,
evaluated at the zookie's snapshot), zookie / read-your-writes semantics (§8.4), and the three-layer caching
(decision cache S6, subproblem/userset cache, the Leopard set index S4) are **unchanged from Phase 3 §8** and
not restated. Two Phase-5 sharpenings:

### 8.6 `CaveatContext` — field/transition ABAC at `check`-time, off the hot path (C3, frozen)

Row/object visibility is the `list_objects` push-down (§7). **Field-level** hiding (issue `field.view`, KN
column hiding) and **transition** approver checks are an **ABAC caveat evaluated at `check`-time** on the
already-filtered, already-fetched rows — **never** on the hot `list_objects` path (that would defeat the
conjoin). The frozen shape (contract 4.2):

```
CaveatContext { object: ArtifactRef, field: Option<FieldId>, transition: Option<TransitionId>, attrs: Map<String, Literal> }
check(subject, view_field, object, zookie?, caveat: CaveatContext) → Allow | Deny | Conditional
```

So `list_objects` returns the visible rows cheaply; `check` with a `CaveatContext` then redacts individual
fields / gates individual transitions on those rows. A caveat needing missing context returns `Conditional`
(the caller supplies it) — never a silent allow. Caveats reuse the **safe, non-Turing-complete `QueryAst`
predicate core** (ADR-07; = the `EventMatcher` core, contract 3.4) — one DoS-hardened evaluation engine, no
second predicate language. This is the field/transition caveat Issues and Knowledge asked for (CR §1),
frozen.

### 8.7 The authz reverse index revision watermark (C2/C10)

S8 carries a per-row `revision_watermark` derived from the zookie of the `iam.tuple_written` event that
produced it. A zookie-stamped scan compares its required revision against the watermark: at-or-after → JOIN
serves; behind → wait-or-fall-back-to-`check`. This is how the new-enemy guard survives the move from a live
graph walk to a materialised JOIN target.

---

## 9. RBAC face, ABAC edges — CONFIRMED (Phase 3 §9)

The three-layer model is unchanged: **RBAC is the authoring UX** (a role compiles to tuples at assignment
time — a write-time projection, never a check-time evaluation; granting `repo-maintainer@alice` writes
`repo:R#writer@alice`); **ReBAC is the engine** (§5–§8); **ABAC predicates at the edges** are caveats on
tuples (SpiceDB caveats / OpenFGA conditions), kept off the hot `list_objects` path and now carried through
the `CaveatContext` (§8.6). Predicates reuse the one safe query-AST predicate core (ADR-07). Carried from
Phase 3 §9.

---

## 10. The fail-static availability cache — CONFIRMED (Phase 3 §10) + bound carried `[OPEN — LEGAL]` (C11)

**Authorization correctness stays fail-closed** (deny when genuinely unsure); **availability fails static** —
an Id-dependency hiccup keeps already-authenticated traffic alive on a bounded-staleness cached
`{actor_active, coarse_grants}` (S6). The cache contents, the zookie interplay (zookie-stamped reads bypass
S6 and fail-closed-or-wait; default-consistency reads are served static during a hiccup), and liveness ≠
readiness shedding are all **unchanged from Phase 3 §10**.

**The staleness bound (C11, `[OPEN — LEGAL]`).** `static_max ≤ revocation SLA` and `static_max ≥ agent/CI
token TTL` (so a revoked machine token expires inside the window regardless). Proposed concrete bound
(default-to-beat): **W = 5 minutes**, agent/CI token TTL ≤ run length and ≤ W, revocation SLA target =
"disabled user → zero access within N = 5 min" (the §12 drill threshold). **The fail-static window is the
residual GDPR-revocation exposure window — DPO ratifies (L-1).** Engineering posture: the bound is written,
dated, and structurally enforced regardless of ratification; counsel/DPO ratify the *number*. Flagged.

---

## 11. Contracts exposed & consumed (the frozen surface — matches contract-index §4)

### 11.1 Exposed (Identity owns; every other system consumes)

| Contract | Signature (frozen shape) | Consumed by | Status vs P3 |
|---|---|---|---|
| **authenticate** | `authenticate(credential) → Principal{tenant, region, principal_id, kind, data_role, status}`; **+ machine-identity** (SSH-pubkey / repo-scoped deploy-key / PAT / per-job token → Principal) | every gateway/entrypoint | **SHARPENED** (C6) — 4.1 |
| **check** | `check(subject, permission, object, zookie?, caveat?: CaveatContext) → {Allow \| Deny \| Conditional}` | every write path, `EffectApi`, gateways, Notif | **SHARPENED** (C3, the `CaveatContext`) — 4.2 |
| **list_objects** | `list_objects(subject, permission, type, zookie?) → Ids{ids, zookie} \| Filter{set_expr, zookie}`; `SetExpr` = the frozen set algebra, lowered to a SQL predicate / JOIN over the consumer's own id column via S8 | Search, Refs, Git/CI/Issues/KN/Chat (every permission-aware read) | **SHARPENED → frozen** (C1/C2) — 4.3. **The single most load-bearing inter-system contract.** |
| **list_subjects / explain** | `list_subjects(object, permission, zookie?) → SubjectTree` + `explain(...) → RewriteTrace`; performant at 50k-member density via S8 | admin inspector, HITL approver set, Notif read-fanout (`watcher`) | **SHARPENED** (C8) — 4.4 |
| **delegation** | `delegation(agent, trigger_actor) → EffectivePolicy` = `agent.policy ∩ delegation ∩ tenant.policy` (monotone, macaroon caveats) | Agent `EffectApi`, workflow activities | **CONFIRMED** — 4.5 |
| **write_tuples** | `write_tuples([Δtuple], precondition?) → zookie` — atomic; returns the zookie to stamp on the object (`page.acl_zookie`, Chat membership); emitted via outbox; feeds S8 | subsystems, role-compile | **CONFIRMED** (C10 consumers named) — 4.6 |
| **mint_run_token / revoke** | `mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` (life == run life; **callable mid-workflow on resume**; **self-hosted-runner token scoped to one tenant's SelfHosted jobs**) + `revoke(jti \| principal_id)` (idempotent even on crash) | Agent Fabric, CI dispatch, workflow | **SHARPENED** (C6/C9) — 4.7 |
| **resolve_pseudonym / erase** | `resolve_pseudonym(subject, tenant)` + PersonalDataHolder `erase(subject)`; **pseudonym grammar `<pseudonym>@<tenant>.noreply` (frozen)**; Git commits pseudonymous-by-default | Git, Audit, DSR orchestrator | **SHARPENED** (C5) — 4.8 |
| **ReBAC namespace fragment** | each subsystem declares relations + permissions, compiled into one cell schema; **frozen fragments** (C4): Git (+`approve_untrusted_ci`), CI (+`read & !is_untrusted_fork`), Issues (+field/transition caveats), KN (page-tree-overrides + row + field caveat), Chat (`channel.read`); `watcher` per watchable type | Id (engine) + each subsystem (fragment) | **SHARPENED → frozen** (C4/C7) — 4.9 |
| **Consistency / zookie** | read-your-writes; zookie-stamped reads bypass fail-static; **S8 honours the revision watermark** | Search, Refs, Notif, every authz read | **CONFIRMED** (C2 watermark) — 4.10 |
| **FailStatic bound** | `static_max ≤ revocation SLA` ≥ agent-token TTL; W = 5 min default-to-beat | substrate + Id | **CONFIRMED**, `[OPEN — LEGAL]` (C11) — 4.11 |
| **telemetry** | `auth_decision_latency`, `cache_hit_ratio`, `staleness_age`, `revocation_lag`, `tuple_write_lag`, **`reverse_index_lag`** (NEW: S8 freshness) | Phase-5 drills | **SHARPENED** (S8 lag signal) — 1.8 |

### 11.2 Consumed (Identity depends on — the short list)

Identity is the dependency root, so it consumes very little. It consumes: the **bus** (contract 2.x outbox to
emit `iam.tuple_written`/`iam.role_granted`/`iam.break_glass`; the consumer template to feed S3 from subsystem
events — SCIM sync, `git.repo.member_added` — and to feed S8 from `iam.tuple_written`); **Storage KMS**
(11.3, per-cell root → per-tenant KEK → per-tenant/per-subject DEK; `KeyOrigin` for BYOK/HYOK; S2's
per-subject key is the erasure lever); the **GDPR `PersonalDataHolder`** spine (10.1 — every Id store incl.
**S8 NEW** auto-registers); the **control plane** (12.x `(tenant, region)` injection, residency). Identity
**does not** consume Refs, Search, Agent, Notif, or any subsystem — it is below them (EI-02 §3).

---

## 12. Failure modes + drills owed — CONFIRMED (Phase 3 §13) + one new assertion

All ten Phase-3 drills (D1 disabled-user→zero-access N=5min; D2 Id-hiccup/fail-static; D3 cross-tenant IDOR;
D4 zero-escape leak; D5 delegation-intersection adversarial; D6 token-TTL/crash-revoke; D7 new-enemy/zookie;
D8 restore-resurrects-authority; D9 authz-store-down→fail-static; D10 30×-agent-surge human-lane-holds) are
**carried forward unchanged** (Phase 3 §13). Phase 5 adds **one assertion** to the existing drills (no new
drill engine):

- **D4 (zero-escape leak) now asserts over the S8 JOIN push-down**: the confidential issue / overridden page
  / private channel must not appear in any `list_objects` **`Filter`-lowered JOIN result** for an
  unauthorised viewer, including under S8 staleness (the JOIN must be at-or-after the zookie revision
  watermark or fall back to `check`). This is the C1/C2 surface's leak gate.
- **D7 (new-enemy / zookie) now asserts the S8 watermark path**: revoke, immediately re-scan with the
  post-revoke zookie; assert the JOIN waits/falls-back rather than serving the stale grant from S8.

Both ride the existing drill obligations; Phase 5 (T-5) executes.

---

## 13. Scaling & sharding — CONFIRMED (Phase 3 §14) + S8 as the named first replica

Unchanged from Phase 3 §14: authz is the highest-QPS shared system, scales **inside a cell** via the cache
hierarchy + the Leopard set index; **measure before you shard** (ID-4) — the committed first scaling move is
the dedicated authz read-replica (S5). Phase 5 names that **S8 (the authz reverse index) is the concrete
realisation of that "likely first real replica need"** (the doctrine's ID-4 prediction): it is a per-tenant,
residency-pinned read replica/projection co-located for the consumer JOIN. The multi-cell principal-authority
question (home-cell-authoritative + cross-cell read-through, SC-2/SC-3) remains the deepest open case and now
rides the **cross-cell PII-free pointer bridge** (OQ-I, contract 12.6) — resolution is always cell-local, so
a principal spanning cells is evaluated in the cell that holds the object, never by pulling tuples
cross-region (preserving no-cross-region-PII, ADR-11). See §14 open questions.

---

## 14. Cited prior art — CONFIRMED (Phase 3 §0)

Unchanged from Phase 3: **Zanzibar** (Pang et al., USENIX ATC 2019) for the tuple/check/expand/zookie core
and the Leopard set index (§3.2.1) behind `list_objects` / S4 / S8; **SpiceDB** and **OpenFGA** as the open,
EU-self-hostable implementations (SpiceDB caveats / OpenFGA conditions for the ABAC edges + the
`CaveatContext`); **macaroons** (Birgisson et al., NDSS 2014) and **biscuit** for attenuable delegation
tokens; **NIST SP 800-162** (ABAC) for the edge predicates; OAuth 2.1 / OIDC Core 1.0 / SAML 2.0 / SCIM 2.0
(RFC 7642/3/4) / WebAuthn L2 / FIDO2 / PASETO / JWT / JWKS / DPoP (RFC 9449) for the auth surfaces. The
SpiceDB `LookupResources` reverse-index pattern is the prior art for the S8 consumer-JOIN push-down (§7.2).

---

## 15. Open questions remaining for Phase 6

- **`[OPEN — LEGAL]` (L-1):** ratify the fail-static staleness bound W (≤ revocation SLA; proposed W = 5 min).
  The structural floor ships regardless; counsel/DPO ratify the number. (C11.)
- **`[OPEN — LEGAL]` (L-4):** EU AI Act classification of the agent-governance / delegation labelling carried
  on `agent_governance` (Phase 3 §15) — engineering posture: always-labelled-as-agent + the AI-Act label
  field ships; the *classification* is counsel's.
- **`[OPEN — LEGAL]` (GD-5):** audit-log retention carve-out for authz decisions — how long `iam.*` decision
  records are retained vs erased; flagged to DPO. Identity's residual erasure contribution is the
  pseudonym-map shred (DSR step-1); the rest is the platform posture (00-reconciliation §X-7), not restated
  here.
- **S8 cardinality cap & the `Ids`-vs-`Filter` threshold** — the boundary at which `list_objects` switches
  from materialise (`Ids`) to push-down (`Filter`) is a **measured tunable, not a contract constant**
  (EI-02 §8, measure-not-predict). Phase 6 picks the default-to-beat and the per-subsystem override; the
  *shape* is frozen (C1), only the threshold is open.
- **S8 freshness SLO (`reverse_index_lag`)** — the acceptable lag between an `iam.tuple_written` and its
  reflection in S8 before a zookie-stamped scan must fall back to `check`. A measured tunable bounded above by
  the revocation SLA (W); Phase 6 sets it against the D7 drill.
- **Multi-cell principal authority (SC-2/SC-3)** — single-home-cell is v1; the cross-cell read-through model
  (home-cell-authoritative + cross-cell coarse-grant read-through over the OQ-I bridge, zookie-bounded) is the
  named multi-cell floor, designed-not-built. The deepest remaining unknown; carried to Phase 6+.

---

## 16. Cross-references

- Refined surface: [`contract-index.md`](./contract-index.md) §4 (Identity & access) — the frozen build-to
  contracts this doc realises.
- Reconciliation rationale: [`00-reconciliation-decisions.md`](./00-reconciliation-decisions.md) §OQ-E (the
  `list_objects` push-down + `CaveatContext`), §X-1/§X-7 (the `approve_untrusted_ci` relation + pseudonym
  shred), §1 (the per-system Identity punch list).
- Phase-3 base (refined here): [`../03-shared-systems-architecture/identity-and-access.md`](../03-shared-systems-architecture/identity-and-access.md).
- Spine: ADR-03, ADR-17, ADR-08, ADR-11, ADR-12, ADR-13, ADR-16;
  [`../02b-doctrine-integration/integration-directives.md`](../02b-doctrine-integration/integration-directives.md)
  (ID-1..ID-4, GD-3, X-1..X-5, AG-2/AG-5, CI-1).
- Doctrine: EI-02 §1 (tenant-first / IDOR), §2 (one principal), §3 (identity depends on nothing), §8
  (measure before shard), §9 (three-surface topology), §10 (fail-static); EI-04 §1 (erasure-vs-immutability).
- Sibling Phase-5 docs consuming this: **Search** (the `list_objects` `Filter` conjoin), **Reference Graph**
  (backlink filtering + projection-API permission checks), **Event Bus** (authz events via outbox; feeds S8),
  **Agent Fabric** (`delegation` / `EffectApi` / `mint_run_token`), **GDPR/Audit** (pseudonym lever, DSR
  fan-out, S8 as a holder), **Storage** (KMS / per-subject DEK), **Git/CI** (the X-1 check seam +
  `approve_untrusted_ci` + `!is_untrusted_fork`).
```