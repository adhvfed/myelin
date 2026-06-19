//! # `myelin-identity-service` — the Identity service shell (P-ID-04 → P-054)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §1 (Id is the **dependency root of the platform** — "identity depends on nothing"; the
//! internal-RPC surface is how every other service calls `check`/`list_objects`).
//!
//! **Contract-index:** rows 1.1 (`serve(AppSpec)`), 1.2 (three-surface topology), 1.3
//! (liveness ≠ readiness) — **CONSUMED / WIRED here** (owned by the harness, P-S12/P-S13/P-S14).
//!
//! ## What this crate is (the bootable shell, NOT a hand-rolled main)
//! This is the FIRST **service** in the platform: the Identity service built as an [`AppSpec`]
//! the harness wires (architecture 00 §3.1, the ONE call). [`identity_app_spec`] assembles the
//! spec; the harness ([`myelin_substrate::serve`]) runs the lifecycle around it:
//!
//! ```text
//! boot → migrate → outbox relay → consumers → the three ports → graceful drain
//! ```
//!
//! with **liveness ≠ readiness** (readiness gates on migrate-complete) and a graceful drain. The
//! internal-RPC surface is the one every other service calls `check`/`list_objects` on (§1); this
//! shell wires that surface's authorization SLOT with a **fail-closed stub** ([`FailClosedCheck`])
//! that denies *every* call until the real depth-bounded Zanzibar `check` body lands (P-ID-09).
//!
//! **No store, no algorithm yet.** This prompt ships the bootable shell with empty handler slots
//! wired for the M1 contract bodies; the slots fail closed (deny) until wired.
//!
//! ## DAG position (a documented, NAMED extension — like `myelin-harness`/`myelin-lints`)
//! This is the "every service `main.rs`" consumer the contract-index row 1.1 names. It depends on
//! the harness ([`myelin_substrate`], the root-last node) + the frozen identity contract surface
//! ([`myelin_identity`]); NOTHING in the production crate DAG depends on it. It is therefore a
//! LEAF consumer ABOVE the harness, outside the eleven-crate library DAG `crate_graph.rs` models
//! — exactly as `myelin-harness` and `myelin-lints` are leaf consumers outside it. The
//! `substrate_is_root()` invariant ("no *library* crate depends on substrate") is preserved: a
//! service main is the harness's terminal consumer, not a node in the library graph.
//!
//! ## Floors named (fail-closed stub now → the handler bodies in their own M1 prompts)
//! Every authorization decision the shell makes is the fail-closed default **`Deny`** (ADR-03 —
//! deny-when-genuinely-unsure). The contract bodies arrive in their own M1 prompts, each named:
//! - `authenticate` (4.1) → P-ID-06 (the human/SSO half — LANDED:
//!   [`authenticate::HumanSsoAuthenticator`] resolves OIDC/SAML/SCIM/passkey/SSH to the polymorphic
//!   `Principal` over S1, tenant-from-credential, `auth_decision_latency` per request) /
//!   P-ID-07 (the capability-token + machine-identity half — LANDED:
//!   [`machine_auth::CapabilityAuthenticator`] resolves PAT/CI/agent/deploy-key/per-job to the
//!   polymorphic `Principal`, tenant-from-token, monotone caveat-chain attenuation, the deploy-key
//!   repo ceiling + the self-hosted-runner one-tenant `SelfHosted` scope (C6), the DPoP-binding
//!   requirement for long-lived PATs, and the S7-denylist + TTL revocation consult — S7 store body
//!   → P-ID-14).
//! - **`check` (4.2)** → **P-ID-09 — LANDED:** [`check_engine::CheckEngine`] is the depth-bounded,
//!   memoised-per-request, fail-closed, zookie-snapshot Zanzibar userset-rewrite over the raw S3
//!   tuples (direct grants + tuple-to-userset inheritance), with the literal-only `CaveatContext`
//!   rider (full `QueryAst` core → P-ID-22). The shell's [`FailClosedCheck::check`] remains the
//!   **no-store** default (a shell with no tuple store wired still denies, fail-closed); a service
//!   instance with the S3 store wired runs the real engine behind the SAME [`CheckAuthorizer`] seam.
//! - `list_objects` (4.3) → P-ID-11 / P-ID-12 — returns `NotYetImplemented` (a leak-free
//!   pre-filter that does not yet exist must not return a permissive set; it errors loudly).
//! - the S1 principal store (the `authenticate` backing) → P-ID-05 (LANDED:
//!   [`principal_store::PrincipalStore`] — RLS-partitioned, per-subject-DEK-encrypted, PII-tagged,
//!   holder-registered; `authenticate` over it is still P-ID-06/07).
//! - the S3 tuple store + `write_tuples` (4.6) → P-ID-08.
//! - the ReBAC engine + the core hierarchy (4.9) → P-ID-10.
//!
//! The readiness-gates-on-migrate-complete property is the harness's (P-S14); this shell declares
//! Identity's migrations so the booting instance is **not-ready until they apply**.

pub mod authenticate;
pub mod check_engine;
pub mod delegation;
pub mod expand;
pub mod failstatic_cache;
pub mod list_objects;
pub mod lowering;
pub mod machine_auth;
pub mod namespace;
pub mod principal_store;
pub mod read_replica;
pub mod revocation;
pub mod reverse_index;
pub mod tuple_store;

pub use authenticate::{
    scheme, AuthTelemetry, CredentialVerifier, HumanSsoAuthenticator, IdorCounters,
    StructuralVerifier, VerifiedAssertion,
};
pub use check_engine::{eval_caveat, CheckEngine, MAX_REWRITE_DEPTH};
pub use delegation::{
    authority_of, effective_policy_of, DelegationAlgebra, DelegationInput, IntersectionProof,
    EFFECTIVE_GRANT_CARRIER,
};
pub use expand::Expand;
pub use failstatic_cache::{
    CachedDecision, CacheTelemetry, CoarseGrant, FailStaticCache, Served, FRESH_TTL_SECS, S6_STORE,
};
pub use list_objects::{ListObjects, DEFAULT_IDS_CARDINALITY_CAP};
pub use read_replica::{
    AuthzReadReplica, ReadRoute, ReplicaRow, ReplicaTelemetry, ReplicaWriteRejected, S5_HOLDER,
    S5_TABLE,
};
pub use lowering::{
    fall_back_to_check, is_fall_back, lower, watermark_verdict, AuthzJoin, BoundParam, Lowered,
    WatermarkVerdict,
};
pub use reverse_index::{
    ReverseIndex, ReverseIndexConsumer, ReverseRow, S8_CONSUMER, S8_HOLDER, S8_TABLE,
};
pub use namespace::{
    core_hierarchy, AdmitReject, FragmentDef, NamespaceEngine, PermissionRule, Userset,
    MAX_RULE_DEPTH,
};
// `StoreBackedCheck` is defined below in this module; re-exported at the crate root for callers.
pub use machine_auth::{
    Authority, CapabilityAuthenticator, CapabilityToken, MachineKind, S7Denylist,
    StructuralTokenVerifier, TokenVerifier,
};
pub use principal_store::{
    PrincipalError, PrincipalProfile, PrincipalRow, PrincipalStore, ProfileRef, S1_HOLDER, S1_TABLE,
};
pub use revocation::{
    RevocationEntry, RevocationStore, RevocationTelemetry, RevokedKind, REVOCATION_SLA_SECS,
    S7_TABLE,
};
pub use tuple_store::{
    run_grant_expiry, StoredTuple, TupleStore, WriteError, S3_HOLDER, S3_TABLE,
};

use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Decision, IdentityService, ListObjectsResult,
    ObjectType, Permission, Principal,
};
use myelin_substrate::{
    boot, AppSpec, Authorizer, Config, CriticalDependencies, InternalRpc, Migration, Migrations,
    PublicRoutes, ServeError, ServeHandle, StoreManifest,
};
use myelin_tenancy::ArtifactRef;

/// The Identity service name — a PII-free label, the telemetry/trace service identifier
/// (architecture 00 §3.5). The harness threads it through holder registration + the signal set.
pub const SERVICE_NAME: &str = "identity";

/// The Identity service's forward-only embedded migrations (architecture 00 §9; contract 1.5),
/// run at boot **before** the instance reports ready (liveness ≠ readiness, §4.3 — readiness gates
/// on migrate-complete). On this shell floor the DDL is a minimal forward-only set that creates
/// the schema marker the store prompts (P-ID-05 the S1 principal store, P-ID-08 the S3 tuple
/// store) extend; the substrate co-located `outbox` + `consumer_dedup` tables are prepended by the
/// harness itself ([`boot`]). The store DDL bodies land in their named prompts; what matters here
/// is that migrations EXIST so the booting instance is **not-ready until they apply** (the gate the
/// shell proves).
///
/// **Floor:** the real S3 table DDL is P-ID-08; the S1 principal table is declared here (P-ID-05).
/// Migration `0101` creates the `(tenant, region)`-partitioned, RLS-scoped S1 principal table
/// (architecture §2; contracts 11.1/12.1) — the row model + the in-memory store the
/// [`principal_store`] module ships maps to it (the live OLTP binding lands with the driver,
/// P-S15). The PII columns (email/display_name) are NOT in this skeletal DDL; they live encrypted
/// under the per-subject DEK ([`principal_store::PrincipalProfile`], 11.3/11.4) — the DDL holds the
/// `profile_ref` (the erasable handle), keeping the §X-7 split structural. Forward-only.
fn identity_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0100_identity_schema_marker",
            "CREATE TABLE IF NOT EXISTS identity_schema_marker (applied_at TEXT)",
        ),
        // The S1 principal table (P-ID-05): (tenant, region)-partitioned, RLS-scoped. The opaque
        // `principal_id` (immutable attribution) is separate from the erasable `profile_ref` (the
        // §X-7 split). Profile PII is encrypted under the per-subject DEK (the store layer), so it
        // is NOT a clear column here — `profile_ref` points at the per-subject key. Forward-only.
        Migration::plain(
            "0101_s1_principal",
            "CREATE TABLE IF NOT EXISTS principal (\
                 tenant TEXT NOT NULL, \
                 region TEXT NOT NULL, \
                 principal_id TEXT NOT NULL, \
                 kind TEXT NOT NULL, \
                 profile_ref TEXT, \
                 data_role TEXT NOT NULL, \
                 status TEXT NOT NULL, \
                 PRIMARY KEY (tenant, region, principal_id))",
        ),
        // The S7 revocation mirror (P-ID-14): the (tenant, region)-partitioned PG MIRROR of the
        // Redis/Valkey-class denylist (architecture §2 S7 row: "Redis/Valkey + PG mirror"). Holds
        // revoked `jti`s, suspended principals, and per-run agent-token TTLs — PII-free handles
        // only (no payloads). The fast Redis/Valkey layer is rebuilt FROM this durable mirror on
        // recovery, which is what makes `revoke` idempotent even on crash. Forward-only.
        Migration::plain(
            "0102_s7_revocation",
            "CREATE TABLE IF NOT EXISTS revocation (\
                 tenant TEXT NOT NULL, \
                 region TEXT NOT NULL, \
                 kind TEXT NOT NULL, \
                 handle TEXT NOT NULL, \
                 revoked_at TEXT NOT NULL, \
                 expires_at TEXT, \
                 PRIMARY KEY (tenant, region, kind, handle))",
        ),
    ])
}

/// The fail-closed `check` / `list_objects` slot the internal-RPC surface re-authorizes against
/// (architecture §1 — "the internal-RPC surface is how everyone calls check/list_objects on"; §8
/// — the depth-bounded Zanzibar evaluation). **This is the named M1 floor:** the shell ships with
/// this stub wired into the surface; it returns the fail-closed default `Deny` for every `check`
/// and `NotYetImplemented` for every `list_objects`, until the real bodies land.
///
/// Why deny (never error) for `check`: a `check` is a security gate. An un-wired gate that errored
/// might be mistaken upstream for "try again / open" — so the shell returns an explicit `Deny`
/// (fail-closed, ADR-03), the same posture the real engine takes on genuine uncertainty (P-ID-09).
///
/// Why error for `list_objects`: a leak-free pre-filter that does not yet exist must NOT return a
/// permissive (or empty-but-trusted) id set a consumer would JOIN against — it returns a loud
/// `NotYetImplemented` so a caller cannot mistake "no filter yet" for "everything is visible".
///
/// **Floor → follow-on:** `check` → P-ID-09; `list_objects` → P-ID-11/P-ID-12. The other nine
/// `IdentityService` methods inherit the frozen surface's defaults; the shell exposes the two the
/// internal-RPC surface gates on today.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedCheck;

impl FailClosedCheck {
    /// A fresh fail-closed slot.
    pub fn new() -> FailClosedCheck {
        FailClosedCheck
    }
}

impl IdentityService for FailClosedCheck {
    /// 4.1 — `authenticate` body is P-ID-06 / P-ID-07. The shell does not resolve credentials yet.
    fn authenticate(&self, _credential: &myelin_identity::Credential) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented(
            "authenticate → P-ID-06/07 (M1); the shell wires the slot, not the body",
        ))
    }

    /// 4.2 — **the fail-closed gate (the load-bearing shell behaviour).** Every `check` returns
    /// `Deny` until the depth-bounded Zanzibar userset-rewrite lands (P-ID-09). Never fail-open.
    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        // Fail-closed (ADR-03): an un-wired authorization gate denies, it never opens. The real
        // depth-bounded evaluation (P-ID-09) replaces this; the Deny posture on genuine
        // uncertainty is the SAME posture it ships with, so this is the correct floor, not a hole.
        Ok(Decision::Deny)
    }

    /// 4.3 — `list_objects` body is P-ID-11 / P-ID-12. Errors loudly (a non-existent leak-free
    /// pre-filter must not be mistaken for a permissive set).
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented(
            "list_objects → P-ID-11/12 (M1); the shell wires the slot, not the body",
        ))
    }

    fn list_subjects(
        &self,
        _object: &myelin_identity::ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(AuthzError::NotYetImplemented("list_subjects → P-ID-13 (M1)"))
    }

    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &myelin_identity::ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(AuthzError::NotYetImplemented("explain → P-ID-13 (M1)"))
    }

    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("delegation → P-ID-17 (M1)"))
    }

    fn write_tuples(
        &self,
        _deltas: &[myelin_identity::TupleDelta],
        _precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(AuthzError::NotYetImplemented("write_tuples → P-ID-08 (M1)"))
    }

    fn mint_run_token(
        &self,
        _agent_id: &myelin_identity::PrincipalId,
        _run_id: &myelin_identity::RunId,
        _delegation_caveats: &myelin_identity::DelegationCaveats,
        _ttl: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("mint_run_token → P-ID-16 (M1)"))
    }

    fn revoke(&self, _target: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("revoke → P-ID-14 (M1)"))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &myelin_identity::PrincipalId,
        _tenant: &myelin_tenancy::TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("resolve_pseudonym → P-ID-19 (M1)"))
    }

    fn erase(&self, _subject: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
    }

    fn admit_fragment(
        &self,
        _fragment: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("admit_fragment → P-ID-10 (M1)"))
    }
}

/// The internal-RPC authorization adapter that re-authorizes every call against the Identity
/// `check` slot (architecture §4.2 — the internal surface re-authorizes every call through the
/// [`Authorizer`] seam; §1 — every service calls Identity's `check` on this surface). It maps the
/// harness's `(principal, action)` re-authorization seam onto [`FailClosedCheck::check`].
///
/// On the shell floor the inner `check` is the fail-closed stub, so **every internal call is
/// denied** — proving the surface re-authorizes (it does not presume "internal = safe") AND
/// proving the slot is fail-closed until P-ID-09 wires the real body. When the body lands, only
/// this adapter's inner `IdentityService` changes; the surface wiring is unchanged.
pub struct CheckAuthorizer<S: IdentityService + Send + Sync> {
    inner: S,
}

impl<S: IdentityService + Send + Sync> CheckAuthorizer<S> {
    /// Wrap an `IdentityService` `check` slot as the internal-RPC re-authorization seam.
    pub fn new(inner: S) -> CheckAuthorizer<S> {
        CheckAuthorizer { inner }
    }
}

impl<S: IdentityService + Send + Sync> Authorizer for CheckAuthorizer<S> {
    /// Re-authorize an internal call by running `check` (architecture §4.2). On this floor the
    /// inner slot is fail-closed, so every call denies. The `action` is the permission being
    /// re-checked; the full `(subject, object, zookie)` threading lands with the real surface
    /// body (P-ID-09's caller). Returns `true` only on an explicit `Allow` — `Deny`,
    /// `Conditional`, and any error all fail closed to `false`.
    fn authorize(&self, subject: &Principal, action: &str) -> bool {
        // The shell re-authorizes through the SAME check the platform calls — the slot, not a
        // bespoke path (EI-01 §7, one primitive). On the floor it is fail-closed; the real
        // depth-bounded evaluation (P-ID-09) swaps in behind this exact seam.
        let permission = Permission(action.to_string());
        // A self-referential object stand-in for the action-level re-authorize (the object-level
        // threading is the surface body's, P-ID-09); the fail-closed stub ignores it and denies.
        let object = ArtifactRef(format!("myelin://{}/identity/action/{}", subject.tenant.0, action));
        let at = Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        matches!(
            self.inner.check(subject, &permission, &object, &at, None),
            Ok(Decision::Allow)
        )
    }
}

/// The store-backed `check` slot (P-ID-09): an [`IdentityService`] whose `check` runs the real
/// depth-bounded Zanzibar [`CheckEngine`] over the S3 tuple store, behind the **same**
/// [`IdentityService`] surface the shell's [`FailClosedCheck`] occupied (EI-01 §7 — one primitive,
/// no bespoke check path: the real engine swaps in behind the exact seam, the surface wiring is
/// unchanged). The other ten methods stay the named-floor stubs (their bodies are their own M1
/// prompts); only `check` is wired live here.
///
/// The `(tenant, region)` scope the engine reads under is derived from the **subject's own**
/// verified `tenant`/`region` (tenant-from-token, never a path — ID-3); the `permission` is mapped
/// to the relation name the raw-tuple rewrite resolves (the compiled-permission namespace engine is
/// P-ID-10, which composes its operators over this same rewrite core).
#[derive(Clone)]
pub struct StoreBackedCheck {
    engine: CheckEngine,
    /// The S3 tuple store (kept so the `list_objects` slot can build a [`ListObjects`] over it +
    /// the S8 index; `check` already reads it through [`CheckEngine`]).
    tuples: TupleStore,
    /// The S8 authz reverse index (P-ID-11) — the JOIN/candidate source `list_objects` materialises
    /// the `Ids` path from + the `Filter` push-down targets. Fed off the bus by
    /// [`reverse_index::ReverseIndexConsumer`]; shared so the live consumer and this `list_objects`
    /// slot read the same projection.
    index: ReverseIndex,
    /// The compiled ReBAC namespace engine (P-ID-10) — the org/team/project core hierarchy + any
    /// admitted subsystem fragments. `check` resolves a **compiled permission** (the four-operator
    /// rewrite) through it; a name that is not a compiled permission falls through to a raw
    /// relation check. Behind an `Arc` so the cloneable `check` handle shares one schema and
    /// [`StoreBackedCheck::admit_fragment`] mutates it under a lock.
    namespace: std::sync::Arc<std::sync::Mutex<NamespaceEngine>>,
    /// The S7 revocation list / token denylist (P-ID-14) — the SCIM-disable cross-surface deny path.
    /// `check` consults it so a disabled/revoked principal is denied within the W = 5 min bound
    /// **regardless of a stale cached `subject.status`** (the F8 / ID-D1 deny: every surface that
    /// calls `check` denies). Cloneable + shared so every surface holding a `check` handle reads the
    /// SAME denylist (one revocation oracle, EI-01 §7 — no bespoke per-surface revocation path).
    revocations: revocation::RevocationStore,
    /// **S5, the authz read-replica (P-ID-16) — the ID-4 first scaling move.** A read-only,
    /// stale-tolerant replica of the OLTP-tier authz read path: a default-consistency
    /// (`BoundedStale`) hot-path read is routed to S5 (relieving the primary), while a zookie-stamped
    /// (`Strong`) read BYPASSES S5 to this engine's primary `check`/`list_objects` (read-your-writes /
    /// the new-enemy guard). Cloneable + shared so a live service wires the SAME replica the
    /// replication feed populates ([`StoreBackedCheck::read_replica`]). The routing decision is
    /// [`StoreBackedCheck::route_read`]; the replica is fed by the primary's replication stream
    /// ([`read_replica::AuthzReadReplica::replicate`]).
    read_replica: read_replica::AuthzReadReplica,
}

impl StoreBackedCheck {
    /// Wire the real `check` engine over the S3 [`TupleStore`], with the **core org/team/project
    /// hierarchy** pre-loaded into the namespace engine (the M3/M4 subsystem fragments are admitted
    /// on top via [`StoreBackedCheck::admit_fragment`] / [`NamespaceEngine::admit`]). A fresh S8
    /// reverse index is created; for a live service the index fed off the bus is shared via
    /// [`StoreBackedCheck::with_index`].
    pub fn new(tuples: TupleStore) -> StoreBackedCheck {
        StoreBackedCheck::with_index(tuples, ReverseIndex::new())
    }

    /// Wire the real `check` + `list_objects` engine over the S3 [`TupleStore`] AND a shared S8
    /// [`ReverseIndex`] (the one fed off the bus by [`reverse_index::ReverseIndexConsumer`]), so the
    /// `list_objects` slot materialises the `Ids` path / targets the `Filter` push-down over the live
    /// projection. The core hierarchy is pre-loaded; subsystem fragments admit on top.
    pub fn with_index(tuples: TupleStore, index: ReverseIndex) -> StoreBackedCheck {
        StoreBackedCheck {
            engine: CheckEngine::new(tuples.clone()),
            tuples,
            index,
            namespace: std::sync::Arc::new(std::sync::Mutex::new(
                NamespaceEngine::with_core_hierarchy(),
            )),
            revocations: revocation::RevocationStore::new(),
            read_replica: read_replica::AuthzReadReplica::new(),
        }
    }

    /// The shared S7 revocation list / token denylist (P-ID-14) this `check` slot consults — so a
    /// caller can `revoke` / SCIM-disable a principal and observe the cross-surface deny, and a
    /// drill can read the `revocation_lag` telemetry. Every clone of this `StoreBackedCheck` shares
    /// the SAME denylist (one revocation oracle).
    pub fn revocations(&self) -> &revocation::RevocationStore {
        &self.revocations
    }

    /// **`revoke(jti | principal_id)` (contract 4.7) — the LIVE, scoped, idempotent, crash-safe
    /// revoke (P-ID-14).** Carries the verified `(tenant, region)` scope the ABI trait method cannot
    /// (it has no caller scope). Writes the S7 denylist (mirror-first, idempotent) so every surface
    /// that consults `check` denies the target within the W = 5 min bound. The scope-less ABI
    /// [`IdentityService::revoke`] delegates to this with the target's own tenant scope.
    pub fn revoke_in(
        &self,
        scope: &myelin_storage::TenantScope,
        target: &myelin_identity::RevokeTarget,
        now: myelin_events::Timestamp,
    ) {
        self.revocations.revoke(scope, target, now);
    }

    /// **The SCIM-disable revocation path (P-ID-14) — the authoritative deny.** Disable a principal
    /// across every surface in the verified `(tenant, region)` partition (the §4 authoritative
    /// revocation path). After this, every surface's `check` for `principal` returns `Deny` within
    /// the bound. Idempotent + crash-safe (it is a `revoke` of the principal).
    pub fn disable_principal_in(
        &self,
        scope: &myelin_storage::TenantScope,
        principal: &myelin_identity::PrincipalId,
        now: myelin_events::Timestamp,
    ) {
        self.revocations.disable_principal(scope, principal, now);
    }

    /// The shared S8 reverse index this slot's `list_objects` reads (so a caller can wire the live
    /// bus consumer over the SAME index).
    pub fn index(&self) -> &ReverseIndex {
        &self.index
    }

    /// **The shared S5 authz read-replica (P-ID-16) this slot routes default-consistency reads to.**
    /// A live service wires the SAME replica the primary's replication stream populates (via
    /// [`read_replica::AuthzReadReplica::replicate`]); the routing decision is
    /// [`StoreBackedCheck::route_read`].
    pub fn read_replica(&self) -> &read_replica::AuthzReadReplica {
        &self.read_replica
    }

    /// **The S5 consistency gate on the live read path (P-ID-16) — route a read to S5 or the primary.**
    /// A `BoundedStale` (default-consistency) read routes to the stale-tolerant replica (the scaling
    /// win: the high-QPS hot-path read comes off S5, relieving the primary `check`/`list_objects`); a
    /// `Strong` (zookie-stamped) read BYPASSES S5 to this engine's primary read (read-your-writes /
    /// the new-enemy guard — S5 is never read on a `Strong` request). This is the SAME zookie-bypass
    /// split S6 (P-ID-15) and S8's watermark path (P-ID-12) enforce, here for the replica. The caller
    /// reads S5 ([`read_replica::AuthzReadReplica::read`]) on [`read_replica::ReadRoute::Replica`] and
    /// runs the authoritative primary [`StoreBackedCheck::check`]/[`StoreBackedCheck::list_objects`] on
    /// [`read_replica::ReadRoute::Primary`] — one primitive, no bespoke replica read path.
    pub fn route_read(&self, at: &Consistency) -> read_replica::ReadRoute {
        self.read_replica.route(at)
    }

    /// **Build the S6 fail-static availability cache (P-ID-15) in front of THIS `check` engine.** The
    /// returned [`failstatic_cache::FailStaticCache`] shares this slot's S7 [`revocation::RevocationStore`]
    /// (so a `revoke`/disable here is seen by the cache's just-revoked deny), and reads the
    /// `static_max ≤ revocation SLA ≥ agent-token-TTL` bound from the thresholds-file `[fail_static]`
    /// row (the W = 5 min `[OPEN — LEGAL]` engineering seed). Wire a `BoundedStale` authz read
    /// through [`StoreBackedCheck::check_failstatic`] (S6 in front of the engine `check`); a
    /// `Strong`/zookie-stamped read bypasses S6 and hits this engine directly (the 4.10 bypass half).
    ///
    /// **One cache primitive (EI-01 §7):** the cache wraps the SAME `check` the platform calls — it
    /// is not a bespoke availability path. On a healthy read it caches + returns the engine's coarse
    /// answer; on an Id-dependency hiccup (the `source` returns `Err`) it serves the bounded-staleness
    /// fallback. Errors (`FailStaticError`) only if the thresholds-file bound is structurally invalid
    /// (it cannot be, given the M0 `[fail_static]` row — but the bound is enforced, never assumed).
    pub fn failstatic_cache(
        &self,
        revocation_sla_secs: myelin_substrate::Seconds,
        threshold: &myelin_substrate::thresholds::FailStaticThreshold,
    ) -> Result<failstatic_cache::FailStaticCache, myelin_substrate::FailStaticError> {
        failstatic_cache::FailStaticCache::try_new(
            revocation_sla_secs,
            threshold,
            self.revocations.clone(),
        )
    }

    /// **Build the S6 fail-static cache against an INJECTED clock (the drill/CDC seam).** Identical
    /// to [`StoreBackedCheck::failstatic_cache`] but with a caller-supplied [`myelin_substrate::Clock`]
    /// so the ID-D2 drill / the 4.11 CDC can advance a `TestClock` across the `fresh_ttl` /
    /// `static_max` boundaries deterministically (the production `SystemClock` exposes no mutators,
    /// so wiring it via [`StoreBackedCheck::failstatic_cache`] leaks no control over wall time). The
    /// SAME shared S7 denylist is threaded (a `revoke` here is seen by the cache's just-revoked deny).
    pub fn failstatic_cache_with_clock<C: myelin_substrate::Clock>(
        &self,
        revocation_sla_secs: myelin_substrate::Seconds,
        threshold: &myelin_substrate::thresholds::FailStaticThreshold,
        clock: C,
    ) -> Result<failstatic_cache::FailStaticCache<C>, myelin_substrate::FailStaticError> {
        failstatic_cache::FailStaticCache::try_new_with_clock(
            revocation_sla_secs,
            threshold,
            self.revocations.clone(),
            clock,
        )
    }

    /// **The S6-fronted `check` (P-ID-15) — the availability read path.** Serve a `check` answer
    /// through the fail-static availability cache `s6` (built via [`StoreBackedCheck::failstatic_cache`]),
    /// honouring the zookie-bypass: a `Strong`/zookie-stamped read bypasses S6 and hits THIS engine
    /// directly (fail-closed-or-wait); a `BoundedStale` read is served static during an Id hiccup.
    ///
    /// `source_ok` decides whether the authoritative engine `check` is currently reachable: on the
    /// healthy path it is `true` (the engine answer is cached + served); during an injected
    /// Id-dependency hiccup it is `false` (the cache serves the bounded-staleness fallback, the ID-D2
    /// drill path). This `source_ok` seam is how a drill injects the hiccup at the authz boundary
    /// (the substrate `ResilientClient` surfaces a real transport hiccup the same way; here the
    /// in-cell engine is local, so the hiccup is injected explicitly). The `question` is the
    /// `permission@object` cache discriminator. The just-revoked subject is denied through the cache
    /// regardless (the S7 consult on every served answer).
    #[allow(clippy::too_many_arguments)]
    pub fn check_failstatic<C: myelin_substrate::Clock>(
        &self,
        s6: &failstatic_cache::FailStaticCache<C>,
        scope: &myelin_storage::TenantScope,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
        now: &myelin_events::Timestamp,
        source_ok: bool,
    ) -> failstatic_cache::CachedDecision {
        let question = format!("{}@{}", permission.0, object.0);
        s6.check_cached(scope, &subject.principal_id, &question, at, now, || {
            if !source_ok {
                // The injected Id-dependency hiccup: the authoritative engine is unreachable. The
                // cache serves the bounded-staleness fallback (BoundedStale) or fails closed (Strong).
                return Err(myelin_substrate::ServeError("identity authz hiccup".into()));
            }
            // The healthy path: run the SAME authoritative engine `check` the platform calls (one
            // primitive, no bespoke availability path). Its decision is cached + served.
            match self.check(subject, permission, object, at, caveat) {
                Ok(decision) => Ok(decision),
                // A genuine engine error (not a hiccup) is treated as a fail-closed Deny by the
                // cache's source — never an open fall-through.
                Err(_) => Ok(Decision::Deny),
            }
        })
    }

    /// Admit a **rich** [`FragmentDef`] (carrying the permission rewrite structure) into the cell
    /// schema — the path the M3/M4 fragment prompts (P-ID-24/26/27/29/30) use to declare a
    /// subsystem's relations + permissions. The names-only ABI [`IdentityService::admit_fragment`]
    /// is the contract-boundary validator; this is the build-time declaration the engine compiles.
    pub fn admit_fragment_def(
        &self,
        frag: &FragmentDef,
    ) -> myelin_identity::FragmentAdmit {
        self.namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(frag)
    }

    /// A read-only snapshot of the compiled namespace engine (for inspection / the explain path).
    pub fn namespace(&self) -> NamespaceEngine {
        self.namespace.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// **`delegation(agent, trigger_actor) → EffectivePolicy` (contract 4.5) — the LIVE
    /// monotone-intersection algebra (P-ID-17).** Returns the composed `agent.policy ∩ delegation ∩
    /// tenant.policy` (with the "you cannot delegate authority you do not have" re-check) so the
    /// Agent Fabric's `EffectApi` (P-ID-23, M2) never re-implements the algebra. The policy SETS
    /// (`input`) come from the caveat chains the agent's + the delegating human's credentials carry
    /// (resolved by `authenticate`, P-ID-07) — the scope-less ABI [`IdentityService::delegation`]
    /// cannot carry them, so the service surface wires THIS entry. The composition runs over the SAME
    /// [`delegation::DelegationAlgebra`] (one primitive, no bespoke intersection path).
    pub fn delegation_in(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
    ) -> myelin_identity::EffectivePolicy {
        delegation::DelegationAlgebra::new().delegation(agent, trigger_actor, input)
    }

    /// **The delegation algebra WITH the recorded [`delegation::IntersectionProof`] (the ID-D5 green
    /// artifact).** Identical to [`StoreBackedCheck::delegation_in`] but returns the proof the drill
    /// records (the four conjunct sets + the composed effective set + the verified monotone
    /// post-condition) — the "prove it" observation (EI-01 §3).
    pub fn delegation_proved_in(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
    ) -> (myelin_identity::EffectivePolicy, delegation::IntersectionProof) {
        delegation::DelegationAlgebra::new().delegation_proved(agent, trigger_actor, input)
    }

    /// **The four-conjunct delegation decision — the three policy sets AND the ordinary object
    /// `check` run AS the agent principal (architecture §6, conjunct 4; P-ID-17).** Wires the
    /// [`delegation::DelegationAlgebra`] over THIS slot's live [`CheckEngine`] (the SAME `check` the
    /// platform calls), so an agent is confined to `agent ∩ delegation ∩ tenant` AND must hold the
    /// object-level relation. Returns `Allow` only when both conjuncts pass (fail-closed otherwise).
    /// This is the conjunct the Agent Fabric's `EffectApi` runs per effect at apply time (P-ID-23).
    #[allow(clippy::too_many_arguments)]
    pub fn delegation_with_check_in(
        &self,
        agent: &Principal,
        trigger_actor: &Principal,
        input: &delegation::DelegationInput,
        scope: &myelin_storage::TenantScope,
        required_grant: &str,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
    ) -> Decision {
        delegation::DelegationAlgebra::with_check(self.engine.clone()).delegation_with_check(
            agent,
            trigger_actor,
            input,
            scope,
            required_grant,
            permission,
            object,
            at,
        )
    }

    /// **The scoped, LIVE `list_subjects` (P-ID-13) — the Zanzibar Expand served by S8 at density.**
    /// The verified `(tenant, region)` scope is carried explicitly (the ABI trait method cannot — it
    /// has no caller principal). The object's type is inferred from its id's `type:` prefix (the §7.3
    /// id-column convention); the [`Expand`] engine resolves the permission's rewrite (a compiled
    /// permission, the four operators) or serves a direct relation via the S8 reverse index. Returns
    /// the flattened [`myelin_identity::SubjectTree`].
    pub fn list_subjects_in(
        &self,
        scope: &myelin_storage::TenantScope,
        object: &myelin_identity::ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> myelin_identity::SubjectTree {
        let namespace = self.namespace.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let expand = Expand::new(self.tuples.clone(), namespace, self.index.clone());
        let object_type = ObjectType(infer_object_type(&object.0));
        expand.list_subjects(scope, object, &object_type, permission, at)
    }

    /// **The scoped, LIVE `explain` (P-ID-13) — the userset-rewrite trace for the admin inspector /
    /// HITL.** The verified scope is carried explicitly (as for [`StoreBackedCheck::list_subjects_in`]).
    /// Returns the [`myelin_identity::RewriteTrace`] of why `subject`'s access to `permission` on
    /// `object` resolved — non-empty, ending in `ALLOW`/`DENY` (never a silent allow).
    pub fn explain_in(
        &self,
        scope: &myelin_storage::TenantScope,
        subject: &myelin_identity::PrincipalId,
        permission: &Permission,
        object: &myelin_identity::ObjectId,
        at: &Consistency,
    ) -> myelin_identity::RewriteTrace {
        let namespace = self.namespace.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let expand = Expand::new(self.tuples.clone(), namespace, self.index.clone());
        let object_type = ObjectType(infer_object_type(&object.0));
        expand.explain(scope, subject, object, &object_type, permission, at)
    }
}

/// Infer an object's TYPE from its id by the leading `type:` prefix (`channel:general` → `channel`).
/// The §7.3 id-column convention the core hierarchy + the M3/M4 fragments follow; a bare id with no
/// prefix is its own type (resolving no compiled permission → a direct relation expand, the safe
/// floor). Mirrors `namespace`/`reverse_index`/`expand`'s local helper (kept here so the service slot
/// does not reach into a module-private helper).
fn infer_object_type(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

impl IdentityService for StoreBackedCheck {
    fn authenticate(&self, _credential: &myelin_identity::Credential) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented("authenticate → P-ID-06/07 (M1)"))
    }

    /// 4.2 — the LIVE depth-bounded Zanzibar `check` (P-ID-09). The scope is the subject's verified
    /// `(tenant, region)` (tenant-from-token). The engine is fail-closed: any uncertainty (malformed
    /// ref, depth exhaustion, suspended subject) is `Deny`/`Conditional`, never `Allow`.
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        // tenant-from-token (ID-3): the scope is the SUBJECT's own verified (tenant, region), never
        // a path. The namespace engine (P-ID-10) resolves a COMPILED permission through the four
        // userset operators; a name that is not a compiled permission falls through to a raw
        // relation check (one primitive — both compose over the SAME depth-bounded rewrite core).
        let scope = myelin_storage::TenantScope::from_verified_token(subject, subject.region.clone());

        // The S7 revocation consult (P-ID-14, the SCIM-disable cross-surface deny path). A
        // disabled/revoked principal is DENIED here — within the W = 5 min bound — regardless of a
        // stale cached `subject.status` (the F8 / ID-D1 deny: every surface that calls `check`
        // denies). This is the FAST cross-surface deny that does not wait for the next
        // `authenticate`. A `Principal` denylist entry carries no TTL, so the consult `now` is the
        // zero instant (it never gates a no-expiry entry). The check engine's own `subject.status`
        // gate (P-ID-09) remains the second line of defence (fail-closed in depth).
        let revoke_target = myelin_identity::RevokeTarget::Principal(subject.principal_id.clone());
        if self
            .revocations
            .is_revoked(&scope, &revoke_target, &myelin_events::Timestamp(String::new()))
        {
            return Ok(Decision::Deny);
        }

        // A caveat is the field/transition ABAC rider evaluated AFTER the relation/permission
        // grant holds (off the hot path, §8.6). The permission-aware namespace resolution is the
        // grant; the raw `check` already threads the caveat. To keep ONE caveat evaluation seam we
        // run the namespace grant first, then apply the caveat through the raw engine's literal
        // evaluator only when a caveat is present.
        let object_type = namespace::type_of_object_ref(object);
        let granted = self.namespace.lock().unwrap_or_else(|e| e.into_inner()).permits(
            &self.engine,
            &scope,
            subject,
            &object_type,
            &permission.0,
            object,
            at,
        );
        if !granted {
            return Ok(Decision::Deny);
        }
        match caveat {
            None => Ok(Decision::Allow),
            // The caveat rides on top of the grant (the literal-only floor, P-ID-09; full QueryAst
            // core P-ID-22). A satisfied caveat keeps Allow; a violated one Denies; a missing-
            // context one is Conditional — never a silent Allow.
            Some(cav) => Ok(check_engine::eval_caveat(cav)),
        }
    }

    /// 4.3 — the LIVE `list_objects` (P-ID-11): the return-shape dispatch + the S4 `Ids` materialise
    /// path over S8, the `Filter` push-down above the cardinality cap. The scope is the subject's
    /// verified `(tenant, region)` (tenant-from-token, never a path — ID-3), exactly as `check`'s is.
    /// The full `Filter` SetExpr→SQL lowering + the watermark read-consistency path are P-ID-12.
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        // tenant-from-token (ID-3): the scope is the SUBJECT's own verified (tenant, region).
        let scope = myelin_storage::TenantScope::from_verified_token(subject, subject.region.clone());
        // Build the list_objects evaluator over the shared S3 store + S8 index + the compiled
        // namespace engine (one snapshot of the schema; admit holds the lock only to clone).
        let namespace = self.namespace.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let lo = ListObjects::new(self.tuples.clone(), namespace, self.index.clone());
        Ok(lo.list_objects(&scope, subject, permission, ty, at))
    }

    /// 4.4 (Expand) — the LIVE `list_subjects` (P-ID-13): the Zanzibar Expand served by S8 at
    /// 50k-member density. The scope is derived from the **caller's own** verified subject — but the
    /// ABI trait method does not carry the caller principal, so this slot uses the internal-RPC
    /// re-authorization seam's scope. To keep the ONE expand seam, the live entry the service wires is
    /// [`StoreBackedCheck::list_subjects_in`] (which carries the verified `(tenant, region)` scope the
    /// ABI method cannot); the ABI method errors loudly so a tenant-less expand is never served.
    fn list_subjects(
        &self,
        _object: &myelin_identity::ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        // The ABI trait method carries no verified (tenant, region) scope (no caller principal); a
        // tenant-less expand has no partition to read and MUST NOT be served (the tenant-predicate
        // floor — a cross-tenant or unscoped expand is a leak). The live, scoped entry the service
        // surface wires is `StoreBackedCheck::list_subjects_in`, which carries the verified scope.
        Err(AuthzError::NotYetImplemented(
            "list_subjects (ABI, scope-less) → use StoreBackedCheck::list_subjects_in (P-ID-13); a \
             tenant-less expand is never served (the tenant-predicate floor)",
        ))
    }

    /// 4.4 (`explain`) — the LIVE `explain` (P-ID-13): the userset-rewrite trace. Like
    /// `list_subjects`, the ABI method carries no verified scope; the scoped entry is
    /// [`StoreBackedCheck::explain_in`]. The ABI method errors loudly (never a scope-less trace).
    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &myelin_identity::ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(AuthzError::NotYetImplemented(
            "explain (ABI, scope-less) → use StoreBackedCheck::explain_in (P-ID-13)",
        ))
    }

    /// 4.5 — the LIVE monotone-intersection delegation algebra (P-ID-17). The ABI trait method
    /// carries only the two principals, NOT the policy SETS (the caveat chains their credentials
    /// carry) the intersection composes over — so a scope-/policy-less ABI call cannot compute the
    /// real effective policy and MUST NOT fabricate one (an empty or all-allow `EffectivePolicy`
    /// would be a silent over- or under-grant). The scoped entry the service surface wires is
    /// [`StoreBackedCheck::delegation_in`], which carries the [`delegation::DelegationInput`] (the
    /// agent ceiling / delegation chain / tenant guardrails / the delegator's held set). The ABI
    /// method errors loudly so a policy-less delegation is never silently served.
    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented(
            "delegation (ABI, policy-less) → use StoreBackedCheck::delegation_in (P-ID-17); the \
             monotone intersection needs the conjunct policy sets the credentials carry, which the \
             scope-less ABI method cannot supply (never a fabricated EffectivePolicy)",
        ))
    }

    fn write_tuples(
        &self,
        _deltas: &[myelin_identity::TupleDelta],
        _precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        // The write path is TupleStore::write_tuples (P-ID-08), which carries the (tenant, region)
        // scope + actor the ABI trait method does not; this slot is not the write entrypoint.
        Err(AuthzError::NotYetImplemented("write_tuples → TupleStore::write_tuples (P-ID-08)"))
    }

    fn mint_run_token(
        &self,
        _agent_id: &myelin_identity::PrincipalId,
        _run_id: &myelin_identity::RunId,
        _delegation_caveats: &myelin_identity::DelegationCaveats,
        _ttl: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("mint_run_token → P-ID-16 (M1)"))
    }

    /// 4.7 (`revoke`) — the LIVE, idempotent, crash-safe `revoke` (P-ID-14) over the S7 denylist.
    /// The ABI trait method carries no verified `(tenant, region)` scope; a revoke writes a
    /// partition and the partition must come from a verified scope (never inferred), so the
    /// scope-carrying entry the service wires is [`StoreBackedCheck::revoke_in`] (it threads the
    /// verified scope + the revoke timestamp), with [`StoreBackedCheck::disable_principal_in`] the
    /// SCIM-disable path. The ABI method errors loudly so an unscoped revoke is never silently
    /// mis-partitioned (the tenant-predicate floor).
    fn revoke(&self, _target: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented(
            "revoke (ABI, scope-less) → use StoreBackedCheck::revoke_in (P-ID-14); a revoke writes a \
             (tenant, region) partition and must carry a verified scope (the tenant-predicate floor)",
        ))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &myelin_identity::PrincipalId,
        _tenant: &myelin_tenancy::TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("resolve_pseudonym → P-ID-19 (M1)"))
    }

    fn erase(&self, _subject: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("erase → P-ID-20 (M1)"))
    }

    /// 4.9 — admit a subsystem's ReBAC namespace fragment into the cell schema. **LIVE (P-ID-10):**
    /// the names-only ABI carrier is validated + admitted through the [`NamespaceEngine`] (the
    /// org/team/project core hierarchy is pre-loaded; subsystem fragments compile on top). A
    /// well-formed fragment returns `Admitted{fragment_id}`; a malformed one (undeclared relation,
    /// id-minting, duplicate type/permission) returns `Rejected{reason}` — loudly, never silently
    /// admitted. The RICH rewrite-carrying declaration the M3/M4 fragments use is
    /// [`StoreBackedCheck::admit_fragment_def`] over a [`namespace::FragmentDef`].
    fn admit_fragment(
        &self,
        fragment: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Ok(self
            .namespace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit_abi(fragment))
    }
}

/// Assemble the Identity service [`AppSpec`] (architecture 00 §3.1; contract 1.1) the harness
/// wires. The eight-field spec declares Identity's migrations (so readiness gates on
/// migrate-complete) and the in-process outbox/holder defaults; the harness opens the three ports
/// (public / internal-RPC / metrics-health) around it.
///
/// `config` is the validated, env-first config (§3.2). The OLTP store is implicitly critical (the
/// harness adds it); Identity declares no further critical downstream here (it is the dependency
/// ROOT — it depends on nothing else, §1), so a healthy boot is ready once migrations apply.
pub fn identity_app_spec(config: Config) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: identity_migrations(),
        hot_tables: myelin_substrate::HotTables::none(),
        // The public surface (gateway-fronted, tenant-from-token); the authenticate route bodies
        // are P-ID-06/07. The harness opens the live tenant-from-token PublicSurface (P-S13).
        public: PublicRoutes::default(),
        // The internal-RPC surface — the one every service calls check/list_objects on (§1). The
        // harness's InternalSurface re-authorizes every call; the shell's fail-closed CheckAuthorizer
        // is what it re-authorizes against (returned from `internal_authorizer`).
        internal: InternalRpc::default(),
        // No consumers yet — the iam.* projection consumers land with write_tuples (P-ID-08).
        consumers: Vec::new(),
        // Every opened store auto-registers as a PersonalDataHolder (§3.4, GD-3) — the S1/S3 store
        // holders land with those stores (P-ID-05/P-ID-08); the OLTP store registers at boot.
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: myelin_substrate::OutboxSpec::default(),
        // Identity is the dependency root — it declares no critical downstream of its own (§1).
        critical: CriticalDependencies::default(),
    }
}

/// The internal-RPC re-authorization seam the harness's `InternalSurface` is opened over for the
/// Identity service — the fail-closed `check` slot ([`FailClosedCheck`]) wrapped as a
/// [`CheckAuthorizer`]. Until P-ID-09 wires the real body, every internal call denies (fail-closed).
/// Exposed as its own constructor so the surface wiring + the body swap are one seam.
pub fn internal_authorizer() -> CheckAuthorizer<FailClosedCheck> {
    CheckAuthorizer::new(FailClosedCheck::new())
}

/// Boot the Identity service shell under the harness (architecture 00 §3.1) up to the pre-serve
/// state, returning the [`ServeHandle`] the lifecycle drives. A thin wrapper over
/// [`myelin_substrate::boot`] of [`identity_app_spec`] — separated so a test/drill can boot, inspect
/// the three ports + the liveness ≠ readiness state, and drive the drain deterministically.
///
/// Returns `Err` (the non-zero exit) on a failed boot (§3.1).
pub fn boot_identity(config: Config) -> Result<ServeHandle, ServeError> {
    boot(identity_app_spec(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::{serve, Readiness, Startup, Surface};
    use myelin_tenancy::TenantId;

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )
    }

    /// **The shell boots under the harness and the three ports bind (contracts 1.1/1.2).** The
    /// Identity AppSpec runs the boot → migrate → relay → ports lifecycle; the public / internal /
    /// metrics-health surfaces are all opened (3/3 ports up).
    #[test]
    fn identity_shell_boots_and_three_ports_bind() {
        let handle = boot_identity(Config::default()).expect("the identity shell boots");
        assert_eq!(handle.name(), "identity");
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    /// **Liveness ≠ readiness (contract 1.3): readiness is false *before* migrations apply.** The
    /// metrics-health surface opens in the `Booting` startup state — not-ready (it cannot serve
    /// correct traffic before its schema exists) but not-killed (liveness stays Up: a booting
    /// instance is not a wedge). This is the readiness-gates-on-migrate-complete property.
    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        // Open the metrics-health surface in its pre-migrate (Booting) state directly — the same
        // surface the harness opens during boot, before it marks the startup gate complete.
        let surface = myelin_substrate::MetricsHealthSurface::new(
            CriticalDependencies::new(["oltp"]),
            myelin_substrate::HealthTable::new(),
        );
        // pre-migrate: the startup gate holds readiness DOWN.
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until migrations apply (the migrate-complete gate)"
        );
        assert!(r.startup_incomplete, "the not-ready reason names the startup (pre-migrate) gate");
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        // not-killed: a slow/booting instance is NOT a wedge.
        assert_eq!(
            surface.liveness(),
            myelin_substrate::Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );

        // post-migrate (boot complete): the gate lifts → ready (the OLTP dep is up).
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    /// **A booted instance reports ready once migrations have applied** (the harness flips the
    /// startup gate to Complete at the end of a successful boot — the post-migrate readiness).
    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_identity(Config::default()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted identity instance (migrations applied, deps up) is ready"
        );
    }

    /// **The stubbed `check` slot fail-closes (the named floor, ADR-03).** Until P-ID-09 wires the
    /// real body, every `check` returns `Deny` — never `Allow`, never an error a caller could
    /// mistake for "open". This is the security floor the shell ships: deny until wired.
    #[test]
    fn stubbed_check_fail_closes_to_deny() {
        let slot = FailClosedCheck::new();
        let at = Consistency {
            at_least: myelin_identity::Zookie("z".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        let d = slot.check(
            &principal(),
            &Permission("read".into()),
            &ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            &at,
            None,
        );
        assert_eq!(d, Ok(Decision::Deny), "the un-wired check slot denies (fail-closed)");
    }

    /// **The internal-RPC surface re-authorizes every call against the fail-closed slot.** A call
    /// arriving on the trusted internal channel is STILL denied (the surface does not presume
    /// "internal = safe", §4.2) AND the slot is fail-closed until P-ID-09. Wiring the
    /// [`CheckAuthorizer`] into the harness's [`InternalSurface`] proves the seam end-to-end.
    #[test]
    fn internal_surface_re_authorizes_against_fail_closed_check() {
        let surface = myelin_substrate::InternalSurface::new(internal_authorizer());
        let r = surface.handle(&principal(), "issues.read");
        assert!(
            matches!(r, Err(myelin_substrate::InternalReject::Unauthorized { .. })),
            "the internal-RPC call is re-authorized against the fail-closed check and denied"
        );
    }

    /// **The real depth-bounded `check` engine (P-ID-09) plugs into the SAME `CheckAuthorizer`
    /// seam (EI-01 §7 — one primitive).** A granted relation re-authorizes to `true`; an un-granted
    /// one fails closed to `false` — through the identical internal-RPC re-authorization path the
    /// shell's fail-closed stub occupied. The surface wiring is unchanged; only the inner
    /// `IdentityService` swapped from `FailClosedCheck` to `StoreBackedCheck`.
    #[test]
    fn real_check_engine_swaps_in_behind_the_same_authorizer_seam() {
        use myelin_events::{OutboxStore, Timestamp};
        use myelin_identity::{ObjectId, RelName, RelationTuple, TupleDelta};
        use myelin_storage::TenantScope;

        // alice's verified principal (tenant acme, region eu-west).
        let alice = Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let scope = TenantScope::from_verified_token(&alice, alice.region.clone());

        // Seed a grant: `issues.read#@p:alice` on the action-object the authorizer builds. The
        // CheckAuthorizer builds object `myelin://acme/identity/action/issues.read` → object id
        // `issues.read`; grant alice the `issues.read` relation on that object.
        let store = TupleStore::new(OutboxStore::new());
        let admin = Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        store
            .write_tuples(
                &scope,
                &admin,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("issues.read".into()),
                    relation: RelName("issues.read".into()),
                    subject: PrincipalId("p:alice".into()),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-06-19T00:00:00Z".into()),
            )
            .expect("grant");

        // The SAME CheckAuthorizer seam, now backed by the real engine.
        let surface = myelin_substrate::InternalSurface::new(CheckAuthorizer::new(
            StoreBackedCheck::new(store.clone()),
        ));
        // alice with the grant → authorized (Allow flows back through the seam).
        assert!(
            surface.handle(&alice, "issues.read").is_ok(),
            "the real engine allows the granted relation through the SAME authorizer seam"
        );
        // bob without a grant → fail-closed (denied) through the identical seam.
        let bob = Principal::stub(
            PrincipalId("p:bob".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        assert!(
            matches!(
                surface.handle(&bob, "issues.read"),
                Err(myelin_substrate::InternalReject::Unauthorized { .. })
            ),
            "an un-granted subject is denied through the same seam (fail-closed)"
        );
    }

    /// **S5 (P-ID-16) is wired to the live read path: a default-consistency read routes to the
    /// replica, a zookie-stamped read bypasses it to the primary.** The `StoreBackedCheck`'s shared S5
    /// is the SAME replica the primary's replication stream populates; the consistency gate on the
    /// live read path (`route_read`) is the load-bearing scaling decision (the ID-4 first scaling
    /// move). 0 writes to S5 (the replica is read-only) and a strong read never consults the replica.
    #[test]
    fn s5_routes_default_consistency_reads_and_bypasses_strong_reads() {
        use myelin_events::OutboxStore;
        use myelin_storage::TenantScope;

        let store = TupleStore::new(OutboxStore::new());
        let slot = StoreBackedCheck::new(store);
        let acme = TenantScope::from_verified_token(&principal(), principal().region.clone());

        // The primary replicates a coarse authz read row into S5 (S5 follows the primary).
        slot.read_replica()
            .replicate(&acme, "add", read_replica::ReplicaRow { key: "p:alice".into(), value: "active".into() }, 5);

        // A default-consistency read routes to S5 (the scaling win) and the row is served off it.
        let stale = Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: myelin_identity::ConsistencyMode::BoundedStale,
        };
        assert!(slot.route_read(&stale).is_replica(), "a default-consistency read is served from S5");
        assert!(
            slot.read_replica().read(&acme, "p:alice").is_some(),
            "the replicated row is served off the stale-tolerant replica"
        );

        // A zookie-stamped read BYPASSES S5 to the primary (read-your-writes / new-enemy guard).
        let strong = Consistency {
            at_least: myelin_identity::Zookie("zk-00000000000000000005".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        assert!(slot.route_read(&strong).is_primary(), "a zookie-stamped read bypasses S5 to the primary");

        // 0 writes to S5: the replica is read-only (the only mutator is replication from the primary).
        assert!(slot.read_replica().reject_write().is_err(), "S5 is read-only (a write attempt errors)");
    }

    /// `list_objects` errors loudly (NotYetImplemented) — a non-existent leak-free pre-filter must
    /// NOT be mistaken for a permissive set (it is not silently empty-but-trusted).
    #[test]
    fn stubbed_list_objects_errors_loudly() {
        let slot = FailClosedCheck::new();
        let at = Consistency {
            at_least: myelin_identity::Zookie("z".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        let r = slot.list_objects(&principal(), &Permission("read".into()), &ObjectType("issue".into()), &at);
        assert!(
            matches!(r, Err(AuthzError::NotYetImplemented(_))),
            "list_objects errors loudly until P-ID-11/12 (never a permissive set)"
        );
    }

    /// **The OLTP store auto-registered as a PersonalDataHolder at boot (contract 1.4, §3.4).**
    /// Opening IS registering — the Identity service's store appears in the holder registry. The
    /// S1/S3 store holders land with those stores (P-ID-05/P-ID-08); the OLTP store registers now.
    #[test]
    fn identity_store_auto_registers_as_holder() {
        let handle = boot_identity(Config::default()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(myelin_substrate::StoreKind::Oltp, "identity"),
            "the identity OLTP store auto-registered as a PersonalDataHolder"
        );
    }

    /// **The whole lifecycle runs end-to-end and graceful-drains.** `serve(identity_app_spec(..))`
    /// boots → migrates → relays → opens the ports → drains cleanly (outbox_depth 0). The §3.1
    /// one-call contract for the Identity service.
    #[test]
    fn identity_service_serves_and_drains_cleanly() {
        assert_eq!(
            serve(identity_app_spec(Config::default())),
            Ok(()),
            "the identity service boots → … → drains cleanly"
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// the Identity service boot with a loud error — never a silent success.
    #[test]
    fn identity_failed_boot_returns_non_zero() {
        let r = boot_identity(Config("BAD_POOL".into()));
        assert!(r.is_err(), "a failed identity boot returns non-zero (Err)");
    }
}
