#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OltpConfig {
    pub max_pool_size: u32,
    pub statement_timeout_ms: u64,
    pub per_tenant_in_flight_cap: u32,
}

impl OltpConfig {
    pub fn validate(&self) -> Result<(), OltpError> {
        if self.max_pool_size == 0 {
            return Err(OltpError::InvalidConfig(
                "max_pool_size must be >= 1 (no unbounded pool)",
            ));
        }
        if self.statement_timeout_ms == 0 {
            return Err(OltpError::InvalidConfig(
                "statement_timeout_ms must be >= 1",
            ));
        }
        if self.per_tenant_in_flight_cap == 0 {
            return Err(OltpError::InvalidConfig(
                "per_tenant_in_flight_cap must be >= 1",
            ));
        }
        if self.per_tenant_in_flight_cap > self.max_pool_size {
            return Err(OltpError::InvalidConfig(
                "per_tenant_in_flight_cap must be <= max_pool_size",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OltpError {
    InvalidConfig(&'static str),
    PoolSaturated,
    TenantInFlightCapExceeded,
}

impl core::fmt::Display for OltpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            OltpError::InvalidConfig(why) => write!(f, "invalid OLTP pool config: {why}"),
            OltpError::PoolSaturated => write!(
                f,
                "OLTP pool saturated - request rejected (fast-fail, not blocked; storage §1.1)"
            ),
            OltpError::TenantInFlightCapExceeded => write!(
                f,
                "per-tenant in-flight cap exceeded - request rejected (noisy-neighbour bound)"
            ),
        }
    }
}

impl std::error::Error for OltpError {}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_tenancy::TenantId;

#[derive(Clone, Debug)]
pub struct OltpPool {
    config: OltpConfig,
    state: Arc<Mutex<PoolState>>,
}

#[derive(Debug, Default)]
struct PoolState {
    global_in_flight: u32,
    per_tenant_in_flight: HashMap<TenantId, u32>,
    saturation_rejections: u64,
}

impl OltpPool {
    pub fn open(config: OltpConfig) -> Result<OltpPool, OltpError> {
        config.validate()?;
        Ok(OltpPool {
            config,
            state: Arc::new(Mutex::new(PoolState::default())),
        })
    }

    pub fn config(&self) -> OltpConfig {
        self.config
    }

    pub fn acquire(&self, tenant: &TenantId) -> Result<PermitGuard, OltpError> {
        let mut state = self.state.lock().expect("OLTP pool mutex poisoned");
        let tenant_in_flight = state.per_tenant_in_flight.get(tenant).copied().unwrap_or(0);
        if tenant_in_flight >= self.config.per_tenant_in_flight_cap {
            state.saturation_rejections += 1;
            return Err(OltpError::TenantInFlightCapExceeded);
        }
        if state.global_in_flight >= self.config.max_pool_size {
            state.saturation_rejections += 1;
            return Err(OltpError::PoolSaturated);
        }
        state.global_in_flight += 1;
        *state
            .per_tenant_in_flight
            .entry(tenant.clone())
            .or_insert(0) += 1;
        Ok(PermitGuard {
            state: Arc::clone(&self.state),
            tenant: tenant.clone(),
            released: false,
        })
    }

    pub fn in_flight(&self) -> u32 {
        self.state
            .lock()
            .expect("OLTP pool mutex poisoned")
            .global_in_flight
    }

    pub fn saturation_rejections(&self) -> u64 {
        self.state
            .lock()
            .expect("OLTP pool mutex poisoned")
            .saturation_rejections
    }
}

#[derive(Debug)]
pub struct PermitGuard {
    state: Arc<Mutex<PoolState>>,
    tenant: TenantId,
    released: bool,
}

impl Drop for PermitGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut state = self.state.lock().expect("OLTP pool mutex poisoned");
        state.global_in_flight = state.global_in_flight.saturating_sub(1);
        if let Some(n) = state.per_tenant_in_flight.get_mut(&self.tenant) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                state.per_tenant_in_flight.remove(&self.tenant);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OltpConfig {
        OltpConfig {
            max_pool_size: 4,
            statement_timeout_ms: 5_000,
            per_tenant_in_flight_cap: 2,
        }
    }

    fn t(name: &str) -> TenantId {
        TenantId(name.into())
    }

    #[test]
    fn valid_config_opens() {
        assert_eq!(cfg().validate(), Ok(()));
        assert!(OltpPool::open(cfg()).is_ok());
    }

    #[test]
    fn zero_pool_size_is_rejected_at_boot() {
        let c = OltpConfig {
            max_pool_size: 0,
            ..cfg()
        };
        assert!(matches!(c.validate(), Err(OltpError::InvalidConfig(_))));
        assert!(OltpPool::open(c).is_err());
    }

    #[test]
    fn zero_statement_timeout_is_rejected() {
        let c = OltpConfig {
            statement_timeout_ms: 0,
            ..cfg()
        };
        assert!(matches!(c.validate(), Err(OltpError::InvalidConfig(_))));
    }

    #[test]
    fn nonsensical_per_tenant_cap_is_rejected() {
        let zero = OltpConfig {
            per_tenant_in_flight_cap: 0,
            ..cfg()
        };
        assert!(matches!(zero.validate(), Err(OltpError::InvalidConfig(_))));
        let too_big = OltpConfig {
            per_tenant_in_flight_cap: 99,
            ..cfg()
        };
        assert!(matches!(
            too_big.validate(),
            Err(OltpError::InvalidConfig(_))
        ));
    }

    #[test]
    fn acquire_and_release_accounts_permits() {
        let pool = OltpPool::open(cfg()).unwrap();
        assert_eq!(pool.in_flight(), 0);
        {
            let _g = pool.acquire(&t("acme")).unwrap();
            assert_eq!(pool.in_flight(), 1);
        }
        assert_eq!(
            pool.in_flight(),
            0,
            "dropping the guard releases the permit"
        );
    }

    #[test]
    fn global_saturation_fast_fails_and_signals() {
        let pool = OltpPool::open(cfg()).unwrap();
        let _a1 = pool.acquire(&t("acme")).unwrap();
        let _a2 = pool.acquire(&t("acme")).unwrap();
        let _b1 = pool.acquire(&t("beta")).unwrap();
        let _b2 = pool.acquire(&t("beta")).unwrap();
        assert_eq!(pool.in_flight(), 4);
        let rejected = pool.acquire(&t("gamma"));
        assert!(matches!(rejected, Err(OltpError::PoolSaturated)));
        assert_eq!(
            pool.saturation_rejections(),
            1,
            "the PoolSaturation signal fired"
        );
    }

    #[test]
    fn per_tenant_cap_fast_fails_before_global_bound() {
        let pool = OltpPool::open(cfg()).unwrap();
        let _a1 = pool.acquire(&t("acme")).unwrap();
        let _a2 = pool.acquire(&t("acme")).unwrap();
        let rejected = pool.acquire(&t("acme"));
        assert!(matches!(
            rejected,
            Err(OltpError::TenantInFlightCapExceeded)
        ));
        let _b1 = pool.acquire(&t("beta")).unwrap();
        assert_eq!(pool.in_flight(), 3);
    }

    #[test]
    fn releasing_frees_capacity() {
        let pool = OltpPool::open(cfg()).unwrap();
        let a1 = pool.acquire(&t("acme")).unwrap();
        let _a2 = pool.acquire(&t("acme")).unwrap();
        assert!(pool.acquire(&t("acme")).is_err());
        drop(a1);
        let _a3 = pool.acquire(&t("acme")).unwrap();
        assert_eq!(pool.in_flight(), 2);
    }
}
