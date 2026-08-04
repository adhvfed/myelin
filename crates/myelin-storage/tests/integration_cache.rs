#![cfg(feature = "integration")]

use fred::prelude::*;
use myelin_config::MyelinConfig;

#[tokio::test]
async fn valkey_cache_roundtrip() {
    let cfg = MyelinConfig::dev();
    let client = Builder::from_config(Config::from_url(&cfg.redis_url).expect("parse REDIS_URL"))
        .build()
        .expect("build fred client");
    client
        .init()
        .await
        .expect("connect to Valkey (is the stack up?)");

    let suffix = std::process::id();
    let key = format!("tenantA:stage1-probe-{suffix}");

    let _: () = client
        .set(&key, "v", Some(Expiration::EX(60)), None, false)
        .await
        .expect("SET with TTL");

    let got: Option<String> = client.get(&key).await.expect("GET");
    assert_eq!(got.as_deref(), Some("v"));

    let ttl: i64 = client.ttl(&key).await.expect("TTL");
    assert!(ttl > 0 && ttl <= 60, "TTL must be set (got {ttl})");

    let _: () = client.del(&key).await.expect("DEL");
    let after: Option<String> = client.get(&key).await.expect("GET after DEL");
    assert_eq!(after, None, "deleted key must be a miss");

    let _ = client.quit().await;
}
