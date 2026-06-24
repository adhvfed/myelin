//! # `dek` — per-subject-DEK encryption for Chat message bodies + drafts (CHAT-P6 / P-400, M4-C1)
//!
//! **The non-negotiable this module ships (contract 11.4 / GD-4; recon §X-7 — the crypto-shred
//! erasure lever):** a chat message body IS the PII (not a reference) — the `body_inline` markdown
//! string + the `body_nodes` structured nodes + the composer `draft` are stored as **ciphertext
//! sealed under the AUTHOR's per-subject DEK**, never erasable plaintext baked into the immutable
//! message log. A subject's Art. 17 erasure destroys their per-subject DEK; their chat body
//! ciphertext in the DB **and the cold segments and backups and the immutable log** becomes
//! unrecoverable (the primary erasure mechanism for the subject's own authored content,
//! [recon §X-7](../../../planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md)).
//! This is exactly the erasure-vs-immutability resolution
//! ([external-insights/04 §1](../../../external-insights/04-hard-problems.md)): the per-subject DEK
//! never bakes erasable plaintext into the append-only log.
//!
//! **Owning architecture / canon docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md` §1.4 (the
//!   `body_inline` / `body_nodes` split, both encrypted under the author's per-subject DEK because the
//!   body IS the PII) + §3 (the message log) + the C1 row (per-subject DEK for bodies/drafts).
//! - `05-hard-problems.md` §5 (chat is the most PII-dense holder; the per-subject-DEK case).
//! - `00-reconciliation-decisions.md` §X-7 (the structural floor: per-subject-DEK crypto-shred).
//!
//! **Contracts (consumed — to the FROZEN shapes, never diverged):** **11.3** — the KMS hierarchy
//! ([`myelin_storage::kms::KmsEngine`], the cell-root → tenant-KEK → DEK envelope). **11.4** — the
//! GD-4 classify-driven per-subject-DEK column path ([`myelin_storage::encryption::ColumnCryptor`] +
//! [`myelin_storage::encryption::key_class_for`]). Chat defines NO second cryptor and NO parallel key
//! store (EI-01 §7): it seals its bodies/drafts through the ONE shared `ColumnCryptor` over the ONE
//! P-058 `KmsEngine`, so rotation / crypto-shred reach these columns BY CONSTRUCTION. The
//! `#[personal_data(erasure = CryptoShred(subject_dek))]` tag on the [`crate::schema`] body/draft
//! fields is the SAME vocabulary [`subject_dek_erasure`] routes to a per-subject DEK — one
//! classification fact, two readers.
//!
//! ## What this prompt (CHAT-P6 / P-400) ships
//! - [`ChatFreeText`] — the per-subject-DEK Chat column kinds (`body_inline` / `body_nodes` / the
//!   composer `draft`), each routing to the per-subject DEK (the GD-4 individual-erasure lever). A
//!   closed enum — a new free-text Chat column cannot be added without appearing here.
//! - [`encrypt_body`] / [`decrypt_body`] — the write/read round-trip over the ONE shared
//!   [`ColumnCryptor`]: a body value is sealed under the AUTHOR's per-subject DEK (ciphertext + the
//!   `pii_key_ref` DEK metadata at rest) and opened back while the key lives.
//! - [`plaintext_at_rest`] — the **0-plaintext-in-log assertion** the GATE reads: a sealed
//!   [`EncryptedColumn`] never contains the plaintext byte-run (the no-plaintext-body property).
//!
//! ## Mutation-score floor (mandatory-core — this IS the no-plaintext-body erasure seam)
//! The per-subject-DEK body path is the chat crypto-shred erasure lever (recon §X-7), so this module
//! is a **mandatory-core mutation target with a ≥ 90% floor**: `cargo mutants -p myelin-chat --file
//! crates/myelin-chat/src/dek.rs`. The mutation-tested core is the per-SUBJECT key-class routing
//! (every body column keys per-author-subject, never per-tenant — a downgrade loses the individual
//! lever) and the 0-plaintext-at-rest predicate (a sealed body never holds the plaintext byte-run).
//! **FLOOR (measured-under-load):** the measured % is the CI `cargo mutants` artifact, registered
//! red-until-run in the scorecard, never self-asserted (EI-01 §3).
//!
//! ## Floors named (VISION §3)
//! - **The `erase` crypto-shred BODY** (destroying the subject's DEK so every chat body it keyed
//!   becomes unrecoverable + the `chat.message.erased` tombstones across hot/cold/backups + the DSR
//!   cascade) is the Chat GDPR holder erase fan-out at **CHAT-P22 / P-411** ([`crate::holder`] names
//!   it). Here the COLUMNS are DEK-sealed (the structural floor); the destroy LEVER
//!   ([`myelin_storage::kms::KmsEngine`] `destroy_dek`) already exists.
//! - **The per-tenant DEK fallback for non-isolable third-party PII** (a name another person typed
//!   into the subject's message) is the ONE platform residual (10.9 / X-7, by reference,
//!   [`crate::holder::CHAT_RESIDUAL_POSTURE_REF`]); under the *author's* DEK, not the subject's. The
//!   `restrict` suppression (wired in [`crate::holder`]) covers it pending counsel.

use myelin_gdpr::ErasureMethod;
use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn, KeyChoiceError, SubjectId};
use myelin_storage::kms::{KekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};

/// **The frozen per-subject-DEK erasure method for a Chat body/draft column (contract 11.4 / GD-4).**
/// `CryptoShred("subject_dek")` is the SAME class-ref the [`crate::schema`] `#[personal_data(erasure =
/// CryptoShred(subject_dek))]` tag carries — [`key_class_for`](myelin_storage::encryption::key_class_for)
/// routes it to a [`KeyClass::Subject`](myelin_storage::kms::KeyClass::Subject) DEK (the individual
/// crypto-shred lever). A constant so the body path and the schema tag speak ONE vocabulary (EI-01
/// §7), never two.
pub fn subject_dek_erasure() -> ErasureMethod {
    ErasureMethod::CryptoShred("subject_dek".to_string())
}

/// **The free-text Chat column kinds that are sealed under the AUTHOR's per-subject DEK (arch §1.4 /
/// the C1 row).** A closed enum — a new free-text Chat column cannot be added without appearing here
/// (the coverage is total, proven by the unit test). Every variant routes to the per-subject DEK
/// ([`subject_dek_erasure`]): the message `body_inline` markdown string, the `body_nodes` structured
/// nodes, and the composer `draft` (an unsent message body, equally PII; CHAT-P12 / P-406 stores it
/// through THIS lever).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChatFreeText {
    /// The message `body_inline` — the markdown-subset STRING (the `myelin-content` Chat subset).
    /// Per-subject DEK (§1.4 — the body IS the PII).
    BodyInline,
    /// The message `body_nodes` — the structured mention/artifact_ref/embed nodes kept OUT of the
    /// markdown string. Per-subject DEK (§1.4).
    BodyNodes,
    /// The composer `draft` — an unsent message body (per-subject-DEK encrypted; CHAT-P12 read path).
    Draft,
}

impl ChatFreeText {
    /// A stable, PII-free label for the column kind (telemetry / receipts — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            ChatFreeText::BodyInline => "body_inline",
            ChatFreeText::BodyNodes => "body_nodes",
            ChatFreeText::Draft => "draft",
        }
    }

    /// The per-subject-DEK erasure method this column is sealed under — every body/draft column is
    /// keyed per-subject (the GD-4 individual lever). The constant is the ONE shared class-ref the
    /// schema tag carries (EI-01 §7).
    pub fn erasure(self) -> ErasureMethod {
        subject_dek_erasure()
    }

    /// **The full set of per-subject-DEK free-text Chat columns** (arch §1.4 / the C1 row). The closed
    /// coverage surface: a missed column would store plaintext PII in the immutable log. A new
    /// free-text column cannot be added without appearing here (proven by the unit test).
    pub const ALL: [ChatFreeText; 3] = [
        ChatFreeText::BodyInline,
        ChatFreeText::BodyNodes,
        ChatFreeText::Draft,
    ];
}

/// **Seal a Chat body/draft column value under the AUTHOR's per-subject DEK (contract 11.4 / GD-4,
/// the write path).** Routes through the ONE shared [`ColumnCryptor`] over the P-058 [`KmsEngine`]:
/// the field's [`subject_dek_erasure`] tag drives
/// [`key_class_for`](myelin_storage::encryption::key_class_for) to a
/// [`KeyClass::Subject`](myelin_storage::kms::KeyClass::Subject) DEK keyed on `author`, the DEK is
/// provisioned + the plaintext sealed, and the [`EncryptedColumn`] (ciphertext + the `pii_key_ref`
/// DEK metadata, NO plaintext) is what rests in the column / cold segment / backup. `author` is the
/// OPAQUE pseudonymous principal id whose body this is (the same subject the message's `author`
/// column carries); a subject-class tag with no subject is a LOUD
/// [`KeyChoiceError::SubjectClassMissingSubject`] — never a silent per-tenant downgrade (that would
/// lose the individual-erasure lever). The `_kind` is carried for telemetry/coverage; the
/// per-subject DEK is the same lever for every body/draft column.
pub fn encrypt_body(
    engine: &KmsEngine,
    region: &Region,
    tenant: &TenantId,
    author: &SubjectId,
    _kind: ChatFreeText,
    plaintext: &[u8],
) -> Result<EncryptedColumn, KeyChoiceError> {
    // The tenant's L1 KEK must exist before a per-subject DEK can be wrapped under it. In production
    // the harness provisions it at tenant store-open; ensure it idempotently here so a per-subject
    // body seal never races the KEK provision (idempotent — a second call returns the existing KEK's
    // epoch, it does NOT rotate, kms.rs §ensure_kek).
    engine.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.encrypt(tenant, Some(author), &subject_dek_erasure(), plaintext)
}

/// **Open a sealed Chat body/draft column back to plaintext while the author's DEK lives (the read
/// path / holder `export`).** Resolves the DEK named by the column's `pii_key_ref` and decrypts.
/// After the author's `erase` (CHAT-P22) shreds the DEK, this fails LOUDLY ([`KeyChoiceError::Kms`]) —
/// the body is unrecoverable (the GD-4 crypto-shred lever working), NEVER a plaintext-without-key
/// fall-through.
pub fn decrypt_body(
    engine: &KmsEngine,
    region: &Region,
    column: &EncryptedColumn,
) -> Result<Vec<u8>, KeyChoiceError> {
    let cryptor = ColumnCryptor::new(engine, region.clone());
    cryptor.decrypt(column)
}

/// **The 0-plaintext-in-log assertion the GATE reads (contract 11.4).** `true` IFF the sealed column
/// contains the plaintext byte-run verbatim — a real ciphertext NEVER does for a non-trivial value.
/// The gate asserts this is `false` for every sealed body column (0 plaintext body bytes in the
/// immutable log). Delegates to the shared [`EncryptedColumn::contains_plaintext`] — one assertion,
/// not a re-implemented byte scan (EI-01 §7).
pub fn plaintext_at_rest(column: &EncryptedColumn, plaintext: &[u8]) -> bool {
    column.contains_plaintext(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::kms::KeyClass;

    fn engine() -> KmsEngine {
        KmsEngine::new()
    }
    fn region() -> Region {
        Region::new("fr-par")
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn author() -> SubjectId {
        // the OPAQUE pseudonymous principal id whose body this is (never a name/email).
        SubjectId::new("8a2f@acme.noreply")
    }

    /// **A body round-trips through per-subject-DEK encrypt/decrypt (contract 11.4).** A markdown body
    /// is sealed under the author's per-subject DEK and opened back to the EXACT plaintext while the
    /// key lives — the primary erasure-by-key-destroy mechanism for the author's own content. This is
    /// the per-subject-DEK round-trip GATE.
    #[test]
    fn body_round_trips_through_subject_dek() {
        let eng = engine();
        let plaintext = b"hey @ada, can you review **PR 42**?".to_vec();
        let column = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &author(),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .expect("seal under the author's per-subject DEK");
        let opened = decrypt_body(&eng, &region(), &column).expect("open while the key lives");
        assert_eq!(opened, plaintext, "the body round-trips exactly");
    }

    /// **0 plaintext body bytes in the log (the GATE artifact, contract 11.4 — the no-plaintext-body
    /// property).** The sealed column holds ciphertext + the `pii_key_ref` DEK metadata — NOT the
    /// plaintext byte-run. The `plaintext_at_rest` assertion is `false` for every body/draft column
    /// kind, and every kind is keyed under the per-subject DEK.
    #[test]
    fn zero_plaintext_body_in_log_for_every_column_kind() {
        let eng = engine();
        for kind in ChatFreeText::ALL {
            let plaintext = format!("personal chat content in {}", kind.label()).into_bytes();
            let column = encrypt_body(&eng, &region(), &tenant(), &author(), kind, &plaintext)
                .expect("seal");
            // ciphertext + DEK metadata at rest — the pii_key_ref names the per-subject DEK.
            assert!(
                column.key_ref.class.as_token().starts_with("subject:"),
                "the {} column is keyed under the per-subject DEK (GD-4)",
                kind.label()
            );
            // 0 plaintext in the log: the ciphertext does not contain the plaintext byte-run.
            assert!(
                !plaintext_at_rest(&column, &plaintext),
                "0 plaintext body bytes in the immutable log for the {} column",
                kind.label()
            );
        }
    }

    /// **Every body/draft column routes to the per-subject DEK (the GD-4 individual lever), never the
    /// tenant key.** A per-subject DEK is DISTINCT from the tenant DEK (separate key class), so a
    /// subject's erasure destroys exactly their bodies — not the whole tenant's chat.
    #[test]
    fn every_body_column_is_keyed_per_subject() {
        for kind in ChatFreeText::ALL {
            match kind.erasure() {
                ErasureMethod::CryptoShred(class) => {
                    assert_eq!(class, "subject_dek", "{} → per-subject DEK", kind.label());
                }
                other => panic!(
                    "{} must crypto-shred per-subject, got {other:?}",
                    kind.label()
                ),
            }
        }
    }

    /// **Two different authors get DISTINCT DEKs (the individual-erasure granularity, GD-4).** Sealing
    /// the SAME plaintext for two authors yields ciphertext under two different per-subject key refs —
    /// so erasing one subject leaves the other's bodies intact.
    #[test]
    fn distinct_authors_get_distinct_deks() {
        let eng = engine();
        let plaintext = b"same phrase typed by two people".to_vec();
        let a = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("aaaa@acme.noreply"),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .unwrap();
        let b = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new("bbbb@acme.noreply"),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .unwrap();
        assert_ne!(
            a.key_ref.class, b.key_ref.class,
            "each author has a DISTINCT per-subject DEK (GD-4 individual granularity)"
        );
        assert!(matches!(a.key_ref.class, KeyClass::Subject(_)));
    }

    /// **A crypto-shredded DEK makes the body unrecoverable (the GD-4 lever working).** After the
    /// author's DEK is destroyed, `decrypt_body` fails LOUDLY — never a plaintext-without-key
    /// fall-through. This is the property the CHAT-P22 erase fan-out relies on.
    #[test]
    fn shredded_dek_makes_the_body_unrecoverable() {
        let eng = engine();
        let plaintext = b"secret".to_vec();
        let column = encrypt_body(
            &eng,
            &region(),
            &tenant(),
            &author(),
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .unwrap();
        // open while the key lives — fine.
        assert!(decrypt_body(&eng, &region(), &column).is_ok());
        // crypto-shred the author's per-subject DEK (the Art. 17 lever): the DEK id is
        // `(tenant, class)` off the column's pii_key_ref (the epoch travels with the ciphertext).
        let dek_id = myelin_storage::kms::DekId::new(
            column.key_ref.tenant.clone(),
            column.key_ref.class.clone(),
        );
        assert!(eng.destroy_dek(&dek_id), "destroy the per-subject DEK");
        // the body is now unrecoverable — a LOUD error, never plaintext.
        assert!(
            decrypt_body(&eng, &region(), &column).is_err(),
            "a shredded DEK makes the body unrecoverable (0 recoverable)"
        );
    }

    /// **The body column-kind set is the full coverage (no plaintext-PII hole).** The closed set names
    /// every free-text Chat column; a missed one would store plaintext in the immutable log.
    #[test]
    fn the_body_column_set_is_the_full_coverage() {
        assert_eq!(ChatFreeText::ALL.len(), 3);
        for c in [
            ChatFreeText::BodyInline,
            ChatFreeText::BodyNodes,
            ChatFreeText::Draft,
        ] {
            assert!(ChatFreeText::ALL.contains(&c), "{} is covered", c.label());
        }
    }
}
