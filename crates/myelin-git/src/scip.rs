use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScipSymbol(pub String);

impl ScipSymbol {
    pub fn new(moniker: impl Into<String>) -> ScipSymbol {
        ScipSymbol(moniker.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolRole {
    Definition,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub symbol: ScipSymbol,
    pub path: String,
    pub line: u32,
    pub role: SymbolRole,
}

impl Occurrence {
    pub fn new(
        symbol: ScipSymbol,
        path: impl Into<String>,
        line: u32,
        role: SymbolRole,
    ) -> Occurrence {
        Occurrence {
            symbol,
            path: path.into(),
            line,
            role,
        }
    }

    pub fn is_definition(&self) -> bool {
        matches!(self.role, SymbolRole::Definition)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScipIndex {
    available: bool,
    occurrences: BTreeMap<ScipSymbol, Vec<Occurrence>>,
}

impl ScipIndex {
    pub fn unavailable() -> ScipIndex {
        ScipIndex {
            available: false,
            occurrences: BTreeMap::new(),
        }
    }

    pub fn from_ci(occurrences: Vec<Occurrence>) -> ScipIndex {
        let mut by_symbol: BTreeMap<ScipSymbol, Vec<Occurrence>> = BTreeMap::new();
        for occ in occurrences {
            by_symbol.entry(occ.symbol.clone()).or_default().push(occ);
        }
        ScipIndex {
            available: true,
            occurrences: by_symbol,
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub fn find_usages(&self, symbol: &ScipSymbol) -> Vec<&Occurrence> {
        self.occurrences
            .get(symbol)
            .into_iter()
            .flatten()
            .filter(|o| !o.is_definition())
            .collect()
    }

    pub fn definition(&self, symbol: &ScipSymbol) -> Option<&Occurrence> {
        self.occurrences
            .get(symbol)
            .into_iter()
            .flatten()
            .find(|o| o.is_definition())
    }

    pub fn occurrence_count(&self) -> usize {
        self.occurrences.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym() -> ScipSymbol {
        ScipSymbol::new("rust cargo myelin-git scip/ScipSymbol#")
    }

    fn index() -> ScipIndex {
        ScipIndex::from_ci(vec![
            Occurrence::new(sym(), "src/scip.rs", 50, SymbolRole::Definition),
            Occurrence::new(sym(), "src/lib.rs", 340, SymbolRole::Reference),
            Occurrence::new(sym(), "src/code_projection.rs", 107, SymbolRole::Reference),
        ])
    }

    #[test]
    fn find_usages_returns_references_not_the_definition() {
        let idx = index();
        let usages = idx.find_usages(&sym());
        assert_eq!(
            usages.len(),
            2,
            "two references (the definition is excluded)"
        );
        assert!(usages.iter().all(|o| !o.is_definition()));
        let paths: Vec<&str> = usages.iter().map(|o| o.path.as_str()).collect();
        assert!(paths.contains(&"src/lib.rs"));
        assert!(paths.contains(&"src/code_projection.rs"));
        assert!(
            !paths.contains(&"src/scip.rs"),
            "the definition site is NOT a usage"
        );
    }

    #[test]
    fn go_to_definition_returns_the_definition() {
        let idx = index();
        let def = idx.definition(&sym()).expect("the symbol has a definition");
        assert!(def.is_definition());
        assert_eq!(def.path, "src/scip.rs");
        assert_eq!(def.line, 50);
    }

    #[test]
    fn role_discriminant_is_exact() {
        let def = Occurrence::new(sym(), "a.rs", 1, SymbolRole::Definition);
        let refr = Occurrence::new(sym(), "b.rs", 2, SymbolRole::Reference);
        assert!(def.is_definition());
        assert!(!refr.is_definition());
    }

    #[test]
    fn unavailable_index_is_the_lexical_floor_fallback() {
        let idx = ScipIndex::unavailable();
        assert!(
            !idx.is_available(),
            "no CI SCIP index → find-usages unavailable"
        );
        assert!(idx.find_usages(&sym()).is_empty());
        assert!(idx.definition(&sym()).is_none());
        assert_eq!(idx.occurrence_count(), 0);

        assert!(index().is_available());
    }

    #[test]
    fn unknown_symbol_yields_nothing() {
        let idx = index();
        let other = ScipSymbol::new("rust cargo other unknown#");
        assert!(idx.find_usages(&other).is_empty());
        assert!(idx.definition(&other).is_none());
    }

    #[test]
    fn external_symbol_has_usages_but_no_local_definition() {
        let ext = ScipSymbol::new("rust cargo std vec/Vec#");
        let idx = ScipIndex::from_ci(vec![
            Occurrence::new(ext.clone(), "src/lib.rs", 10, SymbolRole::Reference),
            Occurrence::new(ext.clone(), "src/scip.rs", 20, SymbolRole::Reference),
        ]);
        assert_eq!(idx.find_usages(&ext).len(), 2, "both references found");
        assert!(
            idx.definition(&ext).is_none(),
            "an external symbol has no local definition (go-to-def is None)"
        );
    }

    #[test]
    fn occurrence_count_sums_all_occurrences() {
        assert_eq!(index().occurrence_count(), 3, "1 def + 2 refs");
    }
}
