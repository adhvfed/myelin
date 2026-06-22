//! # CHAT-D5 + the HITL card — Notif's humanise leak surface at the CHAT seam (NOTIF-P22 / P-343, M4)
//!
//! **Drill catalogue** row **CHAT-D5** (F1): "Notify/unfurl a confidential artifact to a viewer
//! lacking access → tombstone rendered, title never present." **Threshold: 0 title leak.** This
//! re-confirms NOTIF-D4 (the load-bearing humanise leak invariant) AT THE CHAT SEAM — a chat
//! `@mention` / unfurl of a confidential channel/message humanises to a tombstone for a viewer who
//! cannot see it, the secret title NEVER in the output. **Architecture**
//! `05-refined-shared-systems-architecture/notifications.md` §2.1 (`template_args` holds
//! `ArtifactRef`s, never rendered strings — the human string is produced at READ time by resolving
//! each ref per-viewer; a viewer who lost access sees a tombstone, not a stale title) + §1.4 (an HITL
//! approval card is a Notif item `reason = approval_requested` at high priority — the card humanises
//! through the SAME ONE templating surface, OQ-L).
//!
//! Chat REGISTERS its humanise keys (the card / agent-message / `chat.message.mentioned` strings —
//! [`myelin_chat::glue::chat_humanise_templates`]); Notif owns the ONE templating surface
//! ([`myelin_notif::humanise`]) and the per-viewer leak-free resolve. This drill wires chat's
//! REGISTERED card key through the REAL `humanise` pipeline and proves:
//! - a confidential chat subject humanises to a tombstone for a denied viewer — **0 title leak**;
//! - the SAME card renders the title for the ALLOWED approver (the complement — not vacuously blank);
//! - the HITL card surfaces **action + risk + cost** (refined §1.4 / NOTIF-P9) per-viewer-safe: the
//!   subject slot tombstones on deny while action/risk/cost (PII-free agent strings) still render.
//!
//! The named floor: the production Refs `resolve` chokepoint (REF-P10) is the named floor; here a
//! deterministic synthetic resolver models exactly its `Projection | Tombstone` contract so the leak
//! PROPERTY is proven structurally (the SAME pattern Notif's own NOTIF-D4 tests use).

use std::sync::Mutex;

use myelin_chat::glue::{chat_hitl_card_facets, chat_humanise_templates, TPL_CHAT_CARD};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone,
    TombstoneReason, DEFAULT_LOCALE,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

/// The confidential chat subject whose TITLE must never leak (a private channel).
fn confidential_channel() -> ArtifactRef {
    ArtifactRef("myelin://acme/chat/channel/board-secret".into())
}
/// The secret channel title a denied viewer must NEVER see (the leak-test payload).
const SECRET_TITLE: &str = "#board-leadership-comp";

/// A deterministic synthetic Refs resolve chokepoint (REF-P10 floor) — per (viewer, ref) it returns
/// a projection (allowed) or a tombstone (denied), the SAME shape the real chokepoint returns.
#[derive(Default)]
struct SyntheticResolver {
    allowed: Mutex<Vec<(String, String)>>,
}
impl SyntheticResolver {
    fn allow(&self, viewer_id: &str, ref_: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((viewer_id.into(), ref_.0.clone()));
    }
}
impl RefResolvePort for SyntheticResolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        let allowed = self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|(v, r)| v == &viewer.principal_id.0 && r == &ref_.0);
        if allowed {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: SECRET_TITLE.into(),
                icon: "channel".into(),
            })
        } else {
            // DENIED → a tombstone carrying NO title (the leak-free chokepoint).
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

/// The store with chat's REGISTERED humanise keys (chat registers; Notif owns the surface).
fn store_with_chat_keys() -> TemplateStore {
    let mut store = TemplateStore::with_platform_defaults();
    for row in chat_humanise_templates() {
        store.put(row);
    }
    store
}

/// **CHAT-D5 — a confidential chat subject humanises to a TOMBSTONE for a denied viewer; 0 title
/// leak.** The chat `@mention`/unfurl of a private channel resolves per-viewer; the intruder sees the
/// tombstone display, NEVER the secret title (re-confirms NOTIF-D4 at the Chat seam). Threshold 0.
#[test]
fn chat_d5_confidential_unfurl_tombstones_zero_title_leak() {
    let resolver = SyntheticResolver::default(); // nobody allowed → every viewer denied.
    let subject = confidential_channel();
    let store = store_with_chat_keys();

    // an intruder is notified of a chat mention of the confidential channel — humanise the card
    // SUBJECT line (the ONLY per-viewer ref slot; the facets line is separate, PII-free).
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &store,
        TPL_CHAT_CARD,
        std::slice::from_ref(&subject),
        &viewer("intruder"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );

    // 0 LEAK: the secret channel title is absent from EVERY field of the output.
    assert!(
        !h.text.contains(SECRET_TITLE)
            && !h.text.contains("leadership")
            && !h.text.contains("comp"),
        "CHAT-D5: the title must NEVER appear for a denied viewer, got text=`{}`",
        h.text
    );
    // the subject slot renders the PII-free tombstone display (a restricted channel).
    assert!(
        h.text.contains("a restricted channel"),
        "the denied subject slot renders the tombstone display, got `{}`",
        h.text
    );
    // no click-route link to a denied ref (never leak a route to a confidential channel).
    assert!(
        h.links.is_empty(),
        "a denied ref yields no link, got {:?}",
        h.links
    );
}

/// **The complement — the ALLOWED approver DOES see the channel title (not vacuously blank).** The
/// card is leak-free for the denied viewer AND useful for the permitted one: the SAME card, the SAME
/// surface, resolved per-viewer.
#[test]
fn chat_d5_allowed_approver_sees_the_title() {
    let resolver = SyntheticResolver::default();
    let subject = confidential_channel();
    resolver.allow("approver", &subject);
    let store = store_with_chat_keys();

    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &store,
        TPL_CHAT_CARD,
        std::slice::from_ref(&subject),
        &viewer("approver"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_TITLE),
        "the permitted approver DOES see the channel title, got `{}`",
        h.text
    );
    // an allowed projection yields a click-route link (the complement of the denied case).
    assert_eq!(
        h.links,
        vec![subject.0.clone()],
        "allowed ref yields its link"
    );
}

/// **The HITL approval card surfaces ACTION + RISK + COST through the ONE templating surface
/// (refined §1.4 / NOTIF-P9), per-viewer-safe.** The card is two composed lines from the ONE surface:
/// the per-viewer SUBJECT line (`humanise` — tombstone on deny, NOTIF-D4) and the PII-free FACETS
/// line (`chat_hitl_card_facets` via the ONE formatter — action/risk/cost, never ref-resolved). The
/// human approver sees WHAT action at WHAT risk for WHAT cost while the confidential title never leaks.
#[test]
fn hitl_card_surfaces_action_risk_cost_and_is_leak_safe() {
    let resolver = SyntheticResolver::default(); // subject denied.
    let store = store_with_chat_keys();

    // the SUBJECT line for the denied intruder — tombstones (0 title leak).
    let subject_line = humanise(
        &resolver,
        &tenant(),
        &region(),
        &store,
        TPL_CHAT_CARD,
        &[confidential_channel()],
        &viewer("intruder"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    // the FACETS line — PII-free action/risk/cost through the ONE formatter (not ref-resolved).
    let facets = chat_hitl_card_facets(&store, "archive-channel", "irreversible", "0.10 USD");
    let card = format!("{} — {}", subject_line.text, facets);

    // the three HITL facets render (the human knows action + risk + cost)...
    assert!(card.contains("archive-channel"), "action renders: `{card}`");
    assert!(card.contains("irreversible"), "risk renders: `{card}`");
    assert!(card.contains("0.10 USD"), "cost renders: `{card}`");
    // ...AND the confidential subject title still does NOT leak (the subject slot tombstoned).
    assert!(
        card.contains("a restricted channel") && !card.contains(SECRET_TITLE),
        "the HITL card is leak-safe in the subject slot: `{card}`"
    );
}
