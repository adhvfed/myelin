pub const AGENT_TOOL_READ_ATTEMPTED: &str = "agent.tool.read_attempted";
pub const AGENT_TOOL_READ_SUCCEEDED: &str = "agent.tool.read_succeeded";
pub const AGENT_TOOL_READ_DENIED: &str = "agent.tool.read_denied";

pub const AGENT_TOOL_READ_EVENT_TOKENS: &[&str] = &[
    AGENT_TOOL_READ_ATTEMPTED,
    AGENT_TOOL_READ_SUCCEEDED,
    AGENT_TOOL_READ_DENIED,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governed_read_events_are_canonical_agent_events() {
        for event_type in AGENT_TOOL_READ_EVENT_TOKENS {
            assert!(
                myelin_events::validate_event_type(event_type).is_ok(),
                "`{event_type}` must remain in the shared event grammar"
            );
        }
    }
}
