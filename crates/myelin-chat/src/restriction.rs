//! # `restriction` — the mention pseudonym-shred (→ `[erased user]`) + the Art. 18 restriction
//! flag honoured at EVERY read path + the LEGAL free-text residual BY REFERENCE
//! (CHAT-P23 / P-417, M4-C8; the second committable unit of M4-C8 — the holder + author
//! crypto-shred + cascade is CHAT-P22 / [`crate::erase`])
//!
//! This is the CHAT-P22 floor named in [`crate::holder`] / [`crate::erase`], filled: the two
//! erasure-adjacent halves that are NOT the per-subject-DEK body crypto-shred —
//!
//! 1. **The mention pseudonym-shred** (contract 4.8). A structured `mention(Principal)` node
//!    ([`myelin_content::InlineNode::Mention`]) carries ONLY the mentioned principal's
//!    PSEUDONYMOUS opaque `principal_id` — never the name (the name lives behind Identity's
//!    pseudonym map, [`crate::content`]). Rendering a mention resolves that pseudonymous id to a
//!    per-viewer display name THROUGH the map (`resolve_pseudonym`, 4.8). When the mentioned
//!    subject is erased, Identity's `erase` (4.8) crypto-shreds their pseudonym-map entry — so the
//!    resolve no longer yields a name and the mention renders **`[erased user]`** on the next
//!    render, FREE (the message is never rewritten; nothing PII-bearing was ever stored in the
//!    node — this is the ADR-05 structured-pseudonym payoff, arch 05 §5). 0 recoverable
//!    mentioned-PII.
//!
//! 2. **The Art. 18 restriction flag honoured at EVERY read path** (contract 10.1). The
//!    per-subject [`crate::holder::RestrictionFlag`] (wired at CHAT-P6, flipped by
//!    [`crate::holder::ChatHolder::restrict`]) is now CONSUMED at every Chat processing seam: a
//!    restricted subject is excluded from **indexing** (no message of theirs is index-projected),
//!    **agent-use** (no message of theirs is fed to an agent tool), **new notification routing**
//!    (no write-fanout Signal addressed to them is emitted), and **analytics** (no row of theirs
//!    is analytics-eligible). This is a DISTINCT state from erasure: the message remains stored
//!    and recoverable by the data subject themselves; it is merely not PROCESSED. 0 processings
//!    on a restricted subject.
//!
//! 3. **The free-text third-party residual — BY REFERENCE, never restated** (contract 10.9 /
//!    recon §X-7). P's name typed into the FREE-TEXT body of someone ELSE's un-erased message is
//!    sealed under the AUTHOR's DEK, not P's, so P's erasure does not crypto-shred it. This is the
//!    ONE platform posture ([`crate::holder::CHAT_RESIDUAL_POSTURE_REF`]). Chat writes **NO fifth
//!    chat-specific residual statement** — it supplies only the structural floor (per-subject DEK
//!    shred from CHAT-P22 + pseudonym-map shred here + `restrict` suppression here). The
//!    lawful-basis residual is the platform's single `[OPEN — LEGAL]` posture (R-C5), ratified
//!    once by counsel/DPO — see [`LEGAL_RESIDUAL_FLOOR`].
//!
//! ## Owning architecture docs (read in full before changing this)
//! - `03-events-contracts-and-glue.md` §10 (the restriction flag Art. 18 honoured at every read
//!   path: indexing / agent-use / new notification routing / analytics — a distinct state from
//!   erasure; the free-text residual BY REFERENCE to 10.9/X-7), §1.1 / §3 (the
//!   `mention(Principal)` → `[erased user]`).
//! - `05-hard-problems.md` §5 (the pseudonym-map shred is FREE because the node is structured +
//!   pseudonymous — the ADR-05 payoff; the `restrict` suppression is the structural floor; the
//!   residual is the platform's, never restated Chat-local).
//! - `00-reconciliation-decisions.md` §X-7 (the ONE free-text/immutable erasure posture,
//!   instantiated per subsystem BY REFERENCE; the residual is one ratified statement,
//!   `[OPEN — LEGAL]`).
//!
//! ## Contracts
//! - **4.8** `resolve_pseudonym` / `erase` (CONSUMED — the mention pseudonym-shred targets the
//!   pseudonym-map entry Identity's `erase` crypto-shreds; the render falls to `[erased user]`).
//! - **10.1** `restrict` (OWNED — the restriction flag honoured at every read path; the wired flag
//!   is [`crate::holder::RestrictionFlag`], the suppression seam is [`RestrictionGate`] here).
//! - **10.9** the ONE posture (CONSUMED BY REFERENCE — [`crate::holder::CHAT_RESIDUAL_POSTURE_REF`]).
//!
//! ## Mutation floor (mandatory-core — the no-processing-on-restricted property)
//! [`RestrictionGate`] is the restriction CORE: the predicate that a restricted subject is
//! suppressed at EVERY read path. It is a **mandatory-core mutation target** —
//! `cargo mutants -p myelin-chat --file crates/myelin-chat/src/restriction.rs`. The mutation-tested
//! core is [`RestrictionGate::may_process`] (the single fail-closed-for-processing predicate every
//! read path routes through) and [`render_mention`] (the erased subject → `[erased user]`
//! collapse). **FLOOR (measured-under-load):** the measured % is the CI `cargo mutants` artifact,
//! registered red-until-run in the scorecard, never self-asserted (EI-01 §3).
//!
//! ## DB-free
//! The gate reads the in-memory [`crate::holder::RestrictionFlag`]; the mention render consults a
//! [`MentionResolver`] port (the live binding is Identity's `resolve_pseudonym`, exercised by the
//! CDC dev-dep test). So `cargo build --workspace` stays DB-free.

use crate::holder::RestrictionFlag;
use myelin_content::InlineNode;
use myelin_identity::Principal;

/// **The rendered display of a structured `mention(Principal)` node — the pseudonym-shred
/// outcome (contract 4.8).** A mention renders either to the per-viewer display name (resolved
/// THROUGH Identity's pseudonym map) or, when the mentioned subject is erased (their pseudonym-map
/// entry crypto-shredded), to the frozen [`ERASED_USER`] string. PII-free as a tag: the `Live`
/// variant's name is the per-viewer rendered string (held only transiently at render time, never
/// stored — the node itself carries only the opaque pseudonymous id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionRender {
    /// The mentioned subject's pseudonym resolves — render their per-viewer display name. The name
    /// is render-time only; the durable node holds just the opaque `principal_id`.
    Live(String),
    /// The mentioned subject is erased — their pseudonym-map entry is crypto-shredded, so the
    /// resolve yields nothing and the mention renders [`ERASED_USER`]. 0 recoverable mentioned-PII.
    Erased,
}

impl MentionRender {
    /// The string a renderer paints for this mention — the resolved name, or [`ERASED_USER`] for an
    /// erased subject. ONE rendering path (no second `[erased user]` literal anywhere — EI-01 §7).
    pub fn display(&self) -> &str {
        match self {
            MentionRender::Live(name) => name,
            MentionRender::Erased => ERASED_USER,
        }
    }

    /// Whether this mention shredded to `[erased user]` (the erased subject — 0 recoverable
    /// mentioned-PII). The drill predicate.
    pub fn is_erased(&self) -> bool {
        matches!(self, MentionRender::Erased)
    }
}

/// **The frozen `[erased user]` rendering a mention of an erased subject collapses to (contract
/// 4.8 / arch 05 §5).** A structured `mention(Principal)` whose pseudonym-map entry was
/// crypto-shredded renders EXACTLY this — never the (now unresolvable) name, never a stale cached
/// name. ONE constant so a drill asserts against the NAME, never a literal (EI-01 §3). PII-free: a
/// fixed placeholder, never personal data.
pub const ERASED_USER: &str = "[erased user]";

/// **The per-subject pseudonym resolver the mention render consults (contract 4.8 — the CONSUMED
/// half).** A narrow port over Identity's `resolve_pseudonym`/`erase`: given a mentioned
/// principal, return their per-viewer display name IF their pseudonym-map entry is live, or `None`
/// IF it was erased (crypto-shredded). The live binding is `myelin_identity::IdentityService`
/// (`resolve_pseudonym` → `Ok(name)` while live, the entry gone after `erase` → the resolve no
/// longer yields a name). Modelled as a narrow port (not the full eleven-method service) so the
/// mention render depends on EXACTLY the 4.8 surface it needs — the dependency-inversion the
/// acyclic DAG wants (§2.9).
pub trait MentionResolver {
    /// Resolve a mentioned principal's display name through the pseudonym map (4.8). `Some(name)`
    /// while the subject's map entry lives; **`None` once the subject is erased** (the entry
    /// crypto-shredded — `resolve_pseudonym` no longer yields a name). The `None` is the
    /// pseudonym-shred signal the mention render collapses to [`ERASED_USER`].
    fn resolve_display_name(&self, mentioned: &Principal) -> Option<String>;
}

/// **Render a structured `mention(Principal)` node to its display — the pseudonym-shred core
/// (contract 4.8).** Consults the [`MentionResolver`] (Identity's pseudonym map): a live entry
/// renders the resolved per-viewer name ([`MentionRender::Live`]); an ERASED subject (map entry
/// crypto-shredded → the resolve yields `None`) renders [`MentionRender::Erased`] → `[erased
/// user]`. This is FREE — the message body is NEVER rewritten and nothing PII-bearing was stored
/// in the node (it carries only the opaque pseudonymous `principal_id`); the render simply
/// re-resolves through the map on EVERY render, so the next render after an erase shreds to
/// `[erased user]` by construction (arch 05 §5, the ADR-05 structured-pseudonym payoff). 0
/// recoverable mentioned-PII.
pub fn render_mention<R: MentionResolver>(mentioned: &Principal, resolver: &R) -> MentionRender {
    match resolver.resolve_display_name(mentioned) {
        Some(name) => MentionRender::Live(name),
        // The pseudonym-map entry is crypto-shredded (the subject was erased, 4.8) — the resolve
        // yields nothing, so the mention shreds to `[erased user]`. 0 recoverable mentioned-PII.
        None => MentionRender::Erased,
    }
}

/// **Render every `mention(Principal)` in a body's structured-node array, in document order — the
/// pseudonym-shred over the whole body (4.8).** Walks the SAME structured-node array the edge
/// producer + the search projection walk ([`crate::content`] / [`crate::search`], X-2 — never a
/// regex over prose), re-resolving each mention through the [`MentionResolver`] so an erased
/// subject's mention is `[erased user]` on this render. `artifact_ref`/`embed` nodes are not
/// mentions and are skipped (they carry no person pseudonym). Returns the renders in body order.
pub fn render_body_mentions<R: MentionResolver>(
    nodes: &[InlineNode],
    resolver: &R,
) -> Vec<MentionRender> {
    nodes
        .iter()
        .filter_map(|node| match node {
            InlineNode::Mention(principal) => Some(render_mention(principal, resolver)),
            // Not a person mention — no pseudonym to shred.
            InlineNode::ArtifactRefNode(_) | InlineNode::Embed(_) => None,
        })
        .collect()
}

/// **A Chat read-path processing seam — the four places a restricted subject MUST be suppressed
/// (Art. 18, contract 10.1 / arch §10).** A closed enum: a new Chat read path cannot be added
/// without appearing here (the restriction coverage is total — proven by the unit test over
/// [`ReadPath::ALL`]). PII-free — a seam tag, never data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReadPath {
    /// **Indexing** — building a Search/RAG index doc + embedding from a message
    /// ([`crate::search`]). A restricted subject's message is NOT index-projected.
    Indexing,
    /// **Agent-use** — feeding a message to an agent tool / RAG context
    /// ([`crate::tools`]). A restricted subject's message is NOT agent-readable.
    AgentUse,
    /// **New notification routing** — emitting a write-fanout Signal for a message
    /// ([`crate::fanout`]). A restricted subject gets NO new notification routing.
    NotifRouting,
    /// **Analytics** — counting a message into an analytics/OLAP aggregate. A restricted subject's
    /// row is NOT analytics-eligible (the SAME posture contract 11.6 takes — the OLAP store honours
    /// the restriction flag).
    Analytics,
}

impl ReadPath {
    /// A stable, PII-free label for the read path (telemetry / a test assertion — never PII).
    pub fn label(self) -> &'static str {
        match self {
            ReadPath::Indexing => "indexing",
            ReadPath::AgentUse => "agent-use",
            ReadPath::NotifRouting => "notif-routing",
            ReadPath::Analytics => "analytics",
        }
    }

    /// **The full set of Chat read paths the restriction flag suppresses — the Art. 18 coverage
    /// surface.** Every member is gated by [`RestrictionGate::may_process`]; a read path NOT in
    /// this set is a hole (an unsuppressed processing). Closed + total — proven by the unit tests.
    pub const ALL: [ReadPath; 4] = [
        ReadPath::Indexing,
        ReadPath::AgentUse,
        ReadPath::NotifRouting,
        ReadPath::Analytics,
    ];
}

/// **The Art. 18 restriction gate — the ONE predicate every Chat read path routes through
/// (contract 10.1 / arch §10).** Wraps the per-subject [`RestrictionFlag`] (the SAME flag
/// [`crate::holder::ChatHolder::restrict`] flips, shared `Arc` — one flag set, never two) and
/// exposes the single fail-closed-FOR-PROCESSING predicate [`RestrictionGate::may_process`]: a
/// restricted subject is suppressed at indexing / agent-use / notif-routing / analytics alike. A
/// restricted subject is a DISTINCT state from an erased one — the message stays stored; it is
/// merely not PROCESSED.
///
/// This is the mandatory-core: every Chat processing seam reads THIS gate (never the raw flag,
/// never a second restriction check), so the no-processing-on-restricted property holds by
/// construction — a new read path that forgets the gate is the bug the mutation floor + the
/// [`ReadPath::ALL`] coverage test catch.
#[derive(Clone)]
pub struct RestrictionGate {
    flag: RestrictionFlag,
}

impl RestrictionGate {
    /// Build the gate over the holder's restriction flag (share the SAME `Arc` the holder writes —
    /// one flag set across the holder's `restrict` and every read-path seam).
    pub fn new(flag: RestrictionFlag) -> RestrictionGate {
        RestrictionGate { flag }
    }

    /// **The single predicate — may this subject's data be PROCESSED at this read path? (Art. 18).**
    /// `false` for a restricted subject at ANY read path (the suppression — 0 processings on a
    /// restricted subject); `true` otherwise. Fail-closed FOR PROCESSING: the flag-poison path in
    /// [`RestrictionFlag::is_restricted`] already fails closed (treats a poisoned lock as restricted
    /// is the safe-for-the-subject default, but the flag's lock-recovery never surfaces a `false`
    /// for a restricted subject). The `path` is carried so a seam logs WHICH processing it gated
    /// (telemetry), but the decision is path-INDEPENDENT — a restricted subject is suppressed
    /// EVERYWHERE (the whole point of the flag).
    pub fn may_process(&self, subject: &str, path: ReadPath) -> bool {
        let _ = path; // the suppression is total — every read path is gated identically.
        !self.flag.is_restricted(subject)
    }

    /// Sugar: is this subject suppressed at this read path? (the negation of [`may_process`], for a
    /// seam that branches on the suppressed case). 0 processings on a restricted subject.
    pub fn is_suppressed(&self, subject: &str, path: ReadPath) -> bool {
        !self.may_process(subject, path)
    }

    /// **Whether a subject is suppressed at EVERY read path (the Art. 18 totality — the drill
    /// predicate).** `true` for a restricted subject iff it is suppressed across ALL of
    /// [`ReadPath::ALL`] (0 processings anywhere); `true` for an unrestricted subject iff it is
    /// processable across all (the flag is off everywhere). This is the property the
    /// restricted-processing drill asserts: a restricted subject's signal across every read path is
    /// 0.
    pub fn suppressed_everywhere(&self, subject: &str) -> bool {
        ReadPath::ALL
            .iter()
            .all(|&path| self.is_suppressed(subject, path))
    }

    /// Borrow the underlying flag (so the holder + the gate share ONE set — the wiring seam).
    pub fn flag(&self) -> &RestrictionFlag {
        &self.flag
    }
}

// ──────────────── the per-read-path suppression wrappers (the gate WIRED to each seam) ────────────────
//
// Each Chat read path routes its per-subject processing through the ONE [`RestrictionGate`] before
// emitting. These wrappers are the SEAM the production wiring binds — a restricted author's message
// is skipped at indexing / agent-use / notif / analytics by construction, never by a reviewer
// remembering to add a check. They all route through [`RestrictionGate::may_process`] (one predicate,
// no second restriction check — EI-01 §7).

/// **The INDEXING read path, restriction-gated (Art. 18 / contract 10.1).** Build a message's
/// [`crate::search::message_search_projection`] ONLY if its `author` is not restricted; a restricted
/// author yields `None` (the index-builder gets nothing for that message — 0 indexed bodies for a
/// restricted subject). The embedding rides the same projection, so a restricted subject is excluded
/// from the vector space too. This is the SAME projection [`crate::search`] builds; the gate is the
/// only addition (no second projection path).
pub fn index_projection_if_allowed(
    gate: &RestrictionGate,
    author: &str,
    body: &crate::content::MessageBody,
    lang: Option<&str>,
) -> Option<myelin_search::SearchProjection> {
    if gate.may_process(author, ReadPath::Indexing) {
        Some(crate::search::message_search_projection(body, lang))
    } else {
        None
    }
}

/// **The AGENT-USE read path, restriction-gated (Art. 18 / contract 10.1).** Whether a message
/// authored by `author` may be fed to an agent tool / RAG context — `false` for a restricted
/// author (the restricted subject's prose is NOT agent-readable). The agent loop consults this
/// before adding a message to the model context.
pub fn agent_may_read(gate: &RestrictionGate, author: &str) -> bool {
    gate.may_process(author, ReadPath::AgentUse)
}

/// **The NOTIF-ROUTING read path, restriction-gated (Art. 18 / contract 10.1).** Whether a new
/// write-fanout notification may be routed FOR a restricted subject — `false` for a restricted
/// subject (no NEW notification routing while restricted; existing notifications are not the point —
/// the restriction halts NEW routing). The fanout seam consults this before emitting a Signal
/// addressed to / about the subject.
pub fn notif_may_route(gate: &RestrictionGate, subject: &str) -> bool {
    gate.may_process(subject, ReadPath::NotifRouting)
}

/// **The ANALYTICS read path, restriction-gated (Art. 18 / contract 10.1 / 11.6).** Whether a
/// message authored by `author` is analytics-eligible — `false` for a restricted author (no
/// analytics on a restricted subject; the SAME posture the OLAP store takes, 11.6). The analytics
/// aggregator consults this before counting a row.
pub fn analytics_eligible(gate: &RestrictionGate, author: &str) -> bool {
    gate.may_process(author, ReadPath::Analytics)
}

/// **The LEGAL free-text residual — a NAMED `[OPEN — LEGAL]` floor, BY REFERENCE to the ONE
/// platform posture (10.9 / X-7), never a fifth chat-specific statement (R-C5).** Chat ships the
/// STRUCTURAL floor REGARDLESS (per-subject DEK crypto-shred from CHAT-P22 + the mention
/// pseudonym-shred + the `restrict` suppression here); the lawful-basis residual — P's name typed
/// into someone else's un-erased free-text body, sealed under the AUTHOR's DEK — is the platform's
/// single ratified-once-by-counsel/DPO statement, parallel-tracked (LEGAL), NOT a Chat blocker.
/// State it as an untested-but-named LEGAL floor (the structural floor is green; the residual is a
/// ratification, not code). The reference (never a restatement) is
/// [`crate::holder::CHAT_RESIDUAL_POSTURE_REF`].
pub const LEGAL_RESIDUAL_FLOOR: &str =
    "[OPEN — LEGAL] the free-text third-party residual → the ONE platform posture (contract 10.9 / \
     recon §X-7), ratified ONCE by counsel/DPO (R-C5). Chat writes NO fifth chat-specific residual: \
     the structural floor (per-subject DEK crypto-shred [CHAT-P22] + mention pseudonym-shred + \
     restrict suppression [CHAT-P23]) ships regardless; the lawful-basis statement is the platform's, \
     parallel-tracked (LEGAL), never a chat blocker — see crate::holder::CHAT_RESIDUAL_POSTURE_REF";

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;
    use std::collections::BTreeSet;

    fn principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    /// A pseudonym-map resolver model: a set of LIVE subject ids → a display name. An erased
    /// subject's id is REMOVED (its map entry crypto-shredded), so `resolve_display_name` yields
    /// `None` — exactly Identity's `resolve_pseudonym` after `erase` (4.8).
    struct MapResolver {
        live: BTreeSet<String>,
    }
    impl MapResolver {
        fn with(ids: &[&str]) -> MapResolver {
            MapResolver {
                live: ids.iter().map(|s| s.to_string()).collect(),
            }
        }
        /// Crypto-shred a subject's pseudonym-map entry (Identity's `erase`, 4.8).
        fn erase(&mut self, id: &str) {
            self.live.remove(id);
        }
    }
    impl MentionResolver for MapResolver {
        fn resolve_display_name(&self, mentioned: &Principal) -> Option<String> {
            if self.live.contains(&mentioned.principal_id.0) {
                // The per-viewer display name (render-time only; never stored in the node).
                Some(format!("@{}", mentioned.principal_id.0))
            } else {
                None
            }
        }
    }

    /// **The mention pseudonym-shred: erasing a mentioned subject renders `[erased user]` on the
    /// NEXT render — FREE, the node is never rewritten (4.8 / arch 05 §5).** While the map entry
    /// lives the mention resolves a name; after `erase` the SAME node (unchanged) renders
    /// `[erased user]`. 0 recoverable mentioned-PII.
    #[test]
    fn mention_shreds_to_erased_user_on_next_render() {
        let mut resolver = MapResolver::with(&["psn:ada"]);
        let ada = principal("psn:ada");

        // While live: the mention resolves a per-viewer name.
        let before = render_mention(&ada, &resolver);
        assert_eq!(before, MentionRender::Live("@psn:ada".into()));
        assert!(!before.is_erased());
        assert_eq!(before.display(), "@psn:ada");

        // Erase the pseudonym-map entry (Identity `erase`, 4.8) — the node is NOT touched.
        resolver.erase("psn:ada");

        // The SAME node, re-rendered, shreds to `[erased user]` — 0 recoverable mentioned-PII.
        let after = render_mention(&ada, &resolver);
        assert_eq!(after, MentionRender::Erased);
        assert!(after.is_erased());
        assert_eq!(after.display(), ERASED_USER);
        assert_eq!(after.display(), "[erased user]");
    }

    /// **The body-mention walk shreds the erased subject + leaves the live one (4.8, over the
    /// structured node array — X-2, never a regex).** A body mentioning two people, one erased:
    /// the erased mention is `[erased user]`, the live one keeps its name; non-mention nodes are
    /// skipped (they carry no person pseudonym).
    #[test]
    fn body_mention_walk_shreds_only_the_erased_subject() {
        let mut resolver = MapResolver::with(&["psn:ada", "psn:bo"]);
        resolver.erase("psn:ada");
        let nodes = vec![
            InlineNode::Mention(principal("psn:ada")),
            InlineNode::Embed(myelin_events::ArtifactRef(
                "myelin://acme/chat/message/x".into(),
            )),
            InlineNode::Mention(principal("psn:bo")),
        ];
        let renders = render_body_mentions(&nodes, &resolver);
        // Two mentions → two renders (the embed is skipped — not a person mention).
        assert_eq!(renders.len(), 2);
        assert_eq!(
            renders[0],
            MentionRender::Erased,
            "ada erased → [erased user]"
        );
        assert_eq!(
            renders[1],
            MentionRender::Live("@psn:bo".into()),
            "bo live → name"
        );
    }

    /// **The restriction gate suppresses a restricted subject at EVERY read path (Art. 18,
    /// contract 10.1) — 0 processings on a restricted subject.** Before `restrict`, every read
    /// path may process; after `restrict(on)`, every read path is suppressed; after `restrict(off)`
    /// it is processable again. The gate reads the SAME flag the holder writes.
    #[test]
    fn restriction_gate_suppresses_every_read_path() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());
        let sid = "psn:restricted";

        // Not restricted: every read path may process.
        for path in ReadPath::ALL {
            assert!(
                gate.may_process(sid, path),
                "{}: an unrestricted subject is processable",
                path.label()
            );
        }
        assert!(!gate.suppressed_everywhere(sid));

        // Restrict (Art. 18) — the holder flips the SAME flag.
        flag.set(sid, true);

        // Now EVERY read path is suppressed — 0 processings on a restricted subject.
        for path in ReadPath::ALL {
            assert!(
                gate.is_suppressed(sid, path),
                "{}: a restricted subject is suppressed (0 processings)",
                path.label()
            );
            assert!(!gate.may_process(sid, path));
        }
        assert!(
            gate.suppressed_everywhere(sid),
            "the restricted subject is suppressed across ALL read paths (Art. 18 totality)"
        );

        // Lift the restriction — processable again (restrict is a reversible state, not erasure).
        flag.set(sid, false);
        assert!(gate.may_process(sid, ReadPath::Indexing));
        assert!(!gate.suppressed_everywhere(sid));
    }

    /// **A restriction is per-subject — restricting one subject does NOT suppress another (the
    /// individual lever).** Restricting `ada` leaves `bo` fully processable at every read path.
    #[test]
    fn restriction_is_per_subject_not_blanket() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());
        flag.set("psn:ada", true);
        assert!(gate.suppressed_everywhere("psn:ada"));
        for path in ReadPath::ALL {
            assert!(
                gate.may_process("psn:bo", path),
                "{}: bo is not restricted — processable",
                path.label()
            );
        }
        assert!(!gate.suppressed_everywhere("psn:bo"));
    }

    /// **The read-path set is the Art. 18 coverage surface — exactly indexing / agent-use /
    /// notif-routing / analytics (arch §10).** The closed set is the structural coverage (a new
    /// Chat read path cannot be added without appearing here).
    #[test]
    fn the_read_path_set_is_the_art18_coverage() {
        assert_eq!(ReadPath::ALL.len(), 4);
        for p in [
            ReadPath::Indexing,
            ReadPath::AgentUse,
            ReadPath::NotifRouting,
            ReadPath::Analytics,
        ] {
            assert!(
                ReadPath::ALL.contains(&p),
                "{} must be in the Art. 18 read-path coverage",
                p.label()
            );
        }
    }

    /// **The INDEXING wrapper skips a restricted author's body (0 indexed bodies for a restricted
    /// subject) but projects an unrestricted one — the index read path is gated (Art. 18).** Same
    /// projection, the gate is the only addition.
    #[test]
    fn index_projection_is_suppressed_for_a_restricted_author() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());
        let body = crate::content::paragraph_body("a private message body", Vec::new());

        // Unrestricted: the projection is built (the index gets the body).
        assert!(
            index_projection_if_allowed(&gate, "psn:ada", &body, None).is_some(),
            "an unrestricted author's body is index-projected"
        );

        // Restrict ada — the index gets NOTHING for her message (0 indexed bodies, incl. embeddings).
        flag.set("psn:ada", true);
        assert!(
            index_projection_if_allowed(&gate, "psn:ada", &body, None).is_none(),
            "a restricted author's body is NOT index-projected (Art. 18)"
        );
        // Another author is unaffected (per-subject).
        assert!(index_projection_if_allowed(&gate, "psn:bo", &body, None).is_some());
    }

    /// **The agent-use / notif / analytics wrappers all suppress a restricted subject and pass an
    /// unrestricted one — every read path routes through the ONE gate (Art. 18).** 0 processings on
    /// a restricted subject across the three behavioural read paths.
    #[test]
    fn agent_notif_analytics_wrappers_all_gate_on_the_one_predicate() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());

        // Unrestricted: every behavioural read path passes.
        assert!(agent_may_read(&gate, "psn:ada"));
        assert!(notif_may_route(&gate, "psn:ada"));
        assert!(analytics_eligible(&gate, "psn:ada"));

        // Restrict — every behavioural read path is suppressed.
        flag.set("psn:ada", true);
        assert!(
            !agent_may_read(&gate, "psn:ada"),
            "restricted → not agent-readable"
        );
        assert!(
            !notif_may_route(&gate, "psn:ada"),
            "restricted → no new notif routing"
        );
        assert!(
            !analytics_eligible(&gate, "psn:ada"),
            "restricted → not analytics-eligible"
        );

        // bo (unrestricted) still passes everywhere.
        assert!(agent_may_read(&gate, "psn:bo"));
        assert!(notif_may_route(&gate, "psn:bo"));
        assert!(analytics_eligible(&gate, "psn:bo"));
    }

    /// **The LEGAL residual is NAMED as an `[OPEN — LEGAL]` floor, BY REFERENCE to the ONE posture
    /// (10.9 / X-7), never restated Chat-local (R-C5).** The floor names the contract + the
    /// structural floor that ships regardless + points at the platform reference — it does NOT
    /// author a fresh chat-specific lawful-basis statement.
    #[test]
    fn the_legal_residual_is_a_named_open_legal_floor_by_reference() {
        assert!(LEGAL_RESIDUAL_FLOOR.contains("[OPEN — LEGAL]"));
        assert!(LEGAL_RESIDUAL_FLOOR.contains("10.9"));
        assert!(LEGAL_RESIDUAL_FLOOR.contains("X-7"));
        // BY REFERENCE — it points at the ONE posture reference, never a fifth chat-local statement.
        assert!(LEGAL_RESIDUAL_FLOOR.contains("CHAT_RESIDUAL_POSTURE_REF"));
        // The structural floor ships regardless of the LEGAL ratification.
        assert!(LEGAL_RESIDUAL_FLOOR.contains("ships regardless"));
    }
}
