use myelin_content::{InlineNode, OBJ};
use myelin_events::ArtifactRef;
use myelin_git::body::{extract_body_edges, Body};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

fn alice() -> Principal {
    Principal::stub(
        PrincipalId("p-alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn page() -> InlineNode {
    InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into()))
}

fn issue() -> InlineNode {
    InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into()))
}

fn corpus() -> Vec<(String, Vec<InlineNode>)> {
    vec![
        ("LGTM, merging.".into(), vec![]),
        (String::new(), vec![]),
        ("**bold** review note".into(), vec![]),
        ("*nit*: rename this".into(), vec![]),
        ("use `cargo clippy` first".into(), vec![]),
        ("~~obsolete~~ removed".into(), vec![]),
        ("**bold *and italic***".into(), vec![]),
        ("a **b `c` d** e".into(), vec![]),
        ("see [the RFC](https://acme.test/rfc/42)".into(), vec![]),
        ("[**bold link**](https://x.test/p)".into(), vec![]),
        ("[t](https://x.test/a\\)b)".into(), vec![]),
        ("a \\* b".into(), vec![]),
        ("\\[not a link]".into(), vec![]),
        ("rename `do_thing` to `do_other` in f(x)".into(), vec![]),
        (
            format!("cc {OBJ} for review"),
            vec![InlineNode::Mention(alice())],
        ),
        (format!("{OBJ} fixes the panic"), vec![issue()]),
        (format!("design: {OBJ}"), vec![page()]),
        (
            format!("**{OBJ}** please look at *{OBJ}* and `{OBJ}`"),
            vec![InlineNode::Mention(alice()), issue(), page()],
        ),
        (format!("[see {OBJ} here](https://x.test/p)"), vec![page()]),
    ]
}

#[test]
fn git_body_corpus_round_trips_at_100_percent() {
    let corpus = corpus();
    let total = corpus.len();
    let mut passed = 0usize;
    for (md, nodes) in &corpus {
        let body = Body::new(md.clone(), nodes.clone());
        let rendered = body.render();
        assert_eq!(&rendered, md, "round-trip mismatch for body {md:?}");
        assert_eq!(
            body.parse().nodes,
            *nodes,
            "node array not preserved for {md:?}"
        );
        if body.round_trips() {
            passed += 1;
        }
    }
    assert_eq!(
        passed, total,
        "git-body round-trip parity must be 100% ({passed}/{total})"
    );
}

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
