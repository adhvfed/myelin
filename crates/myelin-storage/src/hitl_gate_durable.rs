use crate::migration::{Migration, Migrations};
use crate::rls::TenantScope;
use myelin_identity::PrincipalKind;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;

pub const DEFAULT_HITL_GATE_TTL_SECS: i64 = 3600;

pub const AGENT_HITL_GATE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS agent_hitl_gate (
    tenant_id       text   NOT NULL,
    region          text   NOT NULL,
    gate_id         text   NOT NULL,
    run_id          text   NOT NULL,
    effect_id       text   NOT NULL,
    risk_summary    bytea,
    cost_estimate   bigint NOT NULL,
    approver_filter text[] NOT NULL,
    state           text   NOT NULL,
    card_ref        text,
    requested_by    text   NOT NULL,
    decided_by      text,
    PRIMARY KEY (tenant_id, region, gate_id)
);
ALTER TABLE agent_hitl_gate ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent_hitl_gate FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON agent_hitl_gate;
CREATE POLICY myelin_tenant_isolation ON agent_hitl_gate \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
              WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const AGENT_HITL_GATE_LIFETIME_MIGRATION: &str = "\
ALTER TABLE agent_hitl_gate
  ADD COLUMN IF NOT EXISTS opened_at_unix bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS decided_at_unix bigint,
  ADD COLUMN IF NOT EXISTS expires_at_unix bigint NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS approval_consumed_at_unix bigint;";

pub fn hitl_gate_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0054_agent_hitl_gate", AGENT_HITL_GATE_MIGRATION),
        Migration::plain(
            "0055_agent_hitl_gate_lifetime",
            AGENT_HITL_GATE_LIFETIME_MIGRATION,
        ),
    ])
}

pub fn opaque_gate_id() -> String {
    use aes_gcm::aead::rand_core::RngCore;
    use aes_gcm::aead::OsRng;
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut s = String::from("gate:");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Encodes an opaque gate ID as one ArtifactRef-safe identifier component.
pub fn gate_ref_token(gate_id: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = gate_id.as_bytes();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// Recovers a bounded opaque gate ID from an ArtifactRef identifier component.
pub fn gate_id_from_ref_token(encoded: &str) -> Option<String> {
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok().filter(|gate_id| {
        !gate_id.is_empty()
            && gate_id.len() <= 256
            && gate_id.bytes().all(|byte| byte.is_ascii_graphic())
    })
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateState {
    Waiting,
    Approved,
    Rejected,
    Expired,
}

impl GateState {
    pub fn as_str(self) -> &'static str {
        match self {
            GateState::Waiting => "waiting",
            GateState::Approved => "approved",
            GateState::Rejected => "rejected",
            GateState::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Result<GateState, InvalidGateState> {
        match s {
            "waiting" => Ok(GateState::Waiting),
            "approved" => Ok(GateState::Approved),
            "rejected" => Ok(GateState::Rejected),
            "expired" => Ok(GateState::Expired),
            _ => Err(InvalidGateState),
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, GateState::Waiting)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidGateState;

impl std::fmt::Display for InvalidGateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("agent_hitl_gate row has an invalid state")
    }
}

impl std::error::Error for InvalidGateState {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateRecord {
    pub gate_id: String,
    pub run_id: String,
    pub effect_id: String,
    pub risk_summary: Vec<u8>,
    pub cost_estimate: u64,
    pub approver_filter: Vec<String>,
    pub state: GateState,
    pub card_ref: Option<String>,
    pub requested_by: String,
    pub decided_by: Option<String>,
    pub opened_at_unix: i64,
    pub decided_at_unix: Option<i64>,
    pub expires_at_unix: i64,
    pub approval_consumed_at_unix: Option<i64>,
}

impl GateRecord {
    pub fn authorizes(&self, effect_id: &str, run_id: &str, requester: &str) -> bool {
        self.state == GateState::Approved
            && self.effect_id == effect_id
            && self.run_id == run_id
            && self.requested_by == requester
            && self.approval_consumed_at_unix.is_none()
            && matches!(self.decided_by.as_deref(), Some(d) if d != requester)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateOpenError {
    Duplicate,
    NotWaiting,
}

impl core::fmt::Display for GateOpenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateOpenError::Duplicate => {
                write!(f, "agent_hitl_gate row already exists for this gate_id")
            }
            GateOpenError::NotWaiting => write!(f, "a gate must open in the waiting state"),
        }
    }
}

impl std::error::Error for GateOpenError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecideError {
    NotFound,
    AlreadyDecided(GateState),
    NotEligible,
    SelfApproval,
    MachineApproverRefused,
    ApprovalWindowExpired,
}

impl core::fmt::Display for GateDecideError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateDecideError::NotFound => write!(f, "no such hitl gate in this tenant scope"),
            GateDecideError::AlreadyDecided(s) => {
                write!(f, "hitl gate is already terminal ({})", s.as_str())
            }
            GateDecideError::NotEligible => {
                write!(f, "approver is not in the gate's approver_filter")
            }
            GateDecideError::SelfApproval => write!(
                f,
                "the requesting principal cannot approve its own gate (distinct-approver rule)"
            ),
            GateDecideError::MachineApproverRefused => write!(
                f,
                "a non-human (machine/agent/service) principal cannot approve a HITL gate - the \
                 gate structurally requires a distinct HUMAN approver"
            ),
            GateDecideError::ApprovalWindowExpired => {
                write!(f, "the hitl gate approval window has expired")
            }
        }
    }
}

impl std::error::Error for GateDecideError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateConsumeError {
    NotFound,
    NotApproved,
    BindingMismatch,
    AlreadyConsumed,
    Expired,
}

impl core::fmt::Display for GateConsumeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateConsumeError::NotFound => write!(f, "no such hitl gate in this tenant scope"),
            GateConsumeError::NotApproved => write!(f, "hitl gate is not approved"),
            GateConsumeError::BindingMismatch => {
                write!(
                    f,
                    "hitl approval does not belong to this effect, run, and requester"
                )
            }
            GateConsumeError::AlreadyConsumed => write!(f, "hitl approval was already consumed"),
            GateConsumeError::Expired => write!(f, "hitl approval has expired"),
        }
    }
}

impl std::error::Error for GateConsumeError {}

pub struct HitlVerdictStore {
    backend: HitlVerdictBackend,
}

enum HitlVerdictBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(MemoryHitlGates),
    Durable(Box<DurableHitlGates>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for HitlVerdictStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HitlVerdictStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> HitlVerdictStore {
        HitlVerdictStore {
            backend: HitlVerdictBackend::Memory(MemoryHitlGates::default()),
        }
    }

    pub fn with_pg(provider: crate::provider::SubstrateProvider) -> HitlVerdictStore {
        HitlVerdictStore {
            backend: HitlVerdictBackend::Durable(Box::new(DurableHitlGates::new(provider))),
        }
    }

    pub fn open(&mut self, scope: &TenantScope, record: GateRecord) -> Result<(), GateOpenError> {
        if record.state != GateState::Waiting {
            return Err(GateOpenError::NotWaiting);
        }
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.open(scope, record),
            HitlVerdictBackend::Durable(d) => d.open(scope, record),
        }
    }

    pub fn approve(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        approver: &str,
        approver_kind: PrincipalKind,
    ) -> Result<(), GateDecideError> {
        self.approve_at(scope, gate_id, approver, approver_kind, system_unix_now())
    }

    pub fn approve_at(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        approver: &str,
        approver_kind: PrincipalKind,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let is_human = matches!(approver_kind, PrincipalKind::Human);
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.decide(
                scope,
                gate_id,
                GateState::Approved,
                Some(approver),
                is_human,
                decided_at_unix,
            ),
            HitlVerdictBackend::Durable(d) => d.decide(
                scope,
                gate_id,
                GateState::Approved,
                Some(approver),
                is_human,
                decided_at_unix,
            ),
        }
    }

    pub fn reject(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        decider: &str,
        decider_kind: PrincipalKind,
    ) -> Result<(), GateDecideError> {
        self.reject_at(scope, gate_id, decider, decider_kind, system_unix_now())
    }

    pub fn reject_at(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        decider: &str,
        decider_kind: PrincipalKind,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let is_human = matches!(decider_kind, PrincipalKind::Human);
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.decide(
                scope,
                gate_id,
                GateState::Rejected,
                Some(decider),
                is_human,
                decided_at_unix,
            ),
            HitlVerdictBackend::Durable(d) => d.decide(
                scope,
                gate_id,
                GateState::Rejected,
                Some(decider),
                is_human,
                decided_at_unix,
            ),
        }
    }

    pub fn expire(&mut self, scope: &TenantScope, gate_id: &str) -> Result<(), GateDecideError> {
        self.expire_at(scope, gate_id, system_unix_now())
    }

    pub fn expire_at(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.decide(
                scope,
                gate_id,
                GateState::Expired,
                None,
                false,
                decided_at_unix,
            ),
            HitlVerdictBackend::Durable(d) => d.decide(
                scope,
                gate_id,
                GateState::Expired,
                None,
                false,
                decided_at_unix,
            ),
        }
    }

    pub fn expire_due_for_effect(
        &mut self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
        now_unix: i64,
    ) -> Vec<GateRecord> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => {
                m.expire_due_for_effect(scope, run_id, requester, effect_id, now_unix)
            }
            HitlVerdictBackend::Durable(d) => {
                d.expire_due_for_effect(scope, run_id, requester, effect_id, now_unix)
            }
        }
    }

    pub fn expire_if_due(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Option<GateRecord> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.expire_if_due(scope, gate_id, now_unix),
            HitlVerdictBackend::Durable(d) => d.expire_if_due(scope, gate_id, now_unix),
        }
    }

    pub fn consume_approval(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        effect_id: &str,
        run_id: &str,
        requester: &str,
        now_unix: i64,
    ) -> Result<(), GateConsumeError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => {
                m.consume_approval(scope, gate_id, effect_id, run_id, requester, now_unix)
            }
            HitlVerdictBackend::Durable(d) => {
                d.consume_approval(scope, gate_id, effect_id, run_id, requester, now_unix)
            }
        }
    }

    pub fn fetch(&self, scope: &TenantScope, gate_id: &str) -> Option<GateRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.fetch(scope, gate_id),
            HitlVerdictBackend::Durable(d) => d.fetch(scope, gate_id),
        }
    }

    pub fn find_waiting(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.find_waiting(scope, run_id, requester, effect_id),
            HitlVerdictBackend::Durable(d) => d.find_waiting(scope, run_id, requester, effect_id),
        }
    }

    pub fn find_approved(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            HitlVerdictBackend::Memory(m) => m.find_approved(scope, run_id, requester, effect_id),
            HitlVerdictBackend::Durable(d) => d.find_approved(scope, run_id, requester, effect_id),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct MemoryHitlGates {
    rows: HashMap<(String, String, String), GateRecord>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryHitlGates {
    fn key(scope: &TenantScope, gate_id: &str) -> (String, String, String) {
        (
            scope.tenant().0.clone(),
            scope.region().0.clone(),
            gate_id.to_string(),
        )
    }

    fn open(&mut self, scope: &TenantScope, record: GateRecord) -> Result<(), GateOpenError> {
        let key = Self::key(scope, &record.gate_id);
        if self.rows.contains_key(&key) {
            return Err(GateOpenError::Duplicate);
        }
        self.rows.insert(key, record);
        Ok(())
    }

    fn decide(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        to: GateState,
        decider: Option<&str>,
        approver_is_human: bool,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let key = Self::key(scope, gate_id);
        let row = self.rows.get_mut(&key).ok_or(GateDecideError::NotFound)?;
        decide_rules(row, to, decider, approver_is_human, decided_at_unix)?;
        row.state = to;
        row.decided_by = decider.map(str::to_string);
        row.decided_at_unix = Some(decided_at_unix);
        Ok(())
    }

    fn expire_due_for_effect(
        &mut self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
        now_unix: i64,
    ) -> Vec<GateRecord> {
        let (tenant, region) = (scope.tenant().0.as_str(), scope.region().0.as_str());
        let mut expired = Vec::new();
        for ((row_tenant, row_region, _), row) in &mut self.rows {
            if row_tenant == tenant
                && row_region == region
                && matches!(row.state, GateState::Waiting | GateState::Approved)
                && row.approval_consumed_at_unix.is_none()
                && row.run_id == run_id
                && row.requested_by == requester
                && row.effect_id == effect_id
                && row.expires_at_unix <= now_unix
            {
                row.state = GateState::Expired;
                row.decided_at_unix = Some(now_unix);
                expired.push(row.clone());
            }
        }
        expired
    }

    fn expire_if_due(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Option<GateRecord> {
        let row = self.rows.get_mut(&Self::key(scope, gate_id))?;
        if matches!(row.state, GateState::Waiting | GateState::Approved)
            && row.approval_consumed_at_unix.is_none()
            && row.expires_at_unix <= now_unix
        {
            row.state = GateState::Expired;
            row.decided_at_unix = Some(now_unix);
            Some(row.clone())
        } else {
            None
        }
    }

    fn consume_approval(
        &mut self,
        scope: &TenantScope,
        gate_id: &str,
        effect_id: &str,
        run_id: &str,
        requester: &str,
        now_unix: i64,
    ) -> Result<(), GateConsumeError> {
        let row = self
            .rows
            .get_mut(&Self::key(scope, gate_id))
            .ok_or(GateConsumeError::NotFound)?;
        consume_rules(row, effect_id, run_id, requester, now_unix)?;
        row.approval_consumed_at_unix = Some(now_unix);
        Ok(())
    }

    fn fetch(&self, scope: &TenantScope, gate_id: &str) -> Option<GateRecord> {
        self.rows.get(&Self::key(scope, gate_id)).cloned()
    }

    fn find_waiting(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        let (t, rg) = (scope.tenant().0.as_str(), scope.region().0.as_str());
        self.rows
            .iter()
            .filter(|((kt, kr, _), _)| kt == t && kr == rg)
            .map(|(_, v)| v)
            .find(|v| {
                v.state == GateState::Waiting
                    && v.run_id == run_id
                    && v.requested_by == requester
                    && v.effect_id == effect_id
            })
            .cloned()
    }

    fn find_approved(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        let (tenant, region) = (scope.tenant().0.as_str(), scope.region().0.as_str());
        self.rows
            .iter()
            .filter(|((row_tenant, row_region, _), _)| row_tenant == tenant && row_region == region)
            .map(|(_, record)| record)
            .find(|record| {
                record.state == GateState::Approved
                    && record.approval_consumed_at_unix.is_none()
                    && record.run_id == run_id
                    && record.requested_by == requester
                    && record.effect_id == effect_id
            })
            .cloned()
    }
}

fn decide_rules(
    row: &GateRecord,
    to: GateState,
    decider: Option<&str>,
    approver_is_human: bool,
    decided_at_unix: i64,
) -> Result<(), GateDecideError> {
    if row.state.is_terminal() {
        return Err(GateDecideError::AlreadyDecided(row.state));
    }
    if matches!(to, GateState::Approved | GateState::Rejected) {
        if row.expires_at_unix <= decided_at_unix {
            return Err(GateDecideError::ApprovalWindowExpired);
        }
        let Some(approver) = decider else {
            return Err(GateDecideError::NotEligible);
        };
        if approver == row.requested_by {
            return Err(GateDecideError::SelfApproval);
        }
        if !approver_is_human {
            return Err(GateDecideError::MachineApproverRefused);
        }
        if !row.approver_filter.iter().any(|a| a == approver) {
            return Err(GateDecideError::NotEligible);
        }
    }
    Ok(())
}

fn consume_rules(
    row: &GateRecord,
    effect_id: &str,
    run_id: &str,
    requester: &str,
    now_unix: i64,
) -> Result<(), GateConsumeError> {
    if row.state != GateState::Approved {
        return Err(GateConsumeError::NotApproved);
    }
    if row.effect_id != effect_id || row.run_id != run_id || row.requested_by != requester {
        return Err(GateConsumeError::BindingMismatch);
    }
    if !matches!(row.decided_by.as_deref(), Some(decider) if decider != requester) {
        return Err(GateConsumeError::BindingMismatch);
    }
    if row.approval_consumed_at_unix.is_some() {
        return Err(GateConsumeError::AlreadyConsumed);
    }
    if row.expires_at_unix <= now_unix {
        return Err(GateConsumeError::Expired);
    }
    Ok(())
}

fn system_unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .expect("system clock must be after the Unix epoch")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateDecisionOutcome {
    pub record: GateRecord,
    pub changed: bool,
}

#[derive(Clone)]
pub struct DurableHitlGateBacking {
    provider: crate::provider::SubstrateProvider,
}

impl DurableHitlGateBacking {
    pub fn new(provider: crate::provider::SubstrateProvider) -> Self {
        Self { provider }
    }

    pub async fn fetch(
        &self,
        scope: &TenantScope,
        gate_id: &str,
    ) -> Result<Option<GateRecord>, crate::provider::ProviderError> {
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let gate_id = gate_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                                state, card_ref, requested_by, decided_by, opened_at_unix, \
                                decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                         FROM agent_hitl_gate \
                         WHERE tenant_id = $1 AND region = $2 AND gate_id = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&gate_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
                    row.map(|row| row_to_record(&gate_id, &row)).transpose()
                })
            })
            .await
    }

    pub async fn reject_waiting_for_runs_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
        run_ids: &[String],
        decider: &str,
    ) -> Result<Vec<GateRecord>, crate::pg::PgError> {
        if run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "UPDATE agent_hitl_gate \
                SET state = 'rejected', decided_by = $4, \
                    decided_at_unix = EXTRACT(EPOCH FROM clock_timestamp())::bigint \
              WHERE tenant_id = $1 AND region = $2 AND run_id = ANY($3) \
                AND state = 'waiting' \
          RETURNING gate_id, run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                    state, card_ref, requested_by, decided_by, opened_at_unix, \
                    decided_at_unix, expires_at_unix, approval_consumed_at_unix",
        )
        .bind(tenant)
        .bind(region)
        .bind(run_ids)
        .bind(decider)
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| {
            crate::pg::PgError::Query(format!(
                "reject approval gates for disabled automation: {error}"
            ))
        })?;
        let mut gates = rows
            .iter()
            .map(|row| {
                use sqlx::Row as _;
                let gate_id = row
                    .try_get::<String, _>("gate_id")
                    .map_err(hitl_row_decode)?;
                row_to_record(&gate_id, row)
            })
            .collect::<Result<Vec<_>, _>>()?;
        gates.sort_by(|left, right| left.gate_id.cmp(&right.gate_id));
        Ok(gates)
    }

    pub async fn decide(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        decision: GateState,
        decider: &str,
        decider_kind: PrincipalKind,
        decided_at_unix: i64,
    ) -> Result<Result<GateDecisionOutcome, GateDecideError>, crate::provider::ProviderError> {
        if !matches!(decision, GateState::Approved | GateState::Rejected) {
            return Ok(Err(GateDecideError::NotEligible));
        }
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let gate_id = gate_id.to_string();
        let decider = decider.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                                state, card_ref, requested_by, decided_by, opened_at_unix, \
                                decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                         FROM agent_hitl_gate \
                         WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 FOR UPDATE",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&gate_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
                    let Some(row) = row else {
                        return Ok(Err(GateDecideError::NotFound));
                    };
                    let mut record = row_to_record(&gate_id, &row)?;
                    if record.state == decision
                        && record.decided_by.as_deref() == Some(decider.as_str())
                    {
                        return Ok(Ok(GateDecisionOutcome {
                            record,
                            changed: false,
                        }));
                    }
                    if let Err(error) = decide_rules(
                        &record,
                        decision,
                        Some(&decider),
                        decider_kind == PrincipalKind::Human,
                        decided_at_unix,
                    ) {
                        return Ok(Err(error));
                    }
                    sqlx::query(
                        "UPDATE agent_hitl_gate \
                         SET state = $4, decided_by = $5, decided_at_unix = $6 \
                         WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 \
                           AND state = 'waiting'",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&gate_id)
                    .bind(decision.as_str())
                    .bind(&decider)
                    .bind(decided_at_unix)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
                    record.state = decision;
                    record.decided_by = Some(decider);
                    record.decided_at_unix = Some(decided_at_unix);
                    Ok(Ok(GateDecisionOutcome {
                        record,
                        changed: true,
                    }))
                })
            })
            .await
    }

    pub async fn expire_if_due(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Result<Result<GateDecisionOutcome, GateDecideError>, crate::provider::ProviderError> {
        let tenant = scope.tenant().0.clone();
        let region = scope.region().0.clone();
        let gate_id = gate_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                                state, card_ref, requested_by, decided_by, opened_at_unix, \
                                decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                         FROM agent_hitl_gate \
                         WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 FOR UPDATE",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&gate_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
                    let Some(row) = row else {
                        return Ok(Err(GateDecideError::NotFound));
                    };
                    let mut record = row_to_record(&gate_id, &row)?;
                    if record.state == GateState::Expired {
                        return Ok(Ok(GateDecisionOutcome {
                            record,
                            changed: false,
                        }));
                    }
                    if record.expires_at_unix > now_unix {
                        return Ok(Err(GateDecideError::ApprovalWindowExpired));
                    }
                    if !matches!(record.state, GateState::Waiting | GateState::Approved)
                        || record.approval_consumed_at_unix.is_some()
                    {
                        return Ok(Err(GateDecideError::AlreadyDecided(record.state)));
                    }
                    sqlx::query(
                        "UPDATE agent_hitl_gate \
                         SET state = 'expired', decided_at_unix = $4 \
                         WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 \
                           AND state IN ('waiting', 'approved') \
                           AND approval_consumed_at_unix IS NULL",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&gate_id)
                    .bind(now_unix)
                    .execute(&mut *conn)
                    .await
                    .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
                    record.state = GateState::Expired;
                    record.decided_at_unix = Some(now_unix);
                    Ok(Ok(GateDecisionOutcome {
                        record,
                        changed: true,
                    }))
                })
            })
            .await
    }
}

#[derive(Clone)]
pub struct DurableHitlGates {
    provider: crate::provider::SubstrateProvider,
    rt: tokio::runtime::Handle,
}

impl DurableHitlGates {
    pub fn new(provider: crate::provider::SubstrateProvider) -> DurableHitlGates {
        DurableHitlGates {
            provider,
            rt: tokio::runtime::Handle::current(),
        }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    fn block<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, crate::provider::ProviderError>>,
    ) -> T {
        tokio::task::block_in_place(|| self.rt.block_on(fut)).unwrap_or_else(|e| {
            panic!("FAIL-STATIC: durable hitl gate store fault (the gate row did not commit): {e}")
        })
    }

    fn open(&self, scope: &TenantScope, record: GateRecord) -> Result<(), GateOpenError> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        self.block(self.provider.with_tenant_tx(&scope.tenant().0, move |conn| {
            Box::pin(async move {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&record.gate_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                if exists {
                    return Ok(Err(GateOpenError::Duplicate));
                }
                sqlx::query(
                    "INSERT INTO agent_hitl_gate \
                       (tenant_id, region, gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                        approver_filter, state, card_ref, requested_by, decided_by, opened_at_unix, \
                        decided_at_unix, expires_at_unix, approval_consumed_at_unix) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NULL, $12, NULL, $13, NULL)",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&record.gate_id)
                .bind(&record.run_id)
                .bind(&record.effect_id)
                .bind(&record.risk_summary)
                .bind(record.cost_estimate as i64)
                .bind(&record.approver_filter)
                .bind(GateState::Waiting.as_str())
                .bind(&record.card_ref)
                .bind(&record.requested_by)
                .bind(record.opened_at_unix)
                .bind(record.expires_at_unix)
                .execute(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                Ok(Ok(()))
            })
        }))
    }

    fn decide(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        to: GateState,
        decider: Option<&str>,
        approver_is_human: bool,
        decided_at_unix: i64,
    ) -> Result<(), GateDecideError> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        let decider = decider.map(str::to_string);
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                    "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                            state, card_ref, requested_by, decided_by, opened_at_unix, \
                            decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 FOR UPDATE",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&gate_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        let Some(row) = row else {
                            return Ok(Err(GateDecideError::NotFound));
                        };
                        let record = row_to_record(&gate_id, &row)?;
                        if let Err(e) = decide_rules(
                            &record,
                            to,
                            decider.as_deref(),
                            approver_is_human,
                            decided_at_unix,
                        ) {
                            return Ok(Err(e));
                        }
                        sqlx::query(
                            "UPDATE agent_hitl_gate SET state = $4, decided_by = $5, \
                                                       decided_at_unix = $6 \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 AND state = 'waiting'",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&gate_id)
                        .bind(to.as_str())
                        .bind(&decider)
                        .bind(decided_at_unix)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        Ok(Ok(()))
                    })
                }),
        )
    }

    fn fetch(&self, scope: &TenantScope, gate_id: &str) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                    "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                            state, card_ref, requested_by, decided_by, opened_at_unix, \
                            decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&gate_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        row.map(|row| row_to_record(&gate_id, &row)).transpose()
                    })
                }),
        )
    }

    fn find_waiting(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let run_id = run_id.to_string();
        let requester = requester.to_string();
        let effect_id = effect_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "SELECT gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                            approver_filter, state, card_ref, requested_by, decided_by, \
                            opened_at_unix, decided_at_unix, expires_at_unix, \
                            approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND effect_id = $4 \
                       AND requested_by = $5 AND state = 'waiting' \
                     ORDER BY gate_id LIMIT 1",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&run_id)
                        .bind(&effect_id)
                        .bind(&requester)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        let Some(row) = row else {
                            return Ok(None);
                        };
                        use sqlx::Row as _;
                        let gate_id: String = row.try_get("gate_id").map_err(hitl_row_decode)?;
                        row_to_record(&gate_id, &row).map(Some)
                    })
                }),
        )
    }

    fn find_approved(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
    ) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let run_id = run_id.to_string();
        let requester = requester.to_string();
        let effect_id = effect_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "SELECT gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                                    approver_filter, state, card_ref, requested_by, decided_by, \
                                    opened_at_unix, decided_at_unix, expires_at_unix, \
                                    approval_consumed_at_unix \
                             FROM agent_hitl_gate \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                               AND effect_id = $4 AND requested_by = $5 \
                               AND state = 'approved' \
                               AND approval_consumed_at_unix IS NULL \
                             ORDER BY gate_id LIMIT 1",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&run_id)
                        .bind(&effect_id)
                        .bind(&requester)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
                        let Some(row) = row else {
                            return Ok(None);
                        };
                        use sqlx::Row as _;
                        let gate_id: String = row.try_get("gate_id").map_err(hitl_row_decode)?;
                        row_to_record(&gate_id, &row).map(Some)
                    })
                }),
        )
    }

    fn expire_due_for_effect(
        &self,
        scope: &TenantScope,
        run_id: &str,
        requester: &str,
        effect_id: &str,
        now_unix: i64,
    ) -> Vec<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let run_id = run_id.to_string();
        let requester = requester.to_string();
        let effect_id = effect_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let rows = sqlx::query(
                            "UPDATE agent_hitl_gate \
                     SET state = 'expired', decided_at_unix = $6 \
                     WHERE tenant_id = $1 AND region = $2 \
                       AND state IN ('waiting', 'approved') \
                       AND approval_consumed_at_unix IS NULL \
                       AND run_id = $3 AND requested_by = $4 AND effect_id = $5 \
                       AND expires_at_unix <= $6 \
                     RETURNING gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                               approver_filter, state, card_ref, requested_by, decided_by, \
                               opened_at_unix, decided_at_unix, expires_at_unix, \
                               approval_consumed_at_unix",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&run_id)
                        .bind(&requester)
                        .bind(&effect_id)
                        .bind(now_unix)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        rows.iter()
                            .map(|row| {
                                use sqlx::Row as _;
                                let gate_id: String =
                                    row.try_get("gate_id").map_err(hitl_row_decode)?;
                                row_to_record(&gate_id, row)
                            })
                            .collect()
                    })
                }),
        )
    }

    fn expire_if_due(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        now_unix: i64,
    ) -> Option<GateRecord> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                            "UPDATE agent_hitl_gate \
                     SET state = 'expired', decided_at_unix = $4 \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 \
                       AND state IN ('waiting', 'approved') \
                       AND approval_consumed_at_unix IS NULL \
                       AND expires_at_unix <= $4 \
                     RETURNING gate_id, run_id, effect_id, risk_summary, cost_estimate, \
                               approver_filter, state, card_ref, requested_by, decided_by, \
                               opened_at_unix, decided_at_unix, expires_at_unix, \
                               approval_consumed_at_unix",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&gate_id)
                        .bind(now_unix)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        let Some(row) = row else {
                            return Ok(None);
                        };
                        use sqlx::Row as _;
                        let gate_id: String = row.try_get("gate_id").map_err(hitl_row_decode)?;
                        row_to_record(&gate_id, &row).map(Some)
                    })
                }),
        )
    }

    fn consume_approval(
        &self,
        scope: &TenantScope,
        gate_id: &str,
        effect_id: &str,
        run_id: &str,
        requester: &str,
        now_unix: i64,
    ) -> Result<(), GateConsumeError> {
        let region = self.region();
        let tenant = scope.tenant().0.clone();
        let gate_id = gate_id.to_string();
        let effect_id = effect_id.to_string();
        let run_id = run_id.to_string();
        let requester = requester.to_string();
        self.block(
            self.provider
                .with_tenant_tx(&scope.tenant().0, move |conn| {
                    Box::pin(async move {
                        let row = sqlx::query(
                    "SELECT run_id, effect_id, risk_summary, cost_estimate, approver_filter, \
                            state, card_ref, requested_by, decided_by, opened_at_unix, \
                            decided_at_unix, expires_at_unix, approval_consumed_at_unix \
                     FROM agent_hitl_gate \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 FOR UPDATE",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&gate_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        let Some(row) = row else {
                            return Ok(Err(GateConsumeError::NotFound));
                        };
                        let record = row_to_record(&gate_id, &row)?;
                        if let Err(error) =
                            consume_rules(&record, &effect_id, &run_id, &requester, now_unix)
                        {
                            return Ok(Err(error));
                        }
                        let updated = sqlx::query(
                            "UPDATE agent_hitl_gate SET approval_consumed_at_unix = $4 \
                     WHERE tenant_id = $1 AND region = $2 AND gate_id = $3 \
                       AND approval_consumed_at_unix IS NULL",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&gate_id)
                        .bind(now_unix)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                        if updated.rows_affected() != 1 {
                            return Ok(Err(GateConsumeError::AlreadyConsumed));
                        }
                        Ok(Ok(()))
                    })
                }),
        )
    }
}

fn row_to_record(
    gate_id: &str,
    row: &sqlx::postgres::PgRow,
) -> Result<GateRecord, crate::pg::PgError> {
    use sqlx::Row as _;
    let state_token: String = row.try_get("state").map_err(hitl_row_decode)?;
    let state = GateState::parse(&state_token)
        .map_err(|error| crate::pg::PgError::Query(error.to_string()))?;
    Ok(GateRecord {
        gate_id: gate_id.to_string(),
        run_id: row.try_get("run_id").map_err(hitl_row_decode)?,
        effect_id: row.try_get("effect_id").map_err(hitl_row_decode)?,
        risk_summary: row
            .try_get::<Option<Vec<u8>>, _>("risk_summary")
            .map_err(hitl_row_decode)?
            .unwrap_or_default(),
        cost_estimate: row
            .try_get::<i64, _>("cost_estimate")
            .map_err(hitl_row_decode)? as u64,
        approver_filter: row.try_get("approver_filter").map_err(hitl_row_decode)?,
        state,
        card_ref: row
            .try_get::<Option<String>, _>("card_ref")
            .map_err(hitl_row_decode)?,
        requested_by: row.try_get("requested_by").map_err(hitl_row_decode)?,
        decided_by: row
            .try_get::<Option<String>, _>("decided_by")
            .map_err(hitl_row_decode)?,
        opened_at_unix: row.try_get("opened_at_unix").map_err(hitl_row_decode)?,
        decided_at_unix: row
            .try_get::<Option<i64>, _>("decided_at_unix")
            .map_err(hitl_row_decode)?,
        expires_at_unix: row.try_get("expires_at_unix").map_err(hitl_row_decode)?,
        approval_consumed_at_unix: row
            .try_get::<Option<i64>, _>("approval_consumed_at_unix")
            .map_err(hitl_row_decode)?,
    })
}

fn hitl_row_decode(error: sqlx::Error) -> crate::pg::PgError {
    crate::pg::PgError::Query(format!("agent_hitl_gate row decode failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    #[test]
    fn gate_reference_tokens_round_trip_without_admitting_ambiguous_ids() {
        let gate_id = "gate:0123456789abcdef0123456789abcdef";
        let token = gate_ref_token(gate_id);
        assert_eq!(gate_id_from_ref_token(&token).as_deref(), Some(gate_id));
        assert_eq!(gate_id_from_ref_token("abc"), None);
        assert_eq!(gate_id_from_ref_token("zz"), None);
        assert_eq!(gate_id_from_ref_token("00"), None);
    }

    fn scope() -> TenantScope {
        let p = Principal::stub(
            PrincipalId("psn:human-x".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        TenantScope::from_verified_token(&p, Region("eu-west".into()))
    }

    fn waiting(gate_id: &str) -> GateRecord {
        GateRecord {
            gate_id: gate_id.into(),
            run_id: "mcp-run-1".into(),
            effect_id: "gate:git.merge:myelin://acme/git/pr/40".into(),
            risk_summary: vec![],
            cost_estimate: 50,
            approver_filter: vec!["psn:lead".into(), "psn:maintainer".into()],
            state: GateState::Waiting,
            card_ref: Some("card:R1:0".into()),
            requested_by: "agent:claude".into(),
            decided_by: None,
            opened_at_unix: 100,
            decided_at_unix: None,
            expires_at_unix: i64::MAX,
            approval_consumed_at_unix: None,
        }
    }

    fn expire_mcp_due(store: &mut HitlVerdictStore, now_unix: i64) -> Vec<GateRecord> {
        store.expire_due_for_effect(
            &scope(),
            "mcp-run-1",
            "agent:claude",
            "gate:git.merge:myelin://acme/git/pr/40",
            now_unix,
        )
    }

    #[test]
    fn unknown_durable_state_is_a_redacted_error_not_a_panic() {
        let error = GateState::parse("attacker-controlled-state")
            .expect_err("an unknown durable state must fail closed");
        assert_eq!(
            error.to_string(),
            "agent_hitl_gate row has an invalid state"
        );
        assert!(!error.to_string().contains("attacker-controlled-state"));
    }

    #[test]
    fn open_inserts_waiting_and_is_fetchable_by_gate_id() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:abc123")).expect("opens");
        let rec = s
            .fetch(&scope(), "gate:abc123")
            .expect("lookup-able by gate_id");
        assert_eq!(rec.state, GateState::Waiting);
        assert_eq!(rec.requested_by, "agent:claude");
        assert_eq!(
            s.open(&scope(), waiting("gate:abc123")),
            Err(GateOpenError::Duplicate),
            "a duplicate gate_id refuses"
        );
        assert_eq!(
            s.open(
                &scope(),
                GateRecord {
                    state: GateState::Approved,
                    ..waiting("gate:x")
                }
            ),
            Err(GateOpenError::NotWaiting),
            "a gate always opens undecided"
        );
    }

    #[test]
    fn a_made_up_gate_id_is_nothing() {
        let mut s = HitlVerdictStore::new();
        assert!(s.fetch(&scope(), "gate:forged").is_none());
        assert_eq!(
            s.approve(&scope(), "gate:forged", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::NotFound)
        );
    }

    #[test]
    fn approve_enforces_eligibility_and_distinct_principal() {
        let mut s = HitlVerdictStore::new();
        let mut rec = waiting("gate:abc");
        rec.approver_filter.push("agent:claude".into());
        s.open(&scope(), rec).unwrap();

        assert_eq!(
            s.approve(&scope(), "gate:abc", "agent:claude", PrincipalKind::Human),
            Err(GateDecideError::SelfApproval),
            "the requester can NEVER approve its own gate"
        );
        assert_eq!(
            s.approve(&scope(), "gate:abc", "psn:stranger", PrincipalKind::Human),
            Err(GateDecideError::NotEligible),
            "an out-of-filter principal cannot approve"
        );
        s.approve(&scope(), "gate:abc", "psn:lead", PrincipalKind::Human)
            .expect("an eligible distinct human approves");
        let rec = s.fetch(&scope(), "gate:abc").unwrap();
        assert_eq!(rec.state, GateState::Approved);
        assert_eq!(rec.decided_by.as_deref(), Some("psn:lead"));
    }

    #[test]
    fn approve_requires_a_human_principal_not_merely_a_distinct_one() {
        let machine_kinds = [
            PrincipalKind::Service,
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("rt-2".into()),
                on_behalf_of: None,
            },
        ];
        for kind in machine_kinds {
            let mut s = HitlVerdictStore::new();
            let mut rec = waiting("gate:m");
            rec.approver_filter.push("machine:ci-bot".into());
            s.open(&scope(), rec).unwrap();
            assert_eq!(
                s.approve(&scope(), "gate:m", "machine:ci-bot", kind),
                Err(GateDecideError::MachineApproverRefused),
                "a distinct, in-filter MACHINE approver is still refused (distinct-HUMAN rule)"
            );
            assert_eq!(
                s.fetch(&scope(), "gate:m").unwrap().state,
                GateState::Waiting
            );
        }

        let mut s = HitlVerdictStore::new();
        let mut rec = waiting("gate:m2");
        rec.approver_filter.push("machine:ci-bot".into());
        s.open(&scope(), rec).unwrap();
        s.approve(&scope(), "gate:m2", "psn:lead", PrincipalKind::Human)
            .expect("a human approver clears the gate");
        assert_eq!(
            s.fetch(&scope(), "gate:m2").unwrap().state,
            GateState::Approved
        );
    }

    #[test]
    fn reject_requires_the_same_distinct_eligible_human_as_approve() {
        let mut rec = waiting("gate:reject");
        rec.approver_filter.push("agent:claude".into());
        rec.approver_filter.push("machine:ci-bot".into());
        let mut store = HitlVerdictStore::new();
        store.open(&scope(), rec).unwrap();

        assert_eq!(
            store.reject(
                &scope(),
                "gate:reject",
                "agent:claude",
                PrincipalKind::Human,
            ),
            Err(GateDecideError::SelfApproval)
        );
        assert_eq!(
            store.reject(
                &scope(),
                "gate:reject",
                "psn:stranger",
                PrincipalKind::Human,
            ),
            Err(GateDecideError::NotEligible)
        );
        for kind in [
            PrincipalKind::Service,
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("rt:reject".into()),
                on_behalf_of: None,
            },
        ] {
            assert_eq!(
                store.reject(&scope(), "gate:reject", "machine:ci-bot", kind),
                Err(GateDecideError::MachineApproverRefused)
            );
        }
        assert_eq!(
            store.fetch(&scope(), "gate:reject").unwrap().state,
            GateState::Waiting,
            "every refused reject leaves the gate waiting"
        );
        store
            .reject(&scope(), "gate:reject", "psn:lead", PrincipalKind::Human)
            .expect("eligible distinct Human may reject");
        assert_eq!(
            store.fetch(&scope(), "gate:reject").unwrap().state,
            GateState::Rejected
        );
    }

    #[test]
    fn a_terminal_gate_refuses_re_decision() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:a")).unwrap();
        s.approve(&scope(), "gate:a", "psn:lead", PrincipalKind::Human)
            .unwrap();
        assert_eq!(
            s.approve(&scope(), "gate:a", "psn:maintainer", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Approved))
        );
        assert_eq!(
            s.reject(&scope(), "gate:a", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Approved))
        );

        let mut s2 = HitlVerdictStore::new();
        s2.open(&scope(), waiting("gate:b")).unwrap();
        s2.reject(&scope(), "gate:b", "psn:lead", PrincipalKind::Human)
            .unwrap();
        assert_eq!(
            s2.approve(&scope(), "gate:b", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Rejected)),
            "a rejected gate can never be flipped to approved"
        );
    }

    #[test]
    fn authorizes_is_per_effect_and_distinct_approver() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:a")).unwrap();

        let rec = s.fetch(&scope(), "gate:a").unwrap();
        assert!(!rec.authorizes(
            "gate:git.merge:myelin://acme/git/pr/40",
            "mcp-run-1",
            "agent:claude"
        ));

        s.approve(&scope(), "gate:a", "psn:lead", PrincipalKind::Human)
            .unwrap();
        let rec = s.fetch(&scope(), "gate:a").unwrap();
        assert!(
            rec.authorizes(
                "gate:git.merge:myelin://acme/git/pr/40",
                "mcp-run-1",
                "agent:claude"
            ),
            "approved + same effect/run/requester + distinct approver → authorized"
        );
        assert!(
            !rec.authorizes(
                "gate:git.merge:myelin://acme/git/pr/41",
                "mcp-run-1",
                "agent:claude"
            ),
            "an approval is bound to ITS effect - a sibling sharing the tool name is NOT authorized"
        );
        assert!(
            !rec.authorizes(
                "gate:git.merge:myelin://acme/git/pr/40",
                "mcp-run-1",
                "psn:lead"
            ),
            "the approver themselves re-driving is not a distinct-principal apply"
        );

        let mut s2 = HitlVerdictStore::new();
        s2.open(&scope(), waiting("gate:b")).unwrap();
        s2.reject(&scope(), "gate:b", "psn:lead", PrincipalKind::Human)
            .unwrap();
        let rec = s2.fetch(&scope(), "gate:b").unwrap();
        assert!(!rec.authorizes(
            "gate:git.merge:myelin://acme/git/pr/40",
            "mcp-run-1",
            "agent:claude"
        ));
    }

    #[test]
    fn approval_is_exact_run_requester_bound_and_consumed_once() {
        let effect = "gate:git.merge:myelin://acme/git/pr/40";
        for (run_id, requester) in [
            ("different-run", "agent:claude"),
            ("mcp-run-1", "agent:different"),
        ] {
            let mut store = HitlVerdictStore::new();
            store.open(&scope(), waiting("gate:bound")).unwrap();
            store
                .approve_at(
                    &scope(),
                    "gate:bound",
                    "psn:lead",
                    PrincipalKind::Human,
                    110,
                )
                .unwrap();
            assert_eq!(
                store.consume_approval(&scope(), "gate:bound", effect, run_id, requester, 120,),
                Err(GateConsumeError::BindingMismatch)
            );
        }

        let mut store = HitlVerdictStore::new();
        store.open(&scope(), waiting("gate:once")).unwrap();
        store
            .approve_at(&scope(), "gate:once", "psn:lead", PrincipalKind::Human, 110)
            .unwrap();
        store
            .consume_approval(
                &scope(),
                "gate:once",
                effect,
                "mcp-run-1",
                "agent:claude",
                120,
            )
            .expect("the exact originating run consumes once");
        assert_eq!(
            store.consume_approval(
                &scope(),
                "gate:once",
                effect,
                "mcp-run-1",
                "agent:claude",
                121,
            ),
            Err(GateConsumeError::AlreadyConsumed)
        );
        assert!(!store.fetch(&scope(), "gate:once").unwrap().authorizes(
            effect,
            "mcp-run-1",
            "agent:claude"
        ));
    }

    #[test]
    fn elapsed_waiting_gate_expires_with_durable_timestamps() {
        let mut record = waiting("gate:elapsed");
        record.expires_at_unix = 200;
        let mut store = HitlVerdictStore::new();
        store.open(&scope(), record).unwrap();
        let mut unrelated = waiting("gate:shared-agent-service");
        unrelated.run_id = "agent-service-run".into();
        unrelated.effect_id = "agent-service:v1:deploy:opaque".into();
        unrelated.expires_at_unix = 200;
        store.open(&scope(), unrelated).unwrap();
        assert_eq!(expire_mcp_due(&mut store, 199), Vec::new());
        let expired = expire_mcp_due(&mut store, 200);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, GateState::Expired);
        assert_eq!(expired[0].opened_at_unix, 100);
        assert_eq!(expired[0].decided_at_unix, Some(200));
        assert_eq!(
            store
                .fetch(&scope(), "gate:shared-agent-service")
                .unwrap()
                .state,
            GateState::Waiting,
            "exact MCP ownership selection must not mutate an unrelated due gate"
        );
        assert_eq!(expire_mcp_due(&mut store, 201), Vec::new());

        let mut targeted = waiting("gate:targeted");
        targeted.expires_at_unix = 400;
        store.open(&scope(), targeted).unwrap();
        assert_eq!(store.expire_if_due(&scope(), "gate:targeted", 399), None);
        let expired = store
            .expire_if_due(&scope(), "gate:targeted", 400)
            .expect("the operator path settles the exact due row");
        assert_eq!(expired.state, GateState::Expired);
        assert_eq!(expired.decided_at_unix, Some(400));
        assert_eq!(store.expire_if_due(&scope(), "gate:targeted", 401), None);
    }

    #[test]
    fn elapsed_unconsumed_approval_expires_but_consumed_evidence_does_not() {
        let effect = "gate:git.merge:myelin://acme/git/pr/40";
        let mut unconsumed = waiting("gate:unconsumed");
        unconsumed.expires_at_unix = 200;
        let mut store = HitlVerdictStore::new();
        store.open(&scope(), unconsumed).unwrap();
        store
            .approve_at(
                &scope(),
                "gate:unconsumed",
                "psn:lead",
                PrincipalKind::Human,
                110,
            )
            .unwrap();
        assert_eq!(expire_mcp_due(&mut store, 200)[0].state, GateState::Expired);

        let mut consumed = waiting("gate:consumed");
        consumed.expires_at_unix = 300;
        store.open(&scope(), consumed).unwrap();
        store
            .approve_at(
                &scope(),
                "gate:consumed",
                "psn:lead",
                PrincipalKind::Human,
                210,
            )
            .unwrap();
        store
            .consume_approval(
                &scope(),
                "gate:consumed",
                effect,
                "mcp-run-1",
                "agent:claude",
                220,
            )
            .unwrap();
        assert_eq!(expire_mcp_due(&mut store, 300), Vec::new());
        let row = store.fetch(&scope(), "gate:consumed").unwrap();
        assert_eq!(row.state, GateState::Approved);
        assert_eq!(row.approval_consumed_at_unix, Some(220));
    }

    #[test]
    fn expire_and_find_waiting() {
        let mut s = HitlVerdictStore::new();
        s.open(&scope(), waiting("gate:a")).unwrap();
        let found = s
            .find_waiting(
                &scope(),
                "mcp-run-1",
                "agent:claude",
                "gate:git.merge:myelin://acme/git/pr/40",
            )
            .expect("the pending gate is resurfaced (no duplicate spawn)");
        assert_eq!(found.gate_id, "gate:a");

        s.expire(&scope(), "gate:a").unwrap();
        let rec = s.fetch(&scope(), "gate:a").unwrap();
        assert_eq!(rec.state, GateState::Expired);
        assert_eq!(rec.decided_by, None, "an expiry has no decider");
        assert!(
            s.find_waiting(
                &scope(),
                "mcp-run-1",
                "agent:claude",
                "gate:git.merge:myelin://acme/git/pr/40"
            )
            .is_none(),
            "a decided gate is no longer waiting"
        );
        assert_eq!(
            s.approve(&scope(), "gate:a", "psn:lead", PrincipalKind::Human),
            Err(GateDecideError::AlreadyDecided(GateState::Expired)),
            "an expired gate is terminal (auto-deny holds)"
        );
    }

    #[test]
    fn migration_0054_carries_the_gate_shape_and_rls() {
        let m = hitl_gate_durable_migrations();
        assert_eq!(m.0.len(), 2);
        assert_eq!(m.0[0].id, "0054_agent_hitl_gate");
        assert_eq!(m.0[1].id, "0055_agent_hitl_gate_lifetime");
        for col in [
            "gate_id",
            "run_id",
            "effect_id",
            "risk_summary",
            "cost_estimate",
            "approver_filter",
            "state",
            "card_ref",
            "requested_by",
            "decided_by",
        ] {
            assert!(
                AGENT_HITL_GATE_MIGRATION.contains(col),
                "boot DDL carries `{col}`"
            );
        }
        assert!(AGENT_HITL_GATE_MIGRATION.contains("PRIMARY KEY (tenant_id, region, gate_id)"));
        assert!(AGENT_HITL_GATE_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(
            AGENT_HITL_GATE_MIGRATION.contains("risk_summary    bytea"),
            "the PII slot stays an encrypted byte carrier"
        );
        for column in [
            "opened_at_unix",
            "decided_at_unix",
            "expires_at_unix",
            "approval_consumed_at_unix",
        ] {
            assert!(AGENT_HITL_GATE_LIFETIME_MIGRATION.contains(column));
            assert!(!AGENT_HITL_GATE_MIGRATION.contains(column));
        }
    }
}
