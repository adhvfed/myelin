use myelin_agent::{ToolDef, ToolSurface};
use myelin_identity_service::ci_fragment::object_types as ci_objects;
use myelin_identity_service::ci_fragment::{ADMINISTER, DEPLOY, TRIGGER};

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};

pub const CI_SUBSYSTEM: &str = "ci";

pub const DEPLOY_TOOL: &str = "deploy";

pub const APPROVE_DEPLOY_TOOL: &str = "approve_deploy";

pub const WRITE_SECRET_TOOL: &str = "write_secret";

pub const RUN_PIPELINE_TOOL: &str = "run_pipeline";

pub const CI_TOOL_VERSION: u32 = 1;

pub fn deploy_required_caps() -> Vec<String> {
    cap(ci_objects::ENVIRONMENT, DEPLOY)
}

pub fn write_secret_required_caps() -> Vec<String> {
    cap(ci_objects::CI_PROJECT, ADMINISTER)
}

pub fn run_pipeline_required_caps() -> Vec<String> {
    cap(ci_objects::RUN, TRIGGER)
}

pub fn deploy_tool_def() -> ToolDef {
    mutate_tool_def(
        CI_SUBSYSTEM,
        DEPLOY_TOOL,
        CI_TOOL_VERSION,
        r#"{"type":"object","required":["environment","artifact"],"properties":{"environment":{"type":"string"},"artifact":{"type":"string"}}}"#,
        deploy_required_caps(),
    )
}

pub fn approve_deploy_tool_def() -> ToolDef {
    mutate_tool_def(
        CI_SUBSYSTEM,
        APPROVE_DEPLOY_TOOL,
        CI_TOOL_VERSION,
        r#"{"type":"object","required":["deployment"],"properties":{"deployment":{"type":"string"}}}"#,
        deploy_required_caps(),
    )
}

pub fn write_secret_tool_def() -> ToolDef {
    mutate_tool_def(
        CI_SUBSYSTEM,
        WRITE_SECRET_TOOL,
        CI_TOOL_VERSION,
        r#"{"type":"object","required":["ci_project","name"],"properties":{"ci_project":{"type":"string"},"name":{"type":"string"},"value_ref":{"type":"string"}}}"#,
        write_secret_required_caps(),
    )
}

pub fn run_pipeline_tool_def() -> ToolDef {
    mutate_tool_def(
        CI_SUBSYSTEM,
        RUN_PIPELINE_TOOL,
        CI_TOOL_VERSION,
        r#"{"type":"object","required":["ci_project","ref"],"properties":{"ci_project":{"type":"string"},"ref":{"type":"string"}}}"#,
        run_pipeline_required_caps(),
    )
}

pub fn ci_tool_defs() -> Vec<ToolDef> {
    vec![
        deploy_tool_def(),
        approve_deploy_tool_def(),
        write_secret_tool_def(),
        run_pipeline_tool_def(),
    ]
}

pub fn register_ci_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, ci_tool_defs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{assert_no_silent_loosening, requires_approval_default};
    use myelin_agent::{EffectKind, ToolName};

    struct Catalogue {
        defs: Vec<ToolDef>,
    }
    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs.iter().find(|d| &d.name == name)
        }
    }

    #[test]
    fn deploy_approve_and_write_secret_are_gated_by_the_frozen_default() {
        for (def, tool) in [
            (deploy_tool_def(), DEPLOY_TOOL),
            (approve_deploy_tool_def(), APPROVE_DEPLOY_TOOL),
            (write_secret_tool_def(), WRITE_SECRET_TOOL),
        ] {
            assert!(
                def.requires_approval,
                "ci.{tool} is HITL-gated (§6.3 - consequential/privileged)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(CI_SUBSYSTEM, tool),
                "ci.{tool}'s gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    #[test]
    fn run_pipeline_non_prod_is_not_gated_by_the_frozen_default() {
        let def = run_pipeline_tool_def();
        assert!(
            !def.requires_approval,
            "ci.run_pipeline (non-prod) is cheap/reversible → NOT gated (§6.3)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(CI_SUBSYSTEM, RUN_PIPELINE_TOOL),
            "ci.run_pipeline's (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
    }

    #[test]
    fn required_caps_are_the_ci_rebac_fragment_permissions() {
        assert_eq!(
            deploy_tool_def().required_caps,
            vec!["environment.deploy".to_string()]
        );
        assert_eq!(
            approve_deploy_tool_def().required_caps,
            vec!["environment.deploy".to_string()]
        );
        assert_eq!(
            write_secret_tool_def().required_caps,
            vec!["ci_project.administer".to_string()]
        );
        assert_eq!(
            run_pipeline_tool_def().required_caps,
            vec!["run.trigger".to_string()]
        );
        assert_eq!(ci_objects::ENVIRONMENT, "environment");
        assert_eq!(ci_objects::CI_PROJECT, "ci_project");
        assert_eq!(ci_objects::RUN, "run");
        assert_eq!(DEPLOY, "deploy");
        assert_eq!(ADMINISTER, "administer");
        assert_eq!(TRIGGER, "trigger");
    }

    #[test]
    fn register_ci_tools_registers_all_four_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_ci_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(
            registered.len(),
            4,
            "deploy + approve_deploy + write_secret + run_pipeline"
        );

        let deploy = cat
            .resolve(&ToolName(DEPLOY_TOOL.into()))
            .expect("deploy registered");
        assert!(deploy.requires_approval, "the registered deploy is gated");
        assert_eq!(deploy.required_caps, vec!["environment.deploy".to_string()]);

        let pipeline = cat
            .resolve(&ToolName(RUN_PIPELINE_TOOL.into()))
            .expect("run_pipeline registered");
        assert!(
            !pipeline.requires_approval,
            "the registered run_pipeline is NOT gated"
        );

        assert!(cat.resolve(&ToolName("ci.delete_project".into())).is_none());
    }

    #[test]
    fn a_hand_loosened_deploy_registration_is_rejected_loud() {
        let mut loosened = deploy_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "ci");
        assert_eq!(err.tool, "deploy");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    #[test]
    fn the_ci_tools_are_a_projection_not_a_new_engine() {
        let defs = ci_tool_defs();
        assert_eq!(defs.len(), 4);
        for d in &defs {
            assert_eq!(d.effect_kind, EffectKind::Mutate);
            assert!(d.side_effecting);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["deploy", "approve_deploy", "write_secret"],
            "the three privileged CI gates; run_pipeline (non-prod) is not gated"
        );
    }
}
