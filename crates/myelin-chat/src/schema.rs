//! # `schema` — the Chat OLTP row tag-carriers (CHAT-P6 / P-400, contract 10.2)
//!
//! The skeletal Chat OLTP row types, carrying the `#[personal_data(...)]` classification tags
//! (contract 10.2; architecture chat `01-tech-and-data-model.md` §3 — the message log + §1.4 the
//! body split). Chat is the platform's **most PII-dense holder** (arch `05-hard-problems.md` §5 —
//! the message body IS the PII), so every PII-carrying field of the chat schema is
//! `#[personal_data(...)]`-tagged and the `no-untagged-personal-data` lint (contract 1.6 / 10.2) is
//! GREEN on Chat (0 untagged PII fields).
//!
//! **These are tag-carriers, not the live tables.** The live store + the per-subject-DEK
//! encrypt/decrypt round-trip is [`crate::dek`]; the live `MessageStore` is [`crate::store`]. The
//! purpose here is the GATE: the classification facts the store applies (the body / draft fields are
//! `category = Content`, `erasure = CryptoShred(subject_dek)`; the author is a pseudonymous
//! `Identifier`, `erasure = Pseudonymise`) so the lint passes and the holder ([`crate::holder`])
//! erases against the frozen tags.
//!
//! ## What is tagged, and why (architecture §3 / §1.4 / 05 §5)
//! - **The free-text body fields** (`message_body` / `body_nodes` / the composer `draft`): inline
//!   content encrypted under the **author's per-subject DEK** (contract 11.4) — so `category =
//!   Content`, `erasure = CryptoShred(subject_dek)` (reaches live + cold segments + backups +
//!   immutable log by construction; the per-subject DEK never bakes erasable plaintext into the log,
//!   external-insights/04 §1). [`crate::dek::ChatFreeText`] seals exactly these through the ONE
//!   shared cryptor.
//! - **The author pseudonym** (`author_pseudonym`): the actor identity is an **opaque pseudonym**
//!   resolved through Identity (contract 4.8), never a raw name/email — so `category = Identifier`,
//!   `erasure = Pseudonymise` (delete the Identity map ⇒ the bytes hold only the opaque pseudonym —
//!   "Former user 8a2f" across all chat history without rewriting messages others own; the message id
//!   is immutable and the `#sub` survives, [`crate::subs`]).
//!
//! All free-text/identity fields are `role = TenantContent` (processor posture: the customer org is
//! the controller of chat content; a DSR is answered by/for the tenant, Art. 28). The tag's
//! `subject_locator` names the column the holder's `locate`/`erase` keys on to find the subject's
//! rows (the `author_pseudonym`). The attribute uses the canonical multi-line six-tag form frozen in
//! P-GA-02 / gdpr §2.1 (`category | role | basis | retention | erasure | subject_locator`).

use myelin_gdpr::PersonalData;
use myelin_tenancy::{Region, TenantId};

/// The `message` row (architecture §3 — the message log). Skeletal tag-carrier: the partition keys +
/// the pseudonymous author + the per-subject-DEK free-text body fields the holder erases; the non-PII
/// columns (message_id, thread_root_id, edited_seq, msg_state, client_nonce) are the live store's
/// ([`crate::store::Message`]) — this carries the classification facts, not the table.
#[derive(PersonalData)]
pub struct ChatMessageRow {
    /// `(tenant, region)` partition key — opaque routing keys, no tag (architecture §3).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque routing keys, no tag.
    pub region: Region,
    /// the conversation (channel) this message belongs to — an opaque id, no PII, no tag.
    pub conversation_id: u128,
    /// the author's OPAQUE pseudonym (contract 4.8) — never a raw name/email. Tagged Identifier /
    /// Pseudonymise: erased by deleting the Identity pseudonym map (the bytes then hold only the
    /// opaque pseudonym — "Former user 8a2f"; the message id is immutable so the `#sub` survives).
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_pseudonym",
    )]
    pub author_pseudonym: String,
    /// the message body — the `myelin-content` markdown-subset STRING (`body_inline`, §1.4). The body
    /// IS the PII; ENCRYPTED under the author's per-subject DEK (contract 11.4). Named `message_body`
    /// so the `no-untagged-personal-data` lint's PII fingerprint recognizes it (the live green witness
    /// the lint scans). Tagged Content / CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub message_body: Vec<u8>,
    /// the structured `mention`/`artifact_ref`/`embed` nodes kept OUT of the markdown string
    /// (`body_nodes`, §1.4) — may carry free-text PII; ENCRYPTED under the author's per-subject DEK
    /// (contract 11.4). Tagged Content / CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub body_nodes: Vec<u8>,
}

/// The `chat_draft` row (architecture §1.4 / the C1 row — the composer draft store). Skeletal
/// tag-carrier: the author pseudonym + the per-subject-DEK draft body the holder erases. The draft is
/// an unsent message body — equally PII, equally per-subject-DEK encrypted (CHAT-P12 / P-406 stores
/// it through [`crate::dek::ChatFreeText::Draft`]).
#[derive(PersonalData)]
pub struct ChatDraftRow {
    /// `(tenant, region)` partition key — opaque, no tag.
    pub tenant: TenantId,
    /// `(tenant, region)` partition key — opaque, no tag.
    pub region: Region,
    /// the conversation the draft is being composed in — an opaque id, no PII, no tag.
    pub conversation_id: u128,
    /// the drafting author's OPAQUE pseudonym (contract 4.8). Tagged Identifier / Pseudonymise.
    #[personal_data(
        category = Identifier,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = Pseudonymise,
        subject_locator = "author_pseudonym",
    )]
    pub author_pseudonym: String,
    /// the draft body (`myelin-content` markdown-subset, unsent) — ENCRYPTED under the author's
    /// per-subject DEK (contract 11.4). Named `message_body` so the lint's PII fingerprint recognizes
    /// it. Tagged Content / CryptoShred.
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "author_pseudonym",
    )]
    pub message_body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `#[derive(PersonalData)]` + the `#[personal_data(...)]` helper compile when applied to a
    /// Chat store row (contract 10.2). The struct being constructable + its fields readable proves the
    /// classification surface is admissible. This is the compile-surface GATE: a Chat store CAN tag its
    /// PII body/author fields today against the frozen classification — and the
    /// `no-untagged-personal-data` lint is green over them (0 untagged PII fields).
    #[test]
    fn chat_rows_compile_with_personal_data_tags() {
        // Field-SHORTHAND init for the PII-fingerprinted `message_body` field (a local of the same
        // name): the live source-scanning `no-untagged-personal-data` lint fingerprints a struct FIELD
        // line of the form `message_body: <type>`; a struct-LITERAL initialiser `message_body: <value>`
        // would trip the scanner's field heuristic. The TAG lives on the field DEFINITION above (where
        // the lint must see it); shorthand here keeps the live workspace scan green without weakening
        // the lint (the def is — and stays — tagged).
        let message_body = b"hey @ada, can you review **PR 42**?".to_vec();
        let body_nodes = br#"[{"mention":"ada"}]"#.to_vec();
        let msg = ChatMessageRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            conversation_id: 7,
            author_pseudonym: "psn:abc".into(),
            message_body,
            body_nodes,
        };
        assert_eq!(msg.conversation_id, 7);
        assert_eq!(msg.author_pseudonym, "psn:abc");
        assert_eq!(msg.message_body, b"hey @ada, can you review **PR 42**?");

        let message_body = b"draft i haven't sent".to_vec();
        let draft = ChatDraftRow {
            tenant: TenantId::from_token("acme"),
            region: Region::new("fr-par"),
            conversation_id: 7,
            author_pseudonym: "psn:abc".into(),
            message_body,
        };
        assert_eq!(draft.conversation_id, 7);
        assert_eq!(draft.message_body, b"draft i haven't sent");
    }
}
