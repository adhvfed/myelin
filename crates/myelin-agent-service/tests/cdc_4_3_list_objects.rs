use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSchema, ToolSurface};
use myelin_agent_service::{
    build_scoped_tool_list, lower_list_objects, tool_def_id, ToolCatalogueIds, ToolListObjects,
    ToolScopePredicate, TOOL_DEF_OBJECT_TYPE, TOOL_USE_PERMISSION,
};
use myelin_identity::{ListObjectsResult, ObjectId, ObjectType, Permission, SetExpr, Zookie};
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

struct ListObjectsProvider {
    result: ListObjectsResult,
    calls: Cell<usize>,
    last_permission: std::cell::RefCell<Option<String>>,
    last_type: std::cell::RefCell<Option<String>>,
    last_at: std::cell::RefCell<Option<String>>,
}
impl ToolListObjects for ListObjectsProvider {
    fn list_objects(
        &self,
        _subject: &str,
        permission: &Permission,
        ty: &ObjectType,
        at: &Zookie,
    ) -> ListObjectsResult {
        self.calls.set(self.calls.get() + 1);
        *self.last_permission.borrow_mut() = Some(permission.0.clone());
        *self.last_type.borrow_mut() = Some(ty.0.clone());
        *self.last_at.borrow_mut() = Some(at.0.clone());
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

fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: ToolName(name.into()),
        description: String::new(),
        input_schema: "{}".into(),
    }
}

#[test]
fn consumer_scopes_over_tool_def_in_one_call_and_carries_the_zookie() {
    let cat = Catalogue {
        defs: vec![
            def("merge", "git"),
            def("close", "issues"),
            def("deploy", "ci"),
        ],
    };
    let provider = ListObjectsProvider {
        result: ListObjectsResult::Filter {
            set_expr: SetExpr::Ids(vec![
                ObjectId("git/merge/1".into()),
                ObjectId("issues/close/1".into()),
            ]),
            zookie: Zookie("z-rev-42".into()),
        },
        calls: Cell::new(0),
        last_permission: std::cell::RefCell::new(None),
        last_type: std::cell::RefCell::new(None),
        last_at: std::cell::RefCell::new(None),
    };

    let scoped = build_scoped_tool_list(&cat, &provider, "psn:agent-7", &Zookie("z-run".into()));

    assert_eq!(
        provider.calls.get(),
        1,
        "the consumer issues ONE list_objects call"
    );
    assert_eq!(scoped.query_count, 1);
    assert_eq!(
        provider.last_type.borrow().as_deref(),
        Some(TOOL_DEF_OBJECT_TYPE),
        "the consumer scopes over the tool_def object type"
    );
    assert_eq!(
        provider.last_permission.borrow().as_deref(),
        Some(TOOL_USE_PERMISSION),
        "the consumer scopes with the tool.use permission"
    );
    assert_eq!(provider.last_at.borrow().as_deref(), Some("z-run"));
    assert_eq!(scoped.zookie, Zookie("z-rev-42".into()));
    assert_eq!(scoped.tools.len(), 2);
    assert!(scoped.tools.contains(&schema("merge")));
    assert!(scoped.tools.contains(&schema("close")));
    assert!(!scoped.tools.contains(&schema("deploy")));
}

#[test]
fn filter_push_down_lowers_to_one_predicate() {
    let result = ListObjectsResult::Filter {
        set_expr: SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("git/merge/1".into())]),
            SetExpr::NotIds(vec![ObjectId("ci/deploy/1".into())]),
        ]),
        zookie: Zookie("z".into()),
    };
    let pred = lower_list_objects(&result);
    assert_eq!(
        pred.to_sql("id"),
        "(id IN ('git/merge/1') OR id NOT IN ('ci/deploy/1'))"
    );
}

#[test]
fn materialised_and_filter_ids_agree() {
    let ids = vec![ObjectId("issues/close/1".into())];
    let s4 = lower_list_objects(&ListObjectsResult::Ids {
        ids: ids.clone(),
        zookie: Zookie("z".into()),
    });
    let s8 = lower_list_objects(&ListObjectsResult::Filter {
        set_expr: SetExpr::Ids(ids),
        zookie: Zookie("z".into()),
    });
    assert_eq!(s4, s8);
    assert_eq!(s4, ToolScopePredicate::Ids(vec!["issues/close/1".into()]));
}
