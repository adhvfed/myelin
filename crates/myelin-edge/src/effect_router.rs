use std::collections::BTreeMap;

use myelin_agent::{EffectApi, EffectAuthority, EffectResult, ProposedEffect, RunCtx};

pub struct RoutedEffectApi {
    routes: BTreeMap<String, Box<dyn EffectApi>>,
}

impl RoutedEffectApi {
    pub fn try_new(
        routes: impl IntoIterator<Item = (&'static str, Box<dyn EffectApi>)>,
    ) -> Result<Self, String> {
        let mut by_subsystem = BTreeMap::new();
        for (subsystem, api) in routes {
            if subsystem.is_empty()
                || !subsystem
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            {
                return Err(format!("invalid effect subsystem route `{subsystem}`"));
            }
            if by_subsystem.insert(subsystem.to_string(), api).is_some() {
                return Err(format!("duplicate effect subsystem route `{subsystem}`"));
            }
        }
        if by_subsystem.is_empty() {
            return Err("the MCP effect router requires at least one subsystem".into());
        }
        Ok(Self {
            routes: by_subsystem,
        })
    }
}

impl EffectApi for RoutedEffectApi {
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "MCP mutation requires signed run-token authority before subsystem routing".into(),
        )
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        let Some((subsystem, _)) = authority.tool.split_once('.') else {
            return EffectResult::Denied(format!(
                "effect tool `{}` has no canonical subsystem",
                authority.tool
            ));
        };
        let Some(api) = self.routes.get(subsystem) else {
            return EffectResult::Denied(format!(
                "effect subsystem `{subsystem}` is registered but not wired"
            ));
        };
        api.apply_authorized(run, authority, effect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectApproval, EventId};
    use myelin_identity::{PrincipalId, RunToken};

    struct Applied(&'static str);

    impl EffectApi for Applied {
        fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
            panic!("direct routing is forbidden")
        }

        fn apply_authorized(
            &self,
            _run: &RunCtx,
            _authority: &EffectAuthority,
            _effect: ProposedEffect,
        ) -> EffectResult {
            EffectResult::Applied(EventId(self.0.into()))
        }
    }

    fn authority(tool: &str) -> EffectAuthority {
        EffectAuthority {
            run_token: RunToken {
                token: "secret".into(),
                jti: "jti".into(),
            },
            principal_id: PrincipalId("agent:a".into()),
            tool: tool.into(),
            idempotency_key: "retry-1".into(),
            approval: EffectApproval::NotRequired,
        }
    }

    #[test]
    fn exact_subsystem_routes_share_one_governed_effect_boundary() {
        let router = RoutedEffectApi::try_new([
            ("git", Box::new(Applied("git")) as Box<dyn EffectApi>),
            ("chat", Box::new(Applied("chat")) as Box<dyn EffectApi>),
        ])
        .unwrap();
        let run = RunCtx("run".into());
        let effect = ProposedEffect("opaque".into());
        assert_eq!(
            router.apply_authorized(&run, &authority("chat.post_message"), effect.clone()),
            EffectResult::Applied(EventId("chat".into()))
        );
        assert_eq!(
            router.apply_authorized(&run, &authority("git.open_pr"), effect),
            EffectResult::Applied(EventId("git".into()))
        );
    }

    #[test]
    fn invalid_duplicate_and_unwired_routes_fail_loudly() {
        assert!(RoutedEffectApi::try_new([]).is_err());
        assert!(RoutedEffectApi::try_new([
            ("git", Box::new(Applied("one")) as Box<dyn EffectApi>),
            ("git", Box::new(Applied("two")) as Box<dyn EffectApi>),
        ])
        .is_err());
        let router =
            RoutedEffectApi::try_new([("git", Box::new(Applied("git")) as Box<dyn EffectApi>)])
                .unwrap();
        assert!(matches!(
            router.apply_authorized(
                &RunCtx("run".into()),
                &authority("chat.post_message"),
                ProposedEffect("opaque".into())
            ),
            EffectResult::Denied(reason) if reason.contains("not wired")
        ));
    }
}
