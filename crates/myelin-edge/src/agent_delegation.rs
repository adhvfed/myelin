use myelin_identity::{Principal, PrincipalKind, PrincipalStatus};

pub(crate) struct ActiveDelegation<'a> {
    actor: &'a Principal,
    access_subject: &'a Principal,
}

impl<'a> ActiveDelegation<'a> {
    pub(crate) fn establish(actor: &'a Principal, access_subject: &'a Principal) -> Option<Self> {
        (actor.tenant == access_subject.tenant
            && actor.region == access_subject.region
            && actor.status == PrincipalStatus::Active
            && access_subject.status == PrincipalStatus::Active
            && matches!(&access_subject.kind, PrincipalKind::Human)
            && matches!(
                &actor.kind,
                PrincipalKind::Agent {
                    on_behalf_of: Some(id),
                    ..
                } if id == &access_subject.principal_id
            ))
        .then_some(Self {
            actor,
            access_subject,
        })
    }

    pub(crate) fn actor(&self) -> &Principal {
        self.actor
    }

    pub(crate) fn access_subject(&self) -> &Principal {
        self.access_subject
    }
}

/// The resource-side half of agent authority.
///
/// A run token proves the agent's attenuated capability set. Resource authorization is evaluated
/// against the still-active human named by the durable `on_behalf_of` binding; both conjuncts are
/// required at the final boundary.
pub(crate) fn is_active_delegation(agent: &Principal, delegator: &Principal) -> bool {
    ActiveDelegation::establish(agent, delegator).is_some()
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
