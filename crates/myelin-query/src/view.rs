use crate::QueryAst;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldId(pub String);

impl FieldId {
    pub fn new(id: impl Into<String>) -> FieldId {
        FieldId(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FieldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ViewKind {
    Table = 0,
    Board = 1,
    Calendar = 2,
    Timeline = 3,
    Gallery = 4,
    List = 5,
}

impl ViewKind {
    pub fn wire_id(self) -> &'static str {
        match self {
            ViewKind::Table => "table",
            ViewKind::Board => "board",
            ViewKind::Calendar => "calendar",
            ViewKind::Timeline => "timeline",
            ViewKind::Gallery => "gallery",
            ViewKind::List => "list",
        }
    }

    pub fn all() -> [ViewKind; 6] {
        [
            ViewKind::Table,
            ViewKind::Board,
            ViewKind::Calendar,
            ViewKind::Timeline,
            ViewKind::Gallery,
            ViewKind::List,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    pub field: FieldId,
    pub dir: SortDir,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSpec {
    pub kind: ViewKind,
    pub filter: QueryAst,
    pub group_by: Option<FieldId>,
    pub sort: Vec<SortSpec>,
    pub visible: Vec<FieldId>,
    pub order_field: FieldId,
}

impl ViewSpec {
    pub fn table(order_field: FieldId) -> ViewSpec {
        ViewSpec {
            kind: ViewKind::Table,
            filter: QueryAst::compiled(crate::Predicate::True)
                .expect("the empty `true` filter is always within the cost bound"),
            group_by: None,
            sort: Vec::new(),
            visible: Vec::new(),
            order_field,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr, Predicate};
    use myelin_identity::Literal;

    #[test]
    fn view_kind_taxonomy_is_frozen() {
        let all = ViewKind::all();
        assert_eq!(all.len(), 6, "the frozen view-kind set is six variants");
        for (i, k) in all.iter().enumerate() {
            assert_eq!(
                *k as u8,
                i as u8,
                "{} discriminant is pinned to {i}",
                k.wire_id()
            );
        }
        let ids: Vec<&str> = all.iter().map(|k| k.wire_id()).collect();
        assert_eq!(
            ids,
            ["table", "board", "calendar", "timeline", "gallery", "list"],
            "the frozen wire-id set, in order"
        );
    }

    #[test]
    fn view_spec_round_trips_stably() {
        let spec = ViewSpec {
            kind: ViewKind::Board,
            filter: QueryAst::compiled(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("status".into()),
                rhs: Expr::Lit(Literal::Str("open".into())),
            })
            .unwrap(),
            group_by: Some(FieldId::new("status")),
            sort: vec![SortSpec {
                field: FieldId::new("priority"),
                dir: SortDir::Desc,
            }],
            visible: vec![FieldId::new("title"), FieldId::new("assignee")],
            order_field: FieldId::new("order_key"),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: ViewSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn default_table_view_is_empty_filter() {
        let v = ViewSpec::table(FieldId::new("order_key"));
        assert_eq!(v.kind, ViewKind::Table);
        assert_eq!(v.filter.predicate(), Some(&Predicate::True));
        assert!(v.sort.is_empty() && v.visible.is_empty() && v.group_by.is_none());
    }

    #[test]
    fn view_spec_golden_serialization_is_stable() {
        let spec = ViewSpec {
            kind: ViewKind::List,
            filter: QueryAst::compiled(Predicate::True).unwrap(),
            group_by: None,
            sort: vec![SortSpec {
                field: FieldId::new("due"),
                dir: SortDir::Asc,
            }],
            visible: vec![FieldId::new("title")],
            order_field: FieldId::new("order_key"),
        };
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["kind"], "list");
        assert_eq!(json["sort"][0]["field"], "due");
        assert_eq!(json["sort"][0]["dir"], "asc");
        assert_eq!(json["visible"][0], "title");
        assert_eq!(json["order_field"], "order_key");
    }
}
