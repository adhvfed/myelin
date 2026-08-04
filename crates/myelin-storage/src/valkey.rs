use std::time::Duration;

use fred::prelude::*;
use myelin_tenancy::TenantId;

use crate::cache::{Cache, CacheError};

#[derive(Clone)]
pub struct ValkeyCache {
    client: fred::clients::Client,
    rt: tokio::runtime::Handle,
}

impl ValkeyCache {
    pub fn connect(redis_url: &str, rt: tokio::runtime::Handle) -> Result<ValkeyCache, CacheError> {
        let config =
            Config::from_url(redis_url).map_err(|e| CacheError(format!("bad REDIS_URL: {e}")))?;
        let client = Builder::from_config(config)
            .build()
            .map_err(|e| CacheError(format!("build valkey client: {e}")))?;
        tokio::task::block_in_place(|| rt.block_on(client.init()))
            .map_err(|e| CacheError(format!("connect to valkey: {e}")))?;
        Ok(ValkeyCache { client, rt })
    }

    fn ns_key(tenant: &TenantId, key: &str) -> String {
        format!("{}:{}", tenant.0, key)
    }

    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl Cache for ValkeyCache {
    fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let k = Self::ns_key(tenant, key);
        let got: Option<Vec<u8>> = self
            .block(self.client.get(&k))
            .map_err(|e| CacheError(format!("valkey GET: {e}")))?;
        Ok(got)
    }

    fn set(
        &self,
        tenant: &TenantId,
        key: &str,
        value: &[u8],
        ttl: Duration,
    ) -> Result<(), CacheError> {
        let k = Self::ns_key(tenant, key);
        let secs = ttl.as_secs().max(1) as i64;
        let _: () = self
            .block(
                self.client
                    .set(&k, value.to_vec(), Some(Expiration::EX(secs)), None, false),
            )
            .map_err(|e| CacheError(format!("valkey SET: {e}")))?;
        Ok(())
    }

    fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError> {
        let k = Self::ns_key(tenant, key);
        let _: i64 = self
            .block(self.client.del(&k))
            .map_err(|e| CacheError(format!("valkey DEL: {e}")))?;
        Ok(())
    }
}
