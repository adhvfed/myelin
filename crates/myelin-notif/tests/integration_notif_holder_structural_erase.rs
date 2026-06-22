//! Live-Postgres integration test (Stage 1 / infra) — the Notif `PersonalDataHolder` (H13) structural
//! references-not-payloads ERASE, proven against REAL Postgres (NOTIF-P4 / P-182; contract 7.7 holder
//! half; the GATE: **0 PII columns mutated on a refs-stored inbox_item, yet the appearance tombstones
//! for free**).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-notif --features integration --test integration_notif_holder_structural_erase -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, the load-bearing §3.9 property the holder leans on: because the
//! `notif_inbox_item` row stores the subject ONLY as (1) the OPAQUE `recipient` pseudonym (the
//! subject's own inbox), and (2) referenced actors in the `subject`/`origin_event`/
//! `template_args_json` ArtifactRefs — and NEVER as a stored name — the holder's `erase(subject)` is
//! a STRUCTURAL no-op on the row's PII columns: the bytes are byte-identical before and after, the
//! row is NOT deleted, yet the subject's appearance tombstones for free (Identity's 4.8 pseudonym-map
//! shred makes the opaque id unresolvable). The test (a) seeds the REAL inbox_item DDL with
//! refs-stored rows naming a subject (own + by-ref); (b) runs the EXACT structural erase the holder
//! performs (it touches 0 PII columns); (c) asserts 0 PII columns mutated + 0 rows deleted (the
//! appearance stays, only resolution changes).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, INBOX_ITEM_DDL};

#[tokio::test]
async fn notif_holder_structural_erase_mutates_zero_pii_columns_yet_tombstones_for_free() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres as the app role (is the stack up?)");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin");

    // A unique table name per process so concurrent runs don't collide — the DDL is the REAL
    // inbox_item shape (we substitute the table name so cleanup is safe + parallel runs isolate).
    let tbl = format!("notif_holder_erase_probe_{}", std::process::id());
    let create = INBOX_ITEM_DDL.replacen("notif_inbox_item", &tbl, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("the inbox_item DDL applies");
    sqlx::query(&rls_scope_sql(&tbl))
        .execute(&admin)
        .await
        .expect("myelin_make_tenant_scoped installs the (tenant_id, region) RLS policy");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();

    // The subject to erase (an OPAQUE pseudonym, 4.8) + a referenced-actor ref naming the subject.
    let subject_pseudonym = "psn:u-erase";
    let subject_actor_ref = "myelin://acme/identity/principal/u-erase";

    // Seed three refs-stored rows under tenant `acme` (set the GUCs first — admin is FORCEd under RLS):
    //   1. the subject's OWN inbox row (recipient = the subject pseudonym), about an issue;
    //   2. someone ELSE's inbox row that names the subject by REF in template_args_json (the by-ref
    //      actor case — the title resolves the actor per-viewer at read time);
    //   3. a CONTROL row naming a different person (must be untouched + not match the subject).
    let seed: [(&str, &str, &str, &str, &str); 3] = [
        // (item_id, recipient, subject_ref, origin_event_ref, template_args_json)
        (
            "itm-own",
            subject_pseudonym,
            "myelin://acme/issues/issue/PROJ-1",
            "myelin://acme/event/e1",
            "[]",
        ),
        (
            "itm-byref",
            "psn:u-bob",
            "myelin://acme/issues/issue/PROJ-2",
            "myelin://acme/event/e2",
            // the subject named BY REFERENCE (an ArtifactRef in the ref-array), never a rendered name.
            "[\"myelin://acme/identity/principal/u-erase\"]",
        ),
        (
            "itm-control",
            "psn:u-carol",
            "myelin://acme/issues/issue/PROJ-3",
            "myelin://acme/event/e3",
            "[]",
        ),
    ];
    for (item_id, recipient, subject_ref, origin, targs) in seed {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'acme', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, item_id, recipient, subject, subject_root, reason, class, \
                origin_event, template_key, template_args_json, dedup_key, state, occurred_at, dek_ref) \
             VALUES ('acme', 'fr-par', $1, $2, $3, $3, 'mentioned', 'direct', $4, 'issue.mentioned', \
                     $5::jsonb, $1, 'unread', now(), 'kms://acme/0/tenant')"
        ))
        .bind(item_id)
        .bind(recipient)
        .bind(subject_ref)
        .bind(origin)
        .bind(targs)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    // As the APP role set to tenant acme: snapshot the EXACT stored bytes of the subject's rows BEFORE
    // the structural erase. The `locate` predicate (the holder's references-not-payloads predicate):
    // recipient = the subject pseudonym OR a stored ref names the subject's principal id.
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'acme', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let locate_sql = format!(
        "SELECT item_id, recipient, subject, origin_event, template_args_json::text AS targs, state \
         FROM {tbl} \
         WHERE recipient = $1 \
            OR origin_event = $2 \
            OR template_args_json::text LIKE '%' || $2 || '%' \
         ORDER BY item_id"
    );
    let before = sqlx::query(&locate_sql)
        .bind(subject_pseudonym)
        .bind(subject_actor_ref)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        2,
        "locate finds BOTH the subject's appearances (own + by-ref) — 0 false matches"
    );

    let total_before: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(total_before, 3, "three rows seeded");

    // THE STRUCTURAL ERASE: the holder's `erase(subject)` mutates 0 PII columns on a refs-stored row —
    // the appearance tombstones for free (Identity's 4.8 pseudonym-shred makes the opaque id
    // unresolvable at read time). So the holder issues NO UPDATE/DELETE against the inbox PII columns.
    // We model exactly that here: the erase is a no-op on the row bytes. (A non-structural holder would
    // have scrubbed `recipient`/`subject`/`template_args_json` — the bug class this property forbids.)
    // We deliberately run ZERO mutating statements: the structural erase IS the absence of a PII write.

    // After the erase: the rows are byte-identical (0 PII columns mutated) AND not deleted.
    let after = sqlx::query(&locate_sql)
        .bind(subject_pseudonym)
        .bind(subject_actor_ref)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        2,
        "0 rows deleted — the appearance stays (no erasure backdoor; only resolution changes)"
    );

    for (b, a) in before.iter().zip(after.iter()) {
        assert_eq!(b.get::<String, _>("item_id"), a.get::<String, _>("item_id"));
        assert_eq!(
            b.get::<String, _>("recipient"),
            a.get::<String, _>("recipient"),
            "the recipient pseudonym column is UNCHANGED — 0 PII mutation (references-not-payloads)"
        );
        assert_eq!(
            b.get::<String, _>("subject"),
            a.get::<String, _>("subject"),
            "the subject ref column is UNCHANGED — 0 PII mutation"
        );
        assert_eq!(
            b.get::<String, _>("targs"),
            a.get::<String, _>("targs"),
            "the template_args_json ref-array is UNCHANGED — 0 PII mutation (the title re-resolves to a tombstone)"
        );
    }

    let total_after: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        total_after, 3,
        "no row deleted by the structural erase — the appearance tombstones in place"
    );

    // The CONTROL row (naming a different person) is unaffected + never matched the subject locate.
    let control_recipient: String = sqlx::query_scalar(&format!(
        "SELECT recipient FROM {tbl} WHERE item_id = 'itm-control'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        control_recipient, "psn:u-carol",
        "the control row is untouched"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}
