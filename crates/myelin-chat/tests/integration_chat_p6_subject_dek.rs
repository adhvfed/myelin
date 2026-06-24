//! **CHAT-P6 / P-400 — the Chat per-subject-DEK message bodies (11.4), PROVEN against the live
//! dev-stack Postgres (the no-plaintext-body floor).**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-chat --features integration \
//!     --test integration_chat_p6_subject_dek -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for CHAT-P6: a chat message body —
//! the MOST PII-dense surface on the platform (the body IS the PII, arch 05 §5) — is sealed under the
//! AUTHOR's per-subject DEK ([`myelin_chat::encrypt_body`]) and the resulting CIPHERTEXT lands in a
//! real Postgres `bytea` column shaped like the `message` hot tier's `body_inline` / `body_nodes`
//! columns. We then read the bytes back STRAIGHT FROM THE DB (not the in-memory value) and prove:
//!
//! - **0 plaintext body bytes at rest** — the stored `body_inline` / `body_nodes` bytea columns do
//!   NOT contain the plaintext byte-run (the no-plaintext-body GATE artifact, contract 11.4 — the
//!   per-subject DEK never bakes erasable plaintext into the immutable log, external-insights/04 §1);
//! - **decrypt-while-the-key-lives** — the ciphertext read back from the DB decrypts to the exact
//!   plaintext through the named per-subject DEK (the round-trip the holder `export` rides);
//! - **crypto-shred makes it unrecoverable** — after the author's per-subject DEK is destroyed, the
//!   ciphertext read FROM THE DB no longer decrypts (the GD-4 erasure lever working AT REST), never a
//!   plaintext fall-through.
//!
//! The drill is registered red-until-proven and flips green ONLY here, against the live stack — never
//! mocked. (The DEFAULT-build `cdc_11_4_10_1_chat_body_dek_holder.rs` proves the SAME properties over
//! the in-memory `KmsEngine` + in-memory column value; this is the live-Postgres at-rest artifact.)
#![cfg(feature = "integration")]

use myelin_chat::{decrypt_body, encrypt_body, ChatFreeText};
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{DekId, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn chat_body_per_subject_dek_zero_plaintext_at_rest_in_real_postgres() {
    let engine = KmsEngine::new();
    let tenant = TenantId::from_token("acmeP400");
    let region = Region::new("fr-par");
    // the OPAQUE pseudonymous author principal id whose body this is (never a name/email).
    let author = SubjectId::new("8a2f@acme.noreply");

    // a PII-laden chat body (the markdown subset) + structured nodes — the body IS the PII.
    let plaintext_inline = b"hey @ada, my email is ada@example.com, ping me re **PR 42**".to_vec();
    let plaintext_nodes = br#"[{"mention":"ada@example.com"}]"#.to_vec();

    // ── seal both body columns under the AUTHOR's per-subject DEK (the write path) ────────────────
    let inline_col = encrypt_body(
        &engine,
        &region,
        &tenant,
        &author,
        ChatFreeText::BodyInline,
        &plaintext_inline,
    )
    .expect("seal body_inline under the per-subject DEK");
    let nodes_col = encrypt_body(
        &engine,
        &region,
        &tenant,
        &author,
        ChatFreeText::BodyNodes,
        &plaintext_nodes,
    )
    .expect("seal body_nodes under the per-subject DEK");
    // both keyed under the per-subject DEK (the GD-4 individual lever).
    assert!(inline_col.key_ref.class.as_token().starts_with("subject:"));
    assert!(nodes_col.key_ref.class.as_token().starts_with("subject:"));

    // ── stand up a real message-shaped table + write the AT-REST ciphertext ───────────────────────
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let tbl = format!("chat_msg_p400_{suffix}");
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (\
           message_id TEXT PRIMARY KEY, \
           author_pseudonym TEXT NOT NULL, \
           body_inline BYTEA NOT NULL, \
           body_inline_pii_key_ref TEXT NOT NULL, \
           body_inline_nonce BYTEA NOT NULL, \
           body_nodes BYTEA NOT NULL, \
           body_nodes_pii_key_ref TEXT NOT NULL, \
           body_nodes_nonce BYTEA NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create message table");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant message table");

    // the at-rest write: the CIPHERTEXT columns (never plaintext) + the pii_key_ref naming the DEK.
    sqlx::query(&format!(
        "INSERT INTO {tbl} (message_id, author_pseudonym, \
           body_inline, body_inline_pii_key_ref, body_inline_nonce, \
           body_nodes, body_nodes_pii_key_ref, body_nodes_nonce) \
         VALUES ('01J0MSG', $1, $2, $3, $4, $5, $6, $7)"
    ))
    .bind("8a2f@acme.noreply")
    .bind(&inline_col.ciphertext)
    .bind(inline_col.key_ref.to_uri())
    .bind(inline_col.nonce.as_slice())
    .bind(&nodes_col.ciphertext)
    .bind(nodes_col.key_ref.to_uri())
    .bind(nodes_col.nonce.as_slice())
    .execute(&app)
    .await
    .expect("write the at-rest message row");

    // ── read the bytes back STRAIGHT FROM THE DB + assert the GATE invariants ─────────────────────
    let row = sqlx::query(&format!(
        "SELECT body_inline, body_nodes FROM {tbl} WHERE message_id = '01J0MSG'"
    ))
    .fetch_one(&app)
    .await
    .expect("read the at-rest row back");
    let inline_at_rest: Vec<u8> = row.get("body_inline");
    let nodes_at_rest: Vec<u8> = row.get("body_nodes");

    // (1) 0 plaintext body bytes at rest (11.4): the stored ciphertext does NOT contain the plaintext.
    let contains = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
    assert!(
        !contains(&inline_at_rest, &plaintext_inline),
        "0 plaintext body_inline at rest in the real Postgres column"
    );
    assert!(
        !contains(&inline_at_rest, b"ada@example.com"),
        "the body PII byte-run is never at rest in the immutable log"
    );
    assert!(
        !contains(&nodes_at_rest, b"ada@example.com"),
        "0 plaintext body_nodes PII at rest in the real Postgres column"
    );

    // (2) decrypt-while-the-key-lives: the ciphertext read FROM THE DB opens to the exact plaintext
    //     through the named per-subject DEK (the round-trip the holder `export` rides).
    let mut from_db = inline_col.clone();
    from_db.ciphertext = inline_at_rest;
    let opened = decrypt_body(&engine, &region, &from_db)
        .expect("the ciphertext read from the DB decrypts while the key lives");
    assert_eq!(
        opened, plaintext_inline,
        "the body round-trips through the per-subject DEK"
    );

    // (3) crypto-shred makes it unrecoverable AT REST: destroy the author's per-subject DEK and the
    //     ciphertext read from the DB no longer decrypts (the GD-4 erasure lever working) — never a
    //     plaintext fall-through.
    let dek_id = DekId::new(
        from_db.key_ref.tenant.clone(),
        from_db.key_ref.class.clone(),
    );
    assert!(
        engine.destroy_dek(&dek_id),
        "the per-subject DEK is destroyed"
    );
    assert!(
        decrypt_body(&engine, &region, &from_db).is_err(),
        "after crypto-shred the at-rest body is unrecoverable (0 recoverable) — never plaintext"
    );

    // cleanup (leave the stack up — never drop the database, only this test's table).
    let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await;
}
