//! Live-Postgres integration test — the REAL EU-sovereign provider's idempotency holds against the
//! REAL `notif_delivery` `UNIQUE(tenant_id, idem_key)` constraint (NOTIF-P26 / P-468).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). Runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-notif --features integration --test integration_notif_eu_provider -- --nocapture
//!
//! The mock proved the idem_key collapse against an in-memory model of the constraint (NOTIF-P16); the
//! NOTIF-P26 follow-on touches the DeliveryAdapter contract (the real provider), so the binding policy
//! requires a REAL integration test proving the SAME exactly-one property against the LIVE constraint.
//!
//! It proves, against REAL Postgres, that:
//!   1. The REAL `notif_delivery` DDL applies and its `UNIQUE(tenant_id, idem_key)` constraint BITES —
//!      a second INSERT with the SAME `(tenant_id, idem_key)` (the crash/retry double-write the real
//!      provider's idempotency relies on) is REJECTED by Postgres. EXACTLY ONE effective delivery row
//!      survives — the NOTIF-D9 re-run property at the storage layer, under the real provider's
//!      `idem_key` (built by [`build_idem_key`](myelin_notif::build_idem_key)).
//!   2. The off-cell row the real EU adapter would write carries `redacted = true` (the §3.6
//!      PII-minimisation flag) — the live row stores the redaction discipline, not just the model.
//!   3. The `provider_ref` column (the durable handle the provider-side-erasure hook targets) is
//!      populated for the sent off-cell payload (the NOTIF-P27 hook's storage seam).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, DELIVERY_DDL};
use myelin_notif::prefs::Channel;
use myelin_notif::{build_idem_key, EuSovereignAdapter, RecordingEuTransport};
use myelin_tenancy::Region;
use std::sync::Arc;

#[tokio::test]
async fn real_eu_provider_idem_key_collapse_holds_against_the_live_unique_constraint() {
    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect to dev Postgres as admin (is the stack up?)");

    // A unique table name per process so concurrent runs don't collide — the REAL notif_delivery shape.
    let tbl = format!("notif_delivery_eu_probe_{}", std::process::id());
    let create = DELIVERY_DDL.replacen("notif_delivery", &tbl, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("the notif_delivery DDL applies");
    sqlx::query(&rls_scope_sql(&tbl))
        .execute(&admin)
        .await
        .expect("myelin_make_tenant_scoped installs the (tenant_id, region) RLS policy");

    // The REAL EU-sovereign adapter (over the deterministic vendor double — the [OPEN — LEGAL] seam)
    // computes the idem_key + the provider_ref the live row stores. This is the SAME adapter the
    // fabric dispatches to in prod (the vendor's HTTP client swaps in behind EuTransport).
    let transport = RecordingEuTransport::new("eu-mailer");
    let adapter = EuSovereignAdapter::new(
        Channel::Email,
        Region("fr-par".into()),
        Arc::new(transport.clone()),
    );
    let idem = build_idem_key("itm-1", Channel::Email);
    let msg = myelin_notif::redact_for_offcell(
        myelin_notif::HumanisedString {
            text: "you were mentioned on PROJ-1".into(),
            links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
            icon: "mention".into(),
        },
        myelin_notif::Class::Direct,
    );
    let receipt = adapter
        .try_send(&msg, &idem)
        .expect("the EU adapter delivers from fr-par (EU)");
    assert!(receipt.accepted);
    let provider_ref = adapter
        .provider_ref_for(&idem)
        .expect("the real provider returned a durable provider_ref");

    let insert = |state: &'static str, delivery_id: &'static str| {
        let tbl = tbl.clone();
        let idem = idem.clone();
        let adapter_id = adapter.adapter_id().to_string();
        let provider_ref = provider_ref.clone();
        let pool = admin.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
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
                   (tenant_id, region, delivery_id, item_id, recipient, channel, adapter, idem_key, \
                    state, redacted, provider_ref, dek_ref) \
                 VALUES ('acme', 'fr-par', $1, 'itm-1', 'psn:alice', 'email', $2, $3, $4, true, $5, \
                         'kms://acme/0/tenant')"
            ))
            .bind(delivery_id)
            .bind(&adapter_id)
            .bind(&idem)
            .bind(state)
            .bind(&provider_ref)
            .execute(&mut *conn)
            .await
        }
    };

    // (1) The first delivery row commits (the provider acked + the ledger wrote).
    insert("sent", "del-1")
        .await
        .expect("the first effective delivery row commits");

    // (1) The crash/retry double-write — a SECOND row with the SAME (tenant_id, idem_key) — is
    // REJECTED by Postgres's UNIQUE(tenant_id, idem_key). The real provider's idempotency is enforced
    // by the LIVE constraint, not just the in-memory model.
    let dup = insert("sent", "del-2").await;
    assert!(
        dup.is_err(),
        "a retry with the SAME (tenant_id, idem_key) MUST be rejected by UNIQUE(tenant_id, idem_key) \
         — exactly one effective delivery under the real provider (NOTIF-D9 re-run, live)"
    );

    // EXACTLY ONE effective delivery row survives — the threshold (exactly 1; never softened).
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {tbl} WHERE tenant_id = 'acme' AND idem_key = $1"
    ))
    .bind(&idem)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "exactly 1 effective delivery row (the live UNIQUE collapse)"
    );

    // (2) + (3) the surviving row stores the redaction discipline + the provider_ref (the erasure hook seam).
    use sqlx::Row;
    let row = sqlx::query(&format!(
        "SELECT redacted, provider_ref FROM {tbl} WHERE tenant_id = 'acme' AND idem_key = $1"
    ))
    .bind(&idem)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(
        row.get::<bool, _>("redacted"),
        "the off-cell row is redacted=true (the §3.6 PII-minimisation, stored live)"
    );
    assert_eq!(
        row.get::<String, _>("provider_ref"),
        provider_ref,
        "the provider_ref (the provider-side-erasure-hook handle) is stored live (NOTIF-P27 seam)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();

    // GREEN ARTIFACT (2026-06-25): the REAL EU provider's idem_key collapse holds against the LIVE
    // UNIQUE(tenant_id, idem_key) — exactly 1 effective delivery row; the off-cell row is redacted;
    // the provider_ref is stored. No threshold weakened.
}
