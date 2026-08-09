use myelin_identity::{Principal, PrincipalKind, PrincipalStatus};

/// The resource-side half of agent authority.
///
/// A run token proves the agent's attenuated capability set. Resource authorization is evaluated
/// against the still-active human named by the durable `on_behalf_of` binding; both conjuncts are
/// required at the final boundary.
pub(crate) fn is_active_delegation(agent: &Principal, delegator: &Principal) -> bool {
    agent.tenant == delegator.tenant
        && agent.region == delegator.region
        && agent.status == PrincipalStatus::Active
        && delegator.status == PrincipalStatus::Active
        && matches!(&delegator.kind, PrincipalKind::Human)
        && matches!(
            &agent.kind,
            PrincipalKind::Agent {
                on_behalf_of: Some(id),
                ..
            } if id == &delegator.principal_id
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn agent(delegator: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("agent:reviewer".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("external:mcp".into()),
                on_behalf_of: Some(PrincipalId(delegator.into())),
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    #[test]
    fn both_live_identities_scope_and_the_exact_on_behalf_of_binding_are_required() {
        let delegator = human("human:ada");
        let agent = agent("human:ada");
        assert!(is_active_delegation(&agent, &delegator));

        let mut suspended = delegator.clone();
        suspended.status = PrincipalStatus::Suspended;
        assert!(!is_active_delegation(&agent, &suspended));
        assert!(!is_active_delegation(&agent, &human("human:grace")));

        let mut other_tenant = delegator;
        other_tenant.tenant = TenantId("other".into());
        assert!(!is_active_delegation(&agent, &other_tenant));
    }
}
