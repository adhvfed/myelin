use myelin_agent::{DryRun, InboxEvent, ProposedEffect};

struct ProviderDryRun;

impl DryRun for ProviderDryRun {
    fn dry_run(&self, inbox: InboxEvent) -> Vec<ProposedEffect> {
        if inbox.0 == "mention" {
            vec![
                ProposedEffect("comment".into()),
                ProposedEffect("label".into()),
            ]
        } else {
            vec![]
        }
    }
}

#[test]
fn cdc_8_7_dry_run_plans_without_applying() {
    let provider = ProviderDryRun;

    let plan = provider.dry_run(InboxEvent("mention".into()));
    assert_eq!(
        plan,
        vec![
            ProposedEffect("comment".into()),
            ProposedEffect("label".into()),
        ]
    );

    assert!(provider.dry_run(InboxEvent("noise".into())).is_empty());
}
