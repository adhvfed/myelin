use myelin_events::ArtifactRef;

pub const ISSUE_SUBSYSTEM: &str = "issue";

pub fn issue_root_ref(tenant: &str, key: &str) -> ArtifactRef {
    myelin_refs::parse(&format!("myelin://{tenant}/{ISSUE_SUBSYSTEM}/issue/{key}"))
        .expect("Issues mints a grammatical canonical ArtifactRef")
}

pub use myelin_refs::{REFS_EDGE_CREATED, REL_CLASS_REFERENCE};

pub const REL_CLASS_LIFECYCLE: &str = "lifecycle";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueLifecycleRel {
    Parent,
    Blocks,
    BlockedBy,
    Closes,
    DependsOn,
    Relates,
}

impl IssueLifecycleRel {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueLifecycleRel::Parent => "parent",
            IssueLifecycleRel::Blocks => "blocks",
            IssueLifecycleRel::BlockedBy => "blocked_by",
            IssueLifecycleRel::Closes => "closes",
            IssueLifecycleRel::DependsOn => "depends_on",
            IssueLifecycleRel::Relates => "relates",
        }
    }

    pub fn from_token(token: &str) -> Option<IssueLifecycleRel> {
        match token {
            "parent" => Some(IssueLifecycleRel::Parent),
            "blocks" => Some(IssueLifecycleRel::Blocks),
            "blocked_by" => Some(IssueLifecycleRel::BlockedBy),
            "closes" => Some(IssueLifecycleRel::Closes),
            "depends_on" => Some(IssueLifecycleRel::DependsOn),
            "relates" => Some(IssueLifecycleRel::Relates),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_relation_tokens_round_trip() {
        for relation in [
            IssueLifecycleRel::Parent,
            IssueLifecycleRel::Blocks,
            IssueLifecycleRel::BlockedBy,
            IssueLifecycleRel::Closes,
            IssueLifecycleRel::DependsOn,
            IssueLifecycleRel::Relates,
        ] {
            assert_eq!(
                IssueLifecycleRel::from_token(relation.as_str()),
                Some(relation)
            );
        }
        assert_eq!(IssueLifecycleRel::from_token("unknown"), None);
    }
}
