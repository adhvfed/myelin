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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_stable_and_unambiguous() {
        assert_eq!(StoreKind::Oltp.label(), "oltp");
        assert_eq!(StoreKind::Blob.label(), "blob");
        assert_eq!(StoreKind::Cache.label(), "cache");
        assert_eq!(StoreKind::SearchIndex.label(), "search_index");
    }
}
