//! The frozen **`ViewSpec` view-model** (contract 13.3, byte-identical X-3/OQ-C).
//!
//! **Owning architecture/contract:** `contract-index.md` row 13.3 + Knowledge architecture
//! `01-tech-and-data-model.md` §4.4 ("Views — the frozen `ViewSpec` + `QueryAst`") and the
//! reconciliation `00-reconciliation-decisions.md` X-3 ("View-model (frozen)"). Co-owned by
//! **Issues + Knowledge** (each owns its executor; the definition is **identical**); a `db_view`
//! block in `myelin-content` (13.1) carries a [`ViewSpec`] (this prompt swaps the KN-P01
//! `ViewHandle` floor for the real shape).
//!
//! ## The frozen shape (architecture §4.4 / X-3)
//! ```text
//! ViewSpec {
//!   kind:        table | board | calendar | timeline | gallery | list,
//!   filter:      QueryAst,                 // the shared AST; ALWAYS conjoined with list_objects (ADR-07)
//!   group_by:    Option<FieldId>,
//!   sort:        [ { field: FieldId, dir: asc|desc } ],   // the LAST-resort tiebreak is order_key
//!   visible:     [FieldId],
//!   order_field: FieldId(order_key),       // the manual drag-order field (the frozen LexoRank)
//! }
//! ```
//!
//! ## What this module ships (and what it does NOT)
//! It ships the **shared view-model definition** — the shape both Knowledge and Issues build their
//! (different) executors against. It does **not** ship an executor: the `SetExpr` ACL-filter
//! conjoin (always `filter AND list_objects`, ADR-07) + the JSONB lowering land in **KN-P16/P17**
//! and the Issues board prompts. The [`ViewSpec::filter`] is a [`QueryAst`](crate::QueryAst) — the
//! ONE frozen predicate core (3.4/13.3), so a view filter, the Bus `EventMatcher`, and Notif prefs
//! are the SAME grammar (one grammar, many compile targets — no second view-filter language).

use crate::QueryAst;
use serde::{Deserialize, Serialize};

/// **A field identifier** within a collection's field set (the `FieldId` of the architecture
/// §4.4 shape). An opaque, stable token — the executor maps it to a column/JSONB path; the
/// view-model only references it. PII-free (a schema id, never a value).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldId(pub String);

impl FieldId {
    /// Build a field id from a stable token.
    pub fn new(id: impl Into<String>) -> FieldId {
        FieldId(id.into())
    }
    /// The raw token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FieldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// **The frozen view kind** (architecture §4.4): the six render shapes a saved view takes. A
/// closed, frozen set — adding a kind is a whole-workspace contract PR (X-3), not a local change.
/// The discriminant is pinned (`#[repr(u8)]`) so the byte-identical wire id is structural across
/// the two co-owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ViewKind {
    /// A spreadsheet-style grid. Discriminant `0`.
    Table = 0,
    /// A kanban board (grouped columns). Discriminant `1`.
    Board = 1,
    /// A calendar laid out on a date field. Discriminant `2`.
    Calendar = 2,
    /// A timeline / Gantt-style view. Discriminant `3`.
    Timeline = 3,
    /// A card gallery. Discriminant `4`.
    Gallery = 4,
    /// A flat list. Discriminant `5`.
    List = 5,
}

impl ViewKind {
    /// The stable, PII-free wire id (the byte-identical token the two co-owners share — the drift
    /// anchor). A rename here is the wire-breaking change the consumers' drift tests catch.
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

    /// The full, ordered, frozen set (the closed variant set) — so a consumer can assert
    /// byte-identity over the WHOLE enum.
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

/// A sort direction (`asc|desc`, architecture §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

/// One sort criterion (`{ field: FieldId, dir: asc|desc }`, architecture §4.4). A view's `sort` is
/// an ordered list of these; the **last-resort tiebreak is always the `order_field`** (the frozen
/// LexoRank `order_key`), so two rows are never ambiguously ordered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    /// The field to sort on.
    pub field: FieldId,
    /// The direction.
    pub dir: SortDir,
}

/// **The frozen `ViewSpec` view-model** (contract 13.3, architecture §4.4, byte-identical X-3).
/// The shared definition both Knowledge and Issues compile their (different) executors against. A
/// shape change is a whole-workspace contract PR.
///
/// The [`filter`](ViewSpec::filter) is the ONE frozen [`QueryAst`] predicate core — the SAME
/// grammar that is the Bus `EventMatcher` (3.4) and Notif prefs (7.4). The executor ALWAYS conjoins
/// the `list_objects` `SetExpr` filter with this (ADR-07), so a viewer never sees an un-permitted
/// row — that conjoin is the executor's job (KN-P16/P17 / the Issues board prompts), not part of
/// this shared shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSpec {
    /// The render kind.
    pub kind: ViewKind,
    /// The view filter — the ONE frozen [`QueryAst`] (always conjoined with `list_objects` at
    /// execution; the conjoin is the executor's job, ADR-07).
    pub filter: QueryAst,
    /// The optional group-by field (e.g. a board's column field).
    pub group_by: Option<FieldId>,
    /// The ordered sort criteria; the last-resort tiebreak is [`order_field`](ViewSpec::order_field).
    pub sort: Vec<SortSpec>,
    /// The visible fields (the columns/properties the view shows).
    pub visible: Vec<FieldId>,
    /// The manual drag-order field — a [`crate::FieldType::OrderKey`] field carrying the frozen
    /// LexoRank `order_key` (the always-present total-order tiebreak).
    pub order_field: FieldId,
}

impl ViewSpec {
    /// A minimal table view over an empty filter (the default "show everything I may read" view —
    /// the empty filter is `true`, and the executor still conjoins `list_objects`, so it is never
    /// an over-broad read). `order_field` defaults to a conventional `"order_key"` field id.
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

    /// The frozen `ViewKind` set is exactly six variants, in order, with pinned discriminants and
    /// wire ids — the byte-identical anchor the two co-owners reconcile to.
    #[test]
    fn view_kind_taxonomy_is_frozen() {
        let all = ViewKind::all();
        assert_eq!(all.len(), 6, "the frozen view-kind set is six variants");
        for (i, k) in all.iter().enumerate() {
            assert_eq!(*k as u8, i as u8, "{} discriminant is pinned to {i}", k.wire_id());
        }
        let ids: Vec<&str> = all.iter().map(|k| k.wire_id()).collect();
        assert_eq!(
            ids,
            ["table", "board", "calendar", "timeline", "gallery", "list"],
            "the frozen wire-id set, in order"
        );
    }

    /// A `ViewSpec` carrying a real `QueryAst` filter serializes/deserializes stably (the wire
    /// contract the two co-owners share, golden-serialization).
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
            sort: vec![SortSpec { field: FieldId::new("priority"), dir: SortDir::Desc }],
            visible: vec![FieldId::new("title"), FieldId::new("assignee")],
            order_field: FieldId::new("order_key"),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: ViewSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    /// The default table view is an empty (`true`) filter that the executor still ACL-conjoins.
    #[test]
    fn default_table_view_is_empty_filter() {
        let v = ViewSpec::table(FieldId::new("order_key"));
        assert_eq!(v.kind, ViewKind::Table);
        assert_eq!(v.filter.predicate(), Some(&Predicate::True));
        assert!(v.sort.is_empty() && v.visible.is_empty() && v.group_by.is_none());
    }

    /// The `ViewSpec` golden serialization is byte-stable (the X-3 anti-drift anchor: both
    /// co-owners assert the SAME JSON). A field rename/reorder breaks this.
    #[test]
    fn view_spec_golden_serialization_is_stable() {
        let spec = ViewSpec {
            kind: ViewKind::List,
            filter: QueryAst::compiled(Predicate::True).unwrap(),
            group_by: None,
            sort: vec![SortSpec { field: FieldId::new("due"), dir: SortDir::Asc }],
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
