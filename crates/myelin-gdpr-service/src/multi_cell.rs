use std::collections::{BTreeMap, BTreeSet};

use myelin_tenancy::{CellId, CrossCellPointer};

use crate::full_fanout::GaD1Certificate;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCellSet {
    cells: BTreeSet<CellId>,
    home_cell: CellId,
}

impl MemberCellSet {
    pub fn union(home_cell: CellId, member_cells: &[CellId]) -> MemberCellSet {
        let mut cells: BTreeSet<CellId> = member_cells.iter().cloned().collect();
        cells.insert(home_cell.clone());
        MemberCellSet { cells, home_cell }
    }

    pub fn cells(&self) -> impl Iterator<Item = &CellId> {
        self.cells.iter()
    }

    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn contains(&self, cell: &CellId) -> bool {
        self.cells.contains(cell)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PerCellReceipt {
    pub cell_id: CellId,
    pub cell_certificate: GaD1Certificate,
    pub content_hash: String,
}

impl PerCellReceipt {
    pub fn new(cell_id: CellId, cell_certificate: GaD1Certificate) -> PerCellReceipt {
        let content_hash = per_cell_content_address(&cell_id, &cell_certificate.content_hash);
        PerCellReceipt {
            cell_id,
            cell_certificate,
            content_hash,
        }
    }

    pub fn cell_is_complete(&self) -> bool {
        self.cell_certificate.is_complete()
    }
}

fn per_cell_content_address(cell_id: &CellId, cell_cert_hash: &str) -> String {
    let body = format!(
        "per_cell\u{1f}cell={}\u{1f}cert={cell_cert_hash}",
        cell_id.as_str()
    );
    let digest = blake3::hash(body.as_bytes());
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

#[derive(Clone, Debug)]
pub struct MultiCellCoverage {
    target: MemberCellSet,
    receipts: BTreeMap<CellId, PerCellReceipt>,
}

impl MultiCellCoverage {
    pub fn new(target: MemberCellSet) -> MultiCellCoverage {
        MultiCellCoverage {
            target,
            receipts: BTreeMap::new(),
        }
    }

    pub fn record_receipt(&mut self, receipt: PerCellReceipt) -> bool {
        if !self.target.contains(&receipt.cell_id) {
            return false;
        }
        self.receipts.insert(receipt.cell_id.clone(), receipt);
        true
    }

    pub fn cells_missed(&self) -> usize {
        self.target
            .cells()
            .filter(|c| !self.cell_fully_reached(c))
            .count()
    }

    pub fn missed(&self) -> Vec<CellId> {
        self.target
            .cells()
            .filter(|c| !self.cell_fully_reached(c))
            .cloned()
            .collect()
    }

    fn cell_fully_reached(&self, cell: &CellId) -> bool {
        self.receipts
            .get(cell)
            .map(PerCellReceipt::cell_is_complete)
            .unwrap_or(false)
    }

    pub fn is_complete(&self) -> bool {
        self.cells_missed() == 0
    }

    pub fn per_cell_receipts(&self) -> Vec<PerCellReceipt> {
        self.receipts.values().cloned().collect()
    }

    pub fn target(&self) -> &MemberCellSet {
        &self.target
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MultiCellCertificate {
    pub scope_token: String,
    pub per_cell: Vec<PerCellReceipt>,
    pub cells_missed: usize,
    pub cells_total: usize,
    pub content_hash: String,
}

impl MultiCellCertificate {
    pub fn seal(
        scope_token: &str,
        coverage: &MultiCellCoverage,
    ) -> std::result::Result<MultiCellCertificate, MultiCellGap> {
        if !coverage.is_complete() {
            return Err(MultiCellGap {
                missed: coverage.missed(),
                cells_missed: coverage.cells_missed(),
                cells_total: coverage.target().len(),
            });
        }
        let per_cell = coverage.per_cell_receipts();
        let cells_total = coverage.target().len();
        let content_hash = multi_cell_content_address(scope_token, &per_cell, 0, cells_total);
        Ok(MultiCellCertificate {
            scope_token: scope_token.to_string(),
            per_cell,
            cells_missed: 0,
            cells_total,
            content_hash,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.cells_missed == 0
            && self.per_cell.len() == self.cells_total
            && self.per_cell.iter().all(PerCellReceipt::cell_is_complete)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MultiCellGap {
    pub missed: Vec<CellId>,
    pub cells_missed: usize,
    pub cells_total: usize,
}

fn multi_cell_content_address(
    scope_token: &str,
    per_cell: &[PerCellReceipt],
    cells_missed: usize,
    cells_total: usize,
) -> String {
    let mut body = format!("ga_d8\u{1f}scope={scope_token}");
    for r in per_cell {
        body.push('\u{1f}');
        body.push_str(&format!("cell={}={}", r.cell_id.as_str(), r.content_hash));
    }
    body.push_str(&format!(
        "\u{1f}cells_missed={cells_missed}\u{1f}cells_total={cells_total}"
    ));
    let digest = blake3::hash(body.as_bytes());
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MultiCellFanOut;

impl MultiCellFanOut {
    pub fn new() -> MultiCellFanOut {
        MultiCellFanOut
    }

    pub fn fan_out(
        &self,
        scope_token: &str,
        target: &MemberCellSet,
        pointer: &CrossCellPointer,
        mut resolve_in_cell: impl FnMut(&CellId, &CrossCellPointer) -> GaD1Certificate,
    ) -> std::result::Result<MultiCellCertificate, MultiCellGap> {
        let mut coverage = MultiCellCoverage::new(target.clone());
        for cell in target.cells() {
            let cell_cert = resolve_in_cell(cell, pointer);
            let receipt = PerCellReceipt::new(cell.clone(), cell_cert);
            coverage.record_receipt(receipt);
        }
        MultiCellCertificate::seal(scope_token, &coverage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::full_fanout::{FullFanOutCoverage, Holder};
    use myelin_tenancy::{ArtifactRef, ArtifactType, CorrelationId, OpaqueSubjectId};

    fn complete_cell_cert(scope: &str) -> GaD1Certificate {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        GaD1Certificate::seal(scope, &cov).expect("a complete cell fan-out seals")
    }

    fn incomplete_cell_cert(scope: &str) -> GaD1Certificate {
        let mut cert = complete_cell_cert(scope);
        cert.holders_missed = 1;
        cert.erasure_fanout_coverage = 17.0 / 18.0;
        if let Some(first) = cert.reach.first_mut() {
            first.reached = false;
        }
        cert
    }

    fn cell(token: &str) -> CellId {
        CellId::from_token(token)
    }

    fn sample_pointer(home: &str) -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("corr-1".into()),
            CellId::from_token(home),
        )
    }

    #[test]
    fn member_cell_set_unions_home_and_dedups() {
        let home = cell("cell-fr-par-1");
        let members = vec![
            cell("cell-fr-par-2"),
            cell("cell-fr-par-2"),
            cell("cell-fr-par-3"),
        ];
        let set = MemberCellSet::union(home.clone(), &members);
        let cells: Vec<&CellId> = set.cells().collect();
        assert_eq!(set.len(), 3, "home ∪ {{2 distinct members}} = 3 cells");
        assert!(set.contains(&home), "the home cell is ALWAYS a member");
        assert!(set.contains(&cell("cell-fr-par-2")));
        assert!(set.contains(&cell("cell-fr-par-3")));
        assert_eq!(set.home_cell(), &home);
        let labels: Vec<&str> = cells.iter().map(|c| c.as_str()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        assert_eq!(
            labels, sorted,
            "the cell set is ordered (deterministic merge)"
        );
        assert!(!set.is_empty());
    }

    #[test]
    fn home_cell_is_in_the_set_even_with_no_member_cells() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home.clone(), &[]);
        assert_eq!(
            set.len(),
            1,
            "an empty member_cells still fans the home cell"
        );
        assert!(set.contains(&home));
    }

    #[test]
    fn a_full_multi_cell_fan_out_is_complete_0_cells_missed() {
        let home = cell("cell-fr-par-1");
        let members = vec![cell("cell-fr-par-2"), cell("cell-fr-par-3")];
        let set = MemberCellSet::union(home, &members);
        let pointer = sample_pointer("cell-fr-par-1");
        let cert = MultiCellFanOut::new()
            .fan_out("acme/u-1", &set, &pointer, |c, _p| {
                complete_cell_cert(&format!("acme/u-1@{}", c.as_str()))
            })
            .expect("a complete multi-cell fan-out seals");
        assert_eq!(cert.cells_missed, 0, "0 cells missed");
        assert_eq!(cert.cells_total, 3);
        assert_eq!(cert.per_cell.len(), 3, "one receipt per cell");
        assert!(cert.is_complete());
        assert!(cert.content_hash.starts_with("blake3:"));
        assert!(cert.per_cell.iter().all(|r| r.cell_is_complete()));
    }

    #[test]
    fn a_missed_cell_is_detected_and_refuses_to_seal() {
        let home = cell("cell-fr-par-1");
        let members = vec![cell("cell-fr-par-2"), cell("cell-fr-par-3")];
        let set = MemberCellSet::union(home, &members);
        let mut cov = MultiCellCoverage::new(set);
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-1"),
            complete_cell_cert("acme/u-1@1"),
        ));
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-2"),
            complete_cell_cert("acme/u-1@2"),
        ));
        assert_eq!(cov.cells_missed(), 1, "the missed cell is COUNTED");
        assert_eq!(
            cov.missed(),
            vec![cell("cell-fr-par-3")],
            "named: cell-fr-par-3"
        );
        assert!(!cov.is_complete());
        let gap =
            MultiCellCertificate::seal("acme/u-1", &cov).expect_err("a missed cell does NOT seal");
        assert_eq!(gap.cells_missed, 1);
        assert_eq!(gap.missed, vec![cell("cell-fr-par-3")]);
        assert_eq!(gap.cells_total, 3);
    }

    #[test]
    fn a_cell_with_an_incomplete_inner_fan_out_is_missed() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let mut cov = MultiCellCoverage::new(set);
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-1"),
            complete_cell_cert("acme/u@1"),
        ));
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-2"),
            incomplete_cell_cert("acme/u@2"),
        ));
        assert_eq!(
            cov.cells_missed(),
            1,
            "a cell that did not fully erase is a missed cell"
        );
        assert_eq!(cov.missed(), vec![cell("cell-fr-par-2")]);
        assert!(!cov.is_complete());
    }

    #[test]
    fn a_stray_receipt_outside_the_target_set_is_rejected() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let mut cov = MultiCellCoverage::new(set);
        let accepted = cov.record_receipt(PerCellReceipt::new(
            cell("cell-de-fra-9"),
            complete_cell_cert("acme/u@9"),
        ));
        assert!(!accepted, "a stray non-member cell receipt is rejected");
        assert_eq!(cov.cells_missed(), 2, "both real target cells still missed");
    }

    #[test]
    fn per_cell_receipt_is_pii_free_and_content_addressed() {
        let a = PerCellReceipt::new(cell("cell-fr-par-1"), complete_cell_cert("acme/u@1"));
        let a2 = PerCellReceipt::new(cell("cell-fr-par-1"), complete_cell_cert("acme/u@1"));
        assert_eq!(a.content_hash, a2.content_hash, "deterministic");
        let b = PerCellReceipt::new(cell("cell-fr-par-2"), complete_cell_cert("acme/u@1"));
        assert_ne!(
            a.content_hash, b.content_hash,
            "the cell id is in the content address"
        );
        assert!(a.content_hash.starts_with("blake3:"));
        assert!(a.cell_is_complete());
    }

    #[test]
    fn multi_cell_certificate_is_complete_validates_each_field() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let mut cov = MultiCellCoverage::new(set);
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-1"),
            complete_cell_cert("acme/u@1"),
        ));
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-2"),
            complete_cell_cert("acme/u@2"),
        ));
        let good = MultiCellCertificate::seal("acme/u", &cov).unwrap();
        assert!(good.is_complete());

        let mut t1 = good.clone();
        t1.cells_missed = 1;
        assert!(!t1.is_complete(), "a non-zero missed count fails the gate");

        let mut t2 = good.clone();
        t2.per_cell.pop();
        assert!(!t2.is_complete(), "a dropped per-cell line fails the gate");

        let mut t3 = good.clone();
        t3.per_cell[0].cell_certificate.holders_missed = 1;
        assert!(
            !t3.is_complete(),
            "a per-cell certificate marked incomplete fails the gate"
        );
    }

    #[test]
    fn multi_cell_content_address_is_deterministic_and_scope_sensitive() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let build = |scope: &str| {
            let mut cov = MultiCellCoverage::new(set.clone());
            cov.record_receipt(PerCellReceipt::new(
                cell("cell-fr-par-1"),
                complete_cell_cert(&format!("{scope}@1")),
            ));
            cov.record_receipt(PerCellReceipt::new(
                cell("cell-fr-par-2"),
                complete_cell_cert(&format!("{scope}@2")),
            ));
            MultiCellCertificate::seal(scope, &cov).unwrap()
        };
        let a = build("acme/u-1");
        let a2 = build("acme/u-1");
        assert_eq!(a.content_hash, a2.content_hash, "deterministic");
        let b = build("acme/u-2");
        assert_ne!(
            a.content_hash, b.content_hash,
            "the scope is in the content address"
        );
    }

    #[test]
    fn resolution_is_cell_local_over_the_pii_free_pointer() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let pointer = sample_pointer("cell-fr-par-1");
        let mut cells_seen: Vec<String> = Vec::new();
        let cert = MultiCellFanOut::new()
            .fan_out("acme/u-1", &set, &pointer, |c, p| {
                assert_eq!(
                    p.subject().artifact_ref().0,
                    "myelin://01J0ACME/issues/issue/42"
                );
                assert_eq!(p.artifact_type(), &ArtifactType::Issue);
                cells_seen.push(c.as_str().to_string());
                complete_cell_cert(&format!("acme/u-1@{}", c.as_str()))
            })
            .unwrap();
        assert!(cert.is_complete());
        assert_eq!(cells_seen.len(), 2, "every cell was resolved cell-locally");
        assert!(cells_seen.contains(&"cell-fr-par-1".to_string()));
        assert!(cells_seen.contains(&"cell-fr-par-2".to_string()));
    }
}
