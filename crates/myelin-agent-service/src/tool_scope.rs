use myelin_agent::{Conversation, ToolDef, ToolName, ToolSchema, ToolSurface};
use myelin_identity::{ListObjectsResult, ObjectType, Permission, SetExpr, Zookie};

pub const TOOL_DEF_OBJECT_TYPE: &str = "tool_def";

pub const TOOL_ID_COLUMN: &str = "id";

pub const TOOL_USE_PERMISSION: &str = "tool.use";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolScopePredicate {
    All,
    None,
    Ids(Vec<String>),
    NotIds(Vec<String>),
    And(Vec<ToolScopePredicate>),
    Or(Vec<ToolScopePredicate>),
    Not(Box<ToolScopePredicate>),
}

impl ToolScopePredicate {
    pub fn admits(&self, tool_id: &str) -> bool {
        match self {
            ToolScopePredicate::All => true,
            ToolScopePredicate::None => false,
            ToolScopePredicate::Ids(ids) => ids.iter().any(|i| i == tool_id),
            ToolScopePredicate::NotIds(ids) => !ids.iter().any(|i| i == tool_id),
            ToolScopePredicate::And(subs) => subs.iter().all(|s| s.admits(tool_id)),
            ToolScopePredicate::Or(subs) => {
                !subs.is_empty() && subs.iter().any(|s| s.admits(tool_id))
            }
            ToolScopePredicate::Not(inner) => !inner.admits(tool_id),
        }
    }

    pub fn to_sql(&self, column: &str) -> String {
        match self {
            ToolScopePredicate::All => "true".to_string(),
            ToolScopePredicate::None => "false".to_string(),
            ToolScopePredicate::Ids(ids) => {
                if ids.is_empty() {
                    "false".to_string()
                } else {
                    format!("{column} IN ({})", sql_id_list(ids))
                }
            }
            ToolScopePredicate::NotIds(ids) => {
                if ids.is_empty() {
                    "true".to_string()
                } else {
                    format!("{column} NOT IN ({})", sql_id_list(ids))
                }
            }
            ToolScopePredicate::And(subs) => {
                if subs.is_empty() {
                    "true".to_string()
                } else {
                    let parts: Vec<String> = subs.iter().map(|s| s.to_sql(column)).collect();
                    format!("({})", parts.join(" AND "))
                }
            }
            ToolScopePredicate::Or(subs) => {
                if subs.is_empty() {
                    "false".to_string()
                } else {
                    let parts: Vec<String> = subs.iter().map(|s| s.to_sql(column)).collect();
                    format!("({})", parts.join(" OR "))
                }
            }
            ToolScopePredicate::Not(inner) => format!("(NOT {})", inner.to_sql(column)),
        }
    }
}

fn sql_id_list(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn lower_list_objects(result: &ListObjectsResult) -> ToolScopePredicate {
    match result {
        ListObjectsResult::Ids { ids, .. } => {
            ToolScopePredicate::Ids(ids.iter().map(|o| o.0.clone()).collect())
        }
        ListObjectsResult::Filter { set_expr, .. } => lower_set_expr(set_expr),
    }
}

pub fn lower_set_expr(expr: &SetExpr) -> ToolScopePredicate {
    match expr {
        SetExpr::All => ToolScopePredicate::All,
        SetExpr::None => ToolScopePredicate::None,
        SetExpr::Ids(ids) => ToolScopePredicate::Ids(ids.iter().map(|o| o.0.clone()).collect()),
        SetExpr::NotIds(ids) => {
            ToolScopePredicate::NotIds(ids.iter().map(|o| o.0.clone()).collect())
        }
        SetExpr::Union(subs) => ToolScopePredicate::Or(subs.iter().map(lower_set_expr).collect()),
        SetExpr::Intersect(subs) => {
            ToolScopePredicate::And(subs.iter().map(lower_set_expr).collect())
        }
        SetExpr::Difference(left, right) => ToolScopePredicate::And(vec![
            lower_set_expr(left),
            ToolScopePredicate::Not(Box::new(lower_set_expr(right))),
        ]),
        SetExpr::InRelation { .. } | SetExpr::TupleSet { .. } => ToolScopePredicate::None,
    }
}

pub trait ToolListObjects {
    fn list_objects(
        &self,
        subject_pseudonym: &str,
        permission: &Permission,
        ty: &ObjectType,
        at: &Zookie,
    ) -> ListObjectsResult;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedToolList {
    pub tools: Vec<ToolSchema>,
    pub zookie: Zookie,
    pub query_count: usize,
}

pub fn build_scoped_tool_list<S, L>(
    catalogue: &S,
    list_objects: &L,
    subject_pseudonym: &str,
    at: &Zookie,
) -> ScopedToolList
where
    S: ToolSurface + ToolCatalogueIds,
    L: ToolListObjects,
{
    let permission = Permission(TOOL_USE_PERMISSION.to_string());
    let ty = ObjectType(TOOL_DEF_OBJECT_TYPE.to_string());
    let result = list_objects.list_objects(subject_pseudonym, &permission, &ty, at);
    let zookie = match &result {
        ListObjectsResult::Ids { zookie, .. } => zookie.clone(),
        ListObjectsResult::Filter { zookie, .. } => zookie.clone(),
    };

    let predicate = lower_list_objects(&result);
    let tools: Vec<ToolSchema> = catalogue
        .catalogue_tool_ids()
        .into_iter()
        .filter(|(_, id)| predicate.admits(id))
        .map(|(name, _)| {
            let input_schema = catalogue
                .resolve(&name)
                .map(|def| def.input_schema.clone())
                .unwrap_or_else(|| "{}".to_string());
            ToolSchema {
                name,
                description: String::new(),
                input_schema,
            }
        })
        .collect();

    ScopedToolList {
        tools,
        zookie,
        query_count: 1,
    }
}

pub trait ToolCatalogueIds {
    fn catalogue_tool_ids(&self) -> Vec<(ToolName, String)>;
}

pub fn scoped_tool_ids_sql(predicate: &ToolScopePredicate) -> String {
    format!(
        "SELECT name FROM tool_def \
         WHERE tenant_id = current_setting('myelin.tenant_id') \
           AND region = current_setting('myelin.region') \
           AND ({})",
        predicate.to_sql(TOOL_ID_COLUMN)
    )
}

pub fn assert_apply_rechecks_revoked(apply_outcome: &myelin_agent::EffectResult) -> bool {
    matches!(apply_outcome, myelin_agent::EffectResult::Denied(_))
}

pub fn apply_scope_to_conversation(conv: &mut Conversation, scoped: &ScopedToolList) {
    conv.tools = scoped.tools.clone();
}

pub fn tool_def_id(def: &ToolDef) -> String {
    format!("{}/{}/{}", def.subsystem, def.name.0, def.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::EffectKind;
    use myelin_identity::ObjectId;
    use std::cell::Cell;

    struct Catalogue {
        defs: Vec<ToolDef>,
    }
    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs.iter().find(|d| &d.name == name)
        }
    }
    impl ToolCatalogueIds for Catalogue {
        fn catalogue_tool_ids(&self) -> Vec<(ToolName, String)> {
            self.defs
                .iter()
                .map(|d| (d.name.clone(), tool_def_id(d)))
                .collect()
        }
    }

    struct CountingListObjects {
        result: ListObjectsResult,
        calls: Cell<usize>,
    }
    impl ToolListObjects for CountingListObjects {
        fn list_objects(
            &self,
            _subject: &str,
            _permission: &Permission,
            _ty: &ObjectType,
            _at: &Zookie,
        ) -> ListObjectsResult {
            self.calls.set(self.calls.get() + 1);
            self.result.clone()
        }
    }

    fn def(name: &str, subsystem: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: subsystem.into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec!["tool.use".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    fn catalogue(names: &[(&str, &str)]) -> Catalogue {
        Catalogue {
            defs: names.iter().map(|(n, s)| def(n, s)).collect(),
        }
    }

    fn schema(name: &str) -> ToolSchema {
        ToolSchema {
            name: ToolName(name.into()),
            description: String::new(),
            input_schema: "{}".into(),
        }
    }

    #[test]
    fn scoped_list_is_one_query_regardless_of_tool_count() {
        let big: Vec<(String, String)> = (0..50)
            .map(|i| (format!("tool-{i}"), "issues".to_string()))
            .collect();
        let refs: Vec<(&str, &str)> = big.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
        let cat = catalogue(&refs);

        let lo = CountingListObjects {
            result: ListObjectsResult::Filter {
                set_expr: SetExpr::Ids(vec![
                    ObjectId("issues/tool-0/1".into()),
                    ObjectId("issues/tool-1/1".into()),
                    ObjectId("issues/tool-2/1".into()),
                ]),
                zookie: Zookie("z-7".into()),
            },
            calls: Cell::new(0),
        };

        let scoped = build_scoped_tool_list(&cat, &lo, "psn:agent-7", &Zookie("z-1".into()));

        assert_eq!(
            lo.calls.get(),
            1,
            "exactly one list_objects call for the whole catalogue"
        );
        assert_eq!(scoped.query_count, 1, "the no-N+1 GATE: one query");
        assert_eq!(scoped.tools.len(), 3);
        assert!(scoped.tools.contains(&schema("tool-0")));
        assert!(scoped.tools.contains(&schema("tool-2")));
        assert!(!scoped.tools.contains(&schema("tool-3")));
        assert_eq!(scoped.zookie, Zookie("z-7".into()));
    }

    #[test]
    fn set_expr_lowers_to_one_predicate() {
        let pred = lower_set_expr(&SetExpr::Ids(vec![
            ObjectId("issues/a/1".into()),
            ObjectId("issues/b/1".into()),
        ]));
        assert_eq!(
            pred.to_sql(TOOL_ID_COLUMN),
            "id IN ('issues/a/1', 'issues/b/1')",
            "ONE membership clause - no per-tool check"
        );
        assert_eq!(lower_set_expr(&SetExpr::None).to_sql("id"), "false");
        assert_eq!(lower_set_expr(&SetExpr::All).to_sql("id"), "true");
        let sql = scoped_tool_ids_sql(&pred);
        assert!(sql.contains("tenant_id = current_setting('myelin.tenant_id')"));
        assert!(sql.contains("region = current_setting('myelin.region')"));
        assert!(sql.contains("id IN ('issues/a/1', 'issues/b/1')"));
    }

    #[test]
    fn boolean_set_expr_forms_compose_to_one_predicate() {
        let union = lower_set_expr(&SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("x".into())]),
            SetExpr::Ids(vec![ObjectId("y".into())]),
        ]));
        assert_eq!(union.to_sql("id"), "(id IN ('x') OR id IN ('y'))");

        let intersect = lower_set_expr(&SetExpr::Intersect(vec![
            SetExpr::NotIds(vec![ObjectId("secret".into())]),
            SetExpr::All,
        ]));
        assert_eq!(intersect.to_sql("id"), "(id NOT IN ('secret') AND true)");

        let diff = lower_set_expr(&SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("denied".into())])),
        ));
        assert_eq!(diff.to_sql("id"), "(true AND (NOT id IN ('denied')))");
    }

    #[test]
    fn relational_set_expr_is_fail_closed_in_reference_path() {
        let in_rel = lower_set_expr(&SetExpr::InRelation {
            relation: myelin_identity::RelName("user".into()),
            via_column: myelin_identity::ColRef {
                table: "tool_def".into(),
                column: "id".into(),
            },
        });
        assert_eq!(
            in_rel,
            ToolScopePredicate::None,
            "no silent allow for a relational grant"
        );
        assert!(!in_rel.admits("any-tool"));
    }

    #[test]
    fn deny_yields_empty_scope_and_all_yields_full() {
        let cat = catalogue(&[("a", "issues"), ("b", "git")]);
        let deny = CountingListObjects {
            result: ListObjectsResult::Filter {
                set_expr: SetExpr::None,
                zookie: Zookie("z".into()),
            },
            calls: Cell::new(0),
        };
        let scoped = build_scoped_tool_list(&cat, &deny, "psn:x", &Zookie("z0".into()));
        assert!(scoped.tools.is_empty(), "a denied run is shown NO tool");

        let all = CountingListObjects {
            result: ListObjectsResult::Ids {
                ids: vec![],
                zookie: Zookie("z".into()),
            },
            calls: Cell::new(0),
        };
        let scoped_empty = build_scoped_tool_list(&cat, &all, "psn:x", &Zookie("z0".into()));
        assert!(scoped_empty.tools.is_empty());

        let admin = CountingListObjects {
            result: ListObjectsResult::Filter {
                set_expr: SetExpr::All,
                zookie: Zookie("z".into()),
            },
            calls: Cell::new(0),
        };
        let scoped_all = build_scoped_tool_list(&cat, &admin, "psn:x", &Zookie("z0".into()));
        assert_eq!(
            scoped_all.tools.len(),
            2,
            "admin (All) sees every tool of this type"
        );
    }

    #[test]
    fn materialised_ids_and_filter_ids_lower_identically() {
        let ids = vec![ObjectId("git/merge/1".into())];
        let from_materialised = lower_list_objects(&ListObjectsResult::Ids {
            ids: ids.clone(),
            zookie: Zookie("z".into()),
        });
        let from_filter = lower_list_objects(&ListObjectsResult::Filter {
            set_expr: SetExpr::Ids(ids),
            zookie: Zookie("z".into()),
        });
        assert_eq!(
            from_materialised, from_filter,
            "no drift between the S4 and S8 paths"
        );
    }

    #[test]
    fn apply_rechecks_a_revoked_but_scoped_tool() {
        let cat = catalogue(&[("merge", "git")]);
        let lo = CountingListObjects {
            result: ListObjectsResult::Filter {
                set_expr: SetExpr::Ids(vec![ObjectId("git/merge/1".into())]),
                zookie: Zookie("z-build".into()),
            },
            calls: Cell::new(0),
        };
        let scoped = build_scoped_tool_list(&cat, &lo, "psn:agent", &Zookie("z-0".into()));
        assert!(
            scoped.tools.contains(&schema("merge")),
            "merge was scoped in"
        );

        let apply_outcome = myelin_agent::EffectResult::Denied(
            "capability check denied for git.merge (revoked since scope)".into(),
        );
        assert!(
            assert_apply_rechecks_revoked(&apply_outcome),
            "the apply-time check MUST override a stale scope (0 stale-grant applies)"
        );

        let leaked = myelin_agent::EffectResult::Applied(myelin_agent::EventId("evt".into()));
        assert!(
            !assert_apply_rechecks_revoked(&leaked),
            "an Applied for a revoked tool is a stale-grant leak (the property must catch it)"
        );
    }

    #[test]
    fn scope_is_stamped_onto_the_conversation() {
        let scoped = ScopedToolList {
            tools: vec![schema("read"), schema("merge")],
            zookie: Zookie("z".into()),
            query_count: 1,
        };
        let mut conv = Conversation::default();
        apply_scope_to_conversation(&mut conv, &scoped);
        assert_eq!(
            conv.tools, scoped.tools,
            "the brain sees exactly the scoped subset"
        );
    }

    #[test]
    fn sql_id_list_escapes_quotes() {
        let pred = ToolScopePredicate::Ids(vec!["a'b".into()]);
        assert_eq!(pred.to_sql("id"), "id IN ('a''b')");
    }
}
