use myelin_content::adf::mapping_for;
use myelin_content::{AdfNode, AdfTarget, ImportReport, Loss, MAP};

fn provider_freezes_map() -> &'static [myelin_content::AdfMapping] {
    MAP
}

fn issues_import_consumes_map(nodes: &[AdfNode], resolves_in_tenant: bool) -> ImportReport {
    let mut report = ImportReport::new();
    for &node in nodes {
        let m = mapping_for(node);
        match &m.loss {
            Loss::None => {}
            Loss::Lossy { what } => {
                report.record(node, m.target, what.to_string());
            }
            Loss::Conditional {
                what, degraded_to, ..
            } => {
                if !resolves_in_tenant {
                    report.record(node, *degraded_to, what.to_string());
                }
            }
        }
    }
    report
}

#[test]
fn cdc_13_2_provider_freezes_map_consumer_records_losses() {
    let map = provider_freezes_map();
    assert_eq!(map.len(), 25, "the frozen ADF map is exactly 25 node rows");

    let lossless_doc = [
        AdfNode::Paragraph,
        AdfNode::Heading,
        AdfNode::BulletList,
        AdfNode::Table,
        AdfNode::Expand,
    ];
    let report = issues_import_consumes_map(&lossless_doc, true);
    assert!(
        report.is_lossless(),
        "a direct-equivalent import loses nothing"
    );

    let lossy_doc = [
        AdfNode::Mention,
        AdfNode::Status,
        AdfNode::Extension,
        AdfNode::Paragraph,
    ];
    let report = issues_import_consumes_map(&lossy_doc, false);
    assert_eq!(
        report.loss_count(),
        3,
        "three lossy nodes recorded, the lossless one is not"
    );
    assert_eq!(report.conversions[0].node, AdfNode::Mention);
    assert_eq!(report.conversions[0].degraded_to, AdfTarget::PlainText);
    assert_eq!(report.conversions[1].node, AdfNode::Status);
    assert_eq!(report.conversions[2].node, AdfNode::Extension);

    let resolved = issues_import_consumes_map(&[AdfNode::Mention], true);
    assert!(
        resolved.is_lossless(),
        "an in-tenant mention survives as a structured node"
    );
}
