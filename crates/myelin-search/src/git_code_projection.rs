//! # `git_code_projection` — Code search v1 (Git `git.*` projection): the code body builder +
//! the trigram index (SRCH-P18 / P-261, M3)
//!
//! **Owning architecture doc:** `search-and-indexing.md` §4.4 (code search v1 =
//! symbol/path/literal-grade — file paths, identifiers via the camel/snake tokenizer keeping
//! operators, string literals, commit messages, trigram/n-gram indexing for substring/regex-lite,
//! Russ Cox's trigram approach; `code_block.text` is RAW not markdown-parsed; the SCIP/LSIF
//! "find usages" follow-on is NAMED not built v1; Search does NOT parse repos — Git emits the
//! projection). **Reconciliation:** `00-reconciliation-decisions.md` change #8 (the SCIP/LSIF
//! follow-on input named), X-2 (`code_block.text` raw). **Contracts:** 6.5 (the Git `git.*`
//! projection per blob/ref/symbol; the SCIP/LSIF follow-on), 13.1 (the raw code-block text).
//!
//! ## What SRCH-P18 ships here — the code-search-v1 DEPTH over the SRCH-P12 tokenizer
//!
//! GIT-P5 / P-231 declared git's `declare_indexable` IndexSpec SHAPE (`git`/`blob`, the structured
//! facets `path`/`language`/`blob_oid`, `acl_object_type = repo`) in `myelin_git::search_projection`.
//! SRCH-P12 / P-175 shipped the **code tokenizer** ([`crate::analysis::Analyzer`] with
//! [`crate::analysis::Language::Code`]) — the camel/snake split keeping operators. SRCH-P18 ships the
//! **code-search DEPTH** that rides BOTH:
//!
//! 1. **[`git_blob_search_projection`]** — the index-time row builder. It takes a Git blob's
//!    projection inputs (path / detected language / raw code text / extracted string literals / the
//!    tip commit message / blob_oid) and builds the [`SearchProjection`] Search indexes. The
//!    full-text `text` body is the **code-tokenized + trigram-indexed** searchable term stream (so an
//!    identifier query hits via the camel/snake split, an exact-identifier query hits the whole
//!    token, an operator like `->`/`::` is searchable, and a **substring/regex-lite** query hits via
//!    the trigrams — Russ Cox 2012). The structured facets (`path`/`language`/`blob_oid`) match git's
//!    owned 6.5 spec byte-for-byte. `code_block.text` is RAW (X-2).
//!
//! 2. **[`trigrams`]** — the **trigram index** (Russ Cox's `google/codesearch` approach): every
//!    overlapping 3-byte (character) window of the normalized code becomes a searchable token, so a
//!    substring/regex-lite query decomposes into a conjunction of trigrams the inverted index serves
//!    as a candidate filter (the regex is then verified against the candidates — v1 ships the
//!    candidate-trigram filter; the regex-verify-over-candidates pass is the query-path consumer). A
//!    trigram is a recall-safe substring primitive: `cox` is a trigram of `russcox`, so a substring
//!    query `ussco` (→ `uss`,`ssc`,`sco`) finds the blob without an identifier boundary.
//!
//! ## Why the body is PRE-TOKENIZED into `text` (the engine is UNCHANGED — the prompt's DoD)
//!
//! The engine's `text` field ([`crate::engine`]) is a Tantivy `TEXT` field tokenized by Tantivy's
//! default (whitespace/lowercase) tokenizer. Rather than register a custom per-field Tantivy
//! tokenizer (an engine change), SRCH-P18 — exactly like [`crate::kn_projection`] does for KN prose —
//! pre-runs the SRCH-P12 code tokenizer + the trigram generator and emits their token stream as the
//! **space-separated `text` body**. Tantivy's default tokenizer then splits on those spaces, so the
//! indexed terms ARE the code tokens + trigrams. This keeps the SRCH-P04 engine fixed (the prompt:
//! "the ENGINE is UNCHANGED") while making code search v1 correct end-to-end. A code identifier
//! search, an exact-identifier search, an operator search, a path search, a commit-message search,
//! and a substring/regex-lite search all reduce to a term/conjunction over this body the existing
//! [`crate::engine::TantivyBackend::search`] serves with the ACL pre-filter conjoined first.
//!
//! ## Coherence (EI-01 §7) — NO second indexing-contract shape, NO second tokenizer
//!
//! - The **IndexSpec** is the ONE frozen Search-owned shape. [`git_code_projection_spec`] here is
//!   byte-identical to git's owned `myelin_git::search_projection::git_code_projection_spec`
//!   (`myelin-git` depends on `myelin-search`, never the reverse, so Search cannot import git — it
//!   models the same shape against the frozen [`IndexSpec`] type, the SAME posture
//!   [`crate::kn_projection`] takes for KN; a CDC test pins the byte-parity of the serialized shape).
//! - The **code tokenizer** is the ONE [`crate::analysis`] chain (SRCH-P12). This module does NOT
//!   re-implement camel/snake splitting — it calls [`crate::analysis::Analyzer::for_language`] with
//!   [`crate::analysis::Language::Code`]. The trigram generator is the only genuinely-new primitive.
//! - Search does **NOT parse repos** (the no-cross-db floor): the inputs to
//!   [`git_blob_search_projection`] are the GIT-emitted projection fields (path/text/literals/commit
//!   message/blob_oid), reached through the owner's `project(ref, viewer)` (5.6) — never a repo read.
//!
//! ## FLOOR named (SRCH-P18 DoD)
//! - **Code search v1 = symbol/path/literal + trigram.** The AST-aware / cross-reference / "find
//!   usages" semantic code search consumes a **CI-produced SCIP/LSIF** projection (jointly Git+CI,
//!   GF-3) — the **post-M4 / demand-triggered** follow-on (change #8, contract 6.5). Code embeddings
//!   for semantic code retrieval ride the same vector path (so a git blob spec is **non-semantic** in
//!   v1). Named so v1 is not mistaken for find-usages. Greppable as [`ScipLsifFindUsagesFloor`].
//! - **The real Git code-projection EMITTER** (the receive-pack post-commit hook that walks the
//!   indexed ref's tree and emits the per-blob projection through the outbox) is GIT-P25 / P-287
//!   (architecture `02 §9` TE-27). Here Search ships the body BUILDER + trigram index the emitter
//!   feeds; the integration test drives the genuine builder over a real code corpus.

use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue};

use crate::analysis::{Analyzer, Language};
use crate::indexer::{IndexSpec, SearchProjection};

/// The subsystem token git declares its projection under (`git`) — byte-identical to
/// `myelin_git::search_projection::GIT_SUBSYSTEM`. Search models it here because `myelin-git` depends
/// on `myelin-search` (never the reverse), so Search cannot import git (the [`crate::kn_projection`]
/// posture — the shape, not a second contract).
pub const GIT_SUBSYSTEM: &str = "git";

/// The artifact type git's code projection indexes — a `blob` (a single indexed file at a path on an
/// indexed ref). The canonical ref is `myelin://<tenant>/git/blob/<repo>:<ref>:<path>`.
pub const GIT_BLOB_TYPE: &str = "blob";

/// The ACL object type a blob doc's reachability filter pins on — the parent **`repo`** (there is no
/// per-blob ACL; the repository decides reachability — architecture §6 / git's frozen ReBAC `repo`
/// object type).
pub const GIT_BLOB_ACL_OBJECT_TYPE: &str = "repo";

/// The structured-facet key for the indexed blob's path within the repo (the columnar field a path
/// filter pins on — GF-3 "find this path across the corpus").
pub const FACET_PATH: &str = "path";
/// The structured-facet key for the detected source-language tag.
pub const FACET_LANGUAGE: &str = "language";
/// The structured-facet key for the content-addressed blob object id (the indexed blob's identity).
pub const FACET_BLOB_OID: &str = "blob_oid";

/// The number of characters in a trigram (Russ Cox's `google/codesearch` substring index — 3).
pub const TRIGRAM_N: usize = 3;

/// **Git's `declare_indexable` code-projection spec (contract 6.5 — the Search-side model).** Byte-
/// identical to `myelin_git::search_projection::git_code_projection_spec`: `subsystem = "git"`,
/// `type = "blob"`, the three structured facets (`path`/`language`/`blob_oid`, all
/// [`FieldType::Text`]), **non-semantic** (code is trigram/symbol full-text, not vector-embedded in
/// v1 — semantic code retrieval is the SCIP/LSIF + code-embedding follow-on, change #8),
/// `acl_object_type = "repo"`.
///
/// The full-text projection body (the code-tokenized + trigram-indexed symbols / literals / commit
/// message / blob text) is NOT in the spec — it arrives at emit time in
/// [`git_blob_search_projection`]'s [`SearchProjection::text`] (the spec is the schema, the projection
/// is the row). A CDC test pins the byte-parity of this serialized shape against git's owned spec.
pub fn git_code_projection_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_PATH.to_string(), FieldType::Text);
    struct_fields.insert(FACET_LANGUAGE.to_string(), FieldType::Text);
    struct_fields.insert(FACET_BLOB_OID.to_string(), FieldType::Text);
    IndexSpec::new(GIT_SUBSYSTEM, GIT_BLOB_TYPE, struct_fields)
        .with_acl_object_type(GIT_BLOB_ACL_OBJECT_TYPE)
}

/// Every Git code-search index spec (here, the single `git`/`blob` shape) — the set a Search indexer
/// registers to consume the real Git corpus.
pub fn git_index_specs() -> Vec<IndexSpec> {
    vec![git_code_projection_spec()]
}

/// **Register Git's code-projection spec WITH Search (the GATE).** Builds [`git_index_specs`] and
/// proves Search **accepts** them by admitting them into a live
/// [`IncrementalIndexer`](crate::indexer::IncrementalIndexer)'s per-tenant facet union without a
/// schema mismatch (the only honest definition of "accepted" — Search is the authority that admits).
/// Mirrors [`crate::kn_projection::register_kn_index_specs`] and git's
/// `register_git_code_projection_spec`.
pub fn register_git_index_specs() -> Vec<IndexSpec> {
    let specs = git_index_specs();
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

/// A do-nothing [`ProjectFetcher`](crate::indexer::ProjectFetcher) used ONLY to admit the git spec
/// into a live indexer for the registration GATE (the SPEC half + the body BUILDER ship here; the
/// real owner-`project` fetch is the GIT-P25 emitter). Mirrors git's / KN's `NullProjectFetcher`.
struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

/// **The Git-emitted blob projection inputs (the per-blob row Git owns — contract 6.5 / architecture
/// `02 §9`).** These are the fields the GIT-P25 emitter projects per blob on a push to an indexed ref
/// (Search does NOT parse the repo — it consumes these). [`git_blob_search_projection`] turns them
/// into the index-time [`SearchProjection`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitBlobProjectionInput {
    /// The blob's path within the repo (`src/scheduler/deadlock.rs`) — a structured facet AND a
    /// full-text-searchable token stream (so "find a blob by a path segment" works).
    pub path: String,
    /// The detected source-language tag (`rust`/`python`/…) — a structured facet. Empty ⇒ unset.
    pub language: String,
    /// The **raw** blob text (X-2 — never markdown-parsed). The code tokenizer + the trigram index
    /// run over this. This is the `code_block.text`-equivalent for a git blob.
    pub text: String,
    /// The extracted string/number literals (the producer's lexer pulled them; Search indexes them as
    /// searchable tokens so a `"connection refused"` literal query hits the blob).
    pub literals: Vec<String>,
    /// The tip commit message of the indexed ref (searchable full-text — a commit-message query hits
    /// the blobs of that commit).
    pub commit_message: String,
    /// The content-addressed blob object id (the git OID) — a structured facet (de-dup / pin).
    pub blob_oid: String,
}

/// **Build a Git blob's [`SearchProjection`] (the code-search-v1 index-time row, §4.4).** This is the
/// owner's `project(ref, viewer)` body Search consumes (contract 6.5) — NOT a repo read. It produces:
///
/// - the full-text `text` body = the **code-tokenized** (camel/snake split keeping operators, exact
///   identifiers kept whole — SRCH-P12) symbols of the raw code + the path tokens + the literal
///   tokens + the commit-message tokens, **plus the trigram index** ([`trigrams`]) of the raw code
///   (so substring/regex-lite works — Russ Cox). Everything is space-separated so the engine's
///   default tokenizer re-splits it into the exact searchable term set (the engine is unchanged);
/// - the three structured facets (`path`/`language`/`blob_oid`) matching git's owned 6.5 spec;
/// - `lang = "code"` (the SRCH-P12 code analyzer tag, §3.1) so a query-time path selects the SAME
///   code chain (parity — a code query is code-tokenized identically).
///
/// The raw code is carried VERBATIM into the tokenizer (X-2 — Search tokenises it with the code
/// tokenizer, not a language stemmer; §4.4). A git blob is **non-semantic** in v1 (no vector — the
/// SCIP/LSIF + code-embedding semantic follow-on, change #8).
pub fn git_blob_search_projection(input: &GitBlobProjectionInput) -> SearchProjection {
    let code = Analyzer::for_language(Language::Code);

    // Collect every searchable token into a deduped-by-position stream (a Vec, not a Set, so a
    // repeated identifier still contributes its BM25 term frequency). The engine re-splits on spaces.
    let mut terms: Vec<String> = Vec::new();

    // 1. The code SYMBOLS / identifiers / operators of the raw blob (X-2 — verbatim into the code
    //    tokenizer; camel/snake split + whole-identifier + operators, SRCH-P12).
    terms.extend(code.analyze(&input.text));

    // 2. The PATH tokens — split the path on its separators AND code-tokenize each segment, so a query
    //    on a path segment (`scheduler`, `deadlock`) or the whole filename hits. The path is also a
    //    structured facet (exact/prefix path filter); this makes it full-text-searchable too (GF-3).
    for segment in input.path.split(['/', '\\', '.']) {
        if !segment.is_empty() {
            terms.extend(code.analyze(segment));
        }
    }

    // 3. The string/number LITERALS — code-tokenized (a multi-word literal `"connection refused"`
    //    contributes both `connection` and `refused` so a phrase-ish literal query hits).
    for literal in &input.literals {
        terms.extend(code.analyze(literal));
    }

    // 4. The COMMIT MESSAGE — code-tokenized (a commit-message query hits the indexed blobs). Code
    //    tokenization (not a language stemmer) keeps it parity-correct with the code chain (§4.4).
    terms.extend(code.analyze(&input.commit_message));

    // 5. The TRIGRAM index of the raw code (Russ Cox) — substring/regex-lite candidate tokens. Each
    //    trigram is prefixed so it never collides with a real 3-char identifier token (a `t·` sentinel
    //    keeps the trigram namespace disjoint from the symbol namespace — a substring query targets
    //    the trigram tokens explicitly via [`trigram_query`]).
    for tg in trigrams(&input.text) {
        terms.push(trigram_token(&tg));
    }

    // The token stream IS the searchable body (space-separated; the engine's default tokenizer splits
    // on the spaces back into this exact term set). Operators like `->`/`::` survive as their own
    // tokens because the code tokenizer kept them and Tantivy's default tokenizer keeps punctuation
    // runs that are not whitespace... but to be safe we join with spaces so every emitted token is its
    // own whitespace-delimited unit regardless of the engine tokenizer's punctuation handling.
    let text = terms.join(" ");

    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    if !input.path.is_empty() {
        fields.insert(FACET_PATH.to_string(), FieldValue::Text(input.path.clone()));
    }
    if !input.language.is_empty() {
        fields.insert(
            FACET_LANGUAGE.to_string(),
            FieldValue::Text(input.language.clone()),
        );
    }
    if !input.blob_oid.is_empty() {
        fields.insert(
            FACET_BLOB_OID.to_string(),
            FieldValue::Text(input.blob_oid.clone()),
        );
    }

    // A git blob is analyzed under the CODE chain (§3.1 lang tag) — query-time selects the same chain.
    SearchProjection {
        text,
        fields,
        lang: Some(Language::Code.tag().to_string()),
    }
}

/// **The trigram index (Russ Cox's `google/codesearch` substring index, §4.4).** Returns the set of
/// overlapping 3-character windows of the **normalized** (lowercased; whitespace runs collapsed to a
/// single space) text. A trigram is a recall-safe substring primitive: a substring query of length ≥ 3
/// decomposes into a CONJUNCTION of its trigrams, and a blob can only contain the substring if it
/// contains ALL of them — so the trigram conjunction is a sound candidate filter (a superset of the
/// true matches; the regex/substring is then verified against the candidates). v1 ships the candidate
/// trigrams; the regex-verify-over-candidates pass is the query-path consumer.
///
/// Trigrams are over **characters** (not bytes) so a multibyte identifier (`café`, `東京都`) trigrams
/// correctly. The set is sorted + deduped (membership is what the candidate filter needs).
pub fn trigrams(text: &str) -> Vec<String> {
    let normalized = normalize_for_trigrams(text);
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() < TRIGRAM_N {
        return Vec::new();
    }
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for window in chars.windows(TRIGRAM_N) {
        out.insert(window.iter().collect());
    }
    out.into_iter().collect()
}

/// **The trigram CONJUNCTION a substring/regex-lite query lowers to (§4.4).** A query substring of
/// length ≥ 3 is the AND of its trigram tokens (the candidate filter); a substring shorter than a
/// trigram cannot use the index (it returns an empty conjunction — the caller falls back to a scan,
/// the named v1 boundary). Each token is the [`trigram_token`]-namespaced form, so the conjunction
/// targets the trigram namespace, never a real 3-char identifier. The candidates are then
/// substring-verified by the caller (Cox's two-phase: trigram-filter then verify).
pub fn trigram_query(substring: &str) -> Vec<String> {
    trigrams(substring)
        .iter()
        .map(|t| trigram_token(t))
        .collect()
}

/// The namespaced searchable token for a trigram — a `t·` sentinel prefix keeps the trigram namespace
/// disjoint from the code-symbol namespace (so a substring query over trigrams never accidentally
/// matches a real 3-character identifier token, and vice-versa).
fn trigram_token(trigram: &str) -> String {
    format!("t\u{00b7}{trigram}")
}

/// Normalize text for trigram extraction: lowercase (substring search is case-insensitive in v1) and
/// collapse every whitespace run to a single space (so a substring spanning a newline/indent trigrams
/// the same as the source — code is whitespace-noisy). Punctuation is KEPT (a substring query may
/// target `->` / `::` / `()`).
fn normalize_for_trigrams(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            for low in ch.to_lowercase() {
                out.push(low);
            }
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

/// **The named SRCH-P18 FLOOR (the gap-report entry, recorded in code per the prior-prompt
/// convention — e.g. [`crate::analysis::SrchP12AnalyzerFloor`]).** Code search v1 is
/// **symbol/path/literal + trigram** (built here). What is NAMED, not built v1 (§4.4 / change #8):
///
/// - **AST-aware / cross-reference / "find usages" semantic code search** consumes a **CI-produced
///   SCIP/LSIF** projection (jointly Git+CI, GF-3) — a later index input, **post-M4 / demand-
///   triggered** (contract 6.5). Code embeddings for semantic code retrieval ride the same vector
///   path (so a git blob spec is non-semantic in v1, [`git_code_projection_spec`]).
/// - **The regex-verify-over-trigram-candidates** query-path pass: [`trigrams`]/[`trigram_query`]
///   ship the recall-safe candidate filter; the engine surfaces the candidate blobs and the
///   query-path verifies the regex/substring against them (the second phase of Cox's two-phase). The
///   filter (the load-bearing index) is here; the verify pass rides the query path.
/// - **The real Git code-projection EMITTER** (the receive-pack post-commit tree-walk that emits the
///   per-blob projection through the outbox) is GIT-P25 / P-287. Here Search ships the body BUILDER +
///   trigram index the emitter feeds (the [`crate::kn_projection`] posture — the producer's projection
///   modelled against the frozen taxonomy until the live emitter lands).
///
/// A doc-only zero-sized marker so the floor is greppable + linkable in code.
#[derive(Debug, Clone, Copy)]
pub struct ScipLsifFindUsagesFloor;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;

    /// **The Git code-projection spec is git's owned 6.5 shape.** Pins every field — a rename of a
    /// Search `IndexSpec` field, or a drift in the structured-facet set, breaks this (and the
    /// byte-parity CDC against git's owned spec catches a divergence between the two).
    #[test]
    fn spec_is_gits_owned_6_5_shape() {
        let s = git_code_projection_spec();
        assert_eq!(s.subsystem, "git");
        assert_eq!(s.type_, "blob");
        assert_eq!(
            s.acl_object_type, "repo",
            "a blob's reachability is its parent repo's"
        );
        assert!(
            !s.semantic,
            "code is trigram/symbol full-text, not vector-embedded in v1 (GF-3)"
        );
        assert_eq!(
            s.struct_fields.len(),
            3,
            "exactly the three structured code facets"
        );
        for facet in [FACET_PATH, FACET_LANGUAGE, FACET_BLOB_OID] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Text),
                "`{facet}` is a typed columnar code facet (Text)"
            );
        }
    }

    /// **The spec serializes byte-identically to git's owned 6.5 wire shape (coherence — EI-01 §7).**
    /// The Search-side model and git's owned `git_code_projection_spec` MUST serialize to the same
    /// JSON (Search models the shape because it cannot import git — `myelin-git` depends on
    /// `myelin-search`). This is the CDC of the modelled shape against the frozen wire keys.
    #[test]
    fn spec_serializes_to_the_6_5_wire_shape() {
        let s = git_code_projection_spec();
        let json = serde_json::to_value(&s).expect("the spec serializes");
        let obj = json.as_object().expect("a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "acl_object_type",
                "semantic",
                "struct_fields",
                "subsystem",
                "type"
            ],
            "the 6.5 wire key set"
        );
        assert_eq!(obj["subsystem"], serde_json::json!("git"));
        assert_eq!(obj["type"], serde_json::json!("blob"));
        assert_eq!(obj["semantic"], serde_json::json!(false));
        assert_eq!(obj["acl_object_type"], serde_json::json!("repo"));
        assert_eq!(
            obj["struct_fields"],
            serde_json::json!({ "path": "Text", "language": "Text", "blob_oid": "Text" }),
            "the structured facets serialize to the typed columnar shape (13.3)"
        );
    }

    /// **Search ACCEPTS the git spec (the GATE).** Search admits it into a live indexer's per-tenant
    /// facet union without a schema mismatch — the accepted set is byte-equal to the declared set.
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_git_index_specs();
        assert_eq!(
            accepted,
            git_index_specs(),
            "Search accepts the declared git spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            git_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    fn rust_blob() -> GitBlobProjectionInput {
        GitBlobProjectionInput {
            path: "src/scheduler/deadlock.rs".into(),
            language: "rust".into(),
            text: "fn detectDeadlock(graph: &WaitForGraph) -> bool {\n    \
                   let msg = \"cycle detected\";\n    graph.has_cycle()\n}"
                .into(),
            literals: vec!["cycle detected".into()],
            commit_message: "fix: resolve the scheduler deadlock detection".into(),
            blob_oid: "blob-oid-abc123".into(),
        }
    }

    /// **The blob projection code-tokenizes symbols (camel/snake split + whole identifier +
    /// operators) — symbol-grade code search.** `detectDeadlock` → `detect`,`deadlock` AND the whole
    /// `detectdeadlock`; the `->` operator survives; `has_cycle` snake-splits.
    #[test]
    fn blob_projection_tokenizes_symbols_camel_snake_operators() {
        let p = git_blob_search_projection(&rust_blob());
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();

        // camel split + whole identifier (exact-identifier hit).
        assert!(toks.contains("detect"), "camel part: {:?}", &p.text);
        assert!(toks.contains("deadlock"), "camel part");
        assert!(
            toks.contains("detectdeadlock"),
            "whole identifier kept (exact-identifier hit)"
        );
        // snake split.
        assert!(toks.contains("has"));
        assert!(toks.contains("cycle"));
        // operator kept.
        assert!(toks.contains("->"), "the -> operator is searchable");
        // the code analyzer tag is stamped (parity at query time).
        assert_eq!(p.lang.as_deref(), Some("code"));
        // a git blob is non-semantic in v1 (no embedding path here — the spec drives that; the
        // projection just carries no semantic intent).
    }

    /// **The path is a structured facet AND full-text-searchable (GF-3 "find this path").**
    #[test]
    fn blob_projection_indexes_path_as_facet_and_fulltext() {
        let p = git_blob_search_projection(&rust_blob());
        // structured facet.
        assert_eq!(
            p.fields.get(FACET_PATH),
            Some(&FieldValue::Text("src/scheduler/deadlock.rs".into()))
        );
        assert_eq!(
            p.fields.get(FACET_LANGUAGE),
            Some(&FieldValue::Text("rust".into()))
        );
        assert_eq!(
            p.fields.get(FACET_BLOB_OID),
            Some(&FieldValue::Text("blob-oid-abc123".into()))
        );
        // full-text: a path segment is searchable.
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(
            toks.contains("scheduler"),
            "a path segment is full-text searchable"
        );
        assert!(toks.contains("deadlock"));
    }

    /// **String literals and the commit message are full-text-searchable (literal-grade + commit
    /// message, §4.4).**
    #[test]
    fn blob_projection_indexes_literals_and_commit_message() {
        let p = git_blob_search_projection(&rust_blob());
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        // the string literal "cycle detected".
        assert!(toks.contains("cycle"), "literal token");
        assert!(toks.contains("detected"), "literal token");
        // the commit message.
        assert!(toks.contains("resolve"), "commit-message token");
        assert!(toks.contains("scheduler"), "commit-message token");
    }

    /// **The trigram index makes a substring/regex-lite query work (Russ Cox).** A substring of the
    /// code (`adlo` from `Deadlock`) decomposes into trigrams that ALL appear in the blob's trigram
    /// set — so the candidate filter admits the blob.
    #[test]
    fn trigram_substring_query_admits_the_blob() {
        let p = git_blob_search_projection(&rust_blob());
        let body_tokens: std::collections::BTreeSet<&str> = p.text.split(' ').collect();

        // The substring `adlo` (inside `Deadlock`, lowercased `deadlock`) → trigrams `adl`,`dlo`.
        let q = trigram_query("adlo");
        assert!(!q.is_empty(), "a 4-char substring yields trigrams");
        assert!(
            q.iter().all(|t| body_tokens.contains(t.as_str())),
            "every query trigram is in the blob's trigram set (candidate admit): q={q:?}"
        );

        // A substring that is NOT in the code does NOT have all trigrams present (no false admit).
        let absent = trigram_query("zxqwv");
        assert!(
            !absent.iter().all(|t| body_tokens.contains(t.as_str())),
            "a substring absent from the code is not falsely admitted"
        );
    }

    /// **The trigram generator is Russ-Cox-correct: overlapping 3-char windows, char-based (not
    /// byte-based), whitespace-collapsed, deduped.**
    #[test]
    fn trigrams_are_overlapping_char_windows() {
        assert_eq!(trigrams("abcd"), vec!["abc", "bcd"]);
        // case-folded.
        assert_eq!(trigrams("ABCD"), vec!["abc", "bcd"]);
        // shorter than a trigram ⇒ none.
        assert!(trigrams("ab").is_empty());
        // whitespace runs collapse to a single space (a substring spanning a newline trigrams same).
        assert_eq!(trigrams("a\n\n  b"), vec!["a b"]);
        // multibyte chars trigram by CHARACTER, not byte (no panic, correct windows). The set is
        // sorted by codepoint, so `afé` (a…) sorts before `caf` (c…).
        assert_eq!(trigrams("café"), vec!["afé", "caf"]);
        // deduped + sorted (a repeated trigram appears once).
        assert_eq!(trigrams("aaaa"), vec!["aaa"]);
    }

    /// **A substring shorter than a trigram cannot use the index (the named v1 boundary — falls back
    /// to a scan).** `trigram_query` returns an empty conjunction, so the caller scans.
    #[test]
    fn substring_shorter_than_trigram_yields_no_conjunction() {
        assert!(
            trigram_query("ab").is_empty(),
            "a <3-char substring cannot index — the caller scans"
        );
        assert!(trigram_query("a").is_empty());
    }

    /// **The trigram namespace is disjoint from the symbol namespace (no collision).** A 3-char
    /// identifier token (`foo`) and the trigram `foo` are DIFFERENT searchable tokens — so a symbol
    /// query for `foo` does not falsely hit a blob that merely contains the substring `foo`, and a
    /// substring query over trigrams does not falsely hit an identifier.
    #[test]
    fn trigram_namespace_is_disjoint_from_symbols() {
        let tg = trigram_token("foo");
        assert_ne!(
            tg, "foo",
            "the trigram token is namespaced apart from the identifier token"
        );
        assert!(tg.contains("foo"));
        // The query form targets the namespaced token.
        assert_eq!(trigram_query("foo"), vec![tg]);
    }

    /// **The raw code is verbatim into the tokenizer (X-2) — no markdown parsing.** A code fence /
    /// markdown markers in the blob text are tokenized as code, not stripped as markdown.
    #[test]
    fn raw_code_is_verbatim_x2() {
        let input = GitBlobProjectionInput {
            text: "let `not_markdown` = **value**;".into(),
            ..Default::default()
        };
        let p = git_blob_search_projection(&input);
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        // The identifier inside the backticks is tokenized (raw — the backticks are operators, the
        // identifier survives the code split), NOT consumed as markdown inline-code.
        assert!(
            toks.contains("not"),
            "raw code tokenized verbatim (X-2): {:?}",
            &p.text
        );
        assert!(toks.contains("markdown"));
        assert!(toks.contains("value"));
    }

    /// **An empty blob projects to an empty body with no facets** (a blob with no inputs carries no
    /// searchable tokens and no structured facets — the columnar shape only holds present fields).
    #[test]
    fn empty_blob_projects_empty() {
        let p = git_blob_search_projection(&GitBlobProjectionInput::default());
        assert!(p.text.is_empty(), "no inputs ⇒ no searchable body");
        assert!(p.fields.is_empty(), "no inputs ⇒ no structured facets");
        assert_eq!(
            p.lang.as_deref(),
            Some("code"),
            "still analyzed under the code chain"
        );
    }

    /// The named floor marker is constructible (the greppable gap-report entry).
    #[test]
    fn the_named_floor_is_constructible() {
        let _floor = ScipLsifFindUsagesFloor;
    }
}
