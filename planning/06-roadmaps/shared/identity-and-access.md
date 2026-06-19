# Phase 6 — Roadmap: Identity & Access (`myelin-identity`)

> Phase: `06-roadmaps/shared`. The detailed sequenced roadmap for the **identity-and-access** shared system.
> Slots into the master sequencing bands M0..M6:
> [`../00-master-sequencing.md`](../00-master-sequencing.md) (§1 ordering thesis Tier 4 "the dependency root,
> fail-static", §2 bands, §3 critical-path/DAG, §4 the gate invariant, §5 name-your-floors). Frozen
> architecture (this roadmap SEQUENCES, it does not redesign):
> [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md)
> (the refined Id architecture, C1..C12) + the refined
> [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
> §4 (the contracts Id owns) + §1/§2/§10/§11/§12 (the contracts Id consumes). Drills owed:
> [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> §4.2 (ID-D1..ID-D9) + the F1/F2/F7/F8/F9 families + the cross-owner instances that ride Id (REF-D1/D2/D6,
> SRCH-D1/D2/D3, NOTIF leak, GIT-D8/D11, ISS-D3, KN-D13, CI-D10, CP-D2/D3). Doctrine:
> [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability; §3 prove-it-or-it-isn't-real; §5 the committed gates; §1 name-your-floors,
> code-wins-over-docs) and
> [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md) §1
> (erasure-vs-immutability — Id owns the pseudonym-map shred half). Spine: ADR-03 (ReBAC pre-filter), ADR-17
> (fail-static), ADR-08 (agent fabric delegation), ADR-11 (cells), ADR-12 (GDPR). Date: 2026-06-19.
>
> **The shape of this system, and what that means for sequencing.** Identity is the **dependency root of the
> whole platform** (EI-02 §3: "identity depends on nothing"; master-sequencing Tier 4). Two consequences flow
> from that single fact and dominate this roadmap:
> 1. **Id lands almost entirely in M1, and almost nothing on the platform can be claimed done before it is.**
>    Every read path calls `check`/`list_objects`; every gateway calls `authenticate`; every agent run calls
>    `delegation`/`mint_run_token`; every board/list/search/unfurl pushes down a `SetExpr` `Filter`. So Id is
>    on the **critical path** (master §3.1) and its M1 exit gate (ID-D3 cross-tenant 0, ID-D2 fail-static,
>    ID-D1 disabled-in-5-min) is a hard go/no-go for the entire reactive layer (M2) and everything above it.
> 2. **Id consumes almost nothing** — only the outbox (to emit `iam.tuple_written` and feed S8), the KMS
>    hierarchy (per-subject DEK = the erasure lever), the `PersonalDataHolder` spine, and the control-plane
>    `(tenant, region)` injection. It is *below* Refs, Search, Agent, Notif, GDPR. This is why it can be built
>    so early: its upstreams are only the M0 substrate + the M1 storage/tenancy floor it co-lands with.
>
> The corollary that orders the work *inside* Id: its two cardinal invariants — **no cross-tenant leak ever**
> (F2) and **the leak-free pre-filter is correct at scale** (F1, the `list_objects` `SetExpr` push-down) — are
> not features layered on later; they are the reason Id exists, drilled the moment `check`/`list_objects` exist.
> The namespace **engine** is M1; the per-subsystem namespace **fragments** light up incrementally across
> M2/M3/M4 as each subsystem ships; multi-cell principal authority is the **M5** named floor follow-on.

---

## 0. Where Identity lands in the master bands (the one-paragraph map)

Identity's **core build is M1** (master-sequencing Tier 4: the dependency root, fail-static). But Id is
**named and partially shipped in M0**: four of the twelve committed lints are Id-relevant (`tenant-predicate`,
`no-untagged-personal-data` target Id's stores; `residency-pin`, `control-plane-pii-free` constrain its
partition keys), and the `EventEnvelope` actor/subject fields (2.1) and the `myelin-identity` glue crate
skeleton (ADR-01) are frozen in M0 so consumers compile against Id's contracts before Id's bodies exist. In
**M1** the whole Id surface lands: `authenticate`, `check` + `CaveatContext`, the load-bearing `list_objects`
`SetExpr` push-down + the S8 authz reverse index, `write_tuples`/zookie, `mint_run_token`/`revoke`,
`delegation`, `resolve_pseudonym`/`erase`, the fail-static cache, and the ReBAC **engine** + the core
org/team/project hierarchy. The per-subsystem ReBAC **namespace fragments** are an **incremental** story: the
Git fragment lands with Git in **M3**, the CI/Issues/Chat fragments with their subsystems in **M4**, the KN
fragment in **M3** — but the engine and the contract that admits them are M1. Id's **world-scale hardening +
the multi-cell floor follow-on are M5** (the 30× authz-surge ID-D9, multi-cell principal authority over the
OQ-I bridge). Id participates in every M5 whole-system E2E scenario (it is the authz spine of all four) and in
the M6 dogfood.

The honest progression: **first runnable** = early M1 (`authenticate` + `check` + `write_tuples` on a single
tenant, one hard-coded namespace — an agent can be told "who is this, may they do this"); **first useful** =
late M1 (the `list_objects` `SetExpr` push-down green + fail-static + the org/team/project hierarchy, so a real
subsystem can build a leak-free board on it); **production-hardened** = M5 (the 30× authz surge holds with the
human lane protected, the S8 reverse index is the proven first replica, multi-cell principal authority
designed-and-floored, restore-resurrects-no-authority green at cell scale).

---

## 1. The contracts Identity owns / consumes, mapped to the milestone they land in

From contract-index §4 (owned by Id) + §1/§2/§10/§11/§12 (consumed). "Lands" = the milestone by which the
contract must be implemented or callable for the gate that depends on it to be green. A floor is named inline
and tracked in §6.

### 1.1 Owned by Identity (contract-index §4) — every other system consumes these

| # | Contract | Lands | Notes / floor |
|---|---|---|---|
| 4.1 | `authenticate(credential) → Principal{tenant, region, principal_id, kind, data_role, status}` — tenant from credential, never URL path | **M1** | v1 floor (named): OIDC + SAML + SCIM + passkeys + SSH + the three token types (PAT/CI/agent) + machine-identity (deploy-key/per-job). Hardware-attested device binding + full passkey-sync governance + SAML SLO are the **named P5/P6 follow-on** (arch §4); SCIM deprovision is the authoritative revocation path. |
| 4.2 | `check(subject, permission, object, zookie?, caveat?: CaveatContext) → Allow\|Deny\|Conditional` — fail-closed; `CaveatContext` does field/transition ABAC **off the hot `list_objects` path** | **M1** (engine + `CaveatContext` shape); **caveat *instances* M3/M4** | the engine and the `CaveatContext{object, field?, transition?, attrs}` shape are M1; the specific field/transition caveats (Issues `field.view`, KN column hiding) land with those subsystems (M3/M4). Caveats reuse the one `QueryAst` predicate core (3.4, frozen M2) — so the **caveat evaluator's full predicate surface depends on `myelin-query` (M2)**; M1 ships the engine with a minimal literal-only predicate floor. |
| 4.3 | `list_objects(subject, permission, type, zookie?) → Ids{ids, zookie} \| Filter{set_expr, zookie}` — the `SetExpr` push-down lowered to a SQL predicate/JOIN over the consumer's own id column via S8. **The single most load-bearing inter-system contract.** | **M1** | the crux of the whole system. Floor (named): the `Ids` materialise path (S4) ships first (small bounded sets); the `Filter`/`TupleSet` JOIN path against S8 lands in the same M1 band but is gated on S8 existing. The `Ids`↔`Filter` cardinality cap is a **measured tunable** (M1 default-to-beat, re-measured M5 at scale) — the *shape* is frozen, only the threshold is open. |
| 4.4 | `list_subjects(object, permission, zookie?) → SubjectTree` + `explain(...) → RewriteTrace` — performant at 50k-member density via S8 | **M1** (engine); **density proven M2** | the `watcher`-relation read-fanout (Notif's ambient-unread) needs the `watcher` relation declared, which is per-subsystem (M2+); the engine + S8 service it in M1. 50k-member density is proven when Chat/Notif consume it (M2/M4). |
| 4.5 | `delegation(agent, trigger_actor) → EffectivePolicy = agent.policy ∩ delegation ∩ tenant.policy` (monotone, macaroon caveats) | **M1** | the security floor that makes "an agent can do what no human role can" structurally impossible. Consumed by `EffectApi` (Agent fabric, M2) — so the delegation algebra must be **green before M2's AG-D5/ID-D5 adversarial-delegation drill**. |
| 4.6 | `write_tuples([Δtuple], precondition?) → zookie` — atomic; returns the zookie to stamp on the object; emitted via outbox; feeds S8 | **M1** | the only tuple-write path; emits `iam.tuple_written` through the outbox (so it depends on the M0 outbox). S8 consumes exactly these events. |
| 4.7 | `mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` + `revoke(jti\|principal_id)` — per-run attenuated; life == run life; callable mid-workflow on resume; **self-hosted-runner token scoped to one tenant's `SelfHosted` jobs** | **M1** | consumed by Agent (M2), CI dispatch (M4), workflow (M2). The mid-resume re-mint is needed for multi-day HITL (Workflow M2 durable signal). The self-hosted-runner scope is proven by CI-D10 (M4). |
| 4.8 | `resolve_pseudonym(subject, tenant)` + `erase(subject)` — the pseudonym-map shred (DSR step 1); **pseudonym grammar `<pseudonym>@<tenant>.noreply` (frozen)**; Git commits pseudonymous-by-default | **M1** (the shred mechanism + grammar); **Git's pseudonymous commits M3** | Id owns the **identity half** of erasure-vs-immutability (EI-04 §1): the pseudonym-map (S2) shred + per-subject DEK destroy. The Git data model that *consumes* pseudonymous-by-default commit identity is M3, but the grammar + the shred must be frozen in M1 (decided **before** the git data model freezes, EI-04 §1). |
| 4.9 | Per-subsystem ReBAC namespace fragment — each declares relations + permissions, compiled into one cell schema | **M1** (engine + admit-contract + org/team/project core); **fragments M3/M4** | the **engine** + the core hierarchy namespaces are M1; the **fragments** land with their subsystems: Git (+`approve_untrusted_ci`) and KN (page-tree-overrides) in **M3**; CI (+`read & !is_untrusted_fork`), Issues (field/transition caveats), Chat (`channel.read`) in **M4**. The `watcher` relation per watchable type is declared as each watchable subsystem lands. |
| 4.10 | `Consistency`/zookie semantics — read-your-writes; zookie-stamped reads bypass fail-static; S8 honours the revision watermark | **M1** | the new-enemy guard (Zanzibar §2.4.4) realised through the S8 watermark. Proven by ID-D7. |
| 4.11 | `FailStatic` bound (Id usage) — `static_max ≤ revocation SLA` ≥ agent-token TTL; W = 5 min default-to-beat | **M1** | the structural bound ships M1 and is enforced regardless; **`[OPEN — LEGAL]` (L-1): DPO ratifies the number W** (decision-shaped, EI-01 §8 — a sketch + human sign-off, not autonomous). The engineering floor does not wait on ratification. |
| (1.8) | telemetry — `auth_decision_latency`, `cache_hit_ratio`, `staleness_age`, `revocation_lag`, `tuple_write_lag`, **`reverse_index_lag`** (S8 freshness) | **M1** | every Id drill asserts against this signal set; no signal = failed drill (EI-01 §3, observability is part of the pass condition). `reverse_index_lag` is the S8-freshness SLO the D7 watermark fallback reads. |

### 1.2 Consumed by Identity — the (short) upstream dependency list (contract-index §1/§2/§10/§11/§12)

Identity is the dependency root, so it consumes very little — this is what makes it buildable so early.

| # | Consumed contract | From | Must be green by | Why Id blocks on it |
|---|---|---|---|---|
| 1.1/1.2/1.3 | `serve(AppSpec)` + three-surface (public/internal/metrics) + liveness≠readiness | **substrate (M0)** | **M0** | the service shell Id boots from; the internal-RPC surface is how every other service calls `check`/`list_objects`. |
| 1.4 | `PersonalDataHolder` auto-registration | **harness (M0)** | **M1** | every Id store (S1 principals, S2 pseudonym-map, S3 tuples, **S8 reverse index — NEW holder**) auto-registers so the holder list is exhaustive before real data (10.1). |
| 1.6 | the architecture lints (`tenant-predicate`, `no-untagged-personal-data`, `residency-pin`, `control-plane-pii-free`) | **substrate/CI (M0)** | **M0** | compile-time no-cross-tenant-leak + no-untagged-PII on Id's own stores; ships in the M0 ratchet, stays green forever. |
| 1.9 | `ResilientClient` (timeout+breaker+bulkhead+jittered-retry) | **substrate (M0)** | **M0** | Id is called by everyone; callers wrap Id calls in `ResilientClient` so an Id hiccup degrades (the fail-static partner). |
| 1.10 | `FailStatic<T>` bounded-staleness cache primitive | **substrate (M0)** | **M0** | S6 (the fail-static cache) is built on this primitive; the bound is Id-owned (4.11). |
| 2.1 | `EventEnvelope` (actor/subject/contains_personal_data fields) | **Bus (M0)** | **M0** | Id's emitted events (`iam.tuple_written`/`iam.role_granted`/`iam.break_glass`) use the canonical envelope; actor attribution is by opaque `principal_id` (the erasure-vs-immutability split). |
| 2.2/2.4/2.5 | `OutboxTx::emit` + `EventHandler` template + `consumer_dedup` | **Bus (M0)** | **M0** | `write_tuples` emits via the outbox (the only path); S8 is fed by an `EventHandler` consuming `iam.tuple_written`; Id consumes subsystem events (SCIM sync, `git.repo.member_added`) to feed S3. |
| 11.1 | OLTP tier client (RLS, encrypted columns, the outbox lives here) | **Storage (M1)** | **M1** | S1/S2/S3/S8 are tenant-scoped RLS Postgres-class stores; co-lands with Id in M1. |
| 11.3 | KMS hierarchy + `KeyOrigin` (per-cell root → per-tenant KEK → per-subject DEK) | **Storage (M1)** | **M1** | S2's per-subject key is the erasure lever; per-tenant DEK encrypts S1/S3/S8. The HYOK `can_derive_plaintext_index()=false` case still works for S8 (it indexes tuples, not content). |
| 10.1 | `PersonalDataHolder{locate/export/rectify/restrict/erase}` spine | **GDPR (M1)** | **M1** | S1/S2/S8 are holders; `erase(subject)` (4.8) is Id's holder implementation; the pseudonym-map shred is DSR step 1. |
| 12.1/12.4 | `(tenant, region)` partition key + `residency_verify` | **Tenancy (M1)** | **M1** | every Id table/tuple/cache/queue/**S8 reverse index** is partitioned `(tenant, region)`; no cross-tenant/cross-region query path. Co-lands with Id in M1. |
| 12.6 | cross-cell PII-free pointer bridge frame | **control plane (M1 frame; M5 live)** | **M5** | multi-cell principal authority (home-cell-authoritative + cross-cell coarse-grant read-through) rides this bridge; designed-not-built until M5 (the deepest remaining Id unknown, arch §15 SC-2/SC-3). |

**The critical upstream dependency, stated plainly:** Id's only hard blockers are the **M0 outbox** (so
`write_tuples` can emit and S8 can be fed) and the **M1 storage/tenancy/KMS floor** it co-lands with. There is
no third system above those that Id waits on — that is the defining property of the dependency root, and it is
why Id and the M1 storage/tenancy floor are built **together** as one band.

---

## 2. The milestones (mapped to master-sequencing bands)

Each milestone names its **work**, its **entry dependency**, the **floors it ships (+ their follow-on)**, and
the **gates/drills** (quantified, from the catalogue) that must be green to call it done. The band ordering and
the gate invariant (master §4: no later band done over a red earlier gate) are binding; this roadmap refines
the work *inside* the bands, it does not re-order them.

### M0 — Id is named in the substrate (the lints, the crate, the envelope fields)

**Band:** M0 (substrate + harness + committed gates). Id ships **no service** here; it ships its *constraints*.

**Work:**
- The `myelin-identity` **glue crate skeleton** (ADR-01): the contract signatures for `authenticate`, `check`,
  `list_objects` (incl. the frozen `SetExpr` enum + `CaveatContext` struct), `write_tuples`, `mint_run_token`,
  `delegation`, `resolve_pseudonym` as compile-time contract carriers. A change to any of these breaks every
  consumer's build *now* (ADR-01) — the contract is stable before a single body exists.
- The four **Id-relevant lints** wired into the M0 ratchet (contract 1.6), each with a red-fixture (proves it
  rejects) + a green-fixture (proves it admits): `tenant-predicate` (no Id query without a `(tenant, region)`
  predicate — the no-IDOR-at-compile gate), `no-untagged-personal-data` (S1 profile PII, S2 real-identity must
  carry `#[personal_data]`), `residency-pin` (no cross-region Id read path), `control-plane-pii-free` (Id's
  events/routing carry opaque ids, never PII).
- Freeze the `EventEnvelope` (2.1) **actor/subject/contains_personal_data/data_role** fields as they pertain to
  Id-emitted events (attribution by opaque `principal_id`; the erasure-vs-immutability split baked into the
  envelope shape). Register the `iam.*` event tokens in the taxonomy (2.9).

**Entry dependency:** none beyond the M0 substrate work itself (this is the root band).

**Floors shipped (named):** none that are Id-specific beyond "contracts frozen, bodies deferred to M1" — which
is the M0 band's whole point, not a hidden floor.

**Gate to call this done (rides the M0 → M1 boundary):** the four Id lints **green with both fixtures**; the
contract-coverage scanner sees the `myelin-identity` contract rows (provider stub + consumer CDC slot). No Id
*drill* runs in M0 (there is no Id service yet) — but the lints that make whole Id bug-classes impossible to
compile are green here and stay green forever (EI-01 §5, the ratchet).

### M1 — Identity core: the dependency root, made correct and fail-static (THE Id milestone)

**Band:** M1 (Identity + storage durability + tenancy — master Tiers 4 + 5; co-lands with the Tier-1
silent-data-loss floor). **This is the milestone that defines Identity.** Almost the entire Id surface lands
here, and its exit gate is a hard go/no-go for the whole reactive layer above it.

**Work (the full Id surface):**
- **`authenticate`** (4.1) across the v1 credential set: OIDC + SAML 2.0 + SCIM 2.0 + WebAuthn/FIDO2 passkeys +
  SSH-pubkey + the three capability-token types (PAT/CI-job/agent-run) + machine-identity (repo-scoped
  deploy-key → Service principal; per-job token → Service principal, self-hosted-runner scoped to one tenant's
  SelfHosted jobs). Tenant taken from the verified credential, never the URL path (ID-3, the IDOR floor).
  Capability tokens are attenuable bearer tokens (PASETO/JWT envelope) with macaroon/biscuit caveat chains;
  DPoP sender-constrains long-lived PATs; revocation = denylist (S7) + short TTL.
- **`check`** (4.2): the depth-bounded userset-rewrite Zanzibar evaluation, memoised-per-request, fail-closed on
  genuine uncertainty, evaluated at the zookie snapshot; the three-layer cache (decision cache S6, subproblem
  cache, the Leopard set index S4). The **`CaveatContext`** rider shape is frozen and the evaluator is wired
  with a literal-only predicate floor (the full `QueryAst` predicate core is M2 — see floor below).
- **`list_objects`** (4.3) — **the load-bearing crux.** The `Ids` materialise path (S4) and the
  `Filter`/`TupleSet` JOIN path (S8) both land. The `SetExpr` set algebra lowers to a SQL predicate/JOIN over
  the consumer's own id column. **S8, the per-tenant authz reverse index**, is built here as a first-class
  derived store: the `(subject, relation, object_id)` projection of S3 + a `revision_watermark` column, fed off
  the bus by an `EventHandler` consuming `iam.tuple_written`, co-located so the consumer's own query planner
  does the conjoin (one query, no N+1, no post-filter).
- **`write_tuples`/zookie** (4.6): atomic tuple write → returns the zookie → emits `iam.tuple_written` via the
  outbox → S8 ingests with the zookie as the revision watermark. Read-your-writes through the watermark
  (4.10) — the new-enemy guard.
- **The ReBAC engine + the core namespaces** (4.9): the org → team → project hierarchy as tuples with the
  `parent_team->view` tuple-to-userset inheritance rewrite. The **engine** + the **fragment-admit contract**
  (how a subsystem declares relations + permissions compiled into one cell schema) are M1; the per-subsystem
  fragments are deferred to M3/M4 (named below).
- **`delegation`** (4.5): the monotone-intersection algebra `agent.policy ∩ delegation ∩ tenant.policy`,
  computed as attenuation-never-amplification (macaroon caveats), with the "you cannot delegate authority you
  do not have" re-check at mint. Built in M1 so the Agent fabric (M2) consumes it rather than re-implementing it.
- **`mint_run_token`/`revoke`** (4.7): per-run attenuated tokens (life == run life; `expires_at` auto-expiring
  tuples as revoke-on-crash defence-in-depth); mid-workflow re-mint on resume; the self-hosted-runner
  one-tenant scope; idempotent `revoke` (even on crash).
- **`resolve_pseudonym`/`erase`** (4.8): the pseudonym map (S2, tightest RLS, per-subject key) + the shred
  mechanism (DSR step 1). The pseudonym grammar `<pseudonym>@<tenant>.noreply` is **frozen now** because the
  Git data model (M3) must be built on it (EI-04 §1: decide before the git data model freezes).
- **The fail-static cache** (4.11, S6): availability-fails-static / correctness-stays-fail-closed. The
  bounded-staleness `{actor_active, coarse_grants}` cache; zookie-stamped reads bypass it; the bound
  `static_max ≤ revocation SLA` ≥ agent-token TTL, W = 5 min default-to-beat. S7 (the revocation list / token
  denylist). S5 (the authz read-replica — the doctrine's named first scaling need, ID-4).
- **`PersonalDataHolder` registration** for S1/S2/S3/**S8**; the per-subject DEK crypto-shred unit wired (the
  GDPR structural floor, X-7). The `iam.*` audit emission via the outbox.

**Entry dependency:** **M0 green** (the outbox so `write_tuples` can emit and S8 can be fed; the harness so Id
boots and auto-registers as a holder; the four Id lints green) **+ the M1 storage/tenancy/KMS floor co-built**
(11.1 OLTP+RLS, 11.3 KMS hierarchy, 12.1 partition key, 12.4 residency-verify, 10.1 holder spine). Id and these
are one band, built together.

**Floors shipped (each named, with its follow-on — §6 tracks them):**
- **`list_objects` `Ids` materialise path first; the `Filter`/S8 JOIN path second** (same band). Follow-on:
  none deferred past M1 — both land — but the **`Ids`↔`Filter` cardinality cap is a measured tunable** picked
  here and re-measured at M5 scale.
- **`check`'s `CaveatContext` evaluator ships with a literal-only predicate floor in M1.** Follow-on: the full
  safe `QueryAst` predicate core (3.4) is frozen in **M2**; the caveat evaluator promotes to it then. The
  *shape* (`CaveatContext{object, field?, transition?, attrs}`) is frozen now.
- **The ReBAC namespace engine ships with only the org/team/project core in M1.** Follow-on: the per-subsystem
  fragments land **M3** (Git, KN) / **M4** (CI, Issues, Chat). The engine + admit-contract are complete; the
  fragments are subsystem-owned content.
- **`authenticate` v1 credential floor** (named in arch §4): hardware-attested device binding + full
  passkey-sync governance + SAML SLO are the **P5/P6 follow-on**; SCIM deprovision is the authoritative
  revocation path in v1.
- **Single-home-cell principal authority.** Follow-on: **multi-cell** (M5, over the OQ-I bridge).
- **The fail-static bound W = 5 min is the engineering default; DPO ratification (L-1) is the `[OPEN — LEGAL]`
  follow-on** — decision-shaped, runs in parallel, the floor does not wait on it.

**Gate to call this milestone done (the M1 → M2 boundary — Id's share of it):**
- **ID-D3** (F2) — cross-tenant check/list/read via path spoof → **0 cross-tenant tuples readable**;
  `tenant-predicate` lint green. *CI.* (the silent-no-IDOR floor; nothing above Id is claimed over a red ID-D3.)
- **ID-D2** (F7) — break the Id dependency → authenticated traffic survives on the coarse fail-static cache;
  **just-revoked still denied** (zookie bypass). *CI.* (fail-static, not fail-closed — Tier 4 of the thesis.)
- **ID-D1** (F8) — SCIM-disable → **every surface (UI/API/git wire/agent) denies within N = 5 min**;
  cache + token TTL + denylist all ≤ W. *SCHED.* (the disabled-user floor.)
- **ID-D4** (F1) — confidential issue / overridden page / private channel **absent from any
  `list_objects`/search/refs** for an unauthorized viewer, **incl. the `Filter`-lowered S8 JOIN result and
  under zookie staleness** (the C1/C2 surface's leak gate). *CI.* (the leak-free pre-filter is correct.)
- **ID-D7** (F8) — revoke, immediately re-read with the post-revoke zookie → **no stale allow** (the new-enemy
  guard); assert the **S8 JOIN waits or falls back to `check`** rather than serving the stale grant. *CI.*
- **ID-D5** (F9) — adversarial delegation: agent confined to `agent.policy ∩ delegation ∩ tenant.policy`, incl.
  via a delegator who lost the right → denial + intersection proof. *CI.* (this drill *re-runs* in M2 against
  the live `EffectApi` — see M2; the algebra itself is proven here.)
- **ID-D6** (F8) — kill a run mid-flight → per-run token revoked (teardown) **and** auto-expires (`expires_at`)
  within run-life ≤ W. *CI.*
- **ID-D8** (F3) — restore to a consistent point → **no resurrected grants past an erasure**; post-restore
  re-erasure runs (rides STOR-D1/STOR-D2, the silent-data-loss floor). *SCHED.*
- **GA-D5-adjacent** — the `no-untagged-personal-data` lint **red on an untagged Id PII field**; the per-subject
  crypto-shred of the pseudonym map unrecoverable in backups (STOR-D4). *CI/SCHED.*

> Note on the gate invariant: ID-D3, ID-D2, ID-D1 are the master-sequencing **M1 → M2 hard go/no-go** for
> Identity (master §4 table). The reactive layer (M2) is not started over a red one. ID-D9 (30× surge) and the
> multi-cell drills are M5 — Id is *correct* at M1, *hardened* at M5.

### M2 — Id's namespace consumers go live; the delegation + token surface is exercised in anger

**Band:** M2 (the reactive shared layer + the safety drills). Id ships **no new core contract** here — it ships
the *first real consumption* of its M1 surface, and the `watcher` relation begins to populate.

**Work:**
- **The Agent fabric consumes `delegation`/`mint_run_token`/`EffectApi`** (M2). Id's delegation algebra is now
  exercised against the live plan-then-apply path (8.2): schema → **capability (Id `check`)** → **delegation
  (Id `delegation`)** → tenant → budget → HITL → apply. **ID-D5 re-runs here** against the real `EffectApi`
  (it was proven against the algebra in M1; now it is proven against the consumer) — this is the M2 → M3
  AG-D1/AG-D2/AG-D3 family, which *is* Id's delegation algebra observed from the agent side.
- **The `watcher` relation per watchable type** begins to be declared (4.9, C8); `list_subjects(object,
  watcher)` over the S8 reverse index serves Notif's read-fanout (the 50k-member-density case is proven as Chat
  channels land in M4, but the engine path is exercised in M2 by Notif).
- **Search conjoins the `list_objects` `Filter`** (6.1, the `search-requires-acl-filter` lint); **Refs filters
  backlinks via `list_objects`** (5.3). These are the highest-fan-in consumers of Id's load-bearing contract,
  and they drill the leak-free property from their own side (SRCH-D1/D3, REF-D1/D2) — each is an *instance* of
  Id's F1/F2 families run by another owner against Id's contract.
- **The caveat evaluator promotes from the literal-only floor to the full `QueryAst` predicate core** once
  `myelin-query` is frozen (13.3, M2). One DoS-hardened evaluation engine, no second predicate language.

**Entry dependency:** **M1 green** (Id's full surface is correct + fail-static). Plus the M2 systems that
consume it (Agent, Search, Refs, Notif) are being built in the same band.

**Floors shipped (named):** the caveat evaluator's literal-only floor is **promoted** here (its follow-on
arrives). No new Id floor opens.

**Gate (Id's share of the M2 → M3 boundary):** Id has no *own-owner* drill on the M2 exit gate, but its
contract correctness is re-confirmed through the consumers' drills that ride F1/F2/F7/F9 against it:
- **AG-D5 / ID-D5 re-run** — HITL/delegation: effect outside the ∩ denied. *CI.*
- **SRCH-D1 / SRCH-D3, REF-D1 / REF-D2, NOTIF-D4** — confidential never in any result/edge/notification incl.
  counts; cross-tenant 0 — these prove Id's `list_objects` `Filter` is leak-free **as composed by every
  consumer**. *CI.*
- **SRCH-D2 / REF-D6** — revoke + re-read with post-revoke zookie → excluded within W (the S8 watermark path,
  proven from the consumer side). *CI.*

### M3 / M4 — the per-subsystem ReBAC fragments land with their subsystems

**Bands:** M3 (producers: Git, Knowledge) + M4 (consumers: CI, Issues, Chat). Id ships **the namespace
fragment for each subsystem** as that subsystem is built — the engine + admit-contract were M1; here the
*content* arrives.

**Work — M3 (with the producers):**
- **Git fragment** (4.9, C4/C7): `repo`/`branch`/`ref`/`pull_request`/`pr_comment` with ref-glob-scoped
  relations, branch-protection as `protected_push`, **CODEOWNERS-as-relations**, and the **`approve_untrusted_ci`
  relation** (C7) the fork-endorsement gate (X-1) reads as an ordinary `check`. The `list_objects` `SetExpr`
  conjoin lights up for the PR/repo list (no N+1) — this is **GIT-D11** (partial-visibility 100k-PR list → one
  query, 0 leak, revoke reflected). **Git commits become pseudonymous-by-default** (4.8, the M1 grammar now
  consumed) — **GIT-D2** (erase a commit author → pseudonymous residual == the one platform posture).
- **Knowledge fragment** (4.9): `space`/`page`/`block`/`database_row`; page-tree inheritance **with overrides**;
  row-level ACL via `list_objects`; field-level column hiding as the `check`-time `CaveatContext` (now on the
  full `QueryAst` predicate core). Proves KN-D5/KN-D13 (confidential page/row/field 0 leak incl. COUNT;
  cross-tenant 0).

**Work — M4 (with the consumers):**
- **CI fragment** (4.9, C7): `ci_project`/`environment`/`secret`/`run`; **`secret.read` is NOT inherited** (a
  direct narrow relation — CI-1); the **`read & !is_untrusted_fork` ABAC edge** stamps `trust_tier`. The
  self-hosted-runner one-tenant token scope is proven — **CI-D10** (compromised runner bounded to its tenant's
  SelfHosted jobs; 0 cross-tenant job/secret reads).
- **Issues fragment** (4.9): `issue`/`field`/`transition` + the `confidential` exclusion userset (a confidential
  issue disappears from a normal reader's `list_objects` **by construction**); field/transition `CaveatContext`
  caveats. Proves the board `SetExpr` JOIN at 1M+ issues (ISS-D2 board <1s, ISS-D3 IDOR 0).
- **Chat fragment** (4.9): `channel.read = member + parent_project->read`; `message.view = parent_channel->read`;
  the 50k-member-channel `list_subjects(channel, watcher)` density is proven here (Notif read-fanout).
  Search-as-non-member → 0 results (rides Id's `Filter`).

**Entry dependency:** M2 green (the engine, the caveat core, the consumers' drills). Each fragment lands when
its subsystem does.

**Floors shipped (named):** the namespace-engine fragment floor from M1 is **fully promoted** by end of M4
(every subsystem's fragment exists). No new Id floor.

**Gate (Id's share of the M3 → M4 and M4 → M5 boundaries):**
- **M3:** GIT-D8 (cross-tenant repo access denied at the front door — tenant from token), GIT-D11 (the SetExpr
  leak-free partial-visibility list), GIT-D2 (pseudonymous-commit erasure residual), KN-D5/KN-D13 (KN leak +
  cross-tenant 0). *CI/SCHED.*
- **M4:** ISS-D3 (cross-tenant + confidential IDOR 0 incl. under zookie staleness), CI-D10 (self-hosted-runner
  scope), CHAT confidential-unfurl tombstone + search-as-non-member 0 results. *CI/SCHED.* These are
  *subsystem-owner* drills, but each is an instance of Id's F1/F2 families against an Id namespace fragment.

### M5 — World-scale hardening + the multi-cell principal-authority floor follow-on

**Band:** M5 (world-scale hardening + the floor follow-ons + the E2E wedge). Id's M5 work is **scale + the one
deferred floor (multi-cell)** — Id is *correct* by M1; here it is *hardened* and the deepest open case is built.

**Work:**
- **The 30× authz-surge drill** — **ID-D9** (F6): 30× agent surge on the authz hot path → human lane holds,
  agent lane sheds (429 + Retry-After), cross-tenant impact 0. The protected-human-lane shed order (1.11) on the
  authz surface. *SCHED.*
- **Multi-cell principal authority** (the named floor follow-on — arch §13/§15 SC-2/SC-3, master §5): the
  cross-cell read-through model goes live over the OQ-I PII-free pointer bridge (12.6) — **home-cell-authoritative
  + cross-cell coarse-grant read-through, zookie-bounded; resolution always cell-local** (a principal spanning
  cells is evaluated in the cell holding the object, never by pulling tuples cross-region — preserving
  no-cross-region-PII, ADR-11). This is the deepest remaining Id unknown; single-home-cell was the M1 floor.
- **S8 as the proven first replica at scale** (ID-4): the cardinality cap + `reverse_index_lag` freshness SLO
  re-measured under world-scale load; the `Ids`↔`Filter` threshold finalised.
- **ID-D8 at cell scale** — restore-resurrects-no-authority re-confirmed under world-scale load (rides STOR-D2
  at cell scale). Id participates in the cell-bulkhead drill (a fatal fault in one cell unaffects authz in
  others).
- **Id is the authz spine of all four E2E scenarios** (the M5 whole-system wedge): E2E-1 (the PR context pane
  resolves per-viewer via `check`), E2E-2 (the triage agent runs under `delegation`/`mint_run_token`), E2E-3
  (spec-to-ship lineage is permission-filtered), E2E-4 (DSAR fan-out includes the pseudonym-map shred + S8 as a
  holder). 0 leak, exactly-once, cold-reindex == live, DSAR 0 holders missed.

**Entry dependency:** M4 green (all five fragments exist; the deterministic correctness drills are green; the
single-cell floor is in place to be promoted to multi-cell).

**Floors shipped (named):** **multi-cell principal authority is promoted from designed-not-built to built**
here (the single-home-cell floor's follow-on); S8's measured tunables are finalised.

**Gate (Id's share of the M5 → M6 boundary):**
- **ID-D9 green** (the 30× surge family, master §2 M5 exit). *SCHED.*
- **The multi-cell FLOOR drills** (GA-D8 / CP-D7 / CP-D8 ride Id's cross-cell read-through — cell→cell migration
  0 loss of authority; cross-cell ref PII-free; per-cell DSR receipt incl. the pseudonym shred). *SCHED.*
- **STOR-D2 / ID-D8 at cell scale** re-confirmed (RPO/RTO under world-scale load; no resurrected authority).
- **E2E-1..E2E-4 green** with Id as the authz spine of each.

### M6 — Dogfooding: Id authorizes Myelin's own development

**Band:** M6 (Myelin hosts itself). No new Id work; Id's authz now runs on the platform's own commits, issues,
docs, channels, and CI runs.

**Work:** the Myelin team's own principals (humans + the mock dev agents) authenticate through Id; the
self-hosting CI graph mints per-job tokens through Id; the gap report / scorecard live as Myelin issues
authorized by Id's Issues fragment. A truth-up pass (EI-01 §1) confirms every Id PROVEN scorecard row rests on
a dated green artifact, not a doc claim.

**Gate:** no later-band Id gate is red (the gate invariant holds end-to-end); the self-hosting CI graph mints
and revokes Id tokens correctly on every Myelin commit.

---

## 3. The floor-then-full progressions (name each floor + its follow-on)

The discipline (VISION §3, EI-04 §4): name the floor, name the follow-on, track the gap. Every Id floor:

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **`list_objects` `Ids` materialise path** (small bounded sets via S4) | M1 | **`Filter`/`TupleSet` S8 JOIN path** (large/unbounded sets, consumer-planner conjoin) | M1 (same band, gated on S8) | a consumer list exceeds the cardinality cap, *measured* (the cap is a tunable, not a constant) |
| **`CaveatContext` evaluator, literal-only predicate floor** | M1 | **Full safe `QueryAst` predicate core** (3.4, one DoS-hardened engine) | M2 | `myelin-query` frozen (M2); the first non-literal field/transition caveat (Issues/KN) |
| **ReBAC engine + org/team/project core only** | M1 | **The five per-subsystem fragments** (Git/KN M3; CI/Issues/Chat M4) | M3/M4 | each subsystem lands and declares its fragment |
| **`authenticate` v1 credential set** (OIDC/SAML/SCIM/passkey/SSH/3 tokens/machine-identity) | M1 | **Hardware-attested device binding + full passkey-sync governance + SAML SLO** | P5/P6 (post-M5) | enterprise hardware-attestation demand; SCIM deprovision is the authoritative revocation in the interim |
| **Single-home-cell principal authority** | M1 | **Multi-cell** (home-cell-authoritative + cross-cell coarse-grant read-through over the OQ-I bridge, zookie-bounded, resolution cell-local) | M5 | cross-cell principal/rollup demand (OQ-I); the deepest Id unknown (SC-2/SC-3) |
| **S8 as the dedicated authz read-replica (ID-4 first scaling move)** | M1 | **S8 tunables finalised at scale** (cardinality cap, `reverse_index_lag` SLO) | M5 | authz QPS / list cardinality *measured* under world-scale load |
| **fail-static W = 5 min engineering default (structurally enforced)** | M1 | **DPO/counsel ratification of W** (`[OPEN — LEGAL]` L-1) | parallel (legal) | the floor ships regardless; the residual GDPR-revocation-exposure-window number is one ratified statement |
| **pseudonym-map shred (Id's half of erasure-vs-immutability)** | M1 | **Git pseudonymous-by-default commits consume the grammar; audited history-rewrite path** | M3 (consume) / M5 (rewrite) | the grammar is frozen M1 *before* the git data model (EI-04 §1); history-rewrite when a body must be expunged |

**The world-scale / hard-problem work, scheduled explicitly:** Id touches **two** of EI-04's hard problems.
(1) **Erasure-vs-immutability (§1)** — Id owns the *tractable* identity half: attribution by opaque
`principal_id` + the pseudonym-map per-subject-DEK shred (the floor is M1; it is what makes the git half
solvable, because pseudonymous-by-default commits never bake erasable PII into the immutable bytes). (2)
**World-scale authz QPS** — the highest-QPS shared system (arch §13): the floor is "scale inside a cell via the
cache hierarchy + S4/S8" (M1), measured-before-sharded (ID-4); the multi-cell follow-on is M5. Neither is left
as a "someday" — each has a named band.

---

## 4. The drills/gates owed, consolidated (quantified, source-verified)

Every Id drill from the catalogue §4.2 (ID-D1..ID-D9) + the one cross-cutting assertion arch §12 adds, with its
band, family, quantified threshold, and the green artifact (the named telemetry signal, contract 1.8) it must
emit. A row is **PROVEN** only when the drill produces a dated green artifact (EI-04 §4); until then it is
**CLAIMED**.

| Drill | Band | Fam | Quantified threshold | Green artifact (signal) | Freq |
|---|---|---|---|---|---|
| ID-D3 | M1 | F2 | cross-tenant check/list/read via path spoof → **0 cross-tenant tuples readable** | cross-tenant count 0 | CI |
| ID-D2 | M1 | F7 | break Id dep → authenticated survives on coarse cache; just-revoked still denied (zookie bypass) | fail-static ratios | CI |
| ID-D4 | M1 | F1 | confidential issue/overridden page/private channel **absent from any `list_objects`/search/refs**, incl. the S8 `Filter` JOIN + under zookie staleness | zero-escape counter | CI |
| ID-D7 | M1 | F8 | revoke → re-read with post-revoke zookie → **no stale allow**; S8 JOIN waits/falls-back (the watermark path) | zookie-watermark honoured | CI |
| ID-D5 | M1 (re-run M2) | F9 | agent confined to `agent.policy ∩ delegation ∩ tenant.policy`, incl. via a delegator who lost the right | denial counter; intersection proof | CI |
| ID-D6 | M1 | F8 | kill a run mid-flight → per-run token revoked (teardown) **and** auto-expires within run-life ≤ W | token-revocation lag | CI |
| ID-D1 | M1 | F8 | SCIM-disable → **every surface denies within N = 5 min**; cache+token+denylist ≤ W | deny-latency histogram | SCHED |
| ID-D8 | M1 (re-confirm M5 cell-scale) | F3 | restore to a consistent point → **no resurrected grants** past an erasure; post-restore re-erasure runs | re-erasure receipt | SCHED |
| ID-D9 | M5 | F6 | 30× agent surge on the authz hot path → **human lane holds, agent sheds** (429+Retry-After), cross-tenant 0 | shed-counts; authz p99 | SCHED |

**The cross-owner drills that ride Id's contracts** (run by another owner, but they prove Id's F1/F2/F7
properties as composed): SRCH-D1/D2/D3, REF-D1/D2/D6, NOTIF-D4 (M2); GIT-D8/D11/D2, KN-D5/D13 (M3);
ISS-D3, CI-D10, CHAT confidential-unfurl/search-as-non-member (M4); CP-D2/D3 (misroute 0 + residency-pin, M1,
the partition floor Id's tuples live on); GA-D5 / STOR-D4 (untagged-PII lint + pseudonym-shred-unrecoverable,
M1). Id is the authz spine of **E2E-1..E2E-4** (M5).

**The two permanent gates that touch Id:** STOR-D1/STOR-D2 (restore-verify — ID-D8 rides it; re-runs on every
change touching an Id store) and the `tenant-predicate`/`no-untagged-personal-data` lints (re-run on every
compile). Id is *correct* once ID-D3/D2/D1/D4/D7/D5/D6/D8 are green (M1); *hardened* once ID-D9 + the multi-cell
floor drills are green (M5).

---

## 5. The honest "first runnable / first useful / production-hardened" progression

- **First runnable (early M1).** `authenticate` + `check` + `write_tuples` on a single tenant against one
  hard-coded namespace, emitting `iam.tuple_written` through the outbox. An agent or a test can ask "who is
  this?" and "may this principal do this action on this object?" and get a fail-closed answer. The Leopard set
  index and S8 are stubbed-to-correct (small `Ids` sets only); no fail-static cache yet. **Named-untested
  surfaces:** the `Filter` JOIN path, the caveat predicate core, multi-credential auth.

- **First useful (late M1).** The full M1 surface: the `list_objects` `SetExpr` push-down green (a real
  subsystem can build a leak-free board on it — no N+1, no post-filter), the org/team/project hierarchy, the
  fail-static cache (Id-hiccups degrade, don't cascade), `delegation`/`mint_run_token` (an agent run can be
  authorized), the pseudonym-map shred (the erasure lever exists). ID-D3/D2/D1/D4/D7/D5/D6 green. At this point
  Id is *useful* — every M2 system (Search, Refs, Notif, Agent) can be built against it — but it is **single-cell,
  literal-only caveats, no real namespace fragments yet**, and **not surge-hardened**.

- **Production-hardened (M5).** The 30× authz surge holds with the protected human lane (ID-D9); S8 is the
  proven first replica with its tunables measured at scale; multi-cell principal authority is built over the
  PII-free bridge (the deepest open case, no longer designed-not-built); restore-resurrects-no-authority holds
  at cell scale; Id is the proven authz spine of all four whole-system E2E scenarios. Only here is Id's
  world-scale story *proven*, not *claimed*.

---

## 6. Digest

**Where Id lands:** core build is **M1** (the dependency root, master Tier 4) — co-built with the M1
storage/tenancy/KMS floor it depends on. Named (lints + crate + envelope fields) in **M0**; namespace
**fragments** light up across **M3/M4** with their subsystems; world-scale + multi-cell hardening in **M5**;
dogfood in **M6**. Id is on the **critical path** (master §3.1) and its M1 exit gate is a hard go/no-go for the
whole reactive layer above it.

**Milestones → bands:** M0 (lints + glue crate + envelope fields, no service) · **M1 (the whole Id surface:
`authenticate`, `check`+`CaveatContext`, the load-bearing `list_objects` `SetExpr` push-down + S8,
`write_tuples`/zookie, `delegation`, `mint_run_token`, pseudonym shred, fail-static, the ReBAC engine + core
hierarchy)** · M2 (first real consumption — Agent delegation, Search/Refs `Filter` conjoin, `watcher` fanout;
caveat core promoted) · M3/M4 (the five per-subsystem ReBAC fragments) · M5 (30× authz surge + multi-cell
principal authority + S8 tunables at scale + E2E spine) · M6 (dogfood).

**Floors + follow-ons:** `Ids` materialise → `Filter`/S8 JOIN (M1→M1) · literal-only caveat → full `QueryAst`
predicate core (M1→M2) · engine-only → the five namespace fragments (M1→M3/M4) · v1 credential set →
hardware-attestation/passkey-sync/SLO (M1→P5/P6) · **single-home-cell → multi-cell principal authority
(M1→M5, the deepest open case)** · S8 first-replica → tunables-at-scale (M1→M5) · W=5min engineering default →
DPO ratification (M1→parallel legal, `[OPEN — LEGAL]` L-1) · pseudonym-map shred → Git pseudonymous commits +
audited history-rewrite (M1→M3/M5, Id's half of erasure-vs-immutability).

**Critical upstream dependencies (the short list — the defining property of the dependency root):** the **M0
outbox** (so `write_tuples` emits + S8 is fed) + the **M0 harness/lints/`FailStatic` primitive/`ResilientClient`**;
and the **M1 storage/tenancy/KMS floor co-built in the same band** — 11.1 OLTP+RLS, 11.3 KMS hierarchy
(per-subject DEK = the erasure lever), 12.1 `(tenant, region)` partition key, 12.4 residency-verify, 10.1
`PersonalDataHolder` spine. Id consumes **nothing above** these — not Refs, Search, Agent, Notif, or any
subsystem. That is why Id can be, and must be, built first.

**The must-be-green-first Id gate (the M1 → M2 hard go/no-go):** ID-D3 (cross-tenant 0), ID-D2 (fail-static),
ID-D1 (disabled-user-in-5-min), ID-D4 (leak-free pre-filter incl. the S8 JOIN), ID-D7 (new-enemy/watermark) —
no reactive-layer milestone is claimed done over a red one.
