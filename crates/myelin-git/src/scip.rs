//! # `scip` — SCIP/LSIF "find usages" code intelligence (GF-3 → R-3, GIT-P33 / P-482, M5)
//!
//! **AST-aware "find usages" / "go to definition" fed by CI-produced SCIP indices.** The M3 floor
//! ([`crate::code_projection`] / GF-3) emits a LEXICAL projection per changed blob (symbols split on
//! camel/snake, literals, trigram text) — good for `grep`-class code search, but it cannot tell a
//! DEFINITION of `foo` from a REFERENCE to it, nor resolve `foo` here to `foo` defined in another file.
//! This module promotes it: it consumes a **CI-produced SCIP index** (the Sourcegraph SCIP / Microsoft
//! LSIF schema — precise symbol occurrences with roles: definition vs reference) and answers
//! **find-usages** (all references to a symbol) + **go-to-definition** (the defining occurrence).
//!
//! Git OWNS what to index (the SCIP index is a CI artifact about Git's blobs); Search owns the index
//! storage (contract 6.5 — the SCIP/LSIF follow-on projection). This module is the Git-side
//! consumer/projection of the CI SCIP artifact, the same "Git owns what to index, Search owns the
//! index" boundary the lexical floor honours (no cross-DB; Git emits, Search indexes).
//!
//! **Owning architecture (read first, in full):**
//! `05-hard-problems.md` **HP-9** (code-search v1 scope — "v1 = lexical symbol/path/literal/trigram;
//! follow-on: AST-aware 'find usages' + code embeddings fed by CI-produced SCIP indices —
//! demand-triggered"; SCIP/LSIF (Sourcegraph/Microsoft) as the named follow-on). `02-internals-and-
//! algorithms.md` §9 (the code projection). Contract 6.5 (SCIP/LSIF — OWNED follow-on projection).
//!
//! ## What is REUSED vs NEW (EI-01 §7 coherence)
//! The lexical projection already exists and is NOT re-defined:
//! - [`crate::code_projection`] — the per-blob lexical projection (symbols/literals/trigram text), the
//!   GF-3 floor. The SCIP layer sits ON TOP — it does not replace the trigram search (a SCIP-less repo
//!   still gets lexical search).
//! - [`crate::search_projection`] — Git's owned `declare_indexable` spec surface.
//!
//! What is **genuinely NEW** here (the GF-3 → R-3 promotion):
//! 1. [`ScipSymbol`] — a precise, stable symbol id (the SCIP "moniker" — `<scheme> <package> <descriptor>`,
//!    cross-file/cross-repo stable, unlike a lexical token).
//! 2. [`SymbolRole`] + [`Occurrence`] — a symbol occurrence at a path/range with its ROLE (definition vs
//!    reference) — the AST-precision the lexical floor lacks.
//! 3. [`ScipIndex`] — the CI-produced index of occurrences, with [`ScipIndex::find_usages`] (all
//!    references) + [`ScipIndex::definition`] (the defining occurrence).
//!
//! ## Demand-triggered (the floor's trigger honoured)
//! SCIP indexing is demand-triggered (HP-9): a repo gets find-usages ONLY when CI produces a SCIP index
//! for it. [`ScipIndex::is_available`] is the trigger fact a consumer checks before offering the
//! find-usages affordance — a repo without a CI SCIP index falls back to the lexical floor (never a
//! broken "find usages" button on an un-indexed repo).
//!
//! ## FLOOR PROMOTED (the honesty register — VISION §3 / EI-01 §1)
//! - **GF-3 — lexical trigram code search (M3 floor) → AST-aware find-usages fed by CI-produced SCIP
//!   (R-3).** The SCIP occurrence model + find-usages/go-to-definition ship HERE. Recorded, dated
//!   GIT-P33. The SCIP index BYTES are produced by CI (the language indexers run in the CI sandbox);
//!   this owns the Git-side consumption + the find-usages projection over the contract-6.5 shape. Code
//!   EMBEDDINGS (the other half of the HP-9 follow-on) remain a named floor — demand-triggered, owner
//!   Search (vector index). Named here, not built here.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; a wrong usage set is the failure)
//! The find-usages projection is mandatory-core (a wrong usage set silently misleads a refactor). The
//! load-bearing mutants — the role discriminant ([`Occurrence::is_definition`]), the find-usages filter
//! ([`ScipIndex::find_usages`] returns ONLY references, never the definition), the definition lookup
//! ([`ScipIndex::definition`] returns THE definition), and the availability trigger
//! ([`ScipIndex::is_available`]) — are each killed by an assertion in the unit tests. The floor is
//! **≥ 80%**.

use std::collections::BTreeMap;

/// **A SCIP symbol moniker — a precise, stable, cross-file/cross-repo symbol id** (the SCIP "symbol"
/// string: `<scheme> <package> <descriptor>`, e.g. `rust-analyzer cargo myelin-git 0.0.0 scip/ScipSymbol#`).
/// Unlike a lexical token (`ScipSymbol`), a moniker is the SAME id wherever the symbol is referenced —
/// so a reference in file A resolves to the definition in file B. PII-free (a code symbol, not a person).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScipSymbol(pub String);

impl ScipSymbol {
    /// Wrap a SCIP moniker string (the CI indexer produces it; this carries the handle).
    pub fn new(moniker: impl Into<String>) -> ScipSymbol {
        ScipSymbol(moniker.into())
    }
}

/// **The role of a symbol occurrence (the SCIP `symbol_roles` — the AST-precision the lexical floor
/// lacks).** A `Definition` is where the symbol is DECLARED; a `Reference` is a USE of it. Find-usages
/// returns the references; go-to-definition returns the definition. PII-free closed enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolRole {
    /// The DEFINITION of the symbol (its declaration site) — the go-to-definition target.
    Definition,
    /// A REFERENCE to the symbol (a use site) — a find-usages result.
    Reference,
}

/// **A symbol occurrence at a path + line range, with its role (definition vs reference).** The
/// precise unit a SCIP index carries — the AST-aware fact the lexical projection cannot produce. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// The SCIP symbol moniker this occurrence is of.
    pub symbol: ScipSymbol,
    /// The blob path the occurrence is in (`src/lib.rs`).
    pub path: String,
    /// The 1-based start line of the occurrence.
    pub line: u32,
    /// Whether this occurrence is the symbol's DEFINITION or a REFERENCE to it.
    pub role: SymbolRole,
}

impl Occurrence {
    /// Build an occurrence of `symbol` at `path:line` with `role`.
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

    /// **Is this occurrence the symbol's DEFINITION?** The role discriminant find-usages /
    /// go-to-definition fold over. Mandatory-core: a flipped discriminant would return a definition as
    /// a usage (and vice versa).
    pub fn is_definition(&self) -> bool {
        matches!(self.role, SymbolRole::Definition)
    }
}

/// **A CI-produced SCIP index of a repo's symbol occurrences (contract 6.5 follow-on).** Git OWNS what
/// to index (its blobs); CI produces the SCIP occurrences (the language indexers run in the CI
/// sandbox); Search owns the index storage. This is the Git-side projection of that artifact, answering
/// find-usages + go-to-definition. Demand-triggered: a repo without a CI SCIP index is `!is_available`
/// and falls back to the lexical floor.
#[derive(Clone, Debug, Default)]
pub struct ScipIndex {
    /// Whether CI produced a SCIP index for this repo (the demand trigger — HP-9). A repo without one
    /// has no occurrences and find-usages is unavailable (the lexical floor serves it).
    available: bool,
    /// symbol → its occurrences (definitions + references). The `BTreeMap` is deterministic-ordered
    /// (replay-stable — the find-usages result is the same every time).
    occurrences: BTreeMap<ScipSymbol, Vec<Occurrence>>,
}

impl ScipIndex {
    /// An EMPTY, UNAVAILABLE index (a repo CI has not produced a SCIP index for — the lexical floor
    /// serves it). Find-usages on this returns nothing + `is_available` is false.
    pub fn unavailable() -> ScipIndex {
        ScipIndex {
            available: false,
            occurrences: BTreeMap::new(),
        }
    }

    /// Build an AVAILABLE index from a CI-produced occurrence set (the SCIP artifact CI emitted). The
    /// occurrences are bucketed by symbol moniker.
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

    /// **Is a SCIP index AVAILABLE for this repo? (the demand trigger — HP-9.)** A consumer checks this
    /// before offering the find-usages affordance — an unavailable repo falls back to the lexical floor
    /// (never a broken "find usages" button on an un-indexed repo). Mandatory-core.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// **Find-usages: ALL references to `symbol` (NOT the definition).** The AST-aware usage set the
    /// lexical floor cannot produce — it returns only `Reference`-role occurrences (a reference at a use
    /// site), never the definition. Deterministic-ordered (replay-stable). Empty for an unknown symbol
    /// or an unavailable index.
    pub fn find_usages(&self, symbol: &ScipSymbol) -> Vec<&Occurrence> {
        self.occurrences
            .get(symbol)
            .into_iter()
            .flatten()
            .filter(|o| !o.is_definition())
            .collect()
    }

    /// **Go-to-definition: THE defining occurrence of `symbol`** (the `Definition`-role occurrence).
    /// `None` for an unknown symbol, an unavailable index, or a symbol with no definition in this repo
    /// (a reference to an external symbol). Returns the FIRST definition (a well-formed SCIP index has
    /// exactly one per symbol).
    pub fn definition(&self, symbol: &ScipSymbol) -> Option<&Occurrence> {
        self.occurrences
            .get(symbol)
            .into_iter()
            .flatten()
            .find(|o| o.is_definition())
    }

    /// The total occurrence count (definitions + references) — a coarse index-size signal.
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

    /// **find-usages returns ALL references to a symbol, NOT the definition.** The AST-precision the
    /// lexical floor lacks — a reference in another file resolves to the same symbol moniker.
    #[test]
    fn find_usages_returns_references_not_the_definition() {
        let idx = index();
        let usages = idx.find_usages(&sym());
        assert_eq!(
            usages.len(),
            2,
            "two references (the definition is excluded)"
        );
        // Every result is a Reference (never the Definition).
        assert!(usages.iter().all(|o| !o.is_definition()));
        // The reference paths are the use sites (not the definition site).
        let paths: Vec<&str> = usages.iter().map(|o| o.path.as_str()).collect();
        assert!(paths.contains(&"src/lib.rs"));
        assert!(paths.contains(&"src/code_projection.rs"));
        assert!(
            !paths.contains(&"src/scip.rs"),
            "the definition site is NOT a usage"
        );
    }

    /// **go-to-definition returns THE definition occurrence.** The defining site, distinguished from
    /// the references by role.
    #[test]
    fn go_to_definition_returns_the_definition() {
        let idx = index();
        let def = idx.definition(&sym()).expect("the symbol has a definition");
        assert!(def.is_definition());
        assert_eq!(def.path, "src/scip.rs");
        assert_eq!(def.line, 50);
    }

    /// **The role discriminant is load-bearing: a Definition is not a usage, a Reference is not a
    /// definition.** Kills the flipped-discriminant mutant.
    #[test]
    fn role_discriminant_is_exact() {
        let def = Occurrence::new(sym(), "a.rs", 1, SymbolRole::Definition);
        let refr = Occurrence::new(sym(), "b.rs", 2, SymbolRole::Reference);
        assert!(def.is_definition());
        assert!(!refr.is_definition());
    }

    /// **The index is demand-triggered: an UNAVAILABLE index has no usages + reports unavailable.** A
    /// repo without a CI SCIP index falls back to the lexical floor (no broken find-usages button).
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

        // An index built from a CI artifact IS available.
        assert!(index().is_available());
    }

    /// **An unknown symbol yields no usages / no definition (never a wrong set).** Find-usages on a
    /// symbol not in the index is empty, not a guess.
    #[test]
    fn unknown_symbol_yields_nothing() {
        let idx = index();
        let other = ScipSymbol::new("rust cargo other unknown#");
        assert!(idx.find_usages(&other).is_empty());
        assert!(idx.definition(&other).is_none());
    }

    /// **A symbol with only references (an EXTERNAL symbol defined elsewhere) has usages but no local
    /// definition.** Find-usages works; go-to-definition is `None` (the definition is in another repo).
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

    /// **The occurrence count sums definitions + references.** The coarse index-size signal.
    #[test]
    fn occurrence_count_sums_all_occurrences() {
        assert_eq!(index().occurrence_count(), 3, "1 def + 2 refs");
    }
}
