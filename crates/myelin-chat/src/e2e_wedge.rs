use crate::erase::ChatEraseReport;

#[cfg(any(test, feature = "test-support"))]
pub mod e2e_dsar;
pub mod e2e_pane;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatE2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub evidence: String,
    pub leaks: u64,
}

impl ChatE2eArtifact {
    pub fn is_green(&self) -> bool {
        self.green && self.leaks == 0
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_chat_e2e_wedge() -> Vec<ChatE2eArtifact> {
    vec![
        e2e_pane::run_e2e_1_unfurl_pane(),
        e2e_dsar::run_e2e_4_chat_dsar_holder(),
    ]
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn dsar_holder_green(report: &ChatEraseReport) -> bool {
    report.receipts_complete() && report.destroyed_key_epoch.is_some() && report.cascade_published
}

#[cfg(test)]
#[path = "e2e_wedge/tests.rs"]
mod tests;
