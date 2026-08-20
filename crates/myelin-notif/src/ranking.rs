use myelin_identity::Principal;

use crate::read_state::ReadState;
use crate::router::RoutedInboxItem;
use crate::{Class, Reason};

pub const PRIORITY_MIN: u8 = 0;
pub const PRIORITY_MAX: u8 = 100;

pub fn reason_base_class(reason: Reason) -> (u8, Class) {
    match reason {
        Reason::ApprovalRequested | Reason::Escalated | Reason::Sla => (90, Class::Critical),
        Reason::ReviewRequested | Reason::Assigned | Reason::Mentioned | Reason::Shared => {
            (70, Class::Direct)
        }
        Reason::Replied | Reason::AgentProposal | Reason::Comments => (55, Class::Participating),
        Reason::Watched
        | Reason::StateChanged
        | Reason::ThreadWatched
        | Reason::Blocked
        | Reason::Unblocked => (35, Class::Watching),
        Reason::Fyi => (15, Class::Fyi),
    }
}

pub fn base_priority(reason: Reason) -> u8 {
    reason_base_class(reason).0
}

pub fn class_for(reason: Reason) -> Class {
    reason_base_class(reason).1
}

pub fn band_floor(class: Class) -> u8 {
    match class {
        Class::Critical => 90,
        Class::Direct => 70,
        Class::Participating => 55,
        Class::Watching => 35,
        Class::Fyi => 15,
    }
}

pub fn band_ceiling(class: Class) -> u8 {
    match class {
        Class::Critical => PRIORITY_MAX,
        Class::Direct => band_floor(Class::Critical) - 1,
        Class::Participating => band_floor(Class::Direct) - 1,
        Class::Watching => band_floor(Class::Participating) - 1,
        Class::Fyi => band_floor(Class::Watching) - 1,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainTrace {
    pub reason: Reason,
    pub base: u8,
    pub class: Class,
    pub affinity_bonus: u8,
    pub role_bonus: u8,
    pub final_priority: u8,
}

impl ExplainTrace {
    pub fn render(&self) -> String {
        format!(
            "reason={:?} class={:?} base={} +affinity={} +role={} = priority {}",
            self.reason,
            self.class,
            self.base,
            self.affinity_bonus,
            self.role_bonus,
            self.final_priority
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedItem {
    pub item: RoutedInboxItem,
    pub priority: u8,
    pub trace: ExplainTrace,
}

pub trait AffinitySource {
    fn affinity_bonus(&self, viewer: &Principal, item: &RoutedInboxItem) -> u8;

    fn role_bonus(&self, viewer: &Principal, item: &RoutedInboxItem) -> u8;
}

pub struct NeutralAffinity;

impl AffinitySource for NeutralAffinity {
    fn affinity_bonus(&self, _viewer: &Principal, _item: &RoutedInboxItem) -> u8 {
        0
    }
    fn role_bonus(&self, _viewer: &Principal, _item: &RoutedInboxItem) -> u8 {
        0
    }
}

pub trait RankStrategy {
    fn score(&self, viewer: &Principal, item: &RoutedInboxItem) -> (u8, ExplainTrace);
}

pub struct DeterministicV1<A: AffinitySource> {
    affinity: A,
}

impl Default for DeterministicV1<NeutralAffinity> {
    fn default() -> Self {
        DeterministicV1 {
            affinity: NeutralAffinity,
        }
    }
}

impl<A: AffinitySource> DeterministicV1<A> {
    pub fn new(affinity: A) -> Self {
        DeterministicV1 { affinity }
    }
}

impl<A: AffinitySource> RankStrategy for DeterministicV1<A> {
    fn score(&self, viewer: &Principal, item: &RoutedInboxItem) -> (u8, ExplainTrace) {
        let (base, class) = reason_base_class(item.reason);
        let affinity_bonus = self.affinity.affinity_bonus(viewer, item);
        let role_bonus = self.affinity.role_bonus(viewer, item);
        let raw = u16::from(base) + u16::from(affinity_bonus) + u16::from(role_bonus);
        let floor = band_floor(class);
        let ceiling = band_ceiling(class);
        let final_priority = (raw.min(u16::from(ceiling)) as u8).max(floor);
        let trace = ExplainTrace {
            reason: item.reason,
            base,
            class,
            affinity_bonus,
            role_bonus,
            final_priority,
        };
        (final_priority, trace)
    }
}

pub fn rank_and_order(
    candidates: Vec<RoutedInboxItem>,
    viewer: &Principal,
    strategy: &dyn RankStrategy,
) -> Vec<RankedItem> {
    let mut ranked: Vec<RankedItem> = candidates
        .into_iter()
        .map(|item| {
            let (priority, trace) = strategy.score(viewer, &item);
            RankedItem {
                item,
                priority,
                trace,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        attention_rank(&b.item)
            .cmp(&attention_rank(&a.item))
            .then_with(|| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.item.item_id.cmp(&b.item.item_id))
            })
    });
    ranked
}

fn attention_rank(item: &RoutedInboxItem) -> u8 {
    ReadState::parse(&item.state)
        .map(ReadState::attention_rank)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::ArtifactRef;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn viewer() -> Principal {
        Principal::stub(PrincipalId("u1".into()), PrincipalKind::Human, tenant())
    }
    fn item(item_id: &str, reason: Reason) -> RoutedInboxItem {
        RoutedInboxItem {
            tenant: tenant(),
            region: Region("fr-par".into()),
            item_id: item_id.into(),
            recipient: "u1".into(),
            subject: ArtifactRef(format!("myelin://acme/issue/issue/{item_id}")),
            reason,
            class: class_for(reason),
            origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
            dedup_key: item_id.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        }
    }

    #[test]
    fn reason_base_class_table_is_exact() {
        assert_eq!(
            reason_base_class(Reason::ApprovalRequested),
            (90, Class::Critical)
        );
        assert_eq!(reason_base_class(Reason::Escalated), (90, Class::Critical));
        assert_eq!(reason_base_class(Reason::Sla), (90, Class::Critical));
        assert_eq!(
            reason_base_class(Reason::ReviewRequested),
            (70, Class::Direct)
        );
        assert_eq!(reason_base_class(Reason::Assigned), (70, Class::Direct));
        assert_eq!(reason_base_class(Reason::Mentioned), (70, Class::Direct));
        assert_eq!(
            reason_base_class(Reason::Replied),
            (55, Class::Participating)
        );
        assert_eq!(
            reason_base_class(Reason::AgentProposal),
            (55, Class::Participating)
        );
        assert_eq!(reason_base_class(Reason::Watched), (35, Class::Watching));
        assert_eq!(
            reason_base_class(Reason::StateChanged),
            (35, Class::Watching)
        );
        assert_eq!(reason_base_class(Reason::Fyi), (15, Class::Fyi));
    }

    #[test]
    fn base_priority_and_class_for_project_the_table() {
        for reason in [
            Reason::ApprovalRequested,
            Reason::Assigned,
            Reason::Replied,
            Reason::Watched,
            Reason::Fyi,
        ] {
            let (base, class) = reason_base_class(reason);
            assert_eq!(
                base_priority(reason),
                base,
                "base_priority == the table base for {reason:?}"
            );
            assert_eq!(
                class_for(reason),
                class,
                "class_for == the table class for {reason:?}"
            );
        }
        assert_eq!(base_priority(Reason::Sla), 90);
        assert_eq!(base_priority(Reason::Assigned), 70);
        assert_eq!(base_priority(Reason::Fyi), 15);
        assert_ne!(base_priority(Reason::Sla), base_priority(Reason::Fyi));
    }

    #[test]
    fn table_is_total_and_band_consistent_over_all_reasons() {
        for reason in [
            Reason::ApprovalRequested,
            Reason::Escalated,
            Reason::Sla,
            Reason::ReviewRequested,
            Reason::Assigned,
            Reason::Mentioned,
            Reason::Replied,
            Reason::AgentProposal,
            Reason::Watched,
            Reason::StateChanged,
            Reason::Fyi,
            Reason::Blocked,
            Reason::Unblocked,
            Reason::ThreadWatched,
            Reason::Shared,
            Reason::Comments,
        ] {
            let (base, class) = reason_base_class(reason);
            assert!(
                base >= band_floor(class) && base <= band_ceiling(class),
                "reason {reason:?}: base {base} must be within {class:?} band [{}, {}]",
                band_floor(class),
                band_ceiling(class)
            );
        }
    }

    #[test]
    fn class_bands_are_disjoint_and_strictly_ordered() {
        let bands = [
            Class::Critical,
            Class::Direct,
            Class::Participating,
            Class::Watching,
            Class::Fyi,
        ];
        for win in bands.windows(2) {
            let higher = win[0];
            let lower = win[1];
            assert!(
                band_floor(higher) > band_ceiling(lower),
                "{higher:?} floor {} must be strictly above {lower:?} ceiling {}",
                band_floor(higher),
                band_ceiling(lower)
            );
        }
        assert_eq!(PRIORITY_MIN, 0, "the 0..100 floor is 0");
        for class in bands {
            assert!(band_ceiling(class) <= PRIORITY_MAX);
            assert!(band_floor(class) <= band_ceiling(class));
        }
    }

    #[test]
    fn v1_neutral_scores_base_and_emits_complete_trace() {
        let ranker = DeterministicV1::default();
        let (priority, trace) = ranker.score(&viewer(), &item("x", Reason::Assigned));
        assert_eq!(priority, 70, "neutral affinity → the base (70/direct)");
        assert_eq!(
            trace,
            ExplainTrace {
                reason: Reason::Assigned,
                base: 70,
                class: Class::Direct,
                affinity_bonus: 0,
                role_bonus: 0,
                final_priority: 70,
            }
        );
        let (p2, t2) = ranker.score(&viewer(), &item("x", Reason::Assigned));
        assert_eq!((priority, trace.clone()), (p2, t2));
        assert!(trace.render().contains("priority 70"));
        assert!(trace.render().contains("Assigned"));
    }

    struct FixedAffinity {
        affinity: u8,
        role: u8,
    }
    impl AffinitySource for FixedAffinity {
        fn affinity_bonus(&self, _v: &Principal, _i: &RoutedInboxItem) -> u8 {
            self.affinity
        }
        fn role_bonus(&self, _v: &Principal, _i: &RoutedInboxItem) -> u8 {
            self.role
        }
    }

    #[test]
    fn bonuses_narrow_within_band_order() {
        let with_affinity = DeterministicV1::new(FixedAffinity {
            affinity: 5,
            role: 3,
        });
        let (p, t) = with_affinity.score(&viewer(), &item("x", Reason::Assigned));
        assert_eq!(
            p, 78,
            "70 base + 5 affinity + 3 role = 78 (within the direct band)"
        );
        assert_eq!(t.affinity_bonus, 5);
        assert_eq!(t.role_bonus, 3);
        assert!(p >= band_floor(Class::Direct) && p <= band_ceiling(Class::Direct));
    }

    #[test]
    fn band_clamp_holds_under_saturating_affinity() {
        let huge = DeterministicV1::new(FixedAffinity {
            affinity: 255,
            role: 255,
        });
        let (fyi_p, _) = huge.score(&viewer(), &item("fyi", Reason::Fyi));
        assert_eq!(
            fyi_p,
            band_ceiling(Class::Fyi),
            "fyi clamps to its band ceiling (34)"
        );
        assert!(
            fyi_p < band_floor(Class::Direct),
            "an fyi NEVER reaches the direct floor"
        );
        assert!(
            fyi_p < band_floor(Class::Critical),
            "an fyi NEVER reaches the critical floor"
        );
        let plain = DeterministicV1::default();
        let (crit_p, _) = plain.score(&viewer(), &item("crit", Reason::Sla));
        assert!(
            crit_p > fyi_p,
            "a plain critical (90) outranks a saturated fyi (34)"
        );
        let (crit_huge, _) = huge.score(&viewer(), &item("crit2", Reason::Sla));
        assert_eq!(
            crit_huge, PRIORITY_MAX,
            "critical saturates at the 0..100 ceiling (100)"
        );
    }

    #[test]
    fn rank_and_order_orders_by_priority_then_item_id_with_trace_on_every_item() {
        let candidates = vec![
            item("z-fyi", Reason::Fyi),
            item("a-crit", Reason::Sla),
            item("b-direct", Reason::Assigned),
            item("a-direct", Reason::Mentioned),
        ];
        let ranker = DeterministicV1::default();
        let ordered = rank_and_order(candidates, &viewer(), &ranker);
        let order: Vec<&str> = ordered.iter().map(|r| r.item.item_id.as_str()).collect();
        assert_eq!(order, vec!["a-crit", "a-direct", "b-direct", "z-fyi"]);
        for r in &ordered {
            assert_eq!(
                r.priority, r.trace.final_priority,
                "the trace's final == the rank"
            );
            assert!(
                !r.trace.render().is_empty(),
                "every rank carries a non-empty trace"
            );
        }
    }

    #[test]
    fn attention_state_precedes_reason_priority() {
        let mut completed_approval = item("completed-approval", Reason::ApprovalRequested);
        completed_approval.state = "done".into();
        let fresh_mention = item("fresh-mention", Reason::Mentioned);
        let mut read_approval = item("read-approval", Reason::ApprovalRequested);
        read_approval.state = "read".into();

        let ordered = rank_and_order(
            vec![completed_approval, fresh_mention, read_approval],
            &viewer(),
            &DeterministicV1::default(),
        );
        assert_eq!(
            ordered
                .iter()
                .map(|ranked| ranked.item.item_id.as_str())
                .collect::<Vec<_>>(),
            ["fresh-mention", "read-approval", "completed-approval"],
            "new work is seen before read work, and completed work stays parked"
        );
    }

    #[test]
    fn no_critical_or_direct_ranks_below_any_fyi() {
        let candidates = vec![
            item("fyi-1", Reason::Fyi),
            item("crit-1", Reason::ApprovalRequested),
            item("fyi-2", Reason::Fyi),
            item("direct-1", Reason::Assigned),
            item("fyi-3", Reason::Fyi),
            item("crit-2", Reason::Escalated),
        ];
        let ordered = rank_and_order(candidates, &viewer(), &DeterministicV1::default());
        let last_important = ordered
            .iter()
            .rposition(|r| matches!(r.trace.class, Class::Critical | Class::Direct))
            .unwrap();
        let first_fyi = ordered
            .iter()
            .position(|r| r.trace.class == Class::Fyi)
            .unwrap();
        assert!(
            last_important < first_fyi,
            "every critical/direct ranks above every fyi (important-buried-rate 0)"
        );
    }

    #[test]
    fn rank_strategy_is_swappable() {
        struct FlatStrategy;
        impl RankStrategy for FlatStrategy {
            fn score(&self, _v: &Principal, item: &RoutedInboxItem) -> (u8, ExplainTrace) {
                (
                    50,
                    ExplainTrace {
                        reason: item.reason,
                        base: 50,
                        class: Class::Participating,
                        affinity_bonus: 0,
                        role_bonus: 0,
                        final_priority: 50,
                    },
                )
            }
        }
        let candidates = vec![item("b", Reason::Sla), item("a", Reason::Fyi)];
        let ordered = rank_and_order(candidates, &viewer(), &FlatStrategy);
        let order: Vec<&str> = ordered.iter().map(|r| r.item.item_id.as_str()).collect();
        assert_eq!(order, vec!["a", "b"]);
        assert!(
            ordered.iter().all(|r| r.priority == 50),
            "the swapped strategy is in effect"
        );
    }
}
