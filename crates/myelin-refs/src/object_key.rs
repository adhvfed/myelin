//! # `object_key` — the ONE canonical **type-qualified authz object key** (R2.2)
//!
//! **The defect this kills (identity finding #a, 2026-07-06 review):** the authz read side
//! (`check_engine::object_id_of`, `matcher::subject_object_id`) reduced a check/match object to the
//! **bare trailing `/`-segment** of its ref. That threw the subsystem/type qualification away, so
//! - `myelin://acme/issues/issue/PROJ-1` and `myelin://acme/git/repo/PROJ-1` collapsed to the SAME
//!   key `PROJ-1` — a grant on the issue authorized the repo of the same trailing id (cross-type
//!   confusion within a tenant), and
//! - the reduction was **inconsistent across spellings**: a full URN collapsed to its id while a
//!   bare `repo:core` (no `/`) was kept whole — a grant written in one spelling silently never
//!   matched a check in the other, and a NAMESPACED bare id (`repo:team/app`, the R2.1a git slug
//!   grammar) collapsed to `app`, orphaning the repo's own bootstrap grant.
//!
//! [`object_key`] is the one function both readers (and any writer that needs to normalise) route
//! through. It maps **either spelling** of a logical authz object onto ONE canonical
//! **type-qualified tuple key** — the `type:id` form the S3 tuple writers already store
//! (`repo:<slug>`, `issue:PROJ-1`, `org:acme`, …):
//!
//! | input ref                                   | [`ObjectKey::tuple_key`] |
//! |---------------------------------------------|--------------------------|
//! | `repo:core` (bare, type-prefixed)           | `repo:core` (byte-identical — existing grants keep their keys) |
//! | `repo:team/app` (bare, namespaced slug)     | `repo:team/app` (**never** collapsed to `app`) |
//! | `myelin://acme/git/repo/core` (URN)         | `repo:core` (same key as the bare spelling) |
//! | `myelin://acme/git/repo/repo:core` (URN, id already type-prefixed) | `repo:core` (no double prefix) |
//! | `pr:core:42` / `ref:core::glob` (bare, multi-`:`) | unchanged (the first `:` splits type from id) |
//! | `…/pr/4291#comment-7` (any `#sub` anchor)   | keys at the **root** object (`pr:4291`) — a comment authorizes at its PR |
//! | `level_0` (bare, no type prefix)            | `level_0` (a type-less legacy id is its own key — never guessed) |
//! | `myelin://…` with ≠ 4 scope segments        | `None` — **fail-closed** (an unparseable URN is genuine uncertainty) |
//!
//! Two different types with the same trailing id (`issue:PROJ-1` vs `repo:PROJ-1`, or their URN
//! forms) now **never** share a key; the two spellings of the same logical object **always** do.
//!
//! ## Why the key is `type:id` and not `subsystem/type/id`
//! The bare spelling (`repo:core`) — the grammar every existing tuple writer stores and the R2.1a
//! wire authorizer checks — carries no subsystem segment, and the S3 store's edge set is keyed by
//! exactly that spelling. `type:id` is therefore the ONLY qualification level at which
//! **write-side keys == read-side keys with zero migration**: every stored bare-form grant is
//! byte-identical under this normalisation (see `bare_form_is_a_fixed_point`). The `<type>` tokens
//! are workspace-unique per the Bus §6.2 table, so dropping the subsystem does not re-introduce a
//! collision. The `(tenant, region)` partition is deliberately NOT in the key — the tuple store
//! scopes every read/write by the verified `TenantScope` (there is no cross-tenant query path).
//!
//! ## Why the URN arm is a STRUCTURAL parse, not the strict [`crate::parse`]
//! The strict parser validates subsystem/type against the Bus token table — right for minting and
//! for the reference graph. The authz key, though, must also cover **historically-spelled** check
//! objects that predate the table (`myelin://acme/issues/issue/…` with the plural `issues`,
//! `myelin://<t>/identity/action/<action>` whose `action` type is not a graph artifact type). For
//! the KEY the load-bearing property is qualification + consistency, not table membership — an
//! off-table token still yields a well-qualified, non-colliding key, while a structurally
//! malformed URN (not exactly `tenant/subsystem/type/id`) stays a fail-closed `None`.

use myelin_events::ArtifactRef;

/// The parsed, canonical authz identity of a check/match object — the output of [`object_key`].
/// `tuple_key()` is the ONE string the S3 tuple edge set keys on (write side and read side).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectKey {
    /// The tenant segment — `Some` iff the ref was URN-spelled (`myelin://<tenant>/…`). Bare refs
    /// carry no tenant: the tuple store's verified `TenantScope` is the partition, never the key.
    pub tenant: Option<String>,
    /// The subsystem segment — `Some` iff URN-spelled. NOT part of the tuple key (see module doc).
    pub subsystem: Option<String>,
    /// The object TYPE qualifier: the URN's `<type>` segment, or a bare ref's `type:` prefix.
    /// `None` for a type-less bare id (a legacy/raw tuple key — kept whole, never guessed).
    pub object_type: Option<String>,
    /// The unqualified object id (the subsystem-minted id, sans any duplicate `type:` prefix).
    pub id: String,
}

impl ObjectKey {
    /// **The canonical type-qualified tuple key** — `type:id` when a type is known, the bare id
    /// otherwise. For every bare-form input this is byte-identical with the input's root (existing
    /// stored grants keep their keys); for a URN it is the SAME key the bare spelling yields.
    pub fn tuple_key(&self) -> String {
        match &self.object_type {
            Some(ty) => format!("{ty}:{}", self.id),
            None => self.id.clone(),
        }
    }
}

/// Map an [`ArtifactRef`] (either spelling, with or without a `#sub` anchor) onto its canonical
/// [`ObjectKey`]. Returns `None` for an empty/whitespace ref or a structurally malformed URN —
/// genuine uncertainty the caller MUST fail closed on (deny / zero matches), never guess through.
///
/// - A `#sub` anchor is stripped first: a sub-artifact (`pr:4:…#comment-7`) authorizes at its ROOT
///   object (§3.5 — the object-level check is about the parent).
/// - `myelin://<tenant>/<subsystem>/<type>/<id>` → type-qualified key `type:id`. An id segment that
///   ALREADY carries the `type:` prefix (`…/repo/repo:core` — the historical URN spelling) is not
///   double-prefixed.
/// - A bare ref is split at its FIRST `:` into `type:id` (`pr:core:42` → type `pr`, id `core:42`);
///   the whole root is the key, byte-identical — including namespaced ids (`repo:team/app`).
pub fn object_key(object: &ArtifactRef) -> Option<ObjectKey> {
    let raw = object.0.trim();
    if raw.is_empty() {
        return None;
    }
    // Strip the `#sub` anchor (and, on a subject-position userset spelling, the `#relation`):
    // the root object is what the authz decision is about.
    let root = raw.split('#').next().unwrap_or(raw);
    if root.is_empty() {
        return None;
    }

    if let Some(rest) = root.strip_prefix(crate::SCHEME) {
        // URN spelling: STRUCTURALLY exactly four non-empty `/` segments (tenant/subsystem/type/id).
        // Anything else is malformed → None (fail-closed at the caller). NOTE: the id segment may
        // itself contain `/` only in the bare spelling; the URN grammar admits no `/` inside `id`,
        // so a 5+-segment split here is malformed, not a namespaced id.
        let segs: Vec<&str> = rest.split('/').collect();
        if segs.len() != 4 || segs.iter().any(|s| s.is_empty()) {
            return None;
        }
        let (tenant, subsystem, ty, id_seg) = (segs[0], segs[1], segs[2], segs[3]);
        // De-duplicate a `type:`-prefixed id segment (`…/repo/repo:core` → id `core`), so the URN
        // spelling and the bare spelling produce the SAME `type:id` key.
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

    // A scheme that is not ours ("https://…") is not an authz object ref — fail closed rather than
    // keying on a URL fragment. (A bare id legitimately contains `:`, but never `://`.)
    if root.contains("://") {
        return None;
    }

    // Bare spelling: `type:id` (split at the FIRST `:` — the §7.3 id-column convention), or a
    // type-less legacy id kept whole. Either way the tuple key is the root, byte-identical.
    match root.split_once(':') {
        Some((ty, id)) if !ty.is_empty() && !id.is_empty() => Some(ObjectKey {
            tenant: None,
            subsystem: None,
            object_type: Some(ty.to_string()),
            id: id.to_string(),
        }),
        // `:x` / `x:` / no `:` at all — a type-less bare id; keep it whole (its own key).
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

    /// **The cross-type collision is dead:** two types sharing a trailing id NEVER share a key —
    /// in bare form, URN form, or across forms.
    #[test]
    fn different_types_with_the_same_trailing_id_never_collide() {
        assert_ne!(key("issue:PROJ-1"), key("repo:PROJ-1"));
        assert_ne!(
            key("myelin://acme/issue/issue/PROJ-1"),
            key("myelin://acme/git/repo/PROJ-1")
        );
        assert_ne!(key("myelin://acme/git/repo/PROJ-1"), key("issue:PROJ-1"));
    }

    /// **The two spellings of ONE logical object always agree** — the URN form keys exactly as the
    /// bare form, with and without the historical `type:`-prefixed id segment.
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
            key("myelin://acme/issues/issue/issue:PROJ-1"),
            key("issue:PROJ-1"),
            "the historical plural-subsystem URN spelling still reaches the one key"
        );
    }

    /// **Every bare form is a fixed point** (byte-identical key) — existing stored grants keep
    /// their keys with zero migration, including multi-`:` ids and the R2.1a namespaced slug.
    #[test]
    fn bare_form_is_a_fixed_point() {
        for s in [
            "repo:core",
            "repo:team/app", // the R2.1a namespaced slug — NEVER collapsed to `app`
            "issue:PROJ-1",
            "org:acme",
            "team:eng",
            "pr:core:42",
            "ref:core::glob",
            "issues.read", // the action-grant legacy key (type-less, kept whole)
            "level_0",
        ] {
            assert_eq!(key(s), Some(s.to_string()), "`{s}` must key as itself");
        }
    }

    /// A `#sub` anchor keys at the ROOT object (a comment authorizes at its PR/issue), in both
    /// spellings.
    #[test]
    fn sub_anchor_keys_at_the_root() {
        assert_eq!(
            key("myelin://acme/issue/issue/PROJ-1#comment-7"),
            Some("issue:PROJ-1".into())
        );
        assert_eq!(key("pr:core:42#comment-7"), Some("pr:core:42".into()));
    }

    /// Malformed refs are `None` — the caller fails closed (deny / zero matches), never guesses.
    #[test]
    fn malformed_refs_are_fail_closed_none() {
        for s in [
            "",
            "   ",
            "myelin://acme/git/repo",          // 3 segments
            "myelin://acme/git/repo/a/b",      // 5 segments (no `/` inside a URN id)
            "myelin://acme//repo/core",        // empty segment
            "myelin://acme/git/repo/repo:",    // empty de-duplicated id
            "https://acme/git/repo/core",      // foreign scheme
            "#comment-7",                      // anchor with no root
        ] {
            assert_eq!(key(s), None, "`{s}` must be fail-closed None");
        }
    }

    /// The structured parts are exposed (tenant/subsystem/type/id) so a caller can gate on the
    /// type or tenant without re-parsing.
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
