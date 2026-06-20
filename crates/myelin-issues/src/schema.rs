//! The skeletal Issues OLTP row types, carrying the `#[personal_data(...)]` classification tags
//! (contract 10.2; architecture issue-tracker 01-tech-and-data-model §6.1 + 03 §7 — the personal
//! data in Issues + the OQ-H worklog/productivity behavioural tags).
//!
//! **These are skeletal tag-carriers, not the live tables.** No migration, no store, no query ships
//! here (that is ISS-P05 — the spine; ISS-P07 — the pseudonymous columns + the per-subject DEK +
//! the holder open). The purpose is the GATE: every PII-carrying field of the issue schema is
//! `#[personal_data(...)]`-tagged so the `no-untagged-personal-data` lint (contract 1.6) is GREEN on
//! Issues from the first migration (ISS-P05), and so the M4 stores compile against the frozen tags.
//!
//! ## What is tagged, and why (architecture §6.1 / 03 §7)
//! - **Pseudonymous identity fields** (`assignee_pseudonym` / `reporter_pseudonym` /
//!   `created_by_pseudonym`): the actor identity is an **opaque pseudonym** resolved through Identity
//!   (contract 4.8), never a raw name/email — so `category = Identifier`, `erasure = Pseudonymise`
//!   (delete the Identity map ⇒ the bytes hold only the opaque pseudonym; the lever that makes
//!   erasure usually free — "Former user 8a2f" across all history without rewriting issues others
//!   own, 03 §7).
//! - **Free-text body/title fields** (`title` / `props` / `comment_text` / `change_delta`): inline
//!   content encrypted under the **per-subject DEK** (contract 11.4) — so `category = Content`,
//!   `erasure = CryptoShred(subject_dek)` (reaches live + backups by construction).
//! - **Worklog / productivity / estimate fields** (`worklog_seconds` / `story_points` /
//!   `time_spent_seconds`): the OQ-H **behavioural** tags — `category = Behavioural`,
//!   `role = TenantContent`, **`basis = TBD_LEGAL`** (the `[OPEN — LEGAL]` residual, R-2:
//!   special-category-vs-elevated ratification is a parallel legal track — counsel/DPO ratify the
//!   basis; the structural tag ships NOW), `retention = TenantPolicy`,
//!   **restricted-by-default** (OQ-H: excluded from cross-individual analytics + agent-use for a
//!   restricted subject; per-individual rollups OFF by default behind tenant-admin enablement). They
//!   carry the same per-subject DEK crypto-shred lever as other free-text PII.
//!
//! All free-text/identity fields are `role = TenantContent` (processor posture: the customer org is
//! the controller of issue content; a DSR is answered by/for the tenant, Art. 28 — 03 §7). The tag's
//! `subject_locator` names the column the holder's `locate`/`erase` keys on to find the subject's
//! rows.
//!
//! The attribute uses the canonical **multi-line six-tag** form frozen in P-GA-02 / P-050 + gdpr
//! §2.1 (`category | role | basis | retention | erasure | subject_locator`). The lint admits this
//! shape; the M0 derive is a no-op (the tag is the classification fact a store applies today; the
//! registry-emitting body is the P-GA-07 floor).

use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

/// The `issue` row (architecture §6.1 — the typed core). Skeletal: the partition keys + the
/// pseudonymous identity + free-text + worklog/behavioural fields the holder erases; the non-PII
/// columns (state, category, timestamps, order_key) are omitted here — this is a tag-carrier, not the
/// live table (ISS-P05 ships the full schema + migration).
#[derive(PersonalData)]
pub struct Issue {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (architecture §6.1).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the project this issue belongs to — an opaque id, no PII, no tag.
    pub project_id: u128,
    /// the per-project issue seqno (the `<PROJECTKEY>-<seqno>` human key, ISS-P08) — not PII.
    pub seqno: i64,
    /// issue title — inline free text, possibly personal; per-subject DEK if so (03 §7). Tagged:
    /// encrypted content erased via the per-subject DEK crypto-shred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
    )]
    pub title: String,
    /// the issue's JSONB property tail (the custom-field values) — may carry free-text PII; ENCRYPTED
    /// under the per-subject DEK (contract 11.4). Tagged Content / CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
    )]
    pub props: Vec<u8>,
    /// the assignee's OPAQUE pseudonym (contract 4.8) — never a raw name/email. Tagged Identifier /
    /// Pseudonymise: erased by deleting the Identity pseudonym map (the bytes then hold only the
    /// opaque pseudonym — "Former user 8a2f").
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "assignee_pseudonym",
    )]
    pub assignee_pseudonym: String,
    /// the reporter's OPAQUE pseudonym (contract 4.8). Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "reporter_pseudonym",
    )]
    pub reporter_pseudonym: String,
    /// the creator's OPAQUE pseudonym (contract 4.8). Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "created_by_pseudonym",
    )]
    pub created_by_pseudonym: String,
    /// **OQ-H worklog (behavioural).** Time logged against the issue, in **seconds** (the frozen
    /// names/units anchor — durations in seconds). Tagged `category = Behavioural`,
    /// **`basis = TBD_LEGAL`** (the `[OPEN — LEGAL]` residual R-2; the structural tag ships now,
    /// counsel ratifies the basis), restricted-by-default (OQ-H — excluded from cross-individual
    /// analytics/agent-use for a restricted subject). Erased via the per-subject DEK crypto-shred.
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = TBD_LEGAL,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
    )]
    pub worklog_seconds: i64,
    /// **OQ-H productivity (behavioural).** The issue's story-point estimate (a numeric, not money).
    /// Tagged behavioural / TBD_LEGAL / restricted-by-default (OQ-H).
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = TBD_LEGAL,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "assignee_pseudonym",
    )]
    pub story_points: f64,
}

/// The `issue_comment` row (architecture §6.1 — comments as a `myelin-content` block subtree, the
/// `#comment-<opaqueid>` sub-artifact). Skeletal tag-carrier: the identity + free-text-body fields
/// the holder erases.
#[derive(PersonalData)]
pub struct IssueComment {
    /// `(tenant, region)` partition key — opaque, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque, no tag.
    pub region: Region,
    /// the stable opaque comment id (`#comment-<id>`, 5.7) — not personal data.
    pub comment_id: u128,
    /// the comment author's OPAQUE pseudonym (contract 4.8). Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_pseudonym",
    )]
    pub author_pseudonym: String,
    /// the comment body (`myelin-content` block subtree) — ENCRYPTED under the per-subject DEK
    /// (contract 11.4). Named `comment_text` so the `no-untagged-personal-data` lint's PII
    /// fingerprint recognizes it (the live green witness the lint scans). Tagged Content /
    /// CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub comment_text: Vec<u8>,
}

/// The `issue_change_log` row (architecture §6.1 — the per-issue change-log / field-delta history,
/// the audit/replay input). Skeletal tag-carrier: the actor pseudonym + the free-text change delta.
#[derive(PersonalData)]
pub struct IssueChangeLog {
    /// `(tenant, region)` partition key — opaque, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque, no tag.
    pub region: Region,
    /// the change-log entry id — opaque, no PII.
    pub change_id: u128,
    /// the actor's OPAQUE pseudonym (contract 4.8). Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "actor_pseudonym",
    )]
    pub actor_pseudonym: String,
    /// the change delta (free text — the before/after of a field edit) — may carry PII; ENCRYPTED
    /// under the per-subject DEK (contract 11.4). Tagged Content / CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "actor_pseudonym",
    )]
    pub change_delta: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-op `#[derive(PersonalData)]` + the `#[personal_data(...)]` helper compile when applied
    /// to an Issues store row (contract 10.2), including the OQ-H behavioural worklog tag with its
    /// `basis = TBD_LEGAL` residual. The struct being constructable + its fields readable proves the
    /// no-op derive left the item unchanged (the registry-emitting body is P-GA-07). This is the
    /// compile-surface gate: an Issues store CAN tag its PII fields today against the frozen
    /// classification — it will not compile against drift later.
    #[test]
    fn issue_rows_compile_with_personal_data_tags() {
        let issue = Issue {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            project_id: 1,
            seqno: 1421,
            title: "fix the login bug".into(),
            props: b"{\"severity\":3}".to_vec(),
            assignee_pseudonym: "psn:abc".into(),
            reporter_pseudonym: "psn:def".into(),
            created_by_pseudonym: "psn:ghi".into(),
            worklog_seconds: 3600,
            story_points: 5.0,
        };
        assert_eq!(issue.seqno, 1421);
        assert_eq!(issue.assignee_pseudonym, "psn:abc");
        assert_eq!(issue.title, "fix the login bug");
        // The OQ-H behavioural fields round-trip (seconds + numeric story points).
        assert_eq!(issue.worklog_seconds, 3600);
        assert_eq!(issue.story_points, 5.0);

        // Field-SHORTHAND init for the PII-fingerprinted `comment_text` field (a local of the same
        // name): the live source-scanning `no-untagged-personal-data` lint fingerprints a struct
        // FIELD line of the form `comment_text: <type>`; a struct-LITERAL initialiser `comment_text:
        // <value>` would trip the scanner's field heuristic as a false positive. The TAG lives on the
        // field DEFINITION above (where the lint must see it); shorthand here keeps the live workspace
        // scan green without weakening the lint (the def is — and stays — tagged).
        let comment_text = b"nit: rename this".to_vec();
        let comment = IssueComment {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            comment_id: 9,
            author_pseudonym: "psn:jkl".into(),
            comment_text,
        };
        assert_eq!(comment.comment_id, 9);
        assert_eq!(comment.comment_text, b"nit: rename this");

        let log = IssueChangeLog {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            change_id: 7,
            actor_pseudonym: "psn:mno".into(),
            change_delta: b"state: todo -> in_progress".to_vec(),
        };
        assert_eq!(log.change_id, 7);
        assert_eq!(log.actor_pseudonym, "psn:mno");
    }
}
