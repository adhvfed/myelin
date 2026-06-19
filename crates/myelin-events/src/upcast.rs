//! # `upcast` — the schema-evolution upcaster registry (P-S09, contract 2.8; forward-only)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.1 (`schema_ver` gates evolution; upcasters bridge versions at consume, forward-only) and
//! `event-bus.md` §4.10 (schema evolution / upcasting: `(type, from_ver) → to_ver` pure fns at
//! consume; expand→migrate→contract; an un-upcastable `schema_ver` is term'd to the DLQ, never
//! silently dropped; no rollback migrations). **Contract-index:** row 2.8.
//!
//! ## What this is (and why it exists)
//! An event is durable forever, but its shape evolves. A consumer written for the *current*
//! `schema_ver` must still be able to read an *older* event sitting in the log / replayed from
//! source. The discipline is **forward-only**: producers only ever ADD optional fields and bump
//! `schema_ver`; at CONSUME, a registered chain of pure `(type, from_ver) → to_ver` functions
//! bridges an old envelope up to the current shape BEFORE `handle` sees it. There is no
//! down-cast (a newer event is never rewritten backwards — that would be a rollback, which the
//! `forward-only-migration` lint, P-S10/P-S11, forbids structurally).
//!
//! ## The three rules this registry encodes (EI-01 §1/§2: name the floor, never silently drop)
//! 1. **Forward-only, one step at a time.** An upcaster is registered for ONE adjacent hop
//!    `(type, v) → (v+1)`. The registry composes the hops into a chain `v1 → v2 → v3 → … → cur`.
//!    A registered hop must advance the version by exactly one (a multi-step or backwards hop is
//!    a registration error, rejected loudly — [`UpcasterRegistry::register`]).
//! 2. **An unbridgeable gap is LOUD, never a silent pass.** If the chain cannot reach the
//!    current version for `(type, from_ver)` — a missing hop — [`UpcasterRegistry::upcast`]
//!    returns [`UpcastError::UnbridgeableGap`]. The consumer runtime turns that into a
//!    [`crate::HandleOutcome::NonRetryable`] → the message is dead-lettered (DLQ), NEVER silently
//!    dropped and NEVER passed to a handler at the wrong shape (silent corruption).
//! 3. **Pure functions only.** An upcaster is `Fn(EventEnvelope) -> EventEnvelope` with no side
//!    effects (no I/O, no clock, no emit) — it is a deterministic shape transform, so replaying
//!    the same old event always yields the same current shape (reindex-from-source == live).
//!
//! ## Already-current is the identity case
//! An event already at the registry's `current_ver(type)` (or above — a forward-compatible
//! newer event a tolerant consumer reads) passes through unchanged: there is nothing to bridge.
//! A consumer ignoring an unknown forward-added field is the producer-side half of the same
//! expand→contract discipline (the envelope `payload` is `serde_json::Value`, so an unknown key
//! is simply not read — no upcaster needed for a pure additive field).
//!
//! ## How the consumer plugs it in
//! [`UpcasterRegistry::into_hook`] yields the fallible pre-handle hook
//! [`crate::consumer::Consumer::with_upcaster`] installs: it runs BEFORE `handle`, so every
//! handler sees the current shape, and a gap becomes a loud dead-letter (rule 5) instead of a
//! corrupt read. Until a consumer installs a registry the hook is the identity map (no
//! evolution declared yet → every event is already current).
//!
//! ## Reconciliation note (EI-01 §7) — this is the SAME deliverable EB-10/P-046 reaches
//! The global run order interleaves the substrate + event-bus roadmaps; the upcaster registry is
//! reached from BOTH (P-S09 here, EB-10 / P-046 later). P-S09 ships the registry + the
//! forward-only chain + the un-upcastable→loud-`NonRetryable` rule + the consumer seam. EB-10
//! (P-046) is the Bus-system framing of the same row-2.8 deliverable and **reconciles in place**
//! against THIS module (the file it names — `upcast.rs` — is this file): it adds the
//! Bus-flavoured tests it owns (the `v1→v2→v3` chain at consume, the un-upcastable→DLQ assertion,
//! the unknown-forward-added-field tolerance) and the provider+consumer CDC pair for 2.8 — no
//! second registry, no type re-definition.

use std::collections::HashMap;

use crate::{EventEnvelope, EventType, Reason};

/// A pure `(type, from_ver) → to_ver` shape transform (contract 2.8). One registered upcaster
/// bridges ONE adjacent version hop `v → v+1` for ONE event type. It is `Fn(EventEnvelope) ->
/// EventEnvelope` (pure: no I/O, no clock, no emit) so a replay of the same old event always
/// yields the same current shape. Boxed so a heterogeneous set of hops lives in one registry.
type Upcaster = Box<dyn Fn(EventEnvelope) -> EventEnvelope + Send + Sync>;

/// Why a registration was rejected (loud, never silently ignored — EI-01 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// A hop must advance the version by EXACTLY one (`from + 1 == to`). A backwards hop
    /// (`to <= from`) is a rollback (forbidden, forward-only); a skipping hop (`to > from + 1`)
    /// hides a missing intermediate. Both are registration errors.
    NotAdjacentForwardHop { type_: EventType, from: u32, to: u32 },
    /// Two upcasters were registered for the same `(type, from)` hop. The chain must be
    /// unambiguous (one function per hop), so a duplicate is a programming error, rejected.
    DuplicateHop { type_: EventType, from: u32 },
}

/// Why an upcast could not reach the current shape (loud at consume — rule 2). The consumer
/// runtime maps this to [`crate::HandleOutcome::NonRetryable`] → DLQ, never a silent drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpcastError {
    /// No registered chain bridges `(type, from_ver)` up to the current version — a missing hop.
    /// The event is term'd (dead-lettered), NEVER passed to a handler at the wrong shape and
    /// NEVER silently dropped. Carries the version it got stuck at so the gap is diagnosable.
    UnbridgeableGap { type_: EventType, from_ver: u32, stuck_at: u32, target: u32 },
}

impl UpcastError {
    /// Render the gap as the [`Reason`] the consumer surfaces on the DLQ entry (so an operator
    /// sees exactly which `(type, version)` had no upcaster).
    pub fn into_reason(self) -> Reason {
        match self {
            UpcastError::UnbridgeableGap { type_, from_ver, stuck_at, target } => Reason(format!(
                "unbridgeable schema gap: {} stuck at schema_ver {} (from {}), no upcaster to reach current {}",
                type_.0, stuck_at, from_ver, target
            )),
        }
    }
}

/// The schema-evolution upcaster registry (contract 2.8) — the `(type, from_ver) → to_ver` pure
/// functions applied at consume, forward-only. Holds, per event type, the set of adjacent
/// `v → v+1` hops; [`Self::upcast`] composes the hops into a chain that lifts an old envelope to
/// the current version. A missing hop is a loud [`UpcastError::UnbridgeableGap`], never a silent
/// pass.
#[derive(Default)]
pub struct UpcasterRegistry {
    /// `type → (from_ver → hop)`. The current version of a `type` is `1 + max(from_ver)` over
    /// its registered hops (a type with no hops is current at v1 — nothing has evolved).
    hops: HashMap<EventType, HashMap<u32, Upcaster>>,
}

impl UpcasterRegistry {
    /// A registry with no upcasters: every event is already at its current shape (the identity
    /// case). A subsystem registers its evolution hops onto this.
    pub fn new() -> Self {
        UpcasterRegistry { hops: HashMap::new() }
    }

    /// Register the upcaster for ONE adjacent forward hop `(type, from) → (from + 1)`. Returns
    /// [`RegisterError`] (loud) if the hop is not an exactly-one-step forward advance, or if a
    /// hop is already registered for `(type, from)` (the chain must be unambiguous).
    ///
    /// `f` MUST be pure — a deterministic shape transform with no side effects (the registry
    /// cannot enforce purity structurally; it is a contract-2.8 obligation the CDC + unit tests
    /// and the `flow-determinism`/`no-raw-publish` lints guard around the call sites).
    pub fn register(
        &mut self,
        type_: EventType,
        from: u32,
        to: u32,
        f: impl Fn(EventEnvelope) -> EventEnvelope + Send + Sync + 'static,
    ) -> Result<(), RegisterError> {
        // Rule 1: forward-only, exactly one step. A backwards or skipping hop is rejected loudly.
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

    /// The current `schema_ver` the registry bridges `type` UP to: `1 + max(registered from)`
    /// (a type with no registered hop is current at v1 — nothing has evolved for it). This is
    /// the target [`Self::upcast`] drives an old envelope toward.
    pub fn current_ver(&self, type_: &EventType) -> u32 {
        match self.hops.get(type_) {
            Some(per_type) if !per_type.is_empty() => {
                // hops are `from → from+1`; the highest reachable version is max(from)+1.
                per_type.keys().copied().max().map(|m| m + 1).unwrap_or(1)
            }
            _ => 1,
        }
    }

    /// **Bridge `env` forward to the current shape (the consume-time transform, rule 1+2).**
    /// Applies the registered `from → from+1` chain repeatedly until `env.schema_ver` reaches
    /// [`Self::current_ver`]. Returns:
    /// - `Ok(env)` already at/above current — the identity case (a tolerant consumer reads a
    ///   forward-compatible newer event unchanged; a same-version event passes through).
    /// - `Ok(upcasted)` the chain lifted the old event to the current shape.
    /// - `Err(UnbridgeableGap)` a hop is missing — LOUD; the consumer dead-letters it (rule 2),
    ///   never silently drops it and never hands the wrong shape to a handler.
    pub fn upcast(&self, mut env: EventEnvelope) -> Result<EventEnvelope, UpcastError> {
        let target = self.current_ver(&env.type_);
        // Already at or above the current version: nothing to bridge (forward-only — we never
        // down-cast a newer event). A tolerant consumer reads the forward-added fields it knows.
        if env.schema_ver >= target {
            return Ok(env);
        }
        let Some(per_type) = self.hops.get(&env.type_) else {
            // The type has registered hops only if it appears here; absence at a below-target
            // version is itself an unbridgeable gap (target>1 implies hops exist — defensive).
            return Err(UpcastError::UnbridgeableGap {
                type_: env.type_.clone(),
                from_ver: env.schema_ver,
                stuck_at: env.schema_ver,
                target,
            });
        };
        let from_ver = env.schema_ver;
        // Walk the chain one adjacent hop at a time until we reach the target.
        while env.schema_ver < target {
            let Some(hop) = per_type.get(&env.schema_ver) else {
                // A missing hop in the middle of the chain → loud, never a silent pass (rule 2).
                return Err(UpcastError::UnbridgeableGap {
                    type_: env.type_.clone(),
                    from_ver,
                    stuck_at: env.schema_ver,
                    target,
                });
            };
            let before = env.schema_ver;
            env = hop(env);
            // A pure forward hop MUST advance the version by exactly one (the registration
            // invariant). Defensive: if a (buggy) hop body left the version unchanged we'd loop
            // forever — treat a non-advancing hop as an unbridgeable gap rather than spin.
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

    /// Yield the fallible pre-handle hook the consumer runtime installs
    /// ([`crate::consumer::Consumer::with_upcaster`]). It runs BEFORE `handle`: on success the
    /// handler sees the current shape; on a gap it returns the [`Reason`] the runtime turns into
    /// a [`crate::HandleOutcome::NonRetryable`] (DLQ), never a silent drop.
    pub fn into_hook(self) -> impl Fn(EventEnvelope) -> Result<EventEnvelope, Reason> + Send + Sync {
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
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))
    }

    /// An envelope of `type` at `schema_ver`, with an empty-object payload so an upcaster can add
    /// a forward field (the expand half of expand→migrate→contract).
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

    /// A registry with the `issues.issue.created` v1→v2→v3 chain: v1→v2 adds `priority`, v2→v3
    /// adds `severity`. Each hop is a PURE shape transform (bump `schema_ver`, add the field).
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

    /// Rule 1: an OLD event (v1) is bridged forward to the current shape (v3) at consume, through
    /// the registered chain — the handler would see v3 with both added fields.
    #[test]
    fn old_event_is_upcast_to_current_through_the_chain() {
        let r = registry_v1_to_v3();
        assert_eq!(r.current_ver(&EventType("issues.issue.created".into())), 3);

        let out = r.upcast(env("issues.issue.created", 1)).expect("v1 bridges to v3");
        assert_eq!(out.schema_ver, 3, "the chain lifted v1 all the way to the current v3");
        let p = out.payload.as_object().unwrap();
        assert_eq!(p.get("priority").unwrap(), "normal", "v1->v2 added priority");
        assert_eq!(p.get("severity").unwrap(), "info", "v2->v3 added severity");
        assert_eq!(p.get("title").unwrap(), "old", "the original field is preserved");
    }

    /// A mid-chain event (v2) is bridged the remaining hop(s) to current (v3).
    #[test]
    fn mid_chain_event_is_bridged_the_rest_of_the_way() {
        let r = registry_v1_to_v3();
        let out = r.upcast(env("issues.issue.created", 2)).expect("v2 bridges to v3");
        assert_eq!(out.schema_ver, 3);
        assert!(out.payload.as_object().unwrap().contains_key("severity"));
    }

    /// An event already at the current version is the identity case — passed through unchanged
    /// (nothing to bridge). Forward-only: we never down-cast.
    #[test]
    fn current_version_event_passes_through_unchanged() {
        let r = registry_v1_to_v3();
        let input = env("issues.issue.created", 3);
        let out = r.upcast(input.clone()).expect("already current");
        assert_eq!(out, input, "a current-version event is the identity case");
    }

    /// Forward-only: a NEWER event than this consumer's registry knows (v4 > current v3) is NOT
    /// down-cast — a tolerant consumer reads it unchanged (it ignores the forward-added fields it
    /// does not know). We never rewrite a newer event backwards (that would be a rollback).
    #[test]
    fn newer_event_is_not_downcast() {
        let r = registry_v1_to_v3();
        let input = env("issues.issue.created", 4);
        let out = r.upcast(input.clone()).expect("forward-compatible newer event");
        assert_eq!(out.schema_ver, 4, "no down-cast — the newer event is read as-is");
        assert_eq!(out, input);
    }

    /// Rule 2: a MISSING hop is a LOUD `UnbridgeableGap`, never a silent pass. Here only the
    /// v2→v3 hop is registered (current = v3), but the event arrives at v1 — there is no v1→v2,
    /// so v1 cannot be bridged. It is term'd, never handed to a handler at the wrong shape.
    #[test]
    fn missing_hop_is_a_loud_unbridgeable_gap_never_a_silent_pass() {
        let mut r = UpcasterRegistry::new();
        r.register(EventType("issues.issue.created".into()), 2, 3, |mut e| {
            e.schema_ver = 3;
            e
        })
        .unwrap();
        assert_eq!(r.current_ver(&EventType("issues.issue.created".into())), 3);

        let err = r.upcast(env("issues.issue.created", 1)).expect_err("v1 has no upcaster — loud gap");
        match err {
            UpcastError::UnbridgeableGap { from_ver, stuck_at, target, .. } => {
                assert_eq!(from_ver, 1);
                assert_eq!(stuck_at, 1, "stuck at v1 — there is no v1->v2 hop");
                assert_eq!(target, 3);
            }
        }
    }

    /// The upcasters are PURE: applying the chain twice to the same input yields byte-identical
    /// output, and the input is not mutated in place (we pass a clone). No side effects.
    #[test]
    fn upcasters_are_pure_deterministic_no_side_effects() {
        let r = registry_v1_to_v3();
        let input = env("issues.issue.created", 1);
        let a = r.upcast(input.clone()).unwrap();
        let b = r.upcast(input.clone()).unwrap();
        assert_eq!(a, b, "same input -> same output (deterministic, no hidden state)");
        // The registry holds no per-call mutable state, so a third call still matches.
        let c = r.upcast(input.clone()).unwrap();
        assert_eq!(a, c);
    }

    /// Registration rule: a hop must advance the version by EXACTLY one (forward-only, one step).
    /// A backwards hop (a rollback) and a skipping hop (a hidden missing intermediate) are both
    /// rejected loudly at registration, never silently accepted.
    #[test]
    fn non_adjacent_or_backward_hops_are_rejected_loudly() {
        let mut r = UpcasterRegistry::new();
        let t = EventType("issues.issue.created".into());
        // backwards (rollback): 3 -> 2
        assert!(matches!(
            r.register(t.clone(), 3, 2, |e| e),
            Err(RegisterError::NotAdjacentForwardHop { from: 3, to: 2, .. })
        ));
        // skipping: 1 -> 3 (hides the missing 1->2)
        assert!(matches!(
            r.register(t.clone(), 1, 3, |e| e),
            Err(RegisterError::NotAdjacentForwardHop { from: 1, to: 3, .. })
        ));
        // same version (no-op / not a forward advance): 1 -> 1
        assert!(matches!(
            r.register(t.clone(), 1, 1, |e| e),
            Err(RegisterError::NotAdjacentForwardHop { from: 1, to: 1, .. })
        ));
    }

    /// A duplicate hop for the same `(type, from)` is rejected — the chain must be unambiguous.
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

    /// An empty registry is the identity map: every type is current at v1 and an event passes
    /// through unchanged (no evolution declared — the default the consumer starts with).
    #[test]
    fn empty_registry_is_the_identity_map() {
        let r = UpcasterRegistry::new();
        let input = env("anything.at.all", 1);
        assert_eq!(r.upcast(input.clone()).unwrap(), input);
        assert_eq!(r.current_ver(&EventType("anything.at.all".into())), 1);
    }

    /// The hook the consumer installs surfaces a gap as a [`Reason`] (carrying the type +
    /// version), not a panic and not a silent pass — this is what becomes the DLQ entry.
    #[test]
    fn into_hook_surfaces_a_gap_as_a_reason() {
        let mut r = UpcasterRegistry::new();
        r.register(EventType("issues.issue.created".into()), 2, 3, |mut e| {
            e.schema_ver = 3;
            e
        })
        .unwrap();
        let hook = r.into_hook();
        // a bridgeable event succeeds
        assert!(hook(env("issues.issue.created", 2)).is_ok());
        // an unbridgeable one yields a Reason (the DLQ surface), never panics
        let err = hook(env("issues.issue.created", 1)).expect_err("v1 has no hop");
        assert!(err.0.contains("unbridgeable schema gap"), "the reason names the gap: {}", err.0);
    }
}
