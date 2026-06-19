//! The skeletal git OLTP row types, carrying the `#[personal_data(...)]` classification tags
//! (contract 10.2; architecture 01-tech-and-data-model §4.3 — the hosting OLTP tables).
//!
//! **These are skeletal tag-carriers, not the live tables.** No migration, no store, no query ships
//! here (that is GIT-P8). The purpose is the GATE: every PII-carrying field of the git schema is
//! `#[personal_data(...)]`-tagged so the `no-untagged-personal-data` lint (contract 1.6) is GREEN on
//! git from the first migration (GIT-P8), and so the M1 stores compile against the frozen tags.
//!
//! ## What is tagged, and why (architecture §4.3 / §4.5)
//! - **Pseudonym fields** (`author_pseudonym` / `reviewer_pseudonym` / `pusher_pseudonym`): the
//!   actor identity is an **opaque pseudonym** (GIT-1, contract 4.8), never a raw name/email — so
//!   `category = Identifier`, `erasure = Pseudonymise` (delete the Identity map ⇒ the bytes hold
//!   only the opaque pseudonym; the lever that makes erasure usually free).
//! - **Free-text body/title fields** (`comment_text` / `pr_body` / `pr_title`): inline content
//!   encrypted under the **per-subject DEK** (contract 11.4) — so `category = Content`,
//!   `erasure = CryptoShred(subject_dek)` (reaches live + backups by construction).
//!
//! All are `role = TenantContent` (processor posture: the customer org is the controller of repo
//! content; a DSR is answered by/for the tenant, Art. 28 — §6). The tag's `subject_locator` names
//! the column the holder's `locate`/`erase` keys on to find the subject's rows (§4.5).
//!
//! The attribute uses the canonical **multi-line six-tag** form frozen in P-GA-02 / P-050 + gdpr
//! §2.1 (`category | role | basis | retention | erasure | subject_locator`). The lint admits this
//! shape (the GDPR P-GA-03 sharpening); the M0 derive is a no-op (the tag is the classification fact
//! a store applies today, the registry-emitting body is the P-GA-07 floor).

use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

/// The `pull_request` row (architecture §4.3). Skeletal: the partition keys + the identity +
/// free-text fields the holder erases; the non-PII columns (oids, state, timestamps) are omitted
/// here — this is a tag-carrier, not the live table (GIT-P8 ships the full schema + migration).
#[derive(PersonalData)]
pub struct PullRequest {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (architecture §4.3).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the repo this PR belongs to — an opaque id, no PII, no tag.
    pub repo_id: u128,
    /// the per-repo PR number — not personal data.
    pub pr_number: i32,
    /// PR title — inline free text, possibly personal; per-subject DEK if so (§4.3 comment on
    /// `title`). Tagged: encrypted content erased via the per-subject DEK crypto-shred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub pr_title: String,
    /// PR body markdown — ENCRYPTED under the per-subject DEK (contract 11.4). Tagged Content /
    /// CryptoShred so the crypto-shred erase reaches it live + in backups.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub pr_body: Vec<u8>,
    /// the PR author's OPAQUE pseudonym (GIT-1, contract 4.8) — never a raw name/email. Tagged
    /// Identifier / Pseudonymise: erased by deleting the Identity pseudonym map (the bytes then
    /// hold only the opaque pseudonym).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_pseudonym",
    )]
    pub author_pseudonym: String,
}

/// The `review` row (architecture §4.3). Skeletal tag-carrier.
#[derive(PersonalData)]
pub struct Review {
    /// `(tenant, region)` partition key — opaque, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque, no tag.
    pub region: Region,
    /// the review id — opaque, no PII.
    pub review_id: u128,
    /// the reviewer's OPAQUE pseudonym (GIT-1, contract 4.8). Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "reviewer_pseudonym",
    )]
    pub reviewer_pseudonym: String,
}

/// The `review_comment` row (architecture §4.3 — "THE anchoring battleground"). Skeletal
/// tag-carrier: the identity + free-text-body fields the holder erases.
#[derive(PersonalData)]
pub struct ReviewComment {
    /// `(tenant, region)` partition key — opaque, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque, no tag.
    pub region: Region,
    /// the stable opaque comment id (`#comment-<id>`, 5.7) — not personal data.
    pub comment_id: u128,
    /// the comment author's OPAQUE pseudonym (GIT-1, contract 4.8). Tagged Identifier /
    /// Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_pseudonym",
    )]
    pub author_pseudonym: String,
    /// the comment body markdown — ENCRYPTED under the per-subject DEK (contract 11.4). Named
    /// `comment_text` so the `no-untagged-personal-data` lint's PII fingerprint recognizes it (it
    /// is the live green witness the lint scans). Tagged Content / CryptoShred.
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

/// The `git_reflog` row (architecture §4.2). Operational PII: a pseudonymised actor (GIT-1) +
/// crypto-shred via the per-tenant blob DEK. Skeletal tag-carrier.
#[derive(PersonalData)]
pub struct Reflog {
    /// `(tenant, region)` partition key — opaque, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque, no tag.
    pub region: Region,
    /// the repo this reflog entry belongs to — opaque id, no PII.
    pub repo_id: u128,
    /// the ref name (e.g. `refs/heads/main`) — not personal data.
    pub ref_name: String,
    /// the PUSHER's OPAQUE pseudonym (GIT-1, contract 4.8) — the §4.2 `actor_pseudonym`, never a
    /// raw name/email. Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = LegitimateInterest(git_ops_lia),
        retention = TenantPolicy,
        erasure = Pseudonymise,
        subject_locator = "pusher_pseudonym",
    )]
    pub pusher_pseudonym: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-op `#[derive(PersonalData)]` + the `#[personal_data(...)]` helper compile when applied
    /// to a git store row (contract 10.2). The struct being constructable + its fields readable
    /// proves the no-op derive left the item unchanged (the registry-emitting body is P-GA-07).
    /// This is the compile-surface gate: a git store CAN tag its PII fields today against the frozen
    /// classification — it will not compile against drift later.
    #[test]
    fn git_rows_compile_with_personal_data_tags() {
        let pr = PullRequest {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            repo_id: 1,
            pr_number: 42,
            pr_title: "fix the bug".into(),
            pr_body: b"please review".to_vec(),
            author_pseudonym: "psn:abc".into(),
        };
        assert_eq!(pr.pr_number, 42);
        assert_eq!(pr.author_pseudonym, "psn:abc");
        assert_eq!(pr.pr_title, "fix the bug");

        let review = Review {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            review_id: 7,
            reviewer_pseudonym: "psn:def".into(),
        };
        assert_eq!(review.reviewer_pseudonym, "psn:def");

        // Field-SHORTHAND init for the PII-fingerprinted `comment_text` field (a local of the same
        // name): the live source-scanning `no-untagged-personal-data` lint fingerprints a struct
        // FIELD line of the form `comment_text: <type>`; a struct-LITERAL initialiser `comment_text:
        // <value>` would trip the scanner's field heuristic as a false positive. The TAG lives on
        // the field DEFINITION above (where the lint must see it); shorthand here keeps the live
        // workspace scan green without weakening the lint (the def is — and stays — tagged).
        let comment_text = b"nit: rename this".to_vec();
        let comment = ReviewComment {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            comment_id: 9,
            author_pseudonym: "psn:ghi".into(),
            comment_text,
        };
        assert_eq!(comment.comment_id, 9);
        assert_eq!(comment.comment_text, b"nit: rename this");

        let reflog = Reflog {
            tenant: TenantId::from_token("acme"),
            region: Region::new("eu-west"),
            repo_id: 1,
            ref_name: "refs/heads/main".into(),
            pusher_pseudonym: "psn:jkl".into(),
        };
        assert_eq!(reflog.ref_name, "refs/heads/main");
        assert_eq!(reflog.pusher_pseudonym, "psn:jkl");
    }
}
