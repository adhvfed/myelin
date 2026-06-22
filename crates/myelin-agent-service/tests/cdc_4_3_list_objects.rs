//! # Consumer CDC for 4.3 (`list_objects` SetExpr push-down) + 4.10 (zookie consistency) — the
//! delegation-scoped tool-list (AG-P7 → P-219).
//!
//! The agent-fabric scope builder is a CONSUMER of Identity 4.3 (`list_objects → Ids |
//! Filter{set_expr, zookie}`) and 4.10 (the zookie that bounds the read's staleness). This CDC pairs
//! the consumer seam ([`myelin_agent_service::ToolListObjects`]) with a REAL provider that returns
//! the frozen 4.3 shapes, and asserts the consumer's contract expectations hold:
//!
//! - the scoped tool list is computed in ONE `list_objects` call (no N+1, no per-tool `check`);
//! - a `Filter { set_expr }` push-down lowers to ONE conjoinable predicate over the Fabric's own id;
//! - the zookie watermark (4.10) the provider stamps is carried back so apply reads-its-writes;
//! - the lowering of a materialised `Ids` result == the `Filter{Ids}` push-down (one ACL meaning).
//!
//! The provider shape here is the frozen `IdentityService::list_objects` return contract
//! (`ListObjectsResult`); the real Identity engine (P-ID-11/P-ID-12) materialises the same shape over
//! the live authz reverse index — the CDC binds the WIRE contract, not the engine.

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSchema, ToolSurface};
use myelin_agent_service::{
    build_scoped_tool_list, lower_list_objects, tool_def_id, ToolCatalogueIds, ToolListObjects,
    ToolScopePredicate, TOOL_DEF_OBJECT_TYPE, TOOL_USE_PERMISSION,
};
use myelin_identity::{ListObjectsResult, ObjectId, ObjectType, Permission, SetExpr, Zookie};
use std::cell::Cell;

/// A real in-memory `tool_def` catalogue (the §4.2 registry the subset is drawn from, contract 8.1).
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

/// **A REAL `list_objects` provider returning the frozen 4.3 contract shapes (the CDC provider).**
/// It records the exact `(permission, ty, at)` arguments the consumer passed (the consumer-side
/// contract: it MUST scope over the `tool_def` type with the `tool.use` permission at the run's
/// zookie) and counts its calls (the no-N+1 expectation).
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

/// **CDC 4.3/4.10 — the consumer scopes over `tool_def`/`tool.use` at the run's zookie, in ONE call,
/// and carries the provider's zookie back.** The consumer's contract obligations against the 4.3
/// provider: ONE call (no N+1), the right `(permission, type)` arguments, and the 4.10 zookie
/// threaded in + carried back.
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
        // The provider push-down admits exactly the git/merge and issues/close tools (the S8 path).
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

    // ONE list_objects call (no N+1 — the consumer never calls per-tool check).
    assert_eq!(
        provider.calls.get(),
        1,
        "the consumer issues ONE list_objects call"
    );
    assert_eq!(scoped.query_count, 1);
    // The consumer scoped over the `tool_def` object type with the `tool.use` permission (4.3).
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
    // The run's zookie (4.10) was threaded into the read (read-your-writes).
    assert_eq!(provider.last_at.borrow().as_deref(), Some("z-run"));
    // The provider's revision zookie is carried back so apply reads-its-writes (4.10).
    assert_eq!(scoped.zookie, Zookie("z-rev-42".into()));
    // The brain sees exactly the scoped subset (git/merge + issues/close, NOT ci/deploy).
    assert_eq!(scoped.tools.len(), 2);
    assert!(scoped.tools.contains(&ToolSchema("merge".into())));
    assert!(scoped.tools.contains(&ToolSchema("close".into())));
    assert!(!scoped.tools.contains(&ToolSchema("deploy".into())));
}

/// **CDC 4.3 — the `Filter{set_expr}` push-down lowers to ONE conjoinable predicate (no post-filter).**
/// The frozen `SetExpr` monotone algebra the provider returns lowers to the single
/// [`ToolScopePredicate`] the consumer pushes down — one SQL clause over the Fabric's own id column.
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
    // ONE predicate: an OR of the allow-set and the deny-set — a single conjoinable clause.
    assert_eq!(
        pred.to_sql("id"),
        "(id IN ('git/merge/1') OR id NOT IN ('ci/deploy/1'))"
    );
}

/// **CDC 4.3 — the materialised `Ids` result (S4) and the `Filter{Ids}` push-down (S8) lower to the
/// SAME predicate (one ACL meaning, no drift between the two response shapes).**
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
