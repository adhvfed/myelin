use myelin_query::FieldType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemeKind {
    Workflow,
    Field,
    Permission,
    Sla,
    Type,
}

impl SchemeKind {
    pub fn wire_token(self) -> &'static str {
        match self {
            SchemeKind::Workflow => "workflow",
            SchemeKind::Field => "field",
            SchemeKind::Permission => "permission",
            SchemeKind::Sla => "sla",
            SchemeKind::Type => "type",
        }
    }

    pub fn all() -> [SchemeKind; 5] {
        [
            SchemeKind::Workflow,
            SchemeKind::Field,
            SchemeKind::Permission,
            SchemeKind::Sla,
            SchemeKind::Type,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scheme {
    pub scheme_id: u128,
    pub kind: SchemeKind,
    pub name: String,
    pub body: serde_json::Value,
    pub version: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemeAssignment {
    pub kind: SchemeKind,
    pub type_id: Option<u128>,
    pub project_id: Option<u128>,
    pub team_id: Option<u128>,
    pub scheme_id: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResolveContext {
    pub type_id: u128,
    pub project_id: u128,
    pub team_id: u128,
}

const PRECEDENCE_LATTICE: [(bool, bool, bool); 8] = [
    (true, true, true),
    (true, true, false),
    (true, false, true),
    (true, false, false),
    (false, true, true),
    (false, true, false),
    (false, false, true),
    (false, false, false),
];

pub fn resolve(
    kind: SchemeKind,
    assignments: &[SchemeAssignment],
    ctx: ResolveContext,
) -> Option<u128> {
    for (bind_t, bind_p, bind_m) in PRECEDENCE_LATTICE {
        let want_type = bind_t.then_some(ctx.type_id);
        let want_project = bind_p.then_some(ctx.project_id);
        let want_team = bind_m.then_some(ctx.team_id);
        for a in assignments {
            if a.kind == kind
                && a.type_id == want_type
                && a.project_id == want_project
                && a.team_id == want_team
            {
                return Some(a.scheme_id);
            }
        }
    }
    None
}

pub fn specificity_rank(a: &SchemeAssignment) -> u8 {
    let sig = (
        a.type_id.is_some(),
        a.project_id.is_some(),
        a.team_id.is_some(),
    );
    PRECEDENCE_LATTICE
        .iter()
        .position(|r| *r == sig)
        .expect("every (Some/None)^3 signature is one of the eight lattice rows") as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResolveKey {
    pub kind: SchemeKind,
    pub ctx: ResolveContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reassignment {
    pub issue_rows_touched: u64,
    pub cache_entries_invalidated: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SchemeResolver {
    assignments: Vec<SchemeAssignment>,
    org_defaults: HashMap<SchemeKind, u128>,
    cache: HashMap<ResolveKey, u128>,
}

impl SchemeResolver {
    pub fn linear_simple() -> Self {
        let mut org_defaults = HashMap::new();
        for kind in SchemeKind::all() {
            org_defaults.insert(kind, org_default_scheme_id(kind));
        }
        SchemeResolver {
            assignments: Vec::new(),
            org_defaults,
            cache: HashMap::new(),
        }
    }

    pub fn assignment_count(&self) -> usize {
        self.assignments.len()
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    pub fn resolve(&self, kind: SchemeKind, ctx: ResolveContext) -> u128 {
        resolve(kind, &self.assignments, ctx).unwrap_or_else(|| {
            *self
                .org_defaults
                .get(&kind)
                .expect("every kind has a Linear-simple org_default")
        })
    }

    pub fn resolve_cached(&mut self, kind: SchemeKind, ctx: ResolveContext) -> u128 {
        let key = ResolveKey { kind, ctx };
        if let Some(hit) = self.cache.get(&key) {
            return *hit;
        }
        let resolved = self.resolve(kind, ctx);
        self.cache.insert(key, resolved);
        resolved
    }

    pub fn load_resolved(&mut self, kind: SchemeKind, ctx: ResolveContext) -> u128 {
        self.resolve_cached(kind, ctx)
    }

    pub fn reassign(&mut self, assignment: SchemeAssignment) -> Reassignment {
        let sig = (
            assignment.kind,
            assignment.type_id,
            assignment.project_id,
            assignment.team_id,
        );
        self.assignments
            .retain(|a| (a.kind, a.type_id, a.project_id, a.team_id) != sig);
        self.assignments.push(assignment);
        let invalidated = self.cache.len() as u64;
        self.cache.clear();
        Reassignment {
            issue_rows_touched: 0,
            cache_entries_invalidated: invalidated,
        }
    }

    pub fn unassign(
        &mut self,
        kind: SchemeKind,
        type_id: Option<u128>,
        project_id: Option<u128>,
        team_id: Option<u128>,
    ) -> Reassignment {
        let sig = (kind, type_id, project_id, team_id);
        self.assignments
            .retain(|a| (a.kind, a.type_id, a.project_id, a.team_id) != sig);
        let invalidated = self.cache.len() as u64;
        self.cache.clear();
        Reassignment {
            issue_rows_touched: 0,
            cache_entries_invalidated: invalidated,
        }
    }
}

pub fn org_default_scheme_id(kind: SchemeKind) -> u128 {
    match kind {
        SchemeKind::Workflow => 1,
        SchemeKind::Field => 2,
        SchemeKind::Permission => 3,
        SchemeKind::Sla => 4,
        SchemeKind::Type => 5,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPosture {
    Gin,
    GeneratedIndex,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlexibleField {
    pub field_id: String,
    pub field_type: FieldType,
    pub name: String,
    pub index_posture: IndexPosture,
}

impl FlexibleField {
    pub fn define(
        field_id: impl Into<String>,
        field_type: FieldType,
        name: impl Into<String>,
    ) -> Self {
        FlexibleField {
            field_id: field_id.into(),
            field_type,
            name: name.into(),
            index_posture: IndexPosture::Gin,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlexibleFieldWrite {
    pub field_id: String,
    pub value: serde_json::Value,
    pub ddl_statements: u64,
    pub gin_indexable: bool,
}

pub fn add_flexible_field(field: &FlexibleField, value: serde_json::Value) -> FlexibleFieldWrite {
    FlexibleFieldWrite {
        field_id: field.field_id.clone(),
        value,
        ddl_statements: 0,
        gin_indexable: matches!(field.index_posture, IndexPosture::Gin)
            || matches!(field.index_posture, IndexPosture::GeneratedIndex),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeSchemeBody {
    pub types: Vec<TypeDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    pub type_id: u128,
    pub name: String,
    pub rank: i16,
    pub may_parent_ranks: Vec<i16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ResolveContext {
        ResolveContext {
            type_id: 100,
            project_id: 200,
            team_id: 300,
        }
    }

    #[test]
    fn no_config_resolves_to_org_default_for_every_kind() {
        let resolver = SchemeResolver::linear_simple();
        assert_eq!(
            resolver.assignment_count(),
            0,
            "Linear-simple = zero assignments"
        );
        let ids: Vec<u128> = SchemeKind::all()
            .iter()
            .map(|k| org_default_scheme_id(*k))
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            5,
            "the five org_default ids are distinct (one per kind)"
        );
        assert_eq!(
            ids,
            vec![1, 2, 3, 4, 5],
            "the org_default sentinels are the pinned per-kind ids"
        );
        for kind in SchemeKind::all() {
            assert_eq!(
                resolver.resolve(kind, ctx()),
                org_default_scheme_id(kind),
                "kind {kind:?} resolves to its org_default with no config"
            );
        }
    }

    #[test]
    fn most_specific_wins_over_the_eight_row_lattice() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: None,
            project_id: None,
            team_id: None,
            scheme_id: 1000,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1000,
            "org override wins over default"
        );

        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 1001,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1001,
            "(T,·,·) beats (·,·,·)"
        );

        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: Some(100),
            project_id: Some(200),
            team_id: None,
            scheme_id: 1002,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1002,
            "(T,P,·) beats (T,·,·)"
        );

        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: Some(100),
            project_id: Some(200),
            team_id: Some(300),
            scheme_id: 1003,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1003,
            "(T,P,M) is most specific"
        );
    }

    #[test]
    fn the_full_lattice_order_is_total_and_fixed() {
        let sigs: [(Option<u128>, Option<u128>, Option<u128>); 8] = [
            (Some(100), Some(200), Some(300)),
            (Some(100), Some(200), None),
            (Some(100), None, Some(300)),
            (Some(100), None, None),
            (None, Some(200), Some(300)),
            (None, Some(200), None),
            (None, None, Some(300)),
            (None, None, None),
        ];
        let mut resolver = SchemeResolver::linear_simple();
        for (i, (t, p, m)) in sigs.iter().enumerate() {
            resolver.reassign(SchemeAssignment {
                kind: SchemeKind::Field,
                type_id: *t,
                project_id: *p,
                team_id: *m,
                scheme_id: 2000 + i as u128,
            });
        }
        for (i, (t, p, m)) in sigs.iter().enumerate() {
            assert_eq!(
                resolver.resolve(SchemeKind::Field, ctx()),
                2000 + i as u128,
                "lattice rank {i} is the current most-specific winner"
            );
            assert_eq!(
                specificity_rank(&SchemeAssignment {
                    kind: SchemeKind::Field,
                    type_id: *t,
                    project_id: *p,
                    team_id: *m,
                    scheme_id: 0,
                }),
                i as u8,
                "the signature occupies lattice row {i}"
            );
            resolver.unassign(SchemeKind::Field, *t, *p, *m);
        }
        assert_eq!(
            resolver.resolve(SchemeKind::Field, ctx()),
            org_default_scheme_id(SchemeKind::Field),
            "with no assignments, resolution falls to the org_default"
        );
    }

    #[test]
    fn resolution_is_per_context() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: None,
            project_id: Some(200),
            team_id: None,
            scheme_id: 5000,
        });
        assert_eq!(resolver.resolve(SchemeKind::Workflow, ctx()), 5000);
        let other = ResolveContext {
            type_id: 100,
            project_id: 999,
            team_id: 300,
        };
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, other),
            org_default_scheme_id(SchemeKind::Workflow),
            "a different project does not inherit another project's scheme"
        );
    }

    #[test]
    fn resolution_is_cached_and_deterministic() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Sla,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 7000,
        });
        assert_eq!(resolver.cache_len(), 0, "cold cache (reassign flushed it)");
        let first = resolver.resolve_cached(SchemeKind::Sla, ctx());
        assert_eq!(first, 7000);
        assert_eq!(resolver.cache_len(), 1, "the resolution is cached");
        let second = resolver.resolve_cached(SchemeKind::Sla, ctx());
        assert_eq!(
            second, first,
            "cached resolution equals the pure resolution"
        );
        assert_eq!(resolver.cache_len(), 1, "no new cache entry on a hit");
        assert_eq!(resolver.resolve(SchemeKind::Sla, ctx()), first);
    }

    #[test]
    fn the_write_path_loads_the_resolved_scheme_off_the_hot_path() {
        let mut resolver = SchemeResolver::linear_simple();
        let s1 = resolver.load_resolved(SchemeKind::Workflow, ctx());
        assert_eq!(resolver.cache_len(), 1);
        for _ in 0..1000 {
            assert_eq!(resolver.load_resolved(SchemeKind::Workflow, ctx()), s1);
        }
        assert_eq!(
            resolver.cache_len(),
            1,
            "the hot path adds 0 cache entries (all hits)"
        );
    }

    #[test]
    fn a_scheme_reassignment_touches_zero_issue_rows() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.load_resolved(SchemeKind::Workflow, ctx());
        assert_eq!(resolver.cache_len(), 1);
        let outcome = resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: None,
            project_id: Some(200),
            team_id: None,
            scheme_id: 9000,
        });
        assert_eq!(
            outcome.issue_rows_touched, 0,
            "a reassignment migrates NO data (design rule 1)"
        );
        assert_eq!(
            outcome.cache_entries_invalidated, 1,
            "the config write flushed the cache"
        );
        assert_eq!(
            resolver.cache_len(),
            0,
            "the cache is flushed (rebuilt lazily off the hot path)"
        );
        assert_eq!(resolver.resolve(SchemeKind::Workflow, ctx()), 9000);
    }

    #[test]
    fn reassign_at_the_same_slot_replaces() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Type,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 100,
        });
        assert_eq!(resolver.assignment_count(), 1);
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Type,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 200,
        });
        assert_eq!(
            resolver.assignment_count(),
            1,
            "same slot replaces, not duplicates"
        );
        assert_eq!(
            resolver.resolve(SchemeKind::Type, ctx()),
            200,
            "the replacement is in effect"
        );
    }

    #[test]
    fn the_five_kinds_match_the_check_vocabulary() {
        let tokens: Vec<&str> = SchemeKind::all().iter().map(|k| k.wire_token()).collect();
        assert_eq!(
            tokens,
            vec!["workflow", "field", "permission", "sla", "type"],
            "the five kinds are the frozen scheme.kind CHECK vocabulary (migrations.rs §3)"
        );
        for token in &tokens {
            assert!(
                crate::migrations::CREATE_SCHEME_DDL.contains(&format!("'{token}'")),
                "the scheme.kind CHECK admits `{token}`"
            );
        }
    }

    #[test]
    fn a_flexible_field_is_zero_ddl_and_gin_indexable() {
        let field = FlexibleField::define("severity", FieldType::Int, "Severity");
        assert_eq!(
            field.index_posture,
            IndexPosture::Gin,
            "a new custom field is GIN-served by default"
        );
        assert_eq!(field.field_type, FieldType::Int);

        let write = add_flexible_field(&field, serde_json::json!(3));
        assert_eq!(
            write.ddl_statements, 0,
            "a custom-field write is zero-DDL (design rule 2)"
        );
        assert!(
            write.gin_indexable,
            "the value is immediately GIN-indexable over issue_props_gin"
        );
        assert_eq!(write.value, serde_json::json!(3));

        assert!(
            crate::migrations::CREATE_ISSUE_DDL.contains("props"),
            "the props JSONB tail exists (the custom-field property bag)"
        );
        assert!(
            crate::migrations::CREATE_ISSUE_INDEXES_DDL
                .iter()
                .any(
                    |(name, ddl)| *name == crate::migrations::ISSUE_PROPS_GIN_INDEX
                        && ddl.contains("USING gin")
                ),
            "the default GIN index over props is the flexible-field index (no per-field DDL)"
        );
    }

    #[test]
    fn flexible_field_type_is_the_frozen_field_type() {
        for ft in FieldType::all() {
            let field = FlexibleField::define(format!("f_{}", ft.wire_id()), ft, ft.wire_id());
            assert_eq!(
                field.field_type, ft,
                "the custom field carries the frozen FieldType {ft:?}"
            );
        }
    }

    #[test]
    fn the_scheme_config_shape_round_trips() {
        let scheme = Scheme {
            scheme_id: 42,
            kind: SchemeKind::Workflow,
            name: "Engineering workflow".into(),
            body: serde_json::json!({
                "states": [{"name": "Todo", "category": "unstarted"}],
                "transitions": []
            }),
            version: 1,
        };
        let json = serde_json::to_string(&scheme).expect("scheme serializes");
        let back: Scheme = serde_json::from_str(&json).expect("scheme parses back");
        assert_eq!(
            back, scheme,
            "the scheme config shape round-trips byte-identically"
        );
        assert!(
            json.contains("\"workflow\""),
            "the kind serializes to its CHECK token"
        );

        let assignment = SchemeAssignment {
            kind: SchemeKind::Field,
            type_id: Some(100),
            project_id: None,
            team_id: Some(300),
            scheme_id: 42,
        };
        let ajson = serde_json::to_string(&assignment).expect("assignment serializes");
        let aback: SchemeAssignment = serde_json::from_str(&ajson).expect("assignment parses back");
        assert_eq!(aback, assignment, "the assignment shape round-trips");
    }

    #[test]
    fn the_hierarchy_is_a_tree_floor() {
        let body = TypeSchemeBody {
            types: vec![
                TypeDef {
                    type_id: 1,
                    name: "Story".into(),
                    rank: 1,
                    may_parent_ranks: vec![],
                },
                TypeDef {
                    type_id: 2,
                    name: "Epic".into(),
                    rank: 2,
                    may_parent_ranks: vec![1],
                },
            ],
        };
        assert!(
            crate::migrations::CREATE_ISSUE_DDL.contains("parent_id"),
            "the issue carries a single parent_id (the tree-hierarchy floor)"
        );
        let epic = &body.types[1];
        assert_eq!(
            epic.may_parent_ranks,
            vec![1],
            "the epic may parent rank-1 (Story) - a tree edge"
        );
    }
}
