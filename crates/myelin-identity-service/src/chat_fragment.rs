use crate::namespace::{FragmentDef, PermissionRule, Userset};
use myelin_identity::{ObjectType, Permission, RelName};

pub mod object_types {
    pub const CHANNEL: &str = "channel";
    pub const MESSAGE: &str = "message";
    pub const UNFURL: &str = "unfurl";
}

pub const MEMBER: &str = "member";

pub const TARGET: &str = "target";

pub const READ: &str = "read";
pub const POST: &str = "post";
pub const MANAGE: &str = "manage";

pub const VIEW: &str = "view";

fn rel(n: &str) -> Userset {
    Userset::Relation(RelName(n.into()))
}

fn ttu(tupleset: &str, computed: &str) -> Userset {
    Userset::TupleToUserset {
        tupleset: RelName(tupleset.into()),
        computed: RelName(computed.into()),
    }
}

fn perm(name: &str, rewrite: Userset) -> PermissionRule {
    PermissionRule {
        permission: Permission(name.into()),
        rewrite,
    }
}

fn frag(object_type: &str, relations: &[&str], permissions: Vec<PermissionRule>) -> FragmentDef {
    FragmentDef {
        object_type: ObjectType(object_type.into()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions,
    }
}

pub fn channel_fragment() -> FragmentDef {
    frag(
        object_types::CHANNEL,
        &["parent_project", MEMBER, "watcher"],
        vec![
            perm(
                READ,
                Userset::Union(vec![rel(MEMBER), ttu("parent_project", "view")]),
            ),
            perm(POST, rel(MEMBER)),
            perm(
                MANAGE,
                Userset::Intersect(vec![rel(MEMBER), ttu("parent_project", "view")]),
            ),
        ],
    )
    .watchable()
}

pub fn message_fragment() -> FragmentDef {
    frag(
        object_types::MESSAGE,
        &["parent_channel"],
        vec![perm(VIEW, ttu("parent_channel", READ))],
    )
}

pub fn unfurl_fragment() -> FragmentDef {
    frag(
        object_types::UNFURL,
        &["parent_message", TARGET],
        vec![perm(VIEW, ttu("parent_message", VIEW))],
    )
}

pub fn chat_fragment_defs() -> Vec<FragmentDef> {
    vec![channel_fragment(), message_fragment(), unfurl_fragment()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use myelin_identity::FragmentAdmit;

    #[test]
    fn chat_fragment_admits_into_the_cell_schema() {
        let mut eng = NamespaceEngine::with_core_hierarchy();
        for def in chat_fragment_defs() {
            let admit = eng.admit(&def);
            assert!(
                matches!(admit, FragmentAdmit::Admitted { .. }),
                "the Chat `{}` fragment admits into the cell schema: {admit:?}",
                def.object_type.0
            );
        }
        for ty in ["channel", "message", "unfurl"] {
            assert!(
                eng.object_types().contains(&ty.to_string()),
                "`{ty}` is admitted"
            );
        }
        assert!(
            eng.resolve_permission("channel", READ).is_some(),
            "channel.read is a compiled permission"
        );
        assert!(
            eng.resolve_permission("channel", POST).is_some(),
            "channel.post is a compiled permission"
        );
        assert!(
            eng.resolve_permission("channel", MANAGE).is_some(),
            "channel.manage is a compiled permission"
        );
        assert!(
            eng.resolve_permission("message", VIEW).is_some(),
            "message.view is a compiled permission"
        );
        assert!(
            eng.resolve_permission("unfurl", VIEW).is_some(),
            "unfurl.view is a compiled permission"
        );
    }

    #[test]
    fn channel_read_is_member_union_parent_project_view() {
        let channel = channel_fragment();
        let read = channel
            .permissions
            .iter()
            .find(|p| p.permission.0 == READ)
            .expect("channel declares read");
        assert_eq!(
            read.rewrite,
            Userset::Union(vec![rel(MEMBER), ttu("parent_project", "view")]),
            "channel.read = member ∪ parent_project->view (§5)"
        );
    }

    #[test]
    fn channel_post_and_manage_keep_distinct_collaboration_boundaries() {
        let channel = channel_fragment();
        let post = channel
            .permissions
            .iter()
            .find(|permission| permission.permission.0 == POST)
            .expect("channel declares post");
        let manage = channel
            .permissions
            .iter()
            .find(|permission| permission.permission.0 == MANAGE)
            .expect("channel declares manage");
        assert_eq!(post.rewrite, rel(MEMBER), "channel.post = member");
        assert_eq!(
            manage.rewrite,
            Userset::Intersect(vec![rel(MEMBER), ttu("parent_project", "view")]),
            "channel.manage requires membership and project visibility"
        );
    }

    #[test]
    fn message_view_inherits_parent_channel_read() {
        let message = message_fragment();
        let view = message
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("message declares view");
        assert_eq!(
            view.rewrite,
            ttu("parent_channel", READ),
            "message.view = parent_channel->read (§5)"
        );
    }

    #[test]
    fn unfurl_view_inherits_parent_message_and_carries_target() {
        let unfurl = unfurl_fragment();
        let view = unfurl
            .permissions
            .iter()
            .find(|p| p.permission.0 == VIEW)
            .expect("unfurl declares view");
        assert_eq!(
            view.rewrite,
            ttu("parent_message", VIEW),
            "unfurl.view = parent_message->view (§5)"
        );
        assert!(
            unfurl.relations.iter().any(|r| r.0 == TARGET),
            "unfurl declares the `target` relation (the Refs render-gate edge)"
        );
    }

    #[test]
    fn channel_is_watchable() {
        assert!(
            channel_fragment().is_watchable(),
            "channel is watchable (C8 - the 50k-density read-fanout)"
        );
        assert!(
            !message_fragment().is_watchable(),
            "message is not independently watchable"
        );
        assert!(
            !unfurl_fragment().is_watchable(),
            "unfurl is not independently watchable"
        );
    }

    #[test]
    fn no_chat_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in chat_fragment_defs() {
            assert!(
                !mints(&f.object_type.0),
                "type `{}` is a bare identifier",
                f.object_type.0
            );
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(
                    !mints(&p.permission.0),
                    "permission `{}` is a bare identifier",
                    p.permission.0
                );
            }
        }
    }
}
