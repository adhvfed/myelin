use myelin_identity::{NamespaceFragment, ObjectType, Permission, RelName};

pub mod object_types {
    pub const CHANNEL: &str = "channel";
    pub const MESSAGE: &str = "message";
}

fn fragment(object_type: &str, relations: &[&str], permissions: &[&str]) -> NamespaceFragment {
    NamespaceFragment {
        object_type: ObjectType(object_type.to_string()),
        relations: relations.iter().map(|r| RelName(r.to_string())).collect(),
        permissions: permissions
            .iter()
            .map(|p| Permission(p.to_string()))
            .collect(),
    }
}

pub fn channel_fragment() -> NamespaceFragment {
    fragment(
        object_types::CHANNEL,
        &["parent_project", "member", "watcher"],
        &["read", "post", "manage"],
    )
}

pub fn message_fragment() -> NamespaceFragment {
    fragment(object_types::MESSAGE, &["parent_channel"], &["view"])
}

pub fn chat_fragment() -> Vec<NamespaceFragment> {
    vec![channel_fragment(), message_fragment()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_definition_declares_its_frozen_relations() {
        let channel = channel_fragment();
        let channel_rels: Vec<&str> = channel.relations.iter().map(|r| r.0.as_str()).collect();
        for expected in ["parent_project", "member", "watcher"] {
            assert!(
                channel_rels.contains(&expected),
                "channel must declare the `{expected}` relation (§5)"
            );
        }
        assert!(message_fragment()
            .relations
            .contains(&RelName("parent_channel".into())));
    }

    #[test]
    fn the_channel_read_rewrite_relations_are_declared() {
        let channel = channel_fragment();
        assert!(
            channel.relations.contains(&RelName("member".into())),
            "`member` (the ACL / the `read` + arm) must be declared (§5)"
        );
        assert!(
            channel.relations.contains(&RelName("parent_project".into())),
            "`parent_project` (the inheritance edge - `read` inherits, `manage` intersects) must be \
             declared (§5)"
        );
    }

    #[test]
    fn watcher_is_declared_on_the_watchable_channel_type() {
        assert!(
            channel_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "the `channel` watchable type declares `watcher` (Notif read-fanout, contract 4.9)"
        );
        assert!(
            !message_fragment()
                .relations
                .contains(&RelName("watcher".into())),
            "`message` does not carry its own watcher - per-thread watch derives from channel (§5)"
        );
    }

    #[test]
    fn the_two_chat_object_types_are_frozen() {
        let frag = chat_fragment();
        let types: Vec<&str> = frag.iter().map(|f| f.object_type.0.as_str()).collect();
        assert_eq!(types, vec!["channel", "message"]);
        for p in ["read", "post", "manage"] {
            assert!(
                channel_fragment()
                    .permissions
                    .contains(&Permission(p.into())),
                "channel declares the `{p}` permission (§5)"
            );
        }
        assert!(message_fragment()
            .permissions
            .contains(&Permission("view".into())));
    }

    #[test]
    fn no_name_smuggles_an_object_id() {
        let mints = |s: &str| s.contains(':') || s.contains('/') || s.contains('#');
        for f in chat_fragment() {
            assert!(!mints(&f.object_type.0), "type name is a bare identifier");
            for r in &f.relations {
                assert!(!mints(&r.0), "relation `{}` is a bare identifier", r.0);
            }
            for p in &f.permissions {
                assert!(!mints(&p.0), "permission `{}` is a bare identifier", p.0);
            }
        }
    }
}
