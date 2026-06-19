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
//! - **`check` (4.2)** → **P-ID-09** — the depth-bounded Zanzibar userset-rewrite. Until then,
//!   [`FailClosedCheck::check`] returns `Deny` for every input (fail-closed, never fail-open).
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
pub mod machine_auth;
pub mod principal_store;
pub mod tuple_store;

pub use authenticate::{
    scheme, AuthTelemetry, CredentialVerifier, HumanSsoAuthenticator, IdorCounters,
    StructuralVerifier, VerifiedAssertion,
};
pub use machine_auth::{
    Authority, CapabilityAuthenticator, CapabilityToken, MachineKind, S7Denylist,
    StructuralTokenVerifier, TokenVerifier,
};
pub use principal_store::{
    PrincipalError, PrincipalProfile, PrincipalRow, PrincipalStore, ProfileRef, S1_HOLDER, S1_TABLE,
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
