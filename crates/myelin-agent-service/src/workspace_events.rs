pub const WORKSPACE_WRITE_FILE_ATTEMPTED: &str = "workspace.write_file.attempted";
pub const WORKSPACE_WRITE_FILE_APPLIED: &str = "workspace.write_file.applied";
pub const WORKSPACE_WRITE_FILE_GATED: &str = "workspace.write_file.gated";
pub const WORKSPACE_WRITE_FILE_DENIED: &str = "workspace.write_file.denied";
pub const WORKSPACE_WRITE_FILE_INDETERMINATE: &str = "workspace.write_file.indeterminate";
pub const WORKSPACE_EXEC_ATTEMPTED: &str = "workspace.exec.attempted";
pub const WORKSPACE_EXEC_APPLIED: &str = "workspace.exec.applied";
pub const WORKSPACE_EXEC_GATED: &str = "workspace.exec.gated";
pub const WORKSPACE_EXEC_DENIED: &str = "workspace.exec.denied";
pub const WORKSPACE_EXEC_INDETERMINATE: &str = "workspace.exec.indeterminate";

pub const WORKSPACE_GOVERNANCE_AUDIT_EVENT_TOKENS: &[&str] = &[
    WORKSPACE_WRITE_FILE_ATTEMPTED,
    WORKSPACE_WRITE_FILE_APPLIED,
    WORKSPACE_WRITE_FILE_GATED,
    WORKSPACE_WRITE_FILE_DENIED,
    WORKSPACE_WRITE_FILE_INDETERMINATE,
    WORKSPACE_EXEC_ATTEMPTED,
    WORKSPACE_EXEC_APPLIED,
    WORKSPACE_EXEC_GATED,
    WORKSPACE_EXEC_DENIED,
    WORKSPACE_EXEC_INDETERMINATE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_effect_audits_share_the_platform_event_grammar() {
        for event_type in WORKSPACE_GOVERNANCE_AUDIT_EVENT_TOKENS {
            assert!(
                myelin_events::validate_event_type(event_type).is_ok(),
                "`{event_type}` must remain a canonical event type"
            );
        }
    }
}
