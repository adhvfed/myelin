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

    insert("sent", "del-1")
        .await
        .expect("the first effective delivery row commits");

    let dup = insert("sent", "del-2").await;
    assert!(
        dup.is_err(),
        "a retry with the SAME (tenant_id, idem_key) MUST be rejected by UNIQUE(tenant_id, idem_key) \
         - exactly one effective delivery under the real provider (NOTIF-D9 re-run, live)"
    );

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

}
