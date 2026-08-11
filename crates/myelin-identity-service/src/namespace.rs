use crate::check_engine::{CheckEngine, USERSET_SEP};
use myelin_identity::{
    FragmentAdmit, NamespaceFragment, ObjectType, Permission, Principal, RelName,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RULE_DEPTH: usize = 16;

pub const WATCHER_RELATION: &str = "watcher";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Userset {
    Relation(RelName),
    Union(Vec<Userset>),
    Intersect(Vec<Userset>),
    Exclusion {
        base: Box<Userset>,
        subtracted: Box<Userset>,
    },
    TupleToUserset {
        tupleset: RelName,
        computed: RelName,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRule {
    pub permission: Permission,
    pub rewrite: Userset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentDef {
    pub object_type: ObjectType,
    pub relations: Vec<RelName>,
    pub permissions: Vec<PermissionRule>,
}

impl FragmentDef {
    pub fn to_abi(&self) -> NamespaceFragment {
        NamespaceFragment {
            object_type: self.object_type.clone(),
            relations: self.relations.clone(),
            permissions: self
                .permissions
                .iter()
                .map(|r| r.permission.clone())
                .collect(),
        }
    }

    pub fn watchable(mut self) -> FragmentDef {
        if !self.relations.iter().any(|r| r.0 == WATCHER_RELATION) {
            self.relations.push(RelName(WATCHER_RELATION.to_string()));
        }
        self
    }

    pub fn is_watchable(&self) -> bool {
        self.relations.iter().any(|r| r.0 == WATCHER_RELATION)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmitReject {
    EmptyObjectType,
    DuplicateObjectType { object_type: String },
    NameMintsObjectId { name: String, kind: &'static str },
    UndeclaredRelation {
        permission: String,
        relation: String,
    },
    UndeclaredTupleset {
        permission: String,
        tupleset: String,
    },
    PermissionCycle { permission: String },
    RuleTooDeep { permission: String },
    DuplicatePermission { permission: String },
}

impl core::fmt::Display for AdmitReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AdmitReject::EmptyObjectType => write!(f, "the object type name is empty"),
            AdmitReject::DuplicateObjectType { object_type } => {
                write!(
                    f,
                    "object type `{object_type}` is already admitted (duplicate definition)"
                )
            }
            AdmitReject::NameMintsObjectId { name, kind } => write!(
                f,
                "the {kind} name `{name}` carries an object-id form (`:`/`/`/`#`) - Id never \
                 invents object ids; a fragment declares types/relations/permissions, never ids"
            ),
            AdmitReject::UndeclaredRelation {
                permission,
                relation,
            } => write!(
                f,
                "permission `{permission}` references relation `{relation}`, which this fragment \
                 did not declare"
            ),
            AdmitReject::UndeclaredTupleset {
                permission,
                tupleset,
            } => write!(
                f,
                "permission `{permission}` inherits via tupleset `{tupleset}`, which this fragment \
                 did not declare as a relation"
            ),
            AdmitReject::PermissionCycle { permission } => {
                write!(
                    f,
                    "permission `{permission}` is self-referential (a schema cycle)"
                )
            }
            AdmitReject::RuleTooDeep { permission } => write!(
                f,
                "permission `{permission}` nests deeper than the admit bound ({MAX_RULE_DEPTH}) - \
                 an unbounded schema"
            ),
            AdmitReject::DuplicatePermission { permission } => {
                write!(
                    f,
                    "permission `{permission}` is declared twice in this fragment"
                )
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct NamespaceEngine {
    schema: BTreeMap<String, CompiledType>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompiledType {
    relations: BTreeSet<String>,
    permissions: BTreeMap<String, Userset>,
}

impl NamespaceEngine {
    pub fn new() -> NamespaceEngine {
        NamespaceEngine {
            schema: BTreeMap::new(),
        }
    }

    pub fn with_core_hierarchy() -> NamespaceEngine {
        let mut eng = NamespaceEngine::new();
        for frag in core_hierarchy() {
            match eng.admit(&frag) {
                FragmentAdmit::Admitted { .. } => {}
                FragmentAdmit::Rejected { reason } => {
                    panic!("the core hierarchy fragment must admit, but was rejected: {reason}")
                }
            }
        }
        eng
    }

    pub fn admit(&mut self, frag: &FragmentDef) -> FragmentAdmit {
        match self.validate(frag) {
            Err(reject) => FragmentAdmit::Rejected {
                reason: reject.to_string(),
            },
            Ok(compiled) => {
                let ot = frag.object_type.0.clone();
                self.schema.insert(ot.clone(), compiled);
                FragmentAdmit::Admitted {
                    fragment_id: ot,
                }
            }
        }
    }

    pub fn admit_abi(&mut self, frag: &NamespaceFragment) -> FragmentAdmit {
        let def = FragmentDef {
            object_type: frag.object_type.clone(),
            relations: frag.relations.clone(),
            permissions: frag
                .permissions
                .iter()
                .map(|p| PermissionRule {
                    permission: p.clone(),
                    rewrite: Userset::Relation(RelName(p.0.clone())),
                })
                .collect(),
        };
        self.admit(&def)
    }

    fn validate(&self, frag: &FragmentDef) -> Result<CompiledType, AdmitReject> {
        let ot = frag.object_type.0.trim();
        if ot.is_empty() {
            return Err(AdmitReject::EmptyObjectType);
        }
        if mints_object_id(ot) {
            return Err(AdmitReject::NameMintsObjectId {
                name: ot.to_string(),
                kind: "object type",
            });
        }
        if self.schema.contains_key(ot) {
            return Err(AdmitReject::DuplicateObjectType {
                object_type: ot.to_string(),
            });
        }

        let mut relations: BTreeSet<String> = BTreeSet::new();
        for r in &frag.relations {
            if mints_object_id(&r.0) {
                return Err(AdmitReject::NameMintsObjectId {
                    name: r.0.clone(),
                    kind: "relation",
                });
            }
            relations.insert(r.0.clone());
        }

        let mut permissions: BTreeMap<String, Userset> = BTreeMap::new();
        for rule in &frag.permissions {
            let pname = rule.permission.0.clone();
            if mints_object_id(&pname) {
                return Err(AdmitReject::NameMintsObjectId {
                    name: pname,
                    kind: "permission",
                });
            }
            if permissions.contains_key(&pname) {
                return Err(AdmitReject::DuplicatePermission { permission: pname });
            }
            self.validate_rewrite(&pname, &relations, &rule.rewrite, 0)?;
            permissions.insert(pname, rule.rewrite.clone());
        }

        Ok(CompiledType {
            relations,
            permissions,
        })
    }

    fn validate_rewrite(
        &self,
        permission: &str,
        relations: &BTreeSet<String>,
        rewrite: &Userset,
        depth: usize,
    ) -> Result<(), AdmitReject> {
        if depth > MAX_RULE_DEPTH {
            return Err(AdmitReject::RuleTooDeep {
                permission: permission.to_string(),
            });
        }
        match rewrite {
            Userset::Relation(r) => {
                if !relations.contains(&r.0) {
                    if r.0 == permission {
                        return Err(AdmitReject::PermissionCycle {
                            permission: permission.to_string(),
                        });
                    }
                    return Err(AdmitReject::UndeclaredRelation {
                        permission: permission.to_string(),
                        relation: r.0.clone(),
                    });
                }
                Ok(())
            }
            Userset::Union(arms) | Userset::Intersect(arms) => {
                for arm in arms {
                    self.validate_rewrite(permission, relations, arm, depth + 1)?;
                }
                Ok(())
            }
            Userset::Exclusion { base, subtracted } => {
                self.validate_rewrite(permission, relations, base, depth + 1)?;
                self.validate_rewrite(permission, relations, subtracted, depth + 1)
            }
            Userset::TupleToUserset { tupleset, .. } => {
                if !relations.contains(&tupleset.0) {
                    return Err(AdmitReject::UndeclaredTupleset {
                        permission: permission.to_string(),
                        tupleset: tupleset.0.clone(),
                    });
                }
                Ok(())
            }
        }
    }

    pub fn resolve_permission(&self, object_type: &str, permission: &str) -> Option<Userset> {
        self.schema
            .get(object_type)
            .and_then(|t| t.permissions.get(permission).cloned())
    }

    pub fn relations_of(&self, object_type: &str) -> Vec<String> {
        self.schema
            .get(object_type)
            .map(|t| t.relations.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn has_relation(&self, object_type: &str, relation: &str) -> bool {
        self.schema
            .get(object_type)
            .map(|t| t.relations.contains(relation))
            .unwrap_or(false)
    }

    pub fn object_types(&self) -> Vec<String> {
        self.schema.keys().cloned().collect()
    }

    pub fn is_watchable(&self, object_type: &str) -> bool {
        self.has_relation(object_type, WATCHER_RELATION)
    }

    pub fn watchable_types(&self) -> Vec<String> {
        self.schema
            .iter()
            .filter(|(_, t)| t.relations.contains(WATCHER_RELATION))
            .map(|(ot, _)| ot.clone())
            .collect()
    }

    pub fn declare_watchable(&mut self, object_type: &str) -> FragmentAdmit {
        match self.schema.get_mut(object_type) {
            Some(t) => {
                t.relations.insert(WATCHER_RELATION.to_string());
                FragmentAdmit::Admitted {
                    fragment_id: object_type.to_string(),
                }
            }
            None => FragmentAdmit::Rejected {
                reason: format!(
                    "cannot declare `{object_type}` watchable: it is not an admitted object type (a \
                     watcher relation attaches to a known type, never invents one)"
                ),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn permits(
        &self,
        engine: &CheckEngine,
        scope: &TenantScope,
        subject: &Principal,
        object_type: &str,
        permission: &str,
        object: &ArtifactRef,
        at: &myelin_identity::Consistency,
    ) -> bool {
        self.eval(
            engine,
            scope,
            subject,
            object_type,
            permission,
            object,
            at,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn eval(
        &self,
        engine: &CheckEngine,
        scope: &TenantScope,
        subject: &Principal,
        object_type: &str,
        permission: &str,
        object: &ArtifactRef,
        at: &myelin_identity::Consistency,
        depth: usize,
    ) -> bool {
        if depth > MAX_RULE_DEPTH {
            return false;
        }
        match self.resolve_permission(object_type, permission) {
            Some(rewrite) => self.eval_userset(engine, scope, subject, object, at, &rewrite, depth),
            None => matches!(
                engine.check(
                    scope,
                    subject,
                    &RelName(permission.to_string()),
                    object,
                    at,
                    None
                ),
                myelin_identity::Decision::Allow
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_userset(
        &self,
        engine: &CheckEngine,
        scope: &TenantScope,
        subject: &Principal,
        object: &ArtifactRef,
        at: &myelin_identity::Consistency,
        rewrite: &Userset,
        depth: usize,
    ) -> bool {
        if depth > MAX_RULE_DEPTH {
            return false;
        }
        match rewrite {
            Userset::Relation(r) => {
                if matches!(
                    engine.check(scope, subject, r, object, at, None),
                    myelin_identity::Decision::Allow
                ) {
                    return true;
                }
                engine
                    .direct_subjects(scope, object, r, at)
                    .iter()
                    .filter_map(|userset| crate::check_engine::parse_userset(userset))
                    .any(|(parent_id, parent_relation)| {
                        self.eval(
                            engine,
                            scope,
                            subject,
                            &type_of_object_id(parent_id),
                            parent_relation,
                            &ArtifactRef(parent_id.into()),
                            at,
                            depth + 1,
                        )
                    })
            }
            Userset::Union(arms) => arms
                .iter()
                .any(|a| self.eval_userset(engine, scope, subject, object, at, a, depth + 1)),
            Userset::Intersect(arms) => arms
                .iter()
                .all(|a| self.eval_userset(engine, scope, subject, object, at, a, depth + 1)),
            Userset::Exclusion { base, subtracted } => {
                self.eval_userset(engine, scope, subject, object, at, base, depth + 1)
                    && !self.eval_userset(engine, scope, subject, object, at, subtracted, depth + 1)
            }
            Userset::TupleToUserset { tupleset, computed } => {
                let parents = engine.direct_subjects(scope, object, tupleset, at);
                parents.iter().any(|parent_subject| {
                    match crate::check_engine::parse_userset(parent_subject) {
                        Some((parent_id, parent_rel)) if parent_rel == computed.0 => {
                            let parent_type = type_of_object_id(parent_id);
                            self.eval(
                                engine,
                                scope,
                                subject,
                                &parent_type,
                                computed.0.as_str(),
                                &ArtifactRef(parent_id.to_string()),
                                at,
                                depth + 1,
                            )
                        }
                        _ => parent_subject == &subject.principal_id.0,
                    }
                })
            }
        }
    }
}

fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

pub fn type_of_object_ref(object: &ArtifactRef) -> String {
    let raw = object.0.trim();
    if raw.is_empty() {
        return String::new();
    }
    let root = raw.split(USERSET_SEP).next().unwrap_or(raw);
    if root.contains('/') {
        let segs: Vec<&str> = root.rsplit('/').collect();
        if segs.len() >= 2 {
            return type_of_object_id(segs[1]);
        }
    }
    type_of_object_id(root)
}

fn mints_object_id(name: &str) -> bool {
    name.contains(':') || name.contains('/') || name.contains(USERSET_SEP)
}

pub fn core_hierarchy() -> Vec<FragmentDef> {
    vec![
        FragmentDef {
            object_type: ObjectType("org".into()),
            relations: vec![RelName("member".into()), RelName("admin".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("member".into())),
                    Userset::Relation(RelName("admin".into())),
                ]),
            }],
        },
        FragmentDef {
            object_type: ObjectType("team".into()),
            relations: vec![RelName("member".into()), RelName("parent_org".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("member".into())),
                    Userset::TupleToUserset {
                        tupleset: RelName("parent_org".into()),
                        computed: RelName("view".into()),
                    },
                ]),
            }],
        },
        FragmentDef {
            object_type: ObjectType("project".into()),
            relations: vec![
                RelName("reader".into()),
                RelName("writer".into()),
                RelName("parent_team".into()),
            ],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("reader".into())),
                    Userset::Relation(RelName("writer".into())),
                    Userset::TupleToUserset {
                        tupleset: RelName("parent_team".into()),
                        computed: RelName("view".into()),
                    },
                ]),
            }],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        Consistency, ConsistencyMode, ObjectId, PrincipalId, PrincipalKind, RelationTuple,
        TupleDelta, Zookie,
    };
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn subject(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
        TupleDelta::Add(RelationTuple {
            object: ObjectId(object.into()),
            relation: RelName(relation.into()),
            subject: PrincipalId(subject.into()),
            caveat: None,
        })
    }

    fn latest() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn engine_with(scope: &TenantScope, tuples: &[TupleDelta]) -> CheckEngine {
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(scope, &subject("p-admin"), tuples, None, None, now())
            .expect("seed tuples");
        CheckEngine::new(store)
    }

    use crate::tuple_store::TupleStore;
    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    #[test]
    fn core_hierarchy_project_reader_via_team_membership_allows_nonmember_denies() {
        let ns = NamespaceEngine::with_core_hierarchy();
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
        );

        assert!(
            ns.permits(
                &eng,
                &s,
                &subject("p:alice"),
                "project",
                "view",
                &ArtifactRef("project:web".into()),
                &latest(),
            ),
            "a project reader granted via team membership inherits view (parent_team->view)"
        );
        assert!(
            !ns.permits(
                &eng,
                &s,
                &subject("p:bob"),
                "project",
                "view",
                &ArtifactRef("project:web".into()),
                &latest(),
            ),
            "a non-member does not inherit project view"
        );
    }

    #[test]
    fn core_hierarchy_direct_reader_allows() {
        let ns = NamespaceEngine::with_core_hierarchy();
        let s = scope("acme");
        let eng = engine_with(&s, &[add("project:web", "reader", "p:alice")]);
        assert!(ns.permits(
            &eng,
            &s,
            &subject("p:alice"),
            "project",
            "view",
            &ArtifactRef("project:web".into()),
            &latest(),
        ));
    }

    #[test]
    fn a_relation_can_name_a_computed_userset() {
        let mut ns = NamespaceEngine::with_core_hierarchy();
        assert!(matches!(
            ns.admit(&crate::chat_fragment::channel_fragment()),
            FragmentAdmit::Admitted { .. }
        ));
        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("project:web", "reader", "p:alice"),
                add("channel:release", "member", "project:web#view"),
            ],
        );

        assert!(ns.permits(
            &eng,
            &s,
            &subject("p:alice"),
            "channel",
            "post",
            &ArtifactRef("channel:release".into()),
            &latest(),
        ));
        assert!(!ns.permits(
            &eng,
            &s,
            &subject("p:bob"),
            "channel",
            "post",
            &ArtifactRef("channel:release".into()),
            &latest(),
        ));
    }

    #[test]
    fn the_four_userset_operators_each_evaluate() {
        let mut ns = NamespaceEngine::new();
        let frag = FragmentDef {
            object_type: ObjectType("doc".into()),
            relations: vec![
                RelName("reader".into()),
                RelName("editor".into()),
                RelName("blocked".into()),
                RelName("parent".into()),
            ],
            permissions: vec![
                PermissionRule {
                    permission: Permission("read".into()),
                    rewrite: Userset::Union(vec![
                        Userset::Relation(RelName("reader".into())),
                        Userset::Relation(RelName("editor".into())),
                    ]),
                },
                PermissionRule {
                    permission: Permission("review".into()),
                    rewrite: Userset::Intersect(vec![
                        Userset::Relation(RelName("reader".into())),
                        Userset::Relation(RelName("editor".into())),
                    ]),
                },
                PermissionRule {
                    permission: Permission("view".into()),
                    rewrite: Userset::Exclusion {
                        base: Box::new(Userset::Relation(RelName("reader".into()))),
                        subtracted: Box::new(Userset::Relation(RelName("blocked".into()))),
                    },
                },
                PermissionRule {
                    permission: Permission("inherit".into()),
                    rewrite: Userset::TupleToUserset {
                        tupleset: RelName("parent".into()),
                        computed: RelName("read".into()),
                    },
                },
            ],
        };
        assert!(matches!(ns.admit(&frag), FragmentAdmit::Admitted { .. }));

        let s = scope("acme");
        let eng = engine_with(
            &s,
            &[
                add("doc:1", "reader", "p:alice"),
                add("doc:1", "editor", "p:alice"),
                add("doc:1", "reader", "p:bob"),
                add("doc:1", "blocked", "p:bob"),
                add("doc:2", "parent", "doc:1#reader"),
            ],
        );
        let obj1 = ArtifactRef("doc:1".into());

        assert!(ns.permits(
            &eng,
            &s,
            &subject("p:alice"),
            "doc",
            "read",
            &obj1,
            &latest()
        ));
        assert!(ns.permits(&eng, &s, &subject("p:bob"), "doc", "read", &obj1, &latest()));

        assert!(ns.permits(
            &eng,
            &s,
            &subject("p:alice"),
            "doc",
            "review",
            &obj1,
            &latest()
        ));
        assert!(!ns.permits(
            &eng,
            &s,
            &subject("p:bob"),
            "doc",
            "review",
            &obj1,
            &latest()
        ));

        assert!(ns.permits(
            &eng,
            &s,
            &subject("p:alice"),
            "doc",
            "view",
            &obj1,
            &latest()
        ));
        assert!(
            !ns.permits(&eng, &s, &subject("p:bob"), "doc", "view", &obj1, &latest()),
            "exclusion: a blocked reader is excluded (− blocked)"
        );
    }

    #[test]
    fn admit_validates_well_formed_and_rejects_malformed() {
        let mut ns = NamespaceEngine::new();
        let ok = FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("reader".into()), RelName("writer".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Union(vec![
                    Userset::Relation(RelName("reader".into())),
                    Userset::Relation(RelName("writer".into())),
                ]),
            }],
        };
        assert!(
            matches!(ns.admit(&ok), FragmentAdmit::Admitted { fragment_id } if fragment_id == "repo")
        );

        let bad = FragmentDef {
            object_type: ObjectType("ci_run".into()),
            relations: vec![RelName("triggerer".into())],
            permissions: vec![PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Relation(RelName("reader".into())),
            }],
        };
        match ns.admit(&bad) {
            FragmentAdmit::Rejected { reason } => {
                assert!(
                    reason.contains("reader"),
                    "the rejection names the undeclared relation: {reason}"
                );
            }
            FragmentAdmit::Admitted { .. } => panic!("a malformed fragment must be rejected"),
        }
        assert!(!ns.object_types().contains(&"ci_run".to_string()));
    }

    #[test]
    fn a_fragment_cannot_mint_object_ids() {
        let mut ns = NamespaceEngine::new();
        let id_type = FragmentDef {
            object_type: ObjectType("repo:core".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![],
        };
        assert!(
            matches!(ns.admit(&id_type), FragmentAdmit::Rejected { .. }),
            "a type name that is actually an object id is rejected (Id never invents ids)"
        );
        let id_rel = FragmentDef {
            object_type: ObjectType("repo".into()),
            relations: vec![RelName("repo:core#reader".into())],
            permissions: vec![],
        };
        assert!(matches!(ns.admit(&id_rel), FragmentAdmit::Rejected { .. }));
    }

    #[test]
    fn self_referential_permission_is_rejected() {
        let mut ns = NamespaceEngine::new();
        let cyclic = FragmentDef {
            object_type: ObjectType("page".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: Userset::Relation(RelName("read".into())),
            }],
        };
        match ns.admit(&cyclic) {
            FragmentAdmit::Rejected { reason } => {
                assert!(reason.contains("cycle") || reason.contains("self"))
            }
            FragmentAdmit::Admitted { .. } => {
                panic!("a self-referential permission must be rejected")
            }
        }
    }

    #[test]
    fn duplicate_object_type_is_rejected() {
        let mut ns = NamespaceEngine::new();
        let frag = FragmentDef {
            object_type: ObjectType("space".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![],
        };
        assert!(matches!(ns.admit(&frag), FragmentAdmit::Admitted { .. }));
        assert!(
            matches!(ns.admit(&frag), FragmentAdmit::Rejected { .. }),
            "re-admitting the same object type is a duplicate-definition rejection"
        );
    }

    #[test]
    fn fragment_def_projects_onto_the_frozen_abi_names() {
        let frag = &core_hierarchy()[2];
        let abi = frag.to_abi();
        assert_eq!(abi.object_type, ObjectType("project".into()));
        assert!(abi.relations.contains(&RelName("parent_team".into())));
        assert_eq!(abi.permissions, vec![Permission("view".into())]);
    }

    #[test]
    fn core_hierarchy_admits_and_exposes_vocabulary() {
        let ns = NamespaceEngine::with_core_hierarchy();
        assert_eq!(ns.object_types(), vec!["org", "project", "team"]);
        assert!(ns.has_relation("project", "parent_team"));
        assert!(ns.resolve_permission("project", "view").is_some());
        assert!(ns.resolve_permission("project", "delete").is_none());
    }

    #[test]
    fn engine_adds_no_cross_tenant_path() {
        let ns = NamespaceEngine::with_core_hierarchy();
        let acme = scope("acme");
        let globex = scope("globex");
        let store = TupleStore::new(OutboxStore::new());
        store
            .write_tuples(
                &acme,
                &subject("p-admin"),
                &[add("project:web", "reader", "p:alice")],
                None,
                None,
                now(),
            )
            .expect("acme grant");
        let eng = CheckEngine::new(store);
        assert!(ns.permits(
            &eng,
            &acme,
            &subject("p:alice"),
            "project",
            "view",
            &ArtifactRef("project:web".into()),
            &latest()
        ));
        assert!(
            !ns.permits(
                &eng,
                &globex,
                &subject("p:alice"),
                "project",
                "view",
                &ArtifactRef("project:web".into()),
                &latest()
            ),
            "a grant in one tenant does not permit a resolution in another (ID-D3)"
        );
    }

    #[test]
    fn a_watchable_fragment_declares_the_watcher_relation() {
        let mut ns = NamespaceEngine::new();
        let frag = FragmentDef {
            object_type: ObjectType("channel".into()),
            relations: vec![RelName("member".into())],
            permissions: vec![],
        }
        .watchable();
        assert!(
            frag.is_watchable(),
            "the fragment declares the watcher relation"
        );
        assert!(matches!(ns.admit(&frag), FragmentAdmit::Admitted { .. }));
        assert!(ns.is_watchable("channel"));
        assert!(ns.has_relation("channel", WATCHER_RELATION));
        assert_eq!(ns.watchable_types(), vec!["channel".to_string()]);
        let plain = FragmentDef {
            object_type: ObjectType("secret".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![],
        };
        assert!(matches!(ns.admit(&plain), FragmentAdmit::Admitted { .. }));
        assert!(!ns.is_watchable("secret"));
        assert_eq!(ns.watchable_types(), vec!["channel".to_string()]);
    }

    #[test]
    fn watchable_is_idempotent() {
        let frag = FragmentDef {
            object_type: ObjectType("issue".into()),
            relations: vec![RelName(WATCHER_RELATION.into()), RelName("assignee".into())],
            permissions: vec![],
        }
        .watchable()
        .watchable();
        let watcher_count = frag
            .relations
            .iter()
            .filter(|r| r.0 == WATCHER_RELATION)
            .count();
        assert_eq!(
            watcher_count, 1,
            "watcher is declared exactly once (idempotent)"
        );
        assert!(frag.is_watchable());
    }

    #[test]
    fn declare_watchable_attaches_to_admitted_type_rejects_unknown() {
        let mut ns = NamespaceEngine::with_core_hierarchy();
        assert!(!ns.is_watchable("project"));
        let admit = ns.declare_watchable("project");
        assert!(
            matches!(admit, FragmentAdmit::Admitted { fragment_id } if fragment_id == "project")
        );
        assert!(ns.is_watchable("project"), "project is now watchable");
        assert!(matches!(
            ns.declare_watchable("project"),
            FragmentAdmit::Admitted { .. }
        ));
        match ns.declare_watchable("nonexistent_type") {
            FragmentAdmit::Rejected { reason } => {
                assert!(
                    reason.contains("not an admitted object type"),
                    "rejection names why: {reason}"
                )
            }
            FragmentAdmit::Admitted { .. } => panic!("an unknown type must not be made watchable"),
        }
    }

    #[test]
    fn a_too_deep_rewrite_is_rejected() {
        let mut ns = NamespaceEngine::new();
        let mut rw = Userset::Relation(RelName("reader".into()));
        for _ in 0..(MAX_RULE_DEPTH + 2) {
            rw = Userset::Union(vec![rw]);
        }
        let frag = FragmentDef {
            object_type: ObjectType("deep".into()),
            relations: vec![RelName("reader".into())],
            permissions: vec![PermissionRule {
                permission: Permission("read".into()),
                rewrite: rw,
            }],
        };
        assert!(
            matches!(ns.admit(&frag), FragmentAdmit::Rejected { .. }),
            "a rewrite nested past the admit bound is rejected (bounded schema)"
        );
    }
}
