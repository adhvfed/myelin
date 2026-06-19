//! # `myelin-query` — the single query AST + field/view primitive (permission-aware)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.5 (`myelin-query` — the substrate-relevant seam) and §7.5 (bounded predicate
//! evaluation).
//!
//! **Contract-index cluster:** 13 — the shared crates' refined shapes
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` row 13.3
//! `myelin-query` primitive, frozen byte-identical X-3/OQ-C) and row 3.4 (`EventMatcher`
//! = the frozen `QueryAst`).
//!
//! ## What crosses the crate boundary here (the substrate-relevant surface)
//! The `EventMatcher` predicate core (Bus triggers) and saved-view filters **share the
//! same `QueryAst`** (frozen byte-identical, X-3) — one safe-evaluation engine, one
//! DoS-hardening surface. The substrate's stake: **bounded predicate evaluation** (§7.5)
//! — declarative, safe-to-evaluate, no Turing-complete predicates, no UDFs/loops/
//! recursion, statically cost-bounded; a per-predicate step/time ceiling so a crafted
//! matcher cannot DoS the trigger engine. The `flow-determinism`-adjacent boundedness is
//! structural here.
//!
//! ## Floor named (this is a SUBSTRATE-RELEVANT SEAM, not the full primitive)
//! The full frozen `QueryAst` grammar + the `ViewSpec` view-model + the field-type enum +
//! the `order_key`/LexoRank encoding (13.3, frozen X-3) are **Issues + Knowledge's**
//! co-owned deliverable (their compilers/executors differ; the definitions are
//! identical) — NOT this prompt. P-001 ships only the placeholder `QueryAst` type name on
//! the bounded-evaluation seam so the matcher/view consumers have a stable surface to
//! reference; the grammar + executor land with the Issues/Knowledge roadmaps.

use serde::{Deserialize, Serialize};

/// The single declarative, bounded-to-evaluate query/predicate AST (architecture §2.5,
/// §7.5; contract 13.3 / 3.4, frozen X-3). It is the `EventMatcher` core AND the
/// saved-view filter — one grammar, one bounded interpreter, four compile targets.
///
/// **Floor:** the grammar is a placeholder string here; the byte-identical frozen
/// `QueryAst` (field-type enum, `ViewSpec`, `order_key`/LexoRank) is co-owned by Issues +
/// Knowledge (13.3). P-001 reserves the type on the bounded-evaluation seam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryAst(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the `QueryAst` type exists on the bounded-evaluation seam
    /// (architecture §7.5; contract 3.4/13.3). The frozen grammar is Issues/Knowledge's.
    #[test]
    fn query_ast_seam_exists() {
        let ast = QueryAst("status == 'open'".into());
        assert_eq!(ast, QueryAst("status == 'open'".into()));
    }
}
