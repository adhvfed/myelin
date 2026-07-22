//! # `front_door` — the stateless Git front door / router (GIT-P13 / P-274, M3-G2)
//!
//! **FIRST RUNNABLE** (roadmap §6): clone/push works, authenticated, tenant-isolated,
//! region-pinned, never loses an event. This is the **stateless front door** the architecture
//! `00-overview.md` §2 (A) draws — the one pipeline every SSH (`russh`) and smart-HTTP-v2
//! (`axum`/`hyper`) entrypoint funnels through:
//!
//! ```text
//!   authenticate (Id 4.1) → check (Id 4.2 + CaveatContext) → placement_of(repo) (12.2)
//!     → residency reject-if-leaving-region (ADR-11, 12.4) → stream packs (no buffering)
//! ```
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `02-internals-and-algorithms.md` §1.1 (smart-HTTP-v2 front door — `info/refs?service=…` →
//!   `POST .../git-upload-pack|git-receive-pack`, **streams, no whole-pack buffering**), §1.2
//!   (SSH — `russh`, the offered pubkey → `Id.authenticate(ssh_pubkey)` → `Principal`; a deploy
//!   key is a **repo-scoped machine principal**; then `Id.check(principal, pull|push, repo)`,
//!   `placement_of(repo)`, residency enforced, stream to the serving tier).
//! - `00-overview.md` §2 (A) (the stateless front door: authenticate → check → placement_of →
//!   residency reject → stream; **liveness ≠ readiness** — readiness gates on backend
//!   reachability, liveness never checks deps).
//!
//! **Contracts (consumed — implemented to the frozen shapes):**
//! - **4.1** `authenticate(Credential) → Principal` — EVERY entrypoint (SSH pubkey / deploy-key /
//!   PAT / per-job token) resolves a `Principal`; the **tenant is taken from the verified
//!   credential, never the URL path** (ID-3 / X-1 — the GIT-D8 invariant).
//! - **4.2** `check(subject, permission, object, at, caveat?) → Decision` — the per-action
//!   fail-closed gate (`pull` / `push`) with the optional `CaveatContext` rider.
//! - **12.2** `placement_of(repo) → RepoGitPlacement` (the storage face — region-pinned,
//!   relocatable, NEVER node-pinned) resolves the repo's cell/group + pinned region.
//! - **12.4 / ADR-11** the **residency reject**: a route whose target region ≠ the repo's pinned
//!   region is REJECTED at the door (0 out-of-region routes admitted — the residency-pin lint).
//! - **1.9** `ResilientClient` — the front door is the resilient client of the Id + placement
//!   dependencies (the fail-static/degrade bound on the Id call is the GIT-P14 follow-on; here the
//!   door fails CLOSED on any Id error, never open).
//!
//! ## The GIT-D8 invariant (cross-tenant isolation — the quantified gate)
//!
//! **Tenant comes from the TOKEN, never the URL path.** A client may present a token authenticated
//! to tenant `acme` while addressing a URL path under tenant `globex` — the front door resolves the
//! tenant from the verified credential (4.1) and **denies the cross-tenant route at the door**: the
//! repo it would serve lives under `globex`, but the principal is `acme`'s, so the `(principal.tenant
//! ≠ repo.tenant)` predicate trips BEFORE any `check` / placement / stream. The result: **0
//! cross-tenant reads** (the GIT-D8 green artifact). This is structural — the door never even looks
//! up the URL-path tenant's repo on behalf of a foreign-tenant principal.
//!
//! ## liveness ≠ readiness (`00-overview.md` §2 (A))
//!
//! `liveness()` returns healthy whenever the process is up — it NEVER checks a backend (so a
//! dependency hiccup does not get the pod killed). `readiness()` gates on backend reachability (the
//! Id resolver + the placement resolver being reachable) — an unready door is pulled from the LB
//! rotation but stays alive.
//!
//! ## FLOORS named (VISION §3 — name your floors)
//! - **GIT-P14 (P-275) — DONE.** The Git ReBAC fragment is wired LIVE (the rich rewrites through
//!   Identity's engine, proven enforced at the check) + the **FailStatic** degrade-not-cascade bound
//!   on the Id dependency lands in [`crate::live_check::GitCheckGate`]: the front door's `pull`/`push`
//!   `check` now rides the shared `myelin_substrate::FailStaticAuthz` so a transient Id hiccup
//!   DEGRADES (serves the bounded-stale coarse grant within `static_max ≤ revocation SLA`) instead of
//!   cascading every request closed, while a just-revoked subject is still denied. The
//!   [`FrontDoorError::IdentityUnavailable`] fail-CLOSED below remains the correct posture for a
//!   `Strong` (zookie-stamped) authz read — a security-sensitive read never serves stale (the
//!   new-enemy guard, 4.10); the bounded-stale degrade applies to the availability-tolerant
//!   clone/fetch hot path through `GitCheckGate::front_door_check`.
//! - **GIT-P15** lands the **protected-human-lane shed order** (per-surface admission budgets,
//!   OQ-K) + the **CDN bundle-URI accelerated-clone** floor.
//! - The production `russh` / `axum` transport wiring + the X-6-hardened `WireExecutor` host live in
//!   the serving tier; this module is the **transport-agnostic pipeline** they call (the same
//!   `route()` for SSH and HTTP — one decision, two front ends). The byte plumbing streams through
//!   the [`crate::core::GitCore`] seam (no whole-pack buffering — the streaming property is the
//!   serving-tier executor's, surfaced here as the `serve`/`advertise_refs` calls that pass bytes
//!   through without collecting them into an owned buffer the door holds).

use crate::core::{GitCore, RepoLoc, Service, WireOutput};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Credential, Decision, IdentityService, Permission,
    Principal, PrincipalStatus, Zookie,
};
use myelin_storage::gitpack::{RepoGitPlacement, RepoId, RepoPlacementStatus};
use myelin_tenancy::{ArtifactRef, Region};

// ───────────────────────────── the request the front door routes ────────────────────────────────

/// The git smart-protocol **action** a request asks for — the permission it maps to. `Fetch`
/// (clone/fetch) needs `pull`; `Push` needs `push`. The door maps the action → the 4.9 permission
/// name and the wire [`Service`] in ONE place ([`GitAction::permission`] / [`GitAction::service`]),
/// so a new action can not be added without a permission + service decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitAction {
    /// `git-upload-pack` — fetch/clone (object download). Needs `pull`.
    Fetch,
    /// `git-receive-pack` — push (object upload). Needs `push`.
    Push,
}

impl GitAction {
    /// The frozen 4.9 repo permission this action checks (`pull` / `push`).
    pub fn permission(self) -> Permission {
        match self {
            GitAction::Fetch => Permission("pull".to_string()),
            GitAction::Push => Permission("push".to_string()),
        }
    }

    /// The wire [`Service`] this action serves.
    pub fn service(self) -> Service {
        match self {
            GitAction::Fetch => Service::UploadPack,
            GitAction::Push => Service::ReceivePack,
        }
    }
}

/// **A front-door request** — the SAME shape for SSH and smart-HTTP-v2 (one pipeline, two front
/// ends, `00-overview.md` §2 (A)). The credential is the verified machine identity (the door NEVER
/// reads the tenant from the URL path — ID-3 / X-1); `url_tenant` + `url_repo` are the
/// URL-PATH-derived addressing the client presented (carried so the door can prove the cross-tenant
/// deny against the TOKEN tenant — the GIT-D8 invariant).
#[derive(Clone, Debug)]
pub struct GitRequest {
    /// The verified credential (SSH pubkey / deploy-key / PAT / per-job token) — `authenticate`
    /// resolves the `Principal` (and thus the TENANT) from THIS, never from `url_tenant`.
    pub credential: Credential,
    /// The tenant slug the URL path addressed (`/<tenant>/<repo>.git`). This is the *requested*
    /// path, NOT the trusted tenant — a token tenant ≠ this is a cross-tenant attempt (GIT-D8).
    pub url_tenant: String,
    /// The repo slug the URL path addressed.
    pub url_repo: String,
    /// The action (fetch / push).
    pub action: GitAction,
    /// The streamed client bytes (negotiation / pack). Carried through to the serving tier WITHOUT
    /// the door buffering a whole pack of its own — the streaming property (`02 §1.1`).
    pub body: Vec<u8>,
}

// ───────────────────────────── the placement resolver port (12.2) ───────────────────────────────

/// **The `placement_of(repo)` port (contract 12.2 — region-pinned, relocatable placement).** The
/// front door resolves WHERE a repo lives + WHICH region it is pinned to through this seam. The
/// production impl reads the storage pack tier's [`RepoGitPlacement`] (the storage face of 12.2,
/// `myelin_storage::gitpack`); the front door is generic over it so the serving tier wires the real
/// resolver and tests wire a deterministic one. A repo with no placement resolves to `None` →
/// fail-closed (the door refuses, never fabricates a placement).
pub trait PlacementResolver {
    /// Resolve a repo's region-pinned placement. `None` = no placement (fail-closed).
    fn placement_of(&self, repo: &RepoId) -> Option<RepoGitPlacement>;
}

// ───────────────────────────── the front-door decision / error ──────────────────────────────────

/// **Why the front door REFUSED a request** — every refusal is LOUD + named (EI-01 §3: a refusal is
/// information). The door fails CLOSED: any of these aborts the route BEFORE a single object streams.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontDoorError {
    /// `authenticate` (4.1) could not resolve the credential to a `Principal` (an unknown SSH key /
    /// expired PAT / revoked per-job token). Fail-closed — never an anonymous route.
    Unauthenticated {
        /// The credential scheme that failed (never the secret material — PII/secret-free).
        scheme: String,
    },
    /// The resolved principal is `Suspended` / `Disabled` (a revoked/deprovisioned identity) — the
    /// door fails closed at the front door (ID-D1: a disabled identity gets zero access).
    PrincipalNotActive {
        /// The non-active status the door refused.
        status: PrincipalStatus,
    },
    /// **The GIT-D8 cross-tenant refusal.** The principal's tenant (from the verified TOKEN) is not
    /// the tenant the URL path addressed — a cross-tenant route. Refused at the door (0 cross-tenant
    /// read). The decision keys on `token_tenant`, NEVER on `url_tenant`.
    CrossTenant {
        /// The tenant the verified token authenticated under (the trusted tenant).
        token_tenant: String,
        /// The tenant the URL path tried to address (the foreign tenant — denied).
        url_tenant: String,
    },
    /// `check` (4.2) returned `Deny` / `Conditional` for the action's permission — the principal
    /// lacks `pull` / `push` on this repo (or a caveat needs context the door does not supply). The
    /// door treats `Conditional` as a deny (fail-closed — never a silent allow).
    AuthzDenied {
        /// The permission that was denied (`pull` / `push`).
        permission: Permission,
        /// The decision Identity returned (`Deny` / `Conditional`).
        decision: Decision,
    },
    /// `placement_of(repo)` found no placement — the repo is not hosted here (fail-closed; the door
    /// never fabricates a placement, never serves an unplaced repo).
    NoPlacement {
        /// The repo with no placement.
        repo: String,
    },
    /// The repo's placement is `Offboarding` (its tenant is leaving / its packs are pending
    /// crypto-shred) — the door refuses to serve an offboarding repo.
    RepoOffboarding {
        /// The offboarding repo.
        repo: String,
    },
    /// **The residency reject (ADR-11 / 12.4).** The route's target region differs from the repo's
    /// pinned region — a route that would leave the region. REFUSED at the door (0 out-of-region
    /// routes admitted; the residency-pin lint).
    OutOfRegion {
        /// The repo's pinned region (of record — a repo never leaves this).
        pinned: String,
        /// The target region the route would have served from (rejected).
        target: String,
    },
    /// The Id dependency itself errored (an authenticate/check transport failure). The door fails
    /// CLOSED on it (never open). The bounded-stale fail-static degrade is the GIT-P14 follow-on.
    IdentityUnavailable {
        /// The underlying Id error rendered (never the secret material).
        detail: String,
    },
    /// The serving-tier wire op failed (the streamed `serve`/`advertise_refs`). Surfaced loud.
    Wire {
        /// The rendered wire error.
        detail: String,
    },
}

impl std::fmt::Display for FrontDoorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrontDoorError::Unauthenticated { scheme } => write!(
                f,
                "front door: authenticate REFUSED — credential scheme `{scheme}` did not resolve to \
                 a principal (fail-closed; no anonymous route)"
            ),
            FrontDoorError::PrincipalNotActive { status } => write!(
                f,
                "front door: principal is `{status:?}` (not Active) — fail-closed (ID-D1: a disabled \
                 identity gets zero access)"
            ),
            FrontDoorError::CrossTenant { token_tenant, url_tenant } => write!(
                f,
                "front door: CROSS-TENANT route REFUSED — token tenant `{token_tenant}` ≠ URL-path \
                 tenant `{url_tenant}`; the tenant comes from the TOKEN, never the URL (GIT-D8: 0 \
                 cross-tenant read)"
            ),
            FrontDoorError::AuthzDenied { permission, decision } => write!(
                f,
                "front door: check DENIED — `{}` returned `{decision:?}` (fail-closed; Conditional \
                 is never a silent allow)",
                permission.0
            ),
            FrontDoorError::NoPlacement { repo } => write!(
                f,
                "front door: placement_of(`{repo}`) found no placement — repo not hosted here \
                 (fail-closed; never fabricate a placement)"
            ),
            FrontDoorError::RepoOffboarding { repo } => write!(
                f,
                "front door: repo `{repo}` is offboarding (packs pending crypto-shred) — refused"
            ),
            FrontDoorError::OutOfRegion { pinned, target } => write!(
                f,
                "front door: OUT-OF-REGION route REFUSED — repo pinned to `{pinned}`, route would \
                 serve from `{target}` (ADR-11 residency pin: 0 out-of-region routes admitted)"
            ),
            FrontDoorError::IdentityUnavailable { detail } => write!(
                f,
                "front door: Id dependency unavailable ({detail}) — fail-CLOSED (the bounded-stale \
                 fail-static degrade is GIT-P14)"
            ),
            FrontDoorError::Wire { detail } => {
                write!(f, "front door: serving-tier wire op failed: {detail}")
            }
        }
    }
}

impl std::error::Error for FrontDoorError {}

/// **A granted route** — the door's decision once the whole pipeline passed: the verified principal,
/// the region-pinned repo locator the serving tier streams against, and the wire service. The
/// streamed bytes flow through the [`GitCore`] seam; this value records WHAT was authorised (for the
/// audit / the serving tier), never holds a buffered pack.
#[derive(Clone, Debug)]
pub struct GrantedRoute {
    /// The verified principal (tenant from the token — the attribution subject).
    pub principal: Principal,
    /// The region-pinned repo locator (tenant from the TOKEN, region from the placement — never the
    /// URL path).
    pub repo: RepoLoc,
    /// The wire service the action serves.
    pub service: Service,
}

// ───────────────────────────── the front door (the pipeline) ────────────────────────────────────

/// **The stateless Git front door** (`00-overview.md` §2 (A)). Owns the three dependency ports —
/// the [`IdentityService`] (4.1 authenticate + 4.2 check), the [`PlacementResolver`] (12.2
/// placement_of), and the [`GitCore`] serving seam (the streamed wire backend) — and runs the ONE
/// pipeline for every SSH + smart-HTTP request. Stateless + horizontal: no per-request state, no
/// session affinity, so a clone-storm fans out across replicas cheaply.
///
/// `home_region` is the region THIS front-door replica serves (its cell's region). The residency
/// reject compares the repo's pinned region against it — a repo pinned elsewhere is an out-of-region
/// route, refused (the door never routes a repo out of its pinned region, even to its own home).
pub struct FrontDoor<I: IdentityService, P: PlacementResolver, C: GitCore> {
    id: I,
    placement: P,
    core: C,
    home_region: Region,
}

impl<I: IdentityService, P: PlacementResolver, C: GitCore> FrontDoor<I, P, C> {
    /// Compose the front door over its three dependency ports + the region this replica serves.
    pub fn new(id: I, placement: P, core: C, home_region: Region) -> Self {
        Self {
            id,
            placement,
            core,
            home_region,
        }
    }

    /// The Id dependency port (for the serving tier / drills to inspect — e.g. assert 0
    /// cross-tenant checks were ever issued).
    pub fn id_ref(&self) -> &I {
        &self.id
    }

    /// The placement resolver port.
    pub fn placement_ref(&self) -> &P {
        &self.placement
    }

    /// The serving seam (for the serving tier / drills to inspect what streamed — e.g. assert 0
    /// cross-tenant reads).
    pub fn core_ref(&self) -> &C {
        &self.core
    }

    /// **liveness ≠ readiness — liveness (`00-overview.md` §2 (A)).** Healthy whenever the process
    /// is up; NEVER checks a backend (a dependency hiccup must not get the pod killed). Always
    /// `true` here — the door is alive iff this code runs.
    pub fn liveness(&self) -> bool {
        true
    }

    /// **liveness ≠ readiness — readiness.** Gates on backend reachability: the door is READY iff
    /// the Id resolver answers (a trivial probe credential resolves OR cleanly denies — either is
    /// "Id reachable") AND a probe placement lookup does not panic. An unready door is pulled from
    /// the LB rotation but stays alive (liveness still `true`). The probe never serves traffic; it
    /// only proves the dependency ports are wired + answering.
    pub fn readiness(&self, probe: &Credential, probe_repo: &RepoId) -> bool {
        // Id reachable: authenticate returns EITHER Ok (resolved) OR an authz Deny/NotImplemented —
        // both mean "Id answered". Only a transport-style error means "Id unreachable".
        let id_reachable = self.id.authenticate(probe).is_ok();
        // Placement reachable: the lookup returns (Some/None) without panicking — either is reachable.
        let _ = self.placement.placement_of(probe_repo);
        // Readiness gates on the Id resolver answering at all; the placement probe is a liveness-free
        // reachability touch (its None is a valid answer, not unreadiness).
        id_reachable
    }

    /// **The front-door pipeline (the GIT-P13 deliverable).** Runs, in order, fail-closed at every
    /// step:
    ///
    /// 1. **authenticate (4.1)** — resolve the credential → `Principal`. The TENANT is taken from
    ///    the resolved principal (the token), NEVER from `req.url_tenant` (ID-3 / X-1).
    /// 2. **active-principal guard** — a `Suspended`/`Disabled` principal is refused (ID-D1).
    /// 3. **cross-tenant guard (GIT-D8)** — `principal.tenant` (from the token) MUST equal
    ///    `req.url_tenant`; otherwise the route is a cross-tenant attempt → REFUSED (0 cross-tenant
    ///    read). This runs BEFORE `check`/placement so the door never even looks up a foreign
    ///    tenant's repo.
    /// 4. **check (4.2 + CaveatContext)** — the per-action `pull`/`push` gate. `Deny`/`Conditional`
    ///    → refused (fail-closed; `Conditional` is never a silent allow).
    /// 5. **placement_of (12.2)** — resolve the repo's region-pinned placement. No placement → refused.
    ///    An `Offboarding` placement → refused.
    /// 6. **residency reject (ADR-11 / 12.4)** — the repo's pinned region MUST equal this replica's
    ///    `home_region`; otherwise the route would leave the region → REFUSED (0 out-of-region routes).
    /// 7. **stream** — only now does the wire op run, streaming through the [`GitCore`] seam (no
    ///    whole-pack buffering).
    ///
    /// Returns the [`WireOutput`] (the streamed advertisement/pack bytes) on success, or the FIRST
    /// [`FrontDoorError`] — the route is aborted before a single object streams.
    pub fn route(&self, req: &GitRequest) -> Result<WireOutput, FrontDoorError> {
        let route = self.authorize(req)?;
        // 7. STREAM — the wire op runs only after the whole gate passed.
        self.core
            .serve(&route.repo, route.service, req.body.clone())
            .map_err(|e| FrontDoorError::Wire {
                detail: e.to_string(),
            })
    }

    /// The **gate half** of [`Self::route`] — steps 1–6, returning the [`GrantedRoute`] WITHOUT
    /// streaming. Exposed so the serving tier can authorise then stream separately (and so the
    /// unit/drill tests assert the decision without a wire backend). This is the mandatory-core
    /// authz path the cargo-mutants score covers.
    pub fn authorize(&self, req: &GitRequest) -> Result<GrantedRoute, FrontDoorError> {
        // 1. authenticate (4.1) — tenant FROM THE TOKEN.
        let principal = self.id.authenticate(&req.credential).map_err(|e| {
            // An authn failure is "unauthenticated"; an Id transport failure is "unavailable".
            // Both fail closed; we distinguish them for the operator (degrade vs deny).
            if is_transport_error(&e) {
                FrontDoorError::IdentityUnavailable {
                    detail: format!("{e:?}"),
                }
            } else {
                FrontDoorError::Unauthenticated {
                    scheme: req.credential.scheme.clone(),
                }
            }
        })?;

        // 2. active-principal guard (ID-D1).
        if principal.status != PrincipalStatus::Active {
            return Err(FrontDoorError::PrincipalNotActive {
                status: principal.status,
            });
        }

        // 3. cross-tenant guard (GIT-D8) — the decision keys on the TOKEN tenant, never the URL path.
        let token_tenant = principal.tenant.as_str().to_string();
        if token_tenant != req.url_tenant {
            return Err(FrontDoorError::CrossTenant {
                token_tenant,
                url_tenant: req.url_tenant.clone(),
            });
        }

        // 4. check (4.2 + CaveatContext) — the per-action fail-closed gate.
        let permission = req.action.permission();
        let object = repo_artifact_ref(&token_tenant, &req.url_repo);
        let consistency = Consistency {
            // A push/clone is a strong (read-your-writes) authz read — never bounded-stale on the
            // write/access decision (the fail-static cache is for bounded-stale reads, GIT-P14).
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let caveat = self.caveat_for(req, &object);
        let decision = self
            .id
            .check(
                &principal,
                &permission,
                &object,
                &consistency,
                caveat.as_ref(),
            )
            .map_err(|e| {
                if is_transport_error(&e) {
                    FrontDoorError::IdentityUnavailable {
                        detail: format!("{e:?}"),
                    }
                } else {
                    // A non-transport check error is a deny-shaped outcome — fail closed.
                    FrontDoorError::AuthzDenied {
                        permission: permission.clone(),
                        decision: Decision::Deny,
                    }
                }
            })?;
        if decision != Decision::Allow {
            // Deny AND Conditional fail closed (Conditional is never a silent allow — §8.6).
            return Err(FrontDoorError::AuthzDenied {
                permission,
                decision,
            });
        }

        // 5. placement_of (12.2) — resolve the repo's region-pinned placement.
        let repo_id = RepoId::from_token(repo_placement_key(&token_tenant, &req.url_repo));
        let placement =
            self.placement
                .placement_of(&repo_id)
                .ok_or_else(|| FrontDoorError::NoPlacement {
                    repo: req.url_repo.clone(),
                })?;
        if placement.status == RepoPlacementStatus::Offboarding {
            return Err(FrontDoorError::RepoOffboarding {
                repo: req.url_repo.clone(),
            });
        }

        // 6. residency reject (ADR-11 / 12.4) — a route that would leave the region is REFUSED.
        let pinned = placement.region.clone();
        if pinned != self.home_region {
            return Err(FrontDoorError::OutOfRegion {
                pinned: pinned.as_str().to_string(),
                target: self.home_region.as_str().to_string(),
            });
        }

        // GRANTED — the region-pinned locator the serving tier streams against. Tenant from the
        // TOKEN, region from the PLACEMENT (never the URL path).
        let repo = RepoLoc::new(
            token_tenant,
            pinned.as_str().to_string(),
            req.url_repo.clone(),
        );
        Ok(GrantedRoute {
            principal,
            repo,
            service: req.action.service(),
        })
    }

    /// Advertise refs (the protocol-v2 `info/refs?service=…` / SSH ref-advertisement step). Runs the
    /// SAME gate ([`Self::authorize`]) then streams the advertisement through the seam — a clone
    /// begins with this, so the cross-tenant/residency gate fires HERE too (a foreign-tenant client
    /// never even sees the ref advertisement — 0 cross-tenant read includes ref names).
    pub fn advertise_refs(&self, req: &GitRequest) -> Result<WireOutput, FrontDoorError> {
        let route = self.authorize(req)?;
        self.core
            .advertise_refs(&route.repo, route.service)
            .map_err(|e| FrontDoorError::Wire {
                detail: e.to_string(),
            })
    }

    /// Build the per-request [`CaveatContext`] rider (4.2 §8.6) — the object the caveat scopes,
    /// carrying the action as an attr so a tenant ABAC caveat (e.g. "agents may not push to `main`")
    /// can evaluate against it. The rich attr set lands with the ruleset resolver (GIT-P26); here the
    /// door supplies the action + the object so a caveat needing them returns a decision, not
    /// `Conditional`-for-missing-context.
    fn caveat_for(&self, req: &GitRequest, object: &ArtifactRef) -> Option<CaveatContext> {
        use myelin_identity::Literal;
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert(
            "action".to_string(),
            Literal::Str(match req.action {
                GitAction::Fetch => "fetch".to_string(),
                GitAction::Push => "push".to_string(),
            }),
        );
        Some(CaveatContext {
            object: object.clone(),
            field: None,
            transition: None,
            attrs,
        })
    }
}

// ───────────────────────────── helpers (the one place names are built) ──────────────────────────

/// The `ArtifactRef` a repo authz object is checked against (4.2 object). The grammar is
/// `git:repo:<tenant>/<repo>` — tenant-scoped, so a check is always against the TOKEN tenant's repo
/// (a foreign-tenant principal can never name another tenant's repo object, defence-in-depth atop
/// the cross-tenant guard). Built in ONE place so the check object + any audit reference agree.
fn repo_artifact_ref(tenant: &str, repo: &str) -> ArtifactRef {
    ArtifactRef(format!("git:repo:{tenant}/{repo}"))
}

/// The opaque placement key a repo's [`RepoGitPlacement`] is stored under (`<tenant>/<repo>`) — the
/// storage pack tier keys placements by the tenant-scoped repo id. One place so the resolver + the
/// door agree on the key shape.
fn repo_placement_key(tenant: &str, repo: &str) -> String {
    format!("{tenant}/{repo}")
}

/// Distinguish an Id **transport** failure (the dependency is unreachable → fail-static-eligible,
/// GIT-P14) from an authz **decision** failure (unknown credential / deny → fail-closed deny). The
/// `NotYetImplemented` floor of the M1 Id stub is treated as a transport-style "Id not answering"
/// so the door reports `IdentityUnavailable` (not a silent deny) until the real Id resolver wires —
/// the operator sees "Id floor not wired", not "your key was rejected".
fn is_transport_error(e: &myelin_identity::AuthzError) -> bool {
    matches!(
        e,
        myelin_identity::AuthzError::NotYetImplemented(_)
            | myelin_identity::AuthzError::Unavailable(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Backend, GitCoreError, GitOp};
    use myelin_identity::{
        AuthzError, DataRole, ListObjectsResult, Literal, ObjectId, ObjectType, PrincipalId,
        PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_tenancy::TenantId;
    use std::cell::RefCell;

    // ── a configurable Id stub: resolves a credential → a principal in a given tenant, and a check
    //    policy keyed by permission. The cross-tenant property is enforced by the door, not the stub
    //    (the stub faithfully returns the TOKEN's tenant — the door is what keys the decision on it).
    struct StubId {
        // scheme:material → (tenant, status) the credential resolves to.
        principals: std::collections::HashMap<String, (String, PrincipalStatus)>,
        // permissions the resolved principal is allowed (others Deny).
        allow: Vec<String>,
        // force a transport-style error from authenticate (readiness/unavailable tests).
        authn_unavailable: bool,
        checks_seen: RefCell<Vec<(String, String)>>, // (permission, object)
        // the `action` attr carried on the CaveatContext the last check received (None if the door
        // passed no caveat). Records that the door supplies the ABAC rider (4.2 §8.6).
        last_caveat_action: RefCell<Option<String>>,
    }

    impl StubId {
        fn new() -> Self {
            Self {
                principals: std::collections::HashMap::new(),
                allow: vec!["pull".into(), "push".into()],
                authn_unavailable: false,
                checks_seen: RefCell::new(Vec::new()),
                last_caveat_action: RefCell::new(None),
            }
        }
        fn with_principal(mut self, key: &str, tenant: &str, status: PrincipalStatus) -> Self {
            self.principals
                .insert(key.to_string(), (tenant.to_string(), status));
            self
        }
        fn allowing(mut self, perms: &[&str]) -> Self {
            self.allow = perms.iter().map(|s| s.to_string()).collect();
            self
        }
    }

    impl IdentityService for StubId {
        fn authenticate(&self, c: &Credential) -> IdResult<Principal> {
            if self.authn_unavailable {
                return Err(AuthzError::NotYetImplemented("Id floor not wired"));
            }
            let key = format!("{}:{}", c.scheme, c.material);
            match self.principals.get(&key) {
                Some((tenant, status)) => Ok(Principal::new(
                    TenantId::from_token(tenant.clone()),
                    Region("fr-par".into()),
                    PrincipalId(format!("pid-{}", c.material)),
                    PrincipalKind::Human,
                    DataRole::Controller,
                    *status,
                )),
                None => Err(AuthzError::FailClosed("unknown credential".into())),
            }
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            self.checks_seen
                .borrow_mut()
                .push((permission.0.clone(), object.0.clone()));
            // Record the action attr off the caveat (proves the door supplies the §8.6 rider).
            *self.last_caveat_action.borrow_mut() = cav.and_then(|c| {
                c.attrs.get("action").and_then(|l| match l {
                    Literal::Str(s) => Some(s.clone()),
                    _ => None,
                })
            });
            if self.allow.contains(&permission.0) {
                Ok(Decision::Allow)
            } else {
                Ok(Decision::Deny)
            }
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicyT> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &myelin_identity::RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<myelin_identity::RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> IdResult<myelin_identity::FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }
    use myelin_identity::EffectivePolicy as EffectivePolicyT;

    // ── a placement resolver stub keyed by "<tenant>/<repo>" → (region, status).
    struct StubPlacement {
        placements: std::collections::HashMap<String, (String, RepoPlacementStatus)>,
    }
    impl StubPlacement {
        fn new() -> Self {
            Self {
                placements: std::collections::HashMap::new(),
            }
        }
        fn with(mut self, key: &str, region: &str, status: RepoPlacementStatus) -> Self {
            self.placements
                .insert(key.to_string(), (region.to_string(), status));
            self
        }
    }
    impl PlacementResolver for StubPlacement {
        fn placement_of(&self, repo: &RepoId) -> Option<RepoGitPlacement> {
            self.placements
                .get(repo.as_str())
                .map(|(region, status)| RepoGitPlacement {
                    group: myelin_storage::gitpack::StorageGroup::from_token("g1"),
                    region: Region(region.clone()),
                    status: *status,
                })
        }
    }

    // ── a recording GitCore: records which (repo, service) streamed so the test proves the door only
    //    streams AFTER the gate passes (a denied route records ZERO wire calls — 0 cross-tenant read).
    struct RecCore {
        served: RefCell<Vec<(RepoLoc, Service)>>,
    }
    impl RecCore {
        fn new() -> Self {
            Self {
                served: RefCell::new(Vec::new()),
            }
        }
    }
    impl GitCore for RecCore {
        fn route(&self, op: GitOp) -> Backend {
            crate::core::backend_for(op)
        }
        fn advertise_refs(&self, repo: &RepoLoc, svc: Service) -> Result<WireOutput, GitCoreError> {
            self.served.borrow_mut().push((repo.clone(), svc));
            Ok(WireOutput {
                stdout: b"refs-adv".to_vec(),
                status: 0,
            })
        }
        fn serve(
            &self,
            repo: &RepoLoc,
            svc: Service,
            _stdin: Vec<u8>,
        ) -> Result<WireOutput, GitCoreError> {
            self.served.borrow_mut().push((repo.clone(), svc));
            Ok(WireOutput {
                stdout: b"PACK".to_vec(),
                status: 0,
            })
        }
        fn maintenance(
            &self,
            _r: &RepoLoc,
            _m: crate::core::Maintenance,
        ) -> Result<WireOutput, GitCoreError> {
            unreachable!("front door never runs maintenance")
        }
        fn read_blob(&self, _r: &RepoLoc, _o: &crate::core::Oid) -> Result<Vec<u8>, GitCoreError> {
            unreachable!()
        }
        fn diff_blobs(
            &self,
            _r: &RepoLoc,
            _a: &crate::core::Oid,
            _b: &crate::core::Oid,
        ) -> Result<Vec<crate::core::DiffLine>, GitCoreError> {
            unreachable!()
        }
        fn blame(
            &self,
            _r: &RepoLoc,
            _p: &str,
            _a: &crate::core::Oid,
        ) -> Result<Vec<crate::core::BlameHunk>, GitCoreError> {
            unreachable!()
        }
    }

    fn cred(scheme: &str, material: &str) -> Credential {
        Credential {
            scheme: scheme.to_string(),
            material: material.to_string(),
        }
    }

    #[test]
    fn git_request_debug_cannot_bypass_credential_redaction() {
        let request = GitRequest {
            credential: cred("pat", "secret-bearer"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: Vec::new(),
        };
        let rendered = format!("{request:?}");
        assert!(!rendered.contains("secret-bearer"));
        assert!(rendered.contains("<redacted>"));
    }

    fn door(
        id: StubId,
        placement: StubPlacement,
        core: RecCore,
        home: &str,
    ) -> FrontDoor<StubId, StubPlacement, RecCore> {
        FrontDoor::new(id, placement, core, Region(home.into()))
    }

    // ── 1. happy path: each machine-identity kind resolves → check → placement → stream. ──
    #[test]
    fn fetch_happy_path_authenticates_checks_places_and_streams() {
        for scheme in ["ssh", "deploy_key", "pat", "ci"] {
            let id = StubId::new().with_principal(
                &format!("{scheme}:k1"),
                "acme",
                PrincipalStatus::Active,
            );
            let placement =
                StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
            let core = RecCore::new();
            let d = door(id, placement, core, "fr-par");
            let req = GitRequest {
                credential: cred(scheme, "k1"),
                url_tenant: "acme".into(),
                url_repo: "widgets".into(),
                action: GitAction::Fetch,
                body: b"0000".to_vec(),
            };
            let out = d.route(&req).expect("granted");
            assert_eq!(out.stdout, b"PACK");
            // streamed exactly once, against the region-pinned locator (tenant from token).
            let served = d.core.served.borrow();
            assert_eq!(served.len(), 1);
            assert_eq!(served[0].0, RepoLoc::new("acme", "fr-par", "widgets"));
            assert_eq!(served[0].1, Service::UploadPack);
        }
    }

    // ── 2. GIT-D8: a token whose tenant ≠ the URL-path tenant → tenant from token; 0 cross-tenant
    //       read; refused at the door (NO wire call recorded). ──
    #[test]
    fn git_d8_cross_tenant_token_is_refused_at_the_door_zero_reads() {
        // The token authenticates to `acme`; the URL path addresses `globex`.
        let id = StubId::new().with_principal("pat:stolen", "acme", PrincipalStatus::Active);
        // globex DOES host a repo here — but the acme principal must never reach it.
        let placement =
            StubPlacement::new().with("globex/secret", "fr-par", RepoPlacementStatus::Active);
        let core = RecCore::new();
        let d = door(id, placement, core, "fr-par");
        let req = GitRequest {
            credential: cred("pat", "stolen"),
            url_tenant: "globex".into(), // foreign tenant in the URL path
            url_repo: "secret".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        let err = d.authorize(&req).unwrap_err();
        assert_eq!(
            err,
            FrontDoorError::CrossTenant {
                token_tenant: "acme".into(),
                url_tenant: "globex".into(),
            }
        );
        // THE QUANTIFIED GATE: 0 cross-tenant reads — the door streamed NOTHING.
        assert_eq!(d.core.served.borrow().len(), 0, "0 cross-tenant read");
        // And it never even ran a check against globex's repo object (defence in depth).
        assert!(d.id.checks_seen.borrow().is_empty());
    }

    // ── 3. residency reject: a repo pinned to another region is an out-of-region route → refused. ──
    #[test]
    fn out_of_region_route_is_refused_at_the_door() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        // repo pinned to eu-central, but THIS replica serves fr-par → out-of-region.
        let placement =
            StubPlacement::new().with("acme/widgets", "eu-central", RepoPlacementStatus::Active);
        let core = RecCore::new();
        let d = door(id, placement, core, "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        let err = d.authorize(&req).unwrap_err();
        assert_eq!(
            err,
            FrontDoorError::OutOfRegion {
                pinned: "eu-central".into(),
                target: "fr-par".into(),
            }
        );
        assert_eq!(
            d.core.served.borrow().len(),
            0,
            "0 out-of-region routes admitted"
        );
    }

    // ── 4. authz deny: a principal without `push` is denied the push action (fail-closed). ──
    #[test]
    fn push_without_push_permission_is_denied() {
        let id = StubId::new()
            .with_principal("ssh:k", "acme", PrincipalStatus::Active)
            .allowing(&["pull"]); // pull only — no push
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Push,
            body: vec![],
        };
        let err = d.authorize(&req).unwrap_err();
        assert!(matches!(
            err,
            FrontDoorError::AuthzDenied {
                decision: Decision::Deny,
                ..
            }
        ));
        assert_eq!(d.core.served.borrow().len(), 0);
    }

    // ── 5. unknown credential → unauthenticated (fail-closed; no anonymous route). ──
    #[test]
    fn unknown_credential_is_unauthenticated() {
        let id = StubId::new(); // no principals registered
        let placement = StubPlacement::new();
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "nope"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::Unauthenticated {
                scheme: "ssh".into()
            }
        );
    }

    // ── 6. a disabled principal is refused (ID-D1: zero access). ──
    #[test]
    fn disabled_principal_is_refused() {
        let id = StubId::new().with_principal("pat:old", "acme", PrincipalStatus::Disabled);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("pat", "old"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::PrincipalNotActive {
                status: PrincipalStatus::Disabled
            }
        );
    }

    // ── 7. no placement → fail-closed (never fabricate a placement / serve an unplaced repo). ──
    #[test]
    fn unplaced_repo_is_refused() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement = StubPlacement::new(); // acme/widgets not placed
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::NoPlacement {
                repo: "widgets".into()
            }
        );
    }

    // ── 8. an offboarding repo is refused. ──
    #[test]
    fn offboarding_repo_is_refused() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Offboarding);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert_eq!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::RepoOffboarding {
                repo: "widgets".into()
            }
        );
    }

    // ── 9. the gate runs BEFORE placement: the check sees the TOKEN tenant's repo object, never a
    //       foreign one (defence-in-depth on the artifact-ref grammar). ──
    #[test]
    fn check_object_is_scoped_to_the_token_tenant() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Push,
            body: vec![],
        };
        d.authorize(&req).expect("granted");
        let seen = d.id.checks_seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "push");
        assert_eq!(seen[0].1, "git:repo:acme/widgets");
    }

    // ── 9b. the door supplies the CaveatContext rider (4.2 §8.6) carrying the action — a tenant
    //        ABAC caveat can evaluate against it (so a needed-context caveat is not Conditional). ──
    #[test]
    fn check_carries_the_caveat_action_rider() {
        let id = StubId::new().with_principal("ssh:k", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("acme/widgets", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        d.authorize(&GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Push,
            body: vec![],
        })
        .expect("granted");
        // The check RECEIVED a CaveatContext whose `action` attr is the request's action — proves
        // `caveat_for` returns Some(..) (kills the `caveat_for -> None` mutant).
        assert_eq!(
            d.id.last_caveat_action.borrow().as_deref(),
            Some("push"),
            "the door supplies the §8.6 action rider to check"
        );
    }

    // ── 10. liveness ≠ readiness: liveness is always up; readiness gates on the Id resolver. ──
    #[test]
    fn liveness_is_always_up_readiness_gates_on_id() {
        let id = StubId::new().with_principal("probe:p", "sys", PrincipalStatus::Active);
        let placement = StubPlacement::new();
        let d = door(id, placement, RecCore::new(), "fr-par");
        assert!(d.liveness(), "liveness never checks a backend");
        assert!(
            d.readiness(&cred("probe", "p"), &RepoId::from_token("sys/_probe")),
            "Id reachable → ready"
        );

        // An Id that is not wired (the floor) → not ready, but STILL alive.
        let mut unavailable_id = StubId::new();
        unavailable_id.authn_unavailable = true;
        let d2 = door(
            unavailable_id,
            StubPlacement::new(),
            RecCore::new(),
            "fr-par",
        );
        assert!(d2.liveness(), "liveness stays up even when Id is down");
        assert!(
            !d2.readiness(&cred("probe", "p"), &RepoId::from_token("sys/_probe")),
            "Id unreachable → not ready"
        );
    }

    // ── 11. an Id transport failure fails CLOSED as IdentityUnavailable (not a silent deny). ──
    #[test]
    fn id_transport_failure_fails_closed_as_unavailable() {
        let mut id = StubId::new();
        id.authn_unavailable = true;
        let d = door(id, StubPlacement::new(), RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("ssh", "k"),
            url_tenant: "acme".into(),
            url_repo: "widgets".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert!(matches!(
            d.authorize(&req).unwrap_err(),
            FrontDoorError::IdentityUnavailable { .. }
        ));
        // fail-CLOSED: nothing streamed.
        assert_eq!(d.core.served.borrow().len(), 0);
    }

    // ── 12. advertise_refs runs the same gate — a cross-tenant client never sees the ref adv. ──
    #[test]
    fn advertise_refs_runs_the_same_cross_tenant_gate() {
        let id = StubId::new().with_principal("pat:s", "acme", PrincipalStatus::Active);
        let placement =
            StubPlacement::new().with("globex/secret", "fr-par", RepoPlacementStatus::Active);
        let d = door(id, placement, RecCore::new(), "fr-par");
        let req = GitRequest {
            credential: cred("pat", "s"),
            url_tenant: "globex".into(),
            url_repo: "secret".into(),
            action: GitAction::Fetch,
            body: vec![],
        };
        assert!(matches!(
            d.advertise_refs(&req).unwrap_err(),
            FrontDoorError::CrossTenant { .. }
        ));
        assert_eq!(
            d.core.served.borrow().len(),
            0,
            "no ref adv to a foreign tenant"
        );
    }

    // ── 13. the action → permission/service map is the one source of truth. ──
    #[test]
    fn action_maps_to_permission_and_service() {
        assert_eq!(GitAction::Fetch.permission(), Permission("pull".into()));
        assert_eq!(GitAction::Push.permission(), Permission("push".into()));
        assert_eq!(GitAction::Fetch.service(), Service::UploadPack);
        assert_eq!(GitAction::Push.service(), Service::ReceivePack);
    }

    // ── 14. error Display is distinct + non-empty (kills the fmt→default mutant). ──
    #[test]
    fn error_display_is_distinct_and_nonempty() {
        let xtenant = FrontDoorError::CrossTenant {
            token_tenant: "a".into(),
            url_tenant: "b".into(),
        };
        let region = FrontDoorError::OutOfRegion {
            pinned: "fr-par".into(),
            target: "eu-central".into(),
        };
        let s1 = xtenant.to_string();
        let s2 = region.to_string();
        assert!(s1.contains("CROSS-TENANT") && s1.contains("GIT-D8"));
        assert!(s2.contains("OUT-OF-REGION") && s2.contains("ADR-11"));
        assert_ne!(s1, s2);
    }
}
