//! # The KN-D2-class round-trip parity drill applied to GIT bodies (GIT-P17 / P-278, M3-G3)
//!
//! **Contract 13.1:** PR/review/comment bodies use the FROZEN `myelin-content` markdown-subset +
//! the three structured inline nodes; `render(parse(md)) === md`. The GATE is **100% round-trip parity**
//! over a corpus of representative git bodies (0 regressions) — the same KN-D2 correctness bar Knowledge
//! holds, applied to git content. There is no git-local renderer: [`Body::render`] runs the ONE
//! `myelin_content::serialize_inline` over the ONE `myelin_content::parse_inline`, so a parity pass here
//! is a pass on the SAME single source the WASM editor compiles from (EI-01 §7 — the two-divergent-
//! renderers trap is eliminated structurally).

use myelin_content::{InlineNode, OBJ};
use myelin_events::ArtifactRef;
use myelin_git::body::{extract_body_edges, Body};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

fn alice() -> Principal {
    Principal::stub(PrincipalId("p-alice".into()), PrincipalKind::Human, TenantId("acme".into()))
}

fn page() -> InlineNode {
    InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into()))
}

fn issue() -> InlineNode {
    InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into()))
}

/// The frozen corpus of representative git PR/review/comment bodies: plain prose, every mark, nesting,
/// links, code-verbatim, escapes, and bodies carrying the three structured nodes. Each entry is the
/// canonical markdown-subset `md` + its positional structured-node array. Every entry MUST round-trip
/// `render(parse(md)) === md` byte-identically.
fn corpus() -> Vec<(String, Vec<InlineNode>)> {
    vec![
        // plain prose (a one-line comment).
        ("LGTM, merging.".into(), vec![]),
        // an empty body (a PR opened with no description).
        (String::new(), vec![]),
        // every mark.
        ("**bold** review note".into(), vec![]),
        ("*nit*: rename this".into(), vec![]),
        ("use `cargo clippy` first".into(), vec![]),
        ("~~obsolete~~ removed".into(), vec![]),
        // nested marks (the frozen disambiguation rule).
        ("**bold *and italic***".into(), vec![]),
        ("a **b `c` d** e".into(), vec![]),
        // a link (a PR body referencing a doc).
        ("see [the RFC](https://acme.test/rfc/42)".into(), vec![]),
        ("[**bold link**](https://x.test/p)".into(), vec![]),
        // a url with a `*` (verbatim, not a delimiter) + an escaped `)`.
        ("[t](https://x.test/a\\)b)".into(), vec![]),
        // escapes — a literal `*` and a literal `[`.
        ("a \\* b".into(), vec![]),
        ("\\[not a link]".into(), vec![]),
        // `snake_case` + `f(x)` pass through byte-stable (`_`/`(`/`)` are not delimiters).
        ("rename `do_thing` to `do_other` in f(x)".into(), vec![]),
        // a structured mention node.
        (format!("cc {OBJ} for review"), vec![InlineNode::Mention(alice())]),
        // a structured artifact-ref ("Closes" as a structured link).
        (format!("{OBJ} fixes the panic"), vec![issue()]),
        // a structured embed.
        (format!("design: {OBJ}"), vec![page()]),
        // a body carrying all three structured nodes interleaved with marks.
        (
            format!("**{OBJ}** please look at *{OBJ}* and `{OBJ}`"),
            vec![InlineNode::Mention(alice()), issue(), page()],
        ),
        // a structured node inside a link's text (the node keeps its link/mark context).
        (format!("[see {OBJ} here](https://x.test/p)"), vec![page()]),
    ]
}

/// **THE GATE: 100% round-trip parity over the git-body corpus (0 regressions).** Every body in the
/// corpus re-serialises byte-identically to its canonical `md`.
#[test]
fn git_body_corpus_round_trips_at_100_percent() {
    let corpus = corpus();
    let total = corpus.len();
    let mut passed = 0usize;
    for (md, nodes) in &corpus {
        let body = Body::new(md.clone(), nodes.clone());
        let rendered = body.render();
        assert_eq!(&rendered, md, "round-trip mismatch for body {md:?}");
        // the structured-node array is preserved through parse (positional binding intact).
        assert_eq!(body.parse().nodes, *nodes, "node array not preserved for {md:?}");
        if body.round_trips() {
            passed += 1;
        }
    }
    assert_eq!(passed, total, "git-body round-trip parity must be 100% ({passed}/{total})");
}

/// **The node-count → edge-count invariant over the corpus: N structured nodes → N edges (1 per node).**
/// A body's structured nodes each produce exactly one reference edge (0 dup, 0 missed) — proven across
/// the whole corpus, so the gate is the corpus-wide property, not a single fixture.
#[test]
fn git_body_corpus_emits_exactly_one_edge_per_structured_node() {
    let source = ArtifactRef("myelin://acme/git/pr/repo7:42#comment-cAbc".into());
    for (md, nodes) in corpus() {
        let body = Body::new(md.clone(), nodes.clone());
        let edges = extract_body_edges(&source, body.structured_nodes());
        assert_eq!(
            edges.len(),
            nodes.len(),
            "body {md:?}: {} structured nodes must produce {} edges (1 per node)",
            nodes.len(),
            nodes.len()
        );
    }
}
