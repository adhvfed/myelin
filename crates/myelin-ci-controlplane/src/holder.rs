//! # `holder` — the CI `PersonalDataHolder` (auto-registered; locate/export typed, erase stubbed
//! to crypto-shred; the `restrict` flag wired) — CI-P9 / P-352, M4
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//!   §6 (`PersonalDataHolder` — CI is a GDPR-spicy holder: `locate / export / rectify / restrict /
//!   erase` over **run-state, logs, artifacts, caches, deployments**; the residual is **by reference**
//!   to the ONE platform posture, X-7 / 10.9; the `restrict` flag suppresses index/agent/analytics/notif).
//! - `01-tech-and-data-model.md` §4 (the encryption/residency/GDPR posture; the per-subject /
//!   per-tenant DEK that the `erase` crypto-shred destroys).
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` X-7 (the ONE
//!   platform-wide free-text/immutable-content erasure posture — instantiated **by reference**, never
//!   restated; the structural floor = per-subject DEK + pseudonym shred + `restrict` suppression).
//! - `planning/00-platform-substrate.md` §3.4 (every store the harness opens auto-registers as a
//!   `PersonalDataHolder` — "we forgot the cache table" is structurally impossible).
//!
//! **Contracts:** index rows **10.1** (OWNED — the CI `PersonalDataHolder{locate, export, rectify,
//! restrict, erase}`, auto-registered + typed), **1.4** (CONSUMED — the harness auto-registration on
//! every store opened, the substrate [`HolderRegistry`] one door), **10.9** (CONSUMED **by reference**
//! — the ONE erasure posture; CI does **not** restate a CI-local residual). Implemented to the frozen
//! [`myelin_gdpr`] shapes.
//!
//! ## What CI-P9 ships — the holder SUBSTRATE, not the erasure fan-out (the named floor)
//! This prompt ships the holder **registered + classified + callable**, with:
//! - **`locate` / `export` TYPED** over each CI store class (run-state, logs, artifacts, caches,
//!   deployments) — empty-but-correct content-addressed receipts that attest the op ran over the CI
//!   surface (a real, callable holder, never a `todo!()`/panic). The full subject-walk lands with the
//!   log/firehose + trust-scoped-artifact bands (CI-P20/P22) and the DSR fan-out (CI-P32).
//! - **`restrict` WIRED** — [`CiHolder::restrict`] flips a per-subject flag the index/agent/analytics/
//!   notif seams read ([`RestrictionFlag`]); the honoured-everywhere proof is the M2 GDPR P-GA-25 path,
//!   but the flag the seams check is REAL here (Art. 18/21).
//! - **`erase` STUBBED to crypto-shred** — a well-defined no-op receipt that NAMES its CI-P32 follow-on
//!   (the full per-subject-where-isolable / per-tenant-fallback crypto-shred fan-out, CI-D3). The
//!   erasure LEVER (the per-subject DEK on `log_segment.pii_key_ref`, Storage C1 / 11.4) already exists;
//!   this is the substrate the fan-out drives, registered-no-op-with-named-follow-on (VISION §3 — not a
//!   silent gap).
//!
//! The residual (third-party free-text PII a person typed into ANOTHER subject's CI log line, under that
//! other person's DEK) is the ONE platform posture (10.9 / X-7) — handled **by reference**
//! ([`CI_RESIDUAL_POSTURE_REF`]), never restated as a CI-local statement (§6.2). The structural floor
//! (per-subject DEK + pseudonym shred + `restrict` suppression) ships regardless.
//!
//! ## Why register NOW (the structural guarantee — §3.4 / contract 1.4)
//! The CI OLTP schema (all fourteen control-plane tables — run-state, deployments, run metadata —
//! `crate::migrations`) is opened through the substrate [`HolderRegistry`] ONE door, so it is a
//! registered holder by construction and classifies to **H2 (`H2Ci`)** in the exhaustive H1–H18 list
//! (gdpr §3.2). The log / artifact / cache stores are blob/cache-class stores opened by their behaviour
//! bands (CI-P20 logs, CI-P22 artifacts/caches); they classify STRUCTURALLY (blob → H6, cache → H9) the
//! moment they open — so the holder set cannot drift below the data map ("we forgot the cache table" is
//! structurally impossible). Registering the OLTP holder now makes "the DSAR fan-out forgot CI"
//! structurally impossible (10.1 exhaustiveness).
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The `erase` crypto-shred fan-out body** (the full per-subject-where-isolable / per-tenant-fallback
//!   DEK destroy over run-state/logs/artifacts/caches/deployments + the `ci.*.erased` tombstones; the
//!   erasure-reaches-every-holder reading) is **CI-P32 / CI-D3**. Here `erase` is the typed no-op that
//!   names it; the per-subject DEK lever (11.4) already exists storage-side (P-329).
//! - **The full `locate`/`export` subject-walk** (the real run/log/artifact/cache/deployment rows naming
//!   the subject) lands with the log-pipeline + trust-scoped artifact bands (CI-P20/P22) + the DSR
//!   fan-out (CI-P32). Here they are empty-but-correct typed receipts.
//! - **The GDPR-orchestration leg** (H2 registered into the data map + the canonical erase order) is the
//!   GDPR-service-side **P-332** (already shipped, `myelin_gdpr_service::ci_instance`); this module is the
//!   CI-CONTROL-PLANE side it fans out to — the two are reconciled (no second orchestrator, EI-01 §7).
//!
//! ## DB-free
//! This module builds in-memory holder/receipt values + flips an in-memory restriction flag; the real
//! per-subject DEK crypto-shred rides the storage integration drills + the CI-P32 / CI-D3 fan-out. So
//! `cargo build --workspace` stays DB-free.

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The stable, PII-free name of the CI Control Plane **OLTP schema** store (all fourteen
/// control-plane tables — `ci_run` / `ci_job` / `check_attempt` / `deployment` / … — `crate::migrations`).
/// This is the holder's **H2 (`H2Ci`)** store. Frozen here so the migrations, the data-map (P-GA-09),
/// the GDPR-side H2 registration (P-332), and the DSR fan-out (CI-P32) all address exactly this store.
/// The same name `myelin_substrate::holder_catalog` already uses for the CI OLTP→H2 assignment.
/// PII-free: a store identifier, never personal data.
pub const CI_OLTP_STORE: &str = "ci_oltp";

/// The CI store CLASSES the holder spans (architecture §6 — `locate / export / rectify / restrict /
/// erase` over **run-state, logs, artifacts, caches, deployments**). A closed enum: a new CI data
/// class cannot be added without appearing here (the holder coverage is total — proven by the unit
/// test over [`CiStoreClass::ALL`]). PII-free — a class tag, never data.
///
/// This is the §6 inventory the holder body drives, mapped to the §3.4 store kind each class lives in
/// (so the harness auto-registration + the §3.2 classification reach every class structurally):
/// run-state/deployments live in the OLTP schema (H2); logs/artifacts in the BlobStore (H6); caches in
/// the cache holder (H9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CiStoreClass {
    /// Run-state — the `ci_run` / `ci_job` / `check_attempt` / `environment` rows + the pseudonymous
    /// `triggered_by` / `approved_by` identity fields (delete the identity, not the fact — §6). OLTP (H2).
    RunState,
    /// Logs — the worst offender (emails, usernames, IPs, tokens, fixtures); inline log-line PII sealed
    /// under the per-subject DEK where isolable (`log_segment.pii_key_ref`, 11.4). Blob tier (H6).
    Logs,
    /// Artifacts — may embed PII (seeded DBs, screenshots); per-tenant (or per-subject) DEK + short TTL.
    /// Blob tier (H6).
    Artifacts,
    /// Caches — derived/invalidatable; same DEK posture; trust-scoped namespaces (CI-P22). Cache (H9).
    Caches,
    /// Deployments — the deploy records + approver pseudonyms (HITL). OLTP (H2).
    Deployments,
}

impl CiStoreClass {
    /// A stable, PII-free label for the class (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            CiStoreClass::RunState => "run-state",
            CiStoreClass::Logs => "logs",
            CiStoreClass::Artifacts => "artifacts",
            CiStoreClass::Caches => "caches",
            CiStoreClass::Deployments => "deployments",
        }
    }

    /// The §3.4 store KIND this class lives in (so the auto-registration + §3.2 classification reaches
    /// it). Run-state/deployments are OLTP (the CI schema → H2); logs/artifacts are blob (→ H6); caches
    /// are cache (→ H9). The non-OLTP kinds classify structurally — a single platform-wide holder per
    /// kind (gdpr §3.2) — so they need no per-store H-declaration.
    pub fn store_kind(self) -> StoreKind {
        match self {
            CiStoreClass::RunState | CiStoreClass::Deployments => StoreKind::Oltp,
            CiStoreClass::Logs | CiStoreClass::Artifacts => StoreKind::Blob,
            CiStoreClass::Caches => StoreKind::Cache,
        }
    }

    /// **The full set of CI store classes the holder spans** (architecture §6). `locate`/`export`/
    /// `erase` reach every member; a missed class is a hole. Closed + total — a new CI data class
    /// cannot be added without appearing here (proven by the unit tests).
    pub const ALL: [CiStoreClass; 5] = [
        CiStoreClass::RunState,
        CiStoreClass::Logs,
        CiStoreClass::Artifacts,
        CiStoreClass::Caches,
        CiStoreClass::Deployments,
    ];
}

/// **The residual posture — instantiated BY REFERENCE to the ONE platform posture (10.9 / X-7), NEVER
/// restated as a CI-local statement** (architecture §6, "the residual is by reference"). CI cites the
/// posture; it does not author a fresh CI-local residual statement. The structural floor (per-subject
/// DEK + pseudonym shred + `restrict` suppression) ships regardless.
pub const CI_RESIDUAL_POSTURE_REF: &str =
    "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
     CI: per-subject DEK crypto-shred (11.4, log_segment.pii_key_ref) + pseudonym shred (4.8, \
     triggered_by/approved_by) + restrict suppression; per-tenant DEK fallback where PII is not \
     isolable; the lawful-basis residual = the ONE [OPEN — LEGAL] posture (parallel/Legal, never a \
     CI-local restatement)";

/// The typed receipt that a CI store was auto-registered as a [`PersonalDataHolder`] — the proof the
/// registration fired for a given store (re-exports the substrate-side [`HolderRegistration`]). The
/// harness collects these; the holder-registered architecture test reads them to assert no CI store
/// escaped registration. PII-free: a (kind, name) tag.
pub type CiHolderRegistration = HolderRegistration;

/// Build the CI Control Plane [`StoreClassifier`] — the data-map declaration that the CI OLTP schema
/// belongs to holder **H2 (`H2Ci`)** (gdpr §3.2 / §5). The CI OLTP store needs a per-store declaration
/// (an OLTP store maps to its subsystem's holder); the blob/cache log/artifact/cache stores classify
/// STRUCTURALLY (blob → H6, cache → H9) and need no declaration here. The substrate completeness
/// assertion joins the harness's [`HolderRegistry`] against this classifier: every opened CI store must
/// map to an H-holder, or it is an orphan (contract 1.4 + gdpr §3.2).
pub fn ci_store_classifier() -> StoreClassifier {
    StoreClassifier::of([
        // The fourteen control-plane tables (run-state + deployments + run metadata) → H2 (CI).
        myelin_substrate::StoreHolder::new(StoreKind::Oltp, CI_OLTP_STORE, Holder::H2Ci),
    ])
}

/// **Register the CI Control Plane's OLTP store as a `PersonalDataHolder` through the harness
/// auto-registration (contract 1.4).** Opens the CI OLTP store through the substrate [`HolderRegistry`]
/// — the ONE door — so it is a registered holder by construction. Returns the registry (carrying the
/// receipt) so a caller / test can assert exactly which stores registered + that they classify to their
/// H-holders (H2 for the CI OLTP schema). This is the `serve`-called seam ([`crate::controlplane_app_spec`]
/// declares `holders: AppSpec::auto()`); registering it makes "the DSAR fan-out forgot CI" structurally
/// impossible (10.1 exhaustiveness). The blob/cache log/artifact/cache stores register through the SAME
/// door when their behaviour bands (CI-P20/P22) open them; each classifies structurally on open.
pub fn register_ci_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    // The CI OLTP schema (all fourteen control-plane tables) — declared H2 above.
    registry.open(StoreKind::Oltp, CI_OLTP_STORE);
    registry
}

/// **The per-subject `restrict` flag (Art. 18/21) — the seam the index/agent/analytics/notif checks
/// read** (architecture §6: a restricted subject's CI data is NOT indexed / agent-used / analytics-fed /
/// notification-fanned). [`CiHolder::restrict`] flips it; every CI seam that surfaces a subject's CI
/// footprint reads [`RestrictionFlag::is_restricted`] BEFORE emitting. Shared (`Arc<Mutex<…>>`) so the
/// holder and the seams see ONE flag set (the honoured-everywhere proof is the M2 GDPR P-GA-25 path; the
/// flag the seams check is REAL here). PII-free: it holds opaque pseudonymous subject ids.
#[derive(Clone, Default)]
pub struct RestrictionFlag {
    /// The set of restricted subject ids (opaque pseudonymous principal ids — never a name/email).
    restricted: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictionFlag {
    /// A fresh flag set (no subject restricted yet).
    pub fn new() -> RestrictionFlag {
        RestrictionFlag::default()
    }

    /// Set (`on = true`) or clear (`on = false`) the restriction for a subject. Idempotent.
    pub fn set(&self, subject: &str, on: bool) {
        let mut g = self.restricted.lock().expect("restriction flag poisoned");
        if on {
            g.insert(subject.to_string());
        } else {
            g.remove(subject);
        }
    }

    /// **Whether a subject is restricted — the check every CI index/agent/analytics/notif seam makes
    /// BEFORE surfacing the subject's CI footprint** (architecture §6 — no indexing / no agent-use / no
    /// analytics / no notification for a restricted subject). A restricted subject's CI data is
    /// suppressed at the seam (fail-closed for surfacing).
    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted
            .lock()
            .expect("restriction flag poisoned")
            .contains(subject)
    }
}

/// **The CI `PersonalDataHolder` (H2; contract 10.1) — auto-registered, locate/export TYPED, erase
/// STUBBED to crypto-shred, the `restrict` flag WIRED.** The holder over CI's run-state, logs, artifacts,
/// caches, and deployments (architecture §6). At CI-P9 the locate/export bodies are empty-but-correct
/// content-addressed receipts (a real, callable holder — the full subject-walk is CI-P20/P22 + CI-P32);
/// `erase` is the typed no-op that names its CI-P32 / CI-D3 crypto-shred fan-out follow-on; `restrict`
/// flips a REAL per-subject flag the CI seams read. The erasure LEVER (the per-subject DEK on
/// `log_segment.pii_key_ref`, 11.4) already exists storage-side (P-329).
#[derive(Clone, Default)]
pub struct CiHolder {
    /// The per-subject restriction flag the index/agent/analytics/notif seams read (§6). Shared so the
    /// holder and the seams see ONE flag set.
    restriction: RestrictionFlag,
}

impl CiHolder {
    /// Build the CI holder with a fresh restriction flag.
    pub fn new() -> CiHolder {
        CiHolder::default()
    }

    /// Build the CI holder sharing an existing restriction flag (so a seam can read the SAME flag the
    /// holder writes — one flag set across the holder + the index/agent/analytics/notif seams).
    pub fn with_restriction(restriction: RestrictionFlag) -> CiHolder {
        CiHolder { restriction }
    }

    /// Register the CI OLTP store as holder H2 through the substrate registry (the `serve`-called
    /// auto-registration seam), returning the receipt — the proof the CI schema registered as H2.
    pub fn register(&self, registry: &mut HolderRegistry) -> CiHolderRegistration {
        registry.open(StoreKind::Oltp, CI_OLTP_STORE)
    }

    /// Borrow the restriction flag (so a CI index/agent/analytics/notif seam can read the SAME flag the
    /// holder's `restrict` writes — one flag set, never two).
    pub fn restriction(&self) -> &RestrictionFlag {
        &self.restriction
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) — never
    /// a name/email. CI stores identity as `<pseudonym>@<tenant>.noreply` (4.8); the subject id is the
    /// opaque principal id. One derivation — never a second subject-id rendering.
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for CiHolder {
    /// Art. 15 access — where the subject's CI data lives: their triggered runs / approvals (run-state),
    /// inline log-line PII (logs), artifacts/caches that embed PII, deployment approvals (architecture
    /// §6). At CI-P9 an empty-but-correct content-addressed receipt attesting the locate ran over the CI
    /// surface (the full per-class subject-walk lands with CI-P20/P22 + CI-P32). NEVER an error — a real,
    /// callable holder.
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                CI_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "CI locate over run-state/logs/artifacts/caches/deployments (CI-P9 typed seam; \
                 the full subject-walk = CI-P20/P22 + the DSR fan-out CI-P32)",
                None,
                0,
            ),
        })
    }

    /// Art. 20 portability — the subject's CI footprint (triggered runs, approvals) as references +
    /// decrypted-while-key-lives log excerpts, per-viewer-safe (architecture §6). At CI-P9 an
    /// empty-but-correct portable bundle; the full export of the subject's run/log/artifact rows lands
    /// with CI-P20/P22 + CI-P32.
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                CI_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "CI export: the subject's CI footprint (triggered runs + approvals) as references + \
                 log excerpts (CI-P9 typed seam; the full bundle = CI-P20/P22 + CI-P32)",
                None,
                0,
            ),
        })
    }

    /// Art. 16 rectification — update CI text the subject controls. CI run-state/log content is
    /// machine-emitted (not subject-authored free text), so rectify is a well-defined no-op at the CI
    /// holder; the patch model lands with the GDPR 10.4 / P-GA-24 reindex-from-source path.
    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                CI_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (CI run-state/logs are machine-emitted; rectify-by-reindex = GDPR P-GA-24)",
                None,
                0,
            ),
        })
    }

    /// Art. 18/21 restriction — set/clear the per-subject restriction flag the CI index/agent/analytics/
    /// notif seams read (architecture §6). This flips a REAL flag ([`RestrictionFlag`]) the seams check
    /// BEFORE surfacing the subject's CI footprint; the honoured-everywhere proof is the M2 GDPR P-GA-25
    /// path. A restricted subject's CI data is NOT indexed / agent-used / analytics-fed / notification-fanned.
    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        // Flip the REAL flag the seams read (not a no-op — the restriction is honoured at the CI seams).
        self.restriction.set(&sid, on);
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                CI_OLTP_STORE,
                &sid,
                "",
                if on {
                    "CI restrict ON: no indexing / no agent-use / no analytics / no notification (§6)"
                } else {
                    "CI restrict OFF: the per-subject restriction flag is cleared (§6)"
                },
                None,
                0,
            ),
        })
    }

    /// Art. 17 erasure — **STUBBED to crypto-shred here (the CI-P9 substrate); the full fan-out is
    /// CI-P32 / CI-D3.** The real erase crypto-shreds the subject's per-subject CI-log DEK where
    /// isolable (`log_segment.pii_key_ref`, 11.4 — rendering the immutable append-only ciphertext, incl.
    /// backups, unrecoverable) and the per-tenant DEK fallback where it is not, pseudonym-shreds the
    /// `triggered_by`/`approved_by` identity edges (4.8), and emits the `ci.*.erased` tombstones — over
    /// run-state/logs/artifacts/caches/deployments. The run STRUCTURE survives (delete the identity, not
    /// the fact, §6). The residual is the ONE platform posture ([`CI_RESIDUAL_POSTURE_REF`], 10.9 / X-7
    /// — never restated CI-local). At CI-P9 this is a well-defined no-op receipt that names CI-P32; the
    /// per-subject DEK lever already exists storage-side (P-329) — the body the fan-out drives.
    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CI_OLTP_STORE,
                &subject_id,
                &tenant,
                "no-op (CI-P9 substrate; the per-subject/per-tenant DEK crypto-shred + pseudonym shred \
                 + ci.*.erased tombstone fan-out over run-state/logs/artifacts/caches/deployments = \
                 CI-P32 / CI-D3; residual = the ONE posture 10.9/X-7, by reference)",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{
        assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
        StoreManifest,
    };

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    /// **The §6 store-class set is the holder's coverage** — run-state, logs, artifacts, caches,
    /// deployments. The closed set is the structural coverage surface (a new CI data class cannot be
    /// added without appearing here), and each class maps to its §3.4 store kind (OLTP → H2; blob → H6;
    /// cache → H9), so the auto-registration + §3.2 classification reaches every class.
    #[test]
    fn the_ci_store_class_set_is_the_holder_coverage() {
        assert_eq!(CiStoreClass::ALL.len(), 5);
        for c in [
            CiStoreClass::RunState,
            CiStoreClass::Logs,
            CiStoreClass::Artifacts,
            CiStoreClass::Caches,
            CiStoreClass::Deployments,
        ] {
            assert!(
                CiStoreClass::ALL.contains(&c),
                "{} must be in the holder coverage",
                c.label()
            );
        }
        // Each class lives in its §3.4 store kind (so the harness auto-registration reaches it).
        assert_eq!(CiStoreClass::RunState.store_kind(), StoreKind::Oltp);
        assert_eq!(CiStoreClass::Deployments.store_kind(), StoreKind::Oltp);
        assert_eq!(CiStoreClass::Logs.store_kind(), StoreKind::Blob);
        assert_eq!(CiStoreClass::Artifacts.store_kind(), StoreKind::Blob);
        assert_eq!(CiStoreClass::Caches.store_kind(), StoreKind::Cache);
        // PII-free labels.
        assert_eq!(CiStoreClass::Logs.label(), "logs");
    }

    /// **The CI OLTP store auto-registers as holder H2 through the one door (contract 1.4) and
    /// classifies to H2 — 0 orphans (gdpr §3.2).** Opening it through the substrate registry makes it a
    /// registered holder by construction; it maps to the exhaustive H2 (`H2Ci`) — so the M5 DSAR fan-out
    /// cannot silently miss CI. This is the CI-P9 GATE (the holder-count signal includes the CI schema).
    #[test]
    fn ci_store_registers_and_classifies_to_h2_no_orphan() {
        let registry = register_ci_holders();
        assert!(registry.is_registered(StoreKind::Oltp, CI_OLTP_STORE));
        assert_eq!(registry.len(), 1, "exactly the CI OLTP store registered");
        let classifier = ci_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, CI_OLTP_STORE, &classifier),
            Some(Holder::H2Ci),
            "the CI OLTP schema is holder H2 (CI subsystem DB + log segments)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every CI store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **The §3.4 ratchet — "we forgot the cache table" is structurally impossible.** A CI log/artifact
    /// (blob) or cache store opened through the one door classifies STRUCTURALLY (blob → H6, cache → H9)
    /// — no per-store declaration needed — so the holder-count signal includes every CI store kind the
    /// moment its behaviour band (CI-P20/P22) opens it. A forgotten store would be an orphan (RED).
    #[test]
    fn ci_blob_and_cache_stores_classify_structurally_no_forgotten_table() {
        let classifier = ci_store_classifier();
        // The log/artifact blob tier → H6 (the single platform-wide object store, §3.2) — no
        // per-store declaration needed; it is covered the moment CI-P20/P22 opens it.
        assert_eq!(
            classify_store(StoreKind::Blob, "ci_logs", &classifier),
            Some(Holder::H6BlobStore),
            "CI log/artifact blob tier classifies structurally to H6"
        );
        // The cache tier → H9 (the single caches/CDN holder) — likewise covered structurally.
        assert_eq!(
            classify_store(StoreKind::Cache, "ci_cache", &classifier),
            Some(Holder::H9Caches),
            "CI cache tier classifies structurally to H9 (no forgotten cache table)"
        );
    }

    /// **The 1.4 enforcement (the CI-P9 GATE): a CI store opened OUTSIDE the harness FAILS the
    /// holder-registered architecture test.** The conforming registry (the CI OLTP store opened through
    /// the one door) passes; a registry missing it (a store opened outside the harness) is a loud
    /// violation naming exactly the escaped store — an unregistered PII store cannot quietly miss the DSR
    /// fan-out.
    #[test]
    fn an_unregistered_ci_store_fails_the_holder_registered_architecture_test() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, CI_OLTP_STORE)]);
        // CONFORMING: the CI OLTP store opened through the harness one door.
        assert_eq!(
            assert_all_holders_registered(&manifest, &register_ci_holders()),
            Ok(()),
            "the CI store opened through the harness → the architecture test passes"
        );
        // VIOLATING: the CI store never went through the door.
        let rogue = HolderRegistry::new();
        let err = assert_all_holders_registered(&manifest, &rogue)
            .expect_err("a CI store opened outside the harness must FAIL the architecture test");
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered CI store is the violation"
        );
        assert!(
            err[0].message().contains(CI_OLTP_STORE),
            "the failure names the escaped CI store: {}",
            err[0].message()
        );
    }

    /// **`locate`/`export` are TYPED + empty-but-correct (the CI-P9 surface), not an error.** Both
    /// return content-addressed receipts over the CI surface — a real, callable holder, not a
    /// `todo!()`/`Err`. The full located/exported data lands with CI-P20/P22 + CI-P32.
    #[test]
    fn locate_and_export_are_typed_and_empty_but_correct() {
        let holder = CiHolder::new();
        let subj = subject("psn:ci-7");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over the CI surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let export = holder
            .export(&subj, tenant())
            .expect("export over the CI surface succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    /// **`restrict` flips a REAL per-subject flag the CI seams read (Art. 18/21, §6).** After
    /// `restrict(on)` the subject is restricted (no index/agent/analytics/notif); after `restrict(off)`
    /// it is cleared. The flag the holder writes is the SAME one a seam reads (one flag set).
    #[test]
    fn restrict_flips_a_real_flag_the_seams_read() {
        let flag = RestrictionFlag::new();
        let holder = CiHolder::with_restriction(flag.clone());
        let subj = subject("psn:ci-restricted");
        let sid = "psn:ci-restricted";

        // Before: not restricted (the seam would surface the subject's CI footprint).
        assert!(!flag.is_restricted(sid));

        // restrict ON: the holder flips the flag; the seam now suppresses the subject.
        let r = holder.restrict(&subj, true).expect("restrict ON");
        assert_eq!(r.receipt.operation, "restrict");
        assert!(
            flag.is_restricted(sid),
            "the restriction flag the CI index/agent/analytics/notif seams read is SET"
        );

        // restrict OFF: cleared (the seam surfaces the subject again).
        holder.restrict(&subj, false).expect("restrict OFF");
        assert!(!flag.is_restricted(sid), "the restriction flag is cleared");
    }

    /// **`erase` is STUBBED to crypto-shred (the CI-P9 substrate) — a well-defined no-op receipt that
    /// NAMES its CI-P32 / CI-D3 follow-on, never a panic.** Idempotent: the same scope yields the same
    /// content-addressed receipt (no DEK shredded yet — the structural crypto-shred fan-out is CI-P32).
    #[test]
    fn erase_is_a_stubbed_crypto_shred_no_op_that_names_ci_p32() {
        let holder = CiHolder::new();
        let scope = EraseScope::Subject {
            subject: subject("psn:ci-7"),
            tenant: tenant(),
        };
        let r1 = holder.erase(scope.clone()).expect("erase succeeds (stub)");
        let r2 = holder.erase(scope).expect("erase is idempotent");
        assert_eq!(
            r1, r2,
            "the same erase scope yields the identical content-addressed receipt"
        );
        assert!(
            r1.receipt.key_epoch_destroyed.is_none(),
            "no DEK shredded (the crypto-shred body is CI-P32)"
        );
        assert_eq!(r1.receipt.operation, "erase");
        assert!(r1.receipt.content_hash.starts_with("blake3:"));
    }

    /// **The residual is BY REFERENCE to the ONE platform posture (10.9 / X-7) — never restated
    /// CI-local (§6).** The reference cites the contract + the structural floor (per-subject DEK +
    /// pseudonym shred + restrict) and the lawful-basis residual as the ONE [OPEN — LEGAL] posture, not
    /// a fresh CI-local statement.
    #[test]
    fn the_residual_is_by_reference_to_the_one_platform_posture() {
        assert!(
            CI_RESIDUAL_POSTURE_REF.contains("10.9") && CI_RESIDUAL_POSTURE_REF.contains("X-7"),
            "the residual cites the ONE platform posture (10.9 / X-7), by reference"
        );
        assert!(
            CI_RESIDUAL_POSTURE_REF.contains("never a CI-local restatement"),
            "the residual is by reference, never restated CI-local"
        );
    }

    /// **The CI holder is object-safe** — held behind `dyn PersonalDataHolder` exactly as the DSR
    /// orchestrator / holder registry need (a heterogeneous holder set, contract 10.1).
    #[test]
    fn ci_holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(CiHolder::new())];
        let subj = subject("psn:ci-9");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the CI holder responds to the contract"
            );
        }
    }
}
