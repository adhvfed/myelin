use myelin_agent_service::{catalogue_cursor, PlatformToolCatalogue};
use myelin_mcp::ToolRegistry;

pub fn registry_for_subsystems(subsystems: &[&str]) -> ToolRegistry {
    let catalogue = PlatformToolCatalogue::platform().expect("the platform catalogue is valid");
    let cursors = catalogue
        .latest_definitions()
        .into_iter()
        .filter(|definition| {
            definition.exposed_over_mcp && subsystems.contains(&definition.subsystem.as_str())
        })
        .map(catalogue_cursor)
        .collect::<Vec<_>>();
    ToolRegistry::for_cursors(&cursors).expect("the selected platform tools remain available")
}
