use crate::holders::{HolderRegistration, StoreKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Holder {
    H1Git,
    H2Ci,
    H3Issues,
    H4Knowledge,
    H5Chat,
    H6BlobStore,
    H7SearchIndex,
    H8EventBus,
    H9Caches,
    H10Backups,
    H11AgentMemory,
    H12ReferenceGraph,
    H13NotificationHistory,
    H14AuthzTuples,
    H15Identity,
    H16AuditLog,
    H17AgentTrace,
    H18GdprOwn,
}

impl Holder {
    pub const ALL: [Holder; 18] = [
        Holder::H1Git,
        Holder::H2Ci,
        Holder::H3Issues,
        Holder::H4Knowledge,
        Holder::H5Chat,
        Holder::H6BlobStore,
        Holder::H7SearchIndex,
        Holder::H8EventBus,
        Holder::H9Caches,
        Holder::H10Backups,
        Holder::H11AgentMemory,
        Holder::H12ReferenceGraph,
        Holder::H13NotificationHistory,
        Holder::H14AuthzTuples,
        Holder::H15Identity,
        Holder::H16AuditLog,
        Holder::H17AgentTrace,
        Holder::H18GdprOwn,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            Holder::H1Git => "H1",
            Holder::H2Ci => "H2",
            Holder::H3Issues => "H3",
            Holder::H4Knowledge => "H4",
            Holder::H5Chat => "H5",
            Holder::H6BlobStore => "H6",
            Holder::H7SearchIndex => "H7",
            Holder::H8EventBus => "H8",
            Holder::H9Caches => "H9",
            Holder::H10Backups => "H10",
            Holder::H11AgentMemory => "H11",
            Holder::H12ReferenceGraph => "H12",
            Holder::H13NotificationHistory => "H13",
            Holder::H14AuthzTuples => "H14",
            Holder::H15Identity => "H15",
            Holder::H16AuditLog => "H16",
            Holder::H17AgentTrace => "H17",
            Holder::H18GdprOwn => "H18",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreHolder {
    pub kind: StoreKind,
    pub name: &'static str,
    pub holder: Holder,
}

impl StoreHolder {
    pub fn new(kind: StoreKind, name: &'static str, holder: Holder) -> StoreHolder {
        StoreHolder { kind, name, holder }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StoreClassifier {
    declarations: Vec<StoreHolder>,
}

impl StoreClassifier {
    pub fn new() -> StoreClassifier {
        StoreClassifier {
            declarations: Vec::new(),
        }
    }

    pub fn of(decls: impl IntoIterator<Item = StoreHolder>) -> StoreClassifier {
        StoreClassifier {
            declarations: decls.into_iter().collect(),
        }
    }

    pub fn declare(
        &mut self,
        kind: StoreKind,
        name: &'static str,
        holder: Holder,
    ) -> &mut StoreClassifier {
        self.declarations.push(StoreHolder::new(kind, name, holder));
        self
    }

    pub fn declarations(&self) -> &[StoreHolder] {
        &self.declarations
    }
}

pub fn classify_store(kind: StoreKind, name: &str, classifier: &StoreClassifier) -> Option<Holder> {
    match kind {
        StoreKind::Blob => Some(Holder::H6BlobStore),
        StoreKind::Cache => Some(Holder::H9Caches),
        StoreKind::SearchIndex => Some(Holder::H7SearchIndex),
        StoreKind::Oltp => classifier
            .declarations()
            .iter()
            .find(|d| d.kind == StoreKind::Oltp && d.name == name)
            .map(|d| d.holder),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrphanStore {
    pub kind: StoreKind,
    pub name: String,
}

impl OrphanStore {
    pub fn message(&self) -> String {
        format!(
            "holder-completeness assertion FAILED: store `{}:{}` maps to NONE of the exhaustive \
             H1–H18 holders (gdpr §3.2) - it is an ORPHAN store outside the data map. A store the \
             RoPA inventory never accounted for escapes the DSR fan-out + the §3.2 completeness \
             guarantee (EI-01 §2 - a forgotten store is a GDPR + data-loss hole). Declare its \
             H-holder in the service's StoreClassifier (an OLTP store), or add it to the §3.2 list \
             + the Holder enum (a genuinely new holder kind, a deliberate GDPR co-edit).",
            self.kind.label(),
            self.name,
        )
    }
}

pub fn holder_completeness(
    opened: &[HolderRegistration],
    classifier: &StoreClassifier,
) -> Vec<OrphanStore> {
    opened
        .iter()
        .filter(|reg| classify_store(reg.kind, reg.name, classifier).is_none())
        .map(|reg| OrphanStore {
            kind: reg.kind,
            name: reg.name.to_string(),
        })
        .collect()
}

pub fn assert_holder_completeness(
    opened: &[HolderRegistration],
    classifier: &StoreClassifier,
) -> Result<(), Vec<OrphanStore>> {
    let orphans = holder_completeness(opened, classifier);
    if orphans.is_empty() {
        Ok(())
    } else {
        Err(orphans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::HolderRegistry;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_is_exhaustive_eighteen() {
        assert_eq!(
            Holder::ALL.len(),
            18,
            "the §3.2 holder list is exhaustive: H1–H18"
        );
        let tags: BTreeSet<&str> = Holder::ALL.iter().map(|h| h.tag()).collect();
        assert_eq!(tags.len(), 18, "the eighteen H-tags are distinct");
        for n in 1..=18 {
            assert!(
                tags.contains(format!("H{n}").as_str()),
                "the catalog names H{n}"
            );
        }
    }

    #[test]
    fn every_opened_store_maps_to_an_h_holder_no_orphan() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "issue_oltp");
        reg.open(StoreKind::Blob, "issue_blobs");
        reg.open(StoreKind::Cache, "issue_cache");
        reg.open(StoreKind::SearchIndex, "issue_index");
        let classifier = StoreClassifier::of([StoreHolder::new(
            StoreKind::Oltp,
            "issue_oltp",
            Holder::H3Issues,
        )]);

        assert!(
            holder_completeness(reg.registrations(), &classifier).is_empty(),
            "every opened store is in the H1–H18 set; no orphan"
        );
        assert_eq!(
            assert_holder_completeness(reg.registrations(), &classifier),
            Ok(()),
            "the holder-completeness assertion passes - no store outside the exhaustive list"
        );
        assert_eq!(
            classify_store(StoreKind::Oltp, "issue_oltp", &classifier),
            Some(Holder::H3Issues)
        );
        assert_eq!(
            classify_store(StoreKind::Blob, "issue_blobs", &classifier),
            Some(Holder::H6BlobStore)
        );
        assert_eq!(
            classify_store(StoreKind::Cache, "issue_cache", &classifier),
            Some(Holder::H9Caches)
        );
        assert_eq!(
            classify_store(StoreKind::SearchIndex, "issue_index", &classifier),
            Some(Holder::H7SearchIndex)
        );
    }

    #[test]
    fn a_deliberately_orphaned_store_fails_the_completeness_assertion() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "rogue_oltp");
        let classifier = StoreClassifier::new();

        let orphans = holder_completeness(reg.registrations(), &classifier);
        assert_eq!(
            orphans,
            vec![OrphanStore {
                kind: StoreKind::Oltp,
                name: "rogue_oltp".into()
            }],
            "the OLTP store with no declared holder is the orphan"
        );
        let err = assert_holder_completeness(reg.registrations(), &classifier)
            .expect_err("a store outside the H1–H18 list MUST fail the completeness assertion");
        assert_eq!(err.len(), 1);
        let msg = err[0].message();
        assert!(msg.contains("rogue_oltp"), "names the orphan store: {msg}");
        assert!(msg.contains("H1–H18"), "names the exhaustive list: {msg}");
        assert!(msg.contains("ORPHAN"), "names WHY it failed: {msg}");
    }

    #[test]
    fn reports_only_the_orphan_in_a_partial_violation() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "issue_oltp");
        reg.open(StoreKind::Oltp, "shadow_oltp");
        reg.open(StoreKind::Blob, "issue_blobs");
        let classifier = StoreClassifier::of([StoreHolder::new(
            StoreKind::Oltp,
            "issue_oltp",
            Holder::H3Issues,
        )]);

        let orphans = holder_completeness(reg.registrations(), &classifier);
        assert_eq!(
            orphans.len(),
            1,
            "exactly the one undeclared OLTP store is the orphan"
        );
        assert_eq!(
            orphans[0],
            OrphanStore {
                kind: StoreKind::Oltp,
                name: "shadow_oltp".into()
            }
        );
    }

    #[test]
    fn the_real_m1_holder_stores_classify_to_their_h_numbers() {
        let classifier = StoreClassifier::of([
            StoreHolder::new(StoreKind::Oltp, "git_oltp", Holder::H1Git),
            StoreHolder::new(StoreKind::Oltp, "ci_oltp", Holder::H2Ci),
            StoreHolder::new(StoreKind::Oltp, "issue_oltp", Holder::H3Issues),
            StoreHolder::new(StoreKind::Oltp, "knowledge_oltp", Holder::H4Knowledge),
            StoreHolder::new(StoreKind::Oltp, "chat_oltp", Holder::H5Chat),
            StoreHolder::new(StoreKind::Oltp, "authz_tuples", Holder::H14AuthzTuples),
            StoreHolder::new(StoreKind::Oltp, "identity_oltp", Holder::H15Identity),
            StoreHolder::new(StoreKind::Oltp, "audit_oltp", Holder::H16AuditLog),
            StoreHolder::new(StoreKind::Oltp, "gdpr_oltp", Holder::H18GdprOwn),
        ]);
        for d in classifier.declarations() {
            assert_eq!(
                classify_store(d.kind, d.name, &classifier),
                Some(d.holder),
                "the M1 store `{}` classifies to {}",
                d.name,
                d.holder.tag()
            );
        }
        let mut reg = HolderRegistry::new();
        for d in classifier.declarations() {
            reg.open(d.kind, d.name);
        }
        assert_eq!(
            assert_holder_completeness(reg.registrations(), &classifier),
            Ok(())
        );
    }

    #[test]
    fn empty_harness_passes() {
        assert_eq!(
            assert_holder_completeness(&[], &StoreClassifier::new()),
            Ok(())
        );
    }
}
