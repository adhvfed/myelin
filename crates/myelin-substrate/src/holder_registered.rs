use crate::holders::{HolderRegistry, StoreKind};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclaredStore {
    pub kind: StoreKind,
    pub name: &'static str,
}

impl DeclaredStore {
    pub fn new(kind: StoreKind, name: &'static str) -> DeclaredStore {
        DeclaredStore { kind, name }
    }

    pub fn holder_id(&self) -> String {
        format!("{}:{}", self.kind.label(), self.name)
    }
}

#[derive(Clone, Debug, Default)]
pub struct StoreManifest {
    declared: Vec<DeclaredStore>,
}

impl StoreManifest {
    pub fn new() -> StoreManifest {
        StoreManifest {
            declared: Vec::new(),
        }
    }

    pub fn of(stores: impl IntoIterator<Item = DeclaredStore>) -> StoreManifest {
        StoreManifest {
            declared: stores.into_iter().collect(),
        }
    }

    pub fn declare(&mut self, kind: StoreKind, name: &'static str) -> &mut StoreManifest {
        self.declared.push(DeclaredStore::new(kind, name));
        self
    }

    pub fn stores(&self) -> &[DeclaredStore] {
        &self.declared
    }

    pub fn holder_ids(&self) -> BTreeSet<String> {
        self.declared.iter().map(DeclaredStore::holder_id).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HolderViolation {
    pub store: DeclaredStore,
}

impl HolderViolation {
    pub fn message(&self) -> String {
        format!(
            "holder-registered architecture test FAILED: store `{}` is declared in the data map \
             but was NOT auto-registered as a PersonalDataHolder - it was opened OUTSIDE the \
             harness (bypassing HolderRegistry::open). Open it through serve(AppSpec)/the harness \
             so opening IS registering (gdpr §3.1, contract 1.4).",
            self.store.holder_id()
        )
    }
}

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

    #[test]
    fn conforming_store_opened_through_the_harness_passes() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "issue_oltp")]);
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

    #[test]
    fn violating_store_opened_outside_the_harness_fails() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "rogue_oltp")]);
        let registry = HolderRegistry::new();

        let violations = holder_registered(&manifest, &registry);
        assert_eq!(
            violations,
            vec![HolderViolation {
                store: DeclaredStore::new(StoreKind::Oltp, "rogue_oltp")
            }],
            "the store opened outside the harness is the violation"
        );
        let err = assert_all_holders_registered(&manifest, &registry)
            .expect_err("a store opened outside the harness must FAIL the architecture test");
        assert_eq!(err.len(), 1);
        let msg = err[0].message();
        assert!(
            msg.contains("rogue_oltp"),
            "the failure names the offending store: {msg}"
        );
        assert!(
            msg.contains("OUTSIDE the harness"),
            "the failure names WHY: {msg}"
        );
        assert!(
            msg.contains("HolderRegistry::open"),
            "the failure names the one door: {msg}"
        );
    }

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

        let violations = holder_registered(&manifest, &registry);
        assert_eq!(
            violations.len(),
            1,
            "exactly the one unregistered store is a violation"
        );
        assert_eq!(
            violations[0].store,
            DeclaredStore::new(StoreKind::Blob, "svc_blobs")
        );
    }

    #[test]
    fn extra_registrations_beyond_the_manifest_are_not_a_violation() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "svc_oltp")]);
        let mut registry = HolderRegistry::new();
        registry.open(StoreKind::Oltp, "svc_oltp");
        registry.open(StoreKind::Cache, "extra_cache");

        assert_eq!(assert_all_holders_registered(&manifest, &registry), Ok(()));
    }

    #[test]
    fn empty_manifest_passes() {
        assert_eq!(
            assert_all_holders_registered(&StoreManifest::new(), &HolderRegistry::new()),
            Ok(())
        );
    }

    #[test]
    fn declared_holder_id_matches_the_registry_address() {
        let d = DeclaredStore::new(StoreKind::SearchIndex, "edge_index");
        assert_eq!(d.holder_id(), "search_index:edge_index");
        let mut registry = HolderRegistry::new();
        let reg = registry.open(StoreKind::SearchIndex, "edge_index");
        assert_eq!(
            reg.holder_id(),
            d.holder_id(),
            "declared id == registered id"
        );
    }
}
