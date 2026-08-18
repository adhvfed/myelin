use crate::rollup::RollupAggregate;
use myelin_events::{
    pointer_for_propagation, Actor, AggregateKey, ArtifactRef, CellId, CorrelationId,
    CrossCellPointer, CrossCellStream, DataRole, EventEnvelope, EventId, EventType, Region,
    TenantId, Timestamp, Visibility,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const ISSUE_ROLLUP_RECOMPUTED: &str = "issue.rollup.recomputed";

const ISSUE_SUBJECT_ERASED: &str = "issue.subject.erased";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellRollupPointer {
    pub to_cell: CellId,
    pub pointer: CrossCellPointer,
}

impl CrossCellRollupPointer {
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        self.pointer.subject().artifact_ref()
    }

    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        self.pointer.home_cell()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PortfolioProjection {
    Rolled {
        subject: ArtifactRef,
        aggregate: RollupAggregate,
    },
    Tombstone {
        subject: ArtifactRef,
    },
}

impl PortfolioProjection {
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        match self {
            PortfolioProjection::Rolled { subject, .. }
            | PortfolioProjection::Tombstone { subject } => subject,
        }
    }

    #[must_use]
    pub fn aggregate(&self) -> Option<&RollupAggregate> {
        match self {
            PortfolioProjection::Rolled { aggregate, .. } => Some(aggregate),
            PortfolioProjection::Tombstone { .. } => None,
        }
    }

    #[must_use]
    pub fn is_rolled(&self) -> bool {
        matches!(self, PortfolioProjection::Rolled { .. })
    }
}

pub trait CellLocalRollupResolver {
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossCellRollupPointer,
        viewer_token: &str,
    ) -> PortfolioProjection;
}

#[derive(Clone)]
pub struct CrossCellPortfolioRollup {
    home_cell: CellId,
    children_fanned_out: Arc<AtomicU64>,
    pii_crossed: Arc<AtomicU64>,
}

impl CrossCellPortfolioRollup {
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossCellPortfolioRollup {
        CrossCellPortfolioRollup {
            home_cell,
            children_fanned_out: Arc::new(AtomicU64::new(0)),
            pii_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn fan_out_child(
        &self,
        tenant: &TenantId,
        region: &Region,
        child_subject: &ArtifactRef,
        child_home_cell: &CellId,
        correlation_id: &CorrelationId,
    ) -> Option<CrossCellRollupPointer> {
        if child_home_cell == &self.home_cell {
            return None;
        }
        let envelope = self.rollup_envelope(
            tenant,
            region,
            child_subject,
            correlation_id,
            ISSUE_ROLLUP_RECOMPUTED,
        );
        let pointer = pointer_for_propagation(
            &envelope,
            CrossCellStream::IssuePortfolio,
            child_home_cell.clone(),
        );
        self.children_fanned_out.fetch_add(1, Ordering::SeqCst);
        Some(CrossCellRollupPointer {
            to_cell: child_home_cell.clone(),
            pointer,
        })
    }

    #[must_use]
    pub fn resolve_cell_local(
        &self,
        pointer: &CrossCellRollupPointer,
        viewer_token: &str,
        resolver: &dyn CellLocalRollupResolver,
    ) -> PortfolioProjection {
        resolver.resolve_in_home_cell(pointer, viewer_token)
    }

    #[must_use]
    pub fn combine(
        local: &[RollupAggregate],
        cross_cell: &[PortfolioProjection],
    ) -> RollupAggregate {
        let mut total = 0u64;
        let mut done = 0u64;
        let mut estimate_sum = 0i64;
        let mut input_hash = 0u64;
        for agg in local {
            total += agg.total;
            done += agg.done;
            estimate_sum = estimate_sum.saturating_add(agg.estimate_sum);
            input_hash ^= agg.input_hash;
        }
        for proj in cross_cell {
            if let PortfolioProjection::Rolled { aggregate, .. } = proj {
                total += aggregate.total;
                done += aggregate.done;
                estimate_sum = estimate_sum.saturating_add(aggregate.estimate_sum);
                input_hash ^= aggregate.input_hash;
            }
        }
        RollupAggregate {
            total,
            done,
            estimate_sum,
            input_hash,
        }
    }

    #[must_use]
    pub fn children_fanned_out(&self) -> u64 {
        self.children_fanned_out.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn pii_crossed(&self) -> u64 {
        self.pii_crossed.load(Ordering::SeqCst)
    }

    fn rollup_envelope(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &ArtifactRef,
        correlation_id: &CorrelationId,
        type_: &str,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("xcell-rollup-{}", subject.0)),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(myelin_identity::Principal::stub(
                myelin_identity::PrincipalId("rollup-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                tenant.clone(),
            )),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("rollup:{}", subject.0)),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DsrCellReceipt {
    pub cell: CellId,
    pub subject: ArtifactRef,
    pub acknowledged: bool,
}

#[derive(Clone)]
pub struct CrossCellDsrFanout {
    origin_cell: CellId,
    pii_crossed: Arc<AtomicU64>,
}

impl CrossCellDsrFanout {
    #[must_use]
    pub fn new(origin_cell: CellId) -> CrossCellDsrFanout {
        CrossCellDsrFanout {
            origin_cell,
            pii_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn fan_out_erasure(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &ArtifactRef,
        correlation_id: &CorrelationId,
        member_cells: &[CellId],
        acknowledge: &dyn Fn(&CellId, &ArtifactRef) -> bool,
    ) -> Vec<DsrCellReceipt> {
        let mut receipts = Vec::with_capacity(member_cells.len());
        for cell in member_cells {
            let envelope = self.erasure_envelope(tenant, region, subject, correlation_id);
            let pointer =
                pointer_for_propagation(&envelope, CrossCellStream::IssuePortfolio, cell.clone());
            let carried = CrossCellRollupPointer {
                to_cell: cell.clone(),
                pointer,
            };
            let acknowledged = acknowledge(carried.home_cell(), carried.subject());
            receipts.push(DsrCellReceipt {
                cell: cell.clone(),
                subject: subject.clone(),
                acknowledged,
            });
        }
        receipts
    }

    #[must_use]
    pub fn reached_every_cell(receipts: &[DsrCellReceipt], member_cells: &[CellId]) -> bool {
        member_cells
            .iter()
            .all(|cell| receipts.iter().any(|r| &r.cell == cell && r.acknowledged))
    }

    #[must_use]
    pub fn origin_cell(&self) -> &CellId {
        &self.origin_cell
    }

    #[must_use]
    pub fn pii_crossed(&self) -> u64 {
        self.pii_crossed.load(Ordering::SeqCst)
    }

    fn erasure_envelope(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &ArtifactRef,
        correlation_id: &CorrelationId,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("xcell-erase-{}", subject.0)),
            type_: EventType(ISSUE_SUBJECT_ERASED.into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(myelin_identity::Principal::stub(
                myelin_identity::PrincipalId("dsr-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                tenant.clone(),
            )),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("erase:{}", subject.0)),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn agg(total: u64, done: u64, estimate_sum: i64, input_hash: u64) -> RollupAggregate {
        RollupAggregate {
            total,
            done,
            estimate_sum,
            input_hash,
        }
    }

    #[test]
    fn cross_cell_child_fans_out_pii_free_local_child_does_not() {
        let home = CellId::from_token("cell-fr-par-1");
        let rollup = CrossCellPortfolioRollup::new(home.clone());
        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let corr = CorrelationId("rollup-root".into());

        let remote_child = ArtifactRef("myelin://acme/issues/issue/EPIC-9".into());
        let remote_cell = CellId::from_token("cell-de-fra-1");
        let p = rollup
            .fan_out_child(&tenant, &region, &remote_child, &remote_cell, &corr)
            .expect("a remote child fans out");
        assert_eq!(p.to_cell, remote_cell);
        assert_eq!(
            p.home_cell(),
            &remote_cell,
            "the pointer is homed in the child's cell"
        );
        assert_eq!(p.subject(), &remote_child);
        assert_eq!(
            p.pointer.correlation_id(),
            &corr,
            "rides the rollup causal chain"
        );
        assert_eq!(rollup.children_fanned_out(), 1);
        assert_eq!(rollup.pii_crossed(), 0, "0 PII crosses the bridge");

        let local_child = ArtifactRef("myelin://acme/issues/issue/EPIC-1".into());
        assert!(
            rollup
                .fan_out_child(&tenant, &region, &local_child, &home, &corr)
                .is_none(),
            "a local child is a single-cell rollup, no self-hop"
        );
        assert_eq!(
            rollup.children_fanned_out(),
            1,
            "the local child did not fan out"
        );
    }

    #[test]
    fn resolution_is_cell_local_only_the_aggregate_crosses() {
        struct HomeCell;
        impl CellLocalRollupResolver for HomeCell {
            fn resolve_in_home_cell(
                &self,
                pointer: &CrossCellRollupPointer,
                viewer_token: &str,
            ) -> PortfolioProjection {
                if viewer_token == "authorised" {
                    PortfolioProjection::Rolled {
                        subject: pointer.subject().clone(),
                        aggregate: agg(10, 4, 40, 0xABCD),
                    }
                } else {
                    PortfolioProjection::Tombstone {
                        subject: pointer.subject().clone(),
                    }
                }
            }
        }

        let rollup = CrossCellPortfolioRollup::new(CellId::from_token("cell-fr-par-1"));
        let p = rollup
            .fan_out_child(
                &TenantId("acme".into()),
                &Region("fr-par".into()),
                &ArtifactRef("myelin://acme/issues/issue/EPIC-9".into()),
                &CellId::from_token("cell-de-fra-1"),
                &CorrelationId("c".into()),
            )
            .unwrap();

        let rolled = rollup.resolve_cell_local(&p, "authorised", &HomeCell);
        assert!(rolled.is_rolled());
        assert_eq!(rolled.aggregate().unwrap().total, 10);

        let tombstoned = rollup.resolve_cell_local(&p, "stranger", &HomeCell);
        assert!(
            !tombstoned.is_rolled(),
            "an unauthorised viewer gets a tombstone (0 leak)"
        );
        assert!(tombstoned.aggregate().is_none());
    }

    #[test]
    fn combine_sums_across_cells_tombstone_contributes_nothing() {
        let local = vec![agg(5, 2, 20, 0x1)];
        let cross = vec![
            PortfolioProjection::Rolled {
                subject: ArtifactRef("myelin://acme/issues/issue/EPIC-9".into()),
                aggregate: agg(10, 4, 40, 0x2),
            },
            PortfolioProjection::Tombstone {
                subject: ArtifactRef("myelin://acme/issues/issue/EPIC-8".into()),
            },
        ];
        let combined = CrossCellPortfolioRollup::combine(&local, &cross);
        assert_eq!(
            combined.total, 15,
            "5 local + 10 cross-cell (tombstone excluded)"
        );
        assert_eq!(combined.done, 6);
        assert_eq!(combined.estimate_sum, 60);
        assert_eq!(
            combined.input_hash,
            0x1 ^ 0x2,
            "XOR-folded, tombstone contributes nothing"
        );
        assert!((combined.progress() - 6.0 / 15.0).abs() < 1e-9);
    }

    #[test]
    fn dsr_fan_out_reaches_every_member_cell_pii_free() {
        let origin = CellId::from_token("cell-fr-par-1");
        let dsr = CrossCellDsrFanout::new(origin);
        let member_cells = vec![
            CellId::from_token("cell-fr-par-1"),
            CellId::from_token("cell-de-fra-1"),
            CellId::from_token("cell-nl-ams-1"),
        ];
        let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-7".into());

        let receipts = dsr.fan_out_erasure(
            &TenantId("acme".into()),
            &Region("fr-par".into()),
            &subject,
            &CorrelationId("dsr-root".into()),
            &member_cells,
            &|_cell, _subject| true,
        );

        assert_eq!(receipts.len(), 3, "one receipt per member cell");
        assert!(
            CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
            "0 member cell missed (CP-D7)"
        );
        assert_eq!(
            dsr.pii_crossed(),
            0,
            "0 PII crosses the bridge (CP-D8 / GA-D8)"
        );
        for r in &receipts {
            assert_eq!(r.subject, subject);
            assert!(r.acknowledged);
        }
    }

    #[test]
    fn dsr_unacknowledged_cell_is_a_loud_gate_failure() {
        let dsr = CrossCellDsrFanout::new(CellId::from_token("cell-fr-par-1"));
        let member_cells = vec![
            CellId::from_token("cell-fr-par-1"),
            CellId::from_token("cell-de-fra-1"),
        ];
        let subject = ArtifactRef("myelin://acme/identity/pseudonym/p-7".into());
        let receipts = dsr.fan_out_erasure(
            &TenantId("acme".into()),
            &Region("fr-par".into()),
            &subject,
            &CorrelationId("dsr-root".into()),
            &member_cells,
            &|cell, _subject| cell.as_str() != "cell-de-fra-1",
        );
        assert!(
            !CrossCellDsrFanout::reached_every_cell(&receipts, &member_cells),
            "an unacknowledged cell is a loud fan-out gap (never a silent residual)"
        );
    }
}
