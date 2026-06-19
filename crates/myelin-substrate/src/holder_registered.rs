//! # The `holder-registered` architecture test (contract 1.4 — the enforcement half) — P-GA-04
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §3.1 (the harness
//! auto-registers every store; **a store opened OUTSIDE the harness fails the
//! `holder-registered` architecture test** — the holder list cannot drift below the data map)
//! and `00-platform-substrate.md` §3.4 (auto-register every store the harness opens).
//!
//! **Contract-index:** row 1.4 (`PersonalDataHolder` auto-registration) — the **enforcement**
//! side. **P-GA-04 → global P-055.** DEPENDS-ON P-GA-01 (the trait the registry stores),
//! P-S12/P-S15 (the [`crate::serve`] store-opening hook + the [`crate::holders::HolderRegistry`]
//! auto-registration mechanism this test enforces).
//!
//! ## Why this module exists (the gap P-055 fills over P-032)
//! P-032 (P-S15) shipped the auto-registration **mechanism**: `serve`, opening a store, threads
//! it through [`crate::holders::HolderRegistry::open`] (opening IS registering). That proved a
//! harness-opened store *does* register. It did NOT prove the **enforcement** the GDPR doctrine
//! requires: that a store opened **outside** the harness (bypassing the one door) is a hard CI
//! failure, not a silent miss. This module ships exactly that enforcement — the
//! [`holder_registered`] architecture test + a **violating fixture** (a store opened outside the
//! harness ⇒ the test reports a violation) and a **conforming fixture** (a harness-opened store ⇒
//! the test passes). This is the structural realization of "the holder list cannot drift below
//! the data map" (gdpr §3.1): a developer who opens a store without going through the harness
//! does not get a quiet un-registered store — they get a red architecture test.
//!
//! ## The mechanism (how "opened outside the harness" becomes a build failure)
//! A service declares the set of stores it owns as a [`StoreManifest`] (the data-map's view of
//! "every store this service has"). The harness, booting, registers every store it opens into a
//! [`HolderRegistry`]. The architecture test [`holder_registered`] joins the two: **every store
//! the manifest declares MUST appear in the registry.** A store declared but never threaded
//! through [`HolderRegistry::open`] — i.e. opened outside the harness's one door — is **absent**
//! from the registry, so the join finds it missing and returns a [`HolderViolation`]. The test
//! turns that into a CI failure via [`assert_all_holders_registered`] (a captured-expected
//! failure in the violating fixture; a pass in the conforming one).
//!
//! The manifest is the data map's anchor: the GDPR data-map generator (P-GA-09) walks the SAME
//! manifest + registry to assert no holder escaped — this test is the build-time tripwire that
//! keeps the registry from ever drifting below it.
//!
//! ## Floors named (deferred bodies → filling prompt) — VISION §3 name-your-floors
//! - The **registry holds placeholder entries** (a `(kind, name)` registration per opened store,
//!   the [`HolderRegistration`]) until the holder BODIES (`locate/export/rectify/restrict/erase`)
//!   land — that is **M1 P-GA-05** (the GDPR-owned holders + the trait bodies). Here the
//!   registration + the enforcement test exist; the bodies are the named floor.
//! - The **data-map generator that walks the registry** (turning the registered-holder set into
//!   the machine-readable RoPA inventory) is **M1 P-GA-07/P-GA-09**. This test is the structural
//!   tripwire that generator relies on (the registry never drifts below the manifest).
//! - The manifest here is **declared by the service**; the M1 stores' real manifests (Identity's
//!   principal/pseudonym/tuple stores, Storage's blob tiers, …) are declared as those stores ship
//!   (P-ID-05/P-ST-06/…). The MECHANISM + the enforcement are frozen now.

use crate::holders::{HolderRegistry, StoreKind};
use std::collections::BTreeSet;

/// One store a service declares it owns (the data-map's "this service has this store"). PII-free:
/// a `(kind, name)` tag, never personal data. The architecture test asserts every declared store
/// was auto-registered through the harness's one door — a declared-but-unregistered store is the
/// "opened outside the harness" violation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclaredStore {
    /// The store's class (§3.4 — OLTP / blob / cache / search index).
    pub kind: StoreKind,
    /// The store's stable, PII-free name (the holder identifier).
    pub name: &'static str,
}

impl DeclaredStore {
    /// A declared store of a given kind + name.
    pub fn new(kind: StoreKind, name: &'static str) -> DeclaredStore {
        DeclaredStore { kind, name }
    }

    /// The fully-qualified, PII-free holder id (`<kind>:<name>`) — the same address the registry
    /// keys on, so the join between "declared" and "registered" is exact.
    pub fn holder_id(&self) -> String {
        format!("{}:{}", self.kind.label(), self.name)
    }
}

/// The set of stores a service declares it owns — the data-map's view of "every store this
/// service has" (gdpr §3.1). The architecture test joins this against the [`HolderRegistry`] the
/// harness populated: every declared store MUST be registered. The manifest is the anchor the
/// holder list cannot drift below (a declared store that never registered = a store opened
/// outside the harness = a violation).
#[derive(Clone, Debug, Default)]
pub struct StoreManifest {
    declared: Vec<DeclaredStore>,
}

impl StoreManifest {
    /// An empty manifest (a service that declares no stores yet).
    pub fn new() -> StoreManifest {
        StoreManifest { declared: Vec::new() }
    }

    /// Build a manifest from a set of declared stores.
    pub fn of(stores: impl IntoIterator<Item = DeclaredStore>) -> StoreManifest {
        StoreManifest { declared: stores.into_iter().collect() }
    }

    /// Declare one store this service owns.
    pub fn declare(&mut self, kind: StoreKind, name: &'static str) -> &mut StoreManifest {
        self.declared.push(DeclaredStore::new(kind, name));
        self
    }

    /// The declared stores.
    pub fn stores(&self) -> &[DeclaredStore] {
        &self.declared
    }

    /// The PII-free holder ids the manifest declares (the data-map address book).
    pub fn holder_ids(&self) -> BTreeSet<String> {
        self.declared.iter().map(DeclaredStore::holder_id).collect()
    }
}

/// A store the data map declares the service owns that the registry says was **never
/// auto-registered** — i.e. opened OUTSIDE the harness's one door (bypassing
/// [`HolderRegistry::open`]). This is the `holder-registered` architecture-test violation: a
/// store that escaped registration would also escape the DSR fan-out, so it is a structural
/// (build-time) failure, never a quiet miss (gdpr §3.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HolderViolation {
    /// The declared-but-unregistered store (the holder that escaped registration).
    pub store: DeclaredStore,
}

impl HolderViolation {
    /// A loud, PII-free message naming the offending store + WHY it failed (it was opened
    /// outside the harness, so it never registered as a `PersonalDataHolder`).
    pub fn message(&self) -> String {
        format!(
            "holder-registered architecture test FAILED: store `{}` is declared in the data map \
             but was NOT auto-registered as a PersonalDataHolder — it was opened OUTSIDE the \
             harness (bypassing HolderRegistry::open). Open it through serve(AppSpec)/the harness \
             so opening IS registering (gdpr §3.1, contract 1.4).",
            self.store.holder_id()
        )
    }
}

/// **The `holder-registered` architecture test (contract 1.4 — the enforcement).** Joins the
/// service's declared [`StoreManifest`] against the [`HolderRegistry`] the harness populated and
/// returns every declared store that was NOT auto-registered (opened outside the harness). An
/// **empty** result is the conforming verdict (every store went through the one door); a
/// **non-empty** result is the set of violations (stores that escaped registration).
///
/// This is the pure, inspectable core; [`assert_all_holders_registered`] is the CI-gate wrapper
/// that turns a non-empty result into a build failure.
pub fn holder_registered(
    manifest: &StoreManifest,
    registry: &HolderRegistry,
) -> Vec<HolderViolation> {
    manifest
        .stores()
        .iter()
        .filter(|s| !registry.is_registered(s.kind, s.name))
        .map(|s| HolderViolation { store: *s })
        .collect()
}

/// **The CI gate (contract 1.4): every declared store must be auto-registered.** Runs the
/// [`holder_registered`] architecture test; returns `Ok(())` if every declared store registered
/// through the harness (the conforming verdict), or `Err(violations)` naming every store that was
/// opened outside the harness (the violating verdict). A service's `holder-registered`
/// architecture test asserts this is `Ok` — a store opened outside the harness makes it `Err`,
/// which the test surfaces as a loud CI failure (gdpr §3.1; EI-01 §5 — a committed gate).
pub fn assert_all_holders_registered(
    manifest: &StoreManifest,
    registry: &HolderRegistry,
) -> Result<(), Vec<HolderViolation>> {
    let violations = holder_registered(manifest, registry);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The CONFORMING fixture (the green verdict).** A service declares an OLTP store and opens
    /// it through the harness's one door (`HolderRegistry::open`) — so it auto-registers. The
    /// `holder-registered` architecture test finds NO violation: opening IS registering.
    #[test]
    fn conforming_store_opened_through_the_harness_passes() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "issue_oltp")]);
        // The harness opens the declared store through the ONE door → it auto-registers.
        let mut registry = HolderRegistry::new();
        registry.open(StoreKind::Oltp, "issue_oltp");

        assert!(
            holder_registered(&manifest, &registry).is_empty(),
            "a harness-opened store registers; no violation"
        );
        assert_eq!(
            assert_all_holders_registered(&manifest, &registry),
            Ok(()),
            "the conforming fixture passes the holder-registered architecture test"
        );
    }

    /// **The VIOLATING fixture (the captured-expected failure).** A service declares an OLTP store
    /// but opens it OUTSIDE the harness (it never threads through `HolderRegistry::open`), so it is
    /// absent from the registry. The `holder-registered` architecture test FAILS, naming the store
    /// + why — a store opened outside the harness is a build failure, never a silent miss.
    #[test]
    fn violating_store_opened_outside_the_harness_fails() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "rogue_oltp")]);
        // The store was opened OUTSIDE the harness: the registry was never told about it.
        let registry = HolderRegistry::new();

        let violations = holder_registered(&manifest, &registry);
        assert_eq!(
            violations,
            vec![HolderViolation { store: DeclaredStore::new(StoreKind::Oltp, "rogue_oltp") }],
            "the store opened outside the harness is the violation"
        );
        // The CI gate surfaces it as a failure (captured-expected).
        let err = assert_all_holders_registered(&manifest, &registry)
            .expect_err("a store opened outside the harness must FAIL the architecture test");
        assert_eq!(err.len(), 1);
        // the message is loud + names the offending store + the fix.
        let msg = err[0].message();
        assert!(msg.contains("rogue_oltp"), "the failure names the offending store: {msg}");
        assert!(msg.contains("OUTSIDE the harness"), "the failure names WHY: {msg}");
        assert!(msg.contains("HolderRegistry::open"), "the failure names the one door: {msg}");
    }

    /// A PARTIAL violation: some declared stores registered, one did not. The test reports
    /// exactly the unregistered one (the drift the data map must never have).
    #[test]
    fn reports_only_the_unregistered_store_in_a_partial_violation() {
        let manifest = StoreManifest::of([
            DeclaredStore::new(StoreKind::Oltp, "svc_oltp"),
            DeclaredStore::new(StoreKind::Blob, "svc_blobs"),
            DeclaredStore::new(StoreKind::Cache, "svc_cache"),
        ]);
        let mut registry = HolderRegistry::new();
        registry.open(StoreKind::Oltp, "svc_oltp");
        registry.open(StoreKind::Cache, "svc_cache");
        // svc_blobs was opened outside the harness → the lone violation.

        let violations = holder_registered(&manifest, &registry);
        assert_eq!(violations.len(), 1, "exactly the one unregistered store is a violation");
        assert_eq!(violations[0].store, DeclaredStore::new(StoreKind::Blob, "svc_blobs"));
    }

    /// An over-registration (the registry has MORE than the manifest declares) is NOT a
    /// violation: the test asserts "no declared store escaped registration", not "the registry
    /// holds exactly the manifest". (A store the harness opened that the manifest forgot to
    /// declare is a data-map-completeness concern for the generator, P-GA-09 — not a
    /// holder-registered failure: that store DID register, so the DSR fan-out reaches it.)
    #[test]
    fn extra_registrations_beyond_the_manifest_are_not_a_violation() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "svc_oltp")]);
        let mut registry = HolderRegistry::new();
        registry.open(StoreKind::Oltp, "svc_oltp");
        registry.open(StoreKind::Cache, "extra_cache"); // registered but not declared.

        assert_eq!(assert_all_holders_registered(&manifest, &registry), Ok(()));
    }

    /// An empty manifest trivially passes (a service that owns no stores has no holder to
    /// register).
    #[test]
    fn empty_manifest_passes() {
        assert_eq!(
            assert_all_holders_registered(&StoreManifest::new(), &HolderRegistry::new()),
            Ok(())
        );
    }

    /// The holder id is PII-free + the SAME `<kind>:<name>` address the registry keys on (so the
    /// declared⇄registered join is exact).
    #[test]
    fn declared_holder_id_matches_the_registry_address() {
        let d = DeclaredStore::new(StoreKind::SearchIndex, "edge_index");
        assert_eq!(d.holder_id(), "search_index:edge_index");
        let mut registry = HolderRegistry::new();
        let reg = registry.open(StoreKind::SearchIndex, "edge_index");
        assert_eq!(reg.holder_id(), d.holder_id(), "declared id == registered id");
    }
}
