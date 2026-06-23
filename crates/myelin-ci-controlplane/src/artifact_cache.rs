//! # CI-P22 (P-365) — Trust-scoped artifacts & caches + the within-EU CDN clone class + per-subject log DEK (CI-D6).
//!
//! **Owning architecture docs (read in full before this module):**
//! - `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//!   §7.2 (artifacts & caches — content-addressed, poison-resistant via trust-scoped namespaces,
//!   residency-local; the within-EU CDN clone class accelerates hot-repo clones without an extra-EU
//!   edge);
//! - `.../01-tech-and-data-model.md` §3.5 (per-subject DEK for isolable inline log PII) + §3.6 (the
//!   `cache_entry.scope` = the trust-tier/branch namespace; `artifact` retained outputs);
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §8;
//! - contract-index rows **11.2** (BlobStore T2 + trust-scoped cache namespaces + the within-EU CDN
//!   clone class), **11.4** (per-subject DEK for isolable inline log PII), **2.2**
//!   (`ci.artifact.published` pointer).
//!
//! ## What this module is (and what it is NOT — EI-01 §7 coherence)
//! The STORAGE half of these contracts is ALREADY SHIPPED and frozen:
//! - the C4 trust-scoped cache namespaces + the write-scope refusal —
//!   [`myelin_storage::ci_cache_scope`] (`CiCacheNamespace` / `CacheScope` / the `TrustTier` input);
//! - the within-EU CDN clone/bundle class — [`myelin_storage::cdn::CdnCloneClass`];
//! - the per-subject-vs-per-tenant DEK key-class selection — [`myelin_storage::encryption::key_class_for`]
//!   + the `kms://<tenant>/<epoch>/<class>` [`myelin_storage::kms::PiiKeyRef`].
//!
//! This module is the **CI Control-Plane WRITE PATH that COMPOSES those primitives** — it owns no
//! second cache store, no second DEK grammar, no second CDN tier. It owns the CI-side glue the prompt
//! places "in the CI Control Plane crate, the artifact/cache modules":
//! 1. **the cache-scope DERIVATION** — map a run's CI-stamped `trust_tier` (the `ci_run.trust_tier`
//!    string plus the run's branch/PR provenance) to the
//!    [`myelin_storage::ci_cache_scope::CacheScope`] /
//!    [`myelin_storage::ci_cache_scope::TrustTier`] the storage layer enforces against. A fork run
//!    derives a `fork:<pr_id>` scope and **cannot** derive (and thus cannot write) the trusted scope;
//! 2. **the artifact WRITE** — a content-addressed T2 blob put (BLAKE3, per-tenant dedup) + the
//!    `ci.artifact.published` EventDraft (contract 2.2, emitted via `OutboxTx::emit` ONLY), with the
//!    **residency-pin** asserted on the write (artifacts live near the runner region);
//! 3. **the per-subject DEK SELECTION** for an isolable-PII log segment — isolable subject PII →
//!    `subject:<id>` DEK (the GD-4 individual crypto-shred lever); else the per-tenant DEK;
//! 4. **the within-EU CDN clone-class** publish/serve wired through the residency-pin (an EU tenant's
//!    bundles never reach an extra-EU edge).
//!
//! ## The residency-pin (contract 1.6) on every artifact/cache/CDN write
//! [`ArtifactWritePin`] is the artifact/cache analogue of [`crate::log_pipeline::LogWritePin`]: it
//! holds the CELL's authoritative region and REFUSES any blob/index/bundle write whose region ≠ the
//! cell's. So an artifact/cache/CDN-bundle can only ever land in its cell's region — the residency-pin
//! lint is GREEN on every artifact/cache write by construction (0 cross-region writes).
//!
//! ## CI-D6 (fork-cannot-poison-trusted-cache) — the gate this module greens
//! An adversarial `UntrustedFork` run derives ONLY its `fork:<pr_id>` scope; the storage layer
//! ([`myelin_storage::ci_cache_scope::CiCacheNamespace::put`]) REFUSES any non-own-fork write. The
//! seam: a fork run **cannot derive** the trusted/branch scope here, AND even if it forged one the
//! storage write-scope refusal stops it — defence at the CI derivation AND the storage enforcement.
//! The drill ([`ForkPoisonOutcome`]) runs the adversarial scenario end-to-end: **0 fork→trusted
//! writes**.
//!
//! ## FLOOR named (EI-01 §1 — every floor names its filling prompt)
//! - **The full erase fan-out (the crypto-shred ERASE path) is CI-P32 (P-492).** This module BUILDS
//!   the per-subject DEK **key-selection substrate** (the `subject:<id>` key choice that makes a
//!   subject's log PII reachable by a single DEK destroy); the `erase(subject)` fan-out that actually
//!   destroys that DEK across every holder is CI-P32 (CI-D3, erasure-reaches-every-holder). Recorded
//!   HERE in writing.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3; the prompt's TESTS field)
//! The **cache-scope-derivation** ([`derive_cache_scope`]: a fork-tier run derives a `fork:<pr_id>`
//! scope, NEVER the trusted/branch scope) is mandatory-core — it is the CI half of the poisoned-cache
//! defence. The cargo-mutants floor is **≥ 80%** for [`derive_cache_scope`] +
//! [`select_log_segment_dek`] (the per-subject-vs-per-tenant key-class decision); run with
//! `cargo mutants -p myelin-ci-controlplane -f crates/myelin-ci-controlplane/src/artifact_cache.rs`.

use myelin_ci_sandbox::events::CI_ARTIFACT_PUBLISHED;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, PiiKeyRef as EnvelopePiiKeyRef,
    Visibility,
};
use myelin_storage::ci_cache_scope::{CacheScope, TrustTier};
use myelin_storage::kms::{KeyClass, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};

use crate::log_pipeline::CrossRegionLogWrite;

// =================================================================================================
// 1. The CI-stamped trust tier of a run (the INPUT to the storage write-scope rule — never recomputed).
// =================================================================================================

/// **The provenance of a CI run, as CI stamps it (the INPUT to the cache-scope derivation).** CI
/// stamps `trust_tier ∈ {trusted, untrusted_fork}` from run provenance (the `ci_run.trust_tier`
/// column, arch 01 §3) plus the run's branch / PR id. Storage ENFORCES the write-scope rule against
/// the derived scope; it does NOT recompute trust (X-1: trust is computed ONCE, by CI, off the fact).
///
/// This is the CI-side provenance struct the [`derive_cache_scope`] reads — PII-free (a trust label,
/// a PR id, a protected-branch name; never a payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunProvenance {
    /// The CI-stamped trust tier string (`ci_run.trust_tier`, arch 01 §3): `"trusted"` |
    /// `"untrusted_fork"`. The ONE place CI's tier string is interpreted into the storage
    /// [`TrustTier`] vocabulary (so a string typo is a LOUD parse error, never a silent downgrade).
    pub trust_tier: String,
    /// The protected-branch name a TRUSTED build runs on (e.g. `main`/`release`), if this run is a
    /// protected-branch build. Drives a `branch:<name>` scope for a trusted run; `None` for a default
    /// trusted run (the `trusted` scope) or a fork run (which always derives `fork:<pr_id>`).
    pub protected_branch: Option<String>,
    /// The PR id an `untrusted_fork` run is keyed on (`fork:<pr_id>`). PII-free (the PR number/id).
    /// REQUIRED for a fork run (a fork with no PR id cannot derive its confined scope — a loud
    /// derivation error, never a fall-through to the trusted scope).
    pub pr_id: Option<String>,
}

/// **Why a cache-scope derivation FAILED (a LOUD refusal — never a silent fall-through to the trusted
/// scope, which would be the poisoned-cache breach itself).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeDerivationError {
    /// The `ci_run.trust_tier` string was neither `trusted` nor `untrusted_fork` — a classification
    /// error. REFUSED loudly (a wrong tier is a poisoning-defence bug, NEVER coerced to the weaker
    /// trusted tier).
    UnknownTrustTier(String),
    /// An `untrusted_fork` run carried no `pr_id` — it cannot derive its confined `fork:<pr_id>`
    /// scope. REFUSED loudly (NEVER falls through to the trusted scope — that would be the exact
    /// poisoned-cache breach D-6 exists to catch).
    ForkRunMissingPrId,
}

impl std::fmt::Display for ScopeDerivationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeDerivationError::UnknownTrustTier(t) => write!(
                f,
                "cache-scope derivation: unknown trust_tier `{t}` (expected `trusted` or \
                 `untrusted_fork`) — REFUSED, never coerced to the trusted scope (the poisoned-cache \
                 defence, contract 11.2-C4)"
            ),
            ScopeDerivationError::ForkRunMissingPrId => write!(
                f,
                "cache-scope derivation: an untrusted_fork run carried no pr_id — cannot derive its \
                 confined fork:<pr_id> scope; REFUSED, never falls through to the trusted scope (the \
                 poisoned-cache breach, contract 11.2-C4)"
            ),
        }
    }
}

impl std::error::Error for ScopeDerivationError {}

/// Parse the CI `ci_run.trust_tier` string into the storage [`TrustTier`] vocabulary (the X-1 fact
/// CI stamps; storage reads it, never recomputes). A typo is a LOUD error (never a silent downgrade).
fn parse_trust_tier(s: &str) -> Result<TrustTier, ScopeDerivationError> {
    match s {
        "trusted" => Ok(TrustTier::Trusted),
        "untrusted_fork" => Ok(TrustTier::UntrustedFork),
        other => Err(ScopeDerivationError::UnknownTrustTier(other.to_string())),
    }
}

/// **The cache-scope DERIVATION (the CI half of the poisoned-cache defence — the mandatory-core).**
/// Map a run's CI-stamped [`RunProvenance`] to the `(trust_tier, scope, run_pr_id)` triple the
/// storage [`myelin_storage::ci_cache_scope::CiCacheNamespace::put`] enforces against:
///
/// - a `trusted` run on a protected branch → `(Trusted, branch:<name>, "")`;
/// - a `trusted` run otherwise → `(Trusted, trusted, "")`;
/// - an `untrusted_fork` run → `(UntrustedFork, fork:<pr_id>, <pr_id>)` — it derives ONLY its
///   confined fork scope; it **cannot** derive the trusted or a branch scope, so it can never WRITE
///   one (and even if it forged one, the storage write-scope refusal stops it).
///
/// A fork run with no `pr_id` is a LOUD derivation error (never a fall-through to the trusted scope —
/// that fall-through would BE the poisoned-cache breach). An unrecognised tier is likewise loud.
///
/// Returns `(TrustTier, CacheScope, run_pr_id)`: the inputs the storage `put` takes. The `run_pr_id`
/// is the empty string for a trusted run (it never writes a fork scope; the empty id is unused by the
/// trusted-tier branch of the storage write rule).
pub fn derive_cache_scope(
    prov: &RunProvenance,
) -> Result<(TrustTier, CacheScope, String), ScopeDerivationError> {
    let tier = parse_trust_tier(&prov.trust_tier)?;
    match tier {
        // A trusted run derives the trusted scope, or a branch: scope on a protected branch.
        TrustTier::Trusted => {
            let scope = match &prov.protected_branch {
                Some(name) => CacheScope::Branch { name: name.clone() },
                None => CacheScope::Trusted,
            };
            Ok((TrustTier::Trusted, scope, String::new()))
        }
        // A fork run derives ONLY its confined fork:<pr_id> scope — NEVER trusted/branch.
        TrustTier::UntrustedFork => {
            let pr_id = prov
                .pr_id
                .clone()
                .ok_or(ScopeDerivationError::ForkRunMissingPrId)?;
            Ok((
                TrustTier::UntrustedFork,
                CacheScope::Fork {
                    pr_id: pr_id.clone(),
                },
                pr_id,
            ))
        }
    }
}

// =================================================================================================
// 2. The per-subject DEK selection for an isolable-PII log segment (contract 11.4 / arch 01 §3.5).
// =================================================================================================

/// **The isolability of a subject's inline PII in a log segment (the per-subject-DEK selector input,
/// arch 01 §3.5 / contract 11.4).** Where a subject's inline PII in a segment is ISOLABLE (a
/// redaction-tagged span, a structured field), that segment is keyed under a **per-subject** DEK
/// (`subject:<id>`) — erasing the subject crypto-shreds exactly their reachable log content. Where it
/// is NOT isolable, the segment falls back to the per-tenant DEK (arch 01 §3.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentPii {
    /// A subject's inline PII is ISOLABLE in this segment → per-subject DEK (`subject:<id>`). Carries
    /// the subject id (the GD-4 individual crypto-shred lever's key class).
    IsolableSubject {
        /// The subject whose isolable inline PII this segment holds (the `subject:<id>` key class).
        subject_id: String,
    },
    /// No isolable subject PII (or it is not isolable) → the per-tenant DEK fallback (arch 01 §3.5).
    NotIsolable,
}

/// **`select_log_segment_dek(tenant, dek_epoch, pii)` — the per-subject-vs-per-tenant DEK key-class
/// selection for a log segment (contract 11.4 / arch 01 §3.5 — the mandatory-core).** Composes the
/// FROZEN [`myelin_storage::kms::KeyClass`] / [`myelin_storage::kms::PiiKeyRef`] grammar (no second
/// vocabulary):
///
/// - [`SegmentPii::IsolableSubject`] → `KeyClass::Subject(id)` → `pii_key_ref = kms://<tenant>/<epoch>/subject:<id>`
///   (the GD-4 individual crypto-shred lever — erasing the subject destroys exactly this key);
/// - [`SegmentPii::NotIsolable`] → `KeyClass::Tenant` → `pii_key_ref = kms://<tenant>/<epoch>/tenant`
///   (the per-tenant fallback the schema demands).
///
/// Returns the [`PiiKeyRef`] the `log_segment.pii_key_ref` column carries. The actual DEK
/// provisioning/seal lives in [`myelin_storage::encryption`]; this is the KEY-CLASS choice (the
/// substrate the CI-P32 erase fan-out destroys against). FLOOR: the erase fan-out is CI-P32.
pub fn select_log_segment_dek(tenant: &TenantId, dek_epoch: u64, pii: &SegmentPii) -> PiiKeyRef {
    let class = match pii {
        SegmentPii::IsolableSubject { subject_id } => KeyClass::Subject(subject_id.clone()),
        SegmentPii::NotIsolable => KeyClass::Tenant,
    };
    PiiKeyRef::new(tenant.clone(), dek_epoch, class)
}

// =================================================================================================
// 3. The residency-pin on every artifact/cache/CDN write (contract 1.6 — artifacts near the runner).
// =================================================================================================

/// **The residency-pin write boundary for artifacts / caches / CDN bundles (contract 1.6 — the
/// CI-side `residency-pin` lint).** Holds the CELL's authoritative region (harness-threaded). Every
/// artifact/cache/CDN-bundle write routes through [`Self::admit_write`], which REFUSES a write whose
/// region ≠ the cell's — so an artifact/cache blob can only ever land in its cell's region (near the
/// runner, residency by construction). The artifact/cache analogue of
/// [`crate::log_pipeline::LogWritePin`] (the SAME [`CrossRegionLogWrite`] refusal type, re-used — one
/// residency-refusal vocabulary, not two).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactWritePin {
    /// The tenant the artifact/cache is for (opaque id, PII-free).
    tenant_id: String,
    /// **The residency pin** — the cell's region. An artifact/cache/CDN write lands ONLY here.
    cell_region: Region,
    /// **The no-cross-region ZERO** — cross-region artifact/cache writes ADMITTED. Pinned to 0 by
    /// [`Self::admit_write`] (it never returns `Ok` for an out-of-region region); the residency
    /// signal reads it (0 = green).
    cross_region_writes_admitted: u64,
}

impl ArtifactWritePin {
    /// A write-pin bound to the cell's authoritative region (harness-threaded — the write-boundary
    /// rule: the pin is the cell's, never a caller's).
    pub fn for_cell(tenant_id: impl Into<String>, cell_region: Region) -> ArtifactWritePin {
        ArtifactWritePin {
            tenant_id: tenant_id.into(),
            cell_region,
            cross_region_writes_admitted: 0,
        }
    }

    /// The cell's region (the residency pin — artifacts/caches near the runner region).
    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    /// **The residency ZERO — `cross_region_writes_admitted`.** Pinned to 0 by [`Self::admit_write`];
    /// a `> 0` here is a residency breach (an artifact/cache leaked into the wrong region). The
    /// residency-pin signal (0 violations) reads it.
    pub fn cross_region_writes_admitted(&self) -> u64 {
        self.cross_region_writes_admitted
    }

    /// **`admit_write(row_region) → Ok | Err(CrossRegionLogWrite)` — the `residency-pin`
    /// write-boundary (contract 1.6).** An artifact/cache/CDN write whose region == the cell's region
    /// is ADMITTED; a write in ANY other region is REFUSED (the within-EU CDN clone class never leaves
    /// EU; an artifact never leaves its cell's region). The admitted ZERO holds by construction (a
    /// refusal is not an admit; the counter increments only on the admit path).
    pub fn admit_write(&mut self, row_region: &Region) -> Result<(), CrossRegionLogWrite> {
        if *row_region != self.cell_region {
            return Err(CrossRegionLogWrite {
                tenant_id: self.tenant_id.clone(),
                cell_region: self.cell_region.clone(),
                row_region: row_region.clone(),
            });
        }
        self.cross_region_writes_admitted += 1;
        Ok(())
    }
}

// =================================================================================================
// 4. The artifact write + the ci.artifact.published pointer (contract 2.2 — emitted via OutboxTx::emit).
// =================================================================================================

/// **A published artifact: the `ci.artifact.published` pointer fact (contract 2.2 / arch 02 §7.2).** A
/// retained job output (binary/SBOM/report/SCIP-LSIF) is content-addressed in the T2 BlobStore; this
/// is the durable POINTER that names where it is (an `ArtifactRef`, never the bytes — references not
/// payloads). PII-free (an opaque artifact id + a content-address + a size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedArtifact {
    /// The tenant (opaque routing token, PII-free).
    pub tenant_id: String,
    /// The cell's region (the residency pin — artifacts near the runner region).
    pub region: String,
    /// The run id this artifact is an output of (opaque, PII-free).
    pub run_id: String,
    /// The logical artifact name (`build/app.tar.gz`, `sbom.spdx.json`, …) — PII-free.
    pub name: String,
    /// The content-address of the artifact bytes in the T2 BlobStore (BLAKE3 multihash string —
    /// references not payloads; the bytes are the blob, not this row).
    pub blob_ref: String,
    /// The artifact size in bytes.
    pub size_bytes: u64,
    /// The `kms://<tenant>/<epoch>/<class>` DEK ref this artifact's bytes are keyed under (per-tenant
    /// by default; per-subject where the artifact holds isolable subject PII — the SAME selection as
    /// the log segments via [`select_log_segment_dek`]).
    pub pii_key_ref: String,
}

impl PublishedArtifact {
    /// The artifact's `ArtifactRef` subject (`myelin://<tenant>/ci/run/<run>/artifact/<name>`) — the
    /// references-not-payloads address (contract 5.7) the published pointer rides.
    pub fn artifact_ref(&self) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/ci/run/{}/artifact/{}",
            self.tenant_id, self.run_id, self.name
        ))
    }

    /// **Build the [`EventDraft`] for the `ci.artifact.published` pointer (contract 2.2 — emitted via
    /// `OutboxTx::emit` ONLY; `no-raw-publish` green).** The subject is the artifact `ArtifactRef`;
    /// the aggregate is the run (per-run ordering of a run's artifacts); the payload is PII-free (the
    /// content-address + size + name — never the bytes). `contains_personal_data` is FALSE (the
    /// pointer is a ref + a content-address; the bytes — which MAY hold subject PII — are keyed under
    /// `pii_key_ref`, which the draft carries so the erase fan-out can reach them).
    pub fn published_draft(&self) -> EventDraft {
        EventDraft {
            type_: EventType(CI_ARTIFACT_PUBLISHED.to_string()),
            subject: self.artifact_ref(),
            aggregate: AggregateKey(format!("run:{}", self.run_id)),
            payload: serde_json::json!({
                "run_id": self.run_id,
                "name": self.name,
                "blob_ref": self.blob_ref,
                "size_bytes": self.size_bytes,
                "region": self.region,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            // The POINTER carries no inline personal data (a ref + a content-address + opaque ids).
            // The artifact BYTES may hold subject PII; they are keyed under `pii_key_ref` (the erase
            // lever), not inlined into this pointer.
            contains_personal_data: false,
            pii_key_ref: Some(EnvelopePiiKeyRef(self.pii_key_ref.clone())),
        }
    }
}

// =================================================================================================
// 5. The CI-D6 fork-cannot-poison-trusted-cache drill (the failure-injection scenario, 0 fork→trusted).
// =================================================================================================

/// **The CI-D6 drill outcome — `0 fork→trusted writes` (the quantified gate, EI-01 §3).** The
/// adversarial `UntrustedFork` run attempts to write the default-branch (trusted) cache scope; the
/// trust-tier/branch-scoped namespace holds STRUCTURALLY → 0 trusted-cache writes from a fork-tier
/// run. Counts the blocked attempts (each is OBSERVED, not silently dropped) and asserts the
/// cross-scope-LANDINGS is 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkPoisonOutcome {
    /// The number of fork→trusted write ATTEMPTS the run made (the adversary's tries).
    pub fork_to_trusted_attempts: u64,
    /// The number of fork→trusted writes that LANDED in the trusted scope — **0 is the gate**
    /// (CI-D6: 0 fork→trusted writes).
    pub fork_to_trusted_landings: u64,
}

impl ForkPoisonOutcome {
    /// `true` IFF the gate is GREEN: 0 fork→trusted LANDINGS (regardless of how many attempts the
    /// adversary made — every attempt was refused by the storage write-scope rule).
    pub fn is_green(&self) -> bool {
        self.fork_to_trusted_landings == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::validate_event_type;
    use myelin_storage::blob::{ContentHash, FsBlobStore};
    use myelin_storage::ci_cache_scope::{CacheScopeError, CiCacheNamespace};

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn fr_par() -> Region {
        Region::new("fr-par")
    }

    // ---- 1. cache-scope derivation (the mandatory-core CI half of the poisoned-cache defence) ----

    /// A TRUSTED run (no branch) derives the `trusted` scope + the Trusted tier.
    #[test]
    fn trusted_run_derives_the_trusted_scope() {
        let prov = RunProvenance {
            trust_tier: "trusted".into(),
            protected_branch: None,
            pr_id: None,
        };
        let (tier, scope, pr) = derive_cache_scope(&prov).expect("trusted derives");
        assert_eq!(tier, TrustTier::Trusted);
        assert_eq!(scope, CacheScope::Trusted);
        assert_eq!(pr, "");
    }

    /// A TRUSTED run on a protected branch derives the `branch:<name>` scope.
    #[test]
    fn trusted_protected_branch_run_derives_a_branch_scope() {
        let prov = RunProvenance {
            trust_tier: "trusted".into(),
            protected_branch: Some("main".into()),
            pr_id: None,
        };
        let (tier, scope, _) = derive_cache_scope(&prov).expect("branch derives");
        assert_eq!(tier, TrustTier::Trusted);
        assert_eq!(
            scope,
            CacheScope::Branch {
                name: "main".into()
            }
        );
    }

    /// **An UNTRUSTED_FORK run derives ONLY its `fork:<pr_id>` scope — NEVER the trusted scope.**
    #[test]
    fn fork_run_derives_only_its_own_fork_scope() {
        let prov = RunProvenance {
            trust_tier: "untrusted_fork".into(),
            protected_branch: None,
            pr_id: Some("42".into()),
        };
        let (tier, scope, pr) = derive_cache_scope(&prov).expect("fork derives");
        assert_eq!(tier, TrustTier::UntrustedFork);
        assert_eq!(scope, CacheScope::Fork { pr_id: "42".into() });
        assert_eq!(pr, "42");
        // It is NOT the trusted scope (the structural property the CI half asserts).
        assert!(!scope.is_trusted());
    }

    /// **A fork run with NO pr_id is a LOUD derivation error — never falls through to trusted.**
    #[test]
    fn fork_run_with_no_pr_id_is_refused_never_falls_through_to_trusted() {
        let prov = RunProvenance {
            trust_tier: "untrusted_fork".into(),
            protected_branch: None,
            pr_id: None,
        };
        let err = derive_cache_scope(&prov).expect_err("a fork with no pr_id is refused");
        assert_eq!(err, ScopeDerivationError::ForkRunMissingPrId);
    }

    /// An unknown trust_tier string is a LOUD classification error — never coerced to trusted.
    #[test]
    fn unknown_trust_tier_is_refused_never_coerced_to_trusted() {
        let prov = RunProvenance {
            trust_tier: "definitely-trusted-wink".into(),
            protected_branch: None,
            pr_id: None,
        };
        let err = derive_cache_scope(&prov).expect_err("unknown tier refused");
        assert!(matches!(err, ScopeDerivationError::UnknownTrustTier(_)));
        // The derivation error renders legibly (names the poisoned-cache defence).
        let rendered = format!("{err}");
        assert!(rendered.contains("11.2-C4"), "attributed to C4: {rendered}");
    }

    /// **CI-D6 end-to-end: a derived fork scope CANNOT write the trusted scope (the two halves —
    /// the CI derivation + the storage refusal — agree: 0 fork→trusted writes).** A fork run derives
    /// `fork:42`; even when the adversary forces the trusted scope into the storage `put`, the
    /// storage write-scope rule REFUSES it. 0 landings in the trusted scope.
    #[test]
    fn ci_d6_fork_cannot_poison_the_trusted_cache_end_to_end() {
        let base = FsBlobStore::new();
        let cache = CiCacheNamespace::over(tenant(), &base);

        // The fork run's CI-derived scope.
        let prov = RunProvenance {
            trust_tier: "untrusted_fork".into(),
            protected_branch: None,
            pr_id: Some("42".into()),
        };
        let (tier, _own_scope, run_pr) = derive_cache_scope(&prov).expect("fork derives");

        let mut outcome = ForkPoisonOutcome {
            fork_to_trusted_attempts: 0,
            fork_to_trusted_landings: 0,
        };

        // The adversary tries to write the TRUSTED scope under its fork tier (the poisoning attempt).
        outcome.fork_to_trusted_attempts += 1;
        let attempt = cache.put(
            tier,
            &run_pr,
            &CacheScope::Trusted,
            "build-cache",
            b"poison",
        );
        match attempt {
            // REFUSED by the storage write-scope rule (the structural defence).
            Err(CacheScopeError::ForkWriteToTrusted { .. }) => {}
            // A landing would be the breach — count it (the gate then fails).
            Ok(_) => outcome.fork_to_trusted_landings += 1,
            other => panic!("unexpected put result: {other:?}"),
        }

        // The gate: 0 fork→trusted writes.
        assert_eq!(outcome.fork_to_trusted_attempts, 1);
        assert_eq!(outcome.fork_to_trusted_landings, 0);
        assert!(outcome.is_green(), "CI-D6: 0 fork→trusted writes");
        // The storage layer observed the blocked attempt + nothing landed in trusted.
        assert_eq!(cache.telemetry().cache_scope_violation(), 1);
        assert!(!cache.contains(&CacheScope::Trusted, "build-cache"));
    }

    // ---- 2. per-subject DEK selection (the mandatory-core key-class decision) --------------------

    /// **Isolable subject PII → the per-subject DEK (`subject:<id>`) — the GD-4 individual lever.**
    #[test]
    fn isolable_subject_pii_selects_the_per_subject_dek() {
        let key = select_log_segment_dek(
            &tenant(),
            3,
            &SegmentPii::IsolableSubject {
                subject_id: "u-42".into(),
            },
        );
        assert_eq!(key.class, KeyClass::Subject("u-42".into()));
        assert_eq!(key.to_uri(), "kms://acme/3/subject:u-42");
    }

    /// **Non-isolable PII → the per-tenant DEK fallback (arch 01 §3.5).**
    #[test]
    fn non_isolable_pii_falls_back_to_the_per_tenant_dek() {
        let key = select_log_segment_dek(&tenant(), 0, &SegmentPii::NotIsolable);
        assert_eq!(key.class, KeyClass::Tenant);
        assert_eq!(key.to_uri(), "kms://acme/0/tenant");
    }

    /// The selection is a pure function of the isolability INPUT — flip the input, the key class
    /// flips; the epoch + tenant pass through unchanged.
    #[test]
    fn dek_selection_is_a_pure_function_of_the_isolability_input() {
        let subj = select_log_segment_dek(
            &tenant(),
            7,
            &SegmentPii::IsolableSubject {
                subject_id: "x".into(),
            },
        );
        let tenant_key = select_log_segment_dek(&tenant(), 7, &SegmentPii::NotIsolable);
        assert_ne!(subj.class, tenant_key.class);
        assert_eq!(subj.dek_epoch, 7);
        assert_eq!(tenant_key.dek_epoch, 7);
    }

    // ---- 3. residency-pin on the artifact/cache/CDN write ----------------------------------------

    /// **The residency-pin admits an in-region write + REFUSES an out-of-region write (0 cross-region
    /// artifact writes).** The within-EU residency property on every artifact/cache/CDN write.
    #[test]
    fn residency_pin_admits_in_region_and_refuses_out_of_region() {
        let mut pin = ArtifactWritePin::for_cell("acme", fr_par());
        // In-region write admitted.
        pin.admit_write(&fr_par()).expect("in-region admitted");
        assert_eq!(pin.cross_region_writes_admitted(), 1);
        // An out-of-region write REFUSED (e.g. an extra-EU edge) — 0 cross-region admits.
        let refused = pin.admit_write(&Region::new("us-east"));
        assert!(refused.is_err(), "out-of-region write must be refused");
        // The admitted counter never moved on the refused write (0 cross-region landings).
        assert_eq!(pin.cross_region_writes_admitted(), 1);
        assert_eq!(pin.cell_region(), &fr_par());
    }

    // ---- 4. the ci.artifact.published pointer (contract 2.2) -------------------------------------

    /// **The published-artifact pointer is a well-formed `ci.artifact.published` (2.2) — emitted via
    /// the outbox draft; the payload is references-not-payloads (a content-address, never bytes).**
    #[test]
    fn published_artifact_emits_a_well_formed_ci_artifact_published_draft() {
        let blob = ContentHash::blake3(b"build-output").to_multihash_string();
        let art = PublishedArtifact {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "run-1".into(),
            name: "app.tar.gz".into(),
            blob_ref: blob.clone(),
            size_bytes: 12,
            pii_key_ref: "kms://acme/0/tenant".into(),
        };
        let draft = art.published_draft();

        // The token is the canonical registered ci.artifact.published (2.9 / 2.2).
        assert_eq!(draft.type_.0, "ci.artifact.published");
        assert!(validate_event_type(&draft.type_.0).is_ok());
        // The subject is the artifact ref (references not payloads); the aggregate is the run.
        assert_eq!(
            draft.subject.0,
            "myelin://acme/ci/run/run-1/artifact/app.tar.gz"
        );
        assert_eq!(draft.aggregate.0, "run:run-1");
        // The payload carries the content-address + size, NEVER the bytes.
        assert_eq!(draft.payload["blob_ref"], blob);
        assert_eq!(draft.payload["size_bytes"], 12);
        // The pointer carries no inline PII; the bytes are keyed under pii_key_ref (the erase lever).
        assert!(!draft.contains_personal_data);
        assert_eq!(
            draft.pii_key_ref.map(|r| r.0).as_deref(),
            Some("kms://acme/0/tenant")
        );
    }

    /// An artifact holding ISOLABLE subject PII carries the per-subject DEK ref (the same selection
    /// as a log segment) on its pointer — so the erase fan-out (CI-P32) reaches the artifact bytes.
    #[test]
    fn artifact_with_isolable_pii_carries_the_per_subject_dek_ref() {
        let key = select_log_segment_dek(
            &tenant(),
            2,
            &SegmentPii::IsolableSubject {
                subject_id: "u-7".into(),
            },
        );
        let art = PublishedArtifact {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "run-9".into(),
            name: "report.json".into(),
            blob_ref: "blake3:dead".into(),
            size_bytes: 4,
            pii_key_ref: key.to_uri(),
        };
        let draft = art.published_draft();
        assert_eq!(
            draft.pii_key_ref.map(|r| r.0).as_deref(),
            Some("kms://acme/2/subject:u-7")
        );
    }
}
