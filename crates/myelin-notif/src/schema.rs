use myelin_gdpr::PersonalData;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::{Class, Reason};

#[derive(PersonalData)]
pub struct InboxItemRow {
    pub tenant: TenantId,
    pub region: Region,
    pub item_id: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "recipient",
    )]
    pub recipient: String,
    pub subject: ArtifactRef,
    pub subject_root: ArtifactRef,
    pub reason: Reason,
    pub class: Class,
    pub origin_event: ArtifactRef,
    pub template_key: String,
    pub template_args: Vec<ArtifactRef>,
    pub dedup_key: String,
    pub coalesce_count: i32,
    pub state: String,
    pub snooze_until: Option<String>,
    pub occurred_at: String,
}

#[derive(PersonalData)]
pub struct NotifPrefRow {
    pub tenant: TenantId,
    pub region: Region,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal",
    )]
    pub principal: String,
    pub routing_json: String,
    pub digest_json: Option<String>,
}

#[derive(PersonalData)]
pub struct QuietHoursRow {
    pub tenant: TenantId,
    pub region: Region,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal",
    )]
    pub principal: String,
    pub tz: String,
    pub windows_json: String,
    pub dnd_until: Option<String>,
    pub pierce_classes: Vec<Class>,
}

#[derive(PersonalData)]
pub struct DeliveryRow {
    pub tenant: TenantId,
    pub region: Region,
    pub delivery_id: String,
    pub item_id: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "recipient",
    )]
    pub recipient: String,
    pub channel: String,
    pub adapter: String,
    pub idem_key: String,
    pub state: String,
    pub attempts: i32,
    pub provider_ref: Option<String>,
    pub redacted: bool,
}

#[derive(PersonalData)]
pub struct OncallScheduleRow {
    pub tenant: TenantId,
    pub region: Region,
    pub schedule_id: String,
    pub schedule_name: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "rotation_json",
    )]
    pub rotation_json: String,
    pub tz: String,
}

pub struct EscalationPolicyRow {
    pub tenant: TenantId,
    pub region: Region,
    pub policy_id: String,
    pub name: String,
    pub steps_json: String,
    pub repeat: i32,
    pub ack_window: String,
}

#[derive(PersonalData)]
pub struct EscalationRunRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub policy_id: String,
    pub trigger_event: String,
    pub workflow_ref: String,
    pub current_step: i32,
    pub state: String,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "acked_by",
    )]
    pub acked_by: Option<String>,
    pub acked_at: Option<String>,
}

#[derive(PersonalData)]
pub struct HumaniseTemplateRow {
    pub tenant: Option<TenantId>,
    pub region: Region,
    pub template_key: String,
    pub locale: String,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "template_key",
    )]
    pub template_body: String,
}

#[derive(PersonalData)]
pub struct MuteRow {
    pub tenant: TenantId,
    pub region: Region,
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal",
    )]
    pub principal: String,
    pub subject_root: ArtifactRef,
    pub until: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> TenantId {
        TenantId::from_token("acme")
    }
    fn r() -> Region {
        Region::new("fr-par")
    }

    #[test]
    fn the_nine_tables_compile_tenant_region_first_with_tags() {
        let item = InboxItemRow {
            tenant: t(),
            region: r(),
            item_id: "itm-1".into(),
            recipient: "psn:alice".into(),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            subject_root: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            reason: Reason::Mentioned,
            class: Class::Direct,
            origin_event: ArtifactRef("myelin://acme/bus/event/e1".into()),
            template_key: "issue.mentioned".into(),
            template_args: vec![ArtifactRef("myelin://acme/identity/principal/u1".into())],
            dedup_key: "issue.mentioned:PROJ-1".into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
            occurred_at: "2026-06-18T00:00:00Z".into(),
        };
        assert_eq!(item.tenant, t());
        assert_eq!(item.region, r());
        let _subject: &ArtifactRef = &item.subject;
        let _args: &Vec<ArtifactRef> = &item.template_args;
        assert_eq!(item.state, "unread");
        assert_eq!(item.coalesce_count, 1);

        let pref = NotifPrefRow {
            tenant: t(),
            region: r(),
            principal: "psn:alice".into(),
            routing_json: "{}".into(),
            digest_json: None,
        };
        assert_eq!(pref.routing_json, "{}");

        let quiet = QuietHoursRow {
            tenant: t(),
            region: r(),
            principal: "psn:alice".into(),
            tz: "Europe/Paris".into(),
            windows_json: "[]".into(),
            dnd_until: None,
            pierce_classes: vec![Class::Critical],
        };
        assert_eq!(quiet.pierce_classes, vec![Class::Critical]);

        let delivery = DeliveryRow {
            tenant: t(),
            region: r(),
            delivery_id: "dlv-1".into(),
            item_id: "itm-1".into(),
            recipient: "psn:alice".into(),
            channel: "in_app".into(),
            adapter: "in_app:fr-par".into(),
            idem_key: "itm-1:in_app".into(),
            state: "pending".into(),
            attempts: 0,
            provider_ref: None,
            redacted: false,
        };
        assert_eq!(delivery.idem_key, "itm-1:in_app");

        let schedule = OncallScheduleRow {
            tenant: t(),
            region: r(),
            schedule_id: "sch-1".into(),
            schedule_name: "platform-oncall".into(),
            rotation_json: "[]".into(),
            tz: "Europe/Paris".into(),
        };
        assert_eq!(schedule.schedule_name, "platform-oncall");

        let policy = EscalationPolicyRow {
            tenant: t(),
            region: r(),
            policy_id: "pol-1".into(),
            name: "sev1".into(),
            steps_json: "[]".into(),
            repeat: 1,
            ack_window: "5m".into(),
        };
        assert_eq!(policy.ack_window, "5m");

        let run = EscalationRunRow {
            tenant: t(),
            region: r(),
            run_id: "run-1".into(),
            policy_id: "pol-1".into(),
            trigger_event: "evt:sla".into(),
            workflow_ref: "wf:1".into(),
            current_step: 0,
            state: "active".into(),
            acked_by: None,
            acked_at: None,
        };
        assert_eq!(run.state, "active");

        let template = HumaniseTemplateRow {
            tenant: None,
            region: r(),
            template_key: "git.pr.merged".into(),
            locale: "en".into(),
            template_body: "{actor} merged {pr} into {base}".into(),
        };
        assert!(
            template.tenant.is_none(),
            "the platform-default template has a NULL tenant (§2.5)"
        );

        let mute = MuteRow {
            tenant: t(),
            region: r(),
            principal: "psn:alice".into(),
            subject_root: ArtifactRef("myelin://acme/chat/thread/T1".into()),
            until: None,
        };
        let _root: &ArtifactRef = &mute.subject_root;
    }
}
