use myelin_identity::Principal;
use myelin_tenancy::TenantId;

use crate::escalation::DurableWheel;
use crate::read_state::ReadState;
use crate::router::InboxProjection;

pub const SNOOZE_TIMER_NS: &str = "snooze:";

pub fn snooze_timer_key(tenant: &TenantId, recipient: &str, item_id: &str) -> String {
    format!("{SNOOZE_TIMER_NS}{}:{}:{}", tenant.0, recipient, item_id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResurfaceOutcome {
    Resurfaced,
    NoOp,
}

pub struct SnoozeResurfacer<W: DurableWheel> {
    wheel: W,
}

impl<W: DurableWheel> SnoozeResurfacer<W> {
    pub fn new(wheel: W) -> SnoozeResurfacer<W> {
        SnoozeResurfacer { wheel }
    }

    pub fn wheel(&self) -> &W {
        &self.wheel
    }

    pub fn arm(&self, tenant: &TenantId, recipient: &str, item_id: &str, until_minutes: u32) {
        let key = snooze_timer_key(tenant, recipient, item_id);
        self.wheel.schedule_timer(&key, until_minutes);
    }

    pub fn has_timer(&self, tenant: &TenantId, recipient: &str, item_id: &str) -> bool {
        self.wheel
            .has_timer(&snooze_timer_key(tenant, recipient, item_id))
    }

    pub fn resurface_due(
        &self,
        inbox: &InboxProjection,
        tenant: &TenantId,
        recipient: &str,
        item_id: &str,
    ) -> ResurfaceOutcome {
        let key = snooze_timer_key(tenant, recipient, item_id);
        if !self.wheel.fire_due(&key) {
            return ResurfaceOutcome::NoOp;
        }
        let mut flipped = false;
        let found = inbox.mutate_state(tenant, recipient, item_id, |row| {
            if row.state == ReadState::Snoozed.token() {
                row.state = ReadState::Unread.token().to_string();
                row.snooze_until = None;
                flipped = true;
            }
        });
        if found && flipped {
            ResurfaceOutcome::Resurfaced
        } else {
            ResurfaceOutcome::NoOp
        }
    }

    pub fn cancel(&self, tenant: &TenantId, recipient: &str, item_id: &str) {
        self.wheel
            .cancel_timer(&snooze_timer_key(tenant, recipient, item_id));
    }
}

pub fn snooze_and_arm<W: DurableWheel>(
    inbox: &InboxProjection,
    resurfacer: &SnoozeResurfacer<W>,
    principal: &Principal,
    item_id: &str,
    until: &str,
    until_minutes: u32,
) -> Result<(), crate::read_state::ReadStateError> {
    crate::read_state::snooze(inbox, principal, item_id, until)?;
    resurfacer.arm(
        &principal.tenant,
        principal.principal_id.0.as_str(),
        item_id,
        until_minutes,
    );
    Ok(())
}

#[cfg(test)]
mod tests;
