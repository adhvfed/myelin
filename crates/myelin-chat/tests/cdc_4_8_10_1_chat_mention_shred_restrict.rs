//! # CDC pair — contract 4.8 (the mention pseudonym-shred → `[erased user]`) + 10.1 (the Art. 18
//! restriction flag honoured at EVERY read path) for the Chat mention-shred + restriction
//! suppression (CHAT-P23 / P-417, M4-C8 — the second committable unit of M4-C8)
//!
//! **The two contract halves this artifact proves (the prompt's GATE):**
//! - **4.8 — the mention pseudonym-shred.** PROVIDER: a structured `mention(Principal)` node carries
//!   only the pseudonymous opaque id; [`myelin_chat::render_mention`] resolves it THROUGH the
//!   pseudonym map ([`MentionResolver`], the 4.8 `resolve_pseudonym` consumer). CONSUMER: a renderer
//!   that, after the mentioned subject's pseudonym-map entry is crypto-shredded (Identity `erase`,
//!   4.8), sees the SAME (unchanged) node render `[erased user]` — 0 recoverable mentioned-PII, FREE
//!   (the message body is never rewritten).
//! - **10.1 — the restriction flag at every read path.** PROVIDER: [`myelin_chat::RestrictionGate`]
//!   reads the SAME per-subject flag [`myelin_chat::ChatHolder::restrict`] flips. CONSUMER: every Chat
//!   read path (indexing / agent-use / notif-routing / analytics) routes through the ONE
//!   `may_process` predicate — a restricted subject is suppressed across ALL of them (0 processings
//!   on a restricted subject), a distinct state from erasure.
//!
//! The provider + consumer are the SAME frozen shapes (one pseudonym-map resolve, one restriction
//! flag, one gate — EI-01 §7), proven DB-free. The third-party free-text residual is the ONE platform
//! posture (10.9 / X-7), BY REFERENCE — see [`myelin_chat::LEGAL_RESIDUAL_FLOOR`].

use myelin_chat::{
    agent_may_read, analytics_eligible, index_projection_if_allowed, notif_may_route,
    paragraph_body, render_mention, ChatHolder, MentionRender, MentionResolver, ReadPath,
    RestrictionGate, ERASED_USER,
};
use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;
use std::collections::BTreeSet;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}
fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        GdprTenantId::from_token("acme"),
    ))
}

/// The 4.8 pseudonym-map: live subject ids → a per-viewer display name. `erase` REMOVES the entry
/// (Identity's crypto-shred), so a resolve of an erased subject yields `None` — exactly
/// `resolve_pseudonym` after `erase`.
struct PseudonymMap {
    live: BTreeSet<String>,
}
impl PseudonymMap {
    fn with(ids: &[&str]) -> PseudonymMap {
        PseudonymMap {
            live: ids.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn erase(&mut self, id: &str) {
        self.live.remove(id);
    }
}
impl MentionResolver for PseudonymMap {
    fn resolve_display_name(&self, mentioned: &Principal) -> Option<String> {
        self.live
            .contains(&mentioned.principal_id.0)
            .then(|| format!("@{}", mentioned.principal_id.0))
    }
}

// ───────────────────────────── 4.8 — the mention pseudonym-shred ─────────────────────────────

/// **PROVIDER (4.8): the mention resolves through the pseudonym map.** CONSUMER: after the mentioned
/// subject's map entry is crypto-shredded (Identity `erase`), the SAME node renders `[erased user]`
/// — 0 recoverable mentioned-PII, FREE (the body is never rewritten).
#[test]
fn cdc_4_8_provider_consumer_mention_shreds_to_erased_user() {
    let mut map = PseudonymMap::with(&["psn:ada"]);
    let ada = principal("psn:ada");

    // PROVIDER: while the map entry lives, the mention resolves a name.
    assert_eq!(
        render_mention(&ada, &map),
        MentionRender::Live("@psn:ada".into())
    );

    // The DSR erase crypto-shreds the pseudonym-map entry (4.8). The mention node is NOT touched.
    map.erase("psn:ada");

    // CONSUMER: the SAME node now renders `[erased user]` — 0 recoverable mentioned-PII.
    let after = render_mention(&ada, &map);
    assert_eq!(after, MentionRender::Erased);
    assert_eq!(after.display(), ERASED_USER);
}

/// **The mention-shred is FREE — the structured node carries no name (4.8 / arch 05 §5).** Two
/// people co-mentioned; erasing one shreds ONLY that mention's render, leaving the live co-mentioned
/// subject's name. The mention nodes are never rewritten — the shred is in the render, re-resolved
/// against the post-erase map.
#[test]
fn cdc_4_8_mention_shred_is_free_body_never_rewritten() {
    let mut map = PseudonymMap::with(&["psn:ada", "psn:bo"]);
    let ada = principal("psn:ada");
    let bo = principal("psn:bo");

    // Erase ada only — the pseudonym-map entry is crypto-shredded (4.8).
    map.erase("psn:ada");

    // The (unchanged) ada node shreds to `[erased user]`; bo (live) keeps her name.
    assert_eq!(render_mention(&ada, &map), MentionRender::Erased);
    assert_eq!(
        render_mention(&bo, &map),
        MentionRender::Live("@psn:bo".into())
    );
}

// ───────────────────────────── 10.1 — the restriction flag at every read path ─────────────────────

/// **PROVIDER (10.1): the holder's `restrict` flips the flag; the gate reads the SAME flag.**
/// CONSUMER: every Chat read path (indexing / agent-use / notif / analytics) is suppressed for the
/// restricted subject — 0 processings on a restricted subject (a distinct state from erasure).
#[test]
fn cdc_10_1_provider_consumer_restrict_suppresses_every_read_path() {
    let holder = ChatHolder::new();
    let gate = RestrictionGate::new(holder.restriction().clone());
    let body = paragraph_body("a message", vec![]);

    // Before restrict: every read path processes.
    assert!(index_projection_if_allowed(&gate, "psn:ada", &body, None).is_some());
    assert!(agent_may_read(&gate, "psn:ada"));
    assert!(notif_may_route(&gate, "psn:ada"));
    assert!(analytics_eligible(&gate, "psn:ada"));

    // PROVIDER: the holder restricts the subject (Art. 18) — flips the flag the gate reads.
    holder
        .restrict(&subject("psn:ada"), true)
        .expect("restrict on");

    // CONSUMER: 0 processings on the restricted subject, across EVERY read path.
    assert!(
        index_projection_if_allowed(&gate, "psn:ada", &body, None).is_none(),
        "indexing suppressed"
    );
    assert!(!agent_may_read(&gate, "psn:ada"), "agent-use suppressed");
    assert!(
        !notif_may_route(&gate, "psn:ada"),
        "notif-routing suppressed"
    );
    assert!(
        !analytics_eligible(&gate, "psn:ada"),
        "analytics suppressed"
    );
    assert!(
        gate.suppressed_everywhere("psn:ada"),
        "the restricted subject is suppressed across ALL read paths (Art. 18 totality)"
    );

    // Lifting the restriction restores processing (restrict is reversible — not erasure).
    holder
        .restrict(&subject("psn:ada"), false)
        .expect("restrict off");
    assert!(index_projection_if_allowed(&gate, "psn:ada", &body, None).is_some());
    assert!(!gate.suppressed_everywhere("psn:ada"));
}

/// **The restriction is per-subject — restricting one does NOT suppress another (the individual
/// lever, 10.1).** Restricting `ada` leaves `bo` processable at every read path.
#[test]
fn cdc_10_1_restriction_is_per_subject() {
    let holder = ChatHolder::new();
    let gate = RestrictionGate::new(holder.restriction().clone());
    holder
        .restrict(&subject("psn:ada"), true)
        .expect("restrict ada");
    assert!(gate.suppressed_everywhere("psn:ada"));
    for path in ReadPath::ALL {
        assert!(
            gate.may_process("psn:bo", path),
            "{}: bo unaffected",
            path.label()
        );
    }
}
