//! # Integration — SRCH-P18 (P-261, M3): Search indexes the REAL Git corpus (code search v1)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D1 (F1 — the
//! zero-escape leak: a private repo's blob NEVER in any result incl. counts) + SRCH-D3 (F2 —
//! cross-tenant IDOR = 0), the gate-invariant ratchet **re-confirmed on the REAL Git corpus** (the M2
//! drills re-run on each new producer corpus). **Architecture:** `search-and-indexing.md` §4.4 (code
//! search v1 = symbol/path/literal + trigram, the code tokenizer; `code_block.text` raw — X-2). **Contracts:**
//! 6.5 (consume the real Git `git.*` projection per blob/ref/symbol), 13.1 (the raw code-block text).
//! **Reconciliation:** change #8 (the SCIP/LSIF find-usages follow-on named, post-M4).
//!
//! ## What this proves (the dated green artifact, 2026-06-21)
//! A REAL Git code corpus (blobs of Rust/Python source, paths, string literals, commit messages) is
//! projected through [`git_blob_search_projection`] (git's owned 6.5 projection BUILDER, the genuine
//! code-tokenizer + trigram index) into the LIVE [`IncrementalIndexer`] per-event pipeline
//! (project-fetch → analyze → upsert), then queried back through the engine surface. The GATE:
//!
//! 1. **Code search v1 correctness** — a SYMBOL query (camel/snake-split identifier) returns the right
//!    blob; an EXACT-identifier query hits the whole token; a PATH query hits the blob at that path; a
//!    string-LITERAL query hits; a COMMIT-MESSAGE query hits; a TRIGRAM substring/regex-lite query hits.
//! 2. **SRCH-D1 (F1) on the Git corpus** — a PRIVATE repo's blob never appears in ANY result (incl.
//!    counts) for an unauthorized viewer; a grant ⇒ it appears (the rejection was the ACL, not a deny).
//! 3. **SRCH-D3 (F2) on the Git corpus** — a viewer's tenant partitions the index; a cross-tenant
//!    query sees 0 of the other tenant's blobs (the per-tenant index, partition-keyed).
//!
//! The ENGINE is UNCHANGED — this is producer-corpus wiring (the prompt's DoD). No new mutation-core
//! module is added; the SRCH-P09 mutation floor still holds (the engine decision logic is the same
//! one that drill mutation-tests; here it runs on Git code). The SCIP/LSIF find-usages floor is named
//! (post-M4, change #8).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::FieldValue;
use myelin_search::{
    git_blob_search_projection, git_code_projection_spec, trigram_query, AclFilter,
    GitBlobProjectionInput, IncrementalIndexer, MockEmbeddingAdapter, ProjectFetchError,
    ProjectFetcher, SearchProjection, GIT_FACET_LANGUAGE, GIT_FACET_PATH,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

// ----------------------------------------------------------------------------------------------
// fixtures — the REAL Git corpus (blobs projected through git's owned 6.5 builder)
// ----------------------------------------------------------------------------------------------

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str, t: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(t.into()),
    )
}

/// A scripted [`ProjectFetcher`] over a `ref → SearchProjection` map — the owner's `project(ref,
/// viewer)` (5.6 / 6.5). The REAL Git corpus is built by [`git_blob_search_projection`] over blob
/// inputs, so this fetcher serves Git's genuine 6.5 projection (NOT a repo read — the no-cross-db
/// floor; Search does NOT parse repos, §4.4).
#[derive(Default)]
struct GitFetcher {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl GitFetcher {
    fn put(&self, ref_: &str, p: SearchProjection) {
        self.projections.lock().unwrap().insert(ref_.to_string(), p);
    }
}
impl ProjectFetcher for GitFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.projections.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

fn git_event(id: &str, type_: &str, subject: &str, t: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId(t.into()),
        region: region(),
        actor: Actor(viewer("platform", t)),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-git-1", "version": 1 }),
    }
}

/// Build the indexer over the REAL Git spec (the `git`/`blob` code-projection shape).
fn git_indexer(fetcher: Arc<GitFetcher>) -> IncrementalIndexer {
    IncrementalIndexer::new(
        vec![git_code_projection_spec()],
        fetcher,
        Arc::new(MockEmbeddingAdapter::new(8)),
    )
}

/// A real Rust blob (camelCase + snake_case symbols, a string literal, a commit message).
fn scheduler_blob() -> GitBlobProjectionInput {
    GitBlobProjectionInput {
        path: "src/scheduler/deadlock.rs".into(),
        language: "rust".into(),
        text: "pub fn detectDeadlock(graph: &WaitForGraph) -> bool {\n    \
               let reason = \"cycle detected\";\n    graph.has_cycle()\n}"
            .into(),
        literals: vec!["cycle detected".into()],
        commit_message: "fix: resolve the scheduler deadlock detection".into(),
        blob_oid: "oid-rust-1".into(),
    }
}

/// A real Python blob (a different path/language; distinct symbols so a symbol query is selective).
fn parser_blob() -> GitBlobProjectionInput {
    GitBlobProjectionInput {
        path: "lib/parser/tokenizer.py".into(),
        language: "python".into(),
        text: "def parse_html(source):\n    return Tokenizer(source).run()".into(),
        literals: vec![],
        commit_message: "feat: add the html tokenizer".into(),
        blob_oid: "oid-py-1".into(),
    }
}

// ----------------------------------------------------------------------------------------------
// 1. Code search v1 correctness — symbol / exact-id / path / literal / commit-message / trigram
// ----------------------------------------------------------------------------------------------

/// **A symbol/path/literal/commit-message query returns the right blob; a trigram substring query
/// works; identifiers tokenize via camel/snake keeping operators (the SRCH-P18 GATE).**
#[test]
fn git_code_search_v1_symbol_path_literal_commit_trigram() {
    let rust_ref = "myelin://acme/git/blob/repoA:main:src/scheduler/deadlock.rs";
    let py_ref = "myelin://acme/git/blob/repoA:main:lib/parser/tokenizer.py";
    let fetcher = Arc::new(GitFetcher::default());
    fetcher.put(rust_ref, git_blob_search_projection(&scheduler_blob()));
    fetcher.put(py_ref, git_blob_search_projection(&parser_blob()));
    let ix = git_indexer(fetcher);

    ix.index(&git_event("e-rust", "git.blob.indexed", rust_ref, "acme"))
        .expect("index rust blob");
    ix.index(&git_event("e-py", "git.blob.indexed", py_ref, "acme"))
        .expect("index python blob");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both Git blobs are live"
    );

    let acl = AclFilter::ids([rust_ref, py_ref]);

    // SYMBOL (camel/snake split): `deadlock` is a camel part of `detectDeadlock` → the Rust blob.
    let sym = ix
        .search_ft(&tenant(), &region(), &acl, "deadlock", 10)
        .expect("symbol search");
    assert!(
        sym.iter().any(|h| h.doc_id == rust_ref),
        "the symbol part `deadlock` finds the Rust blob"
    );
    assert!(
        !sym.iter().any(|h| h.doc_id == py_ref),
        "the symbol does not find the unrelated Python blob"
    );

    // EXACT identifier: the whole lowercased identifier `detectdeadlock` hits.
    let exact = ix
        .search_ft(&tenant(), &region(), &acl, "detectdeadlock", 10)
        .expect("exact-id search");
    assert!(
        exact.iter().any(|h| h.doc_id == rust_ref),
        "the whole identifier `detectdeadlock` hits"
    );

    // PATH: a path segment `tokenizer` (and the snake symbol `parse_html`) → the Python blob.
    let path_hit = ix
        .search_ft(&tenant(), &region(), &acl, "tokenizer", 10)
        .expect("path search");
    assert!(
        path_hit.iter().any(|h| h.doc_id == py_ref),
        "the path segment `tokenizer` finds the Python blob"
    );

    // LITERAL: the string literal "cycle detected" → the Rust blob.
    let lit = ix
        .search_ft(&tenant(), &region(), &acl, "cycle", 10)
        .expect("literal search");
    assert!(
        lit.iter().any(|h| h.doc_id == rust_ref),
        "the string-literal token `cycle` finds the Rust blob"
    );

    // COMMIT MESSAGE: `resolve` is from the Rust blob's commit message.
    let commit = ix
        .search_ft(&tenant(), &region(), &acl, "resolve", 10)
        .expect("commit-message search");
    assert!(
        commit.iter().any(|h| h.doc_id == rust_ref),
        "the commit-message token `resolve` finds the Rust blob"
    );

    // STRUCTURED PATH FACET (GF-3 "find this path"): an exact path equality returns just that blob.
    let path_facet = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            GIT_FACET_PATH,
            &FieldValue::Text("src/scheduler/deadlock.rs".into()),
            10,
        )
        .expect("path facet scan");
    assert_eq!(path_facet.len(), 1, "exactly the blob at that path");
    assert_eq!(path_facet[0].doc_id, rust_ref);

    // STRUCTURED LANGUAGE FACET: language == python → just the Python blob.
    let lang_facet = ix
        .search_structured(
            &tenant(),
            &region(),
            &acl,
            GIT_FACET_LANGUAGE,
            &FieldValue::Text("python".into()),
            10,
        )
        .expect("language facet scan");
    assert_eq!(lang_facet.len(), 1, "exactly the python blob");
    assert_eq!(lang_facet[0].doc_id, py_ref);

    // TRIGRAM substring/regex-lite: the substring `adlo` (inside `Deadlock`) decomposes into trigrams;
    // a conjunction of those trigram tokens (the Cox candidate filter) returns the Rust blob. We query
    // the FT body for the trigram conjunction (every trigram MUST be present — `+` is Tantivy MUST).
    let q = trigram_query("adlo");
    assert!(!q.is_empty(), "a 4-char substring yields trigrams");
    let conjunction = q
        .iter()
        .map(|t| format!("+\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let tri = ix
        .search_ft(&tenant(), &region(), &acl, &conjunction, 10)
        .expect("trigram search");
    assert!(
        tri.iter().any(|h| h.doc_id == rust_ref),
        "the trigram substring candidate filter admits the Rust blob: q={conjunction}"
    );
    assert!(
        !tri.iter().any(|h| h.doc_id == py_ref),
        "the Python blob (no `deadlock` substring) is not a trigram candidate"
    );
}

// ----------------------------------------------------------------------------------------------
// 2. SRCH-D1 (F1) — the zero-escape leak on the REAL Git corpus
// ----------------------------------------------------------------------------------------------

/// **SRCH-D1 (F1) re-confirmed on the Git corpus: a PRIVATE repo's blob never appears in ANY result
/// (incl. counts) for an unauthorized viewer — and a grant makes it appear (the rejection was the ACL
/// firing, not a blanket deny).** Both blobs carry the SAME rare symbol so a leak would be exposed by
/// FT/count/IDF inference.
#[test]
fn srch_d1_private_git_blob_never_leaks() {
    let visible = "myelin://acme/git/blob/public-repo:main:src/lib.rs";
    let private = "myelin://acme/git/blob/secret-repo:main:src/secret.rs";
    let fetcher = Arc::new(GitFetcher::default());

    // BOTH blobs contain the SAME rare identifier `zarquonReactor` — so a leak surfaces via FT/count.
    let public_blob = GitBlobProjectionInput {
        path: "src/lib.rs".into(),
        text: "fn zarquonReactor() { /* public */ }".into(),
        ..Default::default()
    };
    let secret_blob = GitBlobProjectionInput {
        path: "src/secret.rs".into(),
        text: "fn zarquonReactor() { classified_launch_codes() }".into(),
        ..Default::default()
    };
    fetcher.put(visible, git_blob_search_projection(&public_blob));
    fetcher.put(private, git_blob_search_projection(&secret_blob));

    let ix = git_indexer(fetcher);
    ix.index(&git_event("v", "git.blob.indexed", visible, "acme"))
        .expect("index visible");
    ix.index(&git_event("p", "git.blob.indexed", private, "acme"))
        .expect("index private");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        2,
        "both blobs are indexed"
    );

    // The unauthorized viewer's reachable set is JUST the visible repo's blob (the private repo is NOT
    // in it — git ACL is repo-granular, and the viewer is not a member of secret-repo).
    let acl_unauth = AclFilter::ids([visible]);

    // FT: the shared rare symbol `zarquonreactor` — only the visible blob surfaces; private never does.
    let hits = ix
        .search_ft(&tenant(), &region(), &acl_unauth, "zarquonreactor", 10)
        .expect("ft");
    assert_eq!(
        hits.len(),
        1,
        "0 count-leak: exactly the one visible blob (hidden blob never counted)"
    );
    assert_eq!(hits[0].doc_id, visible);
    assert!(
        !hits.iter().any(|h| h.doc_id == private),
        "0 leak: the private blob never surfaces"
    );

    // Even a trigram substring of the SECRET-only content never surfaces the private blob.
    let q = trigram_query("classified");
    let conjunction = q
        .iter()
        .map(|t| format!("+\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let tri = ix
        .search_ft(&tenant(), &region(), &acl_unauth, &conjunction, 10)
        .expect("trigram ft");
    assert!(
        !tri.iter().any(|h| h.doc_id == private),
        "0 leak: a substring unique to the private blob never surfaces it for an unauthorized viewer"
    );

    // The chained grant: the viewer is now a member of secret-repo → the private blob becomes visible.
    let acl_granted = AclFilter::ids([visible, private]);
    let granted = ix
        .search_ft(&tenant(), &region(), &acl_granted, "zarquonreactor", 10)
        .expect("ft granted");
    assert_eq!(
        granted.len(),
        2,
        "after the grant BOTH blobs surface (the rejection was the ACL, not a deny)"
    );
    assert!(
        granted.iter().any(|h| h.doc_id == private),
        "the granted private blob now appears"
    );
}

// ----------------------------------------------------------------------------------------------
// 3. SRCH-D3 (F2) — cross-tenant IDOR = 0 on the REAL Git corpus
// ----------------------------------------------------------------------------------------------

/// **SRCH-D3 (F2) re-confirmed on the Git corpus: a viewer's tenant partitions the index — a query
/// against a DIFFERENT tenant's index sees 0 of this tenant's blobs (the per-tenant index, §3.4).**
#[test]
fn srch_d3_cross_tenant_git_blobs_do_not_leak() {
    // Two tenants index a blob under a COLLIDING repo:path namespace, so only the partition key
    // (tenant, region) keeps them apart — not a lucky id difference.
    let acme_blob = "myelin://acme/git/blob/shared:main:src/main.rs";
    let evil_blob = "myelin://evil/git/blob/shared:main:src/main.rs";
    let fetcher = Arc::new(GitFetcher::default());
    fetcher.put(acme_blob, git_blob_search_projection(&scheduler_blob()));
    fetcher.put(evil_blob, git_blob_search_projection(&scheduler_blob()));
    let ix = git_indexer(fetcher);
    ix.index(&git_event("a", "git.blob.indexed", acme_blob, "acme"))
        .expect("index acme");
    ix.index(&git_event("e", "git.blob.indexed", evil_blob, "evil"))
        .expect("index evil");

    let acme_t = TenantId("acme".into());
    let evil_t = TenantId("evil".into());

    // Positive control: acme's viewer querying acme's index sees acme's blob.
    let acme_hits = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([acme_blob]),
            "deadlock",
            10,
        )
        .expect("acme search");
    assert!(
        acme_hits.iter().any(|h| h.doc_id == acme_blob),
        "acme sees its own blob"
    );

    // The cross-tenant attack: even with an allow-set NAMING the evil blob's doc-id, querying ACME's
    // partition returns 0 — the evil blob lives in a DIFFERENT (tenant, region) index entirely.
    let cross = ix
        .search_ft(
            &acme_t,
            &region(),
            &AclFilter::ids([evil_blob]),
            "deadlock",
            10,
        )
        .expect("cross-tenant search");
    assert!(
        cross.is_empty(),
        "0 cross-tenant: acme's index holds none of evil's blobs"
    );

    // And the evil tenant's index, conversely, holds only evil's blob.
    let evil_hits = ix
        .search_ft(
            &evil_t,
            &region(),
            &AclFilter::ids([acme_blob]),
            "deadlock",
            10,
        )
        .expect("evil search");
    assert!(
        evil_hits.is_empty(),
        "0 cross-tenant: evil's index holds none of acme's blobs"
    );
}
