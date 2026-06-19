//! # `myelin-identity` — the frozen Identity & Access contract surface (the ABI)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! (§0 the change list C1–C12, §3 the polymorphic `Principal`, §7.1 the frozen
//! `ListObjectsResult` / `SetExpr` / `ColRef`, §8.6 the frozen `CaveatContext`,
//! §11.1 the full exposed-contract table).
//!
//! **Contract-index cluster:** §4 — Identity & access
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows
//! 4.1 `authenticate`, 4.2 `check`, 4.3 `list_objects`, 4.4 `list_subjects`/`explain`,
//! 4.5 `delegation`, 4.6 `write_tuples`, 4.7 `mint_run_token`/`revoke`,
//! 4.8 `resolve_pseudonym`/`erase`, 4.9 the ReBAC namespace fragment, 4.10 `Consistency`,
//! 4.11 the `FailStatic` bound).
//!
//! ## What this crate is (P-ID-01 → P-022)
//! This crate is a **compile-time contract carrier** (ledger-overview §6, ADR-13): it
//! ships the *frozen call-surface ABI* — the eleven owned contract signatures, the
//! `SetExpr` set algebra (byte-exact to §7.1), and the `CaveatContext` (byte-exact to
//! §8.6) — as types + trait method signatures with **no algorithm bodies**. Consumer
//! crates compile against these shapes today; the bodies arrive in Identity M1. This is
//! the M0 ratchet (EI-01 §5): any drift in a signature here breaks every consumer's build
//! *now*, never silently.
//!
//! **P-ID-01 ships NO service, NO algorithm, and NO event tokens.** The `iam.*` event
//! tokens + their `EventEnvelope` projections are P-ID-02 (P-023) — see [`iam_events`].
//!
//! ## What P-ID-02 (P-023) adds: the `iam.*` tokens + their envelope projections
//! [`iam_events`] registers the three `iam.*` event tokens (architecture §11.2:
//! [`iam_events::IAM_TUPLE_WRITTEN`], [`iam_events::IAM_ROLE_GRANTED`],
//! [`iam_events::IAM_BREAK_GLASS`]) and their `EventEnvelope` projections
//! ([`iam_events::IamEventProjection`]) with **opaque-`principal_id`-only attribution** — the
//! erasable `profile_ref` never enters the immutable envelope, so the GDPR
//! erasure-vs-immutability split (EI-04 §1) is baked into the shape at M0. It also declares
//! the §1.8 telemetry signal-name constants Identity owns ([`iam_events::signals`]). The
//! projection is expressed as an identity-owned compile-time descriptor (NOT an
//! `myelin_events::EventEnvelope` import) because identity is a DAG sink — see the
//! DAG-deviation note in [`iam_events`]. **Floor:** the emit bodies land in M1
//! (`iam.tuple_written` → P-ID-08); the Bus taxonomy seed validator is EB-02 / P-042.
//!
//! ## Reconciliation with the P-001 substrate skeleton
//! P-001 stood up a partial skeleton of this surface (`Principal{id, kind, tenant}`, a
//! placeholder `SetExpr(String)`, a three-method `AuthzClient`). P-ID-01 (this prompt) is
//! the FREEZE prompt named in the architecture: it EXTENDS that skeleton in place to the
//! full §11.1 ABI — it does not duplicate it. The `Principal` field `id` is renamed to the
//! frozen `principal_id` and grows the three frozen governance fields
//! (`region`/`data_role`/`status`); `SetExpr` becomes the real §7.1 algebra; `AuthzClient`
//! is widened to the eleven-method [`IdentityService`] (the old name is preserved as a
//! deprecated alias so no consumer breaks on the rename). The ~31 pre-existing fixtures
//! that built `Principal{id, kind, tenant}` move to [`Principal::stub`].
//!
//! ## DAG position (§2.9): identity is a SINK
//! Identity depends on **`myelin-tenancy` only** (for `TenantId` / `Region` /
//! `ArtifactRef`). It does NOT depend on `myelin-events`, `myelin-gdpr`, etc. The
//! `data_role` discriminant on `Principal` is therefore Identity's *own* enum
//! ([`DataRole`]) rather than a re-export of `myelin_events::DataRole` (which sits ABOVE
//! identity in the DAG) — the two are kept name-aligned by the §2.1 / §11.1 reconciliation
//! and the round-trip drift test below; an events ⇄ identity bridge is a wiring concern of
//! the consumer, not a crate edge from the sink.
//!
//! ## Floors named (frozen shape now → bodies in a later prompt)
//! Every trait method body is `todo!()` / a fail-closed default. The SHAPES are frozen
//! here (P-ID-01 / P-022); the Identity roadmap fills the bodies:
//! - `authenticate` + machine-identity resolution (4.1) → P-ID-06 / P-ID-07 (M1).
//! - `check` literal-only caveat (4.2) → P-ID-09 (M1); the full `QueryAst` caveat core
//!   → P-ID-22 (M2).
//! - `list_objects` + the `SetExpr` push-down / authz reverse index (4.3) → P-ID-11 /
//!   P-ID-12 (M1). The single most load-bearing inter-system contract.
//! - `list_subjects` / `explain` (4.4) → P-ID-13 (M1).
//! - `delegation` (4.5) → P-ID-17 (M1).
//! - `write_tuples` / zookie + the outbox emit path (4.6) → P-ID-08 (M1).
//! - `mint_run_token` / `revoke` (4.7) → P-ID-18 / P-ID-14 (M1).
//! - `resolve_pseudonym` / `erase` (4.8) → P-ID-19 / P-ID-20 (M1).
//! - the ReBAC namespace-fragment `admit` (4.9) → the engine P-ID-10 (M1) + the five
//!   fragments (P-ID-24/26/27/29/30, closed by P-ID-30).
//! - the `FailStatic` bound (4.11) → `myelin-substrate` `FailStatic<T>` (P-S18) wired to
//!   Identity's `W` (the `[OPEN — LEGAL]` L-1 number; the structural bound ships now).

pub mod iam_events;

pub use iam_events::{
    signals, IamEventProjection, IamSubjectRef, IAM_BREAK_GLASS, IAM_EVENT_TOKENS,
    IAM_ROLE_GRANTED, IAM_TUPLE_WRITTEN,
};

use myelin_tenancy::{ArtifactRef, Region, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ===========================================================================
// §3 — the polymorphic Principal model (contract 4.1)
// ===========================================================================

/// The region a resolved principal is pinned to (contract 4.1: `authenticate` returns
/// `Principal{tenant, region, ...}`). Re-exported from `myelin-tenancy` so the envelope's
/// `actor`/`region` threading reads `myelin_identity::PrincipalRegion`; the value type
/// lives in the DAG sink.
pub type PrincipalRegion = Region;

/// Opaque, stable principal id (contract 4.1; §3). PII-free attribution id — events / git
/// / audit attribute by **this** while the erasable `profile_ref` lives separately (the
/// GDPR erasure-vs-immutability split, EI-04 §1). Never a name/email
/// (`control-plane-pii-free`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalId(pub String);

/// Opaque reference to an agent runtime instance (§3 — the agent's `runtime_ref`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeRef(pub String);

/// The GDPR fan-out role a principal acts under (architecture §2.1 `data_role`; ADR-12.4).
///
/// Identity owns its OWN copy of this two-variant discriminant (it is a sink — it cannot
/// import `myelin_events::DataRole`, which is above it in the §2.9 DAG). The variant names
/// are reconciled byte-identical with the events enum (EI-01 §7 — names up front); the
/// round-trip drift test below pins them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataRole {
    /// The tenant is the data controller for this principal's actions.
    Controller,
    /// The tenant acts as a processor (Art. 28) for this principal's actions.
    Processor,
}

/// The lifecycle status of a principal (contract 4.1 `Principal{..., status}`; §11 the
/// lifecycle/revocation flows). A `Suspended`/`Disabled` principal fails closed at `check`
/// (ID-D1 disabled-user → zero access).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrincipalStatus {
    /// Resolvable and able to be granted (the steady state).
    Active,
    /// Temporarily withheld (e.g. SCIM-disable in flight) — fail-closed at `check`.
    Suspended,
    /// Permanently disabled / deprovisioned — fail-closed at `check`.
    Disabled,
}

/// The principal kind (architecture §3; EI-02 §2; ADR-03). Human / Agent / Service — an
/// Agent carries its `runtime_ref` and an optional `on_behalf_of` (delegation). **Kind is
/// data, not a code branch:** `check(subject, …)` never branches on `kind`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrincipalKind {
    Human,
    Agent {
        runtime_ref: RuntimeRef,
        on_behalf_of: Option<PrincipalId>,
    },
    Service,
}

/// The ONE polymorphic principal (architecture §3; contract 4.1; EI-02 §2; ADR-03).
///
/// One record with a `kind` discriminant — not two parallel models. The discriminant
/// changes governance metadata and credential type, **never the authorization code path**.
/// `tenant` is first-class (never optional; tenant-from-credential, ID-3). `region` pins
/// residency. `principal_id` is the opaque stable attribution id; `data_role`/`status` are
/// the §2.1 / §11 governance fields.
///
/// Frozen to the §11.1 / contract-4.1 shape
/// `Principal{tenant, region, principal_id, kind, data_role, status}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub tenant: TenantId,
    pub region: Region,
    pub principal_id: PrincipalId,
    pub kind: PrincipalKind,
    pub data_role: DataRole,
    pub status: PrincipalStatus,
}

impl Principal {
    /// Construct a principal from the six frozen fields (contract 4.1). This is the
    /// constructor `authenticate` (Identity M1) returns through; until then it is the
    /// single seam fixtures and CDC consumers build a well-formed `Principal` from, so a
    /// future field addition is a *compile* break at one call shape, not a silent default.
    pub fn new(
        tenant: TenantId,
        region: Region,
        principal_id: PrincipalId,
        kind: PrincipalKind,
        data_role: DataRole,
        status: PrincipalStatus,
    ) -> Self {
        Principal {
            tenant,
            region,
            principal_id,
            kind,
            data_role,
            status,
        }
    }

    /// A minimal well-formed `Principal` for tests / CDC fixtures — `region`/`data_role`/
    /// `status` take the common-case defaults (a fixture's home region derived from the
    /// tenant, `Controller`, `Active`). Real resolution is `authenticate` (Identity M1);
    /// this exists so the pre-existing fixtures do not each have to spell six fields.
    /// NOT a production constructor — production code calls [`Principal::new`].
    pub fn stub(principal_id: PrincipalId, kind: PrincipalKind, tenant: TenantId) -> Self {
        Principal {
            region: Region(format!("{}-home", tenant.0)),
            tenant,
            principal_id,
            kind,
            data_role: DataRole::Controller,
            status: PrincipalStatus::Active,
        }
    }
}

// ===========================================================================
// §4 — authentication surface (contract 4.1)
// ===========================================================================

/// An opaque credential presented to `authenticate` (contract 4.1). Any of the v1
/// surfaces — SSO/SCIM/passkey/SSH/PAT/CI/agent/deploy-key — resolves to a `Principal`;
/// **tenant is taken from the verified credential, never the URL path** (ID-3). The
/// per-surface parsing is Identity M1 (P-ID-06/07); the carrier is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// The credential family (oidc / saml / scim / passkey / ssh / pat / ci / agent /
    /// deploy_key) — a string token, not an enum, so a new surface is an additive change.
    pub scheme: String,
    /// The opaque presented material (a token, an assertion ref, a public-key fingerprint).
    /// Never logged in the clear; never a name/email at the contract boundary.
    pub material: String,
}

// ===========================================================================
// §7.1 — list_objects return shape + the SetExpr set algebra (contracts 4.3, 4.4)
//        copied byte-exact from the frozen architecture §7.1.
// ===========================================================================

/// An object id in a consumer's id space (architecture §7.1). Minted by the object's
/// owning subsystem — Identity never invents object ids; it only stores tuples about them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub String);

/// The object type discriminant for a `list_objects` pre-filter (contract 4.3; §7.3 — the
/// five id columns: `pr`/`repo`, `run`, `issue`, `database_row`, `channel`/`message`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectType(pub String);

/// A relation name in the ReBAC namespace (architecture §5/§7.1 — e.g. `reader`,
/// `protected_push`, `watcher`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelName(pub String);

/// A permission token checked against an object (contract 4.2). A namespace permission is a
/// computed userset over relations (§5); the carrier is opaque at the ABI boundary.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission(pub String);

/// Names the **consumer's OWN id column** the `Filter` lowers a predicate / JOIN over
/// (architecture §7.1 `ColRef = { table, column }`; §7.2 the no-N+1 lowering). Byte-exact.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColRef {
    /// The consumer table (e.g. `"issue"`, `"pr"`, `"channel"`).
    pub table: String,
    /// The consumer's own id column (e.g. `"id"`).
    pub column: String,
}

/// An opaque reference to a server-materialised authz reverse-index tuple set the consumer
/// JOINs against — the S8 `(subject, relation, object_id)` projection, the big-result path
/// (architecture §7.1 `TupleSet { index: AuthzIndexRef }`, §7.2 the JOIN against
/// `authz_visible`). The materialisation + watermark land in P-ID-11/P-ID-12 (M1); the
/// reference shape is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthzIndexRef(pub String);

/// The tenant-scoped, monotone set algebra over the object-id space (contract 4.3; the
/// crux of the C1/OQ-E push-down). **Copied byte-exact from architecture §7.1** — the
/// variant set is the wire contract; the `myelin-query` compiler lowers it to a SQL
/// predicate / JOIN over the consumer's own `via_column`/[`ColRef`]. No N+1, no post-filter.
///
/// **Floor:** the *lowering* (to SQL) and the S8 JOIN target + zookie watermark are
/// Identity M1 (P-ID-11/P-ID-12); the *algebra shape* is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetExpr {
    /// Subject sees every object of this type in the tenant (e.g. admin).
    All,
    /// Subject sees nothing (deny) — the consumer adds `WHERE false`.
    None,
    /// An explicit allow-set, inlined when small (`WHERE id IN (...)`).
    Ids(Vec<ObjectId>),
    /// An explicit deny-set over an otherwise-visible space (`WHERE id NOT IN (...)`).
    NotIds(Vec<ObjectId>),
    /// Objects where this id is the object of `relation` for the subject — a JOIN against
    /// S8 keyed by the consumer's own `via_column`.
    InRelation {
        relation: RelName,
        via_column: ColRef,
    },
    /// Boolean composition → `AND` / `OR` / `EXCEPT` (architecture §7.2).
    Union(Vec<SetExpr>),
    Intersect(Vec<SetExpr>),
    Difference(Box<SetExpr>, Box<SetExpr>),
    /// A server-materialised tuple set the consumer JOINs against (the big-result path).
    TupleSet {
        index: AuthzIndexRef,
    },
}

/// The leak-free pre-filter result (contract 4.3) — a materialised id set (the S4 path) OR
/// a pushdownable `Filter` (the S8 path). Copied byte-exact from architecture §7.1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListObjectsResult {
    /// Small sets: materialise (default under a cardinality cap; the S4 path).
    Ids { ids: Vec<ObjectId>, zookie: Zookie },
    /// Large/unbounded: push down (the S8 path).
    Filter { set_expr: SetExpr, zookie: Zookie },
}

/// The Zanzibar Expand result for `list_subjects` (contract 4.4) — the subject userset
/// tree at the zookie's snapshot, served at 50k-member density by S8 (C8). The node shape
/// (computed-userset rewrites) is filled in P-ID-13 (M1); the carrier is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectTree {
    /// The object the tree expands `permission` over.
    pub object: ObjectId,
    /// The relation/permission expanded (e.g. `watcher`).
    pub relation: RelName,
    /// The leaf subjects + nested usersets (the rewrite tree). Frozen as an opaque-leaf
    /// list at M0; the recursive userset node shape lands with the engine (P-ID-13).
    pub members: Vec<PrincipalId>,
    pub zookie: Zookie,
}

/// The `explain(...)` userset-rewrite trace (contract 4.4) — why a decision held, for the
/// admin inspector / HITL. The structured trace nodes land in P-ID-13 (M1); the carrier is
/// frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteTrace {
    /// A human-readable, structured-later explanation of the rewrite path.
    pub steps: Vec<String>,
}

// ===========================================================================
// §8.6 — the CaveatContext rider on check (contract 4.2), byte-exact + the safe Literal.
// ===========================================================================

/// A field id for field-level ABAC (architecture §8.6 — issue `field.view`, KN column).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldId(pub String);

/// A transition id for transition-level ABAC (architecture §8.6 — an approver-gated
/// state transition).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransitionId(pub String);

/// A literal value in a caveat's attribute map (architecture §8.6 `attrs: Map<String,
/// Literal>`). The caveat predicate reuses the safe, non-Turing-complete `QueryAst` core
/// (ADR-07 = the `EventMatcher`, contract 3.4) — **no second predicate language**. At M0
/// only the literal value space is frozen; the predicate evaluator is the literal-only
/// floor (P-ID-09) → the full `QueryAst` core (P-ID-22).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Str(String),
}

/// The field/transition ABAC context for `check` (contract 4.2; architecture §8.6).
/// Copied byte-exact: `CaveatContext { object, field?, transition?, attrs }`. Evaluated at
/// `check`-time on already-filtered rows, **off the hot `list_objects` path** (OQ-E). A
/// caveat needing missing context returns `Conditional`, never a silent allow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaveatContext {
    pub object: ArtifactRef,
    pub field: Option<FieldId>,
    pub transition: Option<TransitionId>,
    pub attrs: BTreeMap<String, Literal>,
}

/// The per-action decision (contract 4.2). Fail-closed (ADR-03). `Conditional` means a
/// caveat needs caller-supplied context — never a silent allow (§8.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Deny,
    Conditional,
}

// ===========================================================================
// §8.4 / §10 — Consistency (zookie) + the FailStatic bound (contracts 4.10, 4.11)
// ===========================================================================

/// The opaque consistency watermark (Zanzibar zookie) — returned by `write_tuples`, stamped
/// on the object, and carried on reads for read-your-writes (contract 4.6/4.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zookie(pub String);

/// Strong (read-your-writes) vs bounded-stale (fail-static-eligible) (§8.4, contract 4.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsistencyMode {
    /// Read at-or-after `at_least`; bypasses the fail-static cache; waits/falls-back rather
    /// than serving stale (the new-enemy guard, §8.7).
    Strong,
    /// Bounded-stale; may be served from the fail-static cache during an Id hiccup (§10).
    BoundedStale,
}

/// The consistency token ("zookie") for a read (contract 4.10). A zookie-stamped strong
/// read bypasses the fail-static cache; the authz reverse index (S8) honours the zookie
/// revision watermark (§8.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consistency {
    pub at_least: Zookie,
    pub mode: ConsistencyMode,
}

/// The fail-static staleness bound (contract 4.11; §10, C11). `static_max ≤ revocation SLA`
/// and `≥ agent/CI token TTL` so a revoked machine token expires inside the window. The
/// structural bound ships now; the *number* W (proposed 5 min) is `[OPEN — LEGAL]` (L-1),
/// DPO-ratified. The `FailStatic<T>` wrapper that enforces it lives in `myelin-substrate`
/// (P-S18); this is the bound value Identity hands it. Reused as the `ttl` carrier for
/// `mint_run_token` (the token TTL is bounded by W, §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailStaticBound {
    /// The maximum staleness window, in seconds. Default-to-beat: 300 (W = 5 min).
    pub static_max_secs: u64,
}

impl FailStaticBound {
    /// The proposed default-to-beat: W = 5 minutes (§10, C11). `[OPEN — LEGAL]` (L-1) — the
    /// structural bound is enforced regardless of the legal ratification of the *number*.
    pub const DEFAULT_W: FailStaticBound = FailStaticBound {
        static_max_secs: 300,
    };
}

// ===========================================================================
// §6 / §7 — write_tuples, delegation, mint_run_token (contracts 4.5, 4.6, 4.7)
// ===========================================================================

/// A relation tuple `⟨object#relation@subject⟩` (architecture §6). The only emit path is
/// the outbox (`iam.tuple_written`, P-ID-08); `expires_at` carries per-run agent grants as
/// auto-expiring tuples (revoke-on-crash defence in depth).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationTuple {
    pub object: ObjectId,
    pub relation: RelName,
    pub subject: PrincipalId,
    /// An optional ABAC caveat reference on the tuple (SpiceDB caveats; §9).
    pub caveat: Option<String>,
}

/// A tuple delta for `write_tuples` (contract 4.6) — add or remove a tuple, atomically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TupleDelta {
    Add(RelationTuple),
    Remove(RelationTuple),
}

/// A precondition for an atomic `write_tuples` (contract 4.6) — e.g. the expected current
/// zookie, so a concurrent revoke cannot be lost. The full precondition language lands with
/// the write path (P-ID-08); the carrier is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precondition {
    /// The zookie the writer expects the object to be at (read-modify-write guard).
    pub expected_zookie: Option<Zookie>,
}

/// The effective policy of an agent run after the monotone delegation intersection
/// `agent.policy ∩ delegation ∩ tenant.policy` (contract 4.5; §6, AG-2). The composed
/// algebra lands in P-ID-17 (M1); the carrier is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    /// The attenuated capability chain (macaroon/biscuit caveats), opaque at the ABI.
    pub caveats: Vec<String>,
}

/// The delegation caveats carried into a per-run token mint (contract 4.7; §6). The
/// delegating human's grant, expressed as the token's caveat chain (attenuate-only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationCaveats(pub Vec<String>);

/// A per-run attenuated capability token (contract 4.7) — life == run life, self-hosted
/// scope, re-mintable mid-workflow on resume (C6/C9). The mint (`mint_run_token`) lands in
/// P-ID-18 (M1); the carrier is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunToken {
    /// The opaque bearer material.
    pub token: String,
    /// The token's revocation id (the `jti` the denylist S7 keys on).
    pub jti: String,
}

/// A run identifier a per-run token is minted for (contract 4.7).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// The revocation target for `revoke` (contract 4.7) — a single token (`jti`) or an entire
/// principal (suspend). Idempotent even on crash (ID-D6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevokeTarget {
    Jti(String),
    Principal(PrincipalId),
}

// ===========================================================================
// §5 — the ReBAC namespace fragment admit type (contract 4.9)
// ===========================================================================

/// A per-subsystem ReBAC namespace fragment (contract 4.9) — relations + permissions a
/// subsystem declares at build time, compiled into the one cell schema. Id owns the engine
/// and never invents object ids. The fragment DSL / compiler lands with the engine
/// (P-ID-10) + the five frozen fragments (P-ID-24/26/27/29/30); the *admit* carrier is
/// frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceFragment {
    /// The object type this fragment defines (e.g. `repo`, `issue`, `channel`).
    pub object_type: ObjectType,
    /// The declared relation names (direct edges).
    pub relations: Vec<RelName>,
    /// The declared permission names (computed usersets over relations).
    pub permissions: Vec<Permission>,
}

/// The result of admitting a [`NamespaceFragment`] into the cell schema (contract 4.9) —
/// `Admitted` with the compiled fragment id, or a structured `Rejected` (a relation
/// referencing an unknown parent type, a cycle, …). The validator lands with the engine
/// (P-ID-10); the admit shape is frozen here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FragmentAdmit {
    Admitted { fragment_id: String },
    Rejected { reason: String },
}

// ===========================================================================
// The error taxonomy + Result alias
// ===========================================================================

/// The authz surface error taxonomy (frozen carrier; the full variant set lands with the
/// bodies). `Unavailable` is the fail-static trigger (§10); `FailClosed` is the
/// deny-when-genuinely-unsure posture (ADR-03).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzError {
    /// The credential / argument was malformed.
    BadRequest(String),
    /// An Id dependency was unavailable — the fail-static path decides (§10).
    Unavailable(String),
    /// Deny-when-genuinely-unsure (ADR-03) surfaced as an error rather than `Deny`.
    FailClosed(String),
    /// A not-yet-implemented body (the M0 floor). Replaced by the real taxonomy in M1.
    NotYetImplemented(&'static str),
}

/// `Result` alias for the authz surface.
pub type Result<T> = core::result::Result<T, AuthzError>;

// ===========================================================================
// §11.1 — the eleven owned trait signatures (the frozen call-surface ABI)
// ===========================================================================

/// The full Identity & Access contract surface every consumer links (contracts 4.1–4.11;
/// ADR-13.3). **No service re-implements these; everyone calls them.** All methods are the
/// canonical fail-static surface (§10) and flow through the resilient client — that wiring
/// is the substrate's, not implemented here. Bodies are `todo!()` / fail-closed defaults;
/// the SHAPES are frozen (P-ID-01 / P-022); Identity M1 fills the bodies (floors above).
///
/// The eleven owned rows map to methods as:
/// 4.1 [`authenticate`](IdentityService::authenticate) · 4.2 [`check`](IdentityService::check)
/// · 4.3 [`list_objects`](IdentityService::list_objects) · 4.4
/// [`list_subjects`](IdentityService::list_subjects) + [`explain`](IdentityService::explain)
/// · 4.5 [`delegation`](IdentityService::delegation) · 4.6
/// [`write_tuples`](IdentityService::write_tuples) · 4.7
/// [`mint_run_token`](IdentityService::mint_run_token) + [`revoke`](IdentityService::revoke)
/// · 4.8 [`resolve_pseudonym`](IdentityService::resolve_pseudonym) +
/// [`erase`](IdentityService::erase) · 4.9 [`admit_fragment`](IdentityService::admit_fragment)
/// · 4.10 [`Consistency`] (the zookie type threaded through every read) · 4.11
/// [`FailStaticBound`] (the bound Identity hands the substrate's `FailStatic<T>`).
pub trait IdentityService {
    /// 4.1 — resolve any credential (incl. machine identity) to the one `Principal`; tenant
    /// from the verified credential, never the path (ID-3). Body → P-ID-06/07 (M1).
    fn authenticate(&self, credential: &Credential) -> Result<Principal>;

    /// 4.2 — the per-action fail-closed gate; `caveat` carries field/transition ABAC
    /// evaluated here, off the hot `list_objects` path (OQ-E). Body → P-ID-09 (M1);
    /// `QueryAst` caveat core → P-ID-22 (M2).
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Result<Decision>;

    /// 4.3 — the leak-free pre-filter → `Ids | Filter{set_expr, zookie}`. The single most
    /// load-bearing inter-system contract. Body + `SetExpr` lowering → P-ID-11/12 (M1).
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> Result<ListObjectsResult>;

    /// 4.4 (Expand) — the subject userset tree at the zookie snapshot, 50k-member density
    /// via S8 (C8). Body → P-ID-13 (M1).
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> Result<SubjectTree>;

    /// 4.4 (`explain`) — the userset-rewrite trace for the admin inspector / HITL. Body →
    /// P-ID-13 (M1).
    fn explain(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> Result<RewriteTrace>;

    /// 4.5 — `agent.policy ∩ delegation ∩ tenant.policy` (monotone intersection). Body →
    /// P-ID-17 (M1).
    fn delegation(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
    ) -> Result<EffectivePolicy>;

    /// 4.6 — atomic tuple write; returns the zookie to stamp on the object; emitted via the
    /// outbox (the only emit path). Body + emit → P-ID-08 (M1).
    fn write_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
    ) -> Result<Zookie>;

    /// 4.7 — a per-run attenuated token (life == run life; self-hosted scope; re-mintable
    /// mid-workflow on resume, C6/C9). Body → P-ID-18 (M1).
    fn mint_run_token(
        &self,
        agent_id: &PrincipalId,
        run_id: &RunId,
        delegation_caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> Result<RunToken>;

    /// 4.7 — revoke a token (`jti`) or suspend a principal; idempotent even on crash
    /// (ID-D6). Body → P-ID-14 (M1).
    fn revoke(&self, target: &RevokeTarget) -> Result<()>;

    /// 4.8 — resolve the per-tenant pseudonym for a subject (grammar
    /// `<pseudonym>@<tenant>.noreply`, C5); DSR step 1. Body → P-ID-19 (M1).
    fn resolve_pseudonym(&self, subject: &PrincipalId, tenant: &TenantId) -> Result<String>;

    /// 4.8 — `PersonalDataHolder::erase` (the pseudonym-map crypto-shred lever). Body →
    /// P-ID-20 (M1).
    fn erase(&self, subject: &PrincipalId) -> Result<()>;

    /// 4.9 — admit a subsystem's ReBAC namespace fragment into the one cell schema. Body →
    /// the engine P-ID-10 (M1) + the five fragments.
    fn admit_fragment(&self, fragment: &NamespaceFragment) -> Result<FragmentAdmit>;
}

/// Deprecated alias preserved across the P-001 → P-ID-01 freeze so the P-001-era name does
/// not break any consumer that referenced the three-method skeleton. New code uses
/// [`IdentityService`] (the full eleven-method §11.1 surface).
#[deprecated(
    since = "0.0.0",
    note = "renamed to IdentityService (the full eleven-method §11.1 ABI, P-ID-01)"
)]
pub trait AuthzClient: IdentityService {}

/// **The FROZEN pseudonym grammar `<pseudonym>@<tenant>.noreply` (contract 4.8, pin C5;
/// recon §X-7; EI-04 §1).** P-ID-19.
///
/// This is the pseudonymous identity a subject is attributed under in any IMMUTABLE,
/// erasure-resistant surface — first and foremost **git commit author/email** (the bytes are
/// baked into the commit hash, so they must never contain erasable real PII, EI-04 §1). The
/// grammar is frozen **NOW**, in M1, **before** the Git data model freezes in M3 — the
/// decide-before-the-git-data-model obligation (EI-04 §1: "decide this BEFORE the git
/// subsystem's data model is fixed; it is nearly impossible to bolt on later"). Git commits
/// become pseudonymous-by-default in M3 ([P-ID-25]) by consuming THIS type.
///
/// **Why a frozen value type, not a bare `format!`:** the grammar is a cross-band contract
/// (frozen M1, consumed M3) — freezing it as a parse/format-round-tripping type in the
/// **contract crate** (the DAG sink everyone can import, never the service crate) means a
/// drift in the `@`/`.noreply` shape fails to compile at every consumer, not silently at
/// runtime in a commit's immutable bytes. The `pseudonym` token is the per-tenant opaque
/// handle the S2 map ([the service crate's pseudonym store, P-ID-19]) mints + stores under the
/// subject's per-subject key (the erasure lever); this type carries the **public, PII-free**
/// rendering of that handle — never the real identity, which lives crypto-shred-protected in
/// S2 (resolvable only by `resolve_pseudonym`, [P-ID-20]).
///
/// ## What is frozen (drift here is a compile break at every consumer)
/// - the rendering is **exactly** `<pseudonym>@<tenant>.noreply` ([`PseudonymHandle::render`]);
/// - it round-trips: [`PseudonymHandle::parse`] of a rendering recovers the same
///   `(pseudonym, tenant)` (the byte-identical conformance the Git M3 commit codec relies on);
/// - the local-part (`<pseudonym>`) and the domain label (`<tenant>`) are **opaque, PII-free
///   tokens** — the grammar carries NO real name/email (the immutable-bytes-stay-PII-free
///   invariant, EI-04 §1 / recon §X-7).
///
/// `[P-ID-25]`: Git pseudonymous-by-default commits consume this (M3 — the named follow-on).
/// `[P-ID-20]`: `resolve_pseudonym`/`erase` (the S2 read + crypto-shred) consume the S2 map.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PseudonymHandle {
    /// The per-tenant opaque pseudonym token — the local-part `<pseudonym>`. PII-free: it is
    /// the S2 map's per-tenant handle for a subject (the real identity lives in S2 under the
    /// per-subject key), never a real name/email.
    pseudonym: String,
    /// The tenant label — the `<tenant>` segment before `.noreply`. The tenant slug
    /// ([`TenantId`]); PII-free.
    tenant: String,
}

/// The frozen domain suffix of the pseudonym grammar (contract 4.8, pin C5). The rendering is
/// `<pseudonym>@<tenant>` + THIS. A `.noreply` TLD is unroutable by construction (RFC 6761-class
/// reserved use) — a pseudonymous commit email never reaches a real mailbox.
pub const PSEUDONYM_DOMAIN_SUFFIX: &str = ".noreply";

impl PseudonymHandle {
    /// Construct a pseudonym handle from a PII-free `(pseudonym, tenant)` pair (the S2 map's
    /// per-tenant handle + the tenant slug). Returns `None` if either token is empty or would
    /// break the grammar's unambiguous round-trip (a token containing `@` or `.noreply`, or a
    /// tenant containing `.`/`@`), so a malformed handle can never be minted (the immutable
    /// bytes never carry a grammar-breaking — or PII-smuggling — token).
    pub fn new(pseudonym: impl Into<String>, tenant: impl Into<String>) -> Option<PseudonymHandle> {
        let pseudonym = pseudonym.into();
        let tenant = tenant.into();
        if pseudonym.is_empty() || tenant.is_empty() {
            return None;
        }
        // The local-part must not contain `@` (the separator) so `parse` splits unambiguously.
        if pseudonym.contains('@') {
            return None;
        }
        // The tenant label sits immediately before `.noreply`; it must not contain `@` or `.`
        // (a `.` would make the `.noreply`-suffix split ambiguous, an `@` is the separator).
        if tenant.contains('@') || tenant.contains('.') {
            return None;
        }
        Some(PseudonymHandle { pseudonym, tenant })
    }

    /// The per-tenant opaque pseudonym token (the local-part). PII-free.
    pub fn pseudonym(&self) -> &str {
        &self.pseudonym
    }

    /// The tenant label (the `<tenant>` segment). PII-free.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// **Render the FROZEN grammar `<pseudonym>@<tenant>.noreply` (contract 4.8, pin C5).**
    /// This is the exact string baked into a pseudonymous git commit's author email (M3,
    /// P-ID-25) — the bytes that go into the immutable commit hash, so they carry ONLY the
    /// PII-free handle.
    pub fn render(&self) -> String {
        format!("{}@{}{}", self.pseudonym, self.tenant, PSEUDONYM_DOMAIN_SUFFIX)
    }

    /// **Parse a `<pseudonym>@<tenant>.noreply` rendering back to its `(pseudonym, tenant)`
    /// (the byte-identical round-trip the Git M3 commit codec relies on).** Returns `None` if
    /// the input does not match the frozen grammar exactly (wrong suffix, missing/extra `@`,
    /// empty token) — a non-conforming string is REFUSED, never silently coerced (so a
    /// resurrected real-PII email can never be mistaken for a valid pseudonym).
    pub fn parse(s: &str) -> Option<PseudonymHandle> {
        let local_and_domain = s.strip_suffix(PSEUDONYM_DOMAIN_SUFFIX)?;
        // Exactly ONE `@` (split into the local-part + the tenant label).
        let (pseudonym, tenant) = local_and_domain.split_once('@')?;
        // Re-validate through the constructor so parse and `new` agree on the frozen shape
        // (one source of truth for "what is a well-formed handle").
        PseudonymHandle::new(pseudonym.to_string(), tenant.to_string())
    }
}

impl core::fmt::Display for PseudonymHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen `Principal` carries the §11.1 / contract-4.1 shape
    /// `{tenant, region, principal_id, kind, data_role, status}`. Drift stops compilation.
    #[test]
    fn principal_carries_the_frozen_six_fields() {
        let p = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("p1".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: Some(PrincipalId("human".into())),
            },
            DataRole::Processor,
            PrincipalStatus::Active,
        );
        assert!(matches!(p.kind, PrincipalKind::Agent { .. }));
        assert_eq!(p.data_role, DataRole::Processor);
        assert_eq!(p.status, PrincipalStatus::Active);
        assert_eq!(p.region, Region("eu-west".into()));
        assert_eq!(p.principal_id, PrincipalId("p1".into()));
    }

    /// `DataRole`'s two variant NAMES are reconciled byte-identical with
    /// `myelin_events::DataRole` (EI-01 §7). Identity cannot import the events enum (it is a
    /// sink), so the contract is pinned by the stable serde token. If a rename diverges the
    /// two, this token check fails and the reconciliation breaks loudly.
    #[test]
    fn data_role_variant_tokens_match_the_events_reconciliation() {
        assert_eq!(
            serde_json::to_string(&DataRole::Controller).unwrap(),
            "\"Controller\""
        );
        assert_eq!(
            serde_json::to_string(&DataRole::Processor).unwrap(),
            "\"Processor\""
        );
    }

    /// Round-trip: the `SetExpr` enum and `CaveatContext` serialize/deserialize stably —
    /// the variant names + field names are the wire contract (the names/units anchor).
    #[test]
    fn set_expr_and_caveat_round_trip_stably() {
        let expr = SetExpr::Union(vec![
            SetExpr::All,
            SetExpr::None,
            SetExpr::Ids(vec![ObjectId("a".into()), ObjectId("b".into())]),
            SetExpr::NotIds(vec![ObjectId("c".into())]),
            SetExpr::InRelation {
                relation: RelName("reader".into()),
                via_column: ColRef {
                    table: "issue".into(),
                    column: "id".into(),
                },
            },
            SetExpr::Intersect(vec![SetExpr::All]),
            SetExpr::Difference(Box::new(SetExpr::All), Box::new(SetExpr::None)),
            SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
        ]);
        let json = serde_json::to_string(&expr).unwrap();
        let back: SetExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(expr, back);

        // The §7.1 variant tokens are part of the wire contract — pin a few.
        assert!(json.contains("InRelation"));
        assert!(json.contains("via_column"));
        assert!(json.contains("TupleSet"));

        let mut attrs = BTreeMap::new();
        attrs.insert("severity".to_string(), Literal::Int(3));
        attrs.insert("confidential".to_string(), Literal::Bool(true));
        attrs.insert("owner".to_string(), Literal::Str("alice".into()));
        let caveat = CaveatContext {
            object: ArtifactRef("myelin://acme/issue/issue/PROJ-1".into()),
            field: Some(FieldId("salary".into())),
            transition: Some(TransitionId("approve".into())),
            attrs,
        };
        let cjson = serde_json::to_string(&caveat).unwrap();
        let cback: CaveatContext = serde_json::from_str(&cjson).unwrap();
        assert_eq!(caveat, cback);
        assert!(cjson.contains("\"object\""));
        assert!(cjson.contains("\"field\""));
        assert!(cjson.contains("\"transition\""));
        assert!(cjson.contains("\"attrs\""));
    }

    /// `ListObjectsResult` carries its two frozen variants (`Ids`/`Filter`), each with its
    /// zookie (contract 4.3 / 4.10).
    #[test]
    fn list_objects_result_two_variants_each_with_zookie() {
        let ids = ListObjectsResult::Ids {
            ids: vec![ObjectId("x".into())],
            zookie: Zookie("z1".into()),
        };
        let filter = ListObjectsResult::Filter {
            set_expr: SetExpr::All,
            zookie: Zookie("z2".into()),
        };
        for r in [&ids, &filter] {
            let s = serde_json::to_string(r).unwrap();
            let b: ListObjectsResult = serde_json::from_str(&s).unwrap();
            assert_eq!(r, &b);
        }
    }

    /// The fail-static bound default-to-beat is W = 5 min (300 s), §10 / C11.
    #[test]
    fn fail_static_default_w_is_five_minutes() {
        assert_eq!(FailStaticBound::DEFAULT_W.static_max_secs, 300);
    }

    /// A stub implementer proves the `IdentityService` trait's eleven frozen methods take
    /// the frozen parameter types and is implementable. The bodies are the M0 floor
    /// (`NotYetImplemented`) / fail-closed (`Deny`); Identity M1 fills them. This is the
    /// CONSUMER half of the CDC pair the coverage scanner (P-S21) reads (the provider half
    /// — the real bodies — is M1); the slot existing is what the scanner checks at M0.
    #[test]
    fn identity_service_eleven_signatures_are_frozen_and_implementable() {
        struct StubId;
        impl IdentityService for StubId {
            fn authenticate(&self, _c: &Credential) -> Result<Principal> {
                Err(AuthzError::NotYetImplemented("authenticate → P-ID-06/07 (M1)"))
            }
            fn check(
                &self,
                _s: &Principal,
                _p: &Permission,
                _o: &ArtifactRef,
                _at: &Consistency,
                _cav: Option<&CaveatContext>,
            ) -> Result<Decision> {
                Ok(Decision::Deny) // fail-closed default (ADR-03); real body P-ID-09.
            }
            fn list_objects(
                &self,
                _s: &Principal,
                _p: &Permission,
                _ty: &ObjectType,
                _at: &Consistency,
            ) -> Result<ListObjectsResult> {
                Err(AuthzError::NotYetImplemented("list_objects → P-ID-11/12 (M1)"))
            }
            fn list_subjects(
                &self,
                _o: &ObjectId,
                _p: &Permission,
                _at: &Consistency,
            ) -> Result<SubjectTree> {
                Err(AuthzError::NotYetImplemented("list_subjects → P-ID-13 (M1)"))
            }
            fn explain(
                &self,
                _s: &Principal,
                _p: &Permission,
                _o: &ObjectId,
                _at: &Consistency,
            ) -> Result<RewriteTrace> {
                Err(AuthzError::NotYetImplemented("explain → P-ID-13 (M1)"))
            }
            fn delegation(
                &self,
                _a: &Principal,
                _t: &Principal,
            ) -> Result<EffectivePolicy> {
                Err(AuthzError::NotYetImplemented("delegation → P-ID-17 (M1)"))
            }
            fn write_tuples(
                &self,
                _d: &[TupleDelta],
                _pre: Option<&Precondition>,
            ) -> Result<Zookie> {
                Err(AuthzError::NotYetImplemented("write_tuples → P-ID-08 (M1)"))
            }
            fn mint_run_token(
                &self,
                _a: &PrincipalId,
                _r: &RunId,
                _d: &DelegationCaveats,
                _ttl: &FailStaticBound,
            ) -> Result<RunToken> {
                Err(AuthzError::NotYetImplemented("mint_run_token → P-ID-18 (M1)"))
            }
            fn revoke(&self, _t: &RevokeTarget) -> Result<()> {
                Err(AuthzError::NotYetImplemented("revoke → P-ID-14 (M1)"))
            }
            fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> Result<String> {
                Err(AuthzError::NotYetImplemented("resolve_pseudonym → P-ID-19 (M1)"))
            }
            fn erase(&self, _s: &PrincipalId) -> Result<()> {
                Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
            }
            fn admit_fragment(&self, _f: &NamespaceFragment) -> Result<FragmentAdmit> {
                Err(AuthzError::NotYetImplemented("admit_fragment → P-ID-10 (M1)"))
            }
        }

        let id = StubId;
        let subject = Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Service,
            TenantId("t".into()),
        );
        let at = Consistency {
            at_least: Zookie("z".into()),
            mode: ConsistencyMode::Strong,
        };

        // check returns the fail-closed default; the other ten are the named M0 floor.
        let d = id.check(
            &subject,
            &Permission("read".into()),
            &ArtifactRef("myelin://t/issue/issue/PROJ-1".into()),
            &at,
            None,
        );
        assert_eq!(d, Ok(Decision::Deny));

        assert!(matches!(
            id.authenticate(&Credential {
                scheme: "oidc".into(),
                material: "tok".into()
            }),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.list_objects(
                &subject,
                &Permission("read".into()),
                &ObjectType("issue".into()),
                &at
            ),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.list_subjects(&ObjectId("o".into()), &Permission("watcher".into()), &at),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.delegation(&subject, &subject),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.write_tuples(&[], None),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.mint_run_token(
                &PrincipalId("agent".into()),
                &RunId("run-1".into()),
                &DelegationCaveats(vec![]),
                &FailStaticBound::DEFAULT_W
            ),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.revoke(&RevokeTarget::Jti("j".into())),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.resolve_pseudonym(&PrincipalId("p".into()), &TenantId("t".into())),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.erase(&PrincipalId("p".into())),
            Err(AuthzError::NotYetImplemented(_))
        ));
        assert!(matches!(
            id.admit_fragment(&NamespaceFragment {
                object_type: ObjectType("issue".into()),
                relations: vec![RelName("reader".into())],
                permissions: vec![Permission("view".into())],
            }),
            Err(AuthzError::NotYetImplemented(_))
        ));
    }

    /// **The frozen pseudonym grammar renders EXACTLY `<pseudonym>@<tenant>.noreply` (contract
    /// 4.8, pin C5).** This is the byte-for-byte string baked into a pseudonymous git commit
    /// (M3, P-ID-25) — drift in the `@`/`.noreply` shape fails THIS test (and every consumer's
    /// compile, since the grammar is a frozen type).
    #[test]
    fn pseudonym_grammar_renders_the_frozen_shape() {
        let h = PseudonymHandle::new("anon-7f3a", "acme").expect("a well-formed handle");
        assert_eq!(
            h.render(),
            "anon-7f3a@acme.noreply",
            "the frozen grammar is `<pseudonym>@<tenant>.noreply`"
        );
        // Display agrees with render (one rendering).
        assert_eq!(h.to_string(), "anon-7f3a@acme.noreply");
    }

    /// **The grammar round-trips: parse(render(h)) == h (the byte-identical conformance the
    /// Git M3 commit codec relies on).** A rendering parses back to the same `(pseudonym,
    /// tenant)`; this is the cross-band guarantee that lets Git store the rendered email and
    /// recover the handle.
    #[test]
    fn pseudonym_grammar_round_trips() {
        let h = PseudonymHandle::new("anon-7f3a", "globex").unwrap();
        let parsed = PseudonymHandle::parse(&h.render()).expect("the rendering parses back");
        assert_eq!(parsed, h, "parse(render(h)) == h");
        assert_eq!(parsed.pseudonym(), "anon-7f3a");
        assert_eq!(parsed.tenant(), "globex");
    }

    /// **A non-conforming string is REFUSED (never silently coerced).** A wrong suffix, a
    /// missing/extra `@`, or an empty token must NOT parse to a fabricated handle — otherwise a
    /// resurrected real-PII email could be mistaken for a valid pseudonym. This pins the
    /// fail-loud parse the immutable-bytes-stay-PII-free invariant depends on.
    #[test]
    fn pseudonym_grammar_refuses_non_conforming() {
        // Wrong domain suffix (a routable real email, NOT a `.noreply` pseudonym).
        assert!(PseudonymHandle::parse("alice@acme.com").is_none());
        // Missing the `@` separator.
        assert!(PseudonymHandle::parse("anon-acme.noreply").is_none());
        // Two `@` — ambiguous local-part (a real `user@host` smuggled in).
        assert!(PseudonymHandle::parse("a@b@acme.noreply").is_none());
        // Empty pseudonym / empty tenant.
        assert!(PseudonymHandle::parse("@acme.noreply").is_none());
        assert!(PseudonymHandle::parse("anon@.noreply").is_none());
        // The constructor refuses a tenant with a `.` (would make the `.noreply` split
        // ambiguous) and a pseudonym carrying an `@`.
        assert!(PseudonymHandle::new("anon", "ac.me").is_none());
        assert!(PseudonymHandle::new("a@b", "acme").is_none());
        assert!(PseudonymHandle::new("", "acme").is_none());
        assert!(PseudonymHandle::new("anon", "").is_none());
    }
}
