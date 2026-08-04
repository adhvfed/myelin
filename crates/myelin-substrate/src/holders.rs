use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreKind {
    Oltp,
    Blob,
    Cache,
    SearchIndex,
}

impl StoreKind {
    pub fn label(self) -> &'static str {
        match self {
            StoreKind::Oltp => "oltp",
            StoreKind::Blob => "blob",
            StoreKind::Cache => "cache",
            StoreKind::SearchIndex => "search_index",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HolderRegistration {
    pub kind: StoreKind,
    pub name: &'static str,
}

impl HolderRegistration {
    pub fn holder_id(&self) -> String {
        format!("{}:{}", self.kind.label(), self.name)
    }
}

#[derive(Clone, Debug, Default)]
pub struct HolderRegistry {
    registrations: Vec<HolderRegistration>,
}

impl HolderRegistry {
    pub fn new() -> HolderRegistry {
        HolderRegistry {
            registrations: Vec::new(),
        }
    }

    pub fn open(&mut self, kind: StoreKind, name: &'static str) -> HolderRegistration {
        let reg = HolderRegistration { kind, name };
        if !self.registrations.contains(&reg) {
            self.registrations.push(reg.clone());
        }
        reg
    }

    pub fn registrations(&self) -> &[HolderRegistration] {
        &self.registrations
    }

    pub fn holder_ids(&self) -> BTreeSet<String> {
        self.registrations
            .iter()
            .map(HolderRegistration::holder_id)
            .collect()
    }

    pub fn is_registered(&self, kind: StoreKind, name: &'static str) -> bool {
        self.registrations
            .contains(&HolderRegistration { kind, name })
    }

    pub fn len(&self) -> usize {
        self.registrations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_store_registers_it_as_a_holder() {
        let mut reg = HolderRegistry::new();
        let receipt = reg.open(StoreKind::Oltp, "issue_oltp");
        assert_eq!(
            receipt,
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "issue_oltp"
            }
        );
        assert!(reg.is_registered(StoreKind::Oltp, "issue_oltp"));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn every_store_kind_auto_registers() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "svc_oltp");
        reg.open(StoreKind::Blob, "svc_blobs");
        reg.open(StoreKind::Cache, "svc_cache");
        reg.open(StoreKind::SearchIndex, "svc_index");
        assert_eq!(reg.len(), 4, "all four §3.4 store kinds registered");
        assert!(reg.is_registered(StoreKind::Blob, "svc_blobs"));
        assert!(reg.is_registered(StoreKind::Cache, "svc_cache"));
        assert!(reg.is_registered(StoreKind::SearchIndex, "svc_index"));
        let ids = reg.holder_ids();
        assert!(ids.contains("oltp:svc_oltp"));
        assert!(ids.contains("search_index:svc_index"));
    }

    #[test]
    fn re_opening_a_store_does_not_double_register() {
        let mut reg = HolderRegistry::new();
        reg.open(StoreKind::Oltp, "svc_oltp");
        reg.open(StoreKind::Oltp, "svc_oltp");
        assert_eq!(reg.len(), 1, "the same store registers exactly once");
    }

    #[test]
    fn holder_id_is_pii_free_and_stable() {
        let r = HolderRegistration {
            kind: StoreKind::Cache,
            name: "edge_cache",
        };
        assert_eq!(r.holder_id(), "cache:edge_cache");
    }

    #[test]
    fn fresh_registry_is_empty() {
        let reg = HolderRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }
}
