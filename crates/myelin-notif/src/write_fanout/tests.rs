//! Unit tests for the write-fanout decision logic (NOTIF-P12 / P-190) — the mutation-floor core:
//! [`extract_mentions`] (reads ONLY the structured node, never free text) + [`HotSubjectCap::admit`]
//! (the per-`subject_root` distinct-count cap that bounds write-amplification).

use super::*;
use myelin_content::InlineNode;
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

// --- extract_mentions: reads ONLY the structured Mention node, never free text (AG-6) ---

/// **`extract_mentions` returns the mentioned principals from the STRUCTURED nodes, in order.** Only
/// `InlineNode::Mention(Principal)` is a mention — `ArtifactRefNode`/`Embed` are NOT mentions and do
/// not fan out. The parameter is `&[InlineNode]` (the frozen 13.1 node) — a free-text parse is
/// unconstructable at this seam (AG-6). A mutant that returns the wrong node or an empty vec is caught.
#[test]
fn extract_mentions_reads_only_the_structured_mention_node() {
    let nodes = vec![
        InlineNode::Mention(principal("p-alice")),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())),
        InlineNode::Mention(principal("p-bob")),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into())),
    ];
    let mentions = extract_mentions(&nodes);
    assert_eq!(
        mentions.len(),
        2,
        "exactly the two Mention nodes (ref/embed are not mentions)"
    );
    assert_eq!(mentions[0].principal_id.0, "p-alice", "in order");
    assert_eq!(mentions[1].principal_id.0, "p-bob");
}

/// **No structured Mention node → no mentions (no free-text fallback).** A body of only
/// refs/embeds — or an empty body — fans out NOTHING. There is no free-text scrape (AG-6); the only
/// recipient source is the structured node.
#[test]
fn extract_mentions_no_structured_node_no_fanout() {
    assert!(
        extract_mentions(&[]).is_empty(),
        "an empty body → no mentions"
    );
    let only_refs = vec![
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/git/pr/9".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/1".into())),
    ];
    assert!(
        extract_mentions(&only_refs).is_empty(),
        "ref/embed nodes are not mentions — no free-text fallback, no fanout"
    );
}

/// **The same person mentioned twice in one body is ONE recipient (deduped by `principal_id`).**
/// Mentioning `@alice` twice in one comment is one high-signal recipient, not two — the write-fanout
/// materialises ONE row for her. A mutant that drops the dedup (returns two alice entries) is caught.
#[test]
fn extract_mentions_dedups_the_same_principal_in_one_body() {
    let nodes = vec![
        InlineNode::Mention(principal("p-alice")),
        InlineNode::Mention(principal("p-alice")),
        InlineNode::Mention(principal("p-bob")),
    ];
    let mentions = extract_mentions(&nodes);
    assert_eq!(
        mentions.len(),
        2,
        "alice twice → one recipient (deduped); bob → one"
    );
    assert_eq!(mentions[0].principal_id.0, "p-alice");
    assert_eq!(mentions[1].principal_id.0, "p-bob");
}

// --- HotSubjectCap::admit: the per-subject_root distinct-count cap bounds write-amplification ---

/// **Within the cap, every DISTINCT recipient is ADMITTED (materialises its own row).** A normal
/// thread mentioning a handful of people (well under the cap) fans out one row each. A mutant that
/// overflows early (mis-counts the slot) is caught.
#[test]
fn hot_cap_admits_within_the_cap() {
    let cap = HotSubjectCap::with_cap(3);
    let root = "myelin://acme/chat/thread/T1";
    assert_eq!(cap.admit("p-a", root), CapVerdict::Admit, "1st within cap");
    assert_eq!(cap.admit("p-b", root), CapVerdict::Admit, "2nd within cap");
    assert_eq!(
        cap.admit("p-c", root),
        CapVerdict::Admit,
        "3rd within cap (== cap)"
    );
    assert_eq!(
        cap.admitted_count(root),
        3,
        "three distinct recipients admitted"
    );
    assert_eq!(
        cap.overflow_count(root),
        0,
        "none overflowed within the cap"
    );
}

/// **A NEW distinct recipient PAST the cap OVERFLOWS (bounded write-amplification, §3.2.4).** A
/// mention-storm of 10 distinct recipients on a cap-3 subject_root materialises exactly 3 rows; the
/// other 7 overflow into the coalesced marker (counted, never lost). A mutant that drops the cap
/// (admits all 10) or loses the overflow count is caught — this is the write-amplification bound.
#[test]
fn hot_cap_overflows_a_storm_past_the_cap_bounded() {
    let cap = HotSubjectCap::with_cap(3);
    let root = "myelin://acme/chat/thread/hot";
    let mut admitted = 0;
    let mut overflowed = 0;
    for i in 0..10 {
        match cap.admit(&format!("p-{i}"), root) {
            CapVerdict::Admit => admitted += 1,
            CapVerdict::Overflow => overflowed += 1,
        }
    }
    assert_eq!(
        admitted, 3,
        "exactly `cap` rows materialise (bounded write-fanout)"
    );
    assert_eq!(overflowed, 7, "the rest overflow into the coalesced marker");
    assert_eq!(cap.admitted_count(root), 3, "admitted bounded by the cap");
    assert_eq!(
        cap.overflow_count(root),
        7,
        "the overflow count is preserved (bounded, not lost)"
    );
}

/// **A repeat of an already-admitted recipient is ADMITTED (it collapses on the dedup key, never a
/// fresh slot).** Mentioning the same person again on the same subject_root re-admits (so the UPSERT
/// bumps `coalesce_count`); it does NOT consume a new cap slot or open a new row. A mutant that
/// treats a repeat as a new slot (filling the cap with one spammed recipient) is caught.
#[test]
fn hot_cap_repeat_recipient_does_not_consume_a_fresh_slot() {
    let cap = HotSubjectCap::with_cap(2);
    let root = "myelin://acme/chat/thread/T1";
    assert_eq!(cap.admit("p-a", root), CapVerdict::Admit);
    // p-a again, many times — never consumes a fresh slot (re-admit, dedup-key collapse handles it).
    for _ in 0..100 {
        assert_eq!(
            cap.admit("p-a", root),
            CapVerdict::Admit,
            "a repeat re-admits, no new slot"
        );
    }
    assert_eq!(
        cap.admitted_count(root),
        1,
        "still ONE distinct admitted recipient"
    );
    // A genuinely NEW recipient still fits the cap (the repeats did not exhaust it).
    assert_eq!(
        cap.admit("p-b", root),
        CapVerdict::Admit,
        "a new recipient still fits (cap not eaten)"
    );
    assert_eq!(
        cap.admit("p-c", root),
        CapVerdict::Overflow,
        "now past cap-2 → overflow"
    );
}

/// **The cap is PER subject_root — distinct subjects have independent budgets.** A storm on thread A
/// does not consume thread B's budget. A mutant that shares one global counter is caught.
#[test]
fn hot_cap_is_per_subject_root() {
    let cap = HotSubjectCap::with_cap(1);
    let a = "myelin://acme/chat/thread/A";
    let b = "myelin://acme/chat/thread/B";
    assert_eq!(
        cap.admit("p-a", a),
        CapVerdict::Admit,
        "A's first fits A's cap"
    );
    assert_eq!(
        cap.admit("p-b", a),
        CapVerdict::Overflow,
        "A's second overflows A's cap-1"
    );
    assert_eq!(
        cap.admit("p-a", b),
        CapVerdict::Admit,
        "B has its OWN cap (independent budget)"
    );
    assert_eq!(cap.admitted_count(a), 1);
    assert_eq!(cap.admitted_count(b), 1);
}

/// **The default cap is the frozen platform floor.** Read from ONE place; the drills tune it (T-5)
/// but it is a real bound, never weakened. A mutant that changes the default is caught.
#[test]
fn default_cap_is_the_frozen_floor() {
    assert_eq!(DEFAULT_HOT_SUBJECT_WRITE_CAP, 64);
    assert_eq!(
        HotSubjectCap::new().cap(),
        64,
        "the default constructor uses the frozen floor"
    );
    assert_eq!(HotSubjectCap::default().cap(), 64);
}

/// **An unseen subject_root reports zero admitted / zero overflow** (no phantom state). A mutant that
/// returns a non-zero default is caught.
#[test]
fn unseen_root_reports_zero() {
    let cap = HotSubjectCap::new();
    assert_eq!(cap.admitted_count("myelin://acme/never/seen"), 0);
    assert_eq!(cap.overflow_count("myelin://acme/never/seen"), 0);
}
