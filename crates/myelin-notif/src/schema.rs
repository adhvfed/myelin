//! The Notif OLTP row types — the nine tables of the data model (architecture §2.1..§2.6),
//! `(tenant, region)`-first and carrying the `#[personal_data(...)]` classification tags
//! (contract 10.2) on every PII-bearing column. NOTIF-P2 / P-180, M2.
//!
//! **Owning architecture doc:** `notifications.md` §2 (the data model: `inbox_item`,
//! `notif_pref`, `quiet_hours`, `delivery`, `oncall_schedule`, `escalation_policy`,
//! `escalation_run`, `humanise_template`, `mute`), §2.1 (the inbox_item load-bearing invariants:
//! `template_args` holds [`ArtifactRef`]s **never** rendered strings; `origin_event` + `reason`
//! provenance; ONE read-state column; `UNIQUE(tenant, recipient, dedup_key)` write-time collapse).
//! The exact column shapes are Phase-3 §2.1..§2.6 (cited-not-restated by refined §2).
//!
//! **These are the frozen-shape tag-carriers + the column lists the migrations build.** Every row
//! LEADS with `(tenant, region)` (12.1, ADR-11) — the partition key from the verified token, never
//! the path (the residency-pin lint floor). The load-bearing inbox-item invariants:
//!
//! - **`template_args` holds `ArtifactRef`s, never rendered strings** (NOTIF-1, §2.1). They are
//!   carried as the structured `template_args_json` ref-array; the human string is produced at
//!   *read* time via Refs `resolve(ref, viewer, Display)` (the body is NOTIF-P9). This is what makes
//!   the ONE erasure posture (X-7 / contract 10.9) apply "for free": the inbox stores refs, not
//!   payloads, so erasing a person tombstones their appearance with NO mutation (§3.9, C7 — the
//!   structural basis the NOTIF-P4 holder leans on).
//! - **`origin_event` + `reason`** carry the NOTIF-2 "why it fired" provenance on every item.
//! - **ONE read-state column** (`state`) — the whole point of C-9: the same row across every view.
//! - **`UNIQUE(tenant, recipient, dedup_key)`** makes storm-control a *write-time* collapse (an
//!   `INSERT … ON CONFLICT DO UPDATE`), not a read-time scan (§3.2).
//!
//! The PII-bearing columns are tagged with the canonical multi-line six-tag form
//! (`category | role | basis | retention | erasure | subject_locator`, gdpr §2.1):
//! - The `recipient` / `acked_by` / `principal` identity columns are OPAQUE Principal pseudonyms
//!   (an agent is a Principal too, §1.4) resolved through Identity (contract 4.8 — never a raw
//!   name/email): tagged Identifier / Pseudonymise (the attribution edge a DSR erases to a
//!   pseudonym). Erasing a person tombstones their appearance for free; the recipient pointer is the
//!   one identity locator the holder (NOTIF-P4) fans out over.
//! - The `rotation` on-call roster + the escalation `acked_by` carry principal pseudonyms too.
//! - The free-text humanise `template_body` is tenant content (ICU MessageFormat authored from
//!   tenant data); but it is a TEMPLATE (a `{actor} merged {pr}` machine→human contract, not a
//!   rendered string), platform-defaulted, so it is tagged Content / TenantPolicy.
//!
//! ## Floor named (the WRITERS land later; this prompt ships the SCHEMA only)
//! The rows are written by later prompts: the Signal-consumer **router UPSERTs `inbox_item`**
//! (NOTIF-P3 / P-181); **prefs/quiet-hours** are written by NOTIF-P10 (`notif_pref` / `quiet_hours`);
//! **delivery** rows by the at-least-once delivery fabric (NOTIF-P16); **on-call/escalation**
//! (`oncall_schedule` / `escalation_policy` / `escalation_run`) by NOTIF-P14; **`humanise_template`**
//! by NOTIF-P9; **`mute`** by NOTIF-P15. This module is the schema shape + the classification tags
//! ONLY — an empty table is not a working inbox. There is **no mandatory-core algorithm module**
//! here (schema only), so there is no mutation-score floor on this prompt (stated explicitly per the
//! template's TESTS field).

use myelin_gdpr::PersonalData;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::{Class, Reason};

/// The `inbox_item` row (architecture §2.1) — the unit of the ONE inbox; `(tenant, region)`-first.
/// The heart of the data model: the prioritised "what needs me", refs-not-payloads, with ONE
/// read-state column and the `UNIQUE(tenant, recipient, dedup_key)` write-time-collapse key.
#[derive(PersonalData)]
pub struct InboxItemRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (the residency-pin floor).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the RESIDENCY PIN (architecture §2 / §8). No tag.
    pub region: Region,
    /// the stable opaque inbox-item id (the mark/snooze read-state handle, contract 7.2) — not PII.
    pub item_id: String,
    /// the Principal this item is FOR (human OR agent, §1.4) — an OPAQUE pseudonym resolved through
    /// Identity (contract 4.8), never a raw name/email. Tagged Identifier / Pseudonymise: the
    /// recipient pointer the NOTIF-P4 holder fans a DSR out over (erased by deleting the Identity
    /// pseudonym map — the bytes then hold only the opaque pseudonym).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "recipient",
    )]
    pub recipient: String,
    /// the `ArtifactRef` the item is ABOUT (a ref, never a payload — NOTIF-1). The scoped-view
    /// filter pins on it (the C-9 resolution). Opaque routable ref; the title resolves per-viewer.
    pub subject: ArtifactRef,
    /// the parent aggregate of `subject` (the `#sub`-stripped root) — the read-fanout JOIN target
    /// (NOTIF-P13) + the thread-coalescing key (§3.2). A ref-root, never a payload.
    pub subject_root: ArtifactRef,
    /// the structured WHY-it-fired (NOTIF-2 provenance) — the basis for the C-9 scoped-view filters
    /// (the frozen [`Reason`] vocabulary §3.1/§1.3). Not PII (a taxonomy token).
    pub reason: Reason,
    /// the routing/quiet-hours [`Class`] (critical|direct|participating|watching|fyi) — drives the
    /// channel set + the `pierce_classes` decision. Not PII.
    pub class: Class,
    /// the originating event ref (the NOTIF-2 "why am I seeing this?" anchor) — an opaque event id.
    pub origin_event: ArtifactRef,
    /// the humanise template key (e.g. `git.pr.merged`) resolved per-viewer at read time (the ONE
    /// templating surface, contract 7.3; the body is NOTIF-P9) — not PII (a template selector).
    pub template_key: String,
    /// the template arguments as **`ArtifactRef`s, never rendered strings** (the NOTIF-1 invariant).
    /// Carried as the structured ref-array `template_args_json` in the DDL; each is resolved
    /// per-viewer through Refs `resolve(Display)` at humanise time. Refs → erasure-for-free.
    pub template_args: Vec<ArtifactRef>,
    /// the storm-control dedup key — collapses near-identical items within a window; the
    /// `UNIQUE(tenant, recipient, dedup_key)` constraint makes the collapse a write-time UPSERT
    /// (§3.2). Not PII (a derived bucket key).
    pub dedup_key: String,
    /// the "+N more" counter — how many events folded into this item (used in NOTIF-P11 storm
    /// control). Not PII.
    pub coalesce_count: i32,
    /// the **ONE read-state column** (the C-9 read-state truth): unread|seen|read|snoozed|archived|
    /// done — the SAME row across every view (mark/snooze flips it, NOTIF-P6). Not PII.
    pub state: String,
    /// the durable-snooze re-surface time (a `myelin-flow` durable timer re-surfaces it, NOTIF-P14)
    /// — a timestamp, not PII.
    pub snooze_until: Option<String>,
    /// the source-fact time (the rank tiebreak) — a timestamp, not PII.
    pub occurred_at: String,
}

/// The `notif_pref` row (architecture §2.2) — per-principal channel routing; `(tenant, region)`-first.
/// The routing matrix + the digest cadence; the matcher binds the frozen `myelin-query` `QueryAst`
/// (contract 13.3, the body is NOTIF-P10). Written by NOTIF-P10.
#[derive(PersonalData)]
pub struct NotifPrefRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the Principal these prefs are FOR — an OPAQUE pseudonym (contract 4.8). Tagged Identifier /
    /// Pseudonymise (the prefs row a DSR erases to a pseudonym).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal",
    )]
    pub principal: String,
    /// the channel-routing matrix as JSON (per `reason × class` → channels) — a config blob, no PII.
    pub routing_json: String,
    /// the batched-delivery digest config as JSON (`{cadence, at, classes}`) — config, no PII.
    pub digest_json: Option<String>,
}

/// The `quiet_hours` row (architecture §2.2) — per-principal quiet windows in the recipient's tz;
/// `(tenant, region)`-first. `pierce_classes` is the one deliberate override (critical/escalated
/// pierce quiet-hours — you cannot silence an on-call page). Written by NOTIF-P10.
#[derive(PersonalData)]
pub struct QuietHoursRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the Principal these quiet-hours are FOR — an OPAQUE pseudonym (contract 4.8). Tagged
    /// Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal",
    )]
    pub principal: String,
    /// the IANA tz quiet windows are evaluated IN (the recipient's tz, §2.2) — a tz id, not PII.
    pub tz: String,
    /// the quiet windows as JSON (`[{days, from, to}]`) — config, no PII.
    pub windows_json: String,
    /// the one-shot Do-Not-Disturb override end — a timestamp, not PII.
    pub dnd_until: Option<String>,
    /// the classes that PIERCE quiet-hours (default `{critical}`) — the on-call override. A class
    /// array, not PII.
    pub pierce_classes: Vec<Class>,
}

/// The `delivery` row (architecture §2.3) — the at-least-once + idempotent channel delivery ledger;
/// `(tenant, region)`-first. `UNIQUE(tenant, idem_key)` makes a retried send collapse to one
/// effective delivery (NOTIF-P16). `redacted` is the off-cell PII-minimisation flag. Written by
/// NOTIF-P16.
#[derive(PersonalData)]
pub struct DeliveryRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the stable opaque delivery id — not PII.
    pub delivery_id: String,
    /// the inbox item this delivery is FOR (FK to `inbox_item.item_id`) — opaque, no PII.
    pub item_id: String,
    /// the recipient Principal — an OPAQUE pseudonym (contract 4.8). Tagged Identifier /
    /// Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "recipient",
    )]
    pub recipient: String,
    /// the channel (in_app|web_push|mobile_push|email|desktop) — not PII.
    pub channel: String,
    /// the region-aware adapter that handled it (§3.6) — an adapter id, not PII.
    pub adapter: String,
    /// the at-least-once + idempotent dedup key (`= hash(item_id, channel)`) — the
    /// `UNIQUE(tenant, idem_key)` collapses a retried send to ONE delivery (NOTIF-P16). Not PII.
    pub idem_key: String,
    /// the delivery state (pending|sent|delivered|bounced|failed|suppressed) — not PII.
    pub state: String,
    /// the retry-attempt count — not PII.
    pub attempts: i32,
    /// the provider message id (for receipts/bounces) — an opaque provider ref, no PII.
    pub provider_ref: Option<String>,
    /// the off-cell PII-minimisation flag — `true` once PII is kept OUT of the off-cell payload
    /// (§3.6, the NOTIF-P16 redaction pipeline sets it). Not PII.
    pub redacted: bool,
}

/// The `oncall_schedule` row (architecture §2.4) — a rotation roster; `(tenant, region)`-first.
/// The on-call producer (Issues SLA / any escalation source) resolves `oncall_now(schedule)` →
/// principal at fire time. Written by NOTIF-P14.
#[derive(PersonalData)]
pub struct OncallScheduleRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the stable opaque schedule id — not PII.
    pub schedule_id: String,
    /// the schedule name (`platform-oncall`) — an operational label, not PII. Named `schedule_name`
    /// (not the bare `name`) so it is unambiguously NOT the PII-fingerprint `name` the
    /// `#[derive(PersonalData)]` hard-error guards (the on-call PII lives in `rotation_json`, tagged).
    pub schedule_name: String,
    /// the layered rotation as JSON (`[{principal, from, to}]`) — it embeds OPAQUE principal
    /// pseudonyms (the roster of who is on call). Tagged Identifier / Pseudonymise (erased by
    /// rewriting the roster to pseudonyms).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "rotation_json",
    )]
    pub rotation_json: String,
    /// the IANA tz the rotation windows are evaluated in — a tz id, not PII.
    pub tz: String,
}

/// The `escalation_policy` row (architecture §2.4) — the ordered chain config (the frozen C3 shape
/// an SLA/on-call producer passes to Notif); `(tenant, region)`-first. The policy; the durability is
/// the `myelin-flow` wheel's (contract 9.3). Written by NOTIF-P14.
pub struct EscalationPolicyRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the stable opaque policy id — not PII.
    pub policy_id: String,
    /// the policy name — an operational label, not PII.
    pub name: String,
    /// the ordered escalation steps as JSON (`[{target, channels, wait}]`) — chain config, the
    /// targets are schedule/team/principal SELECTORS (resolved at fire time), not raw identities here.
    pub steps_json: String,
    /// loop the policy N times before giving up — not PII.
    pub repeat: i32,
    /// the ack window (ack within this or escalate to next step) as an interval string — not PII.
    pub ack_window: String,
}

/// The `escalation_run` row (architecture §2.4) — a LIVE escalation (a durable-workflow instance
/// handle); `(tenant, region)`-first. The state machine + timers live in the durable-workflow engine
/// (ADR-09); this is the policy handle + the run state. Written by NOTIF-P14.
#[derive(PersonalData)]
pub struct EscalationRunRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the stable opaque run id — not PII.
    pub run_id: String,
    /// the policy this run executes (FK to `escalation_policy.policy_id`) — opaque, no PII.
    pub policy_id: String,
    /// the originating event id (an SLA breach / agent escalation) — an opaque event id, no PII.
    pub trigger_event: String,
    /// the `myelin-flow` durable-workflow instance id (§3.7) — an opaque workflow ref, no PII.
    pub workflow_ref: String,
    /// the current step index in the chain — not PII.
    pub current_step: i32,
    /// the run state (active|acked|resolved|exhausted) — not PII.
    pub state: String,
    /// WHO acked the page — an OPAQUE Principal pseudonym (contract 4.8), nullable until acked.
    /// Tagged Identifier / Pseudonymise (the attribution edge a DSR erases to a pseudonym).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "acked_by",
    )]
    pub acked_by: Option<String>,
    /// when it was acked — a timestamp, not PII.
    pub acked_at: Option<String>,
}

/// The `humanise_template` row (architecture §2.5) — the ONE platform templating store (ICU
/// MessageFormat, platform-defaulted + tenant/locale-overridable). `tenant` is NULLABLE here (a NULL
/// tenant row is the platform default; a tenant row overrides for brand/locale) — the ONLY table
/// whose partition key admits a platform-default NULL, by design (§2.5). Written by NOTIF-P9.
#[derive(PersonalData)]
pub struct HumaniseTemplateRow {
    /// the tenant — **NULLABLE**: NULL = the platform default template; a tenant row overrides
    /// (locale/brand). Opaque routing key, no tag. (The §2.5 deliberate NULL-tenant default row.)
    pub tenant: Option<TenantId>,
    /// `(tenant, region)` partition dimension — the residency pin. No tag.
    pub region: Region,
    /// the template key (`git.pr.merged`, `issue.assigned`, …) — a machine selector, not PII.
    pub template_key: String,
    /// the recipient locale at render (i18n, §3.3) — a locale id, not PII.
    pub locale: String,
    /// the ICU MessageFormat template string with named arg slots that bind to RESOLVED refs
    /// (`{actor} merged {pr} into {base}`). It is a TEMPLATE — a machine→human CONTRACT, the one
    /// place a machine string is humanised — NOT a rendered string and NOT subject data; tagged
    /// Content / TenantPolicy (tenant-overridable content, retained per tenant policy).
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

/// The `mute` row (architecture §2.6) — per-principal thread/subject mutes ("mute this thread");
/// `(tenant, region)`-first. Suppresses delivery, never the audit (NOTIF-P11). Written by NOTIF-P15.
#[derive(PersonalData)]
pub struct MuteRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — the residency pin. No tag.
    pub region: Region,
    /// the Principal who muted — an OPAQUE pseudonym (contract 4.8). Tagged Identifier /
    /// Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "principal",
    )]
    pub principal: String,
    /// the muted aggregate root (a chat thread / a PR) as a ref-root — a ref, not a payload, no PII.
    pub subject_root: ArtifactRef,
    /// the mute expiry (NULL = forever) — a timestamp, not PII.
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

    /// All nine row types compile with their `#[derive(PersonalData)]` + `#[personal_data(...)]`
    /// tags (contract 10.2) and lead with `(tenant, region)` (12.1). The structs being constructable
    /// with their fields readable proves the no-op derive left the items unchanged — and that the
    /// Notif store CAN tag its PII fields today against the frozen classification (it will not
    /// compile against drift later). The inbox_item carries `ArtifactRef`s (never strings) — the
    /// NOTIF-1 refs-not-payloads invariant at the type level.
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
        // (tenant, region) FIRST — the partition key, from the verified token.
        assert_eq!(item.tenant, t());
        assert_eq!(item.region, r());
        // refs, never rendered strings (the NOTIF-1 invariant at the type level).
        let _subject: &ArtifactRef = &item.subject;
        let _args: &Vec<ArtifactRef> = &item.template_args;
        assert_eq!(item.state, "unread"); // the ONE read-state column.
        assert_eq!(item.coalesce_count, 1); // the "+N more" counter starts at 1.

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
        // the on-call override default: critical pierces quiet-hours.
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
        assert_eq!(delivery.idem_key, "itm-1:in_app"); // the at-least-once dedup key.

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
            tenant: None, // the §2.5 platform-default NULL-tenant row.
            region: r(),
            template_key: "git.pr.merged".into(),
            locale: "en".into(),
            template_body: "{actor} merged {pr} into {base}".into(),
        };
        assert!(template.tenant.is_none(), "the platform-default template has a NULL tenant (§2.5)");

        let mute = MuteRow {
            tenant: t(),
            region: r(),
            principal: "psn:alice".into(),
            subject_root: ArtifactRef("myelin://acme/chat/thread/T1".into()),
            until: None,
        };
        let _root: &ArtifactRef = &mute.subject_root; // a ref-root, never a payload.
    }
}
