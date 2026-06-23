//! The skeletal CI Control-Plane OLTP row mirrors, carrying the `#[personal_data(...)]`
//! classification tags (contract 10.2; arch 01 §3 — the only PII-carrying CI columns: the
//! pseudonym-subject actor fields `ci_run.triggered_by` + `deployment.approved_by`, arch 01 §3.1 /
//! §3.7 / contract 4.8).
//!
//! **These are skeletal tag-carriers, NOT the live tables.** The live tables are the forward-only
//! migrations in [`crate::migrations`]; the store + the query path + the real writes are the
//! per-table behaviour follow-ons (CI-P12..CI-P24 — named in [`crate::migrations`]). The purpose of
//! this module is the GATE: every PII-carrying field of the CI schema is `#[personal_data(...)]`-
//! tagged so the `no-untagged-personal-data` lint (contract 1.6) is GREEN on CI from the first
//! migration, and so the M4 stores compile against the frozen tags.
//!
//! ## What is tagged, and why (arch 01 §3 / §4 / contract 4.8)
//! **Identity is stored as a *reference* (pseudonym), never copied PII.** `triggered_by` /
//! `approved_by` are **opaque pseudonym subjects** resolved through Identity
//! (`resolve_pseudonym`/`erase`, contract 4.8) — never a raw name/email. So they are tagged
//! `category = Identifier`, `erasure = Pseudonymise` (delete the Identity pseudonym map ⇒ the bytes
//! hold only the opaque pseudonym — "Former user 8a2f" across all history without rewriting the runs
//! others own). The `role = Controller` posture mirrors arch 01 §3's `role=controller` (CI is a
//! controller of its own run-provenance audit fact). The `subject_locator` names the column the
//! holder's `locate`/`erase` keys on to find the subject's rows.
//!
//! **Every other CI column is non-PII** (arch 01 §4: "Identity is referenced, never copied"): run
//! state, content-addressed refs (`commit_oid`, `definition_snapshot`, `blob_ref`), opaque ids,
//! timestamps, cost integers — none carry personal data. The inline log PII lives in the log-tier
//! BYTES (keyed per-subject DEK, Storage C1), not in a control-plane column; the control plane holds
//! only the `pii_key_ref` POINTER, not the data (arch 01 §3.5). So the CI control-plane schema's
//! ENTIRE PII surface is these two pseudonym-subject fields — the no-untagged lint is green because
//! they are tagged and nothing else is PII.
//!
//! The attribute uses the canonical **multi-line six-tag** form frozen in P-GA-02 / P-050 + gdpr
//! §2.1 (`category | role | basis | retention | erasure | subject_locator`). The lint admits this
//! shape; the M0 derive is a no-op (the tag is the classification fact a store applies today; the
//! registry-emitting body is the P-GA-07 floor).

use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

/// The `ci_run` row mirror (arch 01 §3.1). Skeletal: the `(tenant, region)` partition key + the ONE
/// PII field (`triggered_by`, a pseudonym subject) + a couple of representative non-PII columns. The
/// live table is [`crate::migrations::CREATE_CI_RUN_DDL`]; this carries the tag the lint scans.
#[derive(PersonalData)]
pub struct CiRunRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (arch 01 §3).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the run id — an opaque uuid, no PII, no tag.
    pub run_id: u128,
    /// the content-addressed commit the run ran against (the CheckStatus key half) — a hash, not
    /// PII, no tag.
    pub commit_oid: String,
    /// the trust tier stamped onto every CheckStatus — an enum string, not PII, no tag.
    pub trust_tier: String,
    /// **the actor that triggered the run — an OPAQUE pseudonym subject (contract 4.8), never a raw
    /// name/email.** Tagged `category = Identifier`, `erasure = Pseudonymise`: erased by deleting the
    /// Identity pseudonym map (the bytes then hold only the opaque pseudonym). `role = Controller`
    /// mirrors arch 01 §3's `role=controller`. This is the run-provenance PII surface (arch 01 §3.1).
    #[personal_data(
        category = Identifier,
        role = Controller,
        basis = LegitimateInterest,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "triggered_by",
    )]
    pub triggered_by: String,
}

/// The `deployment` row mirror (arch 01 §3.7). Skeletal: the `(tenant, region)` partition key + the
/// ONE PII field (`approved_by`, a pseudonym subject) + the non-PII state. The live table is
/// [`crate::migrations::CREATE_DEPLOYMENT_DDL`]; this carries the tag the lint scans.
#[derive(PersonalData)]
pub struct DeploymentRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the deployment id — an opaque uuid, no PII, no tag.
    pub dep_id: u128,
    /// the deployment lifecycle state — an enum string, not PII, no tag.
    pub state: String,
    /// **the approver of a protected-env deploy — an OPAQUE pseudonym subject (contract 4.8), never
    /// a raw name/email.** Tagged `category = Identifier`, `erasure = Pseudonymise` (delete the
    /// Identity pseudonym map ⇒ "Former user 8a2f"). `role = Controller` mirrors arch 01 §3's
    /// `role=controller`. The deploy-approval PII surface (arch 01 §3.7 / the HITL gate, CI-P24).
    #[personal_data(
        category = Identifier,
        role = Controller,
        basis = LegitimateInterest,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "approved_by",
    )]
    pub approved_by: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-op `#[derive(PersonalData)]` + the `#[personal_data(...)]` helper compile when applied
    /// to a CI control-plane row (contract 10.2), with the pseudonym-subject `Identifier` /
    /// `Pseudonymise` tag (contract 4.8). The struct being constructable + its fields readable proves
    /// the no-op derive left the item unchanged (the registry-emitting body is P-GA-07). This is the
    /// compile-surface gate: a CI store CAN tag its PII fields today against the frozen classification.
    #[test]
    fn ci_rows_compile_with_personal_data_tags() {
        let run = CiRunRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            run_id: 42,
            commit_oid: "blake3:abcd".into(),
            trust_tier: "trusted".into(),
            triggered_by: "psn:actor-8a2f".into(),
        };
        assert_eq!(run.run_id, 42);
        assert_eq!(run.triggered_by, "psn:actor-8a2f");
        assert_eq!(run.trust_tier, "trusted");

        let dep = DeploymentRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            dep_id: 7,
            state: "awaiting_approval".into(),
            approved_by: "psn:approver-1b3c".into(),
        };
        assert_eq!(dep.dep_id, 7);
        assert_eq!(dep.approved_by, "psn:approver-1b3c");
        assert_eq!(dep.state, "awaiting_approval");
    }
}
