use myelin_events::ArtifactRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectKey {
    pub tenant: Option<String>,
    pub subsystem: Option<String>,
    pub object_type: Option<String>,
    pub id: String,
}

impl ObjectKey {
    pub fn tuple_key(&self) -> String {
        match &self.object_type {
            Some(ty) => format!("{ty}:{}", self.id),
            None => self.id.clone(),
        }
    }
}

pub fn object_key(object: &ArtifactRef) -> Option<ObjectKey> {
    let raw = object.0.trim();
    if raw.is_empty() {
        return None;
    }
    let root = raw.split('#').next().unwrap_or(raw);
    if root.is_empty() {
        return None;
    }

    if root.starts_with(crate::SCHEME) {
        if let Ok(parsed) = crate::parse_scoped(raw) {
            let id = parsed
                .id
                .strip_prefix(&format!("{}:", parsed.type_))
                .unwrap_or(&parsed.id);
            if id.is_empty() {
                return None;
            }
            return Some(ObjectKey {
                tenant: Some(parsed.tenant.0),
                subsystem: Some(parsed.subsystem),
                object_type: Some(parsed.type_),
                id: id.to_string(),
            });
        }

        // Historical plural-subsystem references predate the canonical parser. Keep their one
        // explicit compatibility path while refusing every other malformed URN.
        let rest = root.strip_prefix(crate::SCHEME)?;
        let segs: Vec<&str> = rest.split('/').collect();
        if segs.len() != 4 || segs[1] != "issues" || segs.iter().any(|s| s.is_empty()) {
            return None;
        }
        let (tenant, subsystem, ty, id_seg) = (segs[0], segs[1], segs[2], segs[3]);
        let id = id_seg.strip_prefix(&format!("{ty}:")).unwrap_or(id_seg);
        if id.is_empty() {
            return None;
        }
        return Some(ObjectKey {
            tenant: Some(tenant.to_string()),
            subsystem: Some(subsystem.to_string()),
            object_type: Some(ty.to_string()),
            id: id.to_string(),
        });
    }

    if root.contains("://") {
        return None;
    }

    match root.split_once(':') {
        Some((ty, id)) if !ty.is_empty() && !id.is_empty() => Some(ObjectKey {
            tenant: None,
            subsystem: None,
            object_type: Some(ty.to_string()),
            id: id.to_string(),
        }),
        _ => Some(ObjectKey {
            tenant: None,
            subsystem: None,
            object_type: None,
            id: root.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Option<String> {
        object_key(&ArtifactRef(s.into())).map(|k| k.tuple_key())
    }

    #[test]
    fn different_types_with_the_same_trailing_id_never_collide() {
        assert_ne!(key("issue:PROJ-1"), key("repo:PROJ-1"));
        assert_ne!(
            key("myelin://acme/issue/issue/PROJ-1"),
            key("myelin://acme/git/repo/PROJ-1")
        );
        assert_ne!(key("myelin://acme/git/repo/PROJ-1"), key("issue:PROJ-1"));
    }

    #[test]
    fn urn_and_bare_spellings_of_the_same_object_agree() {
        assert_eq!(key("myelin://acme/git/repo/core"), Some("repo:core".into()));
        assert_eq!(
            key("myelin://acme/git/repo/repo:core"),
            Some("repo:core".into()),
            "an already-prefixed URN id is not double-prefixed"
        );
        assert_eq!(key("repo:core"), Some("repo:core".into()));
        assert_eq!(
            key("myelin://acme/git/repo/team/app"),
            key("repo:team/app"),
            "a hierarchical repository keeps one authorization key"
        );
        assert_eq!(
            key("myelin://acme/issues/issue/issue:PROJ-1"),
            key("issue:PROJ-1"),
            "the historical plural-subsystem URN spelling still reaches the one key"
        );
    }

    #[test]
    fn bare_form_is_a_fixed_point() {
        for s in [
            "repo:core",
            "repo:team/app",
            "issue:PROJ-1",
            "org:acme",
            "team:eng",
            "pr:core:42",
            "ref:core::glob",
            "issues.read",
            "level_0",
        ] {
            assert_eq!(key(s), Some(s.to_string()), "`{s}` must key as itself");
        }
    }

    #[test]
    fn sub_anchor_keys_at_the_root() {
        assert_eq!(
            key("myelin://acme/issue/issue/PROJ-1#comment-7"),
            Some("issue:PROJ-1".into())
        );
        assert_eq!(key("pr:core:42#comment-7"), Some("pr:core:42".into()));
    }

    #[test]
    fn malformed_refs_are_fail_closed_none() {
        for s in [
            "",
            "   ",
            "myelin://acme/git/repo",
            "myelin://acme/git/repo/a//b",
            "myelin://acme//repo/core",
            "myelin://acme/git/repo/repo:",
            "https://acme/git/repo/core",
            "#comment-7",
        ] {
            assert_eq!(key(s), None, "`{s}` must be fail-closed None");
        }
    }

    #[test]
    fn structured_parts_are_exposed() {
        let k = object_key(&ArtifactRef("myelin://acme/git/repo/core".into())).unwrap();
        assert_eq!(k.tenant.as_deref(), Some("acme"));
        assert_eq!(k.subsystem.as_deref(), Some("git"));
        assert_eq!(k.object_type.as_deref(), Some("repo"));
        assert_eq!(k.id, "core");

        let b = object_key(&ArtifactRef("repo:team/app".into())).unwrap();
        assert_eq!(b.tenant, None);
        assert_eq!(b.object_type.as_deref(), Some("repo"));
        assert_eq!(b.id, "team/app");
    }
}
