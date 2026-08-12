pub use myelin_query::{
    field::{FieldType, FieldValue, Jitter, OrderKey},
    order_key::{tiebreak, ConformanceStep, RankOp},
    parse_query,
    view::{FieldId, SortDir, SortSpec, ViewKind, ViewSpec},
    CmpOp, EvalContext, Expr, Predicate, QueryAst, CONFORMANCE_VECTOR,
};

pub fn issues_replay_conformance_vector() -> Vec<String> {
    CONFORMANCE_VECTOR.iter().map(run_step).collect()
}

fn run_step(step: &ConformanceStep) -> String {
    let key = match step.op {
        RankOp::First { jitter } => OrderKey::rank_first(jitter_of(jitter)),
        RankOp::Last { after, jitter } => {
            let after = after.map(parse_bound);
            OrderKey::rank_last(after.as_ref(), jitter_of(jitter))
        }
        RankOp::Between { lo, hi, jitter } => {
            let lo = lo.map(parse_bound);
            let hi = hi.map(parse_bound);
            OrderKey::rank_between(lo.as_ref(), hi.as_ref(), jitter_of(jitter))
        }
    };
    key.as_str().to_owned()
}

fn parse_bound(s: &str) -> OrderKey {
    OrderKey::parse(s).expect("a conformance-vector bound is a valid base-62 order_key")
}

fn jitter_of((a, b): (usize, usize)) -> Jitter {
    Jitter::from_ranks(a, b).expect("conformance-vector jitter ranks are in 0..62")
}

pub fn issues_canonical_board_view() -> ViewSpec {
    ViewSpec {
        kind: ViewKind::Board,
        filter: parse_query("status == 'open' AND severity >= 3")
            .expect("the co-owned grammar compiles a well-formed Issues board filter"),
        group_by: Some(FieldId::new("status")),
        sort: vec![SortSpec {
            field: FieldId::new("priority"),
            dir: SortDir::Desc,
        }],
        visible: vec![FieldId::new("title"), FieldId::new("assignee")],
        order_field: FieldId::new("order_key"),
    }
}

pub fn issues_field_type_wire_ids() -> Vec<&'static str> {
    FieldType::all().iter().map(|t| t.wire_id()).collect()
}
