//! # `shed_clone` — the protected-human-lane shed order + the CDN bundle-URI clone (GIT-P15 / P-276, M3-G2)
//!
//! Two halves of one front-door concern — *serving the human under load*:
//!
//! 1. **The protected-human-lane shed order** (ADR-16, the OQ-K per-surface budget floor:
//!    `speculative → batch/CI → agent → human-last`) at the Git front door. Under a mixed-principal
//!    clone storm a CI/agent fetch sheds (`429 + Retry-After`) BEFORE a human's interactive fetch —
//!    the human lane is *protected*. Per-tenant, so one tenant's storm never sheds another's human.
//! 2. **The CDN bundle-URI accelerated-clone path** (Git `02 §1.4`, Storage 11.2 C3): a clone may be
//!    served a **bundle-URI** (`transfer.bundleURI`) into the within-EU CDN clone/bundle class so the
//!    bulk of clone-storm read fan-out leaves serving compute for the content-addressed object tier —
//!    the budget is reached *later*, and the human's interactive fetch is the residual delta.
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! - **The shed order itself is the substrate's** [`myelin_substrate::shed`]: this module does NOT
//!   re-author the shed lane / run-class / budget table. It WIRES the existing
//!   [`myelin_substrate::shed::ShedLane`] over the NEW [`myelin_substrate::shed::Surface::GitFrontDoor`]
//!   surface, reading the budget **from the thresholds file** ([`myelin_substrate::thresholds`]) — the
//!   prompt's "the shed budget is read from the thresholds file". The Git front door's only authoring
//!   is the *derivation* of the request's [`RunClass`] from its principal + an optional run-class
//!   header, and the placement of the admit/shed decision at the front of the pipeline.
//! - **The CDN bundle bytes are the storage** [`myelin_storage::cdn::CdnCloneClass`] (P-254): this
//!   module does NOT re-author the content-addressed blob class. It uses the class to PUBLISH a
//!   precomputed bundle and to SERVE it by content-address (the address IS the cache-validity check —
//!   a tampered bundle is refused), then proves a bundle-URI clone round-trips to the *same* repo
//!   bytes the serving tier would have streamed.
//!
//! ## The shed order at the door (architecture `00 §2 (A)`, `02 §6`)
//! The shed gate sits at the FRONT of the front-door pipeline — BEFORE `authenticate`/`check`/stream —
//! because shedding is a saturation-admission decision (an over-budget agent fetch must be refused
//! cheaply, before any backend work). The run-class is derived from the *verified* principal's kind
//! (`PrincipalKind::Human → Human`, `Agent → Agent`, `Service → BatchCi`) plus an optional injected
//! run-class header that may only **down-class** (a human-issued prefetch can declare itself
//! speculative; a machine principal can NEVER up-class to the human lane — the human lane is
//! structurally unspoofable, [`RunClass::derive`]). The decision is per-`(tenant)`: a clone storm on
//! tenant `noisy` fills only `noisy`'s budget and never sheds tenant `quiet`'s human (the blast-radius
//! guarantee, EI-02 §1).
//!
//! Because the run-class is derived from the verified `Principal`, the shed gate runs the cheap
//! [`GitFrontDoorShed::admit_for`] (which takes the principal directly) AFTER `authenticate` resolved
//! the principal but BEFORE the per-action `check` + placement + stream: an over-budget agent fetch is
//! shed with `429 + Retry-After` having done only the (already-required) authenticate, never the
//! heavier check/placement/stream. The [`GitFrontDoorShed::admit_class`] form takes a pre-derived
//! class for the storm drill (which mints classes directly).
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **The OQ-K per-surface shed budget** for [`Surface::GitFrontDoor`] is a **named v1 floor**: the
//!   *discipline* (the surface is bounded, reserves a human lane, applies the shed order) is the
//!   contract; the *numbers* (`cap`/`reservation`/`retry_after`) are **tuned by the clone-storm 30×
//!   drill GIT-D6 in GIT-P34 (M5)** — here the order is asserted at **1× with mixed principals** (the
//!   prompt's gate), not the full surge. The floor is recorded in `thresholds.toml`
//!   (`[[shed_budgets]] surface = "GitFrontDoor"`) and tuned there by the drill, never edited green.
//! - **The CDN bundle-URI floor** here is the content-address-cache *semantics* + the bundle-URI
//!   round-trip over the within-EU CDN class. It **hardens to the full within-EU CDN class (the real
//!   edge POP fleet + cache-fill transport) in GIT-P33 (M5)** — here the load-bearing correctness (a
//!   bundle-URI clone round-trips valid, content-address-verified bytes; a tampered bundle is refused)
//!   is proven over the in-memory `FsBlobStore` floor the storage class rests on.
//!
//! ## Mutation floor (mandatory-core ≥ 80% — EI-01 §2/§3)
//! The shed-order DECISION path ([`GitFrontDoorShed::admit_for`]/[`admit_class`](GitFrontDoorShed::admit_class)
//! → the human-protected per-tenant graded admit) is mandatory-core: an off-by-one that sheds a human
//! before an agent, or that leaks one tenant's budget into another, is the failure this exists to
//! catch. The bundle-URI round-trip's content-address verify (a tampered bundle MUST be refused) is
//! the second mandatory-core branch. The floor is ≥ 80%; the achieved score is recorded in the P-276
//! report (`cargo mutants -p myelin-git -f crates/myelin-git/src/shed_clone.rs`).

use myelin_identity::Principal;
use myelin_storage::blob::{BlobError, ContentHash};
use myelin_storage::cdn::CdnCloneClass;
use myelin_substrate::shed::{RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

// ───────────────────────────── the shed gate at the Git front door ───────────────────────────────

/// **The protected-human-lane shed gate at the Git front door (ADR-16 / OQ-K; contract 1.11).**
///
/// A thin Git-front-door wiring over the substrate's [`ShedLane`] for the
/// [`Surface::GitFrontDoor`] surface: it reads the surface's budget **from the thresholds file** and
/// applies the shed order `speculative → batch/CI → agent → human-last`, per-tenant. The Git front
/// door admits every clone/push through [`GitFrontDoorShed::admit_for`] (the run-class derived from
/// the verified principal); an over-budget non-human lane is shed with `429 + Retry-After`, while the
/// human lane is protected (shed only in true saturation).
pub struct GitFrontDoorShed {
    lane: ShedLane,
}

/// **Why a clone/push was refused at the shed gate** — the typed form the transport maps to the wire
/// `429`. A `Shed` carries the `Retry-After` (seconds) the client honours (the no-amplification
/// guarantee — our ResilientClient honours `Retry-After`, so a shed is not a retry-storm amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShedRejection {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this.
    pub lane: RunClass,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the
    /// `429 Too Many Requests` response.
    pub retry_after_secs: u64,
}

impl GitFrontDoorShed {
    /// Open the Git-front-door shed gate, reading the [`Surface::GitFrontDoor`] budget **from the
    /// thresholds file** (the prompt's "the shed budget is read from the thresholds file"). A missing
    /// `GitFrontDoor` shed-budget row is a LOUD error (the gate refuses to open against a guessed
    /// budget — EI-01 §3), never a silent default.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<GitFrontDoorShed, String> {
        let budget = thresholds
            .shed_budget(Surface::GitFrontDoor)
            .map_err(|e| format!("Git front-door shed budget unavailable: {e}"))?;
        Ok(GitFrontDoorShed {
            lane: ShedLane::with_budget(Surface::GitFrontDoor, budget),
        })
    }

    /// Open the gate against an explicit budget (used by the storm drill to drive the boundary at a
    /// small, deterministic budget without editing the thresholds file).
    pub fn with_budget(budget: SurfaceBudget) -> GitFrontDoorShed {
        GitFrontDoorShed {
            lane: ShedLane::with_budget(Surface::GitFrontDoor, budget),
        }
    }

    /// **Admit a clone/push by its verified principal + an optional injected run-class header.** The
    /// run-class is DERIVED ([`RunClass::derive`]) from `principal.kind` (the kind sets the ceiling)
    /// and the header (which may only down-class) — a machine principal can NEVER up-class to the
    /// protected human lane. Returns `Ok(class)` admitted (a slot was taken — release it on
    /// completion via [`GitFrontDoorShed::release`]) or `Err(ShedRejection)` shed (`429 +
    /// Retry-After`). The decision is per-`principal.tenant`.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, ShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    /// **Admit a request of a pre-derived [`RunClass`] for `tenant`.** The lower-level form the storm
    /// drill drives (it mints classes directly). Returns `Ok(())` admitted (a slot taken) or
    /// `Err(ShedRejection)` shed. The human lane is protected: a human is shed ONLY when every slot
    /// (the reserved human fraction included) is full; the non-human lanes shed first, in the graded
    /// order `speculative → batch/CI → agent`.
    pub fn admit_class(&mut self, tenant: &TenantId, class: RunClass) -> Result<(), ShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(ShedRejection {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// Release a slot a prior [`GitFrontDoorShed::admit_for`]/[`admit_class`](Self::admit_class) took
    /// for `(tenant, class)` — call when the clone/push completes so the lane recovers after the storm.
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` survival signal —
    /// the storm-drill green artifact: `human-lane == 0 shed`, `agent-lane > 0 shed`).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The per-tenant in-flight count (admitted not yet released) — for the blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }
}

// ───────────────────────────── the CDN bundle-URI accelerated-clone path ──────────────────────────

/// **A bundle-URI advertised to a cloning client (`transfer.bundleURI`; Git `02 §1.4`).** The client
/// fetches the static, content-addressed bundle from the within-EU CDN edge (residency-pinned), then
/// does an incremental fetch for the delta. Because the bundle is content-addressed, the URI carries
/// the [`ContentHash`] (the CDN cache key) — an edge cache over it is a pure content-address cache,
/// no per-request authz at the edge (the FRONT DOOR gates *which* tenant may request the URI; the
/// edge serves bytes by hash). PII-free: a hash + the tenant the bundle belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleUri {
    /// The tenant whose keyspace the bundle lives in (the residency/authz scope the front door gated).
    pub tenant: TenantId,
    /// The bundle's content-address — the CDN cache key (`transfer.bundleURI` resolves to this).
    pub content_hash: ContentHash,
}

/// **The CDN bundle-URI accelerated-clone path (Git `02 §1.4`, Storage 11.2 C3 — the GIT-P15 floor).**
///
/// Wires the serving-tier clone offload over the storage [`CdnCloneClass`] (P-254, the within-EU
/// content-addressed clone/bundle class): the serving tier PUBLISHES a precomputed bundle for a hot
/// repo ([`BundleUriClone::publish_bundle`] → a [`BundleUri`]); a cloning client is ADVERTISED the
/// bundle-URI and FETCHES the bundle by content-address ([`BundleUriClone::clone_via_bundle_uri`]) —
/// the address IS the cache-validity check, so a tampered bundle is refused (0 silent serve) and a
/// valid clone round-trips the exact repo bytes the serving tier would otherwise have streamed.
///
/// This is a delivery LAYER over the unchanged blob tier (it BORROWS the [`CdnCloneClass`], which
/// itself borrows the base `BlobStore`) — NOT a new store. The residency property (an EU tenant's
/// bundle never reaches an extra-EU edge) is the storage class's [`CdnCloneClass::eligible_edges`]
/// filter; this path inherits it.
pub struct BundleUriClone<'a> {
    cdn: CdnCloneClass<'a>,
}

impl<'a> BundleUriClone<'a> {
    /// Compose the accelerated-clone path over the storage CDN clone/bundle class (P-254).
    pub fn new(cdn: CdnCloneClass<'a>) -> BundleUriClone<'a> {
        BundleUriClone { cdn }
    }

    /// **Publish a precomputed clone bundle for a hot repo → the advertisable [`BundleUri`].** The
    /// bundle bytes (the serving tier's precomputed clone bundle for the repo at a ref-snapshot) are
    /// put into the content-addressed CDN class; the returned [`BundleUri`] carries the content-hash
    /// the front door advertises as `transfer.bundleURI`.
    pub fn publish_bundle(&self, bundle_bytes: &[u8]) -> Result<BundleUri, BlobError> {
        let content_hash = self.cdn.publish_bundle(bundle_bytes)?;
        Ok(BundleUri {
            tenant: self.cdn.tenant().clone(),
            content_hash,
        })
    }

    /// **Serve a clone via its advertised bundle-URI — the accelerated-clone path.** Fetches the
    /// bundle from the CDN class by the URI's content-address; the storage class re-hash-verifies the
    /// bytes and REFUSES a content-address mismatch (the STOR-D7 0-silent-serve floor), so a served
    /// bundle is provably the exact requested content. A clone served this way round-trips the same
    /// repo bytes the serving tier would have streamed — the accelerated-clone floor holds.
    ///
    /// A `uri.tenant` that does not match THIS class's tenant is refused as a cross-tenant bundle
    /// request (defence-in-depth — the bundle keyspace is per-tenant; the front door already gated
    /// the tenant before advertising, but the serve path never serves another tenant's bundle).
    pub fn clone_via_bundle_uri(&self, uri: &BundleUri) -> Result<Vec<u8>, BundleCloneError> {
        if &uri.tenant != self.cdn.tenant() {
            return Err(BundleCloneError::CrossTenant {
                uri_tenant: uri.tenant.as_str().to_string(),
                class_tenant: self.cdn.tenant().as_str().to_string(),
            });
        }
        self.cdn
            .bundle(&uri.content_hash)
            .map_err(|e| BundleCloneError::Fetch {
                detail: e.to_string(),
            })
    }
}

/// **Why a bundle-URI clone was refused.** Either the bundle bytes failed the content-address verify
/// (a tampered/missing bundle — the content-address IS the cache-validity check) or the URI named a
/// tenant other than the serving class's (a cross-tenant bundle request).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleCloneError {
    /// The bundle fetch failed the content-address verify (tampered / missing) — 0 silent serve.
    Fetch {
        /// The rendered storage error (never the bundle bytes).
        detail: String,
    },
    /// The URI named a foreign tenant — refused (the bundle keyspace is per-tenant).
    CrossTenant {
        /// The tenant the URI addressed.
        uri_tenant: String,
        /// The tenant the serving class belongs to.
        class_tenant: String,
    },
}

impl std::fmt::Display for BundleCloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BundleCloneError::Fetch { detail } => write!(
                f,
                "bundle-URI clone REFUSED — the bundle failed the content-address verify ({detail}); \
                 the content-address is the cache-validity check (0 silent serve)"
            ),
            BundleCloneError::CrossTenant {
                uri_tenant,
                class_tenant,
            } => write!(
                f,
                "bundle-URI clone REFUSED — URI tenant `{uri_tenant}` ≠ serving-class tenant \
                 `{class_tenant}` (the bundle keyspace is per-tenant)"
            ),
        }
    }
}

impl std::error::Error for BundleCloneError {}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
    use myelin_storage::blob::FsBlobStore;
    use myelin_tenancy::Region;

    fn tenant(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn human(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("h-{tenant_slug}")),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn agent(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("a-{tenant_slug}")),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn small_budget() -> SurfaceBudget {
        // cap 6, reserve 2 for humans → non-human budget 4; step = max(4/8,1)=1 →
        // speculative ceiling 2, batch 3, agent 4.
        SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 2,
            retry_after_secs: 5,
        }
    }

    // ───────────────────────── the shed order (the GIT-P15 gate) ─────────────────────────

    /// **The shed budget is read from the thresholds file** (the prompt's explicit requirement). The
    /// gate opens against the canonical `thresholds.toml` `[[shed_budgets]] surface = "GitFrontDoor"`
    /// row — not a hardcoded number.
    #[test]
    fn the_git_front_door_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate = GitFrontDoorShed::from_thresholds(&thresholds).expect("GitFrontDoor budget present");
        // the gate opened (a missing row would have been a loud error). Its budget is the file's.
        let file_budget = thresholds.shed_budget(Surface::GitFrontDoor).expect("present");
        assert!(file_budget.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(file_budget.human_lane_reservation > 0, "the human lane is reserved");
        // and the lane is wired to GitFrontDoor (not some other surface).
        assert_eq!(gate.lane.surface(), Surface::GitFrontDoor);
    }

    /// **The shed order holds under a mixed-principal storm (1×, the GIT-P15 gate):** the human lane
    /// is SERVED while the agent lane SHEDS (`429 + Retry-After`). The green artifact is the per-lane
    /// shed-count signal — `human == 0 shed`, `agent > 0 shed`.
    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        // a clone storm of agents fills the non-human budget (cap-reserved = 4) then sheds.
        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent fetch admitted while under the non-human budget"
            );
        }
        // the agent lane now SHEDS with 429 + Retry-After.
        let shed = gate.admit_for(&a, None).expect_err("the agent clone storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 5, "the shed carries a Retry-After (clients honour it)");

        // THE GATE: the HUMAN's interactive fetch is STILL SERVED (the protected lane, shed last).
        assert_eq!(
            gate.admit_for(&h, None).expect("the human is served while the agent sheds"),
            RunClass::Human
        );

        // the green-artifact signal: the human lane has 0 shed, the agent lane has shed.
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed (served)");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    /// **The full shed PRIORITY order: speculative → batch/CI → agent → human-last.** A graded
    /// threshold sheds the lower-promise lane first.
    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let t = tenant("acme");
        // fill non-human in-flight to 2 with agents (under every non-human ceiling).
        for _ in 0..2 {
            gate.admit_class(&t, RunClass::Agent).expect("agent admitted");
        }
        // non_human == 2 == speculative ceiling → speculative sheds FIRST.
        assert!(gate.admit_class(&t, RunClass::Speculative).is_err(), "speculative sheds first");
        // batch/ci still admitted (ceiling 3).
        gate.admit_class(&t, RunClass::BatchCi).expect("batch admitted"); // non_human → 3
        // non_human == 3 == batch ceiling → batch sheds, agent still admitted (ceiling 4).
        assert!(gate.admit_class(&t, RunClass::BatchCi).is_err(), "batch/ci sheds next");
        gate.admit_class(&t, RunClass::Agent).expect("agent admitted"); // non_human → 4
        // non_human == 4 == agent ceiling → agent sheds, but the HUMAN is admitted (shed last).
        assert!(gate.admit_class(&t, RunClass::Agent).is_err(), "agent sheds before the human");
        gate.admit_class(&t, RunClass::Human).expect("human served — shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0, "the human lane is never shed here");
    }

    /// **Per-tenant: one tenant's clone storm NEVER sheds another tenant's human (blast-radius).**
    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        // saturate the noisy tenant: 4 agents fill the non-human budget, then agents shed.
        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(gate.admit_for(&noisy, None).is_err(), "noisy agent lane sheds");

        // the noisy tenant's in-flight reflects the admitted machine fetches (kills the
        // `in_flight -> 0` mutant: the accessor reports the real per-tenant count, not a constant).
        assert_eq!(gate.in_flight(&tenant("noisy")), 4, "the noisy tenant has 4 in-flight machine fetches");
        // the QUIET tenant is untouched — its human is served, its budget independent.
        assert_eq!(gate.in_flight(&tenant("quiet")), 0, "the quiet tenant's budget is independent");
        assert_eq!(
            gate.admit_for(&quiet_human, None).expect("the quiet human is served"),
            RunClass::Human,
            "the noisy clone storm must NEVER shed another tenant's human",
        );
    }

    /// **A machine principal can NEVER up-class to the human lane** (the human lane is structurally
    /// unspoofable). An agent with no human header derives `Agent`; a header may only down-class.
    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = GitFrontDoorShed::with_budget(small_budget());
        let a = agent("acme");
        // an agent derives the Agent lane (there is no human header to up-class to).
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        // a header DOWN-classes a human-issued prefetch (a human declaring speculative).
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative)).expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch may down-class itself (sheds earlier)",
        );
    }

    /// Release frees a slot so the lane recovers after the storm passes.
    #[test]
    fn release_frees_a_slot_after_the_storm() {
        let mut gate = GitFrontDoorShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_class(&t, RunClass::Agent).expect("admitted"); // non_human 1
        gate.admit_class(&t, RunClass::Agent).expect("admitted"); // non_human 2 == cap-reserved
        assert!(gate.admit_class(&t, RunClass::Agent).is_err(), "agent sheds at cap-reserved");
        gate.release(&t, RunClass::Agent);
        gate.admit_class(&t, RunClass::Agent).expect("a released slot is reusable");
    }

    // ───────────────────────── the CDN bundle-URI accelerated-clone ─────────────────────────

    fn eu_cdn<'a>(store: &'a FsBlobStore, t: &str) -> CdnCloneClass<'a> {
        CdnCloneClass::over(tenant(t), Region::new("fr-par"), true, store)
    }

    /// **A clone served a bundle-URI from the CDN class round-trips a valid clone (the
    /// accelerated-clone floor holds — the GIT-P15 gate).** The serving tier publishes a precomputed
    /// bundle → a bundle-URI; the cloning client fetches by content-address; the round-tripped bytes
    /// are the exact bundle the serving tier would have streamed.
    #[test]
    fn a_bundle_uri_clone_round_trips_a_valid_clone() {
        let store = FsBlobStore::new();
        let path = BundleUriClone::new(eu_cdn(&store, "acme"));

        // the serving tier's precomputed clone bundle for a hot repo at a ref-snapshot.
        let bundle_bytes = b"PACK\0clone-bundle-of-hot-repo@deadbeef";
        let uri = path.publish_bundle(bundle_bytes).expect("publish bundle → bundle-URI");
        // the URI carries the content-address (the CDN cache key the front door advertises).
        assert_eq!(uri.content_hash, ContentHash::blake3(bundle_bytes));
        assert_eq!(uri.tenant, tenant("acme"));

        // the cloning client fetches by the advertised bundle-URI → the exact bytes (valid clone).
        let cloned = path.clone_via_bundle_uri(&uri).expect("clone via bundle-URI");
        assert_eq!(cloned, bundle_bytes, "the bundle-URI clone round-trips the exact repo bytes");
    }

    /// **A tampered bundle is REFUSED — the content-address IS the cache-validity check (0 silent
    /// serve).** The accelerated-clone path never serves corrupt clone bytes.
    #[test]
    fn a_tampered_bundle_is_refused_zero_silent_serve() {
        let store = FsBlobStore::new();
        let path = BundleUriClone::new(eu_cdn(&store, "acme"));
        let uri = path.publish_bundle(b"valid-clone-bundle").expect("publish");

        // corrupt the bundle at rest (a tampered edge cache entry).
        assert!(store.corrupt_for_drill(&tenant("acme"), &uri.content_hash), "bundle present");
        let err = path.clone_via_bundle_uri(&uri).expect_err("a tampered bundle MUST be refused");
        assert!(matches!(err, BundleCloneError::Fetch { .. }), "0 silent serve: {err}");
    }

    /// **A bundle-URI naming a foreign tenant is refused** (the bundle keyspace is per-tenant —
    /// defence-in-depth atop the front-door tenant gate).
    #[test]
    fn a_cross_tenant_bundle_uri_is_refused() {
        let store = FsBlobStore::new();
        let path = BundleUriClone::new(eu_cdn(&store, "acme"));
        // a URI minted for globex's keyspace (a stolen/forged URI).
        let foreign = BundleUri {
            tenant: tenant("globex"),
            content_hash: ContentHash::blake3(b"whatever"),
        };
        let err = path.clone_via_bundle_uri(&foreign).expect_err("a foreign-tenant URI is refused");
        assert!(matches!(err, BundleCloneError::CrossTenant { .. }), "{err}");
    }

    /// Error Display is distinct + non-empty (kills the fmt→default mutant).
    #[test]
    fn bundle_clone_error_display_is_distinct() {
        let fetch = BundleCloneError::Fetch { detail: "x".into() };
        let xtenant = BundleCloneError::CrossTenant {
            uri_tenant: "globex".into(),
            class_tenant: "acme".into(),
        };
        let s1 = fetch.to_string();
        let s2 = xtenant.to_string();
        assert!(s1.contains("content-address") && !s1.is_empty());
        assert!(s2.contains("per-tenant") && !s2.is_empty());
        assert_ne!(s1, s2);
    }
}
