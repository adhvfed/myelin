use crate::holder::RestrictionFlag;
use myelin_content::InlineNode;
use myelin_identity::Principal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionRender {
    Live(String),
    Erased,
}

impl MentionRender {
    pub fn display(&self) -> &str {
        match self {
            MentionRender::Live(name) => name,
            MentionRender::Erased => ERASED_USER,
        }
    }

    pub fn is_erased(&self) -> bool {
        matches!(self, MentionRender::Erased)
    }
}

pub const ERASED_USER: &str = "[erased user]";

pub trait MentionResolver {
    fn resolve_display_name(&self, mentioned: &Principal) -> Option<String>;
}

pub fn render_mention<R: MentionResolver>(mentioned: &Principal, resolver: &R) -> MentionRender {
    match resolver.resolve_display_name(mentioned) {
        Some(name) => MentionRender::Live(name),
        None => MentionRender::Erased,
    }
}

pub fn render_body_mentions<R: MentionResolver>(
    nodes: &[InlineNode],
    resolver: &R,
) -> Vec<MentionRender> {
    nodes
        .iter()
        .filter_map(|node| match node {
            InlineNode::Mention(principal) => Some(render_mention(principal, resolver)),
            InlineNode::ArtifactRefNode(_) | InlineNode::Embed(_) => None,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReadPath {
    Indexing,
    AgentUse,
    NotifRouting,
    Analytics,
}

impl ReadPath {
    pub fn label(self) -> &'static str {
        match self {
            ReadPath::Indexing => "indexing",
            ReadPath::AgentUse => "agent-use",
            ReadPath::NotifRouting => "notif-routing",
            ReadPath::Analytics => "analytics",
        }
    }

    pub const ALL: [ReadPath; 4] = [
        ReadPath::Indexing,
        ReadPath::AgentUse,
        ReadPath::NotifRouting,
        ReadPath::Analytics,
    ];
}

#[derive(Clone)]
pub struct RestrictionGate {
    flag: RestrictionFlag,
}

impl RestrictionGate {
    pub fn new(flag: RestrictionFlag) -> RestrictionGate {
        RestrictionGate { flag }
    }

    pub fn may_process(&self, subject: &str, path: ReadPath) -> bool {
        let _ = path;
        !self.flag.is_restricted(subject)
    }

    pub fn is_suppressed(&self, subject: &str, path: ReadPath) -> bool {
        !self.may_process(subject, path)
    }

    pub fn suppressed_everywhere(&self, subject: &str) -> bool {
        ReadPath::ALL
            .iter()
            .all(|&path| self.is_suppressed(subject, path))
    }

    pub fn flag(&self) -> &RestrictionFlag {
        &self.flag
    }
}

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

pub fn agent_may_read(gate: &RestrictionGate, author: &str) -> bool {
    gate.may_process(author, ReadPath::AgentUse)
}

pub fn notif_may_route(gate: &RestrictionGate, subject: &str) -> bool {
    gate.may_process(subject, ReadPath::NotifRouting)
}

pub fn analytics_eligible(gate: &RestrictionGate, author: &str) -> bool {
    gate.may_process(author, ReadPath::Analytics)
}

pub const LEGAL_RESIDUAL_FLOOR: &str =
    "[OPEN - LEGAL] the free-text third-party residual → the ONE platform posture (contract 10.9 / \
     recon §X-7), ratified ONCE by counsel/DPO (R-C5). Chat writes NO fifth chat-specific residual: \
     the structural floor (per-subject DEK crypto-shred [CHAT-P22] + mention pseudonym-shred + \
     restrict suppression [CHAT-P23]) ships regardless; the lawful-basis statement is the platform's, \
     parallel-tracked (LEGAL), never a chat blocker - see crate::holder::CHAT_RESIDUAL_POSTURE_REF";

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

    struct MapResolver {
        live: BTreeSet<String>,
    }
    impl MapResolver {
        fn with(ids: &[&str]) -> MapResolver {
            MapResolver {
                live: ids.iter().map(|s| s.to_string()).collect(),
            }
        }
        fn erase(&mut self, id: &str) {
            self.live.remove(id);
        }
    }
    impl MentionResolver for MapResolver {
        fn resolve_display_name(&self, mentioned: &Principal) -> Option<String> {
            if self.live.contains(&mentioned.principal_id.0) {
                Some(format!("@{}", mentioned.principal_id.0))
            } else {
                None
            }
        }
    }

    #[test]
    fn mention_shreds_to_erased_user_on_next_render() {
        let mut resolver = MapResolver::with(&["psn:ada"]);
        let ada = principal("psn:ada");

        let before = render_mention(&ada, &resolver);
        assert_eq!(before, MentionRender::Live("@psn:ada".into()));
        assert!(!before.is_erased());
        assert_eq!(before.display(), "@psn:ada");

        resolver.erase("psn:ada");

        let after = render_mention(&ada, &resolver);
        assert_eq!(after, MentionRender::Erased);
        assert!(after.is_erased());
        assert_eq!(after.display(), ERASED_USER);
        assert_eq!(after.display(), "[erased user]");
    }

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

    #[test]
    fn restriction_gate_suppresses_every_read_path() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());
        let sid = "psn:restricted";

        for path in ReadPath::ALL {
            assert!(
                gate.may_process(sid, path),
                "{}: an unrestricted subject is processable",
                path.label()
            );
        }
        assert!(!gate.suppressed_everywhere(sid));

        flag.set(sid, true);

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

        flag.set(sid, false);
        assert!(gate.may_process(sid, ReadPath::Indexing));
        assert!(!gate.suppressed_everywhere(sid));
    }

    #[test]
    fn restriction_is_per_subject_not_blanket() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());
        flag.set("psn:ada", true);
        assert!(gate.suppressed_everywhere("psn:ada"));
        for path in ReadPath::ALL {
            assert!(
                gate.may_process("psn:bo", path),
                "{}: bo is not restricted - processable",
                path.label()
            );
        }
        assert!(!gate.suppressed_everywhere("psn:bo"));
    }

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

    #[test]
    fn index_projection_is_suppressed_for_a_restricted_author() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());
        let body = crate::content::paragraph_body("a private message body", Vec::new());

        assert!(
            index_projection_if_allowed(&gate, "psn:ada", &body, None).is_some(),
            "an unrestricted author's body is index-projected"
        );

        flag.set("psn:ada", true);
        assert!(
            index_projection_if_allowed(&gate, "psn:ada", &body, None).is_none(),
            "a restricted author's body is NOT index-projected (Art. 18)"
        );
        assert!(index_projection_if_allowed(&gate, "psn:bo", &body, None).is_some());
    }

    #[test]
    fn agent_notif_analytics_wrappers_all_gate_on_the_one_predicate() {
        let flag = RestrictionFlag::new();
        let gate = RestrictionGate::new(flag.clone());

        assert!(agent_may_read(&gate, "psn:ada"));
        assert!(notif_may_route(&gate, "psn:ada"));
        assert!(analytics_eligible(&gate, "psn:ada"));

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

        assert!(agent_may_read(&gate, "psn:bo"));
        assert!(notif_may_route(&gate, "psn:bo"));
        assert!(analytics_eligible(&gate, "psn:bo"));
    }

    #[test]
    fn the_legal_residual_is_a_named_open_legal_floor_by_reference() {
        assert!(LEGAL_RESIDUAL_FLOOR.contains("[OPEN - LEGAL]"));
        assert!(LEGAL_RESIDUAL_FLOOR.contains("10.9"));
        assert!(LEGAL_RESIDUAL_FLOOR.contains("X-7"));
        assert!(LEGAL_RESIDUAL_FLOOR.contains("CHAT_RESIDUAL_POSTURE_REF"));
        assert!(LEGAL_RESIDUAL_FLOOR.contains("ships regardless"));
    }
}
