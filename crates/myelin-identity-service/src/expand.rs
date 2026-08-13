use crate::check_engine::{CheckEngine, CheckSnapshot};
use crate::namespace::{NamespaceEngine, Userset};
use crate::reverse_index::ReverseIndex;
use crate::tuple_store::TupleStore;
use myelin_identity::{
    Consistency, ObjectId, ObjectType, Permission, PrincipalId, RelName, RewriteTrace, SubjectTree,
    Zookie,
};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeSet;

#[derive(Clone)]
pub struct Expand {
    engine: CheckEngine,
    namespace: NamespaceEngine,
    index: ReverseIndex,
}

impl Expand {
    pub fn new(tuples: TupleStore, namespace: NamespaceEngine, index: ReverseIndex) -> Expand {
        Expand {
            engine: CheckEngine::new(tuples),
            namespace,
            index,
        }
    }

    pub fn list_subjects(
        &self,
        scope: &TenantScope,
        object: &ObjectId,
        object_type: &ObjectType,
        permission: &Permission,
        at: &Consistency,
    ) -> myelin_identity::Result<SubjectTree> {
        let snapshot = self.engine.snapshot(scope, &at.at_least)?;
        let mut members: BTreeSet<String> = BTreeSet::new();
        self.expand_into(
            scope,
            &snapshot,
            &object.0,
            object_type,
            &permission.0,
            0,
            &mut members,
            &mut Vec::new(),
        );
        Ok(SubjectTree {
            object: object.clone(),
            relation: RelName(permission.0.clone()),
            members: members.into_iter().map(PrincipalId).collect(),
            zookie: self.read_zookie(scope, at),
        })
    }

    pub fn explain(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        object: &ObjectId,
        object_type: &ObjectType,
        permission: &Permission,
        at: &Consistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        let snapshot = self.engine.snapshot(scope, &at.at_least)?;
        let mut steps: Vec<String> = Vec::new();
        steps.push(format!(
            "expand {}#{} for subject {} @ {}",
            object.0,
            permission.0,
            subject.0,
            self.read_zookie(scope, at).0
        ));
        let mut members: BTreeSet<String> = BTreeSet::new();
        self.expand_into(
            scope,
            &snapshot,
            &object.0,
            object_type,
            &permission.0,
            0,
            &mut members,
            &mut steps,
        );
        let granted = members.contains(&subject.0);
        steps.push(format!(
            "{} subject {} {} in the expanded subject set ({} member(s))",
            if granted { "ALLOW -" } else { "DENY -" },
            subject.0,
            if granted { "is" } else { "is NOT" },
            members.len()
        ));
        Ok(RewriteTrace { steps })
    }

    fn read_zookie(&self, scope: &TenantScope, at: &Consistency) -> Zookie {
        if at.at_least.0.is_empty() {
            self.index.watermark(scope)
        } else {
            at.at_least.clone()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_into(
        &self,
        scope: &TenantScope,
        snapshot: &CheckSnapshot,
        object_id: &str,
        object_type: &ObjectType,
        permission: &str,
        depth: usize,
        members: &mut BTreeSet<String>,
        trace: &mut Vec<String>,
    ) {
        if depth > crate::namespace::MAX_RULE_DEPTH {
            if !trace.is_empty() {
                trace.push(format!(
                    "  [depth bound {} reached at {}#{}] - stop (fail-closed, no members added)",
                    crate::namespace::MAX_RULE_DEPTH,
                    object_id,
                    permission
                ));
            }
            return;
        }
        match self
            .namespace
            .resolve_permission(&object_type.0, permission)
        {
            Some(rewrite) => {
                if !trace.is_empty() {
                    trace.push(format!(
                        "  permission {}#{} = {} (compiled rewrite)",
                        object_id,
                        permission,
                        describe_userset(&rewrite)
                    ));
                }
                self.expand_userset(
                    scope,
                    snapshot,
                    object_id,
                    object_type,
                    &rewrite,
                    depth,
                    members,
                    trace,
                );
            }
            None => {
                self.expand_direct_relation(
                    scope,
                    snapshot,
                    object_id,
                    object_type,
                    permission,
                    depth,
                    members,
                    trace,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_userset(
        &self,
        scope: &TenantScope,
        snapshot: &CheckSnapshot,
        object_id: &str,
        object_type: &ObjectType,
        rewrite: &Userset,
        depth: usize,
        members: &mut BTreeSet<String>,
        trace: &mut Vec<String>,
    ) {
        if depth > crate::namespace::MAX_RULE_DEPTH {
            return;
        }
        match rewrite {
            Userset::Relation(r) => {
                self.expand_direct_relation(
                    scope,
                    snapshot,
                    object_id,
                    object_type,
                    &r.0,
                    depth,
                    members,
                    trace,
                );
            }
            Userset::Union(arms) => {
                if !trace.is_empty() {
                    trace.push(format!("  union of {} arm(s) on {}", arms.len(), object_id));
                }
                for arm in arms {
                    self.expand_userset(
                        scope,
                        snapshot,
                        object_id,
                        object_type,
                        arm,
                        depth + 1,
                        members,
                        trace,
                    );
                }
            }
            Userset::Intersect(arms) => {
                if !trace.is_empty() {
                    trace.push(format!(
                        "  intersect of {} arm(s) on {}",
                        arms.len(),
                        object_id
                    ));
                }
                let mut acc: Option<BTreeSet<String>> = None;
                for arm in arms {
                    let mut arm_set: BTreeSet<String> = BTreeSet::new();
                    self.expand_userset(
                        scope,
                        snapshot,
                        object_id,
                        object_type,
                        arm,
                        depth + 1,
                        &mut arm_set,
                        trace,
                    );
                    acc = Some(match acc {
                        None => arm_set,
                        Some(prev) => prev.intersection(&arm_set).cloned().collect(),
                    });
                }
                if let Some(common) = acc {
                    members.extend(common);
                }
            }
            Userset::Exclusion { base, subtracted } => {
                if !trace.is_empty() {
                    trace.push(format!("  exclusion (base − subtracted) on {}", object_id));
                }
                let mut base_set: BTreeSet<String> = BTreeSet::new();
                self.expand_userset(
                    scope,
                    snapshot,
                    object_id,
                    object_type,
                    base,
                    depth + 1,
                    &mut base_set,
                    trace,
                );
                let mut sub_set: BTreeSet<String> = BTreeSet::new();
                self.expand_userset(
                    scope,
                    snapshot,
                    object_id,
                    object_type,
                    subtracted,
                    depth + 1,
                    &mut sub_set,
                    trace,
                );
                members.extend(base_set.difference(&sub_set).cloned());
            }
            Userset::TupleToUserset { tupleset, computed } => {
                if !trace.is_empty() {
                    trace.push(format!(
                        "  inherit {}->{} on {} (tuple-to-userset)",
                        tupleset.0, computed.0, object_id
                    ));
                }
                let object_ref = ArtifactRef(object_id.to_string());
                let parents = snapshot.direct_subjects(&object_ref, tupleset);
                for parent_subject in parents {
                    match crate::check_engine::parse_userset(&parent_subject) {
                        Some((parent_id, parent_rel)) if parent_rel == computed.0 => {
                            let parent_type = ObjectType(type_of_object_id(parent_id));
                            self.expand_into(
                                scope,
                                snapshot,
                                parent_id,
                                &parent_type,
                                &computed.0,
                                depth + 1,
                                members,
                                trace,
                            );
                        }
                        _ => {
                            if !parent_subject.contains(crate::check_engine::USERSET_SEP) {
                                members.insert(parent_subject);
                            }
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_direct_relation(
        &self,
        scope: &TenantScope,
        snapshot: &CheckSnapshot,
        object_id: &str,
        object_type: &ObjectType,
        relation: &str,
        depth: usize,
        members: &mut BTreeSet<String>,
        trace: &mut Vec<String>,
    ) {
        if depth > crate::namespace::MAX_RULE_DEPTH {
            return;
        }
        let rel = RelName(relation.to_string());

        let direct = self.index.subjects_for(scope, object_type, object_id, &rel);
        let direct_count = direct.len();
        for s in direct {
            members.insert(s.0);
        }

        let object_ref = ArtifactRef(object_id.to_string());
        let snapshot_subjects = snapshot.direct_subjects(&object_ref, &rel);
        let mut userset_count = 0usize;
        for s in snapshot_subjects {
            if let Some((obj2, rel2)) = crate::check_engine::parse_userset(&s) {
                userset_count += 1;
                let obj2_type = ObjectType(type_of_object_id(obj2));
                self.expand_into(
                    scope,
                    snapshot,
                    obj2,
                    &obj2_type,
                    rel2,
                    depth + 1,
                    members,
                    trace,
                );
            }
        }

        if !trace.is_empty() {
            trace.push(format!(
                "  relation {}#{}: {} direct subject(s) via S8 + {} inherited userset(s)",
                object_id, relation, direct_count, userset_count
            ));
        }
    }
}

fn describe_userset(u: &Userset) -> String {
    match u {
        Userset::Relation(r) => r.0.clone(),
        Userset::Union(arms) => format!(
            "({})",
            arms.iter()
                .map(describe_userset)
                .collect::<Vec<_>>()
                .join(" ∪ ")
        ),
        Userset::Intersect(arms) => format!(
            "({})",
            arms.iter()
                .map(describe_userset)
                .collect::<Vec<_>>()
                .join(" ∩ ")
        ),
        Userset::Exclusion { base, subtracted } => {
            format!(
                "({} − {})",
                describe_userset(base),
                describe_userset(subtracted)
            )
        }
        Userset::TupleToUserset { tupleset, computed } => {
            format!("{}->{}", tupleset.0, computed.0)
        }
    }
}

fn type_of_object_id(object_id: &str) -> String {
    object_id
        .split_once(':')
        .map(|(ty, _)| ty.to_string())
        .unwrap_or_else(|| object_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::NamespaceEngine;
    use crate::reverse_index::{ReverseIndexConsumer, ReverseRow};
    use myelin_events::{
        BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp,
    };
    use myelin_identity::{ConsistencyMode, Principal, PrincipalKind, RelationTuple, TupleDelta};
    use myelin_tenancy::{Region, TenantId};

    fn scope(tenant: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn actor_in(tenant: &str) -> Principal {
        Principal::stub(
            PrincipalId("p-admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
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

    fn now() -> Timestamp {
        Timestamp("2026-06-19T00:00:00Z".into())
    }

    fn latest() -> Consistency {
        Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn seed(
        scope: &TenantScope,
        deltas: &[TupleDelta],
    ) -> (TupleStore, ReverseIndex, NamespaceEngine) {
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        store
            .write_tuples(
                scope,
                &actor_in(&scope.tenant().0),
                deltas,
                None,
                None,
                now(),
            )
            .expect("seed write");
        let bus = InProcessBus::new();
        let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
        relay.drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
        (store, index, NamespaceEngine::with_core_hierarchy())
    }

    #[test]
    fn list_subjects_expands_direct_relation_via_s8() {
        let s = scope("acme");
        let (store, index, ns) = seed(
            &s,
            &[
                add("channel:general", "watcher", "p:alice"),
                add("channel:general", "watcher", "p:bob"),
                add("channel:general", "watcher", "p:carol"),
                add("channel:random", "watcher", "p:dave"),
            ],
        );
        let expand = Expand::new(store, ns, index);
        let tree = expand
            .list_subjects(
                &s,
                &ObjectId("channel:general".into()),
                &ObjectType("channel".into()),
                &Permission("watcher".into()),
                &latest(),
            )
            .expect("read channel watcher relationships");
        let got: Vec<String> = tree.members.iter().map(|m| m.0.clone()).collect();
        assert_eq!(
            got,
            vec!["p:alice".to_string(), "p:bob".into(), "p:carol".into()],
            "list_subjects returns the channel's direct watchers (and only them), via S8"
        );
        assert_eq!(tree.object, ObjectId("channel:general".into()));
        assert_eq!(tree.relation, RelName("watcher".into()));
    }

    #[test]
    fn list_subjects_expands_compiled_permission_with_inheritance() {
        let s = scope("acme");
        let (store, index, ns) = seed(
            &s,
            &[
                add("project:web", "reader", "p:reader"),
                add("project:web", "writer", "p:writer"),
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:teammember"),
            ],
        );
        let expand = Expand::new(store, ns, index);
        let tree = expand
            .list_subjects(
                &s,
                &ObjectId("project:web".into()),
                &ObjectType("project".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read inherited project relationships");
        let got: BTreeSet<String> = tree.members.iter().map(|m| m.0.clone()).collect();
        assert!(
            got.contains("p:reader"),
            "the direct reader is a view subject"
        );
        assert!(
            got.contains("p:writer"),
            "the direct writer is a view subject"
        );
        assert!(
            got.contains("p:teammember"),
            "the parent-team member inherits view (parent_team->view) and is a subject"
        );
    }

    #[test]
    fn list_subjects_honours_exclusion() {
        let s = scope("acme");
        let outbox = OutboxStore::new();
        let store = TupleStore::new(outbox.clone());
        let index = ReverseIndex::new();
        let consumer = ReverseIndexConsumer::new(index.clone());
        let mut ns = NamespaceEngine::new();
        let frag = crate::namespace::FragmentDef {
            object_type: ObjectType("doc".into()),
            relations: vec![RelName("reader".into()), RelName("blocked".into())],
            permissions: vec![crate::namespace::PermissionRule {
                permission: Permission("view".into()),
                rewrite: Userset::Exclusion {
                    base: Box::new(Userset::Relation(RelName("reader".into()))),
                    subtracted: Box::new(Userset::Relation(RelName("blocked".into()))),
                },
            }],
        };
        assert!(matches!(
            ns.admit(&frag),
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
        store
            .write_tuples(
                &s,
                &actor_in("acme"),
                &[
                    add("doc:1", "reader", "p:alice"),
                    add("doc:1", "reader", "p:bob"),
                    add("doc:1", "blocked", "p:bob"),
                ],
                None,
                None,
                now(),
            )
            .expect("seed");
        let bus = InProcessBus::new();
        Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into())).drain_to_empty();
        for env in bus.consume("") {
            consumer.handle(&env, &mut myelin_events::HandlerTx::none());
        }
        let expand = Expand::new(store, ns, index);
        let tree = expand
            .list_subjects(
                &s,
                &ObjectId("doc:1".into()),
                &ObjectType("doc".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read document relationships");
        let got: BTreeSet<String> = tree.members.iter().map(|m| m.0.clone()).collect();
        assert!(
            got.contains("p:alice"),
            "an un-blocked reader is a view subject"
        );
        assert!(
            !got.contains("p:bob"),
            "a blocked reader is excluded (view = reader − blocked)"
        );
    }

    #[test]
    fn explain_returns_non_empty_correct_trace_for_allow() {
        let s = scope("acme");
        let (store, index, ns) = seed(
            &s,
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
        );
        let expand = Expand::new(store, ns, index);
        let trace = expand
            .explain(
                &s,
                &PrincipalId("p:alice".into()),
                &ObjectId("project:web".into()),
                &ObjectType("project".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read relationships for the allow explanation");
        assert!(!trace.steps.is_empty(), "the trace is non-empty");
        assert!(
            trace.steps.last().unwrap().starts_with("ALLOW"),
            "alice (a parent-team member) resolves to ALLOW: {:?}",
            trace.steps
        );
        assert!(
            trace
                .steps
                .iter()
                .any(|st| st.contains("parent_team->view")),
            "the trace records the parent_team->view inheritance edge: {:?}",
            trace.steps
        );
    }

    #[test]
    fn explain_denies_non_member_never_silent_allow() {
        let s = scope("acme");
        let (store, index, ns) = seed(
            &s,
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
        );
        let expand = Expand::new(store, ns, index);
        let trace = expand
            .explain(
                &s,
                &PrincipalId("p:bob".into()),
                &ObjectId("project:web".into()),
                &ObjectType("project".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read relationships for the deny explanation");
        assert!(!trace.steps.is_empty(), "the deny trace is non-empty");
        assert!(
            trace.steps.last().unwrap().starts_with("DENY"),
            "a non-member resolves to DENY (never a silent allow): {:?}",
            trace.steps
        );
    }

    #[test]
    fn relationship_outage_is_neither_an_empty_subject_set_nor_a_deny_explanation() {
        let s = scope("acme");
        let store = TupleStore::new(OutboxStore::new())
            .with_unavailable_reads("relationship database is offline");
        let expand = Expand::new(
            store,
            NamespaceEngine::with_core_hierarchy(),
            ReverseIndex::new(),
        );
        let object = ObjectId("project:web".into());
        let object_type = ObjectType("project".into());
        let permission = Permission("view".into());

        let subjects = expand.list_subjects(&s, &object, &object_type, &permission, &latest());
        let explanation = expand.explain(
            &s,
            &PrincipalId("p:alice".into()),
            &object,
            &object_type,
            &permission,
            &latest(),
        );

        assert!(
            matches!(subjects, Err(myelin_identity::AuthzError::Unavailable(_))),
            "an unavailable relationship snapshot is not an empty subject set"
        );
        assert!(
            matches!(
                explanation,
                Err(myelin_identity::AuthzError::Unavailable(_))
            ),
            "an unavailable relationship snapshot cannot fabricate a DENY explanation"
        );
    }

    #[test]
    fn no_cross_tenant_list_subjects() {
        let acme = scope("acme");
        let (store, index, ns) = seed(&acme, &[add("channel:general", "watcher", "p:alice")]);
        let expand = Expand::new(store, ns, index);
        let globex = scope("globex");
        let tree = expand
            .list_subjects(
                &globex,
                &ObjectId("channel:general".into()),
                &ObjectType("channel".into()),
                &Permission("watcher".into()),
                &latest(),
            )
            .expect("read the other tenant's relationship partition");
        assert!(
            tree.members.is_empty(),
            "an acme channel's watchers are invisible to a globex expand (0 cross-tenant subjects)"
        );
    }

    #[test]
    fn list_subjects_is_depth_bounded() {
        let s = scope("acme");
        let n = crate::namespace::MAX_RULE_DEPTH + 4;
        let mut deltas: Vec<TupleDelta> = Vec::new();
        for i in 0..n {
            deltas.push(add(
                &format!("project:level_{i}"),
                "parent_team",
                &format!("project:level_{}#view", i + 1),
            ));
        }
        deltas.push(add(&format!("project:level_{n}"), "reader", "p:deep"));
        let (store, index, ns) = seed(&s, &deltas);
        let expand = Expand::new(store, ns, index);
        let tree = expand
            .list_subjects(
                &s,
                &ObjectId("project:level_0".into()),
                &ObjectType("project".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read the depth-bounded relationship graph");
        assert!(
            !tree.members.iter().any(|m| m.0 == "p:deep"),
            "a member beyond the depth bound is NOT expanded (fail-closed, never an unbounded scan)"
        );
    }

    #[test]
    fn inheritance_edge_requires_matching_computed_relation() {
        let s = scope("acme");
        let (store, index, ns) = seed(
            &s,
            &[
                add("project:web", "parent_team", "team:eng#member"),
                add("team:eng", "member", "p:teammember"),
            ],
        );
        let expand = Expand::new(store, ns, index);
        let tree = expand
            .list_subjects(
                &s,
                &ObjectId("project:web".into()),
                &ObjectType("project".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read the mismatched inheritance relationship graph");
        assert!(
            !tree.members.iter().any(|m| m.0 == "p:teammember"),
            "an inheritance edge whose computed relation (member) ≠ the rewrite's (view) is NOT \
             followed - no leak (the match-guard is mandatory-core)"
        );
    }

    #[test]
    fn explain_trace_records_each_operator_step() {
        let s = scope("acme");
        let (store, index, ns) = seed(
            &s,
            &[
                add("project:web", "reader", "p:reader"),
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:alice"),
            ],
        );
        let expand = Expand::new(store, ns, index);
        let trace = expand
            .explain(
                &s,
                &PrincipalId("p:alice".into()),
                &ObjectId("project:web".into()),
                &ObjectType("project".into()),
                &Permission("view".into()),
                &latest(),
            )
            .expect("read relationships for the operator trace");
        let joined = trace.steps.join("\n");
        assert!(
            joined.contains("union of"),
            "the trace names the union operator: {joined}"
        );
        assert!(
            joined.contains("parent_team->view"),
            "the trace names the inheritance edge: {joined}"
        );
        assert!(
            joined.contains("direct subject(s) via S8"),
            "the trace records the S8 density lookup of a direct relation: {joined}"
        );
    }

    #[test]
    fn list_subjects_50k_member_density_within_budget() {
        use std::time::Instant;
        let s = scope("acme");
        let index = ReverseIndex::new();
        const MEMBERS: usize = 50_000;
        let z = Zookie("zk-00000000000000000001".into());
        for i in 0..MEMBERS {
            index.apply_delta(
                &s,
                "add",
                &ObjectType("channel".into()),
                ReverseRow {
                    subject: PrincipalId(format!("p:user{i:06}")),
                    relation: RelName("watcher".into()),
                    object_id: ObjectId("channel:huge".into()),
                },
                &z,
            );
        }
        let store = TupleStore::new(OutboxStore::new());
        let expand = Expand::new(store, NamespaceEngine::with_core_hierarchy(), index);

        let start = Instant::now();
        let tree = expand
            .list_subjects(
                &s,
                &ObjectId("channel:huge".into()),
                &ObjectType("channel".into()),
                &Permission("watcher".into()),
                &latest(),
            )
            .expect("read the high-density watcher relationships");
        let elapsed_ms = start.elapsed().as_millis();

        assert_eq!(
            tree.members.len(),
            MEMBERS,
            "the 50k-member channel expands to all 50k watchers (served by S8)"
        );
        const DENSITY_BUDGET_MS: u128 = 250;
        if myelin_substrate::perf_budget_enforced() {
            assert!(
                elapsed_ms < DENSITY_BUDGET_MS,
                "50k-density list_subjects took {elapsed_ms} ms, over the {DENSITY_BUDGET_MS} ms budget \
                 (it must be served by S8 at density, not a per-member scan)"
            );
        }
    }
}
