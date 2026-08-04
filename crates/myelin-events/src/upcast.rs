use std::collections::HashMap;

use crate::{EventEnvelope, EventType, Reason};

type Upcaster = Box<dyn Fn(EventEnvelope) -> EventEnvelope + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    NotAdjacentForwardHop {
        type_: EventType,
        from: u32,
        to: u32,
    },
    DuplicateHop { type_: EventType, from: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpcastError {
    UnbridgeableGap {
        type_: EventType,
        from_ver: u32,
        stuck_at: u32,
        target: u32,
    },
}

impl UpcastError {
    pub fn into_reason(self) -> Reason {
        match self {
            UpcastError::UnbridgeableGap { type_, from_ver, stuck_at, target } => Reason(format!(
                "unbridgeable schema gap: {} stuck at schema_ver {} (from {}), no upcaster to reach current {}",
                type_.0, stuck_at, from_ver, target
            )),
        }
    }
}

#[derive(Default)]
pub struct UpcasterRegistry {
    hops: HashMap<EventType, HashMap<u32, Upcaster>>,
}

impl UpcasterRegistry {
    pub fn new() -> Self {
        UpcasterRegistry {
            hops: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        type_: EventType,
        from: u32,
        to: u32,
        f: impl Fn(EventEnvelope) -> EventEnvelope + Send + Sync + 'static,
    ) -> Result<(), RegisterError> {
        if to != from + 1 {
            return Err(RegisterError::NotAdjacentForwardHop { type_, from, to });
        }
        let per_type = self.hops.entry(type_.clone()).or_default();
        if per_type.contains_key(&from) {
            return Err(RegisterError::DuplicateHop { type_, from });
        }
        per_type.insert(from, Box::new(f));
        Ok(())
    }

    pub fn current_ver(&self, type_: &EventType) -> u32 {
        match self.hops.get(type_) {
            Some(per_type) if !per_type.is_empty() => {
                per_type.keys().copied().max().map(|m| m + 1).unwrap_or(1)
            }
            _ => 1,
        }
    }

    pub fn upcast(&self, mut env: EventEnvelope) -> Result<EventEnvelope, UpcastError> {
        let target = self.current_ver(&env.type_);
        if env.schema_ver >= target {
            return Ok(env);
        }
        let Some(per_type) = self.hops.get(&env.type_) else {
            return Err(UpcastError::UnbridgeableGap {
                type_: env.type_.clone(),
                from_ver: env.schema_ver,
                stuck_at: env.schema_ver,
                target,
            });
        };
        let from_ver = env.schema_ver;
        while env.schema_ver < target {
            let Some(hop) = per_type.get(&env.schema_ver) else {
                return Err(UpcastError::UnbridgeableGap {
                    type_: env.type_.clone(),
                    from_ver,
                    stuck_at: env.schema_ver,
                    target,
                });
            };
            let before = env.schema_ver;
            env = hop(env);
            if env.schema_ver != before + 1 {
                return Err(UpcastError::UnbridgeableGap {
                    type_: env.type_.clone(),
                    from_ver,
                    stuck_at: before,
                    target,
                });
            }
        }
        Ok(env)
    }

    pub fn into_hook(
        self,
    ) -> impl Fn(EventEnvelope) -> Result<EventEnvelope, Reason> + Send + Sync {
        move |env| self.upcast(env).map_err(UpcastError::into_reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventId, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn env(type_: &str, schema_ver: u32) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType(type_.into()),
            schema_ver,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
            payload: serde_json::json!({ "title": "old" }),
        }
    }

    fn registry_v1_to_v3() -> UpcasterRegistry {
        let mut r = UpcasterRegistry::new();
        r.register(EventType("issues.issue.created".into()), 1, 2, |mut e| {
            e.schema_ver = 2;
            if let serde_json::Value::Object(m) = &mut e.payload {
                m.insert("priority".into(), serde_json::json!("normal"));
            }
            e
        })
        .expect("v1->v2 is an adjacent forward hop");
        r.register(EventType("issues.issue.created".into()), 2, 3, |mut e| {
            e.schema_ver = 3;
            if let serde_json::Value::Object(m) = &mut e.payload {
                m.insert("severity".into(), serde_json::json!("info"));
            }
            e
        })
        .expect("v2->v3 is an adjacent forward hop");
        r
    }

    #[test]
    fn old_event_is_upcast_to_current_through_the_chain() {
        let r = registry_v1_to_v3();
        assert_eq!(r.current_ver(&EventType("issues.issue.created".into())), 3);

        let out = r
            .upcast(env("issues.issue.created", 1))
            .expect("v1 bridges to v3");
        assert_eq!(
            out.schema_ver, 3,
            "the chain lifted v1 all the way to the current v3"
        );
        let p = out.payload.as_object().unwrap();
        assert_eq!(
            p.get("priority").unwrap(),
            "normal",
            "v1->v2 added priority"
        );
        assert_eq!(p.get("severity").unwrap(), "info", "v2->v3 added severity");
        assert_eq!(
            p.get("title").unwrap(),
            "old",
            "the original field is preserved"
        );
    }

    #[test]
    fn mid_chain_event_is_bridged_the_rest_of_the_way() {
        let r = registry_v1_to_v3();
        let out = r
            .upcast(env("issues.issue.created", 2))
            .expect("v2 bridges to v3");
        assert_eq!(out.schema_ver, 3);
        assert!(out.payload.as_object().unwrap().contains_key("severity"));
    }

    #[test]
    fn current_version_event_passes_through_unchanged() {
        let r = registry_v1_to_v3();
        let input = env("issues.issue.created", 3);
        let out = r.upcast(input.clone()).expect("already current");
        assert_eq!(out, input, "a current-version event is the identity case");
    }

    #[test]
    fn newer_event_is_not_downcast() {
        let r = registry_v1_to_v3();
        let input = env("issues.issue.created", 4);
        let out = r
            .upcast(input.clone())
            .expect("forward-compatible newer event");
        assert_eq!(
            out.schema_ver, 4,
            "no down-cast - the newer event is read as-is"
        );
        assert_eq!(out, input);
    }

    #[test]
    fn missing_hop_is_a_loud_unbridgeable_gap_never_a_silent_pass() {
        let mut r = UpcasterRegistry::new();
        r.register(EventType("issues.issue.created".into()), 2, 3, |mut e| {
            e.schema_ver = 3;
            e
        })
        .unwrap();
        assert_eq!(r.current_ver(&EventType("issues.issue.created".into())), 3);

        let err = r
            .upcast(env("issues.issue.created", 1))
            .expect_err("v1 has no upcaster - loud gap");
        match err {
            UpcastError::UnbridgeableGap {
                from_ver,
                stuck_at,
                target,
                ..
            } => {
                assert_eq!(from_ver, 1);
                assert_eq!(stuck_at, 1, "stuck at v1 - there is no v1->v2 hop");
                assert_eq!(target, 3);
            }
        }
    }

    #[test]
    fn upcasters_are_pure_deterministic_no_side_effects() {
        let r = registry_v1_to_v3();
        let input = env("issues.issue.created", 1);
        let a = r.upcast(input.clone()).unwrap();
        let b = r.upcast(input.clone()).unwrap();
        assert_eq!(
            a, b,
            "same input -> same output (deterministic, no hidden state)"
        );
        let c = r.upcast(input.clone()).unwrap();
        assert_eq!(a, c);
    }

    #[test]
    fn non_adjacent_or_backward_hops_are_rejected_loudly() {
        let mut r = UpcasterRegistry::new();
        let t = EventType("issues.issue.created".into());
        assert!(matches!(
            r.register(t.clone(), 3, 2, |e| e),
            Err(RegisterError::NotAdjacentForwardHop { from: 3, to: 2, .. })
        ));
        assert!(matches!(
            r.register(t.clone(), 1, 3, |e| e),
            Err(RegisterError::NotAdjacentForwardHop { from: 1, to: 3, .. })
        ));
        assert!(matches!(
            r.register(t.clone(), 1, 1, |e| e),
            Err(RegisterError::NotAdjacentForwardHop { from: 1, to: 1, .. })
        ));
    }

    #[test]
    fn duplicate_hop_for_same_type_and_version_is_rejected() {
        let mut r = UpcasterRegistry::new();
        let t = EventType("issues.issue.created".into());
        r.register(t.clone(), 1, 2, |e| e).unwrap();
        assert!(matches!(
            r.register(t.clone(), 1, 2, |e| e),
            Err(RegisterError::DuplicateHop { from: 1, .. })
        ));
    }

    #[test]
    fn empty_registry_is_the_identity_map() {
        let r = UpcasterRegistry::new();
        let input = env("anything.at.all", 1);
        assert_eq!(r.upcast(input.clone()).unwrap(), input);
        assert_eq!(r.current_ver(&EventType("anything.at.all".into())), 1);
    }

    #[test]
    fn into_hook_surfaces_a_gap_as_a_reason() {
        let mut r = UpcasterRegistry::new();
        r.register(EventType("issues.issue.created".into()), 2, 3, |mut e| {
            e.schema_ver = 3;
            e
        })
        .unwrap();
        let hook = r.into_hook();
        assert!(hook(env("issues.issue.created", 2)).is_ok());
        let err = hook(env("issues.issue.created", 1)).expect_err("v1 has no hop");
        assert!(
            err.0.contains("unbridgeable schema gap"),
            "the reason names the gap: {}",
            err.0
        );
    }
}
